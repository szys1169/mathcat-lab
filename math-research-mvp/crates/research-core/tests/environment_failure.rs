use std::sync::Arc;

use research_core::{ResearchConfig, ResearchService};
use research_domain::{Budget, CommandMode, ProblemContract, ProjectStatus};
use research_storage::{CommandDraft, SqliteStore};
use research_worker_runtime::MockBackend;
use serde_json::json;

#[tokio::test]
async fn unavailable_model_backend_is_not_reported_as_partial_research() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
        .await
        .expect("store");
    let service = ResearchService::new(
        store.clone(),
        Arc::new(MockBackend::default()),
        ResearchConfig {
            runtime_root: temp.path().join("runtime"),
            output_root: temp.path().join("output"),
            model: None,
            lean_project_root: None,
            planner_timeout_seconds: 1,
            worker_timeout_seconds: 1,
            verifier_timeout_seconds: 1,
            ..ResearchConfig::default()
        },
    );
    let project = service
        .create_project(
            "backend failure".into(),
            ProblemContract {
                original_problem: "Prove 1+1=2".into(),
                target_statement: "1+1=2".into(),
                assumptions: vec![],
                success_criteria: "verified proof".into(),
                version: 1,
            },
            Budget {
                max_rounds: 1,
                max_total_model_calls: 4,
                ..Budget::default()
            },
        )
        .await
        .expect("create project");
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
                idempotency_key: "start-environment-failure".into(),
                reason: "test".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue start");
    store
        .apply_command(&project.project_id, &command.command_id)
        .await
        .expect("apply start");

    service
        .run_project_until_terminal(&project.project_id)
        .await
        .expect("bounded failed run");

    let terminal = store
        .get_project(&project.project_id)
        .await
        .expect("terminal project");
    assert_eq!(terminal.status, ProjectStatus::EnvironmentFailed);
    assert_eq!(
        store
            .successful_model_calls(&project.project_id)
            .await
            .expect("successful calls"),
        0
    );
    let failed_calls: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM usage_records WHERE project_id=? AND outcome='failed'",
    )
    .bind(&project.project_id)
    .fetch_one(store.pool())
    .await
    .expect("failed usage rows");
    assert!(failed_calls > 0);
}
