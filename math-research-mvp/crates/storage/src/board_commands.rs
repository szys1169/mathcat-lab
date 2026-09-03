use std::collections::HashSet;

use chrono::Utc;
use research_domain::{
    DomainEvent, HumanRouteProposal, HumanRouteProposalRequest, HumanRouteProposalResult,
    ProblemContract, ProblemRevisionRequest, ProblemRevisionResult,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{
    SqliteStore, StorageError, StorageResult, append_event, bump_revision, entity, json_text,
    new_id,
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
        sqlx::query("UPDATE task_leases SET status='cancelled',completed_at=? WHERE task_id IN (SELECT task_id FROM tasks WHERE project_id=? AND status='cancelled') AND status='active'")
            .bind(now.to_rfc3339()).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE task_attempts SET status='cancelled',completed_at=? WHERE task_id IN (SELECT task_id FROM tasks WHERE project_id=? AND status='cancelled') AND status IN ('created','running','checkpointed')")
            .bind(now.to_rfc3339()).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE workers SET status='idle',current_task_id=NULL,current_route_id=NULL WHERE project_id=? AND current_task_id IN (SELECT task_id FROM tasks WHERE project_id=? AND status='cancelled')")
            .bind(project_id).bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE worker_instances SET status='exited',exited_at=? WHERE project_id=? AND status IN ('spawn_requested','starting','handshaking','ready','lease_accepted','running','checkpointing','result_submitted','draining')")
            .bind(now.to_rfc3339()).bind(project_id).execute(&mut *tx).await?;

        let removed_assumptions = previous
            .assumptions
            .iter()
            .filter(|assumption| !next.assumptions.contains(assumption))
            .cloned()
            .collect::<HashSet<_>>();
        let mut suspended_fact_ids = Vec::new();
        if !removed_assumptions.is_empty() {
            let fact_rows = sqlx::query(
                "SELECT fact_id,assumptions_json FROM facts WHERE project_id=? AND status='active'",
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
                    suspended_fact_ids.push(fact.try_get::<String, _>("fact_id")?);
                }
            }
            for fact_id in &suspended_fact_ids {
                sqlx::query("UPDATE facts SET status='suspended' WHERE fact_id=?")
                    .bind(fact_id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
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
        sqlx::query("UPDATE projects SET problem_contract_json=?,status=CASE WHEN ?=0 AND status='running' THEN 'paused' ELSE status END,updated_at=? WHERE project_id=?")
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
            .chain(suspended_fact_ids.iter().map(|id| entity("fact", id)))
            .chain(old_bottleneck_ids.iter().map(|id| entity("bottleneck", id)))
            .collect::<Vec<_>>();
        affected_entities.push(entity("goal", &main_goal_id));
        affected_entities.push(entity("bottleneck", &new_bottleneck_id));
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
        let mut events = vec![append_event(
            &mut tx,
            project_id,
            revision,
            "problem_contract.revised",
            entity("project", project_id),
            json!({"contract_version":next.version,"affected_entities":affected_entities,"replan":request.replan}),
            Some(entity("command", &command_id)),
        ).await?];
        for fact_id in suspended_fact_ids {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "fact.suspended",
                    entity("fact", &fact_id),
                    json!({"reason":"removed problem assumption requires re-verification"}),
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
        if request.replan {
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
        } else if previous_project_status == "running" {
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
