use std::sync::Arc;

use research_core::{ResearchConfig, ResearchService};
use research_domain::{
    AssignmentDraft, Budget, CandidateDraft, CandidateType, CommandMode, PlannerOutput,
    ProblemContract, RouteProposal, WorkerOutput,
};
use research_storage::{CommandDraft, SqliteStore};
use research_worker_runtime::MockBackend;
use serde_json::json;

fn route(title: &str, diversity: f64) -> RouteProposal {
    RouteProposal {
        title: title.into(),
        method_summary: format!("auditable {title} route"),
        target_goal_ids: vec![],
        required_fact_ids: vec![],
        expected_subgoals: vec![],
        expected_goal_progress: 0.5,
        uncertainty_reduction: 0.5,
        human_suggestion_alignment: 0.0,
        evidence_support: 0.5,
        route_diversity: diversity,
        verifiability: 0.9,
        novelty: 0.2,
        failure_similarity_penalty: 0.0,
        cost_penalty: 0.1,
        risks: vec![],
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn remote_candidates_pass_through_the_same_fact_gate() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
        .await
        .expect("store");
    let forward_review = json!({
        "verdict":"accepted",
        "summary":"Forward step audit confirms that n = 2k directly witnesses divisibility by two.",
        "critical_errors":[],"gaps":[],"uncertainties":[],"repair_actions":[],
        "checked_fact_ids":[],"checked_source_ids":[],
        "evidence_level":"independent_llm_check"
    });
    let falsification_review = json!({
        "verdict":"accepted",
        "summary":"The minimal and boundary counterexample search finds none because evenness already supplies an integral witness k.",
        "critical_errors":[],"gaps":[],"uncertainties":[],"repair_actions":[],
        "checked_fact_ids":[],"checked_source_ids":[],
        "evidence_level":"independent_llm_check"
    });
    let adversarial_review = json!({
        "verdict":"accepted",
        "summary":"The adversarial audit finds no quantifier or domain escape in the definition-based argument.",
        "critical_errors":[],"gaps":[],"uncertainties":[],"repair_actions":[],
        "checked_fact_ids":[],"checked_source_ids":[],
        "evidence_level":"independent_llm_check"
    });
    let service = ResearchService::new(
        store.clone(),
        Arc::new(MockBackend::from_responses([
            forward_review,
            falsification_review,
            adversarial_review,
        ])),
        ResearchConfig {
            runtime_root: temp.path().join("runtime"),
            output_root: temp.path().join("output"),
            lean_project_root: None,
            ..ResearchConfig::default()
        },
    );
    let project = service
        .create_project(
            "distributed gate".into(),
            ProblemContract {
                original_problem: "Prove a difficult main theorem".into(),
                target_statement: "The difficult main theorem".into(),
                assumptions: vec![],
                success_criteria: "fully certified main target".into(),
                version: 1,
            },
            Budget::default(),
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
                idempotency_key: "distributed-start".into(),
                reason: "test".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue start");
    store
        .apply_command(&project.project_id, &command.command_id)
        .await
        .expect("start project");
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    store
        .save_plan(
            &project.project_id,
            &round,
            &PlannerOutput {
                rationale_summary: "two independent routes".into(),
                routes: vec![route("direct", 0.3), route("adversarial", 1.0)],
                assignments: vec![AssignmentDraft {
                    route_index: 0,
                    worker_role: "prover".into(),
                    strategic_role: "local_milestone".into(),
                    addresses_interface_debt: false,
                    goal_ids: vec![],
                    objective: "produce an intermediate lemma".into(),
                    completion_contract: "candidate or explicit gap".into(),
                    priority: 1.0,
                }],
                targeted_uncertainty_ids: vec![],
                suggestion_decisions: vec![],
            },
        )
        .await
        .expect("save plan");
    let node = store
        .register_worker_node(
            "remote-node",
            "Remote test node",
            json!({"roles":["prover"]}),
            "remote-worker-token-123456",
        )
        .await
        .expect("register node");
    let (lease, task) = service
        .lease_next_distributed_task(
            &project.project_id,
            &node.node_id,
            "remote-worker-token-123456",
            node.node_epoch,
            60,
        )
        .await
        .expect("lease")
        .expect("leased task");
    service
        .complete_distributed_task(
            &lease.lease_id,
            &node.node_id,
            "remote-worker-token-123456",
            node.node_epoch,
            lease.lease_epoch,
            &WorkerOutput {
                summary: "remote lemma completed".into(),
                discoveries: vec![],
                candidates: vec![CandidateDraft {
                    statement: "Every even natural number is divisible by two.".into(),
                    assumptions: vec![],
                    proof_markdown: "By definition, an even natural number n has n = 2k for some natural number k, so 2 divides n.".into(),
                    dependency_fact_ids: vec![],
                    definitions_introduced: std::collections::BTreeMap::default(),
                    external_source_ids: vec![],
                    candidate_type: CandidateType::Lemma,
                    target_goal_ids: task.goal_ids.clone(),
                }],
                failures: vec![],
                uncertainties: vec![],
                sources: vec![],
                experiments: vec![],
            },
        )
        .await
        .expect("complete remote task");

    let snapshot = store.snapshot(&project.project_id).await.expect("snapshot");
    assert_eq!(snapshot.facts.len(), 1);
    assert_eq!(snapshot.facts[0].evidence_level, "reviewed");
    assert_eq!(snapshot.facts[0].created_by, task.task_id);
    let verification = store
        .list_verifications(&project.project_id)
        .await
        .expect("verifications")
        .pop()
        .expect("verification");
    assert_eq!(
        verification.report.expect("report").evidence_level,
        "reviewed"
    );
    assert_eq!(
        store
            .get_task_lease(&lease.lease_id)
            .await
            .expect("lease state")
            .status,
        "completed"
    );
}
