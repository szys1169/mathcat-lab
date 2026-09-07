use std::{collections::BTreeSet, str::FromStr};

use chrono::{DateTime, Utc};
use research_domain::{
    AcceptanceClass, DomainEvent, FactAssurance, FactChallenge, FactGovernanceResult, FactImpact,
    FactStatus, Formalization,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, Transaction};

use crate::{
    SqliteStore, StorageError, StorageResult, append_event, bump_revision, entity, json_text,
    new_id, rows,
};

impl SqliteStore {
    pub async fn list_fact_assurances(&self, fact_id: &str) -> StorageResult<Vec<FactAssurance>> {
        let rows = sqlx::query(
            "SELECT * FROM fact_assurances WHERE fact_id=? ORDER BY created_at,assurance_id",
        )
        .bind(fact_id)
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(fact_assurance_from_row).collect()
    }

    pub async fn list_fact_challenges(&self, fact_id: &str) -> StorageResult<Vec<FactChallenge>> {
        let rows = sqlx::query(
            "SELECT * FROM fact_challenges WHERE fact_id=? ORDER BY created_at,challenge_id",
        )
        .bind(fact_id)
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(fact_challenge_from_row).collect()
    }

    pub async fn verification_governance_kind(
        &self,
        verification_id: &str,
    ) -> StorageResult<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT kind FROM fact_challenges WHERE verification_id=? AND status='open'",
        )
        .bind(verification_id)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn fact_dependency_closure(&self, fact_id: &str) -> StorageResult<Vec<Value>> {
        let root = self.get_fact(fact_id).await?;
        let mut pending = root.dependency_fact_ids.clone();
        let mut visited = BTreeSet::new();
        let mut closure = Vec::new();
        while let Some(dependency_id) = pending.pop() {
            if !visited.insert(dependency_id.clone()) {
                continue;
            }
            let fact = self.get_fact(&dependency_id).await?;
            let assurance = self
                .list_fact_assurances(&dependency_id)
                .await?
                .into_iter()
                .rev()
                .find(|item| item.status == "active");
            pending.extend(fact.dependency_fact_ids.iter().cloned());
            closure.push(json!({
                "fact_id": fact.fact_id,
                "status": fact.status,
                "content_hash": fact.content_hash,
                "acceptance_class": assurance.map(|item| item.acceptance_class),
                "dependency_fact_ids": fact.dependency_fact_ids,
            }));
        }
        Ok(closure)
    }

    pub async fn fact_verification_history(&self, fact_id: &str) -> StorageResult<Value> {
        let fact = self.get_fact(fact_id).await?;
        let mut verifications = Vec::new();
        for verification_id in &fact.verification_ids {
            verifications.push(serde_json::to_value(
                self.get_verification(verification_id).await?,
            )?);
        }
        Ok(json!({
            "fact_id": fact_id,
            "verifications": verifications,
            "assurances": self.list_fact_assurances(fact_id).await?,
            "challenges": self.list_fact_challenges(fact_id).await?,
        }))
    }

    pub async fn get_fact_formalization(&self, fact_id: &str) -> StorageResult<Formalization> {
        let fact = self.get_fact(fact_id).await?;
        for verification_id in fact.verification_ids.iter().rev() {
            let case_id: Option<String> = sqlx::query_scalar(
                "SELECT case_id FROM verification_cases WHERE verification_id=?",
            )
            .bind(verification_id)
            .fetch_optional(self.pool())
            .await?;
            if let Some(case_id) = case_id
                && let Ok(formalization) = self.get_formalization(&case_id).await
            {
                return Ok(formalization);
            }
        }
        Err(StorageError::NotFound {
            kind: "formalization",
            id: fact_id.into(),
        })
    }

    pub async fn fact_impact_detail(&self, fact_id: &str) -> StorageResult<FactImpact> {
        let fact = self.get_fact(fact_id).await?;
        compute_fact_impact(self.pool(), &fact.project_id, fact_id).await
    }

    pub async fn govern_fact(
        &self,
        fact_id: &str,
        action: &str,
        reason: &str,
        requested_by: &str,
        idempotency_key: &str,
    ) -> StorageResult<(FactGovernanceResult, Vec<DomainEvent>)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "govern_fact",
            )
            .await?;
        if !matches!(
            action,
            "challenge"
                | "reverify"
                | "request_formalization"
                | "request_independent_proof"
                | "suspend"
                | "revoke"
        ) {
            return Err(StorageError::InvalidTransition(format!(
                "unsupported fact governance action {action}"
            )));
        }
        if reason.trim().is_empty() || requested_by.trim().is_empty() {
            return Err(StorageError::InvalidTransition(
                "fact governance requires a reason and actor".into(),
            ));
        }
        let opens_challenge = matches!(
            action,
            "challenge" | "reverify" | "request_formalization" | "request_independent_proof"
        );
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query(
            "SELECT f.*,p.status AS project_status FROM facts f JOIN projects p ON p.project_id=f.project_id WHERE f.fact_id=?",
        )
            .bind(fact_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "fact",
                id: fact_id.into(),
            })?;
        let fact = rows::fact(&row)?;
        let project_status: String = row.try_get("project_status")?;
        let canonical = json!({
            "fact_id": fact_id,
            "action": action,
            "reason": reason,
            "requested_by": requested_by,
        });
        let request_hash = hex::encode(Sha256::digest(serde_json::to_vec(&canonical)?));
        if let Some(existing) = sqlx::query(
            "SELECT request_hash,result_json FROM fact_governance_commands WHERE project_id=? AND idempotency_key=?",
        )
        .bind(&fact.project_id)
        .bind(idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            let existing_hash: String = existing.try_get("request_hash")?;
            if existing_hash != request_hash {
                return Err(StorageError::InvalidTransition(format!(
                    "idempotency key {idempotency_key} was already used with a different request"
                )));
            }
            let result: String = existing.try_get("result_json")?;
            tx.rollback().await?;
            return Ok((serde_json::from_str(&result)?, Vec::new()));
        }
        if opens_challenge && matches!(project_status.as_str(), "stopped_by_human" | "error") {
            return Err(StorageError::InvalidTransition(format!(
                "cannot open a Fact governance review while project {} is {project_status}",
                fact.project_id
            )));
        }
        if opens_challenge {
            let active_challenge_id: Option<String> = sqlx::query_scalar(
                "SELECT challenge_id FROM fact_challenges WHERE project_id=? AND fact_id=? AND status='open' ORDER BY created_at,challenge_id LIMIT 1",
            )
            .bind(&fact.project_id)
            .bind(fact_id)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(active_challenge_id) = active_challenge_id {
                return Err(StorageError::InvalidTransition(format!(
                    "fact {fact_id} already has active challenge {active_challenge_id}"
                )));
            }
        }
        if fact.status == FactStatus::Revoked && action != "revoke" {
            return Err(StorageError::InvalidTransition(format!(
                "revoked fact {fact_id} is terminal and cannot transition through {action}"
            )));
        }

        let command_id = new_id("fgcmd");
        let now = Utc::now();
        sqlx::query("INSERT INTO fact_governance_commands(governance_command_id,project_id,fact_id,idempotency_key,action,request_hash,result_json,requested_by,reason,created_at,completed_at) VALUES(?,?,?,?,?,?,NULL,?,?,?,NULL)")
            .bind(&command_id).bind(&fact.project_id).bind(fact_id).bind(idempotency_key)
            .bind(action).bind(&request_hash).bind(requested_by).bind(reason)
            .bind(now.to_rfc3339()).execute(&mut *tx).await?;

        let impact;
        let mut challenge = None;
        let mut verification_id = None;
        let external_events;
        let event_type;
        if opens_challenge {
            let source_verification_id = fact.verification_ids.last().ok_or_else(|| {
                StorageError::CorruptData(format!("fact {fact_id} has no verification history"))
            })?;
            let candidate_id: String = sqlx::query_scalar(
                "SELECT candidate_id FROM verifications WHERE verification_id=?",
            )
            .bind(source_verification_id)
            .fetch_one(&mut *tx)
            .await?;
            let new_verification_id = new_id("verification");
            sqlx::query("INSERT INTO verifications(verification_id,candidate_id,project_id,status,report_json,started_at,completed_at) VALUES(?,?,?,'submitted',NULL,NULL,NULL)")
                .bind(&new_verification_id).bind(&candidate_id).bind(&fact.project_id)
                .execute(&mut *tx).await?;
            sqlx::query("UPDATE candidates SET status='submitted' WHERE candidate_id=?")
                .bind(&candidate_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE facts SET status='challenged' WHERE fact_id=?")
                .bind(fact_id)
                .execute(&mut *tx)
                .await?;
            let challenge_id = new_id("challenge");
            sqlx::query("INSERT INTO fact_challenges(challenge_id,project_id,fact_id,kind,reason,status,requested_by,verification_id,created_at,resolved_at) VALUES(?,?,?,?,?,'open',?,?,?,NULL)")
                .bind(&challenge_id).bind(&fact.project_id).bind(fact_id).bind(action).bind(reason)
                .bind(requested_by).bind(&new_verification_id).bind(now.to_rfc3339())
                .execute(&mut *tx).await?;
            challenge = Some(FactChallenge {
                challenge_id,
                project_id: fact.project_id.clone(),
                fact_id: fact_id.into(),
                kind: action.into(),
                reason: reason.into(),
                status: "open".into(),
                requested_by: requested_by.into(),
                verification_id: Some(new_verification_id.clone()),
                created_at: now,
                resolved_at: None,
            });
            // Starting a governance review immediately withdraws trust from the challenged
            // Fact. Reopen every closure that depended on it in this transaction so there is
            // no window in which a challenged Fact still presents a solved Goal/satisfied POG.
            let (applied_impact, invalidation_events) =
                cascade_fact_invalidation(&mut tx, fact_id, FactStatus::Challenged).await?;
            impact = applied_impact;
            external_events = invalidation_events;
            verification_id = Some(new_verification_id);
            event_type = match action {
                "request_formalization" => "fact.formalization_requested",
                "request_independent_proof" => "fact.independent_proof_requested",
                _ => "fact.challenged",
            };
        } else {
            let root_status = if action == "revoke" {
                FactStatus::Revoked
            } else {
                FactStatus::Suspended
            };
            // A later human suspend/revoke fences every older governance verifier. Otherwise
            // its delayed Accepted result could reactivate the Fact against the newer command.
            cancel_open_fact_challenges_tx(
                &mut tx,
                &fact.project_id,
                fact_id,
                "superseded by a later Fact suspend/revoke command",
                &now,
            )
            .await?;
            let (applied_impact, invalidation_events) =
                cascade_fact_invalidation(&mut tx, fact_id, root_status).await?;
            impact = applied_impact;
            external_events = invalidation_events;
            event_type = if action == "revoke" {
                "fact.revoked"
            } else {
                "fact.suspended"
            };
        }

        let updated_row = sqlx::query("SELECT * FROM facts WHERE fact_id=?")
            .bind(fact_id)
            .fetch_one(&mut *tx)
            .await?;
        let result = FactGovernanceResult {
            fact: rows::fact(&updated_row)?,
            challenge,
            verification_id,
            impact,
        };
        let revision = bump_revision(&mut tx, &fact.project_id).await?;
        let mut events = external_events;
        events.push(append_event(
            &mut tx,
            &fact.project_id,
            revision,
            event_type,
            entity("fact", fact_id),
            json!({"action": action, "reason": reason, "requested_by": requested_by, "verification_id": result.verification_id, "impact": result.impact}),
            Some(entity("fact_governance_command", &command_id)),
        )
        .await?);
        for obligation_id in &result.impact.reopened_obligation_ids {
            events.push(
                append_event(
                    &mut tx,
                    &fact.project_id,
                    revision,
                    "proof_obligation.updated",
                    entity("proof_obligation", obligation_id),
                    json!({"status":"open","reason":"supporting Fact is no longer trusted","fact_id":fact_id}),
                    Some(entity("fact_governance_command", &command_id)),
                )
                .await?,
            );
        }
        if let Some(new_verification_id) = &result.verification_id {
            events.push(
                append_event(
                    &mut tx,
                    &fact.project_id,
                    revision,
                    "verification.submitted",
                    entity("verification", new_verification_id),
                    json!({"fact_id": fact_id, "governance_action": action}),
                    Some(entity("fact", fact_id)),
                )
                .await?,
            );
        }
        sqlx::query("UPDATE fact_governance_commands SET result_json=?,completed_at=? WHERE governance_command_id=?")
            .bind(json_text(&result)?).bind(now.to_rfc3339()).bind(&command_id)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok((result, events))
    }
}

pub(crate) async fn cascade_fact_invalidation(
    tx: &mut Transaction<'_, Sqlite>,
    fact_id: &str,
    root_status: FactStatus,
) -> StorageResult<(FactImpact, Vec<DomainEvent>)> {
    let project_id: String = sqlx::query_scalar("SELECT project_id FROM facts WHERE fact_id=?")
        .bind(fact_id)
        .fetch_one(&mut **tx)
        .await?;
    let impact =
        invalidate_fact_for_project_tx(tx, &project_id, fact_id, Some(root_status)).await?;
    let mut invalidated = impact.affected_fact_ids.clone();
    invalidated.push(fact_id.into());
    let mut external_events = Vec::new();
    for invalidated_id in &invalidated {
        // Project the authoritative Fact status instead of assuming every
        // descendant became suspended. Revoked is an absorbing local state and
        // must remain revoked in the global catalog as well.
        let catalog_status: Option<String> =
            sqlx::query_scalar("SELECT status FROM facts WHERE fact_id=?")
                .bind(invalidated_id)
                .fetch_optional(&mut **tx)
                .await?;
        if let Some(catalog_status) = catalog_status {
            sqlx::query(
                "UPDATE global_fact_catalog SET status=?,updated_at=? WHERE source_fact_id=?",
            )
            .bind(catalog_status)
            .bind(Utc::now().to_rfc3339())
            .bind(invalidated_id)
            .execute(&mut **tx)
            .await?;
        }
        external_events
            .extend(crate::fact_catalog::invalidate_imports_for_source(tx, invalidated_id).await?);
    }
    Ok((impact, external_events))
}

/// Fence every in-flight governance review for one Fact. Direct human commands and problem
/// revisions share this path so a stale lease cannot later reactivate the Fact.
pub(crate) async fn cancel_open_fact_challenges_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    fact_id: &str,
    reason: &str,
    now: &DateTime<Utc>,
) -> StorageResult<Vec<String>> {
    let verification_ids = sqlx::query_scalar::<_, String>(
        "SELECT verification_id FROM fact_challenges WHERE project_id=? AND fact_id=? AND status='open' AND verification_id IS NOT NULL ORDER BY created_at,challenge_id",
    )
    .bind(project_id)
    .bind(fact_id)
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query("UPDATE candidates SET status='superseded' WHERE project_id=? AND candidate_id IN (SELECT v.candidate_id FROM verifications v JOIN fact_challenges fc ON fc.verification_id=v.verification_id WHERE fc.project_id=? AND fc.fact_id=? AND fc.status='open') AND status IN ('submitted','prechecking','verifying')")
        .bind(project_id).bind(project_id).bind(fact_id).execute(&mut **tx).await?;
    sqlx::query("UPDATE verifications SET status='superseded',completed_at=COALESCE(completed_at,?) WHERE project_id=? AND verification_id IN (SELECT verification_id FROM fact_challenges WHERE project_id=? AND fact_id=? AND status='open') AND status IN ('submitted','prechecking','verifying')")
        .bind(now.to_rfc3339()).bind(project_id).bind(project_id).bind(fact_id).execute(&mut **tx).await?;
    sqlx::query("UPDATE verification_cases SET stage='cancelled',cancellation_epoch=cancellation_epoch+1,achieved_acceptance=NULL,completed_at=COALESCE(completed_at,?),updated_at=? WHERE project_id=? AND verification_id IN (SELECT verification_id FROM fact_challenges WHERE project_id=? AND fact_id=? AND status='open') AND stage NOT IN ('committed','rejected','unknown','failed','cancelled')")
        .bind(now.to_rfc3339()).bind(now.to_rfc3339()).bind(project_id).bind(project_id).bind(fact_id).execute(&mut **tx).await?;
    sqlx::query("UPDATE verification_task_leases SET status='cancelled',completed_at=? WHERE project_id=? AND case_id IN (SELECT vc.case_id FROM verification_cases vc JOIN fact_challenges fc ON fc.verification_id=vc.verification_id WHERE fc.project_id=? AND fc.fact_id=? AND fc.status='open') AND status='active'")
        .bind(now.to_rfc3339()).bind(project_id).bind(project_id).bind(fact_id).execute(&mut **tx).await?;
    sqlx::query("UPDATE verification_attempts SET status='cancelled',error_kind='fact_governance_fence',error_message=?,completed_at=? WHERE case_id IN (SELECT vc.case_id FROM verification_cases vc JOIN fact_challenges fc ON fc.verification_id=vc.verification_id WHERE fc.project_id=? AND fc.fact_id=? AND fc.status='open') AND status NOT IN ('completed','failed','orphaned','cancelled')")
        .bind(reason).bind(now.to_rfc3339()).bind(project_id).bind(fact_id).execute(&mut **tx).await?;
    sqlx::query("UPDATE verification_result_envelopes SET status='stale',rejection_reason=? WHERE project_id=? AND case_id IN (SELECT vc.case_id FROM verification_cases vc JOIN fact_challenges fc ON fc.verification_id=vc.verification_id WHERE fc.project_id=? AND fc.fact_id=? AND fc.status='open') AND status='submitted'")
        .bind(reason).bind(project_id).bind(project_id).bind(fact_id).execute(&mut **tx).await?;
    sqlx::query("UPDATE context_packets SET status='invalidated',invalidation_reason=COALESCE(invalidation_reason,?) WHERE project_id=? AND status='active' AND context_packet_id IN (SELECT a.context_packet_id FROM verification_attempts a JOIN verification_cases vc ON vc.case_id=a.case_id JOIN fact_challenges fc ON fc.verification_id=vc.verification_id WHERE fc.project_id=? AND fc.fact_id=? AND fc.status='open')")
        .bind(reason).bind(project_id).bind(project_id).bind(fact_id).execute(&mut **tx).await?;
    sqlx::query("UPDATE worker_instances SET status='exited',quarantine_reason=COALESCE(quarantine_reason,?),exited_at=COALESCE(exited_at,?) WHERE project_id=? AND worker_instance_id IN (SELECT a.worker_instance_id FROM verification_attempts a JOIN verification_cases vc ON vc.case_id=a.case_id JOIN fact_challenges fc ON fc.verification_id=vc.verification_id WHERE fc.project_id=? AND fc.fact_id=? AND fc.status='open') AND status NOT IN ('exited','quarantined')")
        .bind(reason).bind(now.to_rfc3339()).bind(project_id).bind(project_id).bind(fact_id).execute(&mut **tx).await?;
    sqlx::query("UPDATE proof_searches SET status='cancelled',cancellation_epoch=cancellation_epoch+1,completed_at=? WHERE status='running' AND case_id IN (SELECT vc.case_id FROM verification_cases vc JOIN fact_challenges fc ON fc.verification_id=vc.verification_id WHERE fc.project_id=? AND fc.fact_id=? AND fc.status='open')")
        .bind(now.to_rfc3339()).bind(project_id).bind(fact_id).execute(&mut **tx).await?;
    sqlx::query("UPDATE proof_nodes SET status='cancelled',updated_at=? WHERE status IN ('open','running') AND search_id IN (SELECT ps.search_id FROM proof_searches ps JOIN verification_cases vc ON vc.case_id=ps.case_id JOIN fact_challenges fc ON fc.verification_id=vc.verification_id WHERE fc.project_id=? AND fc.fact_id=? AND fc.status='open')")
        .bind(now.to_rfc3339()).bind(project_id).bind(fact_id).execute(&mut **tx).await?;
    sqlx::query("UPDATE fact_challenges SET status='cancelled',resolved_at=COALESCE(resolved_at,?) WHERE project_id=? AND fact_id=? AND status='open'")
        .bind(now.to_rfc3339()).bind(project_id).bind(fact_id).execute(&mut **tx).await?;
    Ok(verification_ids)
}

/// Recompute the project lifecycle after one governance verification succeeds.
/// Suspended descendants remain unavailable and keep the project in review; they are never
/// silently revived from the root Fact's result.
pub(crate) async fn reconcile_project_after_governance_acceptance_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
) -> StorageResult<String> {
    let pending_fact_reviews: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM facts WHERE project_id=? AND status IN ('challenged','suspended')",
    )
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    let open_challenges: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM fact_challenges WHERE project_id=? AND status='open'",
    )
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    let main_goal_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM goals WHERE project_id=? AND priority>=1.0")
            .bind(project_id)
            .fetch_one(&mut **tx)
            .await?;
    let refuted_main_goals: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM goals WHERE project_id=? AND priority>=1.0 AND status='refuted'",
    )
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    let unresolved_main_goals: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM goals WHERE project_id=? AND priority>=1.0 AND status!='solved'",
    )
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    let blocking_uncertainties: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM uncertainties WHERE project_id=? AND status IN ('open','investigating') AND severity IN ('critical','high')",
    )
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    let unresolved_required: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM proof_obligations o JOIN goals g ON g.goal_id=o.goal_id AND g.project_id=o.project_id WHERE o.project_id=? AND g.priority>=1.0 AND o.necessity='required' AND o.status NOT IN ('satisfied','obsolete')",
    )
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    let next_status = if pending_fact_reviews != 0 || open_challenges != 0 {
        "needs_human_review"
    } else if refuted_main_goals != 0 {
        "refuted"
    } else if main_goal_count != 0
        && unresolved_main_goals == 0
        && blocking_uncertainties == 0
        && unresolved_required == 0
    {
        "success"
    } else {
        "running"
    };
    sqlx::query("UPDATE projects SET status=?,updated_at=? WHERE project_id=? AND status NOT IN ('stopped_by_human','error')")
        .bind(next_status)
        .bind(Utc::now().to_rfc3339())
        .bind(project_id)
        .execute(&mut **tx)
        .await?;
    Ok(next_status.into())
}

/// Applies the project-local consequences of losing a trusted fact.
///
/// `root_status` is present when the unavailable fact belongs to `project_id`; imported facts do
/// not have a local `facts` row, so callers pass `None`. Catalog propagation and audit events stay
/// with the caller, while every mutable project projection is updated here in the caller's
/// transaction.
pub(crate) async fn invalidate_fact_for_project_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    fact_id: &str,
    root_status: Option<FactStatus>,
) -> StorageResult<FactImpact> {
    let mut impact = compute_fact_impact_tx(tx, project_id, fact_id).await?;
    let now_value = Utc::now();
    let now = now_value.to_rfc3339();

    if let Some(root_status) = root_status {
        sqlx::query("UPDATE facts SET status=? WHERE project_id=? AND fact_id=?")
            .bind(root_status.to_string())
            .bind(project_id)
            .bind(fact_id)
            .execute(&mut **tx)
            .await?;
        invalidate_fact_assurance_tx(tx, fact_id, &now).await?;
    }
    for affected_id in &impact.affected_fact_ids {
        sqlx::query("UPDATE facts SET status='suspended' WHERE project_id=? AND fact_id=? AND status!='revoked'")
            .bind(project_id)
            .bind(affected_id)
            .execute(&mut **tx)
            .await?;
        invalidate_fact_assurance_tx(tx, affected_id, &now).await?;
    }

    let mut unavailable_fact_ids = impact.affected_fact_ids.clone();
    unavailable_fact_ids.push(fact_id.into());
    for unavailable_fact_id in &unavailable_fact_ids {
        sqlx::query("UPDATE context_packets SET status='invalidated',invalidation_reason=COALESCE(invalidation_reason,?) WHERE project_id=? AND status='active' AND EXISTS (SELECT 1 FROM json_each(context_packets.known_fact_ids_json) WHERE value=?)")
            .bind(format!("fact {unavailable_fact_id} is no longer active"))
            .bind(project_id)
            .bind(unavailable_fact_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("UPDATE context_summaries SET status='invalidated',invalidated_at=? WHERE project_id=? AND status='active' AND EXISTS (SELECT 1 FROM json_each(context_summaries.input_entity_ids_json) WHERE value=?)")
            .bind(&now)
            .bind(project_id)
            .bind(unavailable_fact_id)
            .execute(&mut **tx)
            .await?;
    }

    for goal_id in &impact.reopened_goal_ids {
        sqlx::query("UPDATE goals SET status='open',solved_by_fact_id=NULL WHERE project_id=? AND goal_id=?")
            .bind(project_id)
            .bind(goal_id)
            .execute(&mut **tx)
            .await?;
    }
    let obligation_revision: i64 =
        sqlx::query_scalar("SELECT revision+1 FROM projects WHERE project_id=?")
            .bind(project_id)
            .fetch_one(&mut **tx)
            .await?;
    impact.reopened_obligation_ids = crate::proof_obligations::reopen_required_obligations_tx(
        tx,
        project_id,
        &impact.reopened_goal_ids,
        &unavailable_fact_ids,
        obligation_revision,
        &now_value,
    )
    .await?;
    for route_id in &impact.paused_route_ids {
        sqlx::query("UPDATE routes SET status='paused',cancellation_epoch=cancellation_epoch+1 WHERE project_id=? AND route_id=? AND status IN ('incubating','active','blocked','probation','revived')")
            .bind(project_id)
            .bind(route_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("UPDATE tasks SET status='cancelled',revision=revision+1,result_summary=COALESCE(result_summary,'cancelled because a required fact is no longer active') WHERE project_id=? AND route_id=? AND status IN ('open','queued','assigned','offered','leased','running','checkpointed','result_submitted','ingesting','paused','blocked')")
            .bind(project_id)
            .bind(route_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("UPDATE task_leases SET status='cancelled',completed_at=? WHERE project_id=? AND task_id IN (SELECT task_id FROM tasks WHERE project_id=? AND route_id=?) AND status='active'")
            .bind(&now)
            .bind(project_id)
            .bind(project_id)
            .bind(route_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("UPDATE task_attempts SET status='cancelled',failure_reason=COALESCE(failure_reason,'required fact invalidated'),completed_at=? WHERE project_id=? AND task_id IN (SELECT task_id FROM tasks WHERE project_id=? AND route_id=?) AND status NOT IN ('completed','failed','cancelled','orphaned')")
            .bind(&now)
            .bind(project_id)
            .bind(project_id)
            .bind(route_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("UPDATE worker_instances SET status='exited',exited_at=? WHERE project_id=? AND worker_instance_id IN (SELECT worker_instance_id FROM task_attempts WHERE project_id=? AND task_id IN (SELECT task_id FROM tasks WHERE project_id=? AND route_id=?)) AND status IN ('spawn_requested','starting','handshaking','ready','lease_accepted','running','checkpointing','result_submitted','draining')")
            .bind(&now)
            .bind(project_id)
            .bind(project_id)
            .bind(project_id)
            .bind(route_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("UPDATE workers SET status='idle',current_task_id=NULL,current_route_id=NULL WHERE project_id=? AND current_route_id=?")
            .bind(project_id)
            .bind(route_id)
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("UPDATE projects SET status='needs_human_review',updated_at=? WHERE project_id=? AND status NOT IN ('stopped_by_human','error')")
        .bind(now)
        .bind(project_id)
        .execute(&mut **tx)
        .await?;
    Ok(impact)
}

async fn invalidate_fact_assurance_tx(
    tx: &mut Transaction<'_, Sqlite>,
    fact_id: &str,
    invalidated_at: &str,
) -> StorageResult<()> {
    sqlx::query(
        "UPDATE fact_assurances SET status='invalidated',invalidated_at=? WHERE fact_id=? AND status='active'",
    )
    .bind(invalidated_at)
    .bind(fact_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn compute_fact_impact(
    pool: &sqlx::SqlitePool,
    project_id: &str,
    fact_id: &str,
) -> StorageResult<FactImpact> {
    let mut tx = pool.begin().await?;
    let impact = compute_fact_impact_tx(&mut tx, project_id, fact_id).await?;
    tx.rollback().await?;
    Ok(impact)
}

async fn compute_fact_impact_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    fact_id: &str,
) -> StorageResult<FactImpact> {
    let rows = sqlx::query("WITH RECURSIVE impact(id) AS (SELECT target_id FROM fact_edges WHERE project_id=? AND source_id=? UNION SELECT e.target_id FROM fact_edges e JOIN impact i ON e.source_id=i.id WHERE e.project_id=?) SELECT DISTINCT id FROM impact ORDER BY id")
        .bind(project_id).bind(fact_id).bind(project_id).fetch_all(&mut **tx).await?;
    let affected_fact_ids = rows
        .iter()
        .map(|row| row.try_get("id").map_err(StorageError::from))
        .collect::<StorageResult<Vec<String>>>()?;
    let mut all_facts = affected_fact_ids.clone();
    all_facts.push(fact_id.into());
    let goal_rows = sqlx::query(
        "SELECT goal_id,solved_by_fact_id FROM goals WHERE project_id=? AND solved_by_fact_id IS NOT NULL",
    )
    .bind(project_id)
    .fetch_all(&mut **tx)
    .await?;
    let reopened_goal_ids = goal_rows
        .iter()
        .filter_map(|row| {
            let solved_by = row.try_get::<String, _>("solved_by_fact_id").ok()?;
            all_facts
                .iter()
                .any(|item| item == &solved_by)
                .then(|| row.try_get::<String, _>("goal_id").ok())
                .flatten()
        })
        .collect::<Vec<_>>();
    let route_rows = sqlx::query("SELECT route_id,required_fact_ids_json FROM routes WHERE project_id=? AND status IN ('incubating','active','blocked','probation','revived','paused')")
        .bind(project_id).fetch_all(&mut **tx).await?;
    let mut paused_route_ids = Vec::new();
    for row in route_rows {
        let required: Vec<String> = serde_json::from_str(row.try_get("required_fact_ids_json")?)?;
        if required
            .iter()
            .any(|required_id| all_facts.contains(required_id))
        {
            paused_route_ids.push(row.try_get("route_id")?);
        }
    }
    Ok(FactImpact {
        fact_id: fact_id.into(),
        affected_fact_ids,
        reopened_goal_ids,
        reopened_obligation_ids: Vec::new(),
        paused_route_ids,
    })
}

fn fact_assurance_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<FactAssurance> {
    Ok(FactAssurance {
        fact_id: row.try_get("fact_id")?,
        case_id: row.try_get("case_id")?,
        acceptance_class: AcceptanceClass::from_str(row.try_get("acceptance_class")?)
            .map_err(StorageError::CorruptData)?,
        snapshot_hash: row.try_get("snapshot_hash")?,
        package_id: row.try_get("package_id")?,
        replay_id: row.try_get("replay_id")?,
        status: row.try_get("status")?,
        created_at: timestamp(row.try_get("created_at")?)?,
        invalidated_at: optional_timestamp(row.try_get("invalidated_at")?)?,
    })
}

fn fact_challenge_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<FactChallenge> {
    Ok(FactChallenge {
        challenge_id: row.try_get("challenge_id")?,
        project_id: row.try_get("project_id")?,
        fact_id: row.try_get("fact_id")?,
        kind: row.try_get("kind")?,
        reason: row.try_get("reason")?,
        status: row.try_get("status")?,
        requested_by: row.try_get("requested_by")?,
        verification_id: row.try_get("verification_id")?,
        created_at: timestamp(row.try_get("created_at")?)?,
        resolved_at: optional_timestamp(row.try_get("resolved_at")?)?,
    })
}

fn timestamp(value: &str) -> StorageResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| StorageError::CorruptData(error.to_string()))
}

fn optional_timestamp(value: Option<String>) -> StorageResult<Option<DateTime<Utc>>> {
    value.as_deref().map(timestamp).transpose()
}
