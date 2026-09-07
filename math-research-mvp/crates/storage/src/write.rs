use std::{collections::BTreeSet, path::Path, str::FromStr};

use chrono::Utc;
use research_domain::{
    AcceptanceClass, Artifact, Budget, Candidate, CandidateStatus, CandidateSubmission,
    CandidateType, CommandMode, CommandStatus, DomainEvent, EntityRef, Fact, FactStatus,
    GoalStatus, HumanCommand, ObligationCoverageDisposition, PlannerOutput, ProblemContract,
    Project, ProjectStatus, ResearchRound, ReviewMode, RoundStatus, Route, RouteStatus, Task,
    TaskStatus, UncertaintyStatus, Verification, VerificationReport, VerificationStage,
    VerificationVerdict, Worker, WorkerOutput, WorkerStatus,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{
    SqliteStore, StorageError, StorageResult, append_event, bump_revision, current_revision,
    entity, json_text, new_id, route_attributes,
    route_mutations::{
        cancel_task_execution_tx, merge_routes_tx, prune_route_tx, revive_route_tx, stop_route_tx,
    },
    rows,
};

#[derive(Debug, Clone)]
pub struct CommandDraft {
    pub command_type: String,
    pub target_kind: String,
    pub target_id: String,
    pub mode: CommandMode,
    pub payload: Value,
    pub expected_project_revision: i64,
    pub idempotency_key: String,
    pub reason: String,
    pub requested_by: String,
}

#[derive(Debug, Clone)]
pub struct PlanSaveResult {
    pub round: ResearchRound,
    pub routes: Vec<Route>,
    pub tasks: Vec<Task>,
    pub workers: Vec<Worker>,
    pub events: Vec<DomainEvent>,
}

#[derive(Debug, Clone)]
pub struct UsageReservation {
    pub usage_id: String,
}

#[derive(Debug, Clone, Copy)]
pub enum ModelCallPurpose<'a> {
    Research,
    RouteScopedResearch(&'a str),
    Verification(&'a str),
    Publication(&'a str),
}

#[derive(Debug, Clone, Copy)]
pub struct ModelCallRequest<'a> {
    pub project_id: &'a str,
    pub round: i64,
    pub worker_id: Option<&'a str>,
    pub task_id: Option<&'a str>,
    pub model: Option<&'a str>,
    pub purpose: ModelCallPurpose<'a>,
    pub max_total_calls: u32,
    pub max_task_calls: u32,
}

#[derive(Debug, Clone)]
pub struct SubmissionReceipt {
    pub candidate: Candidate,
    pub verification: Verification,
    pub event: Option<DomainEvent>,
}

#[derive(Debug, Clone)]
pub struct VerificationCommit {
    pub verification: Verification,
    pub fact: Option<Fact>,
    pub events: Vec<DomainEvent>,
}

pub(crate) struct UncommittedArtifactFile {
    path: String,
    committed: bool,
}

impl UncommittedArtifactFile {
    pub(crate) fn new(artifact: &Artifact) -> Self {
        Self {
            path: artifact.storage_path.clone(),
            committed: false,
        }
    }

    pub(crate) const fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for UncommittedArtifactFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub(crate) fn validate_project_input(
    name: &str,
    contract: &ProblemContract,
    budget: &Budget,
) -> StorageResult<()> {
    if name.trim().is_empty() || contract.target_statement.trim().is_empty() {
        return Err(StorageError::InvalidTransition(
            "project name and target statement must not be empty".into(),
        ));
    }
    validate_project_budget(budget)?;
    Ok(())
}

pub(crate) fn validate_project_budget(budget: &Budget) -> StorageResult<()> {
    if budget.max_rounds == 0
        || budget.max_model_calls_per_task == 0
        || budget.max_total_model_calls == 0
    {
        return Err(StorageError::InvalidTransition(
            "max_rounds and model-call limits must be greater than zero".into(),
        ));
    }
    if !(1..=16).contains(&budget.max_parallel_workers) {
        return Err(StorageError::InvalidTransition(
            "max_parallel_workers must be between 1 and 16".into(),
        ));
    }
    if !(1..=1_440).contains(&budget.max_minutes_per_task) {
        return Err(StorageError::InvalidTransition(
            "max_minutes_per_task must be between 1 and 1440".into(),
        ));
    }
    Ok(())
}

/// Creates the complete initial project aggregate inside the caller's transaction.
///
/// Problem-draft confirmation uses this helper so project, Goal, Bottleneck, planner health,
/// and the `project.created` event cannot diverge from the ordinary creation path.
pub(crate) async fn create_project_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    name: &str,
    contract: ProblemContract,
    budget: Budget,
    review_mode: ReviewMode,
) -> StorageResult<(Project, DomainEvent)> {
    validate_project_input(name, &contract, &budget)?;
    let human_route_approval = review_mode == ReviewMode::Strict;
    let project_id = new_id("project");
    let goal_id = new_id("goal");
    let bottleneck_id = new_id("bottleneck");
    let now = Utc::now();
    sqlx::query("INSERT INTO projects(project_id,name,problem_contract_json,status,revision,current_round,budget_json,created_at,updated_at,human_route_approval,review_mode) VALUES(?,?,?,?,1,0,?,?,?,?,?)")
        .bind(&project_id).bind(name.trim()).bind(json_text(&contract)?)
        .bind(ProjectStatus::Created.to_string()).bind(json_text(&budget)?)
        .bind(now.to_rfc3339()).bind(now.to_rfc3339()).bind(human_route_approval)
        .bind(review_mode.to_string()).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO goals(goal_id,project_id,statement,parent_goal_ids_json,status,priority,blocked_by_json,created_in_round) VALUES(?,?,?,?,?,?,?,0)")
        .bind(&goal_id).bind(&project_id).bind(&contract.target_statement).bind("[]")
        .bind(GoalStatus::Open.to_string()).bind(1.0_f64).bind("[]").execute(&mut **tx).await?;
    sqlx::query("INSERT INTO bottlenecks(bottleneck_id,project_id,target_goal_ids_json,kind,precise_statement,completion_contract_json,priority,status,created_revision,updated_revision,created_at,updated_at) VALUES(?,?,?,?,?,?,1.0,'open',1,1,?,?)")
        .bind(&bottleneck_id).bind(&project_id).bind(json_text(&vec![goal_id.clone()])?)
        .bind("main_goal_unresolved").bind(format!("Resolve the main goal exactly as stated: {}", contract.target_statement))
        .bind(json_text(&json!({"success_outputs":["a self-contained candidate proving the exact target, or a verified counterexample"],"partial_outputs":["a strictly narrower named lemma with an explicit implication to the target"],"reject_outputs":["generic discussion","a silently weakened target","an unsupported solved claim"]}))?)
        .bind(now.to_rfc3339()).bind(now.to_rfc3339()).execute(&mut **tx).await?;
    let root_obligation_id = crate::proof_obligations::create_root_obligation_tx(
        tx,
        &project_id,
        &goal_id,
        &bottleneck_id,
        &contract.target_statement,
        1.0,
        1,
        &json!({"source":"project_creation","goal_id":goal_id}),
        &now,
    )
    .await?;
    sqlx::query("INSERT INTO planner_health(project_id,updated_at) VALUES(?,?)")
        .bind(&project_id)
        .bind(now.to_rfc3339())
        .execute(&mut **tx)
        .await?;
    let event = append_event(
        tx,
        &project_id,
        1,
        "project.created",
        entity("project", &project_id),
        json!({"main_goal_id": goal_id,"initial_bottleneck_id":bottleneck_id,"root_obligation_id":root_obligation_id,"human_route_approval":human_route_approval}),
        None,
    )
    .await?;
    Ok((
        Project {
            project_id,
            name: name.trim().into(),
            contract,
            status: ProjectStatus::Created,
            revision: 1,
            current_round: 0,
            budget,
            review_mode,
            human_route_approval,
            created_at: now,
            updated_at: now,
        },
        event,
    ))
}

pub(crate) async fn enqueue_command_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
    draft: CommandDraft,
) -> StorageResult<(HumanCommand, DomainEvent)> {
    if draft.idempotency_key.trim().is_empty() {
        return Err(StorageError::InvalidTransition(
            "Idempotency-Key must not be empty".into(),
        ));
    }
    let actual = current_revision(tx, project_id).await?;
    if actual != draft.expected_project_revision {
        return Err(StorageError::RevisionConflict {
            expected: draft.expected_project_revision,
            actual,
        });
    }
    let command_id = new_id("cmd");
    let now = Utc::now();
    sqlx::query("INSERT INTO human_commands(command_id,project_id,type,target_kind,target_id,mode,payload_json,expected_project_revision,idempotency_key,reason,requested_by,status,before_revision,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(&command_id).bind(project_id).bind(&draft.command_type).bind(&draft.target_kind).bind(&draft.target_id)
        .bind(draft.mode.to_string()).bind(json_text(&draft.payload)?).bind(draft.expected_project_revision)
        .bind(&draft.idempotency_key).bind(&draft.reason).bind(&draft.requested_by).bind(CommandStatus::Queued.to_string())
        .bind(actual).bind(now.to_rfc3339()).execute(&mut **tx).await?;
    let revision = bump_revision(tx, project_id).await?;
    let event = append_event(
        tx,
        project_id,
        revision,
        "human_command.queued",
        entity("command", &command_id),
        json!({"type": draft.command_type, "mode": draft.mode}),
        None,
    )
    .await?;
    let command = HumanCommand {
        command_id,
        project_id: project_id.into(),
        command_type: draft.command_type,
        target_kind: draft.target_kind,
        target_id: draft.target_id,
        mode: draft.mode,
        payload: draft.payload,
        expected_project_revision: draft.expected_project_revision,
        idempotency_key: draft.idempotency_key,
        reason: draft.reason,
        requested_by: draft.requested_by,
        status: CommandStatus::Queued,
        before_revision: Some(actual),
        after_revision: None,
        affected_entities: vec![],
        error: None,
        created_at: now,
        applied_at: None,
    };
    Ok((command, event))
}

impl SqliteStore {
    pub async fn create_project(
        &self,
        name: String,
        contract: ProblemContract,
        budget: Budget,
    ) -> StorageResult<(Project, DomainEvent)> {
        self.create_project_with_route_approval(name, contract, budget, false)
            .await
    }

    pub async fn create_project_with_route_approval(
        &self,
        name: String,
        contract: ProblemContract,
        budget: Budget,
        human_route_approval: bool,
    ) -> StorageResult<(Project, DomainEvent)> {
        let review_mode = if human_route_approval {
            ReviewMode::Strict
        } else {
            ReviewMode::Balanced
        };
        self.create_project_with_review_mode(name, contract, budget, review_mode)
            .await
    }

    pub async fn create_project_with_review_mode(
        &self,
        name: String,
        contract: ProblemContract,
        budget: Budget,
        review_mode: ReviewMode,
    ) -> StorageResult<(Project, DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "create_project",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let result = create_project_tx(&mut tx, &name, contract, budget, review_mode).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn enqueue_command(
        &self,
        project_id: &str,
        draft: CommandDraft,
    ) -> StorageResult<(HumanCommand, Option<DomainEvent>)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "enqueue_command",
            )
            .await?;
        if draft.idempotency_key.trim().is_empty() {
            return Err(StorageError::InvalidTransition(
                "Idempotency-Key must not be empty".into(),
            ));
        }
        if let Some(existing) = self
            .command_by_idempotency(project_id, &draft.idempotency_key)
            .await?
        {
            if existing.command_type != draft.command_type
                || existing.target_kind != draft.target_kind
                || existing.target_id != draft.target_id
                || existing.mode != draft.mode
                || existing.payload != draft.payload
                || existing.expected_project_revision != draft.expected_project_revision
                || existing.reason != draft.reason
                || existing.requested_by != draft.requested_by
            {
                return Err(StorageError::IdempotencyConflict(format!(
                    "Idempotency-Key {} was reused with a different command",
                    draft.idempotency_key
                )));
            }
            return Ok((existing, None));
        }
        let mut tx = self.pool().begin().await?;
        let result = enqueue_command_tx(&mut tx, project_id, draft).await?;
        tx.commit().await?;
        Ok((result.0, Some(result.1)))
    }

    pub async fn apply_command(
        &self,
        project_id: &str,
        command_id: &str,
    ) -> StorageResult<(HumanCommand, Vec<DomainEvent>)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "apply_command",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row =
            sqlx::query("SELECT * FROM human_commands WHERE project_id = ? AND command_id = ?")
                .bind(project_id)
                .bind(command_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StorageError::NotFound {
                    kind: "command",
                    id: command_id.into(),
                })?;
        let mut command = rows::command(&row)?;
        if command.status != CommandStatus::Queued {
            tx.rollback().await?;
            return Ok((command, vec![]));
        }
        let mut affected = Vec::<EntityRef>::new();
        let mut event_specs = Vec::<(String, EntityRef, Value)>::new();
        match command.command_type.as_str() {
            "start_project" => {
                update_project_status(
                    &mut tx,
                    project_id,
                    &[ProjectStatus::Created],
                    ProjectStatus::Running,
                )
                .await?;
                affected.push(entity("project", project_id));
                event_specs.push((
                    "project.started".into(),
                    entity("project", project_id),
                    json!({}),
                ));
            }
            "pause_project" => {
                update_project_status(
                    &mut tx,
                    project_id,
                    &[ProjectStatus::Running],
                    ProjectStatus::Paused,
                )
                .await?;
                affected.push(entity("project", project_id));
                event_specs.push((
                    "project.paused".into(),
                    entity("project", project_id),
                    json!({}),
                ));
            }
            "resume_project" => {
                // Explicit operator resume starts fresh scheduling after a stop;
                // cancelled attempts, route epochs and verification fences stay intact.
                let row = sqlx::query(
                    "SELECT status,budget_json,current_round FROM projects WHERE project_id=?",
                )
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?;
                let status: String = row.try_get("status")?;
                if status == "partial_success" {
                    let budget: Budget = serde_json::from_str(row.try_get("budget_json")?)?;
                    let round: i64 = row.try_get("current_round")?;
                    let calls: i64 = sqlx::query_scalar(
                        "SELECT COALESCE(SUM(model_calls),0) FROM usage_records WHERE project_id=?",
                    )
                    .bind(project_id)
                    .fetch_one(&mut *tx)
                    .await?;
                    let exhausted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM events WHERE project_id=? AND type='project.budget_exhausted')")
                        .bind(project_id).fetch_one(&mut *tx).await?;
                    if !exhausted
                        || round >= i64::from(budget.max_rounds)
                        || calls >= i64::from(budget.max_total_model_calls)
                    {
                        return Err(StorageError::InvalidTransition(
                            "budget-limited research requires additional round and call headroom before resume".into(),
                        ));
                    }
                }
                update_project_status(
                    &mut tx,
                    project_id,
                    &[
                        ProjectStatus::Paused,
                        ProjectStatus::StoppedByHuman,
                        ProjectStatus::PartialSuccess,
                    ],
                    ProjectStatus::Running,
                )
                .await?;
                affected.push(entity("project", project_id));
                event_specs.push((
                    "project.resumed".into(),
                    entity("project", project_id),
                    json!({}),
                ));
            }
            "stop_project" => {
                update_project_status(
                    &mut tx,
                    project_id,
                    &[
                        ProjectStatus::Created,
                        ProjectStatus::Running,
                        ProjectStatus::Paused,
                        ProjectStatus::NeedsHumanReview,
                    ],
                    ProjectStatus::StoppedByHuman,
                )
                .await?;
                let now = Utc::now().to_rfc3339();
                let task_ids = sqlx::query_scalar::<_, String>("SELECT task_id FROM tasks WHERE project_id=? AND status IN ('open','queued','assigned','offered','leased','running','checkpointed','result_submitted','ingesting','paused','blocked') ORDER BY task_id")
                    .bind(project_id).fetch_all(&mut *tx).await?;
                let stopped_tasks = sqlx::query("UPDATE tasks SET status=?,revision=revision+1 WHERE project_id=? AND status IN ('open','queued','assigned','offered','leased','running','checkpointed','result_submitted','ingesting','paused','blocked')")
                    .bind(TaskStatus::HumanStopped.to_string()).bind(project_id).execute(&mut *tx).await?.rows_affected();
                let stopped_routes = sqlx::query("UPDATE routes SET status='human_stopped',cancellation_epoch=cancellation_epoch+1 WHERE project_id=? AND status IN ('proposed','incubating','active','blocked','probation','paused','revived')")
                    .bind(project_id).execute(&mut *tx).await?.rows_affected();
                sqlx::query("UPDATE task_leases SET status='cancelled',completed_at=? WHERE project_id=? AND status='active'")
                    .bind(&now).bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE result_envelopes SET status='stale',rejection_reason='project stopped by human' WHERE project_id=? AND status='submitted'")
                    .bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE task_attempts SET status='orphaned',failure_reason='project stopped by human',completed_at=? WHERE project_id=? AND status NOT IN ('completed','failed','orphaned','cancelled')")
                    .bind(&now).bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE worker_instances SET status='exited',quarantine_reason='project stopped by human',exited_at=? WHERE project_id=? AND status NOT IN ('exited','quarantined')")
                    .bind(&now).bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE rounds SET status='interrupted',completed_at=?,summary='project stopped by human' WHERE project_id=? AND status IN ('created','planning','running','verifying','publishing')")
                    .bind(&now).bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE planning_stage_attempts SET status='failed',error='project stopped by human',last_heartbeat_at=?,completed_at=? WHERE project_id=? AND status='running'")
                    .bind(&now).bind(&now).bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE usage_records SET outcome='cancelled',error_kind='human_stop',error_message='project stopped by human' WHERE project_id=? AND outcome='running'")
                    .bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE publication_runs SET status='failed',error='project stopped by human',updated_at=?,completed_at=? WHERE project_id=? AND status='running'")
                    .bind(&now).bind(&now).bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE workers SET status = ?, current_task_id = NULL, current_route_id = NULL WHERE project_id = ? AND status IN ('idle','running','paused')")
                    .bind(WorkerStatus::Stopped.to_string()).bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE task_steers SET status='stale',applied_at=? WHERE project_id=? AND status='pending'")
                    .bind(&now).bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE suggestions SET status='rejected',decision=? WHERE project_id=? AND status='pending'")
                    .bind(json_text(&json!({"disposition":"rejected","rationale":"project stopped by human"}))?)
                    .bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE human_route_proposals SET status='rejected',decision_reason='project stopped by human',decided_at=? WHERE project_id=? AND status='queued'")
                    .bind(&now).bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE human_commands SET status='superseded',error='project stopped by human',applied_at=? WHERE project_id=? AND command_id<>? AND status IN ('queued','validated','waiting_safe_point')")
                    .bind(&now).bind(project_id).bind(command_id).execute(&mut *tx).await?;
                let interrupted_report = json_text(&VerificationReport {
                    verdict: VerificationVerdict::Unknown,
                    summary:
                        "Verification was interrupted because the project was stopped by a human."
                            .into(),
                    critical_errors: Vec::new(),
                    gaps: Vec::new(),
                    uncertainties: vec![
                        "No mathematical verdict was committed before the stop boundary.".into(),
                    ],
                    repair_actions: Vec::new(),
                    checked_fact_ids: Vec::new(),
                    checked_source_ids: Vec::new(),
                    evidence_level: "system_interrupted".into(),
                })?;
                let stopped_verifications = sqlx::query("UPDATE verifications SET status='unknown',report_json=?,completed_at=?,started_at=COALESCE(started_at,?) WHERE project_id=? AND status IN ('submitted','prechecking','verifying')")
                    .bind(&interrupted_report).bind(&now).bind(&now).bind(project_id)
                    .execute(&mut *tx).await?.rows_affected();
                sqlx::query("UPDATE candidates SET status='unknown' WHERE project_id=? AND status IN ('submitted','prechecking','verifying')")
                    .bind(project_id).execute(&mut *tx).await?;
                let stopped_proof_searches = sqlx::query("UPDATE proof_searches SET status='cancelled',cancellation_epoch=cancellation_epoch+1,completed_at=? WHERE status='running' AND case_id IN (SELECT case_id FROM verification_cases WHERE project_id=?)")
                    .bind(&now).bind(project_id).execute(&mut *tx).await?.rows_affected();
                sqlx::query("UPDATE proof_nodes SET status='cancelled',cancellation_epoch=(SELECT cancellation_epoch FROM proof_searches WHERE proof_searches.search_id=proof_nodes.search_id),updated_at=? WHERE status IN ('open','running') AND search_id IN (SELECT ps.search_id FROM proof_searches ps JOIN verification_cases vc ON vc.case_id=ps.case_id WHERE vc.project_id=?)")
                    .bind(&now).bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE verification_cases SET stage='cancelled',cancellation_epoch=cancellation_epoch+1,completed_at=?,updated_at=? WHERE project_id=? AND stage NOT IN ('committed','rejected','unknown','failed','cancelled')")
                    .bind(&now).bind(&now).bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE verification_task_leases SET status='cancelled',completed_at=? WHERE project_id=? AND status='active'")
                    .bind(&now).bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE verification_attempts SET status='cancelled',error_kind='human_stop',error_message='project stopped by human',completed_at=? WHERE case_id IN (SELECT case_id FROM verification_cases WHERE project_id=?) AND status NOT IN ('completed','failed','orphaned','cancelled')")
                    .bind(&now).bind(project_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE verification_result_envelopes SET status='stale',rejection_reason='project stopped by human' WHERE project_id=? AND status='submitted'")
                    .bind(project_id).execute(&mut *tx).await?;
                affected.extend(task_ids.iter().map(|task_id| entity("task", task_id)));
                affected.push(entity("project", project_id));
                event_specs.push((
                    "project.stopped".into(),
                    entity("project", project_id),
                    json!({"stopped_task_count":stopped_tasks,"stopped_route_count":stopped_routes,"stopped_verification_count":stopped_verifications,"stopped_proof_search_count":stopped_proof_searches}),
                ));
            }
            "pause_route" | "resume_route" | "stop_route" => {
                let route_id = &command.target_id;
                let status: String = sqlx::query_scalar(
                    "SELECT status FROM routes WHERE project_id = ? AND route_id = ?",
                )
                .bind(project_id)
                .bind(route_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StorageError::NotFound {
                    kind: "route",
                    id: route_id.clone(),
                })?;
                match command.command_type.as_str() {
                    "pause_route" if status == "active" => {
                        sqlx::query("UPDATE routes SET status = 'paused' WHERE route_id = ?")
                            .bind(route_id)
                            .execute(&mut *tx)
                            .await?;
                        event_specs.push((
                            "route.paused".into(),
                            entity("route", route_id),
                            json!({}),
                        ));
                    }
                    "resume_route" if status == "paused" || status == "human_stopped" => {
                        sqlx::query("UPDATE routes SET status = 'active' WHERE route_id = ?")
                            .bind(route_id)
                            .execute(&mut *tx)
                            .await?;
                        event_specs.push((
                            "route.resumed".into(),
                            entity("route", route_id),
                            json!({}),
                        ));
                    }
                    "stop_route" => {
                        let stopped = stop_route_tx(
                            &mut tx,
                            project_id,
                            route_id,
                            &command.reason,
                            &Utc::now(),
                        )
                        .await?;
                        affected.extend(
                            stopped
                                .cancelled_task_ids
                                .iter()
                                .map(|task_id| entity("task", task_id)),
                        );
                        affected.extend(
                            stopped
                                .cancelled_worker_ids
                                .iter()
                                .map(|worker_id| entity("worker", worker_id)),
                        );
                        event_specs.push((
                            "route.stopped".into(),
                            entity("route", route_id),
                            json!({"replan_queued": true}),
                        ));
                    }
                    _ => {
                        return Err(StorageError::InvalidTransition(format!(
                            "{} from route status {status}",
                            command.command_type
                        )));
                    }
                }
                affected.push(entity("route", route_id));
            }
            "approve_route" => {
                let route_id = &command.target_id;
                let row = sqlx::query("SELECT r.human_review,p.human_route_approval FROM routes r JOIN projects p ON p.project_id=r.project_id WHERE r.project_id=? AND r.route_id=?")
                    .bind(project_id).bind(route_id).fetch_optional(&mut *tx).await?
                    .ok_or_else(|| StorageError::NotFound { kind: "route", id: route_id.clone() })?;
                let approval_enabled: bool = row.try_get("human_route_approval")?;
                let human_review: String = row.try_get("human_review")?;
                if !approval_enabled {
                    return Err(StorageError::InvalidTransition(
                        "human route approval is not enabled for this project".into(),
                    ));
                }
                if human_review != "pending" {
                    return Err(StorageError::InvalidTransition(format!(
                        "route {route_id} review state is {human_review}, not pending"
                    )));
                }
                sqlx::query("UPDATE routes SET human_review='approved' WHERE route_id=?")
                    .bind(route_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE projects SET status='running' WHERE project_id=? AND status='needs_human_review' AND NOT EXISTS (SELECT 1 FROM routes WHERE project_id=? AND human_review='pending')")
                    .bind(project_id).bind(project_id).execute(&mut *tx).await?;
                affected.push(entity("route", route_id));
                event_specs.push((
                    "route.approved".into(),
                    entity("route", route_id),
                    json!({"mathematical_endorsement":false}),
                ));
            }
            "prune_route" => {
                let route_id = &command.target_id;
                let applied_revision = current_revision(&mut tx, project_id).await? + 1;
                let now = Utc::now();
                let mutation = prune_route_tx(
                    &mut tx,
                    project_id,
                    route_id,
                    applied_revision,
                    &command.reason,
                    &now,
                )
                .await?;
                affected.extend(
                    mutation
                        .cancelled_task_ids
                        .iter()
                        .map(|task_id| entity("task", task_id)),
                );
                affected.extend(
                    mutation
                        .cancelled_worker_ids
                        .iter()
                        .map(|worker_id| entity("worker", worker_id)),
                );
                affected.push(entity("route", route_id));
                event_specs.push((
                    "route.pruned".into(),
                    entity("route", route_id),
                    json!({"reason":command.reason,"tombstone_id":mutation.tombstone_id}),
                ));
            }
            "merge_routes" => {
                let canonical_route_id = command
                    .payload
                    .get("canonical_route_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        StorageError::InvalidTransition("canonical_route_id is required".into())
                    })?;
                let source_route_ids = command
                    .payload
                    .get("source_route_ids")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        StorageError::InvalidTransition("source_route_ids is required".into())
                    })?
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>();
                let merged = merge_routes_tx(
                    &mut tx,
                    project_id,
                    canonical_route_id,
                    &source_route_ids,
                    &command.reason,
                    &Utc::now(),
                )
                .await?;
                for source_route_id in &merged.routes {
                    affected.push(entity("route", source_route_id));
                }
                affected.extend(
                    merged
                        .cancelled_tasks
                        .iter()
                        .map(|task_id| entity("task", task_id)),
                );
                affected.extend(
                    merged
                        .cancelled_workers
                        .iter()
                        .map(|worker_id| entity("worker", worker_id)),
                );
                affected.push(entity("route", canonical_route_id));
                event_specs.push((
                    "route.merged".into(),
                    entity("route", canonical_route_id),
                    json!({"source_route_ids":source_route_ids,"canonical_route_id":canonical_route_id,"reason":command.reason}),
                ));
            }
            "revive_route" => {
                let route_id = &command.target_id;
                let evidence_ids = command
                    .payload
                    .get("evidence_ids")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>();
                let applied_revision = current_revision(&mut tx, project_id).await? + 1;
                revive_route_tx(
                    &mut tx,
                    project_id,
                    route_id,
                    applied_revision,
                    &command.reason,
                    &evidence_ids,
                )
                .await?;
                affected.push(entity("route", route_id));
                event_specs.push((
                    "route.revived".into(),
                    entity("route", route_id),
                    json!({"reason":command.reason,"evidence_ids":evidence_ids}),
                ));
            }
            "pause_worker" | "resume_worker" => {
                let worker_id = &command.target_id;
                let row = sqlx::query(
                    "SELECT status,current_task_id FROM workers WHERE project_id=? AND worker_id=?",
                )
                .bind(project_id)
                .bind(worker_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StorageError::NotFound {
                    kind: "worker",
                    id: worker_id.clone(),
                })?;
                let status: String = row.try_get("status")?;
                match command.command_type.as_str() {
                    "pause_worker" if matches!(status.as_str(), "idle" | "running") => {
                        if let Some(task_id) =
                            row.try_get::<Option<String>, _>("current_task_id")?
                        {
                            let cancelled = cancel_task_execution_tx(
                                &mut tx,
                                project_id,
                                &task_id,
                                "paused",
                                &format!("worker paused by command {command_id}"),
                                &Utc::now(),
                            )
                            .await?;
                            affected.extend(
                                cancelled
                                    .worker_ids
                                    .iter()
                                    .map(|affected_worker_id| entity("worker", affected_worker_id)),
                            );
                            affected.push(entity("task", &task_id));
                        }
                        sqlx::query("UPDATE workers SET status='paused',current_task_id=NULL,current_route_id=NULL WHERE worker_id=?")
                            .bind(worker_id).execute(&mut *tx).await?;
                        event_specs.push((
                            "worker.paused".into(),
                            entity("worker", worker_id),
                            json!({}),
                        ));
                    }
                    "resume_worker" if status == "paused" => {
                        sqlx::query("UPDATE workers SET status='idle' WHERE worker_id=?")
                            .bind(worker_id)
                            .execute(&mut *tx)
                            .await?;
                        event_specs.push((
                            "worker.resumed".into(),
                            entity("worker", worker_id),
                            json!({}),
                        ));
                    }
                    _ => {
                        return Err(StorageError::InvalidTransition(format!(
                            "{} from worker status {status}",
                            command.command_type
                        )));
                    }
                }
                affected.push(entity("worker", worker_id));
            }
            "stop_worker" => {
                let worker_id = &command.target_id;
                let task_id: Option<String> = sqlx::query_scalar(
                    "SELECT current_task_id FROM workers WHERE project_id = ? AND worker_id = ?",
                )
                .bind(project_id)
                .bind(worker_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StorageError::NotFound {
                    kind: "worker",
                    id: worker_id.clone(),
                })?;
                sqlx::query("UPDATE workers SET status = 'stopped', current_task_id = NULL, current_route_id = NULL WHERE worker_id = ?")
                    .bind(worker_id).execute(&mut *tx).await?;
                if let Some(task_id) = task_id {
                    let cancelled = cancel_task_execution_tx(
                        &mut tx,
                        project_id,
                        &task_id,
                        "cancelled",
                        &format!("worker stopped by command {command_id}"),
                        &Utc::now(),
                    )
                    .await?;
                    affected.extend(
                        cancelled
                            .worker_ids
                            .iter()
                            .map(|affected_worker_id| entity("worker", affected_worker_id)),
                    );
                    affected.push(entity("task", &task_id));
                }
                affected.push(entity("worker", worker_id));
                event_specs.push((
                    "worker.stopped".into(),
                    entity("worker", worker_id),
                    json!({}),
                ));
            }
            "retry_task" => {
                let task_id = &command.target_id;
                let row = sqlx::query(
                    "SELECT status,context_packet_id FROM tasks WHERE project_id=? AND task_id=?",
                )
                .bind(project_id)
                .bind(task_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StorageError::NotFound {
                    kind: "task",
                    id: task_id.clone(),
                })?;
                let status: String = row.try_get("status")?;
                if !matches!(status.as_str(), "failed" | "blocked" | "dead_lettered") {
                    return Err(StorageError::InvalidTransition(format!(
                        "task {task_id} cannot be retried from {status}"
                    )));
                }
                let attempts: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM task_attempts WHERE task_id=?")
                        .bind(task_id)
                        .fetch_one(&mut *tx)
                        .await?;
                if attempts >= 3 {
                    return Err(StorageError::InvalidTransition(
                        "retry policy permits at most three attempts; create a new plan revision instead"
                            .into(),
                    ));
                }
                let context_packet_id: Option<String> = row.try_get("context_packet_id")?;
                let context_active: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM context_packets WHERE context_packet_id=? AND status='active'",
                )
                .bind(context_packet_id.as_deref().unwrap_or_default())
                .fetch_one(&mut *tx)
                .await?;
                if context_active != 1 {
                    return Err(StorageError::InvalidTransition(
                        "task context is invalid; rebuild context before retrying".into(),
                    ));
                }
                sqlx::query("UPDATE tasks SET status='queued',revision=revision+1,result_summary=? WHERE task_id=?")
                    .bind(format!("manual retry requested: {}",command.reason.trim())).bind(task_id)
                    .execute(&mut *tx).await?;
                affected.push(entity("task", task_id));
                event_specs.push((
                    "task.retry_queued".into(),
                    entity("task", task_id),
                    json!({"prior_status":status,"attempts_used":attempts,"reason":command.reason}),
                ));
            }
            "quarantine_worker_instance" => {
                let worker_instance_id = &command.target_id;
                if command.reason.trim().is_empty() {
                    return Err(StorageError::InvalidTransition(
                        "worker quarantine requires a reason".into(),
                    ));
                }
                let row = sqlx::query("SELECT status FROM worker_instances WHERE project_id=? AND worker_instance_id=?")
                    .bind(project_id).bind(worker_instance_id).fetch_optional(&mut *tx).await?
                    .ok_or_else(|| StorageError::NotFound { kind:"worker_instance",id:worker_instance_id.clone() })?;
                let prior_status: String = row.try_get("status")?;
                if prior_status == "quarantined" {
                    affected.push(entity("worker_instance", worker_instance_id));
                } else {
                    sqlx::query("UPDATE worker_instances SET status='quarantined',quarantine_reason=?,exited_at=? WHERE worker_instance_id=?")
                        .bind(command.reason.trim()).bind(Utc::now().to_rfc3339()).bind(worker_instance_id).execute(&mut *tx).await?;
                    let leases=sqlx::query("SELECT lease_id,task_id,attempt_id FROM task_leases WHERE worker_instance_id=? AND status='active'")
                        .bind(worker_instance_id).fetch_all(&mut *tx).await?;
                    for lease in leases {
                        let lease_id: String = lease.try_get("lease_id")?;
                        let task_id: String = lease.try_get("task_id")?;
                        let attempt_id: Option<String> = lease.try_get("attempt_id")?;
                        sqlx::query("UPDATE task_leases SET status='cancelled',completed_at=? WHERE lease_id=?")
                            .bind(Utc::now().to_rfc3339()).bind(&lease_id).execute(&mut *tx).await?;
                        if let Some(attempt_id) = attempt_id {
                            sqlx::query("UPDATE task_attempts SET status='orphaned',failure_reason=?,completed_at=? WHERE attempt_id=? AND status NOT IN ('completed','failed','orphaned')")
                                .bind(format!("worker quarantined: {}",command.reason.trim())).bind(Utc::now().to_rfc3339()).bind(&attempt_id).execute(&mut *tx).await?;
                        }
                        sqlx::query("UPDATE tasks SET status=CASE WHEN (SELECT COUNT(*) FROM task_attempts WHERE task_id=?)>=3 THEN 'dead_lettered' ELSE 'queued' END,revision=revision+1,result_summary=? WHERE task_id=? AND status IN ('offered','leased','running','checkpointed','result_submitted','ingesting')")
                            .bind(&task_id).bind(format!("worker quarantined: {}",command.reason.trim())).bind(&task_id).execute(&mut *tx).await?;
                        affected.push(entity("task", &task_id));
                    }
                    affected.push(entity("worker_instance", worker_instance_id));
                    event_specs.push((
                        "worker.quarantined".into(),
                        entity("worker_instance", worker_instance_id),
                        json!({"prior_status":prior_status,"reason":command.reason}),
                    ));
                }
            }
            "rebuild_task_context" => {
                let task_id = &command.target_id;
                let task_row=sqlx::query("SELECT route_id,plan_revision_id,status FROM tasks WHERE project_id=? AND task_id=?")
                    .bind(project_id).bind(task_id).fetch_optional(&mut *tx).await?
                    .ok_or_else(|| StorageError::NotFound{kind:"task",id:task_id.clone()})?;
                let task_status: String = task_row.try_get("status")?;
                if matches!(
                    task_status.as_str(),
                    "offered"
                        | "leased"
                        | "running"
                        | "checkpointed"
                        | "result_submitted"
                        | "ingesting"
                        | "completed"
                ) {
                    return Err(StorageError::InvalidTransition(format!(
                        "cannot rebuild context while task {task_id} is {task_status}"
                    )));
                }
                let route_id: String = task_row.try_get("route_id")?;
                let plan_revision_id: String = task_row
                    .try_get::<Option<String>, _>("plan_revision_id")?
                    .ok_or_else(|| {
                        StorageError::InvalidTransition(
                            "context rebuild is only available for V2 plan tasks".into(),
                        )
                    })?;
                let old=sqlx::query("SELECT * FROM context_packets WHERE project_id=? AND task_id=? ORDER BY source_revision DESC,created_at DESC LIMIT 1")
                    .bind(project_id).bind(task_id).fetch_optional(&mut *tx).await?
                    .ok_or_else(|| StorageError::NotFound{kind:"context_packet",id:task_id.clone()})?;
                let old_packet_id: String = old.try_get("context_packet_id")?;
                let old_known: Vec<String> =
                    serde_json::from_str(old.try_get("known_fact_ids_json")?)?;
                let old_known_count = old_known.len();
                let mut active_known = Vec::new();
                for fact_id in old_known {
                    let active:i64=sqlx::query_scalar("SELECT COUNT(*) FROM facts WHERE project_id=? AND fact_id=? AND status='active'")
                        .bind(project_id).bind(&fact_id).fetch_one(&mut *tx).await?;
                    if active == 1 {
                        active_known.push(fact_id);
                    }
                }
                let source_revision = current_revision(&mut tx, project_id).await?;
                let mut content: Value = serde_json::from_str(old.try_get("content_json")?)?;
                if let Some(object) = content.as_object_mut() {
                    object.insert("known_fact_ids".into(), json!(active_known));
                    if let Some(facts) = object.get_mut("known_facts").and_then(Value::as_array_mut)
                    {
                        facts.retain(|fact| {
                            fact.get("fact_id")
                                .and_then(Value::as_str)
                                .is_some_and(|fact_id| active_known.iter().any(|id| id == fact_id))
                        });
                    }
                    object.insert("source_revision".into(), json!(source_revision));
                    object.insert(
                        "rebuilt_from_context_packet_id".into(),
                        json!(old_packet_id),
                    );
                    object.insert("rebuild_reason".into(), json!(command.reason));
                }
                let context_packet_id = new_id("context");
                let content_hash = hex::encode(Sha256::digest(serde_json::to_vec(&content)?));
                let now = Utc::now().to_rfc3339();
                sqlx::query("UPDATE context_packets SET status='invalidated',invalidation_reason=COALESCE(invalidation_reason,?) WHERE project_id=? AND task_id=? AND status='active'")
                    .bind(format!("rebuilt by command {command_id}")).bind(project_id).bind(task_id).execute(&mut *tx).await?;
                sqlx::query("INSERT INTO context_packets(context_packet_id,project_id,task_id,route_id,packet_kind,source_revision,problem_contract_excerpt,objective,known_fact_ids_json,bottleneck_id,route_progress_json,relevant_failure_pattern_ids_json,relevant_uncertainty_ids_json,source_refs_json,omitted_sections_json,token_estimate,content_json,content_hash,status,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,'active',?)")
                    .bind(&context_packet_id).bind(project_id).bind(task_id).bind(&route_id)
                    .bind(old.try_get::<String,_>("packet_kind")?).bind(source_revision)
                    .bind(old.try_get::<String,_>("problem_contract_excerpt")?).bind(old.try_get::<String,_>("objective")?)
                    .bind(json_text(&active_known)?).bind(old.try_get::<Option<String>,_>("bottleneck_id")?)
                    .bind(old.try_get::<String,_>("route_progress_json")?).bind(old.try_get::<String,_>("relevant_failure_pattern_ids_json")?)
                    .bind(old.try_get::<String,_>("relevant_uncertainty_ids_json")?).bind(old.try_get::<String,_>("source_refs_json")?)
                    .bind(old.try_get::<String,_>("omitted_sections_json")?).bind(old.try_get::<i64,_>("token_estimate")?)
                    .bind(json_text(&content)?).bind(&content_hash).bind(&now).execute(&mut *tx).await?;
                let contract_row=sqlx::query("SELECT contract_version,contract_json FROM task_contracts WHERE project_id=? AND task_id=? ORDER BY contract_version DESC LIMIT 1")
                    .bind(project_id).bind(task_id).fetch_optional(&mut *tx).await?
                    .ok_or_else(|| StorageError::NotFound{kind:"task_contract",id:task_id.clone()})?;
                let contract_version = contract_row.try_get::<i64, _>("contract_version")? + 1;
                let task_contract_id = new_id("taskcontract");
                let mut contract: Value =
                    serde_json::from_str(contract_row.try_get("contract_json")?)?;
                if let Some(object) = contract.as_object_mut() {
                    object.insert("task_contract_id".into(), json!(task_contract_id));
                    object.insert("context_packet_id".into(), json!(context_packet_id));
                    object.insert("contract_version".into(), json!(contract_version));
                    object.insert("allowed_input_ids".into(), json!(active_known));
                    object.insert("content_hash".into(), json!(""));
                }
                let contract_hash = hex::encode(Sha256::digest(serde_json::to_vec(&contract)?));
                if let Some(object) = contract.as_object_mut() {
                    object.insert("content_hash".into(), json!(contract_hash));
                }
                sqlx::query("INSERT INTO task_contracts(task_contract_id,project_id,task_id,plan_revision_id,contract_version,contract_json,content_hash,created_at) VALUES(?,?,?,?,?,?,?,?)")
                    .bind(&task_contract_id).bind(project_id).bind(task_id).bind(&plan_revision_id)
                    .bind(contract_version).bind(json_text(&contract)?).bind(&contract_hash).bind(&now).execute(&mut *tx).await?;
                let missing_required_facts = old_known_count.saturating_sub(active_known.len());
                sqlx::query("UPDATE tasks SET context_packet_id=?,status=CASE WHEN ?>0 THEN 'blocked' WHEN status='blocked' THEN 'queued' ELSE status END,result_summary=CASE WHEN ?>0 THEN 'context rebuilt but one or more required facts are no longer active' ELSE result_summary END,revision=revision+1 WHERE task_id=?")
                    .bind(&context_packet_id).bind(i64::try_from(missing_required_facts).unwrap_or(i64::MAX))
                    .bind(i64::try_from(missing_required_facts).unwrap_or(i64::MAX)).bind(task_id).execute(&mut *tx).await?;
                affected.push(entity("task", task_id));
                affected.push(entity("context_packet", &context_packet_id));
                event_specs.push(("context.packet.invalidated".into(),entity("context_packet",&old_packet_id),json!({"reason":"manual rebuild","replacement_context_packet_id":context_packet_id})));
                event_specs.push(("context.packet.created".into(),entity("context_packet",&context_packet_id),json!({"task_id":task_id,"source_revision":source_revision,"rebuilt_from":old_packet_id,"content_hash":content_hash,"missing_required_facts":missing_required_facts})));
            }
            "steer_task" => {
                let task_id = &command.target_id;
                let content = command
                    .payload
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim();
                let expected_task_revision = command
                    .payload
                    .get("expected_task_revision")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| {
                        StorageError::InvalidTransition("expected_task_revision is required".into())
                    })?;
                let expected_route_epoch = command
                    .payload
                    .get("expected_route_epoch")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| {
                        StorageError::InvalidTransition("expected_route_epoch is required".into())
                    })?;
                if content.is_empty() {
                    return Err(StorageError::InvalidTransition(
                        "steer content must not be empty".into(),
                    ));
                }
                let task_row = sqlx::query("SELECT revision,route_cancellation_epoch,status FROM tasks WHERE project_id=? AND task_id=?")
                    .bind(project_id).bind(task_id).fetch_optional(&mut *tx).await?
                    .ok_or_else(|| StorageError::NotFound { kind: "task", id: task_id.clone() })?;
                let task_revision: i64 = task_row.try_get("revision")?;
                let route_epoch: i64 = task_row.try_get("route_cancellation_epoch")?;
                let task_status: String = task_row.try_get("status")?;
                if task_revision != expected_task_revision || route_epoch != expected_route_epoch {
                    return Err(StorageError::LateSubmission(format!(
                        "task {task_id} is revision/epoch {task_revision}/{route_epoch}"
                    )));
                }
                if !matches!(task_status.as_str(), "leased" | "running" | "checkpointed") {
                    return Err(StorageError::InvalidTransition(format!(
                        "cannot steer task {task_id} in {task_status}"
                    )));
                }
                let steer_id = new_id("steer");
                let now = Utc::now();
                sqlx::query("INSERT INTO task_steers(steer_id,project_id,task_id,command_id,content,expected_task_revision,expected_route_epoch,status,created_at,applied_at) VALUES(?,?,?,?,?,?,?,'pending',?,NULL)")
                    .bind(&steer_id).bind(project_id).bind(task_id).bind(command_id).bind(content)
                    .bind(expected_task_revision).bind(expected_route_epoch).bind(now.to_rfc3339())
                    .execute(&mut *tx).await?;
                sqlx::query(
                    "UPDATE human_commands SET status='waiting_safe_point' WHERE command_id=?",
                )
                .bind(command_id)
                .execute(&mut *tx)
                .await?;
                let revision = bump_revision(&mut tx, project_id).await?;
                let event = append_event(&mut tx, project_id, revision, "task.steer.queued", entity("task", task_id), json!({"steer_id":steer_id,"task_revision":task_revision,"route_epoch":route_epoch}), Some(entity("command", command_id))).await?;
                tx.commit().await?;
                command.status = CommandStatus::WaitingSafePoint;
                return Ok((command, vec![event]));
            }
            "create_task" => {
                let route_id = command
                    .payload
                    .get("route_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        StorageError::InvalidTransition("route_id is required".into())
                    })?;
                let route_epoch: i64 = sqlx::query_scalar(
                    "SELECT cancellation_epoch FROM routes WHERE project_id=? AND route_id=?",
                )
                .bind(project_id)
                .bind(route_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StorageError::NotFound {
                    kind: "route",
                    id: route_id.into(),
                })?;
                let worker_role = command
                    .payload
                    .get("worker_role")
                    .and_then(Value::as_str)
                    .unwrap_or("explorer");
                let objective = command
                    .payload
                    .get("objective")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let completion_contract = command
                    .payload
                    .get("completion_contract")
                    .and_then(Value::as_str)
                    .unwrap_or("submit an independently verifiable result");
                if objective.trim().is_empty() {
                    return Err(StorageError::InvalidTransition(
                        "task objective is required".into(),
                    ));
                }
                let goal_ids: Vec<String> = serde_json::from_value(
                    command
                        .payload
                        .get("goal_ids")
                        .cloned()
                        .unwrap_or_else(|| json!([])),
                )?;
                let worker_id = command.payload.get("worker_id").and_then(Value::as_str);
                if let Some(worker_id) = worker_id {
                    let count: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM workers WHERE project_id=? AND worker_id=?",
                    )
                    .bind(project_id)
                    .bind(worker_id)
                    .fetch_one(&mut *tx)
                    .await?;
                    if count != 1 {
                        return Err(StorageError::NotFound {
                            kind: "worker",
                            id: worker_id.into(),
                        });
                    }
                }
                let round: i64 =
                    sqlx::query_scalar("SELECT current_round FROM projects WHERE project_id=?")
                        .bind(project_id)
                        .fetch_one(&mut *tx)
                        .await?;
                let task_id = new_id("task");
                let status = if worker_id.is_some() {
                    "assigned"
                } else {
                    "open"
                };
                sqlx::query("INSERT INTO tasks(task_id,project_id,route_id,worker_id,worker_role,goal_ids_json,objective,completion_contract,status,priority,revision,route_cancellation_epoch,round,result_summary) VALUES(?,?,?,?,?,?,?,?,?,?,1,?,?,NULL)")
                    .bind(&task_id).bind(project_id).bind(route_id).bind(worker_id).bind(worker_role)
                    .bind(json_text(&goal_ids)?).bind(objective).bind(completion_contract).bind(status)
                    .bind(command.payload.get("priority").and_then(Value::as_f64).unwrap_or(1.0))
                    .bind(route_epoch).bind(round).execute(&mut *tx).await?;
                affected.push(entity("task", &task_id));
                event_specs.push((
                    "task.created_by_human".into(),
                    entity("task", &task_id),
                    json!({"route_id":route_id,"worker_id":worker_id,"round":round}),
                ));
            }
            "pause_task" | "resume_task" | "reassign_task" | "cancel_task"
            | "set_task_priority" => {
                let task_id = &command.target_id;
                let expected_task_revision = command
                    .payload
                    .get("expected_task_revision")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| {
                        StorageError::InvalidTransition("expected_task_revision is required".into())
                    })?;
                let row = sqlx::query(
                    "SELECT status,revision,worker_id FROM tasks WHERE project_id=? AND task_id=?",
                )
                .bind(project_id)
                .bind(task_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StorageError::NotFound {
                    kind: "task",
                    id: task_id.clone(),
                })?;
                let status: String = row.try_get("status")?;
                let revision: i64 = row.try_get("revision")?;
                if revision != expected_task_revision {
                    return Err(StorageError::RevisionConflict {
                        expected: expected_task_revision,
                        actual: revision,
                    });
                }
                match command.command_type.as_str() {
                    "pause_task"
                        if matches!(
                            status.as_str(),
                            "open"
                                | "queued"
                                | "assigned"
                                | "offered"
                                | "leased"
                                | "running"
                                | "checkpointed"
                                | "result_submitted"
                                | "ingesting"
                                | "blocked"
                        ) =>
                    {
                        let cancelled = cancel_task_execution_tx(
                            &mut tx,
                            project_id,
                            task_id,
                            "paused",
                            &format!("task paused by command {command_id}"),
                            &Utc::now(),
                        )
                        .await?;
                        affected.extend(
                            cancelled
                                .worker_ids
                                .iter()
                                .map(|worker_id| entity("worker", worker_id)),
                        );
                        event_specs.push((
                            "task.paused".into(),
                            entity("task", task_id),
                            json!({"previous_status":status}),
                        ));
                    }
                    "resume_task" if status == "paused" => {
                        let worker_id: Option<String> = row.try_get("worker_id")?;
                        let next = if worker_id.is_some() {
                            "assigned"
                        } else {
                            "open"
                        };
                        sqlx::query(
                            "UPDATE tasks SET status=?,revision=revision+1 WHERE task_id=?",
                        )
                        .bind(next)
                        .bind(task_id)
                        .execute(&mut *tx)
                        .await?;
                        event_specs.push((
                            "task.resumed".into(),
                            entity("task", task_id),
                            json!({"status":next}),
                        ));
                    }
                    "reassign_task"
                        if matches!(
                            status.as_str(),
                            "open"
                                | "queued"
                                | "assigned"
                                | "offered"
                                | "leased"
                                | "running"
                                | "checkpointed"
                                | "result_submitted"
                                | "ingesting"
                                | "paused"
                                | "blocked"
                        ) =>
                    {
                        let worker_id = command
                            .payload
                            .get("worker_id")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                StorageError::InvalidTransition("worker_id is required".into())
                            })?;
                        let count: i64 = sqlx::query_scalar(
                            "SELECT COUNT(*) FROM workers WHERE project_id=? AND worker_id=?",
                        )
                        .bind(project_id)
                        .bind(worker_id)
                        .fetch_one(&mut *tx)
                        .await?;
                        if count != 1 {
                            return Err(StorageError::NotFound {
                                kind: "worker",
                                id: worker_id.into(),
                            });
                        }
                        let cancelled = cancel_task_execution_tx(
                            &mut tx,
                            project_id,
                            task_id,
                            "assigned",
                            &format!("task reassigned by command {command_id}"),
                            &Utc::now(),
                        )
                        .await?;
                        affected.extend(
                            cancelled
                                .worker_ids
                                .iter()
                                .map(|old_worker_id| entity("worker", old_worker_id)),
                        );
                        sqlx::query("UPDATE tasks SET worker_id=? WHERE project_id=? AND task_id=? AND status='assigned'")
                            .bind(worker_id).bind(project_id).bind(task_id).execute(&mut *tx).await?;
                        event_specs.push((
                            "task.reassigned".into(),
                            entity("task", task_id),
                            json!({"worker_id":worker_id}),
                        ));
                    }
                    "cancel_task"
                        if matches!(
                            status.as_str(),
                            "open"
                                | "queued"
                                | "assigned"
                                | "offered"
                                | "leased"
                                | "running"
                                | "checkpointed"
                                | "result_submitted"
                                | "ingesting"
                                | "paused"
                                | "blocked"
                        ) =>
                    {
                        let cancelled = cancel_task_execution_tx(
                            &mut tx,
                            project_id,
                            task_id,
                            "cancelled",
                            &format!("task cancelled by command {command_id}"),
                            &Utc::now(),
                        )
                        .await?;
                        affected.extend(
                            cancelled
                                .worker_ids
                                .iter()
                                .map(|worker_id| entity("worker", worker_id)),
                        );
                        event_specs.push((
                            "task.cancelled".into(),
                            entity("task", task_id),
                            json!({"previous_status":status}),
                        ));
                    }
                    "set_task_priority"
                        if !matches!(
                            status.as_str(),
                            "completed" | "rejected" | "cancelled" | "human_stopped"
                        ) =>
                    {
                        let priority = command
                            .payload
                            .get("priority")
                            .and_then(Value::as_f64)
                            .filter(|priority| priority.is_finite())
                            .ok_or_else(|| {
                                StorageError::InvalidTransition(
                                    "finite priority is required".into(),
                                )
                            })?;
                        sqlx::query(
                            "UPDATE tasks SET priority=?,revision=revision+1 WHERE task_id=?",
                        )
                        .bind(priority)
                        .bind(task_id)
                        .execute(&mut *tx)
                        .await?;
                        event_specs.push((
                            "task.priority_changed".into(),
                            entity("task", task_id),
                            json!({"priority":priority}),
                        ));
                    }
                    _ => {
                        return Err(StorageError::InvalidTransition(format!(
                            "{} from task status {status}",
                            command.command_type
                        )));
                    }
                }
                affected.push(entity("task", task_id));
            }
            "goal_review" => {
                let focus = command
                    .payload
                    .get("focus")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim();
                if focus.is_empty() {
                    return Err(StorageError::InvalidTransition(
                        "goal review focus must not be empty".into(),
                    ));
                }
                let status: String =
                    sqlx::query_scalar("SELECT status FROM projects WHERE project_id=?")
                        .bind(project_id)
                        .fetch_one(&mut *tx)
                        .await?;
                if !matches!(
                    status.as_str(),
                    "created" | "running" | "paused" | "needs_human_review"
                ) {
                    return Err(StorageError::InvalidTransition(format!(
                        "goal review cannot restart a project in {status}"
                    )));
                }
                let unfinished_rows = sqlx::query("SELECT task_id,route_id FROM tasks WHERE project_id=? AND status IN ('open','queued','assigned','offered','leased','running','checkpointed','result_submitted','ingesting','paused','blocked') ORDER BY task_id")
                    .bind(project_id)
                    .fetch_all(&mut *tx)
                    .await?;
                let task_ids = unfinished_rows
                    .iter()
                    .map(|row| row.try_get::<String, _>("task_id"))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut route_ids = unfinished_rows
                    .iter()
                    .map(|row| row.try_get::<String, _>("route_id"))
                    .collect::<Result<Vec<_>, _>>()?;
                route_ids.sort();
                route_ids.dedup();
                let interrupted_round_ids = sqlx::query_scalar::<_, String>("SELECT round_id FROM rounds WHERE project_id=? AND status IN ('created','planning','running','verifying','publishing') ORDER BY number")
                    .bind(project_id)
                    .fetch_all(&mut *tx)
                    .await?;
                let now = Utc::now().to_rfc3339();
                sqlx::query("UPDATE routes SET cancellation_epoch=cancellation_epoch+1 WHERE project_id=? AND route_id IN (SELECT DISTINCT route_id FROM tasks WHERE project_id=? AND status IN ('open','queued','assigned','offered','leased','running','checkpointed','result_submitted','ingesting','paused','blocked'))")
                    .bind(project_id)
                    .bind(project_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE tasks SET status='cancelled',revision=revision+1,route_cancellation_epoch=(SELECT cancellation_epoch FROM routes WHERE routes.route_id=tasks.route_id),result_summary=COALESCE(result_summary,'Cancelled for a human-requested goal review') WHERE project_id=? AND status IN ('open','queued','assigned','offered','leased','running','checkpointed','result_submitted','ingesting','paused','blocked')")
                    .bind(project_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE context_packets SET status='invalidated',invalidation_reason='superseded by human-requested goal review' WHERE project_id=? AND task_id IN (SELECT task_id FROM tasks WHERE project_id=? AND status='cancelled') AND status='active'")
                    .bind(project_id)
                    .bind(project_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE task_leases SET status='cancelled',completed_at=? WHERE project_id=? AND status='active'")
                    .bind(&now)
                    .bind(project_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE result_envelopes SET status='stale',rejection_reason='superseded by human-requested goal review' WHERE project_id=? AND status='submitted'")
                    .bind(project_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE task_attempts SET status='cancelled',failure_reason='superseded by human-requested goal review',completed_at=? WHERE project_id=? AND status NOT IN ('completed','failed','orphaned','cancelled')")
                    .bind(&now)
                    .bind(project_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE worker_instances SET status='exited',quarantine_reason='superseded by human-requested goal review',exited_at=? WHERE project_id=? AND status NOT IN ('exited','quarantined')")
                    .bind(&now)
                    .bind(project_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE workers SET status='stopped',current_task_id=NULL,current_route_id=NULL WHERE project_id=? AND status NOT IN ('stopped','offline')")
                    .bind(project_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE planning_stage_attempts SET status='failed',error='superseded by human-requested goal review',last_heartbeat_at=?,completed_at=? WHERE project_id=? AND status='running'")
                    .bind(&now)
                    .bind(&now)
                    .bind(project_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE rounds SET status='interrupted',completed_at=?,summary='Interrupted for a human-requested goal review' WHERE project_id=? AND status IN ('created','planning','running','verifying','publishing')")
                    .bind(&now)
                    .bind(project_id)
                    .execute(&mut *tx)
                    .await?;
                // A goal review is an explicit instruction to start a fresh planning
                // round. Existing per-route review decisions remain auditable but may
                // not hold the project itself in needs_human_review.
                sqlx::query("UPDATE projects SET status='running' WHERE project_id=?")
                    .bind(project_id)
                    .execute(&mut *tx)
                    .await?;
                affected.extend(task_ids.iter().map(|task_id| entity("task", task_id)));
                affected.extend(route_ids.iter().map(|route_id| entity("route", route_id)));
                affected.push(entity("project", project_id));
                event_specs.push((
                    "goal_review.requested".into(),
                    entity("project", project_id),
                    json!({
                        "focus": focus,
                        "cancelled_task_ids": task_ids,
                        "interrupted_round_ids": interrupted_round_ids,
                        "preserved": ["facts", "artifacts", "event_history"],
                        "pending_route_reviews_block_new_round": false,
                    }),
                ));
            }
            "research_settings" => {
                let limits =
                    command.payload.get("limits").cloned().ok_or_else(|| {
                        StorageError::InvalidTransition("limits are required".into())
                    })?;
                let budget: Budget = serde_json::from_value(limits.clone())?;
                validate_project_budget(&budget)?;
                let used: i64 = sqlx::query_scalar(
                    "SELECT COALESCE(SUM(model_calls),0) FROM usage_records WHERE project_id=?",
                )
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?;
                if i64::from(budget.max_total_model_calls) < used {
                    return Err(StorageError::BudgetExhausted(format!(
                        "new total limit {} is below already used {used}",
                        budget.max_total_model_calls
                    )));
                }
                let requested_mode = command
                    .payload
                    .get("review_mode")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        StorageError::InvalidTransition("review_mode is required".into())
                    })?;
                let review_mode = ReviewMode::from_str(requested_mode)
                    .map_err(StorageError::InvalidTransition)?;
                let previous: String =
                    sqlx::query_scalar("SELECT review_mode FROM projects WHERE project_id=?")
                        .bind(project_id)
                        .fetch_one(&mut *tx)
                        .await?;
                let approval_required = review_mode == ReviewMode::Strict;
                let released_route_ids = if !approval_required && previous == "strict" {
                    sqlx::query_scalar::<_, String>("SELECT route_id FROM routes WHERE project_id=? AND human_review='pending' ORDER BY route_id")
                        .bind(project_id)
                        .fetch_all(&mut *tx)
                        .await?
                } else {
                    Vec::new()
                };
                if !released_route_ids.is_empty() {
                    sqlx::query("UPDATE routes SET human_review='not_required' WHERE project_id=? AND human_review='pending'")
                        .bind(project_id)
                        .execute(&mut *tx)
                        .await?;
                }
                sqlx::query("UPDATE projects SET budget_json=?,review_mode=?,human_route_approval=?,status=CASE WHEN ?=0 AND status='needs_human_review' AND NOT EXISTS (SELECT 1 FROM human_questions WHERE project_id=? AND status='open') THEN 'running' ELSE status END WHERE project_id=?")
                    .bind(json_text(&budget)?)
                    .bind(review_mode.to_string())
                    .bind(approval_required)
                    .bind(approval_required)
                    .bind(project_id)
                    .bind(project_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("INSERT INTO budget_overrides(budget_override_id,project_id,scope_kind,scope_id,limits_json,reason,requested_by,created_at) VALUES(?,?,'project',?,?,?,?,?) ON CONFLICT(project_id,scope_kind,scope_id) DO UPDATE SET limits_json=excluded.limits_json,reason=excluded.reason,requested_by=excluded.requested_by,created_at=excluded.created_at")
                    .bind(new_id("budget"))
                    .bind(project_id)
                    .bind(project_id)
                    .bind(json_text(&limits)?)
                    .bind(&command.reason)
                    .bind(&command.requested_by)
                    .bind(Utc::now().to_rfc3339())
                    .execute(&mut *tx)
                    .await?;
                affected.push(entity("project", project_id));
                affected.extend(
                    released_route_ids
                        .iter()
                        .map(|route_id| entity("route", route_id)),
                );
                event_specs.push((
                    "research_settings.updated".into(),
                    entity("project", project_id),
                    json!({
                        "limits": limits,
                        "previous_review_mode": previous,
                        "review_mode": review_mode,
                        "human_route_approval": approval_required,
                        "released_route_ids": released_route_ids,
                        "running_tasks_interrupted": false,
                        "task_contract_policy": "new_tasks_only",
                    }),
                ));
            }
            "review_policy" => {
                let requested = command
                    .payload
                    .get("review_mode")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        StorageError::InvalidTransition("review_mode is required".into())
                    })?;
                let review_mode =
                    ReviewMode::from_str(requested).map_err(StorageError::InvalidTransition)?;
                let previous: String =
                    sqlx::query_scalar("SELECT review_mode FROM projects WHERE project_id=?")
                        .bind(project_id)
                        .fetch_one(&mut *tx)
                        .await?;
                let approval_required = review_mode == ReviewMode::Strict;
                let released_route_ids = if !approval_required && previous == "strict" {
                    sqlx::query_scalar::<_, String>("SELECT route_id FROM routes WHERE project_id=? AND human_review='pending' ORDER BY route_id")
                        .bind(project_id)
                        .fetch_all(&mut *tx)
                        .await?
                } else {
                    Vec::new()
                };
                if !released_route_ids.is_empty() {
                    sqlx::query("UPDATE routes SET human_review='not_required' WHERE project_id=? AND human_review='pending'")
                        .bind(project_id)
                        .execute(&mut *tx)
                        .await?;
                }
                sqlx::query("UPDATE projects SET review_mode=?,human_route_approval=?,status=CASE WHEN ?=0 AND status='needs_human_review' AND NOT EXISTS (SELECT 1 FROM human_questions WHERE project_id=? AND status='open') THEN 'running' ELSE status END WHERE project_id=?")
                    .bind(review_mode.to_string())
                    .bind(approval_required)
                    .bind(approval_required)
                    .bind(project_id)
                    .bind(project_id)
                    .execute(&mut *tx)
                    .await?;
                affected.push(entity("project", project_id));
                affected.extend(
                    released_route_ids
                        .iter()
                        .map(|route_id| entity("route", route_id)),
                );
                event_specs.push((
                    "review_policy.updated".into(),
                    entity("project", project_id),
                    json!({
                        "previous_review_mode": previous,
                        "review_mode": review_mode,
                        "human_route_approval": approval_required,
                        "released_route_ids": released_route_ids,
                        "existing_running_routes_interrupted": false,
                    }),
                ));
            }
            "adjust_budget" => {
                let scope_kind = command
                    .payload
                    .get("scope_kind")
                    .and_then(Value::as_str)
                    .unwrap_or("project");
                let scope_id = command
                    .payload
                    .get("scope_id")
                    .and_then(Value::as_str)
                    .unwrap_or(project_id);
                let limits =
                    command.payload.get("limits").cloned().ok_or_else(|| {
                        StorageError::InvalidTransition("limits are required".into())
                    })?;
                if scope_kind == "project" {
                    let budget: Budget = serde_json::from_value(limits.clone())?;
                    validate_project_budget(&budget)?;
                    let used: i64 = sqlx::query_scalar(
                        "SELECT COALESCE(SUM(model_calls),0) FROM usage_records WHERE project_id=?",
                    )
                    .bind(project_id)
                    .fetch_one(&mut *tx)
                    .await?;
                    if i64::from(budget.max_total_model_calls) < used {
                        return Err(StorageError::BudgetExhausted(format!(
                            "new total limit {} is below already used {used}",
                            budget.max_total_model_calls
                        )));
                    }
                    sqlx::query("UPDATE projects SET budget_json=? WHERE project_id=?")
                        .bind(json_text(&budget)?)
                        .bind(project_id)
                        .execute(&mut *tx)
                        .await?;
                } else if matches!(scope_kind, "task" | "round") {
                    let max_calls = limits
                        .get("max_model_calls")
                        .and_then(Value::as_i64)
                        .filter(|value| *value >= 0)
                        .ok_or_else(|| {
                            StorageError::InvalidTransition(
                                "max_model_calls is required for task/round scope".into(),
                            )
                        })?;
                    let used: i64 = if scope_kind == "task" {
                        sqlx::query_scalar("SELECT COALESCE(SUM(model_calls),0) FROM usage_records WHERE project_id=? AND task_id=?")
                            .bind(project_id).bind(scope_id).fetch_one(&mut *tx).await?
                    } else {
                        let round = scope_id.parse::<i64>().map_err(|_| {
                            StorageError::InvalidTransition(
                                "round scope_id must be an integer".into(),
                            )
                        })?;
                        sqlx::query_scalar("SELECT COALESCE(SUM(model_calls),0) FROM usage_records WHERE project_id=? AND round=?")
                            .bind(project_id).bind(round).fetch_one(&mut *tx).await?
                    };
                    if max_calls < used {
                        return Err(StorageError::BudgetExhausted(format!(
                            "new {scope_kind} limit {max_calls} is below already used {used}"
                        )));
                    }
                } else if scope_kind == "search" {
                    let budget: research_domain::ProofSearchBudget =
                        serde_json::from_value(limits.clone())?;
                    let row = sqlx::query("SELECT nodes_created,nodes_expanded,model_calls FROM proof_searches WHERE search_id=?")
                        .bind(scope_id).fetch_optional(&mut *tx).await?
                        .ok_or_else(|| StorageError::NotFound { kind: "proof_search", id: scope_id.into() })?;
                    let nodes_created: i64 = row.try_get("nodes_created")?;
                    let nodes_expanded: i64 = row.try_get("nodes_expanded")?;
                    let model_calls: i64 = row.try_get("model_calls")?;
                    if i64::from(budget.max_nodes) < nodes_created.max(nodes_expanded)
                        || i64::from(budget.max_model_calls) < model_calls
                    {
                        return Err(StorageError::BudgetExhausted(
                            "new proof-search budget is below consumed nodes or calls".into(),
                        ));
                    }
                    sqlx::query("UPDATE proof_searches SET budget_json=? WHERE search_id=?")
                        .bind(json_text(&budget)?)
                        .bind(scope_id)
                        .execute(&mut *tx)
                        .await?;
                } else if scope_kind == "verification" {
                    let max_attempts = limits
                        .get("max_attempts")
                        .and_then(Value::as_i64)
                        .filter(|value| *value > 0)
                        .ok_or_else(|| {
                            StorageError::InvalidTransition(
                                "max_attempts is required for verification scope".into(),
                            )
                        })?;
                    let case_id: String = sqlx::query_scalar("SELECT case_id FROM verification_cases WHERE case_id=? OR verification_id=? LIMIT 1")
                        .bind(scope_id).bind(scope_id).fetch_optional(&mut *tx).await?
                        .ok_or_else(|| StorageError::NotFound { kind: "verification_case", id: scope_id.into() })?;
                    let used: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM verification_attempts WHERE case_id=?",
                    )
                    .bind(&case_id)
                    .fetch_one(&mut *tx)
                    .await?;
                    if max_attempts < used {
                        return Err(StorageError::BudgetExhausted(format!(
                            "new verification limit {max_attempts} is below {used} attempts"
                        )));
                    }
                    sqlx::query("UPDATE verification_policies SET max_attempts=? WHERE case_id=?")
                        .bind(max_attempts)
                        .bind(&case_id)
                        .execute(&mut *tx)
                        .await?;
                } else {
                    return Err(StorageError::InvalidTransition(format!(
                        "unsupported budget scope {scope_kind}"
                    )));
                }
                sqlx::query("INSERT INTO budget_overrides(budget_override_id,project_id,scope_kind,scope_id,limits_json,reason,requested_by,created_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(project_id,scope_kind,scope_id) DO UPDATE SET limits_json=excluded.limits_json,reason=excluded.reason,requested_by=excluded.requested_by,created_at=excluded.created_at")
                    .bind(new_id("budget")).bind(project_id).bind(scope_kind).bind(scope_id).bind(json_text(&limits)?)
                    .bind(&command.reason).bind(&command.requested_by).bind(Utc::now().to_rfc3339()).execute(&mut *tx).await?;
                affected.push(entity(scope_kind, scope_id));
                event_specs.push((
                    "budget.adjusted".into(),
                    entity(scope_kind, scope_id),
                    json!({"limits":limits}),
                ));
            }
            "answer_question" => {
                let question_id = &command.target_id;
                let answer =
                    command.payload.get("answer").cloned().ok_or_else(|| {
                        StorageError::InvalidTransition("answer is required".into())
                    })?;
                let status: String = sqlx::query_scalar(
                    "SELECT status FROM human_questions WHERE project_id=? AND question_id=?",
                )
                .bind(project_id)
                .bind(question_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StorageError::NotFound {
                    kind: "human_question",
                    id: question_id.clone(),
                })?;
                if status != "open" {
                    return Err(StorageError::InvalidTransition(format!(
                        "question {question_id} is {status}"
                    )));
                }
                sqlx::query("UPDATE human_questions SET status='answered',answer_json=?,answered_by=?,answered_at=? WHERE question_id=?")
                    .bind(json_text(&answer)?).bind(&command.requested_by).bind(Utc::now().to_rfc3339()).bind(question_id).execute(&mut *tx).await?;
                let open: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM human_questions WHERE project_id=? AND status='open'",
                )
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?;
                if open == 0 {
                    sqlx::query("UPDATE projects SET status='running' WHERE project_id=? AND status='needs_human_review'").bind(project_id).execute(&mut *tx).await?;
                }
                affected.push(entity("human_question", question_id));
                event_specs.push((
                    "human_question.answered".into(),
                    entity("human_question", question_id),
                    json!({"answer":answer,"answered_by":command.requested_by}),
                ));
            }
            "add_suggestion" => {
                let content = command
                    .payload
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim();
                if content.is_empty() {
                    return Err(StorageError::InvalidTransition(
                        "suggestion content must not be empty".into(),
                    ));
                }
                let target_route_id = command
                    .payload
                    .get("target_route_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|route_id| !route_id.is_empty())
                    .map(str::to_owned);
                if let Some(route_id) = &target_route_id {
                    let belongs_to_project: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM routes WHERE project_id=? AND route_id=?",
                    )
                    .bind(project_id)
                    .bind(route_id)
                    .fetch_one(&mut *tx)
                    .await?;
                    if belongs_to_project != 1 {
                        return Err(StorageError::InvalidTransition(format!(
                            "target_route_id {route_id} is not a route in this project"
                        )));
                    }
                }
                let current_round: i64 =
                    sqlx::query_scalar("SELECT current_round FROM projects WHERE project_id = ?")
                        .bind(project_id)
                        .fetch_one(&mut *tx)
                        .await?;
                let suggestion_id = new_id("suggestion");
                sqlx::query("INSERT INTO suggestions(suggestion_id,project_id,command_id,content,target_route_id,status,created_in_round,effective_round) VALUES(?,?,?,?,?,'pending',?,?)")
                    .bind(&suggestion_id).bind(project_id).bind(command_id).bind(content)
                    .bind(target_route_id.as_deref()).bind(current_round).bind(current_round + 1).execute(&mut *tx).await?;
                sqlx::query("UPDATE human_commands SET status = 'validated' WHERE command_id = ?")
                    .bind(command_id)
                    .execute(&mut *tx)
                    .await?;
                let revision = bump_revision(&mut tx, project_id).await?;
                let event = append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "human_command.validated",
                    entity("command", command_id),
                    json!({"effective_round": current_round + 1, "suggestion_id": suggestion_id}),
                    Some(entity("command", command_id)),
                )
                .await?;
                tx.commit().await?;
                command.status = CommandStatus::Validated;
                return Ok((command, vec![event]));
            }
            "trigger_replan" => {
                affected.push(entity("project", project_id));
                event_specs.push((
                    "replan.queued".into(),
                    entity("project", project_id),
                    json!({}),
                ));
            }
            other => {
                return Err(StorageError::InvalidTransition(format!(
                    "unsupported command {other}"
                )));
            }
        }
        let revision = bump_revision(&mut tx, project_id).await?;
        let applied_at = Utc::now();
        sqlx::query("UPDATE human_commands SET status = 'applied', after_revision = ?, affected_entities_json = ?, applied_at = ? WHERE command_id = ?")
            .bind(revision).bind(json_text(&affected)?).bind(applied_at.to_rfc3339()).bind(command_id).execute(&mut *tx).await?;
        let mut events = Vec::new();
        for (event_type, event_entity, data) in event_specs {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    &event_type,
                    event_entity,
                    data,
                    Some(entity("command", command_id)),
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
                entity("command", command_id),
                json!({"affected_entities": affected}),
                None,
            )
            .await?,
        );
        tx.commit().await?;
        command.status = CommandStatus::Applied;
        command.after_revision = Some(revision);
        command.affected_entities = affected;
        command.applied_at = Some(applied_at);
        Ok((command, events))
    }

    pub async fn fail_command(
        &self,
        project_id: &str,
        command_id: &str,
        message: &str,
    ) -> StorageResult<(HumanCommand, Option<DomainEvent>)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "fail_command",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query("SELECT * FROM human_commands WHERE project_id=? AND command_id=?")
            .bind(project_id)
            .bind(command_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "command",
                id: command_id.into(),
            })?;
        let command = rows::command(&row)?;
        if !matches!(
            command.status,
            CommandStatus::Queued | CommandStatus::Validated | CommandStatus::WaitingSafePoint
        ) {
            tx.rollback().await?;
            return Ok((command, None));
        }
        let changed = sqlx::query("UPDATE human_commands SET status = 'failed', error = ? WHERE project_id = ? AND command_id = ? AND status IN ('queued','validated','waiting_safe_point')")
            .bind(message).bind(project_id).bind(command_id).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok((self.get_command(project_id, command_id).await?, None));
        }
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            "human_command.failed",
            entity("command", command_id),
            json!({"error": message}),
            None,
        )
        .await?;
        let row = sqlx::query("SELECT * FROM human_commands WHERE command_id = ?")
            .bind(command_id)
            .fetch_one(&mut *tx)
            .await?;
        let command = rows::command(&row)?;
        tx.commit().await?;
        Ok((command, Some(event)))
    }

    /// Returns durable commands whose dispatch did not complete before a
    /// process interruption. Callers may safely race: `apply_command` claims
    /// only the queued state under the `StateWriter`.
    pub async fn pending_command_ids(
        &self,
        project_id: Option<&str>,
    ) -> StorageResult<Vec<(String, String)>> {
        let rows = if let Some(project_id) = project_id {
            sqlx::query("SELECT project_id,command_id FROM human_commands WHERE project_id=? AND status='queued' ORDER BY created_at,command_id")
                .bind(project_id)
                .fetch_all(self.pool())
                .await?
        } else {
            sqlx::query("SELECT project_id,command_id FROM human_commands WHERE status='queued' ORDER BY created_at,command_id")
                .fetch_all(self.pool())
                .await?
        };
        rows.iter()
            .map(|row| Ok((row.try_get("project_id")?, row.try_get("command_id")?)))
            .collect()
    }

    pub async fn recover_project(&self, project_id: &str) -> StorageResult<Vec<DomainEvent>> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "recover_legacy_project",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let project_status: String =
            sqlx::query_scalar("SELECT status FROM projects WHERE project_id=?")
                .bind(project_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StorageError::NotFound {
                    kind: "project",
                    id: project_id.into(),
                })?;
        if project_status != "running" {
            tx.rollback().await?;
            return Ok(vec![]);
        }
        let active_round = sqlx::query(
            "SELECT round_id,status FROM rounds WHERE project_id=? ORDER BY number DESC LIMIT 1",
        )
        .bind(project_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(round_row) = active_round else {
            tx.rollback().await?;
            return Ok(vec![]);
        };
        let round_status: String = round_row.try_get("status")?;
        if matches!(
            round_status.as_str(),
            "completed" | "failed" | "interrupted"
        ) {
            tx.rollback().await?;
            return Ok(vec![]);
        }
        let round_id: String = round_row.try_get("round_id")?;
        let has_v2_plan: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM plan_revisions WHERE project_id=? AND round_id=? AND status='committed'",
        )
        .bind(project_id)
        .bind(&round_id)
        .fetch_one(&mut *tx)
        .await?;
        if has_v2_plan > 0 {
            // V2 recovery is lease/envelope driven. Interrupting this round here would
            // discard resumable tasks and race the startup reconciler.
            tx.rollback().await?;
            return Ok(vec![]);
        }
        sqlx::query("UPDATE rounds SET status='interrupted',completed_at=?,summary='Recovered after process interruption' WHERE round_id=?")
            .bind(Utc::now().to_rfc3339()).bind(&round_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE tasks SET status='cancelled',revision=revision+1,result_summary=COALESCE(result_summary,'Cancelled during recovery') WHERE project_id=? AND status IN ('open','assigned','running','blocked')")
            .bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE workers SET status='offline',current_task_id=NULL,current_route_id=NULL WHERE project_id=? AND status IN ('idle','running','paused')")
            .bind(project_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE verifications SET status='submitted',started_at=NULL WHERE project_id=? AND status='verifying'")
            .bind(project_id).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE candidates SET status='submitted' WHERE project_id=? AND status='verifying'",
        )
        .bind(project_id)
        .execute(&mut *tx)
        .await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            "project.recovered",
            entity("project", project_id),
            json!({"interrupted_round_id":round_id}),
            None,
        )
        .await?;
        tx.commit().await?;
        Ok(vec![event])
    }

    pub async fn begin_round(
        &self,
        project_id: &str,
    ) -> StorageResult<(ResearchRound, DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "begin_round",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query(
            "SELECT status, revision, current_round FROM projects WHERE project_id = ?",
        )
        .bind(project_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "project",
            id: project_id.into(),
        })?;
        let status: String = row.try_get("status")?;
        if status != "running" {
            return Err(StorageError::InvalidTransition(format!(
                "cannot begin round while project is {status}"
            )));
        }
        let based_on_revision: i64 = row.try_get("revision")?;
        let number: i64 = row.try_get::<i64, _>("current_round")? + 1;
        if let Some(open_status) = sqlx::query_scalar::<_, String>(
            "SELECT status FROM rounds WHERE project_id = ? ORDER BY number DESC LIMIT 1",
        )
        .bind(project_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            if !matches!(open_status.as_str(), "completed" | "failed" | "interrupted") {
                return Err(StorageError::InvalidTransition(format!(
                    "round already active: {open_status}"
                )));
            }
        }
        let round_id = new_id("round");
        let now = Utc::now();
        sqlx::query("INSERT INTO rounds(round_id,project_id,number,status,based_on_revision,started_at) VALUES(?,?,?,?,?,?)")
            .bind(&round_id).bind(project_id).bind(number).bind(RoundStatus::Planning.to_string()).bind(based_on_revision).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        sqlx::query("UPDATE projects SET current_round = ? WHERE project_id = ?")
            .bind(number)
            .bind(project_id)
            .execute(&mut *tx)
            .await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            "round.started",
            entity("round", &round_id),
            json!({"number": number}),
            None,
        )
        .await?;
        tx.commit().await?;
        Ok((
            ResearchRound {
                round_id,
                project_id: project_id.into(),
                number,
                status: RoundStatus::Planning,
                based_on_revision,
                started_at: Some(now),
                completed_at: None,
                summary: None,
            },
            event,
        ))
    }

    pub async fn record_planning_stage(
        &self,
        project_id: &str,
        round_id: &str,
        stage: &str,
        input_hash: &str,
        output: &Value,
    ) -> StorageResult<Option<DomainEvent>> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "record_planning_stage",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let round_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM rounds WHERE project_id=? AND round_id=? AND status='planning'",
        )
        .bind(project_id)
        .bind(round_id)
        .fetch_one(&mut *tx)
        .await?;
        if round_exists != 1 {
            return Err(StorageError::InvalidTransition(
                "planning stage requires the active planning round".into(),
            ));
        }
        if let Some(existing_hash) = sqlx::query_scalar::<_, String>(
            "SELECT input_hash FROM planning_stage_runs WHERE round_id=? AND stage=?",
        )
        .bind(round_id)
        .bind(stage)
        .fetch_optional(&mut *tx)
        .await?
        {
            if existing_hash != input_hash {
                return Err(StorageError::InvalidTransition(format!(
                    "planning stage {stage} was already recorded with a different input"
                )));
            }
            tx.rollback().await?;
            return Ok(None);
        }
        let stage_run_id = new_id("planningstage");
        sqlx::query("INSERT INTO planning_stage_runs(stage_run_id,project_id,round_id,stage,status,input_hash,output_json,created_at) VALUES(?,?,?,?,?,?,?,?)")
            .bind(&stage_run_id).bind(project_id).bind(round_id).bind(stage).bind("completed")
            .bind(input_hash).bind(json_text(output)?).bind(Utc::now().to_rfc3339())
            .execute(&mut *tx).await?;
        let revision = current_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            "planning.stage.completed",
            entity("planning_stage", &stage_run_id),
            json!({"round_id":round_id,"stage":stage,"input_hash":input_hash}),
            Some(entity("round", round_id)),
        )
        .await?;
        tx.commit().await?;
        Ok(Some(event))
    }

    pub async fn begin_planning_stage_attempt(
        &self,
        project_id: &str,
        round_id: &str,
        stage: &str,
        input_hash: &str,
        attempt_number: i64,
        timeout_seconds: (i64, i64),
    ) -> StorageResult<(String, Option<DomainEvent>)> {
        let (soft_timeout_seconds, hard_timeout_seconds) = timeout_seconds;
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "begin_planning_stage_attempt",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        if let Some(attempt_id) = sqlx::query_scalar::<_, String>("SELECT attempt_id FROM planning_stage_attempts WHERE round_id=? AND stage=? AND input_hash=? AND attempt_number=?")
            .bind(round_id).bind(stage).bind(input_hash).bind(attempt_number)
            .fetch_optional(&mut *tx).await? {
            tx.rollback().await?;
            return Ok((attempt_id, None));
        }
        let runnable: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM rounds r JOIN projects p ON p.project_id=r.project_id WHERE r.project_id=? AND r.round_id=? AND r.status='planning' AND p.status='running'",
        )
        .bind(project_id)
        .bind(round_id)
        .fetch_one(&mut *tx)
        .await?;
        if runnable != 1 {
            return Err(StorageError::InvalidTransition(
                "planning attempt requires a running project and active planning round".into(),
            ));
        }
        let attempt_id = new_id("planningattempt");
        let now = Utc::now();
        let soft_deadline = now + chrono::Duration::seconds(soft_timeout_seconds.max(1));
        let hard_deadline = now + chrono::Duration::seconds(hard_timeout_seconds.max(1));
        sqlx::query("INSERT INTO planning_stage_attempts(attempt_id,project_id,round_id,stage,input_hash,attempt_number,status,soft_deadline_at,hard_deadline_at,last_heartbeat_at,started_at) VALUES(?,?,?,?,?,?,'running',?,?,?,?)")
            .bind(&attempt_id).bind(project_id).bind(round_id).bind(stage).bind(input_hash).bind(attempt_number)
            .bind(soft_deadline.to_rfc3339()).bind(hard_deadline.to_rfc3339()).bind(now.to_rfc3339()).bind(now.to_rfc3339())
            .execute(&mut *tx).await?;
        let revision = current_revision(&mut tx, project_id).await?;
        let event = append_event(&mut tx,project_id,revision,"planning.stage.attempt.started",entity("planning_stage_attempt",&attempt_id),json!({"round_id":round_id,"stage":stage,"attempt_number":attempt_number,"soft_timeout_seconds":soft_timeout_seconds,"hard_timeout_seconds":hard_timeout_seconds}),Some(entity("round",round_id))).await?;
        tx.commit().await?;
        Ok((attempt_id, Some(event)))
    }

    pub async fn mark_planning_stage_soft_budget_exceeded(
        &self,
        attempt_id: &str,
    ) -> StorageResult<Option<DomainEvent>> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "planning_stage_soft_budget",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query("SELECT project_id,round_id,stage FROM planning_stage_attempts WHERE attempt_id=? AND status='running'")
            .bind(attempt_id).fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Ok(None);
        };
        let project_id: String = row.try_get("project_id")?;
        let round_id: String = row.try_get("round_id")?;
        let stage: String = row.try_get("stage")?;
        sqlx::query("UPDATE planning_stage_attempts SET soft_budget_exceeded=1,last_heartbeat_at=? WHERE attempt_id=? AND status='running'")
            .bind(Utc::now().to_rfc3339()).bind(attempt_id).execute(&mut *tx).await?;
        let revision = current_revision(&mut tx, &project_id).await?;
        let event = append_event(
            &mut tx,
            &project_id,
            revision,
            "planning.stage.soft_budget_exceeded",
            entity("planning_stage_attempt", attempt_id),
            json!({"round_id":round_id,"stage":stage,"action":"continue_until_hard_deadline"}),
            Some(entity("round", &round_id)),
        )
        .await?;
        tx.commit().await?;
        Ok(Some(event))
    }

    pub async fn heartbeat_planning_stage_attempt(&self, attempt_id: &str) -> StorageResult<()> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "planning_stage_heartbeat",
            )
            .await?;
        sqlx::query("UPDATE planning_stage_attempts SET last_heartbeat_at=? WHERE attempt_id=? AND status='running'")
            .bind(Utc::now().to_rfc3339())
            .bind(attempt_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn complete_planning_stage_attempt(
        &self,
        attempt_id: &str,
        succeeded: bool,
        error: Option<&str>,
    ) -> StorageResult<()> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "complete_planning_stage_attempt",
            )
            .await?;
        sqlx::query("UPDATE planning_stage_attempts SET status=?,error=?,last_heartbeat_at=?,completed_at=? WHERE attempt_id=? AND status='running'")
            .bind(if succeeded { "completed" } else { "failed" }).bind(error)
            .bind(Utc::now().to_rfc3339()).bind(Utc::now().to_rfc3339()).bind(attempt_id)
            .execute(self.pool()).await?;
        Ok(())
    }

    pub async fn list_planning_stages(
        &self,
        project_id: &str,
        round_id: &str,
    ) -> StorageResult<Vec<Value>> {
        let rows = sqlx::query("SELECT stage_run_id,stage,status,input_hash,output_json,created_at FROM planning_stage_runs WHERE project_id=? AND round_id=? ORDER BY created_at,stage_run_id")
            .bind(project_id).bind(round_id).fetch_all(self.pool()).await?;
        rows.iter()
            .map(|row| {
                Ok(json!({
                    "stage_run_id":row.try_get::<String,_>("stage_run_id")?,
                    "stage":row.try_get::<String,_>("stage")?,
                    "status":row.try_get::<String,_>("status")?,
                    "input_hash":row.try_get::<String,_>("input_hash")?,
                    "output":serde_json::from_str::<Value>(row.try_get("output_json")?)?,
                    "created_at":row.try_get::<String,_>("created_at")?,
                }))
            })
            .collect()
    }

    /// The first persisted attempt anchors the shared planning budget across
    /// stages, retries and service restarts. Reusing an attempt never resets it.
    pub async fn planning_round_started_at(
        &self,
        project_id: &str,
        round_id: &str,
    ) -> StorageResult<Option<chrono::DateTime<Utc>>> {
        let started = sqlx::query_scalar::<_, Option<String>>(
            "SELECT MIN(started_at) FROM planning_stage_attempts WHERE project_id=? AND round_id=?",
        )
        .bind(project_id)
        .bind(round_id)
        .fetch_one(self.pool())
        .await?;
        rows::optional_timestamp(started)
    }

    pub async fn planning_stage_attempt_completed(
        &self,
        project_id: &str,
        round_id: &str,
        stage: &str,
        input_hash: &str,
    ) -> StorageResult<bool> {
        let completed: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM planning_stage_attempts WHERE project_id=? AND round_id=? AND stage=? AND input_hash=? AND status='completed')",
        )
        .bind(project_id)
        .bind(round_id)
        .bind(stage)
        .bind(input_hash)
        .fetch_one(self.pool())
        .await?;
        Ok(completed != 0)
    }

    pub async fn save_plan(
        &self,
        project_id: &str,
        round: &ResearchRound,
        plan: &PlannerOutput,
    ) -> StorageResult<PlanSaveResult> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "commit_legacy_plan",
            )
            .await?;
        if plan.routes.len() < 2 {
            return Err(StorageError::InvalidTransition(
                "planner must return at least two routes".into(),
            ));
        }
        let mut tx = self.pool().begin().await?;
        let actual = current_revision(&mut tx, project_id).await?;
        if actual != round.based_on_revision + 1 {
            return Err(StorageError::RevisionConflict {
                expected: round.based_on_revision + 1,
                actual,
            });
        }
        let main_goal_id: String = sqlx::query_scalar(
            "SELECT goal_id FROM goals WHERE project_id = ? ORDER BY priority DESC LIMIT 1",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        let mut routes = Vec::new();
        for proposal in &plan.routes {
            let route_id = new_id("route");
            let target_goal_ids = if proposal.target_goal_ids.is_empty() {
                vec![main_goal_id.clone()]
            } else {
                proposal.target_goal_ids.clone()
            };
            let score = proposal.score();
            let attributes = route_attributes(proposal);
            sqlx::query("INSERT INTO routes(route_id,project_id,title,method_summary,target_goal_ids_json,required_fact_ids_json,status,score,priority,cancellation_epoch,created_in_round,attributes_json) VALUES(?,?,?,?,?,?,?,?,?,0,?,?)")
                .bind(&route_id).bind(project_id).bind(&proposal.title).bind(&proposal.method_summary)
                .bind(json_text(&target_goal_ids)?).bind(json_text(&proposal.required_fact_ids)?)
                .bind(RouteStatus::Active.to_string()).bind(score).bind(score).bind(round.number).bind(json_text(&attributes)?).execute(&mut *tx).await?;
            let hypothesis_id = new_id("hyp");
            sqlx::query("INSERT INTO hypotheses(hypothesis_id,project_id,kind,statement,status,route_id,created_in_round,attributes_json) VALUES(?,?,?,?,?,?,?,?)")
                .bind(&hypothesis_id).bind(project_id).bind("route").bind(&proposal.method_summary).bind("active").bind(&route_id).bind(round.number).bind(json_text(&attributes)?).execute(&mut *tx).await?;
            for goal_id in &target_goal_ids {
                sqlx::query("INSERT OR IGNORE INTO hypothesis_edges(edge_id,project_id,source_id,target_id,kind) VALUES(?,?,?,?,?)")
                    .bind(new_id("edge")).bind(project_id).bind(&route_id).bind(goal_id).bind("targets").execute(&mut *tx).await?;
            }
            routes.push(Route {
                route_id,
                project_id: project_id.into(),
                title: proposal.title.clone(),
                method_summary: proposal.method_summary.clone(),
                target_goal_ids,
                required_fact_ids: proposal.required_fact_ids.clone(),
                status: RouteStatus::Active,
                score,
                priority: score,
                cancellation_epoch: 0,
                created_in_round: round.number,
                family_id: None,
                semantic_fingerprint: None,
                consecutive_no_progress_plans: 0,
                failed_attempt_count: 0,
                created_at_revision: round.based_on_revision,
                last_material_progress_revision: None,
                merged_into: None,
                merge_reason: None,
                exit_criteria: Vec::new(),
                attributes,
            });
        }
        let mut tasks = Vec::new();
        let mut workers = Vec::new();
        for assignment in &plan.assignments {
            let route = routes.get(assignment.route_index).ok_or_else(|| {
                StorageError::InvalidTransition(format!(
                    "assignment route_index {} out of range",
                    assignment.route_index
                ))
            })?;
            let task_id = new_id("task");
            let worker_id = new_id("worker");
            let goal_ids = if assignment.goal_ids.is_empty() {
                route.target_goal_ids.clone()
            } else {
                assignment.goal_ids.clone()
            };
            sqlx::query("INSERT INTO workers(worker_id,project_id,role,backend,status,current_task_id,current_route_id,last_heartbeat) VALUES(?,?,?,?,?,?,?,?)")
                .bind(&worker_id).bind(project_id).bind(&assignment.worker_role).bind("codex_cli").bind(WorkerStatus::Idle.to_string()).bind(&task_id).bind(&route.route_id).bind(Utc::now().to_rfc3339()).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO tasks(task_id,project_id,route_id,worker_id,worker_role,goal_ids_json,objective,completion_contract,status,priority,revision,route_cancellation_epoch,round) VALUES(?,?,?,?,?,?,?,?,?,?,1,?,?)")
                .bind(&task_id).bind(project_id).bind(&route.route_id).bind(&worker_id).bind(&assignment.worker_role).bind(json_text(&goal_ids)?)
                .bind(&assignment.objective).bind(&assignment.completion_contract).bind(TaskStatus::Assigned.to_string()).bind(assignment.priority).bind(route.cancellation_epoch).bind(round.number).execute(&mut *tx).await?;
            tasks.push(Task {
                task_id: task_id.clone(),
                project_id: project_id.into(),
                route_id: route.route_id.clone(),
                worker_id: Some(worker_id.clone()),
                worker_role: assignment.worker_role.clone(),
                goal_ids,
                objective: assignment.objective.clone(),
                completion_contract: assignment.completion_contract.clone(),
                status: TaskStatus::Assigned,
                priority: assignment.priority,
                revision: 1,
                route_cancellation_epoch: route.cancellation_epoch,
                round: round.number,
                result_summary: None,
                plan_revision_id: None,
                task_signature: None,
                context_packet_id: None,
            });
            workers.push(Worker {
                worker_id,
                project_id: project_id.into(),
                role: assignment.worker_role.clone(),
                backend: "codex_cli".into(),
                status: WorkerStatus::Idle,
                current_task_id: Some(task_id),
                current_route_id: Some(route.route_id.clone()),
                session_id: None,
                last_heartbeat: Some(Utc::now()),
            });
        }
        sqlx::query("UPDATE rounds SET status = 'running' WHERE round_id = ?")
            .bind(&round.round_id)
            .execute(&mut *tx)
            .await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        let mut events = vec![append_event(&mut tx, project_id, revision, "plan.created", entity("round", &round.round_id), json!({"routes": routes.len(), "tasks": tasks.len(), "summary": plan.rationale_summary}), None).await?];
        for route in &routes {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "route.created",
                    entity("route", &route.route_id),
                    json!({"score": route.score, "title": route.title}),
                    None,
                )
                .await?,
            );
        }
        for task in &tasks {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "task.assigned",
                    entity("task", &task.task_id),
                    json!({"worker_id": task.worker_id, "route_id": task.route_id}),
                    None,
                )
                .await?,
            );
        }
        let suggestion_resolution_time = Utc::now();
        events.extend(
            crate::suggestions::resolve_plan_suggestions(
                &mut tx,
                project_id,
                round.number,
                revision,
                None,
                &plan.suggestion_decisions,
                &suggestion_resolution_time,
            )
            .await?,
        );
        tx.commit().await?;
        let mut saved_round = round.clone();
        saved_round.status = RoundStatus::Running;
        Ok(PlanSaveResult {
            round: saved_round,
            routes,
            tasks,
            workers,
            events,
        })
    }

    pub async fn mark_task_running(
        &self,
        project_id: &str,
        task_id: &str,
    ) -> StorageResult<(Task, DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "mark_legacy_task_running",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let task_row = sqlx::query("SELECT * FROM tasks WHERE project_id = ? AND task_id = ?")
            .bind(project_id)
            .bind(task_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "task",
                id: task_id.into(),
            })?;
        let mut task = rows::task(&task_row)?;
        if task.status != TaskStatus::Assigned {
            return Err(StorageError::InvalidTransition(format!(
                "task {} is {}",
                task_id, task.status
            )));
        }
        let claimed = sqlx::query(
            "UPDATE tasks SET status = 'running', revision = revision + 1 WHERE task_id = ? AND status='assigned'",
        )
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
        if claimed.rows_affected() != 1 {
            return Err(StorageError::LateSubmission(format!(
                "task {task_id} was claimed by another executor"
            )));
        }
        if let Some(worker_id) = &task.worker_id {
            sqlx::query(
                "UPDATE workers SET status = 'running', last_heartbeat = ? WHERE worker_id = ?",
            )
            .bind(Utc::now().to_rfc3339())
            .bind(worker_id)
            .execute(&mut *tx)
            .await?;
        }
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            "task.started",
            entity("task", task_id),
            json!({"worker_id": task.worker_id}),
            None,
        )
        .await?;
        tx.commit().await?;
        task.status = TaskStatus::Running;
        task.revision += 1;
        Ok((task, event))
    }

    pub async fn record_worker_output(
        &self,
        task: &Task,
        output: &WorkerOutput,
    ) -> StorageResult<Vec<DomainEvent>> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "record_legacy_worker_output",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let current = sqlx::query("SELECT t.status,t.revision,r.status AS route_status,r.cancellation_epoch FROM tasks t JOIN routes r ON t.route_id=r.route_id WHERE t.task_id=?")
            .bind(&task.task_id).fetch_one(&mut *tx).await?;
        let task_status: String = current.try_get("status")?;
        let task_revision: i64 = current.try_get("revision")?;
        let route_status: String = current.try_get("route_status")?;
        let epoch: i64 = current.try_get("cancellation_epoch")?;
        if task_status != "running"
            || task_revision != task.revision
            || epoch != task.route_cancellation_epoch
            || route_status == "human_stopped"
        {
            return Err(StorageError::LateSubmission(format!(
                "task status={task_status}, revision={task_revision}, route status={route_status}, epoch={epoch}"
            )));
        }
        let ingestion = crate::worker_output_ingestion::ingest_worker_output_tx(
            &mut tx,
            task,
            output,
            crate::worker_output_ingestion::WorkerOutputOrigin::Legacy,
        )
        .await?;
        let crate::worker_output_ingestion::WorkerOutputIngestion {
            recorded_sources,
            rejected_source_drafts,
            capsule_events,
            ..
        } = ingestion;
        sqlx::query("UPDATE tasks SET status='completed', result_summary=?, revision=revision+1 WHERE task_id=?")
            .bind(&output.summary).bind(&task.task_id).execute(&mut *tx).await?;
        if let Some(worker_id) = &task.worker_id {
            sqlx::query("UPDATE workers SET status='idle', current_task_id=NULL, current_route_id=NULL, last_heartbeat=? WHERE worker_id=?")
            .bind(Utc::now().to_rfc3339()).bind(worker_id).execute(&mut *tx).await?;
        }
        let revision = bump_revision(&mut tx, &task.project_id).await?;
        let mut events = Vec::with_capacity(
            capsule_events.len() + rejected_source_drafts.len() + recorded_sources.len() + 1,
        );
        for (failure_id, reasons) in rejected_source_drafts {
            events.push(
                append_event(
                    &mut tx,
                    &task.project_id,
                    revision,
                    "source.draft.rejected",
                    entity("failure", &failure_id),
                    json!({"task_id":task.task_id,"route_id":task.route_id,"reasons":reasons}),
                    Some(entity("task", &task.task_id)),
                )
                .await?,
            );
        }
        for (source_id, title, admission) in &recorded_sources {
            events.push(
                append_event(
                    &mut tx,
                    &task.project_id,
                    revision,
                    admission.event_type(),
                    entity("source", source_id),
                    json!({"task_id":task.task_id,"route_id":task.route_id,"title":title,"trust":admission.stored_status()}),
                    Some(entity("task", &task.task_id)),
                )
                .await?,
            );
        }
        for (capsule_id, content_hash) in capsule_events {
            events.push(
                append_event(
                    &mut tx,
                    &task.project_id,
                    revision,
                    "experiment.capsule.recorded",
                    entity("experiment_capsule", &capsule_id),
                    json!({"task_id": task.task_id, "route_id": task.route_id, "content_hash": content_hash, "trust": "reported_unverified"}),
                    Some(entity("task", &task.task_id)),
                )
                .await?,
            );
        }
        events.push(append_event(
            &mut tx,
            &task.project_id,
            revision,
            "task.completed",
            entity("task", &task.task_id),
            json!({"summary": output.summary, "candidate_count": output.candidates.len(), "source_draft_count": output.sources.len(), "recorded_source_count":recorded_sources.len(), "experiment_count": output.experiments.len()}),
            None,
        )
        .await?);
        tx.commit().await?;
        Ok(events)
    }

    /// Compress repeated raw failures into an auditable pattern without deleting evidence.
    pub async fn compress_failures(
        &self,
        project_id: &str,
        minimum_occurrences: i64,
    ) -> StorageResult<Vec<DomainEvent>> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "compress_failures",
            )
            .await?;
        let threshold = minimum_occurrences.max(2);
        let groups = sqlx::query("SELECT failure_type, COUNT(*) AS failure_count FROM failures WHERE project_id=? GROUP BY failure_type HAVING COUNT(*) >= ?")
            .bind(project_id)
            .bind(threshold)
            .fetch_all(self.pool())
            .await?;
        let mut events = Vec::new();
        for group in groups {
            let failure_type: String = group.try_get("failure_type")?;
            let title = format!("repeated:{failure_type}");
            let exists = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM failure_patterns WHERE project_id=? AND title=?",
            )
            .bind(project_id)
            .bind(&title)
            .fetch_one(self.pool())
            .await?;
            if exists > 0 {
                continue;
            }
            let failures = sqlx::query("SELECT failure_id,summary,route_id,task_id FROM failures WHERE project_id=? AND failure_type=? ORDER BY created_at")
                .bind(project_id)
                .bind(&failure_type)
                .fetch_all(self.pool())
                .await?;
            let source_failure_ids: Vec<String> = failures
                .iter()
                .map(|row| row.try_get("failure_id"))
                .collect::<Result<_, _>>()?;
            let examples: Vec<Value> = failures
                .iter()
                .take(5)
                .map(|row| {
                    Ok(json!({
                        "summary": row.try_get::<String, _>("summary")?,
                        "route_id": row.try_get::<Option<String>, _>("route_id")?,
                        "task_id": row.try_get::<Option<String>, _>("task_id")?,
                    }))
                })
                .collect::<Result<_, sqlx::Error>>()?;
            let pattern_id = new_id("pattern");
            let count = i64::try_from(source_failure_ids.len()).unwrap_or(i64::MAX);
            let summary = format!("{count} failures share type {failure_type}");
            let pattern = json!({
                "failure_type": failure_type,
                "source_failure_ids": source_failure_ids,
                "examples": examples,
                "occurrences": count,
            });
            let count_for_score = u32::try_from(count).unwrap_or(u32::MAX);
            let confidence = (0.5 + (f64::from(count_for_score) * 0.05)).min(0.95);
            let mut tx = self.pool().begin().await?;
            sqlx::query("INSERT INTO failure_patterns(pattern_id,project_id,title,summary,pattern_json,confidence,created_at) VALUES(?,?,?,?,?,?,?)")
                .bind(&pattern_id)
                .bind(project_id)
                .bind(&title)
                .bind(&summary)
                .bind(json_text(&pattern)?)
                .bind(confidence)
                .bind(Utc::now().to_rfc3339())
                .execute(&mut *tx)
                .await?;
            let revision = bump_revision(&mut tx, project_id).await?;
            let event = append_event(
                &mut tx,
                project_id,
                revision,
                "failure_pattern.created",
                entity("failure_pattern", &pattern_id),
                json!({"title": title, "occurrences": count, "confidence": confidence}),
                None,
            )
            .await?;
            tx.commit().await?;
            events.push(event);
        }
        Ok(events)
    }

    pub async fn submit_candidate(
        &self,
        project_id: &str,
        submission: CandidateSubmission,
        idempotency_key: &str,
    ) -> StorageResult<SubmissionReceipt> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "submit_candidate",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let inserted = crate::candidate_ingestion::insert_candidate_tx(
            &mut tx,
            project_id,
            submission,
            idempotency_key,
        )
        .await?;
        if !inserted.inserted {
            tx.rollback().await?;
            return Ok(SubmissionReceipt {
                candidate: inserted.candidate,
                verification: inserted.verification,
                event: None,
            });
        }
        let project_revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            project_revision,
            "claim.submitted",
            entity("candidate", &inserted.candidate.candidate_id),
            json!({"verification_id": inserted.verification.verification_id}),
            Some(entity("task", &inserted.candidate.submission.task_id)),
        )
        .await?;
        tx.commit().await?;
        Ok(SubmissionReceipt {
            candidate: inserted.candidate,
            verification: inserted.verification,
            event: Some(event),
        })
    }

    pub async fn mark_verification_started(
        &self,
        verification_id: &str,
    ) -> StorageResult<(Verification, DomainEvent)> {
        self.try_mark_verification_started(verification_id)
            .await?
            .ok_or_else(|| {
                StorageError::InvalidTransition(format!(
                    "verification {verification_id} is not submitted"
                ))
            })
    }

    /// Atomically claims a submitted verification for execution.
    ///
    /// `None` means another executor already claimed or completed it. This is
    /// the normal idempotent outcome for concurrent result ingestion.
    pub async fn try_mark_verification_started(
        &self,
        verification_id: &str,
    ) -> StorageResult<Option<(Verification, DomainEvent)>> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "try_mark_verification_started",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let now = Utc::now();
        let row = sqlx::query("SELECT v.* FROM verifications v WHERE v.verification_id=?")
            .bind(verification_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "verification",
                id: verification_id.into(),
            })?;
        let mut verification = rows::verification(&row)?;
        let route_released =
            crate::verification_execution_is_released(&mut tx, verification_id).await?;
        if verification.status != CandidateStatus::Submitted || !route_released {
            tx.rollback().await?;
            return Ok(None);
        }
        let claimed = sqlx::query("UPDATE verifications SET status='verifying',started_at=? WHERE verification_id=? AND status='submitted'")
            .bind(now.to_rfc3339())
            .bind(verification_id)
            .execute(&mut *tx)
            .await?;
        if claimed.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(None);
        }
        sqlx::query("UPDATE candidates SET status='verifying' WHERE candidate_id=?")
            .bind(&verification.candidate_id)
            .execute(&mut *tx)
            .await?;
        let revision = bump_revision(&mut tx, &verification.project_id).await?;
        let event = append_event(
            &mut tx,
            &verification.project_id,
            revision,
            "verification.started",
            entity("verification", verification_id),
            json!({"candidate_id": verification.candidate_id}),
            None,
        )
        .await?;
        tx.commit().await?;
        verification.status = CandidateStatus::Verifying;
        verification.started_at = Some(now);
        Ok(Some((verification, event)))
    }

    /// Releases a verification claim after orchestration failed before a
    /// verdict was committed. The claim and its in-flight worker state are
    /// reset together so a later recovery can safely retry it.
    pub async fn release_verification_claim(
        &self,
        verification_id: &str,
        reason: &str,
    ) -> StorageResult<Option<DomainEvent>> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "release_verification_claim",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query("SELECT v.project_id,v.candidate_id,v.status,p.status AS project_status FROM verifications v JOIN projects p ON p.project_id=v.project_id WHERE v.verification_id=?")
            .bind(verification_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "verification",
                id: verification_id.into(),
            })?;
        let status: String = row.try_get("status")?;
        if status != "verifying" {
            tx.rollback().await?;
            return Ok(None);
        }
        let project_id: String = row.try_get("project_id")?;
        let candidate_id: String = row.try_get("candidate_id")?;
        let project_status: String = row.try_get("project_status")?;
        let route_released =
            crate::verification_execution_is_released(&mut tx, verification_id).await?;
        let retryable = project_status == "paused" || route_released;
        let next_status = if retryable { "submitted" } else { "unknown" };
        let now = Utc::now();
        let report = (!retryable).then(|| VerificationReport {
            verdict: VerificationVerdict::Unknown,
            summary: "Verification was interrupted after the project left its runnable state."
                .into(),
            critical_errors: Vec::new(),
            gaps: Vec::new(),
            uncertainties: vec![reason.to_owned()],
            repair_actions: Vec::new(),
            checked_fact_ids: Vec::new(),
            checked_source_ids: Vec::new(),
            evidence_level: "system_interrupted".into(),
        });
        let updated = sqlx::query("UPDATE verifications SET status=?,report_json=?,started_at=NULL,completed_at=? WHERE verification_id=? AND status='verifying'")
            .bind(next_status)
            .bind(report.as_ref().map(json_text).transpose()?)
            .bind((!retryable).then(|| now.to_rfc3339()))
            .bind(verification_id)
            .execute(&mut *tx)
            .await?;
        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(None);
        }
        sqlx::query("UPDATE candidates SET status=? WHERE candidate_id=? AND status='verifying'")
            .bind(next_status)
            .bind(&candidate_id)
            .execute(&mut *tx)
            .await?;
        let case_ids = sqlx::query_scalar::<_, String>(
            "SELECT case_id FROM verification_cases WHERE verification_id=?",
        )
        .bind(verification_id)
        .fetch_all(&mut *tx)
        .await?;
        for case_id in &case_ids {
            sqlx::query("UPDATE verification_cases SET stage=CASE WHEN ? THEN CASE WHEN snapshot_id IS NULL THEN 'intake' ELSE 'precheck' END ELSE 'cancelled' END,cancellation_epoch=cancellation_epoch+1,achieved_acceptance=NULL,completed_at=?,updated_at=? WHERE case_id=? AND stage NOT IN ('committed','rejected','unknown','failed','cancelled')")
                .bind(retryable)
                .bind((!retryable).then(|| now.to_rfc3339()))
                .bind(now.to_rfc3339())
                .bind(case_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE verification_task_leases SET status='failed',completed_at=? WHERE case_id=? AND status='active'")
                .bind(now.to_rfc3339())
                .bind(case_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE verification_attempts SET status='failed',error_kind='orchestrator_error',error_message=?,completed_at=? WHERE case_id=? AND status NOT IN ('completed','failed','orphaned','cancelled')")
                .bind(reason)
                .bind(now.to_rfc3339())
                .bind(case_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE verification_result_envelopes SET status='stale',rejection_reason='verification claim released after orchestration error' WHERE case_id=? AND status='submitted'")
                .bind(case_id)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("UPDATE worker_instances SET status='exited',quarantine_reason=COALESCE(quarantine_reason,?),exited_at=? WHERE project_id=? AND worker_instance_id IN (SELECT worker_instance_id FROM verification_attempts WHERE case_id IN (SELECT case_id FROM verification_cases WHERE verification_id=?)) AND status NOT IN ('exited','quarantined')")
            .bind(reason)
            .bind(now.to_rfc3339())
            .bind(&project_id)
            .bind(verification_id)
            .execute(&mut *tx)
            .await?;
        let revision = bump_revision(&mut tx, &project_id).await?;
        let event = append_event(
            &mut tx,
            &project_id,
            revision,
            if retryable {
                "verification.claim_released"
            } else {
                "verification.interrupted"
            },
            entity("verification", verification_id),
            json!({"candidate_id":candidate_id,"reason":reason,"retryable":retryable}),
            None,
        )
        .await?;
        tx.commit().await?;
        Ok(Some(event))
    }

    #[allow(clippy::missing_panics_doc)]
    pub async fn commit_verification(
        &self,
        verification_id: &str,
        report: VerificationReport,
    ) -> StorageResult<VerificationCommit> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "commit_verification_fact_gate",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query("SELECT v.*,c.submission_json,c.status AS candidate_status,p.status AS project_status FROM verifications v JOIN candidates c ON v.candidate_id=c.candidate_id JOIN projects p ON p.project_id=v.project_id WHERE v.verification_id=?")
            .bind(verification_id).fetch_optional(&mut *tx).await?
            .ok_or_else(|| StorageError::NotFound { kind: "verification", id: verification_id.into() })?;
        let project_id: String = row.try_get("project_id")?;
        let candidate_id: String = row.try_get("candidate_id")?;
        let verification_status: String = row.try_get("status")?;
        let project_status: String = row.try_get("project_status")?;
        if !crate::verification_execution_is_released(&mut tx, verification_id).await? {
            return Err(StorageError::LateSubmission(format!(
                "verification {verification_id} cannot commit while project is {project_status}"
            )));
        }
        if verification_status != CandidateStatus::Verifying.to_string() {
            return Err(StorageError::LateSubmission(format!(
                "verification {verification_id} is {verification_status}, expected verifying"
            )));
        }
        let submission: CandidateSubmission =
            serde_json::from_str(row.try_get("submission_json")?)?;
        let governance_challenge = sqlx::query(
            "SELECT challenge_id,fact_id FROM fact_challenges WHERE verification_id=? AND status='open'",
        )
        .bind(verification_id)
        .fetch_optional(&mut *tx)
        .await?
        .map(|row| {
            Ok::<_, StorageError>((
                row.try_get::<String, _>("challenge_id")?,
                row.try_get::<String, _>("fact_id")?,
            ))
        })
        .transpose()?;
        if governance_challenge.is_none() {
            let execution_fence = sqlx::query(
                "SELECT t.revision AS task_revision,t.route_cancellation_epoch AS task_route_epoch,r.cancellation_epoch AS current_route_epoch FROM tasks t JOIN routes r ON r.project_id=t.project_id AND r.route_id=t.route_id WHERE t.project_id=? AND t.task_id=? AND t.route_id=?",
            )
            .bind(&project_id)
            .bind(&submission.task_id)
            .bind(&submission.route_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                StorageError::LateSubmission(format!(
                    "verification {verification_id} no longer has its authoritative task and route"
                ))
            })?;
            let task_revision: i64 = execution_fence.try_get("task_revision")?;
            let task_route_epoch: i64 = execution_fence.try_get("task_route_epoch")?;
            let current_route_epoch: i64 = execution_fence.try_get("current_route_epoch")?;
            if submission.route_cancellation_epoch != task_route_epoch
                || submission.route_cancellation_epoch != current_route_epoch
            {
                return Err(StorageError::LateSubmission(format!(
                    "verification {verification_id} belongs to route epoch {}, task epoch is {task_route_epoch}, current route epoch is {current_route_epoch}",
                    submission.route_cancellation_epoch
                )));
            }
            if task_revision != submission.task_revision {
                return Err(StorageError::LateSubmission(format!(
                    "verification {verification_id} belongs to task revision {}, current revision is {task_revision}",
                    submission.task_revision
                )));
            }
        }
        let status = match report.verdict {
            VerificationVerdict::Accepted => CandidateStatus::Accepted,
            VerificationVerdict::Rejected => CandidateStatus::Rejected,
            VerificationVerdict::Unknown => CandidateStatus::Unknown,
        };
        let gate = if report.verdict == VerificationVerdict::Accepted {
            let gate = crate::verification::validate_fact_gate(&mut tx, verification_id).await?;
            if !submission.external_source_ids.is_empty() {
                let citation_passed: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM verification_checks c JOIN verification_cases vc ON vc.case_id=c.case_id WHERE vc.verification_id=? AND c.kind='citation_review' AND c.mandatory=1 AND c.status='passed'",
                )
                .bind(verification_id)
                .fetch_one(&mut *tx)
                .await?;
                if citation_passed == 0 {
                    return Err(StorageError::InvalidTransition(
                        "source-backed candidate requires a passed mandatory citation_review"
                            .into(),
                    ));
                }
            }
            Some(gate)
        } else {
            let (case, _) =
                crate::verification::validate_verification_snapshot_fence(&mut tx, verification_id)
                    .await?;
            let adverse_mandatory_checks: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM verification_checks WHERE case_id=? AND mandatory=1 AND status IN ('failed','unknown','error')",
            )
            .bind(&case.case_id)
            .fetch_one(&mut *tx)
            .await?;
            if !matches!(
                case.stage,
                VerificationStage::Adjudication | VerificationStage::CommitReady
            ) && adverse_mandatory_checks == 0
            {
                return Err(StorageError::InvalidTransition(format!(
                    "verification case {} cannot commit an adverse verdict from {} without a failed, unknown, or error mandatory check",
                    case.case_id, case.stage
                )));
            }
            None
        };
        let now = Utc::now();
        let obligation_revision = current_revision(&mut tx, &project_id).await? + 1;
        sqlx::query("UPDATE verifications SET status=?,report_json=?,completed_at=? WHERE verification_id=?")
            .bind(status.to_string()).bind(json_text(&report)?).bind(now.to_rfc3339()).bind(verification_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE candidates SET status=? WHERE candidate_id=?")
            .bind(status.to_string())
            .bind(&candidate_id)
            .execute(&mut *tx)
            .await?;
        let mut fact = None;
        let mut admitted_source_ids = Vec::new();
        let mut advisory_bottleneck_id = None;
        let mut created_obligation_ids = Vec::new();
        let mut updated_obligation_ids = Vec::new();
        let mut obligation_coverage = Vec::new();
        if report.verdict == VerificationVerdict::Accepted {
            for dependency in &submission.dependency_fact_ids {
                let active: Option<String> =
                    sqlx::query_scalar("SELECT f.status FROM facts f WHERE f.fact_id=? AND (f.project_id=? OR EXISTS (SELECT 1 FROM project_fact_imports i WHERE i.target_project_id=? AND i.source_fact_id=f.fact_id AND i.status='active'))")
                        .bind(dependency)
                        .bind(&project_id)
                        .bind(&project_id)
                        .fetch_optional(&mut *tx)
                        .await?;
                if active.as_deref() != Some("active") {
                    return Err(StorageError::InvalidDependency(dependency.clone()));
                }
            }
            let canonical = json!({"project_id": project_id, "statement": submission.statement, "assumptions": submission.assumptions, "proof": submission.proof_markdown, "dependencies": submission.dependency_fact_ids, "definitions": submission.definitions_introduced});
            let content_hash = hex::encode(Sha256::digest(serde_json::to_vec(&canonical)?));
            let global_canonical = json!({"statement": submission.statement, "assumptions": submission.assumptions, "proof": submission.proof_markdown, "dependencies": submission.dependency_fact_ids, "definitions": submission.definitions_introduced});
            let global_content_hash =
                hex::encode(Sha256::digest(serde_json::to_vec(&global_canonical)?));
            let existing: Option<String> =
                sqlx::query_scalar("SELECT fact_id FROM facts WHERE content_hash=?")
                    .bind(&content_hash)
                    .fetch_optional(&mut *tx)
                    .await?;
            let fact_id = existing
                .clone()
                .unwrap_or_else(|| format!("fact_{}", &content_hash[..24]));
            if existing.is_some() {
                let existing_status: String =
                    sqlx::query_scalar("SELECT status FROM facts WHERE fact_id=?")
                        .bind(&fact_id)
                        .fetch_one(&mut *tx)
                        .await?;
                let is_matching_governance_review = governance_challenge
                    .as_ref()
                    .is_some_and(|(_, challenged_fact_id)| challenged_fact_id == &fact_id);
                if existing_status == "revoked" {
                    return Err(StorageError::InvalidTransition(format!(
                        "revoked Fact {fact_id} is terminal and cannot be recertified"
                    )));
                }
                if existing_status != "active" && !is_matching_governance_review {
                    return Err(StorageError::InvalidTransition(format!(
                        "ordinary verification cannot reuse non-active Fact {fact_id} ({existing_status}); an identity-matched open governance challenge is required"
                    )));
                }
            }
            if existing.is_some() {
                let stored: String =
                    sqlx::query_scalar("SELECT verification_ids_json FROM facts WHERE fact_id=?")
                        .bind(&fact_id)
                        .fetch_one(&mut *tx)
                        .await?;
                let mut verification_ids: Vec<String> = serde_json::from_str(&stored)?;
                if !verification_ids.iter().any(|id| id == verification_id) {
                    verification_ids.push(verification_id.into());
                }
                sqlx::query("UPDATE facts SET verification_ids_json=? WHERE fact_id=?")
                    .bind(json_text(&verification_ids)?)
                    .bind(&fact_id)
                    .execute(&mut *tx)
                    .await?;
            } else {
                sqlx::query("INSERT INTO facts(fact_id,project_id,statement,assumptions_json,proof_markdown,dependency_fact_ids_json,definitions_introduced_json,external_source_ids_json,verification_ids_json,evidence_level,created_by,status,content_hash,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
                    .bind(&fact_id).bind(&project_id).bind(&submission.statement).bind(json_text(&submission.assumptions)?).bind(&submission.proof_markdown)
                    .bind(json_text(&submission.dependency_fact_ids)?).bind(json_text(&submission.definitions_introduced)?).bind(json_text(&submission.external_source_ids)?)
                    .bind(json_text(&vec![verification_id.to_owned()])?).bind(&report.evidence_level).bind(&submission.task_id).bind(FactStatus::ActiveFact.to_string()).bind(&content_hash).bind(now.to_rfc3339()).execute(&mut *tx).await?;
            }
            for dependency in &submission.dependency_fact_ids {
                sqlx::query("INSERT OR IGNORE INTO fact_edges(edge_id,project_id,source_id,target_id,kind) VALUES(?,?,?,?,?)")
                    .bind(new_id("edge")).bind(&project_id).bind(dependency).bind(&fact_id).bind("depends_on").execute(&mut *tx).await?;
            }
            let gate = gate.as_ref().expect("accepted verdict has gate context");
            // A passed semantic coverage review is not enough to solve a goal:
            // non-counterexamples need the Fact Gate's strongest assurance.
            let closure_certified = gate.acceptance == AcceptanceClass::FullyCertified;
            let coverage_check = sqlx::query(
                    "SELECT c.check_id,c.status,c.summary,c.details_json FROM verification_checks c JOIN verification_cases vc ON vc.case_id=c.case_id WHERE vc.verification_id=? AND c.kind='goal_coverage_review' ORDER BY c.completed_at DESC LIMIT 1",
                )
                .bind(verification_id)
                .fetch_optional(&mut *tx)
                .await?;
            let passed_coverage = if closure_certified {
                coverage_check.as_ref().filter(|row| {
                    row.try_get::<String, _>("status")
                        .is_ok_and(|status| status == "passed")
                })
            } else {
                None
            };
            let nonpassing_coverage = coverage_check
                .as_ref()
                .filter(|row| {
                    row.try_get::<String, _>("status")
                        .is_ok_and(|status| status != "passed")
                })
                .map(|row| {
                    let status: String = row.try_get("status")?;
                    let summary: String = row.try_get("summary")?;
                    let details_json: String = row.try_get("details_json")?;
                    crate::proof_obligations::nonpassing_goal_coverage_report(
                        &status,
                        &summary,
                        &details_json,
                    )
                })
                .transpose()?
                .flatten();
            let nonpassing_disposition = coverage_check
                .as_ref()
                .and_then(|row| row.try_get::<String, _>("status").ok())
                .and_then(|status| match status.as_str() {
                    "failed" => Some(ObligationCoverageDisposition::Insufficient),
                    "passed" => None,
                    _ => Some(ObligationCoverageDisposition::Unknown),
                });
            let target_goal_ids = submission.target_goal_ids.iter().collect::<BTreeSet<_>>();
            let mut counterexample_refuted_main_goal = false;
            for goal_id in target_goal_ids {
                let root_obligation_id =
                    crate::proof_obligations::root_obligation_id_tx(&mut tx, &project_id, goal_id)
                        .await?;
                let root_status: String = sqlx::query_scalar(
                    "SELECT status FROM proof_obligations WHERE obligation_id=?",
                )
                .bind(&root_obligation_id)
                .fetch_one(&mut *tx)
                .await?;
                let mut disposition =
                    nonpassing_disposition.unwrap_or(ObligationCoverageDisposition::Supports);
                let mut rationale = match disposition {
                    ObligationCoverageDisposition::Insufficient => "the mathematical candidate was accepted as a Fact, but its Goal coverage review rejected root coverage".to_owned(),
                    ObligationCoverageDisposition::Unknown => "the mathematical candidate was accepted as a Fact, but its Goal coverage review was inconclusive".to_owned(),
                    ObligationCoverageDisposition::Supports | ObligationCoverageDisposition::Satisfies => "accepted Fact supports the root obligation but did not close it".to_owned(),
                };
                if let Some(coverage) = passed_coverage {
                    let check_id: String = coverage.try_get("check_id")?;
                    let evidence_json: String = coverage.try_get("details_json")?;
                    if submission.candidate_type == CandidateType::Counterexample {
                        if root_status == "open" {
                            let goal_priority: f64 = sqlx::query_scalar(
                                "SELECT priority FROM goals WHERE project_id=? AND goal_id=?",
                            )
                            .bind(&project_id)
                            .bind(goal_id)
                            .fetch_one(&mut *tx)
                            .await?;
                            sqlx::query("UPDATE goals SET status='refuted',solved_by_fact_id=? WHERE project_id=? AND goal_id=?")
                                .bind(&fact_id).bind(&project_id).bind(goal_id).execute(&mut *tx).await?;
                            crate::proof_obligations::mark_root_obsolete_tx(
                                &mut tx,
                                &root_obligation_id,
                                obligation_revision,
                                &now,
                            )
                            .await?;
                            updated_obligation_ids.push(root_obligation_id.clone());
                            counterexample_refuted_main_goal |= goal_priority >= 1.0;
                            rationale = "FullyCertified counterexample passed Goal coverage and refuted the Goal; the proof obligation became obsolete rather than satisfied".into();
                        } else if root_status == "obsolete" {
                            let goal_priority: f64 = sqlx::query_scalar(
                                "SELECT priority FROM goals WHERE project_id=? AND goal_id=?",
                            )
                            .bind(&project_id)
                            .bind(goal_id)
                            .fetch_one(&mut *tx)
                            .await?;
                            sqlx::query("UPDATE goals SET status='refuted',solved_by_fact_id=COALESCE(solved_by_fact_id,?) WHERE project_id=? AND goal_id=?")
                                .bind(&fact_id).bind(&project_id).bind(goal_id).execute(&mut *tx).await?;
                            counterexample_refuted_main_goal |= goal_priority >= 1.0;
                            rationale = "another FullyCertified counterexample independently confirms the already-refuted Goal".into();
                        } else {
                            if root_status == "satisfied" {
                                crate::proof_obligations::mark_root_conflicted_tx(
                                    &mut tx,
                                    &root_obligation_id,
                                    obligation_revision,
                                    &now,
                                )
                                .await?;
                                updated_obligation_ids.push(root_obligation_id.clone());
                            }
                            sqlx::query("UPDATE goals SET status='open',solved_by_fact_id=NULL WHERE project_id=? AND goal_id=?")
                                .bind(&project_id).bind(goal_id).execute(&mut *tx).await?;
                            sqlx::query("UPDATE projects SET status='needs_human_review' WHERE project_id=? AND status NOT IN ('stopped_by_human','error')")
                                .bind(&project_id).execute(&mut *tx).await?;
                            sqlx::query("INSERT INTO uncertainties(uncertainty_id,project_id,description,type,severity,affects_goal_ids_json,affects_route_ids_json,introduced_by,resolution_methods_json,status,created_at) SELECT ?,?,?,?,?,?,?,?,?,'open',? WHERE NOT EXISTS (SELECT 1 FROM uncertainties WHERE project_id=? AND type='certified_closure_conflict' AND status IN ('open','investigating') AND EXISTS (SELECT 1 FROM json_each(affects_goal_ids_json) WHERE value=?))")
                                .bind(new_id("unc")).bind(&project_id)
                                .bind("Mutually incompatible FullyCertified proof and counterexample closures require independent adjudication.")
                                .bind("certified_closure_conflict").bind("critical")
                                .bind(json_text(&vec![goal_id.to_owned()])?)
                                .bind(json_text(&vec![submission.route_id.clone()])?)
                                .bind(verification_id)
                                .bind(json_text(&vec!["independent replay of both closure artifacts", "human adjudication of the conflicting certified results"])?)
                                .bind(now.to_rfc3339()).bind(&project_id).bind(goal_id)
                                .execute(&mut *tx).await?;
                            disposition = ObligationCoverageDisposition::Unknown;
                            rationale = "the FullyCertified counterexample conflicts with an existing certified closure; the Goal and root obligation were conservatively reopened for adjudication".into();
                        }
                    } else {
                        let blocked_by_required =
                            crate::proof_obligations::has_unresolved_required_children_tx(
                                &mut tx,
                                &project_id,
                                goal_id,
                                &root_obligation_id,
                            )
                            .await?;
                        if root_status == "obsolete" {
                            crate::proof_obligations::mark_root_conflicted_tx(
                                &mut tx,
                                &root_obligation_id,
                                obligation_revision,
                                &now,
                            )
                            .await?;
                            updated_obligation_ids.push(root_obligation_id.clone());
                            sqlx::query("UPDATE goals SET status='open',solved_by_fact_id=NULL WHERE project_id=? AND goal_id=?")
                                .bind(&project_id).bind(goal_id).execute(&mut *tx).await?;
                            sqlx::query("UPDATE projects SET status='needs_human_review' WHERE project_id=? AND status NOT IN ('stopped_by_human','error')")
                                .bind(&project_id).execute(&mut *tx).await?;
                            sqlx::query("INSERT INTO uncertainties(uncertainty_id,project_id,description,type,severity,affects_goal_ids_json,affects_route_ids_json,introduced_by,resolution_methods_json,status,created_at) SELECT ?,?,?,?,?,?,?,?,?,'open',? WHERE NOT EXISTS (SELECT 1 FROM uncertainties WHERE project_id=? AND type='certified_closure_conflict' AND status IN ('open','investigating') AND EXISTS (SELECT 1 FROM json_each(affects_goal_ids_json) WHERE value=?))")
                                .bind(new_id("unc")).bind(&project_id)
                                .bind("Mutually incompatible FullyCertified counterexample and proof closures require independent adjudication.")
                                .bind("certified_closure_conflict").bind("critical")
                                .bind(json_text(&vec![goal_id.to_owned()])?)
                                .bind(json_text(&vec![submission.route_id.clone()])?)
                                .bind(verification_id)
                                .bind(json_text(&vec!["independent replay of both closure artifacts", "human adjudication of the conflicting certified results"])?)
                                .bind(now.to_rfc3339()).bind(&project_id).bind(goal_id)
                                .execute(&mut *tx).await?;
                            disposition = ObligationCoverageDisposition::Unknown;
                            rationale = "the FullyCertified proof conflicts with an existing certified refutation; the Goal and root obligation were conservatively reopened for adjudication".into();
                        } else if root_status == "blocked" {
                            disposition = ObligationCoverageDisposition::Unknown;
                            rationale = "the root obligation is blocked by a conflicting certified closure and requires adjudication".into();
                        } else if blocked_by_required {
                            rationale = "FullyCertified coverage was present, but an unresolved required child obligation blocked Goal closure".into();
                        } else if root_status == "satisfied" {
                            disposition = ObligationCoverageDisposition::Satisfies;
                            rationale = "another FullyCertified Fact independently satisfies the already-closed root obligation".into();
                        } else {
                            sqlx::query("UPDATE goals SET status='solved',solved_by_fact_id=? WHERE project_id=? AND goal_id=?")
                                .bind(&fact_id).bind(&project_id).bind(goal_id).execute(&mut *tx).await?;
                            crate::proof_obligations::mark_root_satisfied_tx(
                                &mut tx,
                                &root_obligation_id,
                                &fact_id,
                                obligation_revision,
                                &now,
                            )
                            .await?;
                            updated_obligation_ids.push(root_obligation_id.clone());
                            disposition = ObligationCoverageDisposition::Satisfies;
                            rationale = "FullyCertified proof Fact passed Goal coverage with no unresolved required child obligations".into();
                        }
                    }
                    if disposition == ObligationCoverageDisposition::Satisfies
                        || (submission.candidate_type == CandidateType::Counterexample
                            && matches!(root_status.as_str(), "open" | "obsolete"))
                    {
                        let outcome = if submission.candidate_type == CandidateType::Counterexample
                        {
                            "refuted"
                        } else {
                            "solved"
                        };
                        sqlx::query("INSERT OR IGNORE INTO goal_closure_records(closure_id,project_id,goal_id,fact_id,verification_id,check_id,outcome,evidence_json,created_at) VALUES(?,?,?,?,?,?,?,?,?)")
                            .bind(new_id("closure")).bind(&project_id).bind(goal_id).bind(&fact_id)
                            .bind(verification_id).bind(&check_id).bind(outcome).bind(&evidence_json)
                            .bind(now.to_rfc3339()).execute(&mut *tx).await?;
                    }
                }
                crate::proof_obligations::record_root_coverage_tx(
                    &mut tx,
                    crate::proof_obligations::RootCoverage {
                        project_id: &project_id,
                        candidate_id: &candidate_id,
                        verification_id,
                        obligation_id: &root_obligation_id,
                        disposition,
                        fact_id: Some(&fact_id),
                        rationale: &rationale,
                        now: &now,
                    },
                )
                .await?;
                obligation_coverage.push(json!({
                    "goal_id":goal_id,
                    "obligation_id":root_obligation_id,
                    "disposition":disposition,
                }));
            }
            if let Some(coverage_report) = &nonpassing_coverage {
                let bottleneck_id = new_id("bottleneck");
                let coverage_outputs = coverage_report
                    .gaps
                    .iter()
                    .map(|gap| format!("Close {} at {}: {}", gap.gap_type, gap.location, gap.issue))
                    .chain(coverage_report.repair_actions.iter().cloned())
                    .collect::<Vec<_>>();
                let success_outputs = if coverage_outputs.is_empty() {
                    vec![format!(
                        "Resolve Goal coverage review: {}",
                        coverage_report.summary
                    )]
                } else {
                    coverage_outputs
                };
                sqlx::query("INSERT INTO bottlenecks(bottleneck_id,project_id,target_goal_ids_json,kind,precise_statement,completion_contract_json,evidence_ids_json,blocked_route_ids_json,attempted_task_ids_json,repair_action_ids_json,priority,status,created_revision,updated_revision,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,1.15,'open',(SELECT revision FROM projects WHERE project_id=?),(SELECT revision FROM projects WHERE project_id=?),?,?)")
                    .bind(&bottleneck_id).bind(&project_id).bind(json_text(&submission.target_goal_ids)?)
                    .bind("goal_coverage_debt").bind(format!("Resolve the non-passing Goal coverage review without changing the accepted intermediate Fact: {}", coverage_report.summary))
                    .bind(json_text(&json!({"success_outputs":success_outputs,"partial_outputs":["a verified implication from the accepted Fact toward the exact target"],"reject_outputs":["claiming that Fact acceptance alone closes the Goal"]}))?)
                    .bind(json_text(&vec![verification_id.to_owned()])?).bind(json_text(&vec![submission.route_id.clone()])?)
                    .bind(json_text(&vec![submission.task_id.clone()])?).bind(json_text(&coverage_report.repair_actions)?)
                    .bind(&project_id).bind(&project_id).bind(now.to_rfc3339()).bind(now.to_rfc3339()).execute(&mut *tx).await?;
                created_obligation_ids.extend(
                    crate::proof_obligations::insert_verifier_advisory_obligations_tx(
                        &mut tx,
                        crate::proof_obligations::AdvisoryObligationContext {
                            project_id: &project_id,
                            candidate_id: &candidate_id,
                            verification_id,
                            target_goal_ids: &submission.target_goal_ids,
                            bottleneck_id: Some(&bottleneck_id),
                            origin_kind: "goal_coverage_review",
                            report: coverage_report,
                            project_revision: obligation_revision,
                            now: &now,
                        },
                    )
                    .await?,
                );
            }
            if counterexample_refuted_main_goal {
                sqlx::query("UPDATE projects SET status='refuted' WHERE project_id=?")
                    .bind(&project_id)
                    .execute(&mut *tx)
                    .await?;
            }
            sqlx::query("UPDATE candidates SET status='promoted_to_fact' WHERE candidate_id=?")
                .bind(&candidate_id)
                .execute(&mut *tx)
                .await?;
            let fact_row = sqlx::query("SELECT * FROM facts WHERE fact_id=?")
                .bind(&fact_id)
                .fetch_one(&mut *tx)
                .await?;
            fact = Some(rows::fact(&fact_row)?);
            if let Some((_, challenged_fact_id)) = &governance_challenge {
                if challenged_fact_id != &fact_id {
                    return Err(StorageError::CorruptData(format!(
                        "reverification for {challenged_fact_id} produced a different fact {fact_id}"
                    )));
                }
                sqlx::query("UPDATE fact_assurances SET status='superseded',invalidated_at=? WHERE fact_id=? AND status='active'")
                    .bind(now.to_rfc3339()).bind(&fact_id).execute(&mut *tx).await?;
            }
            sqlx::query("INSERT INTO fact_assurances(assurance_id,fact_id,case_id,acceptance_class,snapshot_hash,package_id,replay_id,status,created_at) VALUES(?,?,?,?,?,?,?,?,?)")
                .bind(new_id("assurance")).bind(&fact_id).bind(&gate.case_id)
                .bind(gate.acceptance.to_string()).bind(&gate.snapshot_hash).bind(&gate.package_id)
                .bind(&gate.replay_id).bind("active").bind(now.to_rfc3339()).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO global_fact_catalog(content_hash,source_project_id,source_fact_id,statement,status,updated_at) VALUES(?,?,?,?,?,?) ON CONFLICT(content_hash) DO UPDATE SET status=excluded.status,updated_at=excluded.updated_at")
                .bind(&global_content_hash).bind(&project_id).bind(&fact_id).bind(&submission.statement)
                .bind("active").bind(now.to_rfc3339()).execute(&mut *tx).await?;
            for source_id in &submission.external_source_ids {
                let updated = sqlx::query("UPDATE sources SET status='admitted' WHERE source_id=? AND project_id=? AND status='reported_unverified'")
                    .bind(source_id)
                    .bind(&project_id)
                    .execute(&mut *tx)
                    .await?;
                if updated.rows_affected() == 1 {
                    admitted_source_ids.push(source_id.clone());
                }
            }
        } else if report.verdict == VerificationVerdict::Rejected {
            sqlx::query("INSERT INTO failures(failure_id,project_id,route_id,task_id,failure_type,summary,repairable,raw_json,created_at) VALUES(?,?,?,?,?,?,?,?,?)")
                .bind(new_id("failure")).bind(&project_id).bind(&submission.route_id).bind(&submission.task_id).bind("verification_rejected").bind(&report.summary).bind(!report.repair_actions.is_empty()).bind(json_text(&report)?).bind(now.to_rfc3339()).execute(&mut *tx).await?;
            let precise_statement = if report.repair_actions.is_empty() {
                format!(
                    "Resolve the rejected candidate's concrete proof debt: {}",
                    report.summary
                )
            } else {
                format!(
                    "Apply the verifier repair actions: {}",
                    report.repair_actions.join("; ")
                )
            };
            let bottleneck_id = new_id("bottleneck");
            sqlx::query("INSERT INTO bottlenecks(bottleneck_id,project_id,target_goal_ids_json,kind,precise_statement,completion_contract_json,evidence_ids_json,blocked_route_ids_json,attempted_task_ids_json,repair_action_ids_json,priority,status,created_revision,updated_revision,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,1.1,'open',(SELECT revision FROM projects WHERE project_id=?),(SELECT revision FROM projects WHERE project_id=?),?,?)")
                .bind(&bottleneck_id).bind(&project_id).bind(json_text(&submission.target_goal_ids)?)
                .bind("verification_proof_debt").bind(precise_statement)
                .bind(json_text(&json!({"success_outputs":report.repair_actions,"partial_outputs":["a strictly narrower named lemma that removes part of the reported gap"],"reject_outputs":["the same rejected proof without new evidence"]}))?)
                .bind(json_text(&vec![verification_id.to_owned()])?).bind(json_text(&vec![submission.route_id.clone()])?)
                .bind(json_text(&vec![submission.task_id.clone()])?).bind(json_text(&report.repair_actions)?)
                .bind(&project_id).bind(&project_id).bind(now.to_rfc3339()).bind(now.to_rfc3339()).execute(&mut *tx).await?;
            advisory_bottleneck_id = Some(bottleneck_id);
        } else {
            let uncertainties = if report.uncertainties.is_empty() {
                vec![report.summary.clone()]
            } else {
                report.uncertainties.clone()
            };
            for description in uncertainties {
                sqlx::query("INSERT INTO uncertainties(uncertainty_id,project_id,description,type,severity,affects_goal_ids_json,affects_route_ids_json,introduced_by,resolution_methods_json,status,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
                    .bind(new_id("unc")).bind(&project_id).bind(description).bind("verification_unknown").bind("high").bind(json_text(&submission.target_goal_ids)?).bind(json_text(&vec![submission.route_id.clone()])?).bind(verification_id).bind(json_text(&report.repair_actions)?).bind(UncertaintyStatus::Open.to_string()).bind(now.to_rfc3339()).execute(&mut *tx).await?;
            }
            if !report.gaps.is_empty() || !report.repair_actions.is_empty() {
                let bottleneck_id = new_id("bottleneck");
                let gap_outputs = report
                    .gaps
                    .iter()
                    .map(|gap| format!("Close {} at {}: {}", gap.gap_type, gap.location, gap.issue))
                    .chain(report.repair_actions.iter().cloned())
                    .collect::<Vec<_>>();
                sqlx::query("INSERT INTO bottlenecks(bottleneck_id,project_id,target_goal_ids_json,kind,precise_statement,completion_contract_json,evidence_ids_json,blocked_route_ids_json,attempted_task_ids_json,repair_action_ids_json,priority,status,created_revision,updated_revision,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,1.05,'open',(SELECT revision FROM projects WHERE project_id=?),(SELECT revision FROM projects WHERE project_id=?),?,?)")
                    .bind(&bottleneck_id).bind(&project_id).bind(json_text(&submission.target_goal_ids)?)
                    .bind("verification_uncertainty_debt").bind(format!("Resolve the verifier's structured unknowns: {}", gap_outputs.join("; ")))
                    .bind(json_text(&json!({"success_outputs":gap_outputs,"partial_outputs":["new evidence narrowing a named verifier uncertainty"],"reject_outputs":["repeating the inconclusive candidate without new evidence"]}))?)
                    .bind(json_text(&vec![verification_id.to_owned()])?).bind(json_text(&vec![submission.route_id.clone()])?)
                    .bind(json_text(&vec![submission.task_id.clone()])?).bind(json_text(&report.repair_actions)?)
                    .bind(&project_id).bind(&project_id).bind(now.to_rfc3339()).bind(now.to_rfc3339()).execute(&mut *tx).await?;
                advisory_bottleneck_id = Some(bottleneck_id);
            }
        }
        if matches!(
            report.verdict,
            VerificationVerdict::Rejected | VerificationVerdict::Unknown
        ) {
            created_obligation_ids.extend(
                crate::proof_obligations::insert_verifier_advisory_obligations_tx(
                    &mut tx,
                    crate::proof_obligations::AdvisoryObligationContext {
                        project_id: &project_id,
                        candidate_id: &candidate_id,
                        verification_id,
                        target_goal_ids: &submission.target_goal_ids,
                        bottleneck_id: advisory_bottleneck_id.as_deref(),
                        origin_kind: "verification_report",
                        report: &report,
                        project_revision: obligation_revision,
                        now: &now,
                    },
                )
                .await?,
            );
            let disposition = match report.verdict {
                VerificationVerdict::Rejected => ObligationCoverageDisposition::Insufficient,
                VerificationVerdict::Unknown => ObligationCoverageDisposition::Unknown,
                VerificationVerdict::Accepted => unreachable!("accepted handled above"),
            };
            let rationale = match report.verdict {
                VerificationVerdict::Rejected => {
                    "verification rejected the candidate as insufficient for the root obligation"
                }
                VerificationVerdict::Unknown => {
                    "verification could not determine whether the candidate covers the root obligation"
                }
                VerificationVerdict::Accepted => unreachable!("accepted handled above"),
            };
            for goal_id in submission.target_goal_ids.iter().collect::<BTreeSet<_>>() {
                let root_obligation_id =
                    crate::proof_obligations::root_obligation_id_tx(&mut tx, &project_id, goal_id)
                        .await?;
                crate::proof_obligations::record_root_coverage_tx(
                    &mut tx,
                    crate::proof_obligations::RootCoverage {
                        project_id: &project_id,
                        candidate_id: &candidate_id,
                        verification_id,
                        obligation_id: &root_obligation_id,
                        disposition,
                        fact_id: None,
                        rationale,
                        now: &now,
                    },
                )
                .await?;
                obligation_coverage.push(json!({
                    "goal_id":goal_id,
                    "obligation_id":root_obligation_id,
                    "disposition":disposition,
                }));
            }
        }
        let mut governance_event: Option<(&str, String, Value)> = None;
        let mut external_governance_events = Vec::new();
        if let Some((challenge_id, challenged_fact_id)) = &governance_challenge {
            match report.verdict {
                VerificationVerdict::Accepted => {
                    sqlx::query("UPDATE facts SET status='active' WHERE fact_id=?")
                        .bind(challenged_fact_id)
                        .execute(&mut *tx)
                        .await?;
                    sqlx::query("UPDATE fact_challenges SET status='resolved',resolved_at=? WHERE challenge_id=?")
                        .bind(now.to_rfc3339()).bind(challenge_id).execute(&mut *tx).await?;
                    let reconciled_project_status =
                        crate::fact_governance::reconcile_project_after_governance_acceptance_tx(
                            &mut tx,
                            &project_id,
                        )
                        .await?;
                    governance_event = Some((
                        "fact.reverified",
                        challenged_fact_id.clone(),
                        json!({"challenge_id":challenge_id,"verification_id":verification_id,"outcome":"accepted","project_status":reconciled_project_status}),
                    ));
                }
                VerificationVerdict::Rejected => {
                    let (impact, invalidation_events) =
                        crate::fact_governance::cascade_fact_invalidation(
                            &mut tx,
                            challenged_fact_id,
                            FactStatus::Revoked,
                        )
                        .await?;
                    external_governance_events = invalidation_events;
                    sqlx::query("UPDATE fact_challenges SET status='rejected',resolved_at=? WHERE challenge_id=?")
                        .bind(now.to_rfc3339()).bind(challenge_id).execute(&mut *tx).await?;
                    governance_event = Some((
                        "fact.revoked",
                        challenged_fact_id.clone(),
                        json!({"challenge_id":challenge_id,"verification_id":verification_id,"outcome":"rejected","impact":impact}),
                    ));
                }
                VerificationVerdict::Unknown => {
                    let (impact, invalidation_events) =
                        crate::fact_governance::cascade_fact_invalidation(
                            &mut tx,
                            challenged_fact_id,
                            FactStatus::Suspended,
                        )
                        .await?;
                    external_governance_events = invalidation_events;
                    sqlx::query("UPDATE fact_challenges SET status='inconclusive',resolved_at=? WHERE challenge_id=?")
                        .bind(now.to_rfc3339()).bind(challenge_id).execute(&mut *tx).await?;
                    governance_event = Some((
                        "fact.suspended",
                        challenged_fact_id.clone(),
                        json!({"challenge_id":challenge_id,"verification_id":verification_id,"outcome":"unknown","impact":impact}),
                    ));
                }
            }
            if fact.is_some() {
                let updated = sqlx::query("SELECT * FROM facts WHERE fact_id=?")
                    .bind(challenged_fact_id)
                    .fetch_one(&mut *tx)
                    .await?;
                fact = Some(rows::fact(&updated)?);
            }
        }
        let terminal_stage = match report.verdict {
            VerificationVerdict::Accepted => "committed",
            VerificationVerdict::Rejected => "rejected",
            VerificationVerdict::Unknown => "unknown",
        };
        sqlx::query("UPDATE verification_cases SET stage=?,updated_at=?,completed_at=? WHERE verification_id=?")
            .bind(terminal_stage).bind(now.to_rfc3339()).bind(now.to_rfc3339())
            .bind(verification_id).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &project_id).await?;
        if let Some(accepted_fact) = &fact {
            let closed_goal_ids = sqlx::query_scalar::<_, String>(
                "SELECT goal_id FROM goals WHERE project_id=? AND solved_by_fact_id=? AND status IN ('solved','refuted')",
            )
            .bind(&project_id)
            .bind(&accepted_fact.fact_id)
            .fetch_all(&mut *tx)
            .await?;
            if !closed_goal_ids.is_empty() {
                sqlx::query(
                    "UPDATE uncertainties SET status='obsolete',resolved_by=? WHERE project_id=? AND status IN ('open','investigating') AND json_array_length(affects_goal_ids_json)>0 AND NOT EXISTS (SELECT 1 FROM json_each(uncertainties.affects_goal_ids_json) j LEFT JOIN goals g ON g.project_id=uncertainties.project_id AND g.goal_id=j.value WHERE g.goal_id IS NULL OR g.status NOT IN ('solved','refuted'))",
                )
                .bind(&accepted_fact.fact_id)
                .bind(&project_id)
                .execute(&mut *tx)
                .await?;
            }
            let resolved_uncertainty_ids = sqlx::query_scalar::<_, String>(
                "SELECT uncertainty_id FROM uncertainties WHERE project_id=? AND status IN ('resolved','obsolete') AND resolved_by=?",
            )
            .bind(&project_id)
            .bind(&accepted_fact.fact_id)
            .fetch_all(&mut *tx)
            .await?;
            let unblocked_route_ids = sqlx::query_scalar::<_, String>(
                "SELECT r.route_id FROM routes r WHERE r.project_id=? AND r.status='blocked' AND EXISTS (SELECT 1 FROM json_each(r.required_fact_ids_json) j WHERE j.value=?) AND NOT EXISTS (SELECT 1 FROM json_each(r.required_fact_ids_json) j LEFT JOIN facts f ON f.fact_id=j.value WHERE f.fact_id IS NULL OR f.status!='active')",
            )
            .bind(&project_id)
            .bind(&accepted_fact.fact_id)
            .fetch_all(&mut *tx)
            .await?;
            let invalidated_task_ids = if closed_goal_ids.is_empty() {
                Vec::new()
            } else {
                sqlx::query_scalar::<_, String>("SELECT DISTINCT t.task_id FROM tasks t,json_each(t.goal_ids_json) j WHERE t.project_id=? AND t.status IN ('open','assigned','queued','offered','leased','running','checkpointed') AND j.value IN (SELECT value FROM json_each(?))")
                    .bind(&project_id).bind(json_text(&closed_goal_ids)?).fetch_all(&mut *tx).await?
            };
            let dominated_route_ids = sqlx::query_scalar::<_, String>("SELECT r.route_id FROM routes r WHERE r.project_id=? AND r.status IN ('incubating','active','blocked','probation','revived') AND NOT EXISTS (SELECT 1 FROM json_each(r.target_goal_ids_json) j LEFT JOIN goals g ON g.goal_id=j.value WHERE g.goal_id IS NULL OR g.status NOT IN ('solved','refuted'))")
                .bind(&project_id).fetch_all(&mut *tx).await?;
            for task_id in &invalidated_task_ids {
                sqlx::query("UPDATE tasks SET status='cancelled',revision=revision+1,result_summary='cancelled because a newly accepted fact closed the target goal' WHERE task_id=?")
                    .bind(task_id).execute(&mut *tx).await?;
            }
            for route_id in &unblocked_route_ids {
                sqlx::query(
                    "UPDATE routes SET status='probation' WHERE route_id=? AND status='blocked'",
                )
                .bind(route_id)
                .execute(&mut *tx)
                .await?;
            }
            let newly_enabled_task_templates = unblocked_route_ids
                .iter()
                .map(|route_id| format!("route:{route_id}:next_bottleneck_task"))
                .collect::<Vec<_>>();
            sqlx::query("UPDATE bottlenecks SET status='resolved',updated_revision=?,updated_at=? WHERE project_id=? AND status='open' AND NOT EXISTS (SELECT 1 FROM json_each(bottlenecks.target_goal_ids_json) j JOIN goals g ON g.goal_id=j.value WHERE g.status NOT IN ('solved','refuted'))")
                .bind(revision).bind(now.to_rfc3339()).bind(&project_id).execute(&mut *tx).await?;
            updated_obligation_ids.extend(
                crate::proof_obligations::obsolete_resolved_bottleneck_advisories_tx(
                    &mut tx,
                    &project_id,
                    revision,
                    &now,
                )
                .await?,
            );
            sqlx::query("INSERT INTO fact_impact_records(impact_id,project_id,fact_id,source_revision,closed_goal_ids_json,unblocked_route_ids_json,invalidated_task_ids_json,newly_enabled_task_templates_json,dominated_route_ids_json,resolved_uncertainty_ids_json,planner_disposition,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,'pending',?)")
                .bind(new_id("factimpact")).bind(&project_id).bind(&accepted_fact.fact_id).bind(revision)
                .bind(json_text(&closed_goal_ids)?).bind(json_text(&unblocked_route_ids)?).bind(json_text(&invalidated_task_ids)?)
                .bind(json_text(&newly_enabled_task_templates)?).bind(json_text(&dominated_route_ids)?)
                .bind(json_text(&resolved_uncertainty_ids)?).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        }
        let case_id: String =
            sqlx::query_scalar("SELECT case_id FROM verification_cases WHERE verification_id=?")
                .bind(verification_id)
                .fetch_one(&mut *tx)
                .await?;
        created_obligation_ids.sort();
        created_obligation_ids.dedup();
        updated_obligation_ids.sort();
        updated_obligation_ids.dedup();
        let mut events = external_governance_events;
        for source_id in admitted_source_ids {
            events.push(
                append_event(
                    &mut tx,
                    &project_id,
                    revision,
                    "source.admitted",
                    entity("source", &source_id),
                    json!({"verification_id": verification_id, "candidate_id": candidate_id}),
                    Some(entity("verification", verification_id)),
                )
                .await?,
            );
        }
        events.push(append_event(&mut tx, &project_id, revision, "verification.finished", entity("verification", verification_id), json!({"verdict": report.verdict, "candidate_id": candidate_id, "gap_count": report.gaps.len(), "case_id": case_id,"created_obligation_ids":created_obligation_ids.clone(),"updated_obligation_ids":updated_obligation_ids.clone(),"obligation_coverage":obligation_coverage.clone()}), None).await?);
        events.push(
            append_event(
                &mut tx,
                &project_id,
                revision,
                "verification.case.completed",
                entity("verification_case", &case_id),
                json!({"stage": terminal_stage, "verdict": report.verdict}),
                Some(entity("verification", verification_id)),
            )
            .await?,
        );
        for obligation_id in &created_obligation_ids {
            events.push(
                append_event(
                    &mut tx,
                    &project_id,
                    revision,
                    "proof_obligation.created",
                    entity("proof_obligation", obligation_id),
                    json!({"verification_id":verification_id,"candidate_id":candidate_id}),
                    Some(entity("verification", verification_id)),
                )
                .await?,
            );
        }
        for obligation_id in &updated_obligation_ids {
            let obligation_status: String = sqlx::query_scalar(
                "SELECT status FROM proof_obligations WHERE obligation_id=? AND project_id=?",
            )
            .bind(obligation_id)
            .bind(&project_id)
            .fetch_one(&mut *tx)
            .await?;
            events.push(
                append_event(
                    &mut tx,
                    &project_id,
                    revision,
                    "proof_obligation.updated",
                    entity("proof_obligation", obligation_id),
                    json!({"status":obligation_status,"verification_id":verification_id,"candidate_id":candidate_id}),
                    Some(entity("verification", verification_id)),
                )
                .await?,
            );
        }
        for coverage in &obligation_coverage {
            let obligation_id = coverage
                .get("obligation_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    StorageError::CorruptData(
                        "obligation coverage event has no obligation_id".into(),
                    )
                })?;
            events.push(
                append_event(
                    &mut tx,
                    &project_id,
                    revision,
                    "candidate_obligation.coverage_recorded",
                    entity("proof_obligation", obligation_id),
                    coverage.clone(),
                    Some(entity("verification", verification_id)),
                )
                .await?,
            );
        }
        if let Some(accepted_fact) = &fact {
            events.push(
                append_event(
                    &mut tx,
                    &project_id,
                    revision,
                    "fact.accepted",
                    entity("fact", &accepted_fact.fact_id),
                    json!({"candidate_id": candidate_id}),
                    Some(entity("verification", verification_id)),
                )
                .await?,
            );
        }
        if let Some((event_type, governed_fact_id, payload)) = governance_event {
            events.push(
                append_event(
                    &mut tx,
                    &project_id,
                    revision,
                    event_type,
                    entity("fact", &governed_fact_id),
                    payload,
                    Some(entity("verification", verification_id)),
                )
                .await?,
            );
        }
        tx.commit().await?;
        Ok(VerificationCommit {
            verification: Verification {
                verification_id: verification_id.into(),
                candidate_id,
                project_id,
                status,
                report: Some(report),
                started_at: None,
                completed_at: Some(now),
            },
            fact,
            events,
        })
    }

    pub async fn complete_round(
        &self,
        project_id: &str,
        round_id: &str,
        summary: &str,
    ) -> StorageResult<Vec<DomainEvent>> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "complete_round",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let now = Utc::now();
        let project_row =
            sqlx::query("SELECT status,budget_json,current_round FROM projects WHERE project_id=?")
                .bind(project_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StorageError::NotFound {
                    kind: "project",
                    id: project_id.into(),
                })?;
        let project_status: String = project_row.try_get("status")?;
        if !matches!(
            project_status.as_str(),
            "running" | "refuted" | "needs_human_review"
        ) {
            tx.rollback().await?;
            return Err(StorageError::InvalidTransition(format!(
                "cannot complete a round while project is {project_status}"
            )));
        }
        let completed = sqlx::query("UPDATE rounds SET status='completed',completed_at=?,summary=? WHERE project_id=? AND round_id=? AND status IN ('running','verifying','publishing')")
            .bind(now.to_rfc3339()).bind(summary).bind(project_id).bind(round_id).execute(&mut *tx).await?;
        if completed.rows_affected() != 1 {
            tx.rollback().await?;
            return Err(StorageError::InvalidTransition(format!(
                "round {round_id} is missing or is not completable"
            )));
        }
        let main_goal_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM goals WHERE project_id=? AND priority>=1.0")
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?;
        let unresolved_main_goals: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM goals WHERE project_id=? AND priority>=1.0 AND status!='solved'",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        let blocking: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM uncertainties WHERE project_id=? AND status IN ('open','investigating') AND severity IN ('critical','high')").bind(project_id).fetch_one(&mut *tx).await?;
        let unresolved_required_obligations: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM proof_obligations o JOIN goals g ON g.goal_id=o.goal_id AND g.project_id=o.project_id WHERE o.project_id=? AND g.priority>=1.0 AND o.necessity='required' AND o.status<>'obsolete' AND ((o.source_kind='root_goal' AND o.status!='satisfied') OR (o.source_kind!='root_goal' AND o.status!='satisfied'))",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        let project_succeeded = project_status == ProjectStatus::Running.to_string()
            && main_goal_count > 0
            && unresolved_main_goals == 0
            && blocking == 0
            && unresolved_required_obligations == 0;
        let budget: Budget = serde_json::from_str(project_row.try_get("budget_json")?)?;
        let current_round: i64 = project_row.try_get("current_round")?;
        let total_calls: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(model_calls),0) FROM usage_records WHERE project_id=?",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        let successful_calls: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM usage_records WHERE project_id=? AND outcome='succeeded'",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        let round_budget_exhausted = current_round >= i64::from(budget.max_rounds);
        let call_budget_exhausted = total_calls >= i64::from(budget.max_total_model_calls);
        let budget_exhausted = project_status == ProjectStatus::Running.to_string()
            && !project_succeeded
            && (round_budget_exhausted || call_budget_exhausted);
        let environment_failed = budget_exhausted && total_calls > 0 && successful_calls == 0;
        if project_succeeded {
            sqlx::query(
                "UPDATE projects SET status='success',updated_at=? WHERE project_id=? AND status='running'",
            )
            .bind(now.to_rfc3339())
            .bind(project_id)
            .execute(&mut *tx)
            .await?;
        } else if budget_exhausted {
            sqlx::query(
                "UPDATE projects SET status=?,updated_at=? WHERE project_id=? AND status='running'",
            )
            .bind(if environment_failed {
                ProjectStatus::EnvironmentFailed.to_string()
            } else {
                ProjectStatus::PartialSuccess.to_string()
            })
            .bind(now.to_rfc3339())
            .bind(project_id)
            .execute(&mut *tx)
            .await?;
        }
        let revision = bump_revision(&mut tx, project_id).await?;
        let mut events = vec![
            append_event(
                &mut tx,
                project_id,
                revision,
                "round.completed",
                entity("round", round_id),
                json!({"summary": summary}),
                None,
            )
            .await?,
        ];
        if project_succeeded {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "project.succeeded",
                    entity("project", project_id),
                    json!({}),
                    Some(entity("round", round_id)),
                )
                .await?,
            );
        } else if budget_exhausted {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    if environment_failed {
                        "project.environment_failed"
                    } else {
                        "project.budget_exhausted"
                    },
                    entity("project", project_id),
                    json!({
                        "model_calls": total_calls,
                        "successful_model_calls": successful_calls,
                        "round_limit_reached": round_budget_exhausted,
                        "model_call_limit_reached": call_budget_exhausted,
                    }),
                    Some(entity("round", round_id)),
                )
                .await?,
            );
        }
        tx.commit().await?;
        Ok(events)
    }

    /// Atomically reserves one model call against project and optional task limits.
    pub async fn reserve_model_call(
        &self,
        request: ModelCallRequest<'_>,
    ) -> StorageResult<UsageReservation> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::Telemetry,
                "reserve_model_call",
            )
            .await?;
        let ModelCallRequest {
            project_id,
            round,
            worker_id,
            task_id,
            model,
            purpose,
            max_total_calls,
            max_task_calls,
        } = request;
        let mut tx = self.pool().begin().await?;
        let project_row = sqlx::query("SELECT status,budget_json FROM projects WHERE project_id=?")
            .bind(project_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "project",
                id: project_id.into(),
            })?;
        let project_status: String = project_row.try_get("status")?;
        let lifecycle_allows_call = match purpose {
            ModelCallPurpose::Research => project_status == "running",
            ModelCallPurpose::RouteScopedResearch(route_id) => {
                if matches!(project_status.as_str(), "running" | "needs_human_review") {
                    let released: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM routes WHERE project_id=? AND route_id=? \
                         AND human_review IN ('approved','not_required') \
                         AND status IN ('incubating','active','probation','revived')",
                    )
                    .bind(project_id)
                    .bind(route_id)
                    .fetch_one(&mut *tx)
                    .await?;
                    released == 1
                } else {
                    false
                }
            }
            ModelCallPurpose::Verification(verification_id) => {
                crate::verification_execution_is_released(&mut tx, verification_id).await?
            }
            ModelCallPurpose::Publication(publication_id) => {
                let active_publication: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM publication_runs WHERE publication_id=? AND project_id=? AND status='running'",
                )
                .bind(publication_id)
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?;
                active_publication == 1
                    && !matches!(project_status.as_str(), "created" | "stopped_by_human")
            }
        };
        if !lifecycle_allows_call {
            return Err(StorageError::InvalidTransition(format!(
                "model calls cannot start while project is {project_status}"
            )));
        }
        let budget_json: String = project_row.try_get("budget_json")?;
        let project_budget: Budget = serde_json::from_str(&budget_json)?;
        let effective_total_calls =
            i64::from(max_total_calls.min(project_budget.max_total_model_calls));
        let task_override: Option<i64> = if let Some(task_id) = task_id {
            sqlx::query_scalar("SELECT CAST(json_extract(limits_json,'$.max_model_calls') AS INTEGER) FROM budget_overrides WHERE project_id=? AND scope_kind='task' AND scope_id=?")
                .bind(project_id).bind(task_id).fetch_optional(&mut *tx).await?
        } else {
            None
        };
        let configured_task_calls =
            task_override.unwrap_or_else(|| i64::from(project_budget.max_model_calls_per_task));
        let effective_task_calls = configured_task_calls.min(i64::from(max_task_calls));
        let round_override: Option<i64> = sqlx::query_scalar("SELECT CAST(json_extract(limits_json,'$.max_model_calls') AS INTEGER) FROM budget_overrides WHERE project_id=? AND scope_kind='round' AND scope_id=?")
            .bind(project_id).bind(round.to_string()).fetch_optional(&mut *tx).await?;
        let usage_id = new_id("usage");
        let result = sqlx::query(
            "INSERT INTO usage_records(usage_id,project_id,round,worker_id,task_id,model,model_calls,input_tokens,output_tokens,elapsed_ms,created_at,outcome) \
             SELECT ?,?,?,?,?,?,1,0,0,0,?,'running' \
             WHERE (SELECT COALESCE(SUM(model_calls),0) FROM usage_records WHERE project_id=?) < ? \
             AND (? IS NULL OR (SELECT COALESCE(SUM(model_calls),0) FROM usage_records WHERE project_id=? AND task_id=?) < ?) \
             AND (? IS NULL OR (SELECT COALESCE(SUM(model_calls),0) FROM usage_records WHERE project_id=? AND round=?) < ?)",
        )
        .bind(&usage_id)
        .bind(project_id)
        .bind(round)
        .bind(worker_id)
        .bind(task_id)
        .bind(model)
        .bind(Utc::now().to_rfc3339())
        .bind(project_id)
        .bind(effective_total_calls)
        .bind(task_id)
        .bind(project_id)
        .bind(task_id)
        .bind(effective_task_calls)
        .bind(round_override)
        .bind(project_id)
        .bind(round)
        .bind(round_override)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            tx.rollback().await?;
            return Err(StorageError::BudgetExhausted(format!(
                "effective project limit {effective_total_calls} (stored {}, caller {max_total_calls}), effective task limit {effective_task_calls} (stored {configured_task_calls}, caller {max_task_calls}), or round override {round_override:?} reached",
                project_budget.max_total_model_calls,
            )));
        }
        tx.commit().await?;
        Ok(UsageReservation { usage_id })
    }

    pub async fn finish_model_call(&self, usage_id: &str, elapsed_ms: i64) -> StorageResult<()> {
        self.finish_model_call_with_tokens(usage_id, elapsed_ms, 0, 0)
            .await
    }

    pub async fn finish_model_call_with_tokens(
        &self,
        usage_id: &str,
        elapsed_ms: i64,
        input_tokens: i64,
        output_tokens: i64,
    ) -> StorageResult<()> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::Telemetry,
                "finish_model_call",
            )
            .await?;
        let updated = sqlx::query(
            "UPDATE usage_records SET elapsed_ms=?,input_tokens=?,output_tokens=?,outcome='succeeded',error_kind=NULL,error_message=NULL WHERE usage_id=? AND outcome='running'",
        )
        .bind(elapsed_ms.max(0))
        .bind(input_tokens.max(0))
        .bind(output_tokens.max(0))
        .bind(usage_id)
        .execute(self.pool())
        .await?;
        self.accept_model_call_terminal_transition(usage_id, updated.rows_affected())
            .await
    }

    pub async fn fail_model_call(
        &self,
        usage_id: &str,
        elapsed_ms: i64,
        error_kind: &str,
        error_message: &str,
    ) -> StorageResult<()> {
        self.fail_model_call_with_tokens(usage_id, elapsed_ms, error_kind, error_message, 0, 0)
            .await
    }

    pub async fn fail_model_call_with_tokens(
        &self,
        usage_id: &str,
        elapsed_ms: i64,
        error_kind: &str,
        error_message: &str,
        input_tokens: i64,
        output_tokens: i64,
    ) -> StorageResult<()> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::Telemetry,
                "fail_model_call",
            )
            .await?;
        let updated = sqlx::query(
            "UPDATE usage_records SET elapsed_ms=?,input_tokens=?,output_tokens=?,outcome='failed',error_kind=?,error_message=? WHERE usage_id=? AND outcome='running'",
        )
        .bind(elapsed_ms.max(0))
        .bind(input_tokens.max(0))
        .bind(output_tokens.max(0))
        .bind(error_kind)
        .bind(error_message)
        .bind(usage_id)
        .execute(self.pool())
        .await?;
        self.accept_model_call_terminal_transition(usage_id, updated.rows_affected())
            .await
    }

    async fn accept_model_call_terminal_transition(
        &self,
        usage_id: &str,
        rows_affected: u64,
    ) -> StorageResult<()> {
        if rows_affected == 0
            && sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM usage_records WHERE usage_id=?")
                .bind(usage_id)
                .fetch_one(self.pool())
                .await?
                == 0
        {
            return Err(StorageError::NotFound {
                kind: "usage",
                id: usage_id.into(),
            });
        }
        Ok(())
    }

    pub async fn total_model_calls(&self, project_id: &str) -> StorageResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT COALESCE(SUM(model_calls),0) FROM usage_records WHERE project_id=?",
        )
        .bind(project_id)
        .fetch_one(self.pool())
        .await?)
    }

    pub async fn successful_model_calls(&self, project_id: &str) -> StorageResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT COUNT(*) FROM usage_records WHERE project_id=? AND outcome='succeeded'",
        )
        .bind(project_id)
        .fetch_one(self.pool())
        .await?)
    }

    pub async fn mark_budget_exhausted(&self, project_id: &str) -> StorageResult<DomainEvent> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "mark_budget_exhausted",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let status: String = sqlx::query_scalar("SELECT status FROM projects WHERE project_id=?")
            .bind(project_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "project",
                id: project_id.into(),
            })?;
        if status != "running" {
            return Err(StorageError::InvalidTransition(format!(
                "cannot exhaust budget from {status}"
            )));
        }
        let total_calls: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM usage_records WHERE project_id=?")
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?;
        let successful_calls: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM usage_records WHERE project_id=? AND outcome='succeeded'",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        let environment_failed = total_calls > 0 && successful_calls == 0;
        let terminal_status = if environment_failed {
            "environment_failed"
        } else {
            "partial_success"
        };
        sqlx::query("UPDATE projects SET status=? WHERE project_id=?")
            .bind(terminal_status)
            .bind(project_id)
            .execute(&mut *tx)
            .await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            if environment_failed {
                "project.environment_failed"
            } else {
                "project.budget_exhausted"
            },
            entity("project", project_id),
            json!({"model_calls":total_calls,"successful_model_calls":successful_calls}),
            None,
        )
        .await?;
        tx.commit().await?;
        Ok(event)
    }

    pub async fn store_artifact(
        &self,
        project_id: &str,
        kind: &str,
        filename: &str,
        content: &[u8],
        round: i64,
        related: Vec<String>,
    ) -> StorageResult<(Artifact, DomainEvent)> {
        let artifact = self
            .prepare_artifact_file(project_id, kind, filename, content, round, related)
            .await?;
        let mut artifact_file = UncommittedArtifactFile::new(&artifact);
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "store_artifact_reference",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        insert_artifact_record_tx(&mut tx, &artifact).await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            "artifact.published",
            entity("artifact", &artifact.artifact_id),
            json!({"kind": kind, "filename": filename}),
            None,
        )
        .await?;
        tx.commit().await?;
        artifact_file.commit();
        Ok((artifact, event))
    }

    pub(crate) async fn prepare_artifact_file(
        &self,
        project_id: &str,
        kind: &str,
        filename: &str,
        content: &[u8],
        round: i64,
        related: Vec<String>,
    ) -> StorageResult<Artifact> {
        if filename.contains(['/', '\\']) || filename == "." || filename == ".." {
            return Err(StorageError::InvalidTransition(
                "unsafe artifact filename".into(),
            ));
        }
        let directory = self.artifact_root().join(project_id);
        tokio::fs::create_dir_all(&directory).await?;
        let artifact_id = new_id("artifact");
        let stored_name = format!("{artifact_id}-{filename}");
        let path = directory.join(&stored_name);
        tokio::fs::write(&path, content).await?;
        let sha256 = hex::encode(Sha256::digest(content));
        let now = Utc::now();
        let storage_path = path.to_string_lossy().into_owned();
        let media_type = mime_from_filename(filename);
        Ok(Artifact {
            artifact_id,
            project_id: project_id.into(),
            kind: kind.into(),
            media_type,
            filename: filename.into(),
            size: i64::try_from(content.len()).unwrap_or(i64::MAX),
            sha256,
            created_in_round: round,
            related_entity_ids: related,
            created_at: now,
            storage_path,
        })
    }
}

pub(crate) async fn insert_artifact_record_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    artifact: &Artifact,
) -> StorageResult<()> {
    sqlx::query("INSERT INTO artifacts(artifact_id,project_id,kind,media_type,filename,size,sha256,created_in_round,related_entity_ids_json,storage_path,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
        .bind(&artifact.artifact_id).bind(&artifact.project_id).bind(&artifact.kind)
        .bind(&artifact.media_type).bind(&artifact.filename).bind(artifact.size)
        .bind(&artifact.sha256).bind(artifact.created_in_round)
        .bind(json_text(&artifact.related_entity_ids)?).bind(&artifact.storage_path)
        .bind(artifact.created_at.to_rfc3339()).execute(&mut **tx).await?;
    // Persist an explicit unverified audit state atomically with the artifact
    // reference. Computing the write-time digest protects content addressing,
    // but it is not a later independent read of the durable file.
    sqlx::query("INSERT INTO artifact_integrity_audit_state(artifact_id,project_id,status,check_count,updated_at) VALUES(?,?,'unverified',0,?)")
        .bind(&artifact.artifact_id)
        .bind(&artifact.project_id)
        .bind(artifact.created_at.to_rfc3339())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn update_project_status(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
    allowed: &[ProjectStatus],
    desired: ProjectStatus,
) -> StorageResult<()> {
    let status: String = sqlx::query_scalar("SELECT status FROM projects WHERE project_id=?")
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "project",
            id: project_id.into(),
        })?;
    if !allowed.iter().any(|candidate| candidate.as_str() == status) {
        return Err(StorageError::InvalidTransition(format!(
            "project {status} -> {desired}"
        )));
    }
    sqlx::query("UPDATE projects SET status=? WHERE project_id=?")
        .bind(desired.to_string())
        .bind(project_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn mime_from_filename(filename: &str) -> String {
    match Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some(extension) if extension.eq_ignore_ascii_case("md") => {
            "text/markdown; charset=utf-8".into()
        }
        Some(extension) if extension.eq_ignore_ascii_case("json") => "application/json".into(),
        Some(extension) if extension.eq_ignore_ascii_case("tex") => "application/x-tex".into(),
        _ => "application/octet-stream".into(),
    }
}
