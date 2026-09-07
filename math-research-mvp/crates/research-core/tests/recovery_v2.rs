use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use chrono::Utc;
use research_core::{ResearchConfig, ResearchService};
use research_domain::{
    AcceptanceClass, AssignmentDraft, Budget, CandidateSubmission, CandidateType, CommandMode,
    PlannerOutput, ProblemContract, ProjectStatus, RoundStatus, RouteProposal, TaskStatus,
    VerificationProfile, WorkerOutput,
};
use research_storage::{
    CommandDraft, SqliteStore, VerificationCaseDraft, VerificationSnapshotDraft,
};
use research_worker_runtime::{
    AgentBackend, AgentError, AgentHandle, AgentRunResult, AgentSpec, AgentTask,
    BackendCapabilities, MockBackend,
};
use serde_json::json;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct BlockingFirstWorkerBackend {
    first_started: Arc<Notify>,
    release_first: Arc<Notify>,
    run_count: Arc<AtomicUsize>,
}

#[async_trait]
impl AgentBackend for BlockingFirstWorkerBackend {
    fn name(&self) -> &'static str {
        "blocking-first-worker"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            resumable_session: false,
            safe_point_steering: false,
            non_waking_injection: false,
            graceful_cancel: false,
            event_stream: false,
            structured_output: true,
            tool_permissions: false,
        }
    }

    async fn create(&self, spec: AgentSpec) -> Result<AgentHandle, AgentError> {
        Ok(AgentHandle {
            handle_id: "blocking-first-worker-handle".into(),
            project_id: spec.project_id,
            role: spec.role,
            model: spec.model,
            working_directory: spec.working_directory,
            session_id: None,
        })
    }

    async fn run(
        &self,
        _handle: &AgentHandle,
        _task: AgentTask,
        cancellation: CancellationToken,
    ) -> Result<AgentRunResult, AgentError> {
        let run_index = self.run_count.fetch_add(1, Ordering::SeqCst);
        if run_index == 0 {
            self.first_started.notify_one();
            tokio::select! {
                () = self.release_first.notified() => {}
                () = cancellation.cancelled() => return Err(AgentError::Cancelled),
            }
        }
        let now = Utc::now();
        Ok(AgentRunResult {
            structured_output: json!({
                "summary":format!("route worker {} completed", run_index + 1),
                "discoveries":[],
                "candidates":[],
                "failures":[],
                "uncertainties":[],
                "sources":[],
                "experiments":[]
            }),
            session_id: None,
            raw_events: vec![],
            input_tokens: 1,
            output_tokens: 1,
            stderr: String::new(),
            started_at: now,
            completed_at: now,
        })
    }

    async fn resume(
        &self,
        _handle: &AgentHandle,
        _task: AgentTask,
        _cancellation: CancellationToken,
    ) -> Result<AgentRunResult, AgentError> {
        Err(AgentError::Unsupported("resumable session"))
    }
}

fn recovery_plan() -> PlannerOutput {
    let route = |title: &str, method: &str| RouteProposal {
        title: title.into(),
        method_summary: method.into(),
        approach_kind: String::new(),
        route_role: String::new(),
        user_title: String::new(),
        plain_language_summary: String::new(),
        why_this_route: String::new(),
        expected_output: String::new(),
        relation_to_goal: String::new(),
        steps: vec![],
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

fn recovery_plan_scores() -> Vec<f64> {
    recovery_plan()
        .routes
        .iter()
        .map(RouteProposal::score)
        .collect()
}

fn two_route_recovery_plan() -> PlannerOutput {
    let mut plan = recovery_plan();
    plan.assignments.push(AssignmentDraft {
        route_index: 1,
        worker_role: "counterexample_hunter".into(),
        strategic_role: "adversarial".into(),
        addresses_interface_debt: false,
        goal_ids: vec![],
        objective: "search boundary counterexamples".into(),
        completion_contract: "counterexample or exclusion report".into(),
        priority: 0.8,
    });
    plan
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn run_once_resumes_committed_round_from_immutable_contract_without_replanning() {
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
            &recovery_plan_scores(),
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
    assert!(
        history["checkpoints"]
            .as_array()
            .expect("checkpoints")
            .is_empty(),
        "a handshake is already recoverable from its immutable contract and context packet"
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

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn each_approved_route_runs_without_replanning_or_releasing_pending_siblings() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
        .await
        .expect("store");
    let config = ResearchConfig {
        runtime_root: temp.path().join("runtime"),
        output_root: temp.path().join("output"),
        lean_project_root: None,
        planner_timeout_seconds: 5,
        worker_timeout_seconds: 5,
        verifier_timeout_seconds: 5,
        ..ResearchConfig::default()
    };
    let planning_service = ResearchService::new(
        store.clone(),
        Arc::new(MockBackend::from_responses([
            json!({
                "verdict_summary":"The fixed target needs a direct derivation and an independent adversarial check.",
                "fixed_goal":"stable target",
                "proof_skeleton":["derive the stable target from the stated assumptions"],
                "route_portfolio":[
                    {"title":"direct","mechanism":"direct derivation","mathematical_frontier":"stable target","decisive_obstacle":"missing derivation","evidence_for":[],"evidence_against":[],"status":"active","revisit_condition":"revisit after a checked derivation"},
                    {"title":"attack","mechanism":"boundary search","mathematical_frontier":"small cases","decisive_obstacle":"no checked counterexample","evidence_for":[],"evidence_against":[],"status":"active","revisit_condition":"revisit after the direct route changes"}
                ],
                "interface_debts":[
                    {"interface_name":"assumptions-to-target","input_required":"the stated assumptions","output_available":"the target statement","missing_matches":["checked derivation"],"failure_if_ignored":"the target would be asserted without proof","affected_goal_ids":[]}
                ],
                "central_missing_bridge":"A checked derivation of the exact target.",
                "method_vs_proposition_failure":"undetermined",
                "dangerous_shortcuts":["Do not weaken the fixed target."],
                "strategy_directives":["Keep one direct and one adversarial route."],
                "literature_priorities":[],
                "macro_replan_required":false
            }),
            json!({
                "rationale_summary":"two auditable routes",
                "routes":[
                    {"title":"direct","method_summary":"derive the target directly","approach_kind":"direct_proof","route_role":"primary","user_title":"直接证明","plain_language_summary":"从前提直接推到目标。","why_this_route":"它正面处理主目标。","expected_output":"一条可核验的推导。","relation_to_goal":"成功时直接解决目标。","steps":["整理前提","闭合目标"],"target_goal_ids":[],"required_fact_ids":[],"expected_subgoals":[],"expected_goal_progress":0.9,"uncertainty_reduction":0.5,"human_suggestion_alignment":0.0,"evidence_support":0.5,"route_diversity":0.5,"verifiability":0.9,"novelty":0.2,"failure_similarity_penalty":0.0,"cost_penalty":0.1,"risks":[]},
                    {"title":"attack","method_summary":"search boundary counterexamples","approach_kind":"counterexample","route_role":"adversarial","user_title":"寻找反例","plain_language_summary":"检查最小与边界情形。","why_this_route":"它能独立检验命题真假。","expected_output":"一个反例或排除记录。","relation_to_goal":"有效反例将否定目标。","steps":["列举边界情形","逐项核验"],"target_goal_ids":[],"required_fact_ids":[],"expected_subgoals":[],"expected_goal_progress":0.4,"uncertainty_reduction":0.8,"human_suggestion_alignment":0.0,"evidence_support":0.3,"route_diversity":1.0,"verifiability":0.8,"novelty":0.4,"failure_similarity_penalty":0.0,"cost_penalty":0.1,"risks":[]}
                ]
            }),
            json!({
                "summary":"Both routes keep the exact target and have checkable milestones.",
                "reviews":[
                    {"route_index":0,"changes_problem":false,"uses_unverified_claims":false,"conflicts_with_facts":false,"repeats_failure_pattern":false,"has_verifiable_milestone":true,"risk_score":0.1,"goal_closure_leverage":1.0,"generality_gain":1.0,"assumption_debt":0.0,"bridge_centrality":1.0,"architecture_fit":1.0,"unjustified_narrowing":false,"remaining_goal_gaps_if_successful":[],"blockers":[],"suggestions":[]},
                    {"route_index":1,"changes_problem":false,"uses_unverified_claims":false,"conflicts_with_facts":false,"repeats_failure_pattern":false,"has_verifiable_milestone":true,"risk_score":0.1,"goal_closure_leverage":0.5,"generality_gain":0.5,"assumption_debt":0.0,"bridge_centrality":0.4,"architecture_fit":0.6,"unjustified_narrowing":false,"remaining_goal_gaps_if_successful":["A negative search alone would not prove the target."],"blockers":[],"suggestions":[]}
                ]
            }),
            json!({
                "rationale_summary":"Queue both reviewed routes.",
                "assignments":[
                    {"route_index":0,"worker_role":"prover","strategic_role":"central_bridge","addresses_interface_debt":true,"goal_ids":[],"objective":"produce a checked direct derivation","completion_contract":"candidate or precise gap","priority":1.0},
                    {"route_index":1,"worker_role":"counterexample_hunter","strategic_role":"adversarial","addresses_interface_debt":false,"goal_ids":[],"objective":"search and check boundary counterexamples","completion_contract":"counterexample or exclusion report","priority":0.8}
                ],
                "targeted_uncertainty_ids":[],
                "suggestion_decisions":[],
                "deferred_route_indices":[]
            }),
        ])),
        config.clone(),
    );
    let project = planning_service
        .create_project_with_route_approval(
            "approval recovery".into(),
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
            true,
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
                idempotency_key: "start-approval-recovery".into(),
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

    planning_service
        .run_one_round(&project.project_id)
        .await
        .expect("planning pauses cleanly for review");
    let waiting_project = store
        .get_project(&project.project_id)
        .await
        .expect("waiting project");
    assert_eq!(waiting_project.status, ProjectStatus::NeedsHumanReview);
    let rounds = store
        .list_rounds(&project.project_id)
        .await
        .expect("rounds before approval");
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].status, RoundStatus::Running);
    let tasks = store
        .list_tasks(&project.project_id)
        .await
        .expect("tasks before approval");
    assert!(
        !tasks.is_empty(),
        "the reviewed plan must keep its assignments"
    );
    assert!(tasks.iter().all(|task| task.status == TaskStatus::Queued));

    let routes = store
        .list_routes(&project.project_id)
        .await
        .expect("routes");
    let direct_route = routes
        .iter()
        .find(|route| route.title == "direct")
        .expect("direct route");
    let attack_route = routes
        .iter()
        .find(|route| route.title == "attack")
        .expect("attack route");
    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project before direct route approval")
        .revision;
    let (direct_approval, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "approve_route".into(),
                target_kind: "route".into(),
                target_id: direct_route.route_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: revision,
                idempotency_key: "approval-recovery-direct".into(),
                reason: "test direct route approval".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue direct route approval");
    store
        .apply_command(&project.project_id, &direct_approval.command_id)
        .await
        .expect("apply direct route approval");
    assert_eq!(
        store
            .get_project(&project.project_id)
            .await
            .expect("partially approved project")
            .status,
        ProjectStatus::NeedsHumanReview
    );

    let execution_service = ResearchService::new(
        store.clone(),
        Arc::new(MockBackend::from_responses([
            json!({
                "summary":"resumed approved task produced a reviewed intermediate lemma",
                "discoveries":[],
                "candidates":[{
                    "statement":"Every mathematical object is equal to itself.",
                    "assumptions":[],
                    "proof_markdown":"For an arbitrary object x, reflexivity of equality gives x = x.",
                    "dependency_fact_ids":[],
                    "definitions_introduced":{},
                    "external_source_ids":[],
                    "candidate_type":"lemma",
                    "target_goal_ids":[]
                }],
                "failures":[],
                "uncertainties":[],
                "sources":[],
                "experiments":[]
            }),
            json!({
                "verdict":"accepted",
                "summary":"The equality-reflexivity argument is valid.",
                "critical_errors":[],"gaps":[],"uncertainties":[],"repair_actions":[],
                "checked_fact_ids":[],"checked_source_ids":[],
                "evidence_level":"independent_llm_check"
            }),
            json!({
                "verdict":"accepted",
                "summary":"The adversarial audit finds no hidden premise in reflexivity.",
                "critical_errors":[],"gaps":[],"uncertainties":[],"repair_actions":[],
                "checked_fact_ids":[],"checked_source_ids":[],
                "evidence_level":"independent_llm_check"
            }),
            json!({
                "verdict":"rejected",
                "summary":"The reflexivity lemma is valid but does not establish the stable target.",
                "critical_errors":[],
                "gaps":[{"location":"goal coverage","type":"partial_result","issue":"The lemma does not imply the project target."}],
                "uncertainties":[],"repair_actions":[],
                "checked_fact_ids":[],"checked_source_ids":[],
                "evidence_level":"goal_coverage_review"
            }),
            json!({
                "summary":"resumed adversarial task completed its boundary check",
                "discoveries":[],
                "candidates":[],
                "failures":[],
                "uncertainties":[],
                "sources":[],
                "experiments":[]
            }),
        ])),
        config,
    );
    execution_service
        .run_one_round(&project.project_id)
        .await
        .expect("run the first approved route");

    let partially_executed_tasks = store
        .list_tasks(&project.project_id)
        .await
        .expect("tasks after first approval");
    assert_eq!(
        partially_executed_tasks
            .iter()
            .find(|task| task.route_id == direct_route.route_id)
            .expect("direct task")
            .status,
        TaskStatus::Completed
    );
    assert_eq!(
        partially_executed_tasks
            .iter()
            .find(|task| task.route_id == attack_route.route_id)
            .expect("attack task")
            .status,
        TaskStatus::Queued,
        "the pending sibling route must remain frozen"
    );
    assert_eq!(
        store
            .list_rounds(&project.project_id)
            .await
            .expect("round after first approval")[0]
            .status,
        RoundStatus::Running,
        "partial route release must not complete the committed round"
    );
    assert_eq!(
        store
            .list_plan_revisions(&project.project_id)
            .await
            .expect("plan revisions after first route")
            .len(),
        1
    );
    let partial_snapshot = store
        .snapshot(&project.project_id)
        .await
        .expect("snapshot after approved route verification");
    assert!(
        partial_snapshot.facts.iter().any(|fact| fact.created_by
            == partially_executed_tasks
                .iter()
                .find(|task| task.route_id == direct_route.route_id)
                .expect("direct task")
                .task_id),
        "an approved route must complete Worker -> independent verification -> Fact Gate while sibling review is pending"
    );

    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project before attack route approval")
        .revision;
    let (attack_approval, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "approve_route".into(),
                target_kind: "route".into(),
                target_id: attack_route.route_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: revision,
                idempotency_key: "approval-recovery-attack".into(),
                reason: "test attack route approval".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue attack route approval");
    store
        .apply_command(&project.project_id, &attack_approval.command_id)
        .await
        .expect("apply attack route approval");
    assert_eq!(
        store
            .get_project(&project.project_id)
            .await
            .expect("fully reviewed project")
            .status,
        ProjectStatus::Running
    );
    let released_after_final_approval = store
        .list_released_queued_tasks(&project.project_id, rounds[0].number)
        .await
        .expect("released task after final approval");
    assert_eq!(
        released_after_final_approval
            .iter()
            .map(|task| task.task_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            partially_executed_tasks
                .iter()
                .find(|task| task.route_id == attack_route.route_id)
                .expect("attack task")
                .task_id
                .as_str()
        ]
    );
    execution_service
        .run_one_round(&project.project_id)
        .await
        .expect("run the second approved route");

    let completed_rounds = store
        .list_rounds(&project.project_id)
        .await
        .expect("rounds after approval");
    let final_tasks = store
        .list_tasks(&project.project_id)
        .await
        .expect("tasks after approval");
    assert_eq!(
        completed_rounds.len(),
        1,
        "route-by-route approval must not create a replacement planning round"
    );
    assert_eq!(
        completed_rounds[0].status,
        RoundStatus::Completed,
        "final task states: {:?}",
        final_tasks
            .iter()
            .map(|task| (&task.route_id, task.status))
            .collect::<Vec<_>>()
    );
    assert!(
        final_tasks
            .iter()
            .all(|task| task.status == TaskStatus::Completed)
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn approval_while_another_route_is_running_coalesces_a_follow_up_run() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
        .await
        .expect("store");
    let backend = BlockingFirstWorkerBackend::default();
    let service = ResearchService::new(
        store.clone(),
        Arc::new(backend.clone()),
        ResearchConfig {
            runtime_root: temp.path().join("runtime"),
            output_root: temp.path().join("output"),
            lean_project_root: None,
            worker_timeout_seconds: 5,
            ..ResearchConfig::default()
        },
    );
    let project = service
        .create_project_with_route_approval(
            "coalesced route approval".into(),
            ProblemContract {
                original_problem: "Prove a stable target".into(),
                target_statement: "stable target".into(),
                assumptions: vec![],
                success_criteria: "accepted".into(),
                version: 1,
            },
            Budget {
                max_rounds: 1,
                max_parallel_workers: 1,
                max_total_model_calls: 20,
                ..Budget::default()
            },
            true,
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
                idempotency_key: "start-coalesced-approval".into(),
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
    let plan = two_route_recovery_plan();
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &plan,
            &plan
                .routes
                .iter()
                .map(RouteProposal::score)
                .collect::<Vec<_>>(),
            &delta,
            "coalesced approval fixture",
            None,
        )
        .await
        .expect("saved plan");
    assert_eq!(saved.tasks.len(), 2);
    let direct_route = saved
        .routes
        .iter()
        .find(|route| route.title == "direct")
        .expect("direct route");
    let attack_route = saved
        .routes
        .iter()
        .find(|route| route.title == "attack")
        .expect("attack route");

    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("revision before direct approval")
        .revision;
    let (approve_direct, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "approve_route".into(),
                target_kind: "route".into(),
                target_id: direct_route.route_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: revision,
                idempotency_key: "coalesced-approve-direct".into(),
                reason: "release first route".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue direct approval");
    service
        .dispatch_command(&project.project_id, &approve_direct.command_id)
        .await
        .expect("dispatch direct approval");
    tokio::time::timeout(Duration::from_secs(2), backend.first_started.notified())
        .await
        .expect("first route worker did not start");

    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("revision before second approval")
        .revision;
    let (approve_attack, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "approve_route".into(),
                target_kind: "route".into(),
                target_id: attack_route.route_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: revision,
                idempotency_key: "coalesced-approve-attack".into(),
                reason: "release second route while first is running".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue attack approval");
    service
        .dispatch_command(&project.project_id, &approve_attack.command_id)
        .await
        .expect("dispatch attack approval");
    assert_eq!(
        store
            .get_task(
                &project.project_id,
                &saved
                    .tasks
                    .iter()
                    .find(|task| task.route_id == attack_route.route_id)
                    .expect("attack task")
                    .task_id,
            )
            .await
            .expect("attack task while first route is blocked")
            .status,
        TaskStatus::Queued,
        "the second approval must queue a follow-up runner, not bypass the project lock"
    );
    backend.release_first.notify_one();

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let tasks = store
                .list_tasks(&project.project_id)
                .await
                .expect("poll tasks");
            let rounds = store
                .list_rounds(&project.project_id)
                .await
                .expect("poll rounds");
            if tasks
                .iter()
                .all(|task| task.status == TaskStatus::Completed)
                && rounds
                    .first()
                    .is_some_and(|round| round.status == RoundStatus::Completed)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("coalesced follow-up run left an approved task queued");
    assert_eq!(backend.run_count.load(Ordering::SeqCst), 2);
    assert_eq!(
        store
            .list_plan_revisions(&project.project_id)
            .await
            .expect("plan revisions")
            .len(),
        1,
        "the follow-up runner must resume the committed round instead of replanning"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn startup_recovery_schedules_released_verification_without_an_active_task() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
        .await
        .expect("store");
    let config = ResearchConfig {
        runtime_root: temp.path().join("runtime"),
        output_root: temp.path().join("output"),
        lean_project_root: None,
        verifier_timeout_seconds: 5,
        ..ResearchConfig::default()
    };
    let setup_service = ResearchService::new(
        store.clone(),
        Arc::new(MockBackend::default()),
        config.clone(),
    );
    let project = setup_service
        .create_project_with_route_approval(
            "verification-only restart recovery".into(),
            ProblemContract {
                original_problem: "Check a self-contained intermediate lemma".into(),
                target_statement: "A stable target".into(),
                assumptions: vec![],
                success_criteria: "reviewed result".into(),
                version: 1,
            },
            Budget {
                max_rounds: 2,
                max_parallel_workers: 1,
                ..Budget::default()
            },
            true,
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
                idempotency_key: "start-verification-only-recovery".into(),
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
    let plan = two_route_recovery_plan();
    let scores = plan
        .routes
        .iter()
        .map(RouteProposal::score)
        .collect::<Vec<_>>();
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &plan,
            &scores,
            &delta,
            "primary",
            None,
        )
        .await
        .expect("persisted approval-gated plan");
    let direct_route = saved
        .routes
        .iter()
        .find(|route| route.title == "direct")
        .expect("direct route");
    let attack_route = saved
        .routes
        .iter()
        .find(|route| route.title == "attack")
        .expect("attack route");
    let direct_task = saved
        .tasks
        .iter()
        .find(|task| task.route_id == direct_route.route_id)
        .expect("direct task");
    let attack_task_id = saved
        .tasks
        .iter()
        .find(|task| task.route_id == attack_route.route_id)
        .expect("attack task")
        .task_id
        .clone();

    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project before route approval")
        .revision;
    let (approval, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "approve_route".into(),
                target_kind: "route".into(),
                target_id: direct_route.route_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: revision,
                idempotency_key: "approve-verification-only-route".into(),
                reason: "test".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue route approval");
    store
        .apply_command(&project.project_id, &approval.command_id)
        .await
        .expect("approve direct route");

    sqlx::query(
        "UPDATE tasks SET status='completed',result_summary='worker completed before restart' WHERE task_id=?",
    )
    .bind(&direct_task.task_id)
    .execute(store.pool())
    .await
    .expect("complete direct task fixture");
    let completed_task = store
        .get_task(&project.project_id, &direct_task.task_id)
        .await
        .expect("completed direct task");
    let receipt = store
        .submit_candidate(
            &project.project_id,
            CandidateSubmission {
                task_id: completed_task.task_id.clone(),
                route_id: completed_task.route_id.clone(),
                target_goal_ids: vec![],
                statement: "Every mathematical object is equal to itself.".into(),
                assumptions: vec![],
                proof_markdown: "For arbitrary x, reflexivity of equality gives x = x.".into(),
                dependency_fact_ids: vec![],
                definitions_introduced: std::collections::BTreeMap::default(),
                external_source_ids: vec![],
                candidate_type: CandidateType::Lemma,
                task_revision: completed_task.revision,
                route_cancellation_epoch: completed_task.route_cancellation_epoch,
            },
            "verification-only-restart-candidate",
        )
        .await
        .expect("submitted verification fixture");
    assert_eq!(
        store
            .get_project(&project.project_id)
            .await
            .expect("partially reviewed project")
            .status,
        ProjectStatus::NeedsHumanReview
    );
    assert_eq!(
        store
            .get_task(&project.project_id, &attack_task_id)
            .await
            .expect("pending sibling task")
            .status,
        TaskStatus::Queued
    );

    let accepted_review = || {
        json!({
            "verdict":"accepted",
            "summary":"The self-contained reflexivity argument is valid.",
            "critical_errors":[],"gaps":[],"uncertainties":[],"repair_actions":[],
            "checked_fact_ids":[],"checked_source_ids":[],
            "evidence_level":"independent_llm_check"
        })
    };
    let restarted = ResearchService::new(
        store.clone(),
        Arc::new(MockBackend::from_responses([
            accepted_review(),
            accepted_review(),
        ])),
        config,
    );
    assert_eq!(
        restarted
            .recover_running_projects()
            .await
            .expect("restart recovery scan"),
        1,
        "an approved route's submitted verification must make the review-paused project recoverable"
    );

    let verification = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let verification = store
                .get_verification(&receipt.verification.verification_id)
                .await
                .expect("verification during recovery");
            if !matches!(
                verification.status,
                research_domain::CandidateStatus::Submitted
                    | research_domain::CandidateStatus::Prechecking
                    | research_domain::CandidateStatus::Verifying
            ) {
                break verification;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("verification recovery timed out");
    assert_eq!(
        verification.status,
        research_domain::CandidateStatus::Accepted
    );
    assert_eq!(
        store
            .get_task(&project.project_id, &attack_task_id)
            .await
            .expect("pending sibling after verification recovery")
            .status,
        TaskStatus::Queued,
        "restarting an approved route's verification must not release its pending sibling"
    );
    assert_eq!(
        store
            .get_project(&project.project_id)
            .await
            .expect("project after verification recovery")
            .status,
        ProjectStatus::NeedsHumanReview
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn startup_replays_submitted_reviewer_result_without_duplicate_review() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
        .await
        .expect("store");
    let backend = MockBackend::from_responses([
        json!({
            "verdict":"accepted","summary":"second reviewer independently checks the arithmetic step",
            "critical_errors":[],"gaps":[],"uncertainties":[],"repair_actions":[],
            "checked_fact_ids":[],"checked_source_ids":[],"evidence_level":"independent_llm_check"
        }),
        json!({
            "verdict":"accepted","summary":"adversarial reviewer finds no hidden assumption",
            "critical_errors":[],"gaps":[],"uncertainties":[],"repair_actions":[],
            "checked_fact_ids":[],"checked_source_ids":[],"evidence_level":"independent_llm_check"
        }),
    ]);
    let service = ResearchService::new(
        store.clone(),
        Arc::new(backend),
        ResearchConfig {
            runtime_root: temp.path().join("runtime"),
            output_root: temp.path().join("output"),
            verifier_timeout_seconds: 5,
            ..ResearchConfig::default()
        },
    );
    let project = service
        .create_project(
            "verification crash recovery".into(),
            ProblemContract {
                original_problem: "Check 1+1=2".into(),
                target_statement: "1+1=2".into(),
                assumptions: vec![],
                success_criteria: "reviewed result".into(),
                version: 1,
            },
            Budget {
                max_rounds: 1,
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
                idempotency_key: "start-verification-recovery".into(),
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
    let saved = store
        .save_plan(&project.project_id, &round, &recovery_plan())
        .await
        .expect("plan");
    let (running_task, _) = store
        .mark_task_running(&project.project_id, &saved.tasks[0].task_id)
        .await
        .expect("running task");
    store
        .record_worker_output(
            &running_task,
            &WorkerOutput {
                summary: "candidate ready".into(),
                discoveries: vec![],
                candidates: vec![],
                failures: vec![],
                uncertainties: vec![],
                sources: vec![],
                experiments: vec![],
            },
        )
        .await
        .expect("worker output");
    let task = store
        .get_task(&project.project_id, &running_task.task_id)
        .await
        .expect("task");
    let receipt = store
        .submit_candidate(
            &project.project_id,
            CandidateSubmission {
                task_id: task.task_id.clone(),
                route_id: task.route_id.clone(),
                target_goal_ids: vec![],
                statement: "1+1=2".into(),
                assumptions: vec![],
                proof_markdown: "By the recursive definition of natural-number addition.".into(),
                dependency_fact_ids: vec![],
                definitions_introduced: std::collections::BTreeMap::default(),
                external_source_ids: vec![],
                candidate_type: CandidateType::Lemma,
                task_revision: task.revision,
                route_cancellation_epoch: task.route_cancellation_epoch,
            },
            "verification-recovery-candidate",
        )
        .await
        .expect("candidate");
    store
        .mark_verification_started(&receipt.verification.verification_id)
        .await
        .expect("verification started");
    let (case, _) = store
        .ensure_verification_case(
            &receipt.verification.verification_id,
            VerificationCaseDraft {
                name: "recovery-review".into(),
                profile: VerificationProfile::StandardReview,
                required_acceptance: AcceptanceClass::Reviewed,
                required_checks: vec![
                    "deterministic_precheck".into(),
                    "math_review_1".into(),
                    "math_review_2".into(),
                    "reviewer_independence".into(),
                    "adversarial_review".into(),
                ],
                independent_reviewer_count: 2,
                require_citation_review: false,
                require_adversarial_review: true,
                require_alignment_review: false,
                require_fresh_replay: false,
                max_attempts: 3,
                risk_score: 0.2,
                risk_reasons: vec![],
            },
        )
        .await
        .expect("verification case");
    store
        .create_verification_snapshot(
            &case.case_id,
            VerificationSnapshotDraft {
                toolchain_hash: None,
                extra_payload: json!({"test":"crash_after_result_submit"}),
            },
        )
        .await
        .expect("verification snapshot");
    let verification_context = json!({
        "problem_contract":project.contract,
        "candidate":receipt.candidate.submission,
        "active_dependency_facts":[],
        "source_snapshot":[],
        "target_goals":[],
        "reviewer_kind":"math_review_1",
        "reviewer_role_contract":"执行正向证明审计：按候选证明的原顺序建立逐步义务表，逐项核对量词、假设、定义域、推理方向和依赖 Fact；明确指出第一个不能从前提推出的步骤。只审查候选自身，目标覆盖由单独的 goal_coverage_review 裁决。",
        "reviewer_isolation":"No other reviewer result is present in this packet.",
        "trust_rule":"Only active_dependency_facts are mathematical premises; sources remain evidence requiring citation review.",
    });
    let offer = store
        .offer_verification_worker(
            &case.case_id,
            "math_review_1",
            "mock",
            None,
            &temp.path().join("interrupted-review").to_string_lossy(),
            &verification_context,
            &[],
            &json!({"success":"one schema-valid report"}),
        )
        .await
        .expect("review offer");
    let lease = store
        .accept_verification_worker_handshake(&offer, Some("mock-1"), &json!({}), 3_600)
        .await
        .expect("review lease");
    let recovered_report = json!({
        "verdict":"accepted","summary":"first reviewer checked the direct arithmetic proof before the crash",
        "critical_errors":[],"gaps":[],"uncertainties":[],"repair_actions":[],
        "checked_fact_ids":[],"checked_source_ids":[],"evidence_level":"independent_llm_check"
    });
    let (envelope_id, _) = store
        .submit_verification_result(&lease, "completed", &recovered_report)
        .await
        .expect("durable submitted result");

    service
        .run_project_until_terminal(&project.project_id)
        .await
        .expect("startup recovery");

    let first_reviewer_attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM verification_attempts WHERE case_id=? AND kind='math_review_1'",
    )
    .bind(&case.case_id)
    .fetch_one(store.pool())
    .await
    .expect("first reviewer attempt count");
    assert_eq!(
        first_reviewer_attempts, 1,
        "startup must replay the submitted review instead of calling that reviewer again"
    );
    let envelope_status: String = sqlx::query_scalar(
        "SELECT status FROM verification_result_envelopes WHERE verification_result_envelope_id=?",
    )
    .bind(envelope_id)
    .fetch_one(store.pool())
    .await
    .expect("envelope status");
    assert_eq!(envelope_status, "ingested");
}
