#![allow(clippy::too_many_lines)]

use std::sync::Arc;

use research_core::{ResearchConfig, ResearchService};
use research_domain::{
    Budget, CheckStatus, CommandMode, ProblemContract, ProjectStatus, ProofSearchBudget,
};
use research_storage::{CommandDraft, SqliteStore};
use research_worker_runtime::{
    BackendOutcome, MockBackend, MockInteractiveProofBackend, MockVerificationBackend, ProofGoal,
    ProofState,
};
use serde_json::json;

#[tokio::test]
async fn accepted_candidate_flows_through_independent_verification_and_solves_goal() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
        .await
        .expect("store");
    let backend = MockBackend::from_responses([
        json!({
            "verdict_summary":"The fixed arithmetic goal is open and has a direct one-step proof architecture.",
            "fixed_goal":"Prove that 1+1=2",
            "proof_skeleton":["Establish the recursive computation 1+1=2 from the definition of addition."],
            "route_portfolio":[
                {"title":"direct","mechanism":"recursive definition","mathematical_frontier":"target equality","decisive_obstacle":"none beyond a checked derivation","evidence_for":[],"evidence_against":[],"status":"active","revisit_condition":"revisit only if the direct derivation is rejected"}
            ],
            "interface_debts":[
                {"interface_name":"definition-to-target","input_required":"the recursive definition of natural-number addition","output_available":"the problem statement","missing_matches":["checked derivation"],"failure_if_ignored":"the equality would be asserted without proof","affected_goal_ids":[]}
            ],
            "central_missing_bridge":"A checked recursive derivation of 1+1=2.",
            "method_vs_proposition_failure":"undetermined",
            "dangerous_shortcuts":["Do not treat the easier equality 0+1=1 as coverage of 1+1=2."],
            "strategy_directives":["Prioritize the exact target and retain an adversarial check."],
            "literature_priorities":[],
            "macro_replan_required":false
        }),
        json!({
            "rationale_summary":"two independent routes",
            "routes":[
                {"title":"direct","method_summary":"prove directly","approach_kind":"direct_proof","route_role":"primary","user_title":"直接证明：从定义出发","plain_language_summary":"从定义逐步推出目标。","why_this_route":"先测试最短的证明链。","expected_output":"一条可独立核验的证明。","relation_to_goal":"成功即可直接证明主目标。","steps":["展开定义","闭合目标"],"target_goal_ids":[],"required_fact_ids":[],"expected_subgoals":[],"expected_goal_progress":0.9,"uncertainty_reduction":0.4,"human_suggestion_alignment":0.0,"evidence_support":0.5,"route_diversity":0.5,"verifiability":0.9,"novelty":0.2,"failure_similarity_penalty":0.0,"cost_penalty":0.1,"risks":[]},
                {"title":"attack","method_summary":"look for counterexamples","approach_kind":"counterexample","route_role":"adversarial","user_title":"寻找反例：检查边界对象","plain_language_summary":"系统检查最小与边界对象。","why_this_route":"可以尽早发现命题是否错误。","expected_output":"经过核验的反例或排除记录。","relation_to_goal":"有效反例将直接否定主目标。","steps":["生成候选","逐项核验"],"target_goal_ids":[],"required_fact_ids":[],"expected_subgoals":[],"expected_goal_progress":0.4,"uncertainty_reduction":0.8,"human_suggestion_alignment":0.0,"evidence_support":0.3,"route_diversity":1.0,"verifiability":0.8,"novelty":0.4,"failure_similarity_penalty":0.0,"cost_penalty":0.1,"risks":[]}
            ],
            "assignments":[
                {"route_index":0,"worker_role":"prover","strategic_role":"central_bridge","addresses_interface_debt":true,"goal_ids":[],"objective":"prove","completion_contract":"candidate or gap","priority":1.0},
                {"route_index":1,"worker_role":"counterexample_hunter","strategic_role":"adversarial","addresses_interface_debt":false,"goal_ids":[],"objective":"attack","completion_contract":"counterexample or report","priority":0.8}
            ],
            "targeted_uncertainty_ids":[],"suggestion_decisions":[]
        }),
        json!({
            "summary":"Both routes preserve the target; the attack route is the independent adversarial alternative.",
            "reviews":[
                {"route_index":0,"changes_problem":false,"uses_unverified_claims":false,"conflicts_with_facts":false,"repeats_failure_pattern":false,"has_verifiable_milestone":true,"risk_score":0.1,"goal_closure_leverage":1.0,"generality_gain":1.0,"assumption_debt":0.0,"bridge_centrality":1.0,"architecture_fit":1.0,"unjustified_narrowing":false,"remaining_goal_gaps_if_successful":[],"blockers":[],"suggestions":[]},
                {"route_index":1,"changes_problem":false,"uses_unverified_claims":false,"conflicts_with_facts":false,"repeats_failure_pattern":false,"has_verifiable_milestone":true,"risk_score":0.1,"goal_closure_leverage":0.5,"generality_gain":0.5,"assumption_debt":0.0,"bridge_centrality":0.4,"architecture_fit":0.6,"unjustified_narrowing":false,"remaining_goal_gaps_if_successful":["A negative search would not prove the universal target."],"blockers":[],"suggestions":[]}
            ]
        }),
        json!({
            "rationale_summary":"Advance the direct route and retain an adversarial task.",
            "assignments":[
                {"route_index":0,"worker_role":"prover","strategic_role":"central_bridge","addresses_interface_debt":true,"goal_ids":[],"objective":"prove","completion_contract":"candidate or gap","priority":1.0},
                {"route_index":1,"worker_role":"counterexample_hunter","strategic_role":"adversarial","addresses_interface_debt":false,"goal_ids":[],"objective":"attack","completion_contract":"counterexample or report","priority":0.8}
            ],
            "targeted_uncertainty_ids":[],"suggestion_decisions":[],"deferred_route_indices":[]
        }),
        json!({
            "summary":"proved by the Peano successor definition","discoveries":[],"failures":[],"uncertainties":[],
            "candidates":[
                {"statement":"0+1=1","assumptions":[],"proof_markdown":"By the base clause for addition, 0+n=n, hence 0+1=1.","dependency_fact_ids":[],"definitions_introduced":{},"external_source_ids":[],"candidate_type":"lemma","target_goal_ids":[]},
                {"statement":"1+1=2","assumptions":[],"proof_markdown":"By the recursive definition of addition, 1+S(0)=S(1+0)=S(1)=2.","dependency_fact_ids":[],"definitions_introduced":{},"external_source_ids":[],"candidate_type":"theorem","target_goal_ids":[]}
            ]
        }),
        // Both worker outputs must precede reviewer responses: round verification
        // now starts only after the concurrent worker batch reaches a safe point.
        json!({"summary":"no counterexample in the intended arithmetic structure","discoveries":[],"candidates":[],"failures":[],"uncertainties":[],"sources":[],"experiments":[]}),
        json!({"verdict":"accepted","summary":"First independent mathematical review accepts the base-clause computation.","critical_errors":[],"gaps":[],"uncertainties":[],"repair_actions":[],"checked_fact_ids":[],"checked_source_ids":[],"evidence_level":"independent_llm_check"}),
        json!({"verdict":"accepted","summary":"Adversarial review finds no hidden assumption in the base-clause computation.","critical_errors":[],"gaps":[],"uncertainties":[],"repair_actions":[],"checked_fact_ids":[],"checked_source_ids":[],"evidence_level":"independent_llm_check"}),
        json!({"verdict":"rejected","summary":"The base-clause lemma is valid but does not imply the target 1+1=2.","critical_errors":[],"gaps":[{"location":"goal coverage","type":"partial_result","issue":"Only an intermediate lemma is proved."}],"uncertainties":[],"repair_actions":["Use the lemma in a separate closure candidate for the original goal."],"checked_fact_ids":[],"checked_source_ids":[],"evidence_level":"goal_coverage_review"}),
        json!({"verdict":"accepted","summary":"First independent mathematical review accepts the recursive computation.","critical_errors":[],"gaps":[],"uncertainties":[],"repair_actions":[],"checked_fact_ids":[],"checked_source_ids":[],"evidence_level":"independent_llm_check"}),
        json!({"verdict":"accepted","summary":"Second independent mathematical review accepts the recursive computation.","critical_errors":[],"gaps":[],"uncertainties":[],"repair_actions":[],"checked_fact_ids":[],"checked_source_ids":[],"evidence_level":"independent_llm_check"}),
        json!({"verdict":"accepted","summary":"Adversarial review finds no counterexample to the recursive computation.","critical_errors":[],"gaps":[],"uncertainties":[],"repair_actions":[],"checked_fact_ids":[],"checked_source_ids":[],"evidence_level":"independent_llm_check"}),
        json!({
            "semantic_contract":{
                "variables":[],"assumptions":[],"conclusion":"1+1=2 in Nat",
                "definitions":[],"boundary_conditions":[],"ambiguity_notes":[]
            },
            "theorem_name":"one_add_one_eq_two",
            "lean_statement":"(1 : Nat) + 1 = 2",
            "lean_source":"import Mathlib\n\ntheorem one_add_one_eq_two : (1 : Nat) + 1 = 2 := by norm_num",
            "mapping":[{"natural_component":"1+1=2","lean_component":"(1 : Nat) + 1 = 2","relation":"equivalent"}]
        }),
        json!({"relation":"equivalent","rationale":"Both statements assert the same equality in natural numbers.","missing_assumptions":[],"extra_assumptions":[],"confidence":1.0}),
        json!({"candidates":[{"tactic":"norm_num","rationale_summary":"arithmetic normalization","expected_goal_reduction":1.0,"premise_names":[]}],"retrieval_summary":"deterministic arithmetic tactic"}),
    ]);
    let lean_root = temp.path().join("lean-verifier");
    std::fs::create_dir_all(&lean_root).expect("lean root");
    std::fs::write(
        lean_root.join("lean-toolchain"),
        "leanprover/lean4:v4.32.2\n",
    )
    .expect("toolchain");
    std::fs::write(lean_root.join("lakefile.toml"), "name = \"test\"\n").expect("lakefile");
    std::fs::write(
        lean_root.join("lake-manifest.json"),
        "{\"version\":1,\"packagesDir\":\".lake/packages\",\"packages\":[]}\n",
    )
    .expect("manifest");
    std::fs::write(
        lean_root.join("SOURCE_LOCK.json"),
        "{\"schema_version\":1,\"packages\":[]}",
    )
    .expect("source lock");
    let verification_backend = MockVerificationBackend::from_outcomes([
        BackendOutcome {
            backend: "mock_lean_kernel".into(),
            backend_version: "Lean 4.32.2".into(),
            status: CheckStatus::Failed,
            command: vec!["lake".into(), "env".into(), "lean".into()],
            working_directory: "initial_failed".into(),
            exit_code: Some(1),
            stdout: String::new(),
            stderr: String::new(),
            diagnostics: json!({}),
            axioms: vec![],
            elapsed_ms: 1,
            input_hash: "initial-formalization".into(),
            output_hash: "initial-failed".into(),
        },
        BackendOutcome {
            backend: "mock_lean_kernel".into(),
            backend_version: "Lean 4.32.2".into(),
            status: CheckStatus::Passed,
            command: vec!["lake".into(), "env".into(), "lean".into()],
            working_directory: "after_search".into(),
            exit_code: Some(0),
            stdout: "does not depend on any axioms".into(),
            stderr: String::new(),
            diagnostics: json!({}),
            axioms: vec![],
            elapsed_ms: 1,
            input_hash: "searched-formalization".into(),
            output_hash: "after-search".into(),
        },
        BackendOutcome {
            backend: "mock_lean_kernel".into(),
            backend_version: "Lean 4.32.2".into(),
            status: CheckStatus::Passed,
            command: vec!["lake".into(), "env".into(), "lean".into()],
            working_directory: "replay".into(),
            exit_code: Some(0),
            stdout: "does not depend on any axioms".into(),
            stderr: String::new(),
            diagnostics: json!({}),
            axioms: vec![],
            elapsed_ms: 1,
            input_hash: "searched-formalization".into(),
            output_hash: "replay".into(),
        },
    ]);
    let interactive_backend = MockInteractiveProofBackend::new(
        ProofState {
            state_id: Some(0),
            goals: vec![ProofGoal {
                name: "goal".into(),
                target: "(1 : Nat) + 1 = 2".into(),
                local_context: vec![],
            }],
            messages: json!([]),
            succeeded: true,
            has_sorry: false,
            has_unsafe: false,
        },
        [ProofState {
            state_id: Some(1),
            goals: vec![],
            messages: json!([]),
            succeeded: true,
            has_sorry: false,
            has_unsafe: false,
        }],
    );
    let config = ResearchConfig {
        runtime_root: temp.path().join("runtime"),
        output_root: temp.path().join("output"),
        model: None,
        lean_project_root: Some(lean_root),
        proof_search_budget: ProofSearchBudget {
            max_nodes: 16,
            max_depth: 4,
            max_seconds: 30,
            max_model_calls: 2,
            beam_width: 4,
        },
        ranking_weights: research_domain::RankingWeights::default(),
        planner_timeout_seconds: 5,
        worker_timeout_seconds: 5,
        verifier_timeout_seconds: 5,
        ..ResearchConfig::default()
    };
    let service = ResearchService::new_with_verification_backends(
        store.clone(),
        Arc::new(backend),
        Some(Arc::new(verification_backend)),
        Some(Arc::new(interactive_backend)),
        config.clone(),
    );
    let project = service
        .create_project(
            "arithmetic".into(),
            ProblemContract {
                original_problem: "Prove 1+1=2".into(),
                target_statement: "Prove that 1+1=2".into(),
                assumptions: vec![],
                success_criteria: "accepted".into(),
                version: 1,
            },
            Budget {
                max_rounds: 1,
                max_parallel_workers: 1,
                ..Budget::default()
            },
        )
        .await
        .expect("create");
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
                idempotency_key: "start".into(),
                reason: "test".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue");
    store
        .apply_command(&project.project_id, &command.command_id)
        .await
        .expect("apply");
    service
        .run_one_round(&project.project_id)
        .await
        .expect("round");
    let snapshot = store.snapshot(&project.project_id).await.expect("snapshot");
    let completed_round = snapshot.current_round.as_ref().expect("completed round");
    let planning_stages = store
        .list_planning_stages(&project.project_id, &completed_round.round_id)
        .await
        .expect("planning stages");
    assert_eq!(planning_stages.len(), 6);
    assert_eq!(
        planning_stages
            .iter()
            .map(|stage| stage["stage"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        [
            "strategy_director",
            "route_generator",
            "reflection",
            "proximity",
            "ranking",
            "supervisor"
        ]
    );
    let strategy_states = store
        .list_strategy_states(&project.project_id)
        .await
        .expect("strategy states");
    assert_eq!(strategy_states.len(), 1);
    assert_eq!(strategy_states[0]["audit_kind"], "initial");
    assert_eq!(strategy_states[0]["fixed_goal"], "Prove that 1+1=2");
    assert_eq!(
        strategy_states[0]["central_missing_bridge"],
        "A checked recursive derivation of 1+1=2."
    );
    assert_eq!(snapshot.project.status, ProjectStatus::Success);
    assert_eq!(snapshot.facts.len(), 2);
    let goal = snapshot
        .goals
        .iter()
        .find(|goal| goal.status.to_string() == "solved")
        .expect("solved goal");
    let solving_fact = snapshot
        .facts
        .iter()
        .find(|fact| Some(&fact.fact_id) == goal.solved_by_fact_id.as_ref())
        .expect("solving fact");
    assert_eq!(solving_fact.statement, "1+1=2");
    assert_eq!(solving_fact.evidence_level, "fully_certified");
    let target_verification = store
        .list_verifications(&project.project_id)
        .await
        .expect("verifications")
        .into_iter()
        .find(|verification| {
            verification
                .report
                .as_ref()
                .is_some_and(|report| report.evidence_level == "fully_certified")
        })
        .expect("target verification");
    let target_case = store
        .verification_case_for_verification(&target_verification.verification_id)
        .await
        .expect("target case");
    assert_eq!(target_case.stage.to_string(), "committed");
    assert_eq!(
        target_case
            .achieved_acceptance
            .expect("acceptance")
            .to_string(),
        "fully_certified"
    );
    assert_eq!(
        store
            .list_verification_replays(&target_case.case_id)
            .await
            .expect("replays")
            .len(),
        1
    );
    let proof_search = store
        .get_case_proof_search(&target_case.case_id)
        .await
        .expect("proof search");
    assert_eq!(proof_search.status, CheckStatus::Passed);
    assert!(
        store
            .list_artifacts(&project.project_id)
            .await
            .expect("artifacts")
            .len()
            >= 3
    );
    let leased_backend_runs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM backend_runs WHERE case_id=? AND attempt_id IS NOT NULL",
    )
    .bind(&target_case.case_id)
    .fetch_one(store.pool())
    .await
    .expect("leased backend runs");
    assert_eq!(leased_backend_runs, 3);
    let completed_tool_attempts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM verification_attempts WHERE case_id=? AND kind IN ('lean_final','lean_after_search','lean_replay','pantograph_proof_search') AND status='completed'")
        .bind(&target_case.case_id).fetch_one(store.pool()).await.expect("completed tool attempts");
    assert_eq!(completed_tool_attempts, 4);
    let synthetic_start_checkpoints: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM artifacts WHERE project_id=? AND kind='task_checkpoint'",
    )
    .bind(&project.project_id)
    .fetch_one(store.pool())
    .await
    .expect("task checkpoint count");
    assert_eq!(synthetic_start_checkpoints, 0);

    let publication_backend = MockBackend::from_responses([json!({
        "status":"ready",
        "article_plan_md":"# Plan\n\nState and prove the verified arithmetic theorem.",
        "claim_evidence_ledger_md":format!("Main theorem -> {}", solving_fact.fact_id),
        "related_work_tex":"",
        "article_candidate_tex":"\\documentclass{article}\n\\usepackage{amsthm}\n\\newtheorem{theorem}{Theorem}\n\\title{A Verified Arithmetic Identity}\n\\author{}\n\\begin{document}\n\\maketitle\n\\begin{abstract}We record a complete proof of a basic arithmetic identity.\\end{abstract}\n\\section{Main result}\n\\begin{theorem}In the natural numbers, $1+1=2$.\\end{theorem}\n\\begin{proof}This follows from the recursive definition of addition.\\end{proof}\n\\section{Conclusion}The stated identity has been proved.\\end{document}\n",
        "revision_notes_md":"Initial evidence-bound draft.",
        "evidence_gaps_md":""
    })]);
    let publication_backend_control = publication_backend.clone();
    let publication_service =
        ResearchService::new(store.clone(), Arc::new(publication_backend), config);
    let publication = publication_service
        .publish_paper_idempotent(&project.project_id, true, "full-round-publication")
        .await
        .expect("publication");
    assert_eq!(publication.status, "ready");
    let writer_root = std::path::PathBuf::from(&publication.output_directory);
    assert!(writer_root.join("article_candidate.pdf").is_file());
    assert!(writer_root.join("pdf_quality_report.json").is_file());
    assert!(
        writer_root
            .join("article_candidate_preview-1.png")
            .is_file()
    );
    let runs = store
        .list_publications(&project.project_id)
        .await
        .expect("publication runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "ready");

    publication_backend_control
        .push(json!({
            "status":"ready",
            "article_plan_md":"# Plan",
            "claim_evidence_ledger_md":format!("Main theorem -> {}", solving_fact.fact_id),
            "related_work_tex":"",
            "article_candidate_tex":"\\documentclass{article}\\begin{document}\\input{../../secret}\\end{document}",
            "revision_notes_md":"",
            "evidence_gaps_md":""
        }))
        .await;
    let failure = publication_service
        .publish_paper_idempotent(&project.project_id, true, "unsafe-publication")
        .await
        .expect_err("unsafe publication must fail");
    assert!(failure.to_string().contains("unsafe TeX"));
    let failed_run = store
        .list_publications(&project.project_id)
        .await
        .expect("publication runs")
        .into_iter()
        .find(|run| run.idempotency_key == "unsafe-publication")
        .expect("failed publication run");
    assert_eq!(failed_run.status, "failed");
    assert!(
        failed_run
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unsafe TeX"))
    );
    let replayed_failure = publication_service
        .publish_paper_idempotent(&project.project_id, true, "unsafe-publication")
        .await
        .expect_err("failed publication must replay its stored error");
    assert!(replayed_failure.to_string().contains("unsafe TeX"));
}
