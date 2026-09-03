use std::sync::Arc;

use research_core::{ResearchConfig, ResearchService};
use research_domain::{Budget, CommandMode, ProblemContract};
use research_storage::{CommandDraft, SqliteStore};
use research_worker_runtime::MockBackend;
use serde_json::json;

#[tokio::test]
async fn unavailable_planner_waits_instead_of_manufacturing_generic_work() {
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
            lean_project_root: None,
            planner_timeout_seconds: 1,
            worker_timeout_seconds: 1,
            verifier_timeout_seconds: 1,
            ..ResearchConfig::default()
        },
    );
    let project = service
        .create_project(
            "degraded planner".into(),
            ProblemContract {
                original_problem: "Prove a named theorem with no existing route".into(),
                target_statement: "A named theorem".into(),
                assumptions: vec![],
                success_criteria: "fully certified".into(),
                version: 1,
            },
            Budget {
                max_rounds: 1,
                ..Budget::default()
            },
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
                idempotency_key: "start-degraded".into(),
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

    service
        .run_one_round(&project.project_id)
        .await
        .expect("degraded round");

    assert!(
        store
            .list_routes(&project.project_id)
            .await
            .expect("routes")
            .is_empty()
    );
    assert!(
        store
            .list_tasks(&project.project_id)
            .await
            .expect("tasks")
            .is_empty()
    );
    let revisions = store
        .list_plan_revisions(&project.project_id)
        .await
        .expect("plan revisions");
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].planner_mode, "deterministic_continuity");
    let events = store
        .list_events_after(&project.project_id, 0, 1_000)
        .await
        .expect("events");
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "planner.degraded_waiting")
    );
    assert!(!events.iter().any(|event| {
        event.data.to_string().contains("直接证明") || event.data.to_string().contains("边界反例")
    }));
}
