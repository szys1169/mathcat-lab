use std::collections::HashSet;

use chrono::Utc;
use research_domain::{
    AssignmentDraft, Budget, DomainEvent, HumanRouteCreateRequest, HumanRouteCreateResult,
    HumanRouteProposal, HumanRouteProposalRequest, HumanRouteProposalResult, ProblemContract,
    ProblemRevisionRequest, ProblemRevisionResult,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{
    SqliteStore, StorageError, StorageResult, append_event, bump_revision, entity, json_text,
    new_id,
    reliability_v2::{route_family_key, route_fingerprint},
    task_materialization::{V2TaskMaterialization, materialize_v2_task_tx},
    write::validate_project_budget,
};

#[derive(Debug)]
pub struct BoardMutation<T> {
    pub data: T,
    pub project_revision: i64,
    pub event_cursor: i64,
    pub events: Vec<DomainEvent>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredBoardWrite<T> {
    data: T,
    project_revision: i64,
    event_cursor: i64,
}

impl SqliteStore {
    pub async fn revise_problem_contract(
        &self,
        project_id: &str,
        request: &ProblemRevisionRequest,
        requested_by: &str,
        idempotency_key: &str,
    ) -> StorageResult<BoardMutation<ProblemRevisionResult>> {
        validate_problem_revision(request, idempotency_key)?;
        let request_hash = request_hash("problem_revision", request, requested_by)?;
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "revise_problem_contract",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        if let Some(stored) = replay_board_write(
            &mut tx,
            project_id,
            idempotency_key,
            "problem_revision",
            &request_hash,
        )
        .await?
        {
            tx.rollback().await?;
            return Ok(BoardMutation {
                data: stored.data,
                project_revision: stored.project_revision,
                event_cursor: stored.event_cursor,
                events: vec![],
            });
        }
        let row = sqlx::query(
            "SELECT problem_contract_json,revision,status FROM projects WHERE project_id=?",
        )
        .bind(project_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "project",
            id: project_id.into(),
        })?;
        let actual: i64 = row.try_get("revision")?;
        let previous_project_status: String = row.try_get("status")?;
        if actual != request.expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: request.expected_revision,
                actual,
            });
        }
        if matches!(
            previous_project_status.as_str(),
            "stopped_by_human" | "error"
        ) {
            return Err(StorageError::InvalidTransition(format!(
                "cannot revise a project while it is {previous_project_status}"
            )));
        }
        let previous: ProblemContract =
            serde_json::from_str(row.try_get("problem_contract_json")?)?;
        let next = ProblemContract {
            original_problem: previous.original_problem.clone(),
            target_statement: request.target_statement.trim().to_owned(),
            assumptions: request.assumptions.clone(),
            success_criteria: request.success_criteria.trim().to_owned(),
            version: previous.version + 1,
        };
        if previous.target_statement == next.target_statement
            && previous.assumptions == next.assumptions
            && previous.success_criteria == next.success_criteria
        {
            return Err(StorageError::InvalidProblemRevision(
                "problem revision does not change the contract".into(),
            ));
        }

        let command_id = new_id("cmd");
        let now = Utc::now();
        insert_applied_command(
            &mut tx,
            project_id,
            &command_id,
            "revise_problem_contract",
            "project",
            project_id,
            request.expected_revision,
            idempotency_key,
            &request.change_reason,
            requested_by,
            &serde_json::to_value(request)?,
            actual,
            now.to_rfc3339().as_str(),
        )
        .await?;

        let route_rows = sqlx::query("SELECT route_id FROM routes WHERE project_id=? AND status IN ('proposed','incubating','active','blocked','probation','revived','paused')")
            .bind(project_id).fetch_all(&mut *tx).await?;
        let route_ids = route_rows
            .iter()
            .map(|row| row.try_get::<String, _>("route_id"))
            .collect::<Result<Vec<_>, _>>()?;
        let task_rows = sqlx::query("SELECT task_id FROM tasks WHERE project_id=? AND status IN ('open','queued','assigned','offered','leased','running','checkpointed','result_submitted','ingesting','paused','blocked')")
            .bind(project_id).fetch_all(&mut *tx).await?;
        let task_ids = task_rows
            .iter()
            .map(|row| row.try_get::<String, _>("task_id"))
            .collect::<Result<Vec<_>, _>>()?;
        let result_envelope_ids = sqlx::query_scalar::<_, String>(
            "SELECT result_envelope_id FROM result_envelopes WHERE project_id=? AND status='submitted' ORDER BY submitted_at,result_envelope_id",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        let verification_rows = sqlx::query(
            "SELECT verification_id,candidate_id FROM verifications WHERE project_id=? AND status IN ('submitted','prechecking','verifying')",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        let verification_ids = verification_rows
            .iter()
            .map(|row| row.try_get::<String, _>("verification_id"))
            .collect::<Result<Vec<_>, _>>()?;
        let candidate_ids = verification_rows
            .iter()
            .map(|row| row.try_get::<String, _>("candidate_id"))
            .collect::<Result<Vec<_>, _>>()?;
        let publication_ids = sqlx::query_scalar::<_, String>(
            "SELECT publication_id FROM publication_runs WHERE project_id=? AND source_revision<=? AND status IN ('running','ready','blocked_by_evidence','blocked') ORDER BY publication_id",
        )
        .bind(project_id)
        .bind(actual)
        .fetch_all(&mut *tx)
        .await?;
        let open_challenged_fact_ids = sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT fact_id FROM fact_challenges WHERE project_id=? AND status='open' ORDER BY fact_id",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        let main_goal_id: String = sqlx::query_scalar(
            "SELECT goal_id FROM goals WHERE project_id=? ORDER BY priority DESC,goal_id LIMIT 1",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        let old_bottleneck_rows = sqlx::query(
            "SELECT bottleneck_id FROM bottlenecks WHERE project_id=? AND status='open'",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        let old_bottleneck_ids = old_bottleneck_rows
            .iter()
            .map(|row| row.try_get::<String, _>("bottleneck_id"))
            .collect::<Result<Vec<_>, _>>()?;
        sqlx::query("UPDATE routes SET cancellation_epoch=cancellation_epoch+1 WHERE project_id=? AND status IN ('proposed','incubating','active','blocked','probation','revived','paused')")
            .bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE tasks SET status='cancelled',revision=revision+1,result_summary='invalidated by problem contract revision' WHERE project_id=? AND status IN ('open','queued','assigned','offered','leased','running','checkpointed','result_submitted','ingesting','paused','blocked')")
            .bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE result_envelopes SET status='stale',rejection_reason='problem contract revision invalidated the task result' WHERE project_id=? AND status='submitted'")
            .bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE task_leases SET status='cancelled',completed_at=? WHERE task_id IN (SELECT task_id FROM tasks WHERE project_id=? AND status='cancelled') AND status='active'")
            .bind(now.to_rfc3339()).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE task_attempts SET status='cancelled',completed_at=? WHERE task_id IN (SELECT task_id FROM tasks WHERE project_id=? AND status='cancelled') AND status IN ('created','offered','leased','running','checkpointed','result_submitted','ingesting')")
            .bind(now.to_rfc3339()).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE workers SET status='idle',current_task_id=NULL,current_route_id=NULL WHERE project_id=? AND current_task_id IN (SELECT task_id FROM tasks WHERE project_id=? AND status='cancelled')")
            .bind(project_id).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE worker_instances SET status='exited',exited_at=? WHERE project_id=? AND status IN ('spawn_requested','starting','handshaking','ready','lease_accepted','running','checkpointing','result_submitted','draining')")
            .bind(now.to_rfc3339()).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE verifications SET status='superseded',completed_at=COALESCE(completed_at,?) WHERE project_id=? AND status IN ('submitted','prechecking','verifying')")
            .bind(now.to_rfc3339()).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE candidates SET status='superseded' WHERE project_id=? AND candidate_id IN (SELECT candidate_id FROM verifications WHERE project_id=? AND status='superseded') AND status IN ('submitted','prechecking','verifying')")
            .bind(project_id).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE verification_cases SET stage='cancelled',cancellation_epoch=cancellation_epoch+1,achieved_acceptance=NULL,completed_at=COALESCE(completed_at,?),updated_at=? WHERE project_id=? AND verification_id IN (SELECT verification_id FROM verifications WHERE project_id=? AND status='superseded') AND stage NOT IN ('committed','rejected','unknown','failed','cancelled')")
            .bind(now.to_rfc3339()).bind(now.to_rfc3339()).bind(project_id).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE verification_task_leases SET status='cancelled',completed_at=? WHERE project_id=? AND case_id IN (SELECT case_id FROM verification_cases WHERE project_id=? AND stage='cancelled') AND status='active'")
            .bind(now.to_rfc3339()).bind(project_id).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE verification_attempts SET status='cancelled',error_kind='problem_revision',error_message='verification invalidated by problem contract revision',completed_at=? WHERE case_id IN (SELECT case_id FROM verification_cases WHERE project_id=? AND stage='cancelled') AND status NOT IN ('completed','failed','orphaned','cancelled')")
            .bind(now.to_rfc3339()).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE verification_result_envelopes SET status='stale',rejection_reason='problem contract revision invalidated the verification snapshot' WHERE project_id=? AND case_id IN (SELECT case_id FROM verification_cases WHERE project_id=? AND stage='cancelled') AND status='submitted'")
            .bind(project_id).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE context_packets SET status='invalidated',invalidation_reason=COALESCE(invalidation_reason,'problem contract revision invalidated the verification context') WHERE project_id=? AND status='active' AND context_packet_id IN (SELECT a.context_packet_id FROM verification_attempts a JOIN verification_cases vc ON vc.case_id=a.case_id WHERE vc.project_id=? AND vc.stage='cancelled')")
            .bind(project_id).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE proof_searches SET status='cancelled',cancellation_epoch=cancellation_epoch+1,completed_at=? WHERE status='running' AND case_id IN (SELECT case_id FROM verification_cases WHERE project_id=? AND stage='cancelled')")
            .bind(now.to_rfc3339()).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE proof_nodes SET status='cancelled',updated_at=? WHERE status IN ('open','running') AND search_id IN (SELECT ps.search_id FROM proof_searches ps JOIN verification_cases vc ON vc.case_id=ps.case_id WHERE vc.project_id=? AND vc.stage='cancelled')")
            .bind(now.to_rfc3339()).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE publication_runs SET status='stale',updated_at=?,completed_at=COALESCE(completed_at,?) WHERE project_id=? AND source_revision<=? AND status IN ('running','ready','blocked_by_evidence','blocked')")
            .bind(now.to_rfc3339()).bind(now.to_rfc3339()).bind(project_id).bind(actual)
            .execute(&mut *tx).await?;
        let obsolete_uncertainty_ids = sqlx::query_scalar::<_, String>(
            "SELECT uncertainty_id FROM uncertainties u WHERE u.project_id=? AND u.status IN ('open','investigating') AND (EXISTS (SELECT 1 FROM verifications v WHERE v.project_id=u.project_id AND v.verification_id=u.introduced_by) OR EXISTS (SELECT 1 FROM tasks t WHERE t.project_id=u.project_id AND t.task_id=u.introduced_by) OR EXISTS (SELECT 1 FROM routes r WHERE r.project_id=u.project_id AND r.route_id=u.introduced_by)) ORDER BY uncertainty_id",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE uncertainties SET status='obsolete',resolved_by=? WHERE project_id=? AND status IN ('open','investigating') AND (EXISTS (SELECT 1 FROM verifications v WHERE v.project_id=uncertainties.project_id AND v.verification_id=uncertainties.introduced_by) OR EXISTS (SELECT 1 FROM tasks t WHERE t.project_id=uncertainties.project_id AND t.task_id=uncertainties.introduced_by) OR EXISTS (SELECT 1 FROM routes r WHERE r.project_id=uncertainties.project_id AND r.route_id=uncertainties.introduced_by))",
        )
        .bind(&command_id)
        .bind(project_id)
        .execute(&mut *tx)
        .await?;

        let removed_assumptions = previous
            .assumptions
            .iter()
            .filter(|assumption| !next.assumptions.contains(assumption))
            .cloned()
            .collect::<HashSet<_>>();
        let mut suspended_fact_ids = HashSet::new();
        let mut reopened_fact_obligation_ids = HashSet::new();
        let mut fact_invalidation_events = Vec::new();
        let mut invalidation_root_fact_ids =
            open_challenged_fact_ids.into_iter().collect::<HashSet<_>>();
        if !removed_assumptions.is_empty() {
            let fact_rows = sqlx::query(
                "SELECT fact_id,assumptions_json FROM facts WHERE project_id=? AND status<>'revoked'",
            )
            .bind(project_id)
            .fetch_all(&mut *tx)
            .await?;
            for fact in fact_rows {
                let assumptions: Vec<String> =
                    serde_json::from_str(fact.try_get("assumptions_json")?)?;
                if assumptions
                    .iter()
                    .any(|item| removed_assumptions.contains(item))
                {
                    invalidation_root_fact_ids.insert(fact.try_get("fact_id")?);
                }
            }
        }
        let mut invalidation_root_fact_ids =
            invalidation_root_fact_ids.into_iter().collect::<Vec<_>>();
        invalidation_root_fact_ids.sort();
        for fact_id in invalidation_root_fact_ids {
            if suspended_fact_ids.contains(&fact_id) {
                continue;
            }
            crate::fact_governance::cancel_open_fact_challenges_tx(
                &mut tx,
                project_id,
                &fact_id,
                "problem contract revision invalidated the governance review",
                &now,
            )
            .await?;
            let (impact, external_events) = crate::fact_governance::cascade_fact_invalidation(
                &mut tx,
                &fact_id,
                research_domain::FactStatus::Suspended,
            )
            .await?;
            let mut unavailable = impact.affected_fact_ids;
            unavailable.push(fact_id);
            for unavailable_fact_id in unavailable {
                suspended_fact_ids.insert(unavailable_fact_id.clone());
                crate::fact_governance::cancel_open_fact_challenges_tx(
                    &mut tx,
                    project_id,
                    &unavailable_fact_id,
                    "problem contract revision invalidated the governance review",
                    &now,
                )
                .await?;
            }
            reopened_fact_obligation_ids.extend(impact.reopened_obligation_ids);
            fact_invalidation_events.extend(external_events);
        }
        let mut suspended_fact_ids = suspended_fact_ids.into_iter().collect::<Vec<_>>();
        suspended_fact_ids.sort();
        let obsolete_obligation_ids = crate::proof_obligations::obsolete_project_obligations_tx(
            &mut tx,
            project_id,
            actual + 1,
            &now,
        )
        .await?;
        sqlx::query(
            "UPDATE goals SET statement=?,status='open',solved_by_fact_id=NULL WHERE goal_id=?",
        )
        .bind(&next.target_statement)
        .bind(&main_goal_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE bottlenecks SET status='superseded',updated_revision=?,updated_at=? WHERE project_id=? AND status='open'")
            .bind(actual + 1).bind(now.to_rfc3339()).bind(project_id).execute(&mut *tx).await?;
        let new_bottleneck_id = new_id("bottleneck");
        sqlx::query("INSERT INTO bottlenecks(bottleneck_id,project_id,target_goal_ids_json,kind,precise_statement,completion_contract_json,priority,status,created_revision,updated_revision,created_at,updated_at) VALUES(?,?,?,?,?,?,1.0,'open',?,?,?,?)")
            .bind(&new_bottleneck_id).bind(project_id).bind(json_text(&vec![main_goal_id.clone()])?)
            .bind("problem_revision_main_goal").bind(format!("Resolve the revised main goal exactly as stated: {}", next.target_statement))
            .bind(json_text(&json!({"success_outputs":["a self-contained candidate proving the exact revised target, or a verified counterexample"],"partial_outputs":["a named lemma with an explicit bridge to the revised target"],"reject_outputs":["a result for the superseded contract","a silently strengthened assumption"]}))?)
            .bind(actual + 1).bind(actual + 1).bind(now.to_rfc3339()).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        let new_root_obligation_id = crate::proof_obligations::create_root_obligation_tx(
            &mut tx,
            project_id,
            &main_goal_id,
            &new_bottleneck_id,
            &next.target_statement,
            1.0,
            actual + 1,
            &json!({
                "source":"problem_revision",
                "goal_id":main_goal_id,
                "contract_version":next.version,
                "command_id":command_id,
            }),
            &now,
        )
        .await?;
        sqlx::query("UPDATE projects SET problem_contract_json=?,status=CASE WHEN ?=1 THEN 'running' ELSE 'paused' END,updated_at=? WHERE project_id=?")
            .bind(json_text(&next)?)
            .bind(request.replan)
            .bind(now.to_rfc3339())
            .bind(project_id)
            .execute(&mut *tx)
            .await?;
        let revision = bump_revision(&mut tx, project_id).await?;

        let mut affected_entities = route_ids
            .iter()
            .map(|id| entity("route", id))
            .chain(task_ids.iter().map(|id| entity("task", id)))
            .chain(
                result_envelope_ids
                    .iter()
                    .map(|id| entity("result_envelope", id)),
            )
            .chain(verification_ids.iter().map(|id| entity("verification", id)))
            .chain(candidate_ids.iter().map(|id| entity("candidate", id)))
            .chain(publication_ids.iter().map(|id| entity("publication", id)))
            .chain(
                obsolete_uncertainty_ids
                    .iter()
                    .map(|id| entity("uncertainty", id)),
            )
            .chain(suspended_fact_ids.iter().map(|id| entity("fact", id)))
            .chain(old_bottleneck_ids.iter().map(|id| entity("bottleneck", id)))
            .chain(
                obsolete_obligation_ids
                    .iter()
                    .map(|id| entity("proof_obligation", id)),
            )
            .collect::<Vec<_>>();
        affected_entities.push(entity("goal", &main_goal_id));
        affected_entities.push(entity("bottleneck", &new_bottleneck_id));
        affected_entities.push(entity("proof_obligation", &new_root_obligation_id));
        affected_entities.push(entity("project", project_id));
        sqlx::query("INSERT INTO problem_contract_versions(problem_contract_version_id,project_id,contract_version,contract_json,previous_contract_json,change_reason,changed_by,command_id,created_revision,created_at) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(new_id("contractver")).bind(project_id).bind(next.version).bind(json_text(&next)?)
            .bind(json_text(&previous)?).bind(&request.change_reason).bind(requested_by).bind(&command_id)
            .bind(revision).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        sqlx::query("UPDATE human_commands SET after_revision=?,affected_entities_json=? WHERE command_id=?")
            .bind(revision).bind(json_text(&affected_entities)?).bind(&command_id).execute(&mut *tx).await?;
        let result = ProblemRevisionResult {
            command_id: command_id.clone(),
            contract_version: next.version,
            status: "applied".into(),
            affected_entities: affected_entities.clone(),
        };
        let mut events = fact_invalidation_events;
        events.push(append_event(
            &mut tx,
            project_id,
            revision,
            "problem_contract.revised",
            entity("project", project_id),
            json!({"contract_version":next.version,"affected_entities":affected_entities,"replan":request.replan,"obsolete_obligation_ids":obsolete_obligation_ids,"root_obligation_id":new_root_obligation_id}),
            Some(entity("command", &command_id)),
        ).await?);
        for verification_id in &verification_ids {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "verification.superseded",
                    entity("verification", verification_id),
                    json!({"reason":"problem contract revised","contract_version":next.version}),
                    Some(entity("command", &command_id)),
                )
                .await?,
            );
        }
        for publication_id in &publication_ids {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "publication.stale",
                    entity("publication", publication_id),
                    json!({"reason":"problem contract revised","contract_version":next.version,"source_revision_lte":actual}),
                    Some(entity("command", &command_id)),
                )
                .await?,
            );
        }
        for uncertainty_id in &obsolete_uncertainty_ids {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "uncertainty.obsolete",
                    entity("uncertainty", uncertainty_id),
                    json!({"reason":"introducing verification, task, or route was superseded by problem contract revision"}),
                    Some(entity("command", &command_id)),
                )
                .await?,
            );
        }
        for fact_id in suspended_fact_ids {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "fact.suspended",
                    entity("fact", &fact_id),
                    json!({"reason":"problem contract revision invalidated this Fact or its dependency context"}),
                    Some(entity("command", &command_id)),
                )
                .await?,
            );
        }
        events.push(
            append_event(
                &mut tx,
                project_id,
                revision,
                "goal.reopened",
                entity("goal", &main_goal_id),
                json!({"contract_version":next.version,"statement":next.target_statement}),
                Some(entity("command", &command_id)),
            )
            .await?,
        );
        for obligation_id in &obsolete_obligation_ids {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "proof_obligation.updated",
                    entity("proof_obligation", obligation_id),
                    json!({"status":"obsolete","reason":"problem contract revised","contract_version":next.version}),
                    Some(entity("command", &command_id)),
                )
                .await?,
            );
        }
        for obligation_id in reopened_fact_obligation_ids
            .iter()
            .filter(|id| !obsolete_obligation_ids.contains(id))
        {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "proof_obligation.updated",
                    entity("proof_obligation", obligation_id),
                    json!({"status":"open","reason":"problem revision removed an assumption used by a supporting Fact"}),
                    Some(entity("command", &command_id)),
                )
                .await?,
            );
        }
        events.push(
            append_event(
                &mut tx,
                project_id,
                revision,
                "proof_obligation.created",
                entity("proof_obligation", &new_root_obligation_id),
                json!({"status":"open","necessity":"required","goal_id":main_goal_id,"contract_version":next.version,"source_bottleneck_id":new_bottleneck_id}),
                Some(entity("command", &command_id)),
            )
            .await?,
        );
        events.push(
            append_event(
                &mut tx,
                project_id,
                revision,
                "bottleneck.created",
                entity("bottleneck", &new_bottleneck_id),
                json!({"contract_version":next.version,"supersedes":old_bottleneck_ids}),
                Some(entity("command", &command_id)),
            )
            .await?,
        );
        for route_id in &route_ids {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "route.invalidated",
                    entity("route", route_id),
                    json!({"reason":"problem contract revised"}),
                    Some(entity("command", &command_id)),
                )
                .await?,
            );
        }
        for task_id in &task_ids {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "task.cancelled",
                    entity("task", task_id),
                    json!({"reason":"problem contract revised","stale_submission_rejected":true}),
                    Some(entity("command", &command_id)),
                )
                .await?,
            );
        }
        for result_envelope_id in &result_envelope_ids {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "result_envelope.stale",
                    entity("result_envelope", result_envelope_id),
                    json!({"reason":"problem contract revised"}),
                    Some(entity("command", &command_id)),
                )
                .await?,
            );
        }
        if request.replan {
            if previous_project_status != "running" {
                events.push(
                    append_event(
                        &mut tx,
                        project_id,
                        revision,
                        "project.reopened",
                        entity("project", project_id),
                        json!({"previous_status":previous_project_status,"reason":"problem contract revised with automatic replanning"}),
                        Some(entity("command", &command_id)),
                    )
                    .await?,
                );
            }
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "replan.queued",
                    entity("project", project_id),
                    json!({"contract_version":next.version}),
                    Some(entity("command", &command_id)),
                )
                .await?,
            );
        } else if previous_project_status != "paused" {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "project.paused",
                    entity("project", project_id),
                    json!({"reason":"problem contract revised without automatic replanning"}),
                    Some(entity("command", &command_id)),
                )
                .await?,
            );
        }
        events.push(
            append_event(
                &mut tx,
                project_id,
                revision,
                "human_command.applied",
                entity("command", &command_id),
                json!({"affected_entities":affected_entities}),
                None,
            )
            .await?,
        );
        let event_cursor = events.last().map_or(0, |event| event.cursor);
        let stored = StoredBoardWrite {
            data: result.clone(),
            project_revision: revision,
            event_cursor,
        };
        save_board_write(
            &mut tx,
            project_id,
            idempotency_key,
            "problem_revision",
            &request_hash,
            &command_id,
            &stored,
        )
        .await?;
        tx.commit().await?;
        Ok(BoardMutation {
            data: result,
            project_revision: revision,
            event_cursor,
            events,
        })
    }

    pub async fn propose_human_route(
        &self,
        project_id: &str,
        request: &HumanRouteProposalRequest,
        requested_by: &str,
        idempotency_key: &str,
    ) -> StorageResult<BoardMutation<HumanRouteProposalResult>> {
        validate_route_proposal(request, idempotency_key)?;
        let request_hash = request_hash("route_proposal", request, requested_by)?;
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "propose_human_route",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        if let Some(stored) = replay_board_write(
            &mut tx,
            project_id,
            idempotency_key,
            "route_proposal",
            &request_hash,
        )
        .await?
        {
            tx.rollback().await?;
            return Ok(BoardMutation {
                data: stored.data,
                project_revision: stored.project_revision,
                event_cursor: stored.event_cursor,
                events: vec![],
            });
        }
        let actual: i64 = sqlx::query_scalar("SELECT revision FROM projects WHERE project_id=?")
            .bind(project_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "project",
                id: project_id.into(),
            })?;
        if actual != request.expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: request.expected_revision,
                actual,
            });
        }
        for goal_id in &request.target_goal_ids {
            let exists: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM goals WHERE project_id=? AND goal_id=?")
                    .bind(project_id)
                    .bind(goal_id)
                    .fetch_one(&mut *tx)
                    .await?;
            if exists != 1 {
                return Err(StorageError::InvalidRouteProposal(format!(
                    "target goal {goal_id} is not in the project"
                )));
            }
        }
        for fact_id in &request.required_fact_ids {
            let active: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM facts WHERE project_id=? AND fact_id=? AND status='active'",
            )
            .bind(project_id)
            .bind(fact_id)
            .fetch_one(&mut *tx)
            .await?;
            if active != 1 {
                return Err(StorageError::InvalidDependency(fact_id.clone()));
            }
        }
        let duplicate: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM human_route_proposals WHERE project_id=? AND title=? AND method_summary=? AND status='queued'")
            .bind(project_id).bind(request.title.trim()).bind(request.method_summary.trim()).fetch_one(&mut *tx).await?;
        if duplicate > 0 {
            return Err(StorageError::InvalidRouteProposal(
                "an equivalent human route proposal is already queued".into(),
            ));
        }

        let command_id = new_id("cmd");
        let proposal_id = new_id("routeproposal");
        let now = Utc::now();
        insert_applied_command(
            &mut tx,
            project_id,
            &command_id,
            "propose_route",
            "route_proposal",
            &proposal_id,
            actual,
            idempotency_key,
            &request.reason,
            requested_by,
            &serde_json::to_value(request)?,
            actual,
            now.to_rfc3339().as_str(),
        )
        .await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        sqlx::query("INSERT INTO human_route_proposals(proposal_id,project_id,title,method_summary,target_goal_ids_json,required_fact_ids_json,known_risks_json,reason,proposed_by,command_id,status,created_revision,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,'queued',?,?)")
            .bind(&proposal_id).bind(project_id).bind(request.title.trim()).bind(request.method_summary.trim())
            .bind(json_text(&request.target_goal_ids)?).bind(json_text(&request.required_fact_ids)?).bind(json_text(&request.known_risks)?)
            .bind(&request.reason).bind(requested_by).bind(&command_id).bind(revision).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        let affected = vec![entity("route_proposal", &proposal_id)];
        sqlx::query("UPDATE human_commands SET after_revision=?,affected_entities_json=? WHERE command_id=?")
            .bind(revision).bind(json_text(&affected)?).bind(&command_id).execute(&mut *tx).await?;
        let result = HumanRouteProposalResult {
            proposal_id: proposal_id.clone(),
            command_id: command_id.clone(),
            status: "queued".into(),
            route_id: None,
        };
        let events = vec![
            append_event(
                &mut tx,
                project_id,
                revision,
                "route.proposal.queued",
                entity("route_proposal", &proposal_id),
                json!({"title":request.title,"target_goal_ids":request.target_goal_ids}),
                Some(entity("command", &command_id)),
            )
            .await?,
            append_event(
                &mut tx,
                project_id,
                revision,
                "human_command.applied",
                entity("command", &command_id),
                json!({"affected_entities":affected}),
                None,
            )
            .await?,
        ];
        let event_cursor = events.last().map_or(0, |event| event.cursor);
        let stored = StoredBoardWrite {
            data: result.clone(),
            project_revision: revision,
            event_cursor,
        };
        save_board_write(
            &mut tx,
            project_id,
            idempotency_key,
            "route_proposal",
            &request_hash,
            &command_id,
            &stored,
        )
        .await?;
        tx.commit().await?;
        Ok(BoardMutation {
            data: result,
            project_revision: revision,
            event_cursor,
            events,
        })
    }

    /// Atomically materializes a human-authored route through the same immutable
    /// V2 task boundary used by Planner output. Human authorship authorizes route
    /// execution; it never promotes the eventual mathematical output to Fact.
    #[allow(clippy::too_many_lines)]
    pub async fn create_human_route(
        &self,
        project_id: &str,
        request: &HumanRouteCreateRequest,
        requested_by: &str,
        idempotency_key: &str,
    ) -> StorageResult<BoardMutation<HumanRouteCreateResult>> {
        validate_human_route_create(request, idempotency_key)?;
        let request_hash = request_hash("human_route_create", request, requested_by)?;
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "create_human_route",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        if let Some(stored) = replay_board_write(
            &mut tx,
            project_id,
            idempotency_key,
            "human_route_create",
            &request_hash,
        )
        .await?
        {
            tx.rollback().await?;
            return Ok(BoardMutation {
                data: stored.data,
                project_revision: stored.project_revision,
                event_cursor: stored.event_cursor,
                events: vec![],
            });
        }

        let project_row = sqlx::query(
            "SELECT problem_contract_json,budget_json,status,revision,current_round FROM projects WHERE project_id=?",
        )
        .bind(project_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "project",
            id: project_id.into(),
        })?;
        let actual: i64 = project_row.try_get("revision")?;
        if actual != request.expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: request.expected_revision,
                actual,
            });
        }
        let project_status: String = project_row.try_get("status")?;
        if !matches!(
            project_status.as_str(),
            "created" | "running" | "paused" | "needs_human_review"
        ) {
            return Err(StorageError::InvalidRouteProposal(format!(
                "a human route cannot start while project is {project_status}"
            )));
        }
        let budget: Budget = serde_json::from_str(project_row.try_get("budget_json")?)?;
        validate_project_budget(&budget)?;
        let used_calls: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(model_calls),0) FROM usage_records WHERE project_id=?",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        if used_calls >= i64::from(budget.max_total_model_calls) {
            return Err(StorageError::BudgetExhausted(format!(
                "project already used {used_calls} of {} model calls",
                budget.max_total_model_calls
            )));
        }
        let problem: ProblemContract =
            serde_json::from_str(project_row.try_get("problem_contract_json")?)?;
        let main_goal_id: String = sqlx::query_scalar(
            "SELECT goal_id FROM goals WHERE project_id=? ORDER BY priority DESC,goal_id LIMIT 1",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        let target_goal_ids = if request.target_goal_ids.is_empty() {
            vec![main_goal_id]
        } else {
            request.target_goal_ids.clone()
        };
        for goal_id in &target_goal_ids {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM goals WHERE project_id=? AND goal_id=? AND status<>'superseded'",
            )
            .bind(project_id)
            .bind(goal_id)
            .fetch_one(&mut *tx)
            .await?;
            if exists != 1 {
                return Err(StorageError::InvalidRouteProposal(format!(
                    "target goal {goal_id} is missing or superseded"
                )));
            }
        }
        for fact_id in &request.required_fact_ids {
            let active: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM facts WHERE project_id=? AND fact_id=? AND status='active'",
            )
            .bind(project_id)
            .bind(fact_id)
            .fetch_one(&mut *tx)
            .await?;
            if active != 1 {
                return Err(StorageError::InvalidDependency(fact_id.clone()));
            }
        }
        let fingerprint = route_fingerprint(
            &target_goal_ids,
            request.method_summary.trim(),
            &request.required_fact_ids,
        )?;
        let duplicate: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM routes WHERE project_id=? AND semantic_fingerprint=? AND status NOT IN ('merged','pruned','human_stopped','superseded')")
            .bind(project_id)
            .bind(&fingerprint)
            .fetch_one(&mut *tx)
            .await?;
        if duplicate > 0 {
            return Err(StorageError::InvalidRouteProposal(
                "an equivalent live route already exists".into(),
            ));
        }
        let tombstoned: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM route_tombstones WHERE project_id=? AND semantic_fingerprint=? AND revived_revision IS NULL")
            .bind(project_id)
            .bind(&fingerprint)
            .fetch_one(&mut *tx)
            .await?;
        if tombstoned > 0 {
            return Err(StorageError::InvalidRouteProposal(
                "an equivalent route is tombstoned and must be explicitly revived".into(),
            ));
        }
        let live_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM routes WHERE project_id=? AND status IN ('incubating','active','blocked','probation','revived')")
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await?;
        if live_count >= 6 {
            return Err(StorageError::InvalidRouteProposal(
                "live route capacity of six is exhausted".into(),
            ));
        }
        for goal_id in &target_goal_ids {
            let per_goal: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM routes r,json_each(r.target_goal_ids_json) goal WHERE r.project_id=? AND r.status IN ('incubating','active','blocked','probation','revived') AND goal.value=?")
                .bind(project_id)
                .bind(goal_id)
                .fetch_one(&mut *tx)
                .await?;
            if per_goal >= 3 {
                return Err(StorageError::InvalidRouteProposal(format!(
                    "target goal {goal_id} already has three live routes"
                )));
            }
        }
        let family_key = route_family_key(&target_goal_ids, request.method_summary.trim())?;
        if let Some(existing_route_id) = sqlx::query_scalar::<_, String>("SELECT r.route_id FROM routes r JOIN route_families f ON f.family_id=r.family_id WHERE r.project_id=? AND f.semantic_key=? AND r.status IN ('incubating','active','blocked','probation','revived') LIMIT 1")
            .bind(project_id)
            .bind(&family_key)
            .fetch_optional(&mut *tx)
            .await?
        {
            return Err(StorageError::InvalidRouteProposal(format!(
                "route family already has live route {existing_route_id}"
            )));
        }

        let now = Utc::now();
        let current_round: i64 = project_row.try_get("current_round")?;
        let active_round = sqlx::query("SELECT round_id,number,status FROM rounds WHERE project_id=? AND status IN ('created','planning','running','verifying','publishing') ORDER BY number DESC LIMIT 1")
            .bind(project_id)
            .fetch_optional(&mut *tx)
            .await?;
        let (round_id, round_number) = if let Some(row) = active_round {
            let status: String = row.try_get("status")?;
            if status == "planning" {
                let interrupted_id: String = row.try_get("round_id")?;
                sqlx::query("UPDATE rounds SET status='interrupted',completed_at=?,summary='Superseded by an immediately executed human route' WHERE round_id=?")
                    .bind(now.to_rfc3339())
                    .bind(&interrupted_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE planning_stage_attempts SET status='failed',error='superseded by an immediately executed human route',last_heartbeat_at=?,completed_at=? WHERE round_id=? AND status='running'")
                    .bind(now.to_rfc3339())
                    .bind(now.to_rfc3339())
                    .bind(&interrupted_id)
                    .execute(&mut *tx)
                    .await?;
                if current_round >= i64::from(budget.max_rounds) {
                    return Err(StorageError::BudgetExhausted(
                        "no research round remains for the human route".into(),
                    ));
                }
                let number = current_round + 1;
                let id = new_id("round");
                sqlx::query("INSERT INTO rounds(round_id,project_id,number,status,based_on_revision,started_at) VALUES(?, ?,?,'running',?,?)")
                    .bind(&id)
                    .bind(project_id)
                    .bind(number)
                    .bind(actual)
                    .bind(now.to_rfc3339())
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE projects SET current_round=? WHERE project_id=?")
                    .bind(number)
                    .bind(project_id)
                    .execute(&mut *tx)
                    .await?;
                (id, number)
            } else {
                let id: String = row.try_get("round_id")?;
                let number: i64 = row.try_get("number")?;
                sqlx::query("UPDATE rounds SET status='running' WHERE round_id=?")
                    .bind(&id)
                    .execute(&mut *tx)
                    .await?;
                (id, number)
            }
        } else {
            if current_round >= i64::from(budget.max_rounds) {
                return Err(StorageError::BudgetExhausted(
                    "no research round remains for the human route".into(),
                ));
            }
            let number = current_round + 1;
            let id = new_id("round");
            sqlx::query("INSERT INTO rounds(round_id,project_id,number,status,based_on_revision,started_at) VALUES(?, ?,?,'running',?,?)")
                .bind(&id)
                .bind(project_id)
                .bind(number)
                .bind(actual)
                .bind(now.to_rfc3339())
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE projects SET current_round=? WHERE project_id=?")
                .bind(number)
                .bind(project_id)
                .execute(&mut *tx)
                .await?;
            (id, number)
        };

        let route_id = new_id("route");
        let plan_revision_id = new_id("planrev");
        let command_id = new_id("cmd");
        let family_id = if let Some(id) = sqlx::query_scalar::<_, String>(
            "SELECT family_id FROM route_families WHERE project_id=? AND semantic_key=?",
        )
        .bind(project_id)
        .bind(&family_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            id
        } else {
            let id = new_id("routefamily");
            sqlx::query("INSERT INTO route_families(family_id,project_id,semantic_key,status,created_revision,updated_revision) VALUES(?,?,?,'active',?,?)")
                .bind(&id)
                .bind(project_id)
                .bind(&family_key)
                .bind(actual)
                .bind(actual)
                .execute(&mut *tx)
                .await?;
            id
        };
        let ordinal: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(ordinal),0)+1 FROM plan_revisions WHERE project_id=?",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        let consumed_watermark: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(consumed_delta_to),0) FROM plan_revisions WHERE project_id=? AND status='committed'")
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await?;
        insert_applied_command(
            &mut tx,
            project_id,
            &command_id,
            "create_human_route",
            "route",
            &route_id,
            actual,
            idempotency_key,
            &request.reason,
            requested_by,
            &serde_json::to_value(request)?,
            actual,
            &now.to_rfc3339(),
        )
        .await?;
        sqlx::query("INSERT INTO plan_revisions(plan_revision_id,project_id,ordinal,round_id,based_on_project_revision,consumed_delta_from,consumed_delta_to,planner_mode,route_decisions_json,bottleneck_updates_json,task_contract_ids_json,status,rationale,created_at,committed_at) VALUES(?,?,?,?,?,?,?,'human_direct','[]','[]','[]','preparing',?,?,NULL)")
            .bind(&plan_revision_id)
            .bind(project_id)
            .bind(ordinal)
            .bind(&round_id)
            .bind(actual)
            .bind(consumed_watermark)
            .bind(consumed_watermark)
            .bind(format!("Human-authored route: {}", request.reason.trim()))
            .bind(now.to_rfc3339())
            .execute(&mut *tx)
            .await?;
        let attributes = json!({
            "source": "human",
            "requested_by": requested_by,
            "risks": request.known_risks,
            "approach_kind": if request.approach_kind.trim().is_empty() { "other" } else { request.approach_kind.trim() },
            "route_role": if request.route_role.trim().is_empty() { "primary" } else { request.route_role.trim() },
            "user_title": request.title.trim(),
            "plain_language_summary": if request.plain_language_summary.trim().is_empty() { request.method_summary.trim() } else { request.plain_language_summary.trim() },
            "why_this_route": request.reason.trim(),
            "expected_output": request.completion_contract.trim(),
            "relation_to_goal": "Human-authored route targeting the selected goals",
            "steps": request.steps,
            "mathematical_endorsement": false,
        });
        let exit_criteria = vec![
            request.completion_contract.trim().to_owned(),
            "verified counterexample invalidates a necessary premise".to_owned(),
            "explicit human stop or a later Planner prune decision".to_owned(),
        ];
        sqlx::query("INSERT INTO routes(route_id,project_id,title,method_summary,target_goal_ids_json,required_fact_ids_json,status,score,priority,cancellation_epoch,created_in_round,attributes_json,family_id,semantic_fingerprint,created_at_revision,exit_criteria_json,human_review) VALUES(?,?,?,?,?,?,'active',?,?,0,?,?,?,?,?,?,'approved')")
            .bind(&route_id)
            .bind(project_id)
            .bind(request.title.trim())
            .bind(request.method_summary.trim())
            .bind(json_text(&target_goal_ids)?)
            .bind(json_text(&request.required_fact_ids)?)
            .bind(request.priority)
            .bind(request.priority)
            .bind(round_number)
            .bind(json_text(&attributes)?)
            .bind(&family_id)
            .bind(&fingerprint)
            .bind(actual)
            .bind(json_text(&exit_criteria)?)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE route_families SET canonical_route_id=COALESCE(canonical_route_id,?),updated_revision=? WHERE family_id=?")
            .bind(&route_id)
            .bind(actual)
            .bind(&family_id)
            .execute(&mut *tx)
            .await?;
        let hypothesis_id = new_id("hyp");
        sqlx::query("INSERT INTO hypotheses(hypothesis_id,project_id,kind,statement,status,route_id,created_in_round,attributes_json) VALUES(?,?,'route',?,'active',?,?,?)")
            .bind(&hypothesis_id)
            .bind(project_id)
            .bind(request.method_summary.trim())
            .bind(&route_id)
            .bind(round_number)
            .bind(json_text(&attributes)?)
            .execute(&mut *tx)
            .await?;
        for goal_id in &target_goal_ids {
            sqlx::query("INSERT OR IGNORE INTO hypothesis_edges(edge_id,project_id,source_id,target_id,kind) VALUES(?,?,?,?, 'targets')")
                .bind(new_id("edge"))
                .bind(project_id)
                .bind(&route_id)
                .bind(goal_id)
                .execute(&mut *tx)
                .await?;
        }

        let route_row = sqlx::query("SELECT * FROM routes WHERE route_id=?")
            .bind(&route_id)
            .fetch_one(&mut *tx)
            .await?;
        let route = crate::rows::route(&route_row)?;
        let assignment = AssignmentDraft {
            route_index: 0,
            worker_role: request.worker_role.trim().to_owned(),
            strategic_role: "human_direct".into(),
            addresses_interface_debt: true,
            goal_ids: target_goal_ids.clone(),
            objective: request.objective.trim().to_owned(),
            completion_contract: request.completion_contract.trim().to_owned(),
            priority: request.priority,
        };
        let materialized = materialize_v2_task_tx(
            &mut tx,
            V2TaskMaterialization {
                project_id,
                route: &route,
                assignment: &assignment,
                problem_contract: &problem,
                budget: &budget,
                round_number,
                plan_revision_id: &plan_revision_id,
                source_revision: actual,
                source_command_id: Some(&command_id),
                now: &now,
            },
        )
        .await?
        .ok_or_else(|| {
            StorageError::InvalidRouteProposal("an equivalent active task already exists".into())
        })?;
        let task_id = materialized.task.task_id.clone();
        let worker_id = materialized.worker.worker_id.clone();
        let packet_id = materialized.task.context_packet_id.clone().ok_or_else(|| {
            StorageError::InvalidRouteProposal(
                "V2 task materialization omitted its context packet".into(),
            )
        })?;
        let signature = materialized.task.task_signature.clone().ok_or_else(|| {
            StorageError::InvalidRouteProposal(
                "V2 task materialization omitted its task signature".into(),
            )
        })?;
        let task_contract_id = materialized.task_contract_id;

        sqlx::query("UPDATE plan_revisions SET route_decisions_json=?,task_contract_ids_json=?,status='committed',committed_at=? WHERE plan_revision_id=?")
            .bind(json_text(&vec![json!({"route_id": route_id, "decision": "human_created", "human_approved": true, "mathematical_endorsement": false})])?)
            .bind(json_text(&vec![task_contract_id.clone()])?)
            .bind(now.to_rfc3339())
            .bind(&plan_revision_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE projects SET status=CASE WHEN EXISTS (SELECT 1 FROM human_questions WHERE project_id=? AND status='open') OR EXISTS (SELECT 1 FROM routes WHERE project_id=? AND human_review='pending') THEN 'needs_human_review' ELSE 'running' END WHERE project_id=?")
            .bind(project_id)
            .bind(project_id)
            .bind(project_id)
            .execute(&mut *tx)
            .await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        let affected = vec![
            entity("route", &route_id),
            entity("task", &task_id),
            entity("worker", &worker_id),
            entity("context_packet", &packet_id),
            entity("task_contract", &task_contract_id),
            entity("plan_revision", &plan_revision_id),
        ];
        sqlx::query("UPDATE human_commands SET after_revision=?,affected_entities_json=? WHERE command_id=?")
            .bind(revision)
            .bind(json_text(&affected)?)
            .bind(&command_id)
            .execute(&mut *tx)
            .await?;
        let result = HumanRouteCreateResult {
            command_id: command_id.clone(),
            route_id: route_id.clone(),
            task_id: task_id.clone(),
            plan_revision_id: plan_revision_id.clone(),
            execution_started: true,
            status: "queued".into(),
        };
        let events = vec![
            append_event(
                &mut tx,
                project_id,
                revision,
                "route.human_created",
                entity("route", &route_id),
                json!({"human_review":"approved","mathematical_endorsement":false,"requested_by":requested_by}),
                Some(entity("command", &command_id)),
            )
            .await?,
            append_event(
                &mut tx,
                project_id,
                revision,
                "planning.revision.committed",
                entity("plan_revision", &plan_revision_id),
                json!({"ordinal":ordinal,"planner_mode":"human_direct","round_id":round_id,"routes":1,"tasks":1}),
                Some(entity("command", &command_id)),
            )
            .await?,
            append_event(
                &mut tx,
                project_id,
                revision,
                "task.queued",
                entity("task", &task_id),
                json!({"route_id":route_id,"plan_revision_id":plan_revision_id,"task_signature":signature,"source":"human"}),
                Some(entity("plan_revision", &plan_revision_id)),
            )
            .await?,
            append_event(
                &mut tx,
                project_id,
                revision,
                "context.packet.created",
                entity("context_packet", &packet_id),
                json!({"task_id":task_id,"source_revision":actual}),
                Some(entity("task", &task_id)),
            )
            .await?,
            append_event(
                &mut tx,
                project_id,
                revision,
                "human_command.applied",
                entity("command", &command_id),
                json!({"affected_entities":affected}),
                None,
            )
            .await?,
        ];
        let event_cursor = events.last().map_or(0, |event| event.cursor);
        let stored = StoredBoardWrite {
            data: result.clone(),
            project_revision: revision,
            event_cursor,
        };
        save_board_write(
            &mut tx,
            project_id,
            idempotency_key,
            "human_route_create",
            &request_hash,
            &command_id,
            &stored,
        )
        .await?;
        tx.commit().await?;
        Ok(BoardMutation {
            data: result,
            project_revision: revision,
            event_cursor,
            events,
        })
    }

    pub async fn list_human_route_proposals(
        &self,
        project_id: &str,
    ) -> StorageResult<Vec<HumanRouteProposal>> {
        let rows = sqlx::query("SELECT * FROM human_route_proposals WHERE project_id=? ORDER BY created_at,proposal_id")
            .bind(project_id).fetch_all(self.read_pool()).await?;
        rows.iter().map(route_proposal_from_row).collect()
    }
}

fn validate_problem_revision(request: &ProblemRevisionRequest, key: &str) -> StorageResult<()> {
    if key.trim().is_empty() {
        return Err(StorageError::InvalidProblemRevision(
            "Idempotency-Key must not be empty".into(),
        ));
    }
    if request.target_statement.trim().is_empty()
        || request.success_criteria.trim().is_empty()
        || request.change_reason.trim().is_empty()
    {
        return Err(StorageError::InvalidProblemRevision(
            "target_statement, success_criteria, and change_reason are required".into(),
        ));
    }
    Ok(())
}

fn validate_route_proposal(request: &HumanRouteProposalRequest, key: &str) -> StorageResult<()> {
    if key.trim().is_empty() {
        return Err(StorageError::InvalidRouteProposal(
            "Idempotency-Key must not be empty".into(),
        ));
    }
    if request.title.trim().is_empty()
        || request.method_summary.trim().is_empty()
        || request.reason.trim().is_empty()
    {
        return Err(StorageError::InvalidRouteProposal(
            "title, method_summary, and reason are required".into(),
        ));
    }
    Ok(())
}

fn validate_human_route_create(request: &HumanRouteCreateRequest, key: &str) -> StorageResult<()> {
    if key.trim().is_empty() {
        return Err(StorageError::InvalidRouteProposal(
            "Idempotency-Key must not be empty".into(),
        ));
    }
    if request.title.trim().is_empty()
        || request.method_summary.trim().is_empty()
        || request.objective.trim().is_empty()
        || request.completion_contract.trim().is_empty()
        || request.worker_role.trim().is_empty()
        || request.reason.trim().is_empty()
    {
        return Err(StorageError::InvalidRouteProposal(
            "title, method_summary, objective, completion_contract, worker_role, and reason are required"
                .into(),
        ));
    }
    if !request.priority.is_finite() || !(0.0..=1.0).contains(&request.priority) {
        return Err(StorageError::InvalidRouteProposal(
            "priority must be finite and between 0 and 1".into(),
        ));
    }
    Ok(())
}

fn request_hash<T: Serialize>(action: &str, request: &T, actor: &str) -> StorageResult<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(
        &json!({"action":action,"request":request,"actor":actor}),
    )?)))
}

async fn replay_board_write<T: DeserializeOwned>(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
    key: &str,
    action: &str,
    hash: &str,
) -> StorageResult<Option<StoredBoardWrite<T>>> {
    let row = sqlx::query("SELECT action,request_hash,result_json FROM board_write_requests WHERE project_id=? AND idempotency_key=?")
        .bind(project_id).bind(key).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let old_action: String = row.try_get("action")?;
    let old_hash: String = row.try_get("request_hash")?;
    if old_action != action || old_hash != hash {
        return Err(StorageError::IdempotencyConflict(format!(
            "Idempotency-Key {key} was reused with a different request"
        )));
    }
    Ok(Some(serde_json::from_str(row.try_get("result_json")?)?))
}

async fn save_board_write<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
    key: &str,
    action: &str,
    hash: &str,
    command_id: &str,
    result: &T,
) -> StorageResult<()> {
    sqlx::query("INSERT INTO board_write_requests(project_id,idempotency_key,action,request_hash,command_id,result_json,created_at) VALUES(?,?,?,?,?,?,?)")
        .bind(project_id).bind(key).bind(action).bind(hash).bind(command_id).bind(json_text(result)?)
        .bind(Utc::now().to_rfc3339()).execute(&mut **tx).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_applied_command(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
    command_id: &str,
    command_type: &str,
    target_kind: &str,
    target_id: &str,
    expected_revision: i64,
    idempotency_key: &str,
    reason: &str,
    requested_by: &str,
    payload: &Value,
    before_revision: i64,
    now: &str,
) -> StorageResult<()> {
    sqlx::query("INSERT INTO human_commands(command_id,project_id,type,target_kind,target_id,mode,payload_json,expected_project_revision,idempotency_key,reason,requested_by,status,before_revision,created_at,applied_at) VALUES(?,?,?,?,?,'immediate',?,?,?,?,?,'applied',?,?,?)")
        .bind(command_id).bind(project_id).bind(command_type).bind(target_kind).bind(target_id)
        .bind(json_text(payload)?).bind(expected_revision).bind(idempotency_key).bind(reason).bind(requested_by)
        .bind(before_revision).bind(now).bind(now).execute(&mut **tx).await?;
    Ok(())
}

fn route_proposal_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<HumanRouteProposal> {
    Ok(HumanRouteProposal {
        proposal_id: row.try_get("proposal_id")?,
        project_id: row.try_get("project_id")?,
        title: row.try_get("title")?,
        method_summary: row.try_get("method_summary")?,
        target_goal_ids: serde_json::from_str(row.try_get("target_goal_ids_json")?)?,
        required_fact_ids: serde_json::from_str(row.try_get("required_fact_ids_json")?)?,
        known_risks: serde_json::from_str(row.try_get("known_risks_json")?)?,
        reason: row.try_get("reason")?,
        proposed_by: row.try_get("proposed_by")?,
        command_id: row.try_get("command_id")?,
        status: row.try_get("status")?,
        route_id: row.try_get("route_id")?,
        decision_reason: row.try_get("decision_reason")?,
        created_revision: row.try_get("created_revision")?,
        created_at: row
            .try_get::<String, _>("created_at")?
            .parse()
            .map_err(|error| {
                StorageError::CorruptData(format!("invalid route proposal timestamp: {error}"))
            })?,
        decided_at: row
            .try_get::<Option<String>, _>("decided_at")?
            .map(|value| value.parse())
            .transpose()
            .map_err(|error| {
                StorageError::CorruptData(format!("invalid route proposal timestamp: {error}"))
            })?,
    })
}
