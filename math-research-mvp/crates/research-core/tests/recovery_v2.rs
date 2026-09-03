use std::sync::Arc;

use research_core::{ResearchConfig, ResearchService};
use research_domain::{
    AssignmentDraft, Budget, CommandMode, PlannerOutput, ProblemContract, RoundStatus,
    RouteProposal, TaskStatus,
};
use research_storage::{CommandDraft, SqliteStore};
use research_worker_runtime::MockBackend;
use serde_json::json;

fn recovery_plan() -> PlannerOutput {
    let route = |title: &str, method: &str| RouteProposal {
        title: title.into(),
        method_summary: method.into(),
        target_goal_ids: vec![],
        required_fact_ids: vec![],
        expected_subgoals: vec![],
        expected_goal_progress: 0.5,
        uncertainty_reduction: 0.5,
        human_suggestion_alignment: 0.0,
        evidence_support: 0.5,
        route_diversity: 0.8,
        verifiability: 0.8,
        novelty: 0.4,
        failure_similarity_penalty: 0.0,
        cost_penalty: 0.1,
        risks: vec![],
    };
    PlannerOutput {
        rationale_summary: "persisted plan before simulated process restart".into(),
        routes: vec![
            route("direct", "derive the target from the definitions"),
            route("attack", "search degenerate boundary cases"),
        ],
        assignments: vec![AssignmentDraft {
            route_index: 0,
            worker_role: "prover".into(),
            strategic_role: "central_bridge".into(),
            addresses_interface_debt: true,
            goal_ids: vec![],
            objective: "produce one auditable narrow result".into(),
            completion_contract: "candidate, blocker, or precise partial result".into(),
            priority: 1.0,
        }],
        targeted_uncertainty_ids: vec![],
        suggestion_decisions: vec![],
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn run_once_resumes_committed_round_from_checkpoint_policy_without_replanning() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
        .await
        .expect("store");
    let backend = MockBackend::from_responses([json!({
        "summary":"resumed task produced a precise partial result",
        "discoveries":[],
        "candidates":[],
        "failures":[],
        "uncertainties":[],
        "sources":[],
        "experiments":[]
    })]);
    let service = ResearchService::new(
        store.clone(),
        Arc::new(backend),
        ResearchConfig {
            runtime_root: temp.path().join("runtime"),
            output_root: temp.path().join("output"),
            planner_timeout_seconds: 5,
            worker_timeout_seconds: 5,
            verifier_timeout_seconds: 5,
            ..ResearchConfig::default()
        },
    );
    let project = service
        .create_project(
            "recovery-v2".into(),
            ProblemContract {
                original_problem: "Prove a stable target".into(),
                target_statement: "stable target".into(),
                assumptions: vec![],
                success_criteria: "accepted".into(),
                version: 1,
            },
            Budget {
                max_rounds: 3,
                max_parallel_workers: 1,
                ..Budget::default()
            },
        )
        .await
        .expect("project");
    let (start, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "start_project".into(),
                target_kind: "project".into(),
                target_id: project.project_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: project.revision,
                idempotency_key: "start-recovery-v2".into(),
                reason: "test".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue start");
    store
        .apply_command(&project.project_id, &start.command_id)
        .await
        .expect("start project");
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &recovery_plan(),
            &delta,
            "primary",
            None,
        )
        .await
        .expect("persisted plan");
    let task_id = saved.tasks[0].task_id.clone();

    service
        .run_one_round(&project.project_id)
        .await
        .expect("resume committed round");

    let rounds = store
        .list_rounds(&project.project_id)
        .await
        .expect("rounds");
    assert_eq!(rounds.len(), 1, "recovery must not create a second round");
    assert_eq!(rounds[0].status, RoundStatus::Completed);
    assert_eq!(
        store
            .get_task(&project.project_id, &task_id)
            .await
            .expect("task")
            .status,
        TaskStatus::Completed
    );
    let history = store
        .task_attempt_history(&project.project_id, &task_id)
        .await
        .expect("history");
    assert_eq!(
        history["checkpoints"]
            .as_array()
            .expect("checkpoints")
            .len(),
        1
    );
    assert_eq!(
        store
            .list_plan_revisions(&project.project_id)
            .await
            .expect("plan revisions")
            .len(),
        1
    );
}
