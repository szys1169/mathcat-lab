use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use research_domain::{Budget, CommandMode, ProblemContract, ResearchRound};
use research_storage::{CommandDraft, SqliteStore, StorageError};
use research_worker_runtime::{
    AgentBackend, AgentError, AgentHandle, AgentRunResult, AgentSpec, AgentTask,
    BackendCapabilities,
};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::{CoreError, PlanningStageRequest, ResearchConfig, ResearchService};

struct PlanningBackend {
    store: SqliteStore,
    calls: Mutex<Vec<(String, u64)>>,
}

#[async_trait]
impl AgentBackend for PlanningBackend {
    fn name(&self) -> &'static str {
        "planning-budget-fixture"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            resumable_session: false,
            safe_point_steering: false,
            non_waking_injection: false,
            graceful_cancel: true,
            event_stream: false,
            structured_output: true,
            tool_permissions: false,
        }
    }

    async fn create(&self, spec: AgentSpec) -> Result<AgentHandle, AgentError> {
        Ok(AgentHandle {
            handle_id: spec.role.clone(),
            project_id: spec.project_id,
            role: spec.role,
            model: spec.model,
            working_directory: spec.working_directory,
            session_id: None,
        })
    }

    async fn run(
        &self,
        handle: &AgentHandle,
        task: AgentTask,
        _: CancellationToken,
    ) -> Result<AgentRunResult, AgentError> {
        self.calls
            .lock()
            .await
            .push((handle.role.clone(), task.timeout_seconds));
        if handle.role == "retry_stage" {
            sqlx::query("UPDATE planning_stage_attempts SET started_at=? WHERE project_id=? AND stage='strategy_director'")
                .bind((Utc::now() - chrono::Duration::seconds(601)).to_rfc3339())
                .bind(&handle.project_id).execute(self.store.pool()).await.expect("expire round fixture");
            return Err(AgentError::Timeout(task.timeout_seconds));
        }
        Ok(AgentRunResult {
            structured_output: json!({"stage":handle.role}),
            session_id: None,
            raw_events: vec![],
            input_tokens: 1,
            output_tokens: 1,
            stderr: String::new(),
            started_at: Utc::now(),
            completed_at: Utc::now(),
        })
    }

    async fn resume(
        &self,
        handle: &AgentHandle,
        task: AgentTask,
        cancellation: CancellationToken,
    ) -> Result<AgentRunResult, AgentError> {
        self.run(handle, task, cancellation).await
    }
}

async fn run_stage(
    service: &ResearchService,
    round: &ResearchRound,
    stage: &str,
) -> Result<Value, CoreError> {
    let snapshot = service.store.snapshot(&round.project_id).await?;
    service
        .run_or_resume_planning_stage(PlanningStageRequest {
            round,
            snapshot: &snapshot,
            stage,
            role: stage,
            input: &json!({"stable":"input"}),
            prompt: format!("plan {stage}"),
            output_schema: json!({"type":"object"}),
        })
        .await
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn planning_round_budget_is_shared_by_stages_retries_and_restarts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
        .await
        .expect("store");
    let backend = Arc::new(PlanningBackend {
        store: store.clone(),
        calls: Mutex::new(vec![]),
    });
    let config = ResearchConfig {
        planner_timeout_seconds: 1_200,
        planner_round_timeout_seconds: Some(600),
        runtime_root: temp.path().join("runtime"),
        output_root: temp.path().join("output"),
        ..ResearchConfig::default()
    };
    let service = ResearchService::new(store.clone(), backend.clone(), config.clone());
    let project = service
        .create_project(
            "planning-budget".into(),
            ProblemContract {
                original_problem: "A".into(),
                target_statement: "A".into(),
                assumptions: vec![],
                success_criteria: "A".into(),
                version: 1,
            },
            Budget::default(),
        )
        .await
        .expect("project");
    let (command, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "start_project".into(),
                target_kind: "project".into(),
                target_id: project.project_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: project.revision,
                idempotency_key: "planning-budget-start".into(),
                reason: "test".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue");
    store
        .apply_command(&project.project_id, &command.command_id)
        .await
        .expect("start");
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    assert!(
        store
            .planning_round_started_at(&project.project_id, &round.round_id)
            .await
            .expect("initial start")
            .is_none()
    );
    run_stage(&service, &round, "strategy_director")
        .await
        .expect("first stage");
    let earlier = Utc::now() - chrono::Duration::seconds(400);
    sqlx::query("UPDATE planning_stage_attempts SET started_at=? WHERE round_id=?")
        .bind(earlier.to_rfc3339())
        .bind(&round.round_id)
        .execute(store.pool())
        .await
        .expect("elapsed fixture");
    run_stage(&service, &round, "route_generator")
        .await
        .expect("second stage");
    let calls = backend.calls.lock().await.clone();
    assert_eq!(calls[0].1, 600);
    assert!(
        calls[1].1 <= 200,
        "second stage must consume the first stage's elapsed budget"
    );
    assert!(calls[1].1 > 0);
    let deadlines: (String, String) = sqlx::query_as("SELECT soft_deadline_at,hard_deadline_at FROM planning_stage_attempts WHERE round_id=? AND stage='route_generator'")
        .bind(&round.round_id).fetch_one(store.pool()).await.expect("deadlines");
    let soft = chrono::DateTime::parse_from_rfc3339(&deadlines.0).expect("soft");
    let hard = chrono::DateTime::parse_from_rfc3339(&deadlines.1).expect("hard");
    assert!(soft <= hard);
    assert!((hard.with_timezone(&Utc) - earlier).num_seconds() <= 600);
    assert!(matches!(run_stage(&service, &round, "retry_stage").await,
        Err(CoreError::Storage(StorageError::BudgetExhausted(reason))) if reason.contains("planner round")));
    assert_eq!(
        backend.calls.lock().await.len(),
        3,
        "expired retry must not dispatch another model call"
    );
    let first_start = store
        .planning_round_started_at(&project.project_id, &round.round_id)
        .await
        .expect("persisted start");

    let restarted = ResearchService::new(store.clone(), backend.clone(), config);
    assert!(matches!(
        run_stage(&restarted, &round, "fresh_after_restart").await,
        Err(CoreError::Storage(StorageError::BudgetExhausted(_)))
    ));
    assert!(matches!(
        run_stage(&restarted, &round, "retry_stage").await,
        Err(CoreError::Storage(StorageError::BudgetExhausted(_)))
    ));
    assert_eq!(
        store
            .planning_round_started_at(&project.project_id, &round.round_id)
            .await
            .expect("same start"),
        first_start
    );
    assert_eq!(
        run_stage(&restarted, &round, "strategy_director")
            .await
            .expect("completed checkpoint"),
        json!({"stage":"strategy_director"})
    );
    assert_eq!(backend.calls.lock().await.len(), 3);

    // Legacy input-only checkpoint hashes remain safe to reuse only because a
    // completed attempt attests to the full unchanged prompt/schema contract.
    sqlx::query(
        "UPDATE planning_stage_runs SET input_hash=? WHERE round_id=? AND stage='route_generator'",
    )
    .bind(super::sha256_json(&json!({"stable":"input"})).expect("hash"))
    .bind(&round.round_id)
    .execute(store.pool())
    .await
    .expect("legacy checkpoint");
    run_stage(&restarted, &round, "route_generator")
        .await
        .expect("legacy checkpoint reuse");
    assert_eq!(backend.calls.lock().await.len(), 3);

    let uncapped = ResearchService::new(
        store.clone(),
        backend.clone(),
        ResearchConfig {
            planner_timeout_seconds: 1_200,
            runtime_root: temp.path().join("runtime"),
            ..ResearchConfig::default()
        },
    );
    run_stage(&uncapped, &round, "uncapped_default")
        .await
        .expect("default preserves existing behavior");
    assert_eq!(backend.calls.lock().await.last().expect("call").1, 600);
}
