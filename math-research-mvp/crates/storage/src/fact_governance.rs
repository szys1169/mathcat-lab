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
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query("SELECT * FROM facts WHERE fact_id=?")
            .bind(fact_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "fact",
                id: fact_id.into(),
            })?;
        let fact = rows::fact(&row)?;
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

        let command_id = new_id("fgcmd");
        let now = Utc::now();
        sqlx::query("INSERT INTO fact_governance_commands(governance_command_id,project_id,fact_id,idempotency_key,action,request_hash,result_json,requested_by,reason,created_at,completed_at) VALUES(?,?,?,?,?,?,NULL,?,?,?,NULL)")
            .bind(&command_id).bind(&fact.project_id).bind(fact_id).bind(idempotency_key)
            .bind(action).bind(&request_hash).bind(requested_by).bind(reason)
            .bind(now.to_rfc3339()).execute(&mut *tx).await?;

        let impact = compute_fact_impact_tx(&mut tx, &fact.project_id, fact_id).await?;
        let mut challenge = None;
        let mut verification_id = None;
        let mut external_events = Vec::new();
        let event_type;
        if matches!(
            action,
            "challenge" | "reverify" | "request_formalization" | "request_independent_proof"
        ) {
            if fact.status == FactStatus::Revoked {
                return Err(StorageError::InvalidTransition(
                    "a revoked fact cannot be reverified in place".into(),
                ));
            }
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
            if action == "revoke" {
                sqlx::query("UPDATE verifications SET status='superseded',completed_at=? WHERE verification_id IN (SELECT verification_id FROM fact_challenges WHERE fact_id=? AND status='open') AND status IN ('submitted','verifying')")
                    .bind(now.to_rfc3339()).bind(fact_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE fact_challenges SET status='cancelled',resolved_at=? WHERE fact_id=? AND status='open'")
                    .bind(now.to_rfc3339()).bind(fact_id).execute(&mut *tx).await?;
            }
            let (_, invalidation_events) =
                cascade_fact_invalidation(&mut tx, fact_id, root_status).await?;
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
    let impact = compute_fact_impact_tx(tx, &project_id, fact_id).await?;
    let mut invalidated = impact.affected_fact_ids.clone();
    invalidated.push(fact_id.into());
    let mut external_events = Vec::new();
    for invalidated_id in &invalidated {
        let catalog_status = if invalidated_id == fact_id {
            root_status.to_string()
        } else {
            FactStatus::Suspended.to_string()
        };
        sqlx::query("UPDATE global_fact_catalog SET status=?,updated_at=? WHERE source_fact_id=?")
            .bind(catalog_status)
            .bind(Utc::now().to_rfc3339())
            .bind(invalidated_id)
            .execute(&mut **tx)
            .await?;
        external_events
            .extend(crate::fact_catalog::invalidate_imports_for_source(tx, invalidated_id).await?);
    }
    sqlx::query("UPDATE facts SET status=? WHERE fact_id=?")
        .bind(root_status.to_string())
        .bind(fact_id)
        .execute(&mut **tx)
        .await?;
    for affected_id in &impact.affected_fact_ids {
        sqlx::query("UPDATE facts SET status='suspended' WHERE fact_id=? AND status!='revoked'")
            .bind(affected_id)
            .execute(&mut **tx)
            .await?;
    }
    for invalidated_id in &invalidated {
        sqlx::query(
            "UPDATE fact_assurances SET status='invalidated',invalidated_at=? WHERE fact_id=? AND status='active'",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(invalidated_id)
        .execute(&mut **tx)
        .await?;
        sqlx::query("UPDATE context_packets SET status='invalidated',invalidation_reason=? WHERE project_id=? AND status='active' AND EXISTS (SELECT 1 FROM json_each(context_packets.known_fact_ids_json) WHERE value=?)")
            .bind(format!("fact {invalidated_id} is no longer active")).bind(&project_id).bind(invalidated_id)
            .execute(&mut **tx).await?;
        sqlx::query("UPDATE context_summaries SET status='invalidated',invalidated_at=? WHERE project_id=? AND status='active' AND EXISTS (SELECT 1 FROM json_each(context_summaries.input_entity_ids_json) WHERE value=?)")
            .bind(Utc::now().to_rfc3339()).bind(&project_id).bind(invalidated_id)
            .execute(&mut **tx).await?;
    }
    for goal_id in &impact.reopened_goal_ids {
        sqlx::query("UPDATE goals SET status='open',solved_by_fact_id=NULL WHERE goal_id=?")
            .bind(goal_id)
            .execute(&mut **tx)
            .await?;
    }
    for route_id in &impact.paused_route_ids {
        sqlx::query("UPDATE routes SET status='paused',cancellation_epoch=cancellation_epoch+1 WHERE route_id=? AND status IN ('incubating','active','probation','revived')")
            .bind(route_id).execute(&mut **tx).await?;
        sqlx::query("UPDATE tasks SET status='cancelled',revision=revision+1 WHERE route_id=? AND status IN ('open','assigned','queued','offered','leased','running','checkpointed','result_submitted','ingesting','blocked')")
            .bind(route_id).execute(&mut **tx).await?;
        sqlx::query("UPDATE workers SET status='idle',current_task_id=NULL,current_route_id=NULL WHERE current_route_id=?")
            .bind(route_id).execute(&mut **tx).await?;
    }
    sqlx::query("UPDATE projects SET status='needs_human_review',updated_at=? WHERE project_id=? AND status NOT IN ('stopped_by_human','error')")
        .bind(Utc::now().to_rfc3339()).bind(&project_id).execute(&mut **tx).await?;
    Ok((impact, external_events))
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

pub(crate) async fn compute_fact_impact_tx(
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
