// Storage exposes many fallible repository methods with one shared StorageError contract;
// duplicating identical error sections on every method would obscure the transaction logic.
#![allow(
    clippy::default_trait_access,
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

pub mod research_v2;

mod board;
mod board_commands;
mod candidate_ingestion;
mod control_plane;
mod distribution;
mod fact_catalog;
mod fact_governance;
#[cfg(feature = "postgres")]
mod postgres_committer;
mod problem_intake;
mod proof_obligations;
mod publication;
mod reliability_v2;
mod route_mutations;
mod rows;
mod security;
mod source_ingestion;
mod state_writer;
mod strategy;
mod suggestions;
mod task_materialization;
#[cfg(test)]
mod tests;
mod verification;
mod verification_runtime;
mod worker_output_ingestion;
mod write;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use chrono::Utc;
use research_domain::{
    Artifact, Budget, Candidate, DomainEvent, EntityRef, ExperimentCapsule, Fact, Goal, GraphEdge,
    GraphNode, GraphProjection, HumanCommand, Hypothesis, ProblemContract, Project,
    ProjectSnapshot, ResearchRound, Route, RouteProposal, SourceRecord, Task, Uncertainty,
    Verification, Worker,
};
use serde_json::{Value, json};
use sqlx::{
    Row, SqlitePool,
    sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow, SqliteSynchronous,
    },
};
use thiserror::Error;
use ulid::Ulid;

pub use board::BoardInclude;
pub use board_commands::BoardMutation;
pub use distribution::{TaskLeaseCompletion, TaskLeaseCompletionRequest};
#[cfg(feature = "postgres")]
pub use postgres_committer::{
    PostgresLease, PostgresPlanCommit, PostgresPlanTask, PostgresStateCommitter,
};
pub use problem_intake::{
    ProblemDraftBeginRequest, ProblemDraftCompletionRequest, ProblemDraftConfirmationRequest,
    ProblemDraftControlRequest, ProblemDraftFailureRequest, problem_document_hash,
    problem_material_manifest_hash,
};
pub use reliability_v2::{
    LocalResultIngestion, LocalResultSubmission, LocalTaskLease, LocalTaskOffer,
};
pub use state_writer::StateWriterSnapshot;
pub use verification::{
    BackendRunDraft, CheckDraft, EvidenceDraft, FindingDraft, ProofNodeDraft,
    VerificationCaseDraft, VerificationSnapshotDraft,
};
pub use verification_runtime::{VerificationWorkerLease, VerificationWorkerOffer};
pub use write::{
    CommandDraft, ModelCallPurpose, ModelCallRequest, PlanSaveResult, SubmissionReceipt,
    UsageReservation, VerificationCommit,
};

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("entity not found: {kind} {id}")]
    NotFound { kind: &'static str, id: String },
    #[error("revision conflict: expected {expected}, actual {actual}")]
    RevisionConflict { expected: i64, actual: i64 },
    #[error("idempotency conflict: {0}")]
    IdempotencyConflict(String),
    #[error("invalid problem revision: {0}")]
    InvalidProblemRevision(String),
    #[error("invalid route proposal: {0}")]
    InvalidRouteProposal(String),
    #[error("invalid transition: {0}")]
    InvalidTransition(String),
    #[error("late or stale worker submission: {0}")]
    LateSubmission(String),
    #[error("candidate dependency is missing or inactive: {0}")]
    InvalidDependency(String),
    #[error("fact dependency cycle detected")]
    DependencyCycle,
    #[error("model-call budget exhausted: {0}")]
    BudgetExhausted(String),
    #[error("corrupt stored data: {0}")]
    CorruptData(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type StorageResult<T> = Result<T, StorageError>;

pub(crate) fn route_execution_is_released(project_status: &str, human_review: &str) -> bool {
    matches!(project_status, "running" | "needs_human_review")
        && matches!(human_review, "approved" | "not_required")
}

pub(crate) async fn verification_execution_is_released(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    verification_id: &str,
) -> StorageResult<bool> {
    let released: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM verifications v \
         JOIN candidates c ON c.candidate_id=v.candidate_id AND c.project_id=v.project_id \
         JOIN projects p ON p.project_id=v.project_id \
         LEFT JOIN routes r ON r.project_id=v.project_id AND r.route_id=json_extract(c.submission_json,'$.route_id') \
         WHERE v.verification_id=? AND ( \
           (p.status='running' AND json_extract(c.submission_json,'$.route_id') IS NULL) OR ( \
             p.status IN ('running','needs_human_review') \
             AND r.human_review IN ('approved','not_required') \
             AND r.status IN ('incubating','active','probation','revived') \
           ) OR ( \
             p.status='needs_human_review' \
             AND EXISTS ( \
               SELECT 1 FROM fact_challenges fc \
               WHERE fc.project_id=v.project_id \
                 AND fc.verification_id=v.verification_id \
                 AND fc.status='open' \
             ) \
           ) \
         )",
    )
    .bind(verification_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(released == 1)
}

pub(crate) async fn verification_case_execution_is_released(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    case_id: &str,
) -> StorageResult<bool> {
    let verification_id: String =
        sqlx::query_scalar("SELECT verification_id FROM verification_cases WHERE case_id=?")
            .bind(case_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "verification_case",
                id: case_id.into(),
            })?;
    verification_execution_is_released(tx, &verification_id).await
}

pub(crate) fn route_attributes(proposal: &RouteProposal) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("risks".into(), json!(proposal.risks)),
        ("approach_kind".into(), json!(proposal.approach_kind)),
        ("route_role".into(), json!(proposal.route_role)),
        ("user_title".into(), json!(proposal.user_title)),
        (
            "plain_language_summary".into(),
            json!(proposal.plain_language_summary),
        ),
        ("why_this_route".into(), json!(proposal.why_this_route)),
        ("expected_output".into(), json!(proposal.expected_output)),
        ("relation_to_goal".into(), json!(proposal.relation_to_goal)),
        ("steps".into(), json!(proposal.steps)),
        (
            "expected_subgoals".into(),
            json!(proposal.expected_subgoals),
        ),
    ])
}

pub(crate) fn merged_route_attributes(
    existing_json: &str,
    proposal: &RouteProposal,
) -> StorageResult<String> {
    let mut attributes = serde_json::from_str::<BTreeMap<String, Value>>(existing_json)?;
    for (key, value) in route_attributes(proposal) {
        let empty = value.as_str().is_some_and(str::is_empty)
            || value.as_array().is_some_and(Vec::is_empty);
        if !empty {
            attributes.insert(key, value);
        }
    }
    json_text(&attributes)
}

pub(crate) fn normalized_source_identity(
    url: Option<&str>,
    citation_key: Option<&str>,
) -> (Option<String>, Option<String>, Option<String>) {
    let normalized_url = url.and_then(|raw| {
        let without_fragment = raw.trim().split('#').next().unwrap_or_default().trim();
        let normalized = without_fragment.trim_end_matches('/');
        (!normalized.is_empty()).then(|| normalized.to_owned())
    });
    let Some(normalized) = normalized_url.as_deref() else {
        let citation_key = citation_key
            .map(str::trim)
            .filter(|value| !value.is_empty());
        return match citation_key {
            Some(value) => (
                None,
                Some("citation_key".into()),
                Some(value.to_ascii_lowercase()),
            ),
            None => (None, None, None),
        };
    };
    let lower = normalized.to_ascii_lowercase();
    if let Some(index) = lower.find("doi.org/") {
        let value = lower[index + "doi.org/".len()..].trim().to_owned();
        if !value.is_empty() {
            return (normalized_url, Some("doi".into()), Some(value));
        }
    }
    if let Some(index) = lower.find("arxiv.org/") {
        let mut value = lower[index + "arxiv.org/".len()..].trim();
        value = value.strip_prefix("abs/").unwrap_or(value);
        value = value.strip_prefix("pdf/").unwrap_or(value);
        value = value.strip_suffix(".pdf").unwrap_or(value);
        if !value.is_empty() {
            return (normalized_url, Some("arxiv".into()), Some(value.to_owned()));
        }
    }
    (normalized_url, None, None)
}

#[derive(Debug, Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
    read_pool: SqlitePool,
    artifact_root: PathBuf,
    state_writer: state_writer::StateWriter,
}

impl SqliteStore {
    pub async fn connect(
        database_url: &str,
        artifact_root: impl Into<PathBuf>,
    ) -> StorageResult<Self> {
        let options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5))
            .synchronous(SqliteSynchronous::Normal);
        // SQLite is an MVP-only single-writer mode. A single pooled connection makes the
        // exclusivity enforceable even for legacy repository methods while the StateWriter
        // command facade provides bounded admission and metrics.
        let max_connections = 1;
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .connect_with(options.clone())
            .await?;
        sqlx::migrate!("../../migrations").run(&pool).await?;
        let read_pool = if database_url.contains(":memory:") {
            // SQLite in-memory databases are connection-local. Tests therefore share the
            // single writer pool, while file-backed deployments get a separate read pool.
            pool.clone()
        } else {
            SqlitePoolOptions::new()
                .max_connections(4)
                .connect_with(options.read_only(true).create_if_missing(false))
                .await?
        };
        let artifact_root = artifact_root.into();
        tokio::fs::create_dir_all(&artifact_root).await?;
        Ok(Self {
            pool,
            read_pool,
            artifact_root,
            state_writer: state_writer::StateWriter::spawn(),
        })
    }

    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    #[must_use]
    pub fn read_pool(&self) -> &SqlitePool {
        &self.read_pool
    }

    #[must_use]
    pub fn artifact_root(&self) -> &Path {
        &self.artifact_root
    }

    #[must_use]
    pub fn state_writer_status(&self) -> StateWriterSnapshot {
        self.state_writer.snapshot()
    }

    async fn admit_write(
        &self,
        priority: state_writer::WritePriority,
        command_kind: &'static str,
    ) -> StorageResult<state_writer::WriteAdmission> {
        self.state_writer.admit(priority, command_kind).await
    }

    pub async fn get_project(&self, project_id: &str) -> StorageResult<Project> {
        let row = sqlx::query("SELECT * FROM projects WHERE project_id = ?")
            .bind(project_id)
            .fetch_optional(&self.read_pool)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "project",
                id: project_id.into(),
            })?;
        rows::project(&row)
    }

    pub async fn list_rounds(&self, project_id: &str) -> StorageResult<Vec<ResearchRound>> {
        self.fetch_map(
            "SELECT * FROM rounds WHERE project_id = ? ORDER BY number",
            project_id,
            rows::round,
        )
        .await
    }

    pub async fn current_round(&self, project_id: &str) -> StorageResult<Option<ResearchRound>> {
        let row =
            sqlx::query("SELECT * FROM rounds WHERE project_id = ? ORDER BY number DESC LIMIT 1")
                .bind(project_id)
                .fetch_optional(&self.read_pool)
                .await?;
        row.as_ref().map(rows::round).transpose()
    }

    pub async fn list_routes(&self, project_id: &str) -> StorageResult<Vec<Route>> {
        self.fetch_map(
            "SELECT * FROM routes WHERE project_id = ? ORDER BY created_in_round, priority DESC",
            project_id,
            rows::route,
        )
        .await
    }

    pub async fn get_route(&self, project_id: &str, route_id: &str) -> StorageResult<Route> {
        self.fetch_one(
            "route",
            project_id,
            route_id,
            "SELECT * FROM routes WHERE project_id = ? AND route_id = ?",
            rows::route,
        )
        .await
    }

    pub async fn list_tasks(&self, project_id: &str) -> StorageResult<Vec<Task>> {
        self.fetch_map(
            "SELECT * FROM tasks WHERE project_id = ? ORDER BY round DESC, priority DESC",
            project_id,
            rows::task,
        )
        .await
    }

    /// Returns queued work whose own route has cleared its approval gate.
    ///
    /// A project may remain in `needs_human_review` while sibling routes are
    /// pending. In that state only tasks on explicitly approved routes are
    /// runnable. The same route-local check also prevents a pruned or rejected
    /// route from borrowing the enclosing project's runnable state.
    pub async fn list_released_queued_tasks(
        &self,
        project_id: &str,
        round: i64,
    ) -> StorageResult<Vec<Task>> {
        let rows = sqlx::query(
            "SELECT t.* FROM tasks t \
             JOIN routes r ON r.project_id=t.project_id AND r.route_id=t.route_id \
             JOIN projects p ON p.project_id=t.project_id \
             WHERE t.project_id=? AND t.round=? AND t.status='queued' \
               AND r.status IN ('incubating','active','probation','revived') \
               AND p.status IN ('running','needs_human_review') \
               AND r.human_review IN ('approved','not_required') \
             ORDER BY t.priority DESC,t.task_id",
        )
        .bind(project_id)
        .bind(round)
        .fetch_all(&self.read_pool)
        .await?;
        rows.iter().map(rows::task).collect()
    }

    pub async fn has_pending_route_reviews(&self, project_id: &str) -> StorageResult<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM routes WHERE project_id=? AND human_review='pending' \
             AND status NOT IN ('merged','pruned','failed','human_stopped')",
        )
        .bind(project_id)
        .fetch_one(&self.read_pool)
        .await?;
        Ok(count > 0)
    }

    pub async fn round_has_unfinished_tasks(
        &self,
        project_id: &str,
        round: i64,
    ) -> StorageResult<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM tasks WHERE project_id=? AND round=? \
             AND status IN ('open','queued','assigned','offered','leased','running','checkpointed','result_submitted','ingesting','paused','blocked')",
        )
        .bind(project_id)
        .bind(round)
        .fetch_one(&self.read_pool)
        .await?;
        Ok(count > 0)
    }

    /// Returns whether the round's currently released execution batch still has
    /// unfinished work. Tasks behind a pending human route review are not part of
    /// that batch and must not indefinitely block verification of an approved
    /// sibling route.
    pub async fn round_has_unfinished_released_tasks(
        &self,
        project_id: &str,
        round: i64,
    ) -> StorageResult<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM tasks t \
             JOIN routes r ON r.project_id=t.project_id AND r.route_id=t.route_id \
             WHERE t.project_id=? AND t.round=? \
               AND t.status IN ('open','queued','assigned','offered','leased','running','checkpointed','result_submitted','ingesting','paused','blocked') \
               AND r.status IN ('incubating','active','probation','revived') \
               AND r.human_review IN ('approved','not_required')",
        )
        .bind(project_id)
        .bind(round)
        .fetch_one(&self.read_pool)
        .await?;
        Ok(count > 0)
    }

    pub async fn get_task(&self, project_id: &str, task_id: &str) -> StorageResult<Task> {
        self.fetch_one(
            "task",
            project_id,
            task_id,
            "SELECT * FROM tasks WHERE project_id = ? AND task_id = ?",
            rows::task,
        )
        .await
    }

    pub async fn list_workers(&self, project_id: &str) -> StorageResult<Vec<Worker>> {
        self.fetch_map(
            "SELECT * FROM workers WHERE project_id = ? ORDER BY worker_id",
            project_id,
            rows::worker,
        )
        .await
    }

    pub async fn get_worker(&self, project_id: &str, worker_id: &str) -> StorageResult<Worker> {
        self.fetch_one(
            "worker",
            project_id,
            worker_id,
            "SELECT * FROM workers WHERE project_id = ? AND worker_id = ?",
            rows::worker,
        )
        .await
    }

    pub async fn list_goals(&self, project_id: &str) -> StorageResult<Vec<Goal>> {
        self.fetch_map(
            "SELECT * FROM goals WHERE project_id = ? ORDER BY priority DESC, goal_id",
            project_id,
            rows::goal,
        )
        .await
    }

    pub async fn list_hypotheses(&self, project_id: &str) -> StorageResult<Vec<Hypothesis>> {
        self.fetch_map("SELECT * FROM hypotheses WHERE project_id = ? ORDER BY created_in_round, hypothesis_id", project_id, rows::hypothesis).await
    }

    pub async fn list_facts(&self, project_id: &str) -> StorageResult<Vec<Fact>> {
        self.fetch_map(
            "SELECT * FROM facts WHERE project_id = ? ORDER BY created_at",
            project_id,
            rows::fact,
        )
        .await
    }

    pub async fn get_fact(&self, fact_id: &str) -> StorageResult<Fact> {
        let row = sqlx::query("SELECT * FROM facts WHERE fact_id = ?")
            .bind(fact_id)
            .fetch_optional(&self.read_pool)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "fact",
                id: fact_id.into(),
            })?;
        rows::fact(&row)
    }

    pub async fn fact_impact(&self, fact_id: &str) -> StorageResult<Vec<String>> {
        let rows = sqlx::query(
            "WITH RECURSIVE impact(id) AS (SELECT target_id FROM fact_edges WHERE source_id = ? UNION SELECT e.target_id FROM fact_edges e JOIN impact i ON e.source_id = i.id) SELECT DISTINCT id FROM impact",
        ).bind(fact_id).fetch_all(&self.read_pool).await?;
        rows.into_iter()
            .map(|row| row.try_get("id").map_err(StorageError::from))
            .collect()
    }

    pub async fn list_uncertainties(&self, project_id: &str) -> StorageResult<Vec<Uncertainty>> {
        self.fetch_map("SELECT * FROM uncertainties WHERE project_id = ? ORDER BY CASE severity WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 ELSE 3 END, created_at", project_id, rows::uncertainty).await
    }

    pub async fn list_sources(&self, project_id: &str) -> StorageResult<Vec<SourceRecord>> {
        self.fetch_map(
            "SELECT * FROM sources WHERE project_id = ? ORDER BY retrieved_at, source_id",
            project_id,
            rows::source,
        )
        .await
    }

    pub async fn list_source_ingestions(&self, project_id: &str) -> StorageResult<Vec<Value>> {
        let rows = sqlx::query("SELECT ingestion_id,task_id,route_id,source_id,disposition,reasons_json,normalized_url,identifier_kind,identifier_value,raw_json,created_at FROM source_ingestion_records WHERE project_id=? ORDER BY created_at,ingestion_id")
            .bind(project_id).fetch_all(&self.read_pool).await?;
        rows.iter()
            .map(|row| {
                Ok(json!({
                    "ingestion_id":row.try_get::<String,_>("ingestion_id")?,
                    "task_id":row.try_get::<Option<String>,_>("task_id")?,
                    "route_id":row.try_get::<Option<String>,_>("route_id")?,
                    "source_id":row.try_get::<Option<String>,_>("source_id")?,
                    "disposition":row.try_get::<String,_>("disposition")?,
                    "reasons":serde_json::from_str::<Value>(row.try_get("reasons_json")?)?,
                    "normalized_url":row.try_get::<Option<String>,_>("normalized_url")?,
                    "identifier_kind":row.try_get::<Option<String>,_>("identifier_kind")?,
                    "identifier_value":row.try_get::<Option<String>,_>("identifier_value")?,
                    "raw":serde_json::from_str::<Value>(row.try_get("raw_json")?)?,
                    "created_at":row.try_get::<String,_>("created_at")?,
                }))
            })
            .collect()
    }

    pub async fn list_experiment_capsules(
        &self,
        project_id: &str,
    ) -> StorageResult<Vec<ExperimentCapsule>> {
        self.fetch_map(
            "SELECT * FROM experiment_capsules WHERE project_id = ? ORDER BY created_at, capsule_id",
            project_id,
            rows::experiment_capsule,
        )
        .await
    }

    pub async fn get_experiment_capsule(
        &self,
        capsule_id: &str,
    ) -> StorageResult<ExperimentCapsule> {
        let row = sqlx::query("SELECT * FROM experiment_capsules WHERE capsule_id = ?")
            .bind(capsule_id)
            .fetch_optional(&self.read_pool)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "experiment_capsule",
                id: capsule_id.into(),
            })?;
        rows::experiment_capsule(&row)
    }

    pub async fn list_verifications(&self, project_id: &str) -> StorageResult<Vec<Verification>> {
        self.fetch_map("SELECT * FROM verifications WHERE project_id = ? ORDER BY COALESCE(completed_at, started_at) DESC", project_id, rows::verification).await
    }

    pub async fn get_verification(&self, verification_id: &str) -> StorageResult<Verification> {
        let row = sqlx::query("SELECT * FROM verifications WHERE verification_id = ?")
            .bind(verification_id)
            .fetch_optional(&self.read_pool)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "verification",
                id: verification_id.into(),
            })?;
        rows::verification(&row)
    }

    pub async fn get_candidate(&self, candidate_id: &str) -> StorageResult<Candidate> {
        let row = sqlx::query("SELECT * FROM candidates WHERE candidate_id = ?")
            .bind(candidate_id)
            .fetch_optional(&self.read_pool)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "candidate",
                id: candidate_id.into(),
            })?;
        rows::candidate(&row)
    }

    pub async fn get_command(
        &self,
        project_id: &str,
        command_id: &str,
    ) -> StorageResult<HumanCommand> {
        self.fetch_one(
            "command",
            project_id,
            command_id,
            "SELECT * FROM human_commands WHERE project_id = ? AND command_id = ?",
            rows::command,
        )
        .await
    }

    pub async fn command_by_idempotency(
        &self,
        project_id: &str,
        key: &str,
    ) -> StorageResult<Option<HumanCommand>> {
        let row = sqlx::query(
            "SELECT * FROM human_commands WHERE project_id = ? AND idempotency_key = ?",
        )
        .bind(project_id)
        .bind(key)
        .fetch_optional(&self.read_pool)
        .await?;
        row.as_ref().map(rows::command).transpose()
    }

    pub async fn latest_cursor(&self, project_id: &str) -> StorageResult<i64> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(cursor), 0) FROM events WHERE project_id = ?",
        )
        .bind(project_id)
        .fetch_one(&self.read_pool)
        .await?)
    }

    pub async fn list_events_after(
        &self,
        project_id: &str,
        after: i64,
        limit: i64,
    ) -> StorageResult<Vec<DomainEvent>> {
        let rows = sqlx::query(
            "SELECT * FROM events WHERE project_id = ? AND cursor > ? ORDER BY cursor LIMIT ?",
        )
        .bind(project_id)
        .bind(after)
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.read_pool)
        .await?;
        rows.iter().map(event_from_row).collect()
    }

    pub async fn list_artifacts(&self, project_id: &str) -> StorageResult<Vec<Artifact>> {
        self.fetch_map(
            "SELECT * FROM artifacts WHERE project_id = ? ORDER BY created_at DESC",
            project_id,
            rows::artifact,
        )
        .await
    }

    pub async fn get_artifact(
        &self,
        project_id: &str,
        artifact_id: &str,
    ) -> StorageResult<Artifact> {
        self.fetch_one(
            "artifact",
            project_id,
            artifact_id,
            "SELECT * FROM artifacts WHERE project_id = ? AND artifact_id = ?",
            rows::artifact,
        )
        .await
    }

    pub async fn latest_report(&self, project_id: &str) -> StorageResult<Artifact> {
        let row = sqlx::query("SELECT * FROM artifacts WHERE project_id = ? AND kind IN ('latest_report','round_report') ORDER BY created_at DESC LIMIT 1")
            .bind(project_id).fetch_optional(&self.read_pool).await?
            .ok_or_else(|| StorageError::NotFound { kind: "report", id: project_id.into() })?;
        rows::artifact(&row)
    }

    pub async fn list_failure_patterns(&self, project_id: &str) -> StorageResult<Vec<Value>> {
        let rows = sqlx::query("SELECT pattern_json FROM failure_patterns WHERE project_id = ? ORDER BY confidence DESC")
            .bind(project_id).fetch_all(&self.read_pool).await?;
        rows.into_iter()
            .map(|row| Ok(serde_json::from_str(row.try_get("pattern_json")?)?))
            .collect()
    }

    pub async fn pending_suggestions(
        &self,
        project_id: &str,
        planning_round: i64,
    ) -> StorageResult<Vec<Value>> {
        let rows = sqlx::query("SELECT suggestion_id, content, target_route_id, effective_round FROM suggestions WHERE project_id = ? AND status = 'pending' AND effective_round <= ? ORDER BY effective_round, suggestion_id")
            .bind(project_id).bind(planning_round).fetch_all(&self.read_pool).await?;
        rows.into_iter()
            .map(|row| {
                Ok(serde_json::json!({
                    "suggestion_id": row.try_get::<String, _>("suggestion_id")?,
                    "content": row.try_get::<String, _>("content")?,
                    "target_route_id": row.try_get::<Option<String>, _>("target_route_id")?,
                    "effective_round": row.try_get::<i64, _>("effective_round")?,
                }))
            })
            .collect()
    }

    pub async fn snapshot(&self, project_id: &str) -> StorageResult<ProjectSnapshot> {
        let project = self.get_project(project_id).await?;
        let (
            current_round,
            workers,
            tasks,
            routes,
            goals,
            hypotheses,
            facts,
            uncertainties,
            sources,
            event_cursor,
        ) = tokio::try_join!(
            self.current_round(project_id),
            self.list_workers(project_id),
            self.list_tasks(project_id),
            self.list_routes(project_id),
            self.list_goals(project_id),
            self.list_hypotheses(project_id),
            self.list_facts(project_id),
            self.list_uncertainties(project_id),
            self.list_sources(project_id),
            self.latest_cursor(project_id),
        )?;
        let project_revision = project.revision;
        Ok(ProjectSnapshot {
            project,
            current_round,
            workers,
            tasks,
            routes,
            goals,
            hypotheses,
            facts,
            uncertainties,
            sources,
            project_revision,
            event_cursor,
        })
    }

    pub async fn graph(
        &self,
        project_id: &str,
        graph_type: &str,
    ) -> StorageResult<GraphProjection> {
        let project = self.get_project(project_id).await?;
        let (nodes, edges) = match graph_type {
            "goals" | "goal" => {
                let nodes = self
                    .list_goals(project_id)
                    .await?
                    .into_iter()
                    .map(|g| GraphNode {
                        id: g.goal_id,
                        kind: "goal".into(),
                        label: g.statement,
                        status: g.status.to_string(),
                        attributes: Default::default(),
                    })
                    .collect();
                (nodes, self.graph_edges(project_id, "goal_edges").await?)
            }
            "hypotheses" | "hypothesis" => {
                let mut nodes: Vec<_> = self
                    .list_hypotheses(project_id)
                    .await?
                    .into_iter()
                    .map(|h| GraphNode {
                        id: h.hypothesis_id,
                        kind: h.kind,
                        label: h.statement,
                        status: h.status,
                        attributes: h.attributes,
                    })
                    .collect();
                nodes.extend(
                    self.list_routes(project_id)
                        .await?
                        .into_iter()
                        .map(|r| GraphNode {
                            id: r.route_id,
                            kind: "route".into(),
                            label: r.title,
                            status: r.status.to_string(),
                            attributes: r.attributes,
                        }),
                );
                (
                    nodes,
                    self.graph_edges(project_id, "hypothesis_edges").await?,
                )
            }
            "facts" | "fact" => {
                let nodes = self
                    .list_facts(project_id)
                    .await?
                    .into_iter()
                    .map(|f| GraphNode {
                        id: f.fact_id,
                        kind: "fact".into(),
                        label: f.statement,
                        status: f.status.to_string(),
                        attributes: Default::default(),
                    })
                    .collect();
                (nodes, self.graph_edges(project_id, "fact_edges").await?)
            }
            "combined" => {
                let mut nodes = Vec::new();
                let mut edges = Vec::new();
                for kind in ["goals", "hypotheses", "facts"] {
                    let graph = Box::pin(self.graph(project_id, kind)).await?;
                    nodes.extend(graph.nodes);
                    edges.extend(graph.edges);
                }
                for source in self.list_sources(project_id).await? {
                    nodes.push(GraphNode {
                        id: source.source_id,
                        kind: "source".into(),
                        label: source.title,
                        status: source.status,
                        attributes: BTreeMap::from([
                            ("url".into(), serde_json::to_value(source.url)?),
                            (
                                "theorem_reference".into(),
                                serde_json::to_value(source.theorem_reference)?,
                            ),
                        ]),
                    });
                }
                for verification in self.list_verifications(project_id).await? {
                    let candidate = self.get_candidate(&verification.candidate_id).await?;
                    for source_id in &candidate.submission.external_source_ids {
                        edges.push(GraphEdge {
                            id: format!("edge_source_{source_id}_{}", verification.verification_id),
                            source: source_id.clone(),
                            target: verification.verification_id.clone(),
                            kind: "evidence_for".into(),
                        });
                    }
                    nodes.push(GraphNode {
                        id: verification.verification_id,
                        kind: "verification".into(),
                        label: format!("Verification of {}", verification.candidate_id),
                        status: verification.status.to_string(),
                        attributes: BTreeMap::from([(
                            "candidate_id".into(),
                            json!(verification.candidate_id),
                        )]),
                    });
                }
                for fact in self.list_facts(project_id).await? {
                    for verification_id in &fact.verification_ids {
                        edges.push(GraphEdge {
                            id: format!("edge_{verification_id}_{}", fact.fact_id),
                            source: verification_id.clone(),
                            target: fact.fact_id.clone(),
                            kind: "certifies".into(),
                        });
                    }
                }
                for goal in self.list_goals(project_id).await? {
                    if let Some(fact_id) = goal.solved_by_fact_id {
                        edges.push(GraphEdge {
                            id: format!("edge_{fact_id}_{}", goal.goal_id),
                            source: fact_id,
                            target: goal.goal_id,
                            kind: "solves".into(),
                        });
                    }
                }
                let proof_rows = sqlx::query("SELECT n.node_id,n.parent_node_id,n.status,n.tactic,n.depth,c.verification_id FROM proof_nodes n JOIN proof_searches s ON n.search_id=s.search_id JOIN formalizations f ON s.formalization_id=f.formalization_id JOIN verification_cases c ON f.case_id=c.case_id WHERE c.project_id=? ORDER BY n.created_at,n.node_id")
                        .bind(project_id).fetch_all(&self.read_pool).await?;
                for row in proof_rows {
                    let node_id: String = row.try_get("node_id")?;
                    let verification_id: String = row.try_get("verification_id")?;
                    let tactic: Option<String> = row.try_get("tactic")?;
                    let parent_id: Option<String> = row.try_get("parent_node_id")?;
                    nodes.push(GraphNode {
                        id: node_id.clone(),
                        kind: "proof".into(),
                        label: tactic
                            .clone()
                            .unwrap_or_else(|| "initial proof state".into()),
                        status: row.try_get("status")?,
                        attributes: BTreeMap::from([
                            ("depth".into(), json!(row.try_get::<i64, _>("depth")?)),
                            ("tactic".into(), json!(tactic)),
                        ]),
                    });
                    edges.push(GraphEdge {
                        id: format!("edge_proof_context_{node_id}"),
                        source: verification_id,
                        target: node_id.clone(),
                        kind: "has_proof_state".into(),
                    });
                    if let Some(parent_id) = parent_id {
                        edges.push(GraphEdge {
                            id: format!("edge_proof_{parent_id}_{node_id}"),
                            source: parent_id,
                            target: node_id,
                            kind: "expands_to".into(),
                        });
                    }
                }
                (nodes, edges)
            }
            other => {
                return Err(StorageError::InvalidTransition(format!(
                    "unknown graph type {other}"
                )));
            }
        };
        Ok(GraphProjection {
            graph_type: graph_type.into(),
            revision: project.revision,
            nodes,
            edges,
        })
    }

    async fn graph_edges(&self, project_id: &str, table: &str) -> StorageResult<Vec<GraphEdge>> {
        let sql =
            format!("SELECT edge_id, source_id, target_id, kind FROM {table} WHERE project_id = ?");
        let rows = sqlx::query(&sql)
            .bind(project_id)
            .fetch_all(&self.read_pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(GraphEdge {
                    id: row.try_get("edge_id")?,
                    source: row.try_get("source_id")?,
                    target: row.try_get("target_id")?,
                    kind: row.try_get("kind")?,
                })
            })
            .collect()
    }

    async fn fetch_map<T>(
        &self,
        sql: &str,
        project_id: &str,
        map: fn(&SqliteRow) -> StorageResult<T>,
    ) -> StorageResult<Vec<T>> {
        let rows = sqlx::query(sql)
            .bind(project_id)
            .fetch_all(&self.read_pool)
            .await?;
        rows.iter().map(map).collect()
    }

    async fn fetch_one<T>(
        &self,
        kind: &'static str,
        project_id: &str,
        id: &str,
        sql: &str,
        map: fn(&SqliteRow) -> StorageResult<T>,
    ) -> StorageResult<T> {
        let row = sqlx::query(sql)
            .bind(project_id)
            .bind(id)
            .fetch_optional(&self.read_pool)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind,
                id: id.into(),
            })?;
        map(&row)
    }
}

fn event_from_row(row: &SqliteRow) -> StorageResult<DomainEvent> {
    let occurred_at = rows::timestamp(row.try_get("occurred_at")?)?;
    Ok(DomainEvent {
        event_id: row.try_get("event_id")?,
        cursor: row.try_get("cursor")?,
        project_id: row.try_get("project_id")?,
        project_revision: row.try_get("project_revision")?,
        event_type: row.try_get("type")?,
        entity: serde_json::from_str(row.try_get("entity_json")?)?,
        data: serde_json::from_str(row.try_get("data_json")?)?,
        caused_by: row
            .try_get::<Option<String>, _>("caused_by_json")?
            .map(|v| serde_json::from_str(&v))
            .transpose()?,
        occurred_at,
    })
}

pub(crate) fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", Ulid::new())
}

pub(crate) async fn append_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
    revision: i64,
    event_type: &str,
    entity: EntityRef,
    data: Value,
    caused_by: Option<EntityRef>,
) -> StorageResult<DomainEvent> {
    let event_id = new_id("event");
    let occurred_at = Utc::now();
    let result = sqlx::query("INSERT INTO events(event_id, project_id, project_revision, type, entity_json, data_json, caused_by_json, occurred_at) VALUES(?,?,?,?,?,?,?,?)")
        .bind(&event_id).bind(project_id).bind(revision).bind(event_type)
        .bind(serde_json::to_string(&entity)?).bind(serde_json::to_string(&data)?)
        .bind(caused_by.as_ref().map(serde_json::to_string).transpose()?).bind(occurred_at.to_rfc3339())
        .execute(&mut **tx).await?;
    let cursor = result.last_insert_rowid();
    let outbox_id = new_id("outbox");
    let payload = json!({
        "event_id": event_id,
        "cursor": cursor,
        "project_id": project_id,
        "project_revision": revision,
        "type": event_type,
        "entity": entity,
        "data": data,
        "caused_by": caused_by,
        "occurred_at": occurred_at,
    });
    // SQLite's durable `events` table is the local delivery sink. Live SSE/WS
    // clients replay from it by cursor, so leaving a second undrained pending
    // queue would report false storage degradation and grow without bound.
    // The experimental PostgreSQL committer keeps a real pending outbox for an
    // external dispatcher.
    sqlx::query("INSERT INTO event_outbox(outbox_id,event_id,project_id,event_type,payload_json,status,available_at,delivered_at) VALUES(?,?,?,?,?,'delivered',?,?)")
        .bind(outbox_id)
        .bind(&event_id)
        .bind(project_id)
        .bind(event_type)
        .bind(serde_json::to_string(&payload)?)
        .bind(occurred_at.to_rfc3339())
        .bind(occurred_at.to_rfc3339())
        .execute(&mut **tx)
        .await?;
    Ok(DomainEvent {
        event_id,
        cursor,
        project_id: project_id.into(),
        project_revision: revision,
        event_type: event_type.into(),
        entity,
        data,
        caused_by,
        occurred_at,
    })
}

pub(crate) async fn current_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
) -> StorageResult<i64> {
    sqlx::query_scalar("SELECT revision FROM projects WHERE project_id = ?")
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "project",
            id: project_id.into(),
        })
}

pub(crate) async fn bump_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
) -> StorageResult<i64> {
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE projects SET revision = revision + 1, updated_at = ? WHERE project_id = ?")
        .bind(now)
        .bind(project_id)
        .execute(&mut **tx)
        .await?;
    current_revision(tx, project_id).await
}

pub(crate) fn json_text<T: serde::Serialize + ?Sized>(value: &T) -> StorageResult<String> {
    Ok(serde_json::to_string(value)?)
}

pub(crate) fn entity(kind: &str, id: &str) -> EntityRef {
    EntityRef {
        kind: kind.into(),
        id: id.into(),
    }
}

#[must_use]
pub fn default_project_values(problem: String) -> (ProblemContract, Budget) {
    (
        ProblemContract {
            original_problem: problem.clone(),
            target_statement: problem,
            assumptions: vec![],
            success_criteria: "目标陈述得到 accepted 裁决，依赖闭包均为 active，且无阻塞不确定性"
                .into(),
            version: 1,
        },
        Budget::default(),
    )
}
