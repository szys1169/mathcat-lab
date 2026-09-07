use std::sync::Arc;

use research_core::{CoreError, ResearchConfig, ResearchService};
use research_domain::{Budget, CommandMode, ProblemContract, ProjectStatus, RoundStatus};
use research_storage::{CommandDraft, SqliteStore};
use research_worker_runtime::MockBackend;
use serde_json::json;

fn planning_and_worker_responses() -> Vec<serde_json::Value> {
    vec![
        json!({
            "verdict_summary":"The fixed target remains open.",
            "fixed_goal":"P",
            "proof_skeleton":["Establish one exact bridge to P."],
            "route_portfolio":[{
                "title":"direct","mechanism":"deduction","mathematical_frontier":"P",
                "decisive_obstacle":"missing bridge","evidence_for":[],"evidence_against":[],
                "status":"active","revisit_condition":"after a checked bridge"
            }],
            "interface_debts":[{
                "interface_name":"bridge","input_required":"assumptions","output_available":"target P",
                "missing_matches":["checked implication"],"failure_if_ignored":"P would be asserted",
                "affected_goal_ids":[]
            }],
            "central_missing_bridge":"A checked implication to P.",
            "method_vs_proposition_failure":"undetermined",
            "dangerous_shortcuts":[],"strategy_directives":[],"literature_priorities":[],
            "macro_replan_required":false
        }),
        json!({
            "rationale_summary":"Use direct and adversarial routes.",
            "routes":[
                {
                    "title":"direct","method_summary":"derive the exact target","approach_kind":"direct_proof",
                    "route_role":"primary","user_title":"Direct proof","plain_language_summary":"Derive P.",
                    "why_this_route":"It addresses P.","expected_output":"A checked proof or gap.",
                    "relation_to_goal":"A proof closes P.","steps":["derive P"],"target_goal_ids":[],
                    "required_fact_ids":[],"expected_subgoals":[],"expected_goal_progress":0.8,
                    "uncertainty_reduction":0.5,"human_suggestion_alignment":0.0,"evidence_support":0.5,
                    "route_diversity":0.8,"verifiability":0.8,"novelty":0.2,
                    "failure_similarity_penalty":0.0,"cost_penalty":0.1,"risks":[]
                },
                {
                    "title":"counterexample attack","method_summary":"search boundary counterexamples",
                    "approach_kind":"counterexample","route_role":"adversarial","user_title":"Counterexample search",
                    "plain_language_summary":"Attack P.","why_this_route":"It can refute P.",
                    "expected_output":"A checked counterexample or exclusion log.","relation_to_goal":"A counterexample refutes P.",
                    "steps":["search boundaries"],"target_goal_ids":[],"required_fact_ids":[],
                    "expected_subgoals":[],"expected_goal_progress":0.4,"uncertainty_reduction":0.8,
                    "human_suggestion_alignment":0.0,"evidence_support":0.3,"route_diversity":1.0,
                    "verifiability":0.8,"novelty":0.3,"failure_similarity_penalty":0.0,
                    "cost_penalty":0.1,"risks":[]
                }
            ]
        }),
        json!({
            "summary":"Both routes preserve the fixed target.",
            "reviews":[
                {
                    "route_index":0,"changes_problem":false,"uses_unverified_claims":false,
                    "conflicts_with_facts":false,"repeats_failure_pattern":false,
                    "has_verifiable_milestone":true,"risk_score":0.1,"goal_closure_leverage":0.8,
                    "generality_gain":0.5,"assumption_debt":0.8,"bridge_centrality":0.8,
                    "architecture_fit":0.8,"unjustified_narrowing":false,
                    "remaining_goal_gaps_if_successful":[],
                    "blockers":["The bridge is an unproved research objective, not an admitted premise."],
                    "suggestions":["Prove or refute the bridge before using it."]
                },
                {
                    "route_index":1,"changes_problem":false,"uses_unverified_claims":false,
                    "conflicts_with_facts":false,"repeats_failure_pattern":false,
                    "has_verifiable_milestone":true,"risk_score":0.1,"goal_closure_leverage":0.5,
                    "generality_gain":0.5,"assumption_debt":0.0,"bridge_centrality":0.4,
                    "architecture_fit":0.7,"unjustified_narrowing":false,
                    "remaining_goal_gaps_if_successful":["A negative search does not prove P."],
                    "blockers":[],"suggestions":[]
                }
            ]
        }),
        json!({
            "rationale_summary":"Execute both bounded routes.",
            "assignments":[
                {
                    "route_index":0,"worker_role":"prover","strategic_role":"central_bridge",
                    "addresses_interface_debt":true,"goal_ids":[],"objective":"derive P",
                    "completion_contract":"return a proof candidate or precise gap","priority":1.0
                },
                {
                    "route_index":1,"worker_role":"counterexample_hunter","strategic_role":"adversarial",
                    "addresses_interface_debt":false,"goal_ids":[],"objective":"attack P",
                    "completion_contract":"return a counterexample or exclusion log","priority":0.8
                }
            ],
            "targeted_uncertainty_ids":[],"suggestion_decisions":[],"deferred_route_indices":[]
        }),
        json!({
            "summary":"The direct route produced only a bounded partial analysis.",
            "discoveries":[],"candidates":[],"failures":[],"uncertainties":[],
            "sources":[],"experiments":[]
        }),
        json!({
            "summary":"The adversarial route found no certified counterexample.",
            "discoveries":[],"candidates":[],"failures":[],"uncertainties":[],
            "sources":[],"experiments":[]
        }),
    ]
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn final_report_uses_terminal_state_and_recovers_projection_without_duplicates() {
    let temp = tempfile::tempdir().expect("tempdir");
    let artifact_root = temp.path().join("artifacts");
    let output_root = temp.path().join("output");
    let store = SqliteStore::connect("sqlite::memory:", &artifact_root)
        .await
        .expect("store");
    let config = ResearchConfig {
        runtime_root: temp.path().join("runtime"),
        output_root: output_root.clone(),
        planner_timeout_seconds: 5,
        worker_timeout_seconds: 5,
        verifier_timeout_seconds: 5,
        ..ResearchConfig::default()
    };
    let service = ResearchService::new(
        store.clone(),
        Arc::new(MockBackend::from_responses(planning_and_worker_responses())),
        config.clone(),
    );
    let project = service
        .create_project(
            "terminal reporting".into(),
            ProblemContract {
                original_problem: "Prove P".into(),
                target_statement: "P".into(),
                assumptions: vec![],
                success_criteria: "A certified proof or counterexample".into(),
                version: 1,
            },
            Budget {
                max_rounds: 1,
                max_parallel_workers: 1,
                ..Budget::default()
            },
        )
        .await
        .expect("create project");
    let current = store
        .get_project(&project.project_id)
        .await
        .expect("project before start");
    let (start, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "start_project".into(),
                target_kind: "project".into(),
                target_id: project.project_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: current.revision,
                idempotency_key: "terminal-report-start".into(),
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

    tokio::fs::remove_dir_all(&output_root)
        .await
        .expect("remove initial disposable projection");
    tokio::fs::write(&output_root, b"block directory creation")
        .await
        .expect("block report projection");
    assert!(matches!(
        service.run_one_round(&project.project_id).await,
        Err(CoreError::Io(_))
    ));

    let terminal = store
        .snapshot(&project.project_id)
        .await
        .expect("terminal state survived projection failure");
    assert_eq!(terminal.project.status, ProjectStatus::PartialSuccess);
    assert_eq!(
        terminal.tasks.len(),
        2,
        "open research debt must not veto execution"
    );
    assert_eq!(
        terminal.current_round.as_ref().expect("round").status,
        RoundStatus::Completed
    );
    let report_artifacts_before = store
        .list_artifacts(&project.project_id)
        .await
        .expect("report artifacts")
        .into_iter()
        .filter(|artifact| {
            artifact.created_in_round == 1
                && matches!(artifact.kind.as_str(), "round_report" | "latest_report")
        })
        .collect::<Vec<_>>();
    assert_eq!(report_artifacts_before.len(), 2);
    assert_eq!(
        report_artifacts_before[0].sha256,
        report_artifacts_before[1].sha256
    );

    let recovered_output_root = temp.path().join("recovered-output");
    let recovered = ResearchService::new(
        store.clone(),
        Arc::new(MockBackend::default()),
        ResearchConfig {
            output_root: recovered_output_root.clone(),
            ..config
        },
    );
    recovered
        .run_one_round(&project.project_id)
        .await
        .expect("terminal recovery rebuilds report projection");

    let latest = tokio::fs::read_to_string(
        recovered_output_root
            .join(&project.project_id)
            .join("LATEST.md"),
    )
    .await
    .expect("recovered LATEST");
    assert!(latest.contains("- 状态：`partial_success`"));
    assert!(latest.contains("不保证已取得数学进展"));
    assert!(!latest.contains("尚无研究任务"));
    assert!(!latest.contains("- 状态：`running`"));
    let report_artifacts_after = store
        .list_artifacts(&project.project_id)
        .await
        .expect("report artifacts after recovery")
        .into_iter()
        .filter(|artifact| {
            artifact.created_in_round == 1
                && matches!(artifact.kind.as_str(), "round_report" | "latest_report")
        })
        .collect::<Vec<_>>();
    assert_eq!(report_artifacts_after.len(), 2);
}

#[tokio::test]
async fn rejected_premises_still_stop_before_workers_and_report_no_research_progress() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
        .await
        .expect("store");
    let mut responses = planning_and_worker_responses();
    for review in responses[2]["reviews"].as_array_mut().expect("reviews") {
        review["uses_unverified_claims"] = json!(true);
        review["blockers"] = json!(["method_summary treats an unproved bridge as established."]);
    }
    responses.truncate(3);
    let output_root = temp.path().join("output");
    let service = ResearchService::new(
        store.clone(),
        Arc::new(MockBackend::from_responses(responses)),
        ResearchConfig {
            runtime_root: temp.path().join("runtime"),
            output_root: output_root.clone(),
            planner_timeout_seconds: 5,
            ..ResearchConfig::default()
        },
    );
    let project = service
        .create_project(
            "rejected premises".into(),
            ProblemContract {
                original_problem: "Prove P".into(),
                target_statement: "P".into(),
                assumptions: vec![],
                success_criteria: "A certified proof or counterexample".into(),
                version: 1,
            },
            Budget {
                max_rounds: 1,
                ..Budget::default()
            },
        )
        .await
        .expect("project");
    let current = store
        .get_project(&project.project_id)
        .await
        .expect("current");
    let (command, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "start_project".into(),
                target_kind: "project".into(),
                target_id: project.project_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: current.revision,
                idempotency_key: "rejected-premises-start".into(),
                reason: "test".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue start");
    store
        .apply_command(&project.project_id, &command.command_id)
        .await
        .expect("start");
    service
        .run_one_round(&project.project_id)
        .await
        .expect("round");
    let snapshot = store.snapshot(&project.project_id).await.expect("snapshot");
    assert_eq!(snapshot.project.status, ProjectStatus::PartialSuccess);
    assert!(snapshot.tasks.is_empty());
    assert!(snapshot.facts.is_empty());
    let report = tokio::fs::read_to_string(output_root.join(&project.project_id).join("LATEST.md"))
        .await
        .expect("report");
    assert!(report.contains("no policy-eligible route remains after reflection"));
    assert!(report.contains("不保证已取得数学进展"));
    assert!(report.contains("尚无研究任务"));
    assert!(report.contains("- 主目标完成：否"));
}
