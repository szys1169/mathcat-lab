use research_domain::{
    AcceptanceClass, AssignmentDraft, Budget, CandidateSubmission, CandidateType, CommandMode,
    ExperimentDraft, FailureDraft, FormalizerOutput, HumanRouteProposalRequest, PlannerOutput,
    ProblemContract, ProofNodeStatus, ProofSearchBudget, RouteProposal, SemanticContractDraft,
    SourceDraft, StrategyDirectorOutput, StrategyInterfaceDebt, StrategyRouteState,
    VerificationProfile, VerificationReport, VerificationStage, VerificationVerdict, WorkerOutput,
};
use serde_json::json;
use sqlx::Row;
use tempfile::TempDir;

use crate::{
    CommandDraft, ModelCallRequest, ProofNodeDraft, SqliteStore, StorageError,
    VerificationCaseDraft,
};

async fn store() -> (SqliteStore, TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path())
        .await
        .expect("store");
    (store, temp)
}

#[tokio::test]
async fn file_backed_sqlite_uses_a_separate_read_pool_after_migration() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp
        .path()
        .join("read-pool.sqlite")
        .to_string_lossy()
        .replace('\\', "/");
    let database_url = format!("sqlite://{path}");
    let store = SqliteStore::connect(&database_url, temp.path().join("artifacts"))
        .await
        .expect("file-backed store");
    let (project, _) = store
        .create_project("read-pool".into(), contract(), Budget::default())
        .await
        .expect("create through writer");
    assert_eq!(
        store
            .get_project(&project.project_id)
            .await
            .expect("read through read pool")
            .name,
        "read-pool"
    );
    assert_eq!(store.pool().size(), 1);
    assert!(store.read_pool().size() >= 1);
}

#[tokio::test]
async fn committed_plan_closes_human_route_proposals_with_auditable_decisions() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("proposal-decision".into(), contract(), Budget::default())
        .await
        .expect("project");
    store
        .propose_human_route(
            &project.project_id,
            &HumanRouteProposalRequest {
                expected_revision: project.revision,
                title: "unused human route".into(),
                method_summary: "an approach the committed plan does not adopt".into(),
                target_goal_ids: vec![],
                required_fact_ids: vec![],
                known_risks: vec![],
                reason: "whiteboard suggestion".into(),
            },
            "researcher",
            "proposal-decision-once",
        )
        .await
        .expect("proposal");
    let revision_after_rejection_candidate = store
        .get_project(&project.project_id)
        .await
        .expect("project")
        .revision;
    store
        .propose_human_route(
            &project.project_id,
            &HumanRouteProposalRequest {
                expected_revision: revision_after_rejection_candidate,
                title: "researcher wording may differ".into(),
                method_summary: "  DIRECT  ".into(),
                target_goal_ids: vec![],
                required_fact_ids: vec![],
                known_risks: vec![],
                reason: "whiteboard route matching the plan".into(),
            },
            "researcher",
            "proposal-decision-match",
        )
        .await
        .expect("matching proposal");
    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project")
        .revision;
    start_project(&store, &project.project_id, revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &plan(),
            &delta,
            "proposal decision test",
            None,
        )
        .await
        .expect("plan");
    let proposals = store
        .list_human_route_proposals(&project.project_id)
        .await
        .expect("proposals");
    assert_eq!(proposals.len(), 2);
    let rejected = proposals
        .iter()
        .find(|proposal| proposal.title == "unused human route")
        .expect("rejected proposal");
    assert_eq!(rejected.status, "rejected");
    assert!(rejected.decision_reason.is_some());
    let accepted = proposals
        .iter()
        .find(|proposal| proposal.title == "researcher wording may differ")
        .expect("accepted proposal");
    assert_eq!(accepted.status, "accepted");
    assert!(accepted.route_id.is_some());
    assert!(saved.events.iter().any(|event| {
        event.event_type == "route.proposal.rejected" && event.entity.id == rejected.proposal_id
    }));
    assert!(saved.events.iter().any(|event| {
        event.event_type == "route.proposal.accepted" && event.entity.id == accepted.proposal_id
    }));
}

fn contract() -> ProblemContract {
    ProblemContract {
        original_problem: "Prove 1+1=2".into(),
        target_statement: "1+1=2".into(),
        assumptions: vec![],
        success_criteria: "accepted".into(),
        version: 1,
    }
}

fn plan() -> PlannerOutput {
    let route = |title: &str| RouteProposal {
        title: title.into(),
        method_summary: title.into(),
        target_goal_ids: vec![],
        required_fact_ids: vec![],
        expected_subgoals: vec![],
        expected_goal_progress: 0.5,
        uncertainty_reduction: 0.5,
        human_suggestion_alignment: 0.0,
        evidence_support: 0.5,
        route_diversity: 0.5,
        verifiability: 0.5,
        novelty: 0.5,
        failure_similarity_penalty: 0.0,
        cost_penalty: 0.0,
        risks: vec![],
    };
    PlannerOutput {
        rationale_summary: "test plan".into(),
        routes: vec![route("direct"), route("attack")],
        assignments: vec![AssignmentDraft {
            route_index: 0,
            worker_role: "prover".into(),
            strategic_role: "central_bridge".into(),
            addresses_interface_debt: true,
            goal_ids: vec![],
            objective: "prove".into(),
            completion_contract: "candidate".into(),
            priority: 1.0,
        }],
        targeted_uncertainty_ids: vec![],
        suggestion_decisions: vec![],
    }
}

async fn start_project(store: &SqliteStore, project_id: &str, expected_revision: i64) {
    let (command, _) = store
        .enqueue_command(
            project_id,
            CommandDraft {
                command_type: "start_project".into(),
                target_kind: "project".into(),
                target_id: project_id.into(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: expected_revision,
                idempotency_key: "start-helper".into(),
                reason: "test".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue start");
    store
        .apply_command(project_id, &command.command_id)
        .await
        .expect("apply start");
}

#[tokio::test]
async fn accepted_source_backed_fact_atomically_admits_its_cited_source() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("source-admission".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &plan(),
            &delta,
            "source admission test",
            None,
        )
        .await
        .expect("plan");
    let task = &saved.tasks[0];
    let packet = store
        .get_context_packet_for_task(&task.task_id)
        .await
        .expect("packet");
    let offer = store
        .offer_local_task(task, "mock", None, "runtime/source-test", json!({}))
        .await
        .expect("offer");
    let lease = store
        .accept_local_handshake(
            &offer,
            Some("source-worker"),
            json!({"context_hash": packet.content_hash}),
            60,
        )
        .await
        .expect("lease");
    sqlx::query("INSERT INTO sources(source_id,project_id,title,authors_json,url,citation_key,theorem_reference,statement_excerpt,assumptions_json,applicability,status,retrieved_at) VALUES('source-test',?,'Primary theorem','[]','https://example.test/paper','Primary2026','Theorem 1','If A then A','[]','exact match','reported_unverified',?)")
        .bind(&project.project_id)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(store.pool())
        .await
        .expect("source");
    let receipt = store
        .submit_candidate(
            &project.project_id,
            CandidateSubmission {
                task_id: lease.task.task_id.clone(),
                route_id: lease.task.route_id.clone(),
                target_goal_ids: Vec::new(),
                statement: "A implies A".into(),
                assumptions: Vec::new(),
                proof_markdown: "Identity.".into(),
                dependency_fact_ids: Vec::new(),
                definitions_introduced: Default::default(),
                external_source_ids: vec!["source-test".into()],
                candidate_type: CandidateType::Lemma,
                task_revision: lease.task.revision,
                route_cancellation_epoch: lease.task.route_cancellation_epoch,
            },
            "source-admission-candidate",
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
                name: "source-backed".into(),
                profile: VerificationProfile::StandardReview,
                required_acceptance: AcceptanceClass::Reviewed,
                required_checks: vec!["citation_review".into()],
                independent_reviewer_count: 1,
                require_citation_review: true,
                require_adversarial_review: false,
                require_alignment_review: false,
                require_fresh_replay: false,
                max_attempts: 1,
                risk_score: 0.1,
                risk_reasons: Vec::new(),
            },
        )
        .await
        .expect("case");
    store
        .create_verification_snapshot(
            &case.case_id,
            crate::VerificationSnapshotDraft {
                toolchain_hash: None,
                extra_payload: json!({}),
            },
        )
        .await
        .expect("snapshot");
    store
        .record_verification_check(
            &case.case_id,
            crate::CheckDraft {
                attempt_id: None,
                kind: "citation_review".into(),
                status: research_domain::CheckStatus::Passed,
                mandatory: true,
                summary: "source statement, assumptions and applicability checked".into(),
                details: json!({"checked_source_ids":["source-test"]}),
            },
        )
        .await
        .expect("citation review");
    store
        .transition_verification_case(&case.case_id, 0, VerificationStage::Review, None)
        .await
        .expect("review");
    store
        .transition_verification_case(&case.case_id, 0, VerificationStage::Adjudication, None)
        .await
        .expect("adjudication");
    store
        .transition_verification_case(
            &case.case_id,
            0,
            VerificationStage::CommitReady,
            Some(AcceptanceClass::Reviewed),
        )
        .await
        .expect("commit ready");
    let commit = store
        .commit_verification(
            &receipt.verification.verification_id,
            VerificationReport {
                verdict: VerificationVerdict::Accepted,
                summary: "accepted".into(),
                critical_errors: Vec::new(),
                gaps: Vec::new(),
                uncertainties: Vec::new(),
                repair_actions: Vec::new(),
                checked_fact_ids: Vec::new(),
                checked_source_ids: vec!["source-test".into()],
                evidence_level: "reviewed".into(),
            },
        )
        .await
        .expect("commit");
    let sources = store
        .list_sources(&project.project_id)
        .await
        .expect("sources");
    assert_eq!(sources[0].status, "admitted");
    assert!(
        commit
            .events
            .iter()
            .any(|event| event.event_type == "source.admitted")
    );
}

#[tokio::test]
async fn goal_closure_requires_independent_coverage_and_accepts_semantic_equivalence() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project(
            "semantic-goal-closure".into(),
            contract(),
            Budget::default(),
        )
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let goal = store
        .list_goals(&project.project_id)
        .await
        .expect("goals")
        .into_iter()
        .next()
        .expect("main goal");
    sqlx::query("INSERT INTO uncertainties(uncertainty_id,project_id,description,type,severity,affects_goal_ids_json,affects_route_ids_json,introduced_by,resolution_methods_json,status,created_at) VALUES('uncertainty-obsolete-on-closure',?,'an older route-specific doubt','worker_uncertainty','high',?,'[]','older-task','[]','open',?)")
        .bind(&project.project_id)
        .bind(json!([goal.goal_id.clone()]).to_string())
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(store.pool())
        .await
        .expect("seed route-specific uncertainty");
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &plan(),
            &delta,
            "semantic goal closure test",
            None,
        )
        .await
        .expect("plan");
    let task = &saved.tasks[0];
    let packet = store
        .get_context_packet_for_task(&task.task_id)
        .await
        .expect("packet");
    let offer = store
        .offer_local_task(task, "mock", None, "runtime/goal-closure", json!({}))
        .await
        .expect("offer");
    let lease = store
        .accept_local_handshake(
            &offer,
            Some("goal-closure-worker"),
            json!({"context_hash": packet.content_hash}),
            60,
        )
        .await
        .expect("lease");
    let receipt = store
        .submit_candidate(
            &project.project_id,
            CandidateSubmission {
                task_id: lease.task.task_id.clone(),
                route_id: lease.task.route_id.clone(),
                target_goal_ids: vec![goal.goal_id.clone()],
                statement: "The sum of the two natural-number units is the natural number two"
                    .into(),
                assumptions: Vec::new(),
                proof_markdown: "This is the same natural-number equality as the target.".into(),
                dependency_fact_ids: Vec::new(),
                definitions_introduced: Default::default(),
                external_source_ids: Vec::new(),
                candidate_type: CandidateType::Theorem,
                task_revision: lease.task.revision,
                route_cancellation_epoch: lease.task.route_cancellation_epoch,
            },
            "semantic-goal-closure-candidate",
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
                name: "semantic-goal-closure".into(),
                profile: VerificationProfile::StandardReview,
                required_acceptance: AcceptanceClass::Reviewed,
                required_checks: Vec::new(),
                independent_reviewer_count: 2,
                require_citation_review: false,
                require_adversarial_review: true,
                require_alignment_review: false,
                require_fresh_replay: false,
                max_attempts: 1,
                risk_score: 0.8,
                risk_reasons: vec!["candidate_targets_main_goal".into()],
            },
        )
        .await
        .expect("case");
    store
        .create_verification_snapshot(
            &case.case_id,
            crate::VerificationSnapshotDraft {
                toolchain_hash: None,
                extra_payload: json!({}),
            },
        )
        .await
        .expect("snapshot");
    store
        .record_verification_check(
            &case.case_id,
            crate::CheckDraft {
                attempt_id: None,
                kind: "goal_coverage_review".into(),
                status: research_domain::CheckStatus::Passed,
                mandatory: false,
                summary: "Independent review confirms semantic equivalence to 1+1=2".into(),
                details: json!({"target_goal_ids":[goal.goal_id]}),
            },
        )
        .await
        .expect("goal coverage");
    store
        .transition_verification_case(&case.case_id, 0, VerificationStage::Review, None)
        .await
        .expect("review");
    store
        .transition_verification_case(&case.case_id, 0, VerificationStage::Adjudication, None)
        .await
        .expect("adjudication");
    store
        .transition_verification_case(
            &case.case_id,
            0,
            VerificationStage::CommitReady,
            Some(AcceptanceClass::Reviewed),
        )
        .await
        .expect("commit ready");
    store
        .commit_verification(
            &receipt.verification.verification_id,
            VerificationReport {
                verdict: VerificationVerdict::Accepted,
                summary: "candidate and semantic goal coverage accepted independently".into(),
                critical_errors: Vec::new(),
                gaps: Vec::new(),
                uncertainties: Vec::new(),
                repair_actions: Vec::new(),
                checked_fact_ids: Vec::new(),
                checked_source_ids: Vec::new(),
                evidence_level: "reviewed".into(),
            },
        )
        .await
        .expect("commit");
    store
        .complete_round(&project.project_id, &round.round_id, "semantic closure")
        .await
        .expect("complete round");

    let snapshot = store.snapshot(&project.project_id).await.expect("snapshot");
    assert_eq!(snapshot.project.status.to_string(), "success");
    assert_eq!(snapshot.goals[0].status.to_string(), "solved");
    assert!(snapshot.goals[0].solved_by_fact_id.is_some());
    let obsolete = snapshot
        .uncertainties
        .iter()
        .find(|item| item.uncertainty_id == "uncertainty-obsolete-on-closure")
        .expect("obsolete uncertainty retained for audit");
    assert_eq!(
        obsolete.status,
        research_domain::UncertaintyStatus::Obsolete
    );
    assert_eq!(obsolete.resolved_by, snapshot.goals[0].solved_by_fact_id);
    let closures = store
        .list_goal_closures(&project.project_id, &snapshot.goals[0].goal_id)
        .await
        .expect("goal closures");
    assert_eq!(closures.len(), 1);
    assert_eq!(closures[0]["outcome"], "solved");
    assert_eq!(
        closures[0]["verification_id"],
        receipt.verification.verification_id
    );
}

#[allow(clippy::too_many_arguments)]
async fn apply_test_command(
    store: &SqliteStore,
    project_id: &str,
    command_type: &str,
    target_kind: &str,
    target_id: &str,
    payload: serde_json::Value,
    key: &str,
    reason: &str,
) -> research_domain::HumanCommand {
    let revision = store
        .get_project(project_id)
        .await
        .expect("project revision")
        .revision;
    let (command, _) = store
        .enqueue_command(
            project_id,
            CommandDraft {
                command_type: command_type.into(),
                target_kind: target_kind.into(),
                target_id: target_id.into(),
                mode: CommandMode::Immediate,
                payload,
                expected_project_revision: revision,
                idempotency_key: key.into(),
                reason: reason.into(),
                requested_by: "test-operator".into(),
            },
        )
        .await
        .expect("enqueue test command");
    let (applied, _) = store
        .apply_command(project_id, &command.command_id)
        .await
        .expect("apply test command");
    applied
}

#[tokio::test]
async fn reliability_v2_plan_and_local_worker_commit_are_atomic_and_queryable() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("reliability-v2".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &plan(),
            &delta,
            "primary",
            None,
        )
        .await
        .expect("v2 plan");
    assert_eq!(saved.tasks.len(), 1);
    let task = &saved.tasks[0];
    let contract = store
        .get_task_contract(&project.project_id, &task.task_id)
        .await
        .expect("task contract");
    let packet = store
        .get_context_packet_for_task(&task.task_id)
        .await
        .expect("context packet");
    assert_eq!(contract.task_id, task.task_id);
    assert_eq!(packet.task_id.as_deref(), Some(task.task_id.as_str()));
    assert!(!packet.content_hash.is_empty());

    let offer = store
        .offer_local_task(
            task,
            "mock",
            None,
            "runtime/test-worker",
            json!({"structured_output":true}),
        )
        .await
        .expect("offer");
    assert_eq!(offer.task.status, research_domain::TaskStatus::Offered);
    let lease = store
        .accept_local_handshake(
            &offer,
            Some("mock-1"),
            json!({"context_hash":packet.content_hash}),
            60,
        )
        .await
        .expect("handshake");
    assert_eq!(lease.task.status, research_domain::TaskStatus::Running);
    store
        .heartbeat_local_lease(&lease, 60)
        .await
        .expect("heartbeat");
    let output = WorkerOutput {
        summary: "recorded a precise obstruction".into(),
        discoveries: vec![],
        candidates: vec![],
        failures: vec![FailureDraft {
            failure_type: "missing_bridge".into(),
            summary: "the named bridge lemma is still missing".into(),
            repairable: true,
        }],
        uncertainties: vec![],
        sources: vec![],
        experiments: vec![],
    };
    let (envelope_id, _) = store
        .submit_local_result_envelope(&lease, &output)
        .await
        .expect("submit envelope");
    let (duplicate_id, duplicate_events) = store
        .submit_local_result_envelope(&lease, &output)
        .await
        .expect("idempotent envelope");
    assert_eq!(duplicate_id, envelope_id);
    assert!(duplicate_events.is_empty());
    let (ingested, _) = store
        .ingest_local_result_envelope(&envelope_id)
        .await
        .expect("ingest envelope");
    assert_eq!(ingested.summary, output.summary);
    assert_eq!(
        store
            .get_task(&project.project_id, &task.task_id)
            .await
            .expect("completed task")
            .status,
        research_domain::TaskStatus::Completed
    );
    assert_eq!(
        store
            .list_plan_revisions(&project.project_id)
            .await
            .expect("plan revisions")
            .len(),
        1
    );
    let pending_outbox: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM event_outbox WHERE project_id=? AND status='pending'",
    )
    .bind(&project.project_id)
    .fetch_one(store.pool())
    .await
    .expect("outbox count");
    assert!(pending_outbox > 0);
    let writer = store.state_writer_status();
    assert_eq!(writer.status, "running");
    assert!(writer.completed_total > 0);
}

#[tokio::test]
async fn reliability_v2_quarantines_bad_literature_source_without_losing_good_source() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project(
            "literature-ingestion-v2".into(),
            contract(),
            Budget::default(),
        )
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    let mut literature_plan = plan();
    literature_plan.assignments[0].worker_role = "literature_researcher".into();
    literature_plan.assignments[0].strategic_role = "literature".into();
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &literature_plan,
            &delta,
            "literature source test",
            None,
        )
        .await
        .expect("plan");
    let task = &saved.tasks[0];
    let offer = store
        .offer_local_task(task, "mock", None, "runtime/literature", json!({}))
        .await
        .expect("offer");
    let lease = store
        .accept_local_handshake(
            &offer,
            Some("literature-worker"),
            json!({"context_hash":offer.context_packet.content_hash}),
            60,
        )
        .await
        .expect("lease");
    let document = b"archived primary source";
    let (artifact, _) = store
        .store_artifact(
            &project.project_id,
            "source_fulltext",
            "primary.txt",
            document,
            round.number,
            vec![task.task_id.clone()],
        )
        .await
        .expect("fulltext artifact");
    let source = |title: &str, artifact_id: Option<String>| SourceDraft {
        title: title.into(),
        authors: vec!["A. Author".into()],
        url: Some(format!("https://example.test/{title}")),
        citation_key: Some(title.into()),
        theorem_reference: Some("Theorem 1".into()),
        statement_excerpt: Some("If A, then A".into()),
        assumptions: vec!["A".into()],
        applicability: "candidate tool only".into(),
        status: "reported".into(),
        retrieval_query: Some("A theorem".into()),
        document_version: Some("v1".into()),
        fulltext_path: None,
        fulltext_sha256: Some(artifact.sha256.clone()),
        fulltext_artifact_id: artifact_id,
    };
    let output = WorkerOutput {
        summary: "one source archived and one source unavailable".into(),
        discoveries: vec![],
        candidates: vec![],
        failures: vec![],
        uncertainties: vec![],
        sources: vec![
            source("good", Some(artifact.artifact_id.clone())),
            source("bad", None),
        ],
        experiments: vec![],
    };
    let (envelope_id, _) = store
        .submit_local_result_envelope(&lease, &output)
        .await
        .expect("submit");
    let (_, events) = store
        .ingest_local_result_envelope(&envelope_id)
        .await
        .expect("ingest");
    assert_eq!(
        store
            .list_sources(&project.project_id)
            .await
            .expect("sources")
            .len(),
        1
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "source.reported")
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "source.draft.rejected")
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "route.material_progress")
    );
    let progress_kind: String = sqlx::query_scalar(
        "SELECT progress_kind FROM route_progress_entries WHERE route_id=? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&task.route_id)
    .fetch_one(store.pool())
    .await
    .expect("source progress");
    assert_eq!(progress_kind, "source");
}

#[tokio::test]
async fn reliability_v2_checkpoint_is_resumable_and_does_not_stale_the_result_lease() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("checkpoint-v2".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &plan(),
            &delta,
            "primary",
            None,
        )
        .await
        .expect("plan");
    let task = &saved.tasks[0];
    let offer = store
        .offer_local_task(task, "mock", None, "runtime/checkpoint", json!({}))
        .await
        .expect("offer");
    assert!(offer.resume_checkpoint.is_none());
    let lease = store
        .accept_local_handshake(
            &offer,
            Some("mock-1"),
            json!({"context_hash":offer.context_packet.content_hash}),
            90,
        )
        .await
        .expect("lease");
    let (artifact, _) = store
        .store_artifact(
            &project.project_id,
            "task_checkpoint",
            "checkpoint.json",
            br#"{"safe_point":"handshake"}"#,
            round.number,
            vec![task.task_id.clone(), lease.attempt_id.clone()],
        )
        .await
        .expect("checkpoint artifact");
    store
        .save_local_checkpoint(&lease, &artifact.artifact_id, "handshake safe point")
        .await
        .expect("save checkpoint");
    let history = store
        .task_attempt_history(&project.project_id, &task.task_id)
        .await
        .expect("attempt history");
    assert_eq!(
        history["checkpoints"]
            .as_array()
            .expect("checkpoints")
            .len(),
        1
    );
    let output = WorkerOutput {
        summary: "partial but auditable".into(),
        discoveries: vec![],
        candidates: vec![],
        failures: vec![],
        uncertainties: vec![],
        sources: vec![],
        experiments: vec![],
    };
    let (envelope_id, _) = store
        .submit_local_result_envelope(&lease, &output)
        .await
        .expect("submit after checkpoint");
    store
        .ingest_local_result_envelope(&envelope_id)
        .await
        .expect("ingest after checkpoint");
    assert_eq!(
        store
            .get_task(&project.project_id, &task.task_id)
            .await
            .expect("completed task")
            .status,
        research_domain::TaskStatus::Completed
    );
}

#[tokio::test]
async fn service_startup_reconciliation_orphans_old_local_lease_and_prefers_checkpoint_retry() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("startup-recovery-v2".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &plan(),
            &delta,
            "primary",
            None,
        )
        .await
        .expect("plan");
    let task = &saved.tasks[0];
    let offer = store
        .offer_local_task(task, "mock", None, "runtime/restart", json!({}))
        .await
        .expect("offer");
    let lease = store
        .accept_local_handshake(&offer, Some("mock-1"), json!({}), 3_600)
        .await
        .expect("lease");
    let (artifact, _) = store
        .store_artifact(
            &project.project_id,
            "task_checkpoint",
            "restart.json",
            br#"{"resume":"from here"}"#,
            round.number,
            vec![task.task_id.clone()],
        )
        .await
        .expect("artifact");
    store
        .save_local_checkpoint(&lease, &artifact.artifact_id, "restart point")
        .await
        .expect("checkpoint");
    let report = store
        .run_reconciliation(Some(&project.project_id), "service_startup")
        .await
        .expect("startup reconciliation");
    assert!(report["repairs"].as_array().is_some_and(|repairs| {
        repairs
            .iter()
            .any(|repair| repair["kind"] == "expired_lease")
    }));
    assert_eq!(
        store
            .get_task(&project.project_id, &task.task_id)
            .await
            .expect("requeued task")
            .status,
        research_domain::TaskStatus::Queued
    );
    let resumed_offer = store
        .offer_local_task(task, "mock", None, "runtime/restart-2", json!({}))
        .await
        .expect("resume offer");
    assert_eq!(
        resumed_offer
            .resume_checkpoint
            .as_ref()
            .and_then(|value| value["artifact_id"].as_str()),
        Some(artifact.artifact_id.as_str())
    );
}

#[tokio::test]
async fn reconciliation_repairs_duplicate_attempt_terminal_event_and_artifact_integrity() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project(
            "reconciliation-integrity".into(),
            contract(),
            Budget::default(),
        )
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &plan(),
            &delta,
            "primary",
            None,
        )
        .await
        .expect("plan");
    let task = saved.tasks[0].clone();
    let first_offer = store
        .offer_local_task(&task, "mock", None, "runtime/duplicate-1", json!({}))
        .await
        .expect("first offer");
    let first_lease = store
        .accept_local_handshake(&first_offer, Some("mock-1"), json!({}), 3_600)
        .await
        .expect("first lease");
    sqlx::query("UPDATE task_leases SET status='expired' WHERE lease_id=?")
        .bind(&first_lease.lease_id)
        .execute(store.pool())
        .await
        .expect("inject expired first lease");
    sqlx::query("UPDATE tasks SET status='queued' WHERE task_id=?")
        .bind(&task.task_id)
        .execute(store.pool())
        .await
        .expect("inject requeue");
    let second_offer = store
        .offer_local_task(&task, "mock", None, "runtime/duplicate-2", json!({}))
        .await
        .expect("second offer");
    store
        .accept_local_handshake(&second_offer, Some("mock-2"), json!({}), 3_600)
        .await
        .expect("second lease");

    let duplicate_report = store
        .run_reconciliation(Some(&project.project_id), "watchdog_tick")
        .await
        .expect("duplicate reconciliation");
    assert!(
        duplicate_report["repairs"]
            .as_array()
            .is_some_and(|repairs| {
                repairs
                    .iter()
                    .any(|repair| repair["kind"] == "duplicate_active_attempt_orphaned")
            })
    );
    let first_status: String =
        sqlx::query_scalar("SELECT status FROM task_attempts WHERE attempt_id=?")
            .bind(&first_offer.attempt.attempt_id)
            .fetch_one(store.pool())
            .await
            .expect("first attempt status");
    let second_status: String =
        sqlx::query_scalar("SELECT status FROM task_attempts WHERE attempt_id=?")
            .bind(&second_offer.attempt.attempt_id)
            .fetch_one(store.pool())
            .await
            .expect("second attempt status");
    assert_eq!(first_status, "orphaned");
    assert_eq!(second_status, "running");

    sqlx::query("UPDATE tasks SET status='completed' WHERE task_id=?")
        .bind(&task.task_id)
        .execute(store.pool())
        .await
        .expect("inject missing terminal event");
    let terminal_report = store
        .run_reconciliation(Some(&project.project_id), "watchdog_tick")
        .await
        .expect("terminal reconciliation");
    assert!(
        terminal_report["repairs"]
            .as_array()
            .is_some_and(|repairs| {
                repairs
                    .iter()
                    .any(|repair| repair["kind"] == "missing_terminal_event_repaired")
            })
    );
    let terminal_events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE project_id=? AND type='task.result_ingested' AND json_extract(data_json,'$.task_id')=?")
        .bind(&project.project_id).bind(&task.task_id).fetch_one(store.pool()).await.expect("terminal event count");
    assert_eq!(terminal_events, 1);

    let (artifact, _) = store
        .store_artifact(
            &project.project_id,
            "integrity_fixture",
            "integrity.txt",
            b"trusted",
            round.number,
            vec![task.task_id],
        )
        .await
        .expect("artifact");
    tokio::fs::write(&artifact.storage_path, b"tampered")
        .await
        .expect("tamper fixture");
    let integrity_report = store
        .run_reconciliation(Some(&project.project_id), "manual")
        .await
        .expect("integrity reconciliation");
    assert!(
        integrity_report["repairs"]
            .as_array()
            .is_some_and(|repairs| {
                repairs
                    .iter()
                    .any(|repair| repair["kind"] == "artifact_integrity_error")
            })
    );
    assert_eq!(
        store
            .get_project(&project.project_id)
            .await
            .expect("integrity project")
            .status,
        research_domain::ProjectStatus::NeedsHumanReview
    );
}

#[tokio::test]
async fn v2_human_commands_are_atomic_idempotent_and_preserve_route_provenance() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("commands-v2".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &plan(),
            &delta,
            "primary",
            None,
        )
        .await
        .expect("plan");
    let task = saved.tasks[0].clone();
    let old_packet_id = task.context_packet_id.clone().expect("initial packet");
    apply_test_command(
        &store,
        &project.project_id,
        "rebuild_task_context",
        "task",
        &task.task_id,
        json!({}),
        "rebuild-context-once",
        "refresh provenance-aware context",
    )
    .await;
    let rebuilt_task = store
        .get_task(&project.project_id, &task.task_id)
        .await
        .expect("rebuilt task");
    assert_ne!(
        rebuilt_task.context_packet_id.as_deref(),
        Some(old_packet_id.as_str())
    );
    assert_eq!(
        store
            .get_task_contract(&project.project_id, &task.task_id)
            .await
            .expect("new contract version")
            .contract_version,
        2
    );
    sqlx::query("UPDATE tasks SET status='blocked' WHERE task_id=?")
        .bind(&task.task_id)
        .execute(store.pool())
        .await
        .expect("simulate precise blocker");
    apply_test_command(
        &store,
        &project.project_id,
        "retry_task",
        "task",
        &task.task_id,
        json!({}),
        "retry-task-once",
        "blocker was repaired",
    )
    .await;
    let retry_task = store
        .get_task(&project.project_id, &task.task_id)
        .await
        .expect("retry task");
    assert_eq!(retry_task.status, research_domain::TaskStatus::Queued);
    let offer = store
        .offer_local_task(&retry_task, "mock", None, "runtime/quarantine", json!({}))
        .await
        .expect("offer for quarantine");
    store
        .accept_local_handshake(&offer, Some("mock-1"), json!({}), 90)
        .await
        .expect("lease for quarantine");
    apply_test_command(
        &store,
        &project.project_id,
        "quarantine_worker_instance",
        "worker_instance",
        &offer.worker_instance.worker_instance_id,
        json!({}),
        "quarantine-once",
        "worker failed capability audit",
    )
    .await;
    assert_eq!(
        store
            .list_worker_instances(&project.project_id)
            .await
            .expect("worker instances")
            .into_iter()
            .find(|instance| instance.worker_instance_id == offer.worker_instance.worker_instance_id)
            .expect("quarantined instance")
            .status,
        "quarantined"
    );
    let canonical = &saved.routes[0].route_id;
    let source = &saved.routes[1].route_id;
    let applied = apply_test_command(
        &store,
        &project.project_id,
        "merge_routes",
        "project",
        &project.project_id,
        json!({"canonical_route_id":canonical,"source_route_ids":[source]}),
        "merge-routes-once",
        "same semantic family after operator review",
    )
    .await;
    assert_eq!(applied.status, research_domain::CommandStatus::Applied);
    let merged = store
        .get_route(&project.project_id, source)
        .await
        .expect("merged route");
    assert_eq!(merged.status, research_domain::RouteStatus::Merged);
    assert_eq!(merged.merged_into.as_deref(), Some(canonical.as_str()));
    let (duplicate, event) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "merge_routes".into(),
                target_kind: "project".into(),
                target_id: project.project_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({"canonical_route_id":canonical,"source_route_ids":[source]}),
                expected_project_revision: applied.expected_project_revision,
                idempotency_key: "merge-routes-once".into(),
                reason: "same semantic family after operator review".into(),
                requested_by: "test-operator".into(),
            },
        )
        .await
        .expect("deduplicated command");
    assert_eq!(duplicate.command_id, applied.command_id);
    assert!(event.is_none());
}

#[tokio::test]
async fn reliability_v2_same_failure_is_bounded_and_dead_lettered() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("bounded-retry".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &plan(),
            &delta,
            "primary",
            None,
        )
        .await
        .expect("v2 plan");
    let task = &saved.tasks[0];
    let first = store
        .offer_local_task(task, "mock", None, "runtime/attempt-1", json!({}))
        .await
        .expect("first offer");
    store
        .fail_local_attempt(None, &first, "schema mismatch")
        .await
        .expect("first failure");
    let queued = store
        .get_task(&project.project_id, &task.task_id)
        .await
        .expect("queued task");
    assert_eq!(queued.status, research_domain::TaskStatus::Queued);
    let second = store
        .offer_local_task(&queued, "mock", None, "runtime/attempt-2", json!({}))
        .await
        .expect("second offer");
    store
        .fail_local_attempt(None, &second, "schema mismatch")
        .await
        .expect("second failure");
    assert_eq!(
        store
            .get_task(&project.project_id, &task.task_id)
            .await
            .expect("dead letter task")
            .status,
        research_domain::TaskStatus::DeadLettered
    );
}

#[tokio::test]
async fn reliability_v2_pruned_route_is_tombstoned_until_evidence_backed_revival() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("route-governance".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &plan(),
            &delta,
            "primary",
            None,
        )
        .await
        .expect("plan");
    let route_id = saved.routes[0].route_id.clone();
    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project")
        .revision;
    store
        .prune_route_v2(
            &project.project_id,
            &route_id,
            revision,
            "superseded by evidence",
        )
        .await
        .expect("prune");
    assert_eq!(
        store
            .get_route(&project.project_id, &route_id)
            .await
            .expect("route")
            .status,
        research_domain::RouteStatus::Pruned
    );
    assert_eq!(
        store
            .list_route_tombstones(&project.project_id)
            .await
            .expect("tombstones")
            .len(),
        1
    );
    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project")
        .revision;
    assert!(matches!(
        store
            .revive_route_v2(&project.project_id, &route_id, revision, "", &[])
            .await,
        Err(StorageError::InvalidTransition(_))
    ));
    store
        .revive_route_v2(
            &project.project_id,
            &route_id,
            revision,
            "new certified lemma changes the failed premise",
            &["fact_certified_1".into()],
        )
        .await
        .expect("revive");
    assert_eq!(
        store
            .get_route(&project.project_id, &route_id)
            .await
            .expect("revived route")
            .status,
        research_domain::RouteStatus::Revived
    );
}

#[tokio::test]
async fn verification_workers_use_packet_contract_lease_and_result_envelope() {
    let (store, _temp) = store().await;
    let (case, _formalization) = proof_search_fixture(&store).await;
    let content = json!({
        "problem_contract":{"target":"1+1=2"},
        "candidate":{"statement":"1+1=2"},
        "active_facts":[],
        "reviewer_isolation":true,
    });
    let offer = store
        .offer_verification_worker(
            &case.case_id,
            "math_review_1",
            "mock",
            None,
            "runtime/verifier",
            &content,
            &[],
            &json!({"success":"accepted/rejected/unknown report"}),
        )
        .await
        .expect("offer verification worker");
    assert_eq!(offer.context_packet.packet_kind, "verification");
    let lease = store
        .accept_verification_worker_handshake(
            &offer,
            Some("mock-1"),
            &json!({"context_hash":offer.context_packet.content_hash}),
            60,
        )
        .await
        .expect("verification handshake");
    let result = json!({"verdict":"accepted","summary":"independent review"});
    let (envelope_id, _) = store
        .submit_verification_result(&lease, "success", &result)
        .await
        .expect("submit verification result");
    let (ingested, _) = store
        .ingest_verification_result(&envelope_id)
        .await
        .expect("ingest verification result");
    assert_eq!(ingested, result);
    let status: String =
        sqlx::query_scalar("SELECT status FROM verification_attempts WHERE attempt_id=?")
            .bind(&offer.attempt_id)
            .fetch_one(store.pool())
            .await
            .expect("attempt status");
    assert_eq!(status, "completed");
}

#[tokio::test]
async fn startup_reconciliation_resets_interrupted_verification_without_trusting_orphaned_output() {
    let (store, _temp) = store().await;
    let (case, _formalization) = proof_search_fixture(&store).await;
    store
        .mark_verification_started(&case.verification_id)
        .await
        .expect("mark verification started");
    let offer = store
        .offer_verification_worker(
            &case.case_id,
            "math_review_1",
            "mock",
            None,
            "runtime/verifier-restart",
            &json!({"candidate":"1+1=2","active_facts":[]}),
            &[],
            &json!({"success":"one independent verdict"}),
        )
        .await
        .expect("offer verification worker");
    let lease = store
        .accept_verification_worker_handshake(&offer, Some("mock-1"), &json!({}), 3_600)
        .await
        .expect("verification lease");
    let (envelope_id, _) = store
        .submit_verification_result(
            &lease,
            "success",
            &json!({"verdict":"accepted","summary":"orphaned process output"}),
        )
        .await
        .expect("submit pending verification result");
    store
        .run_reconciliation(Some(&case.project_id), "service_startup")
        .await
        .expect("startup reconciliation");
    let verification = store
        .get_verification(&case.verification_id)
        .await
        .expect("verification");
    assert_eq!(
        verification.status,
        research_domain::CandidateStatus::Submitted
    );
    let recovered_case = store
        .get_verification_case(&case.case_id)
        .await
        .expect("recovered case");
    assert_eq!(
        recovered_case.cancellation_epoch,
        case.cancellation_epoch + 1
    );
    let envelope_status: String = sqlx::query_scalar(
        "SELECT status FROM verification_result_envelopes WHERE verification_result_envelope_id=?",
    )
    .bind(envelope_id)
    .fetch_one(store.pool())
    .await
    .expect("envelope status");
    assert_eq!(envelope_status, "stale");
    let attempt_status: String =
        sqlx::query_scalar("SELECT status FROM verification_attempts WHERE attempt_id=?")
            .bind(&offer.attempt_id)
            .fetch_one(store.pool())
            .await
            .expect("attempt status");
    assert_eq!(attempt_status, "orphaned");
}

#[tokio::test]
async fn project_creation_is_atomic_and_snapshot_has_cursor() {
    let (store, _temp) = store().await;
    let (project, event) = store
        .create_project("arithmetic".into(), contract(), Budget::default())
        .await
        .expect("create");
    assert_eq!(project.revision, 1);
    assert_eq!(event.cursor, 1);
    let snapshot = store.snapshot(&project.project_id).await.expect("snapshot");
    assert_eq!(snapshot.goals.len(), 1);
    assert_eq!(snapshot.event_cursor, 1);
    assert_eq!(snapshot.project_revision, 1);
}

#[tokio::test]
async fn command_idempotency_and_revision_conflicts_are_enforced() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("arithmetic".into(), contract(), Budget::default())
        .await
        .expect("create");
    let draft = CommandDraft {
        command_type: "start_project".into(),
        target_kind: "project".into(),
        target_id: project.project_id.clone(),
        mode: CommandMode::Immediate,
        payload: json!({}),
        expected_project_revision: 1,
        idempotency_key: "start-1".into(),
        reason: "test".into(),
        requested_by: "test".into(),
    };
    let (first, first_event) = store
        .enqueue_command(&project.project_id, draft.clone())
        .await
        .expect("enqueue");
    let (second, second_event) = store
        .enqueue_command(&project.project_id, draft)
        .await
        .expect("dedupe");
    assert_eq!(first.command_id, second.command_id);
    assert!(first_event.is_some());
    assert!(second_event.is_none());
    let stale = CommandDraft {
        command_type: "pause_project".into(),
        target_kind: "project".into(),
        target_id: project.project_id.clone(),
        mode: CommandMode::Immediate,
        payload: json!({}),
        expected_project_revision: 1,
        idempotency_key: "pause-stale".into(),
        reason: "test".into(),
        requested_by: "test".into(),
    };
    assert!(matches!(
        store.enqueue_command(&project.project_id, stale).await,
        Err(StorageError::RevisionConflict { .. })
    ));
}

#[tokio::test]
async fn candidate_submission_is_idempotent() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("arithmetic".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("begin");
    let saved = store
        .save_plan(&project.project_id, &round, &plan())
        .await
        .expect("plan");
    let (running, _) = store
        .mark_task_running(&project.project_id, &saved.tasks[0].task_id)
        .await
        .expect("running");
    store
        .record_worker_output(
            &running,
            &WorkerOutput {
                summary: "done".into(),
                discoveries: vec![],
                candidates: vec![],
                failures: vec![],
                uncertainties: vec![],
                sources: vec![],
                experiments: vec![],
            },
        )
        .await
        .expect("complete");
    let task = store
        .get_task(&project.project_id, &running.task_id)
        .await
        .expect("task");
    let submission = CandidateSubmission {
        task_id: task.task_id.clone(),
        route_id: task.route_id.clone(),
        target_goal_ids: task.goal_ids.clone(),
        statement: "1+1=2".into(),
        assumptions: vec![],
        proof_markdown: "direct calculation".into(),
        dependency_fact_ids: vec![],
        definitions_introduced: Default::default(),
        external_source_ids: vec![],
        candidate_type: CandidateType::Theorem,
        task_revision: task.revision,
        route_cancellation_epoch: task.route_cancellation_epoch,
    };
    let first = store
        .submit_candidate(&project.project_id, submission.clone(), "candidate-1")
        .await
        .expect("first");
    let second = store
        .submit_candidate(&project.project_id, submission, "candidate-1")
        .await
        .expect("second");
    assert_eq!(first.candidate.candidate_id, second.candidate.candidate_id);
    assert!(first.event.is_some());
    assert!(second.event.is_none());
}

#[tokio::test]
async fn route_stop_rejects_late_worker_submission_by_epoch() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("arithmetic".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("begin");
    let saved = store
        .save_plan(&project.project_id, &round, &plan())
        .await
        .expect("plan");
    let task = &saved.tasks[0];
    let current = store
        .get_project(&project.project_id)
        .await
        .expect("current");
    let (command, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "stop_route".into(),
                target_kind: "route".into(),
                target_id: task.route_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: current.revision,
                idempotency_key: "stop-route".into(),
                reason: "test".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue stop");
    store
        .apply_command(&project.project_id, &command.command_id)
        .await
        .expect("apply stop");
    let submission = CandidateSubmission {
        task_id: task.task_id.clone(),
        route_id: task.route_id.clone(),
        target_goal_ids: task.goal_ids.clone(),
        statement: "late".into(),
        assumptions: vec![],
        proof_markdown: "late proof".into(),
        dependency_fact_ids: vec![],
        definitions_introduced: Default::default(),
        external_source_ids: vec![],
        candidate_type: CandidateType::Lemma,
        task_revision: task.revision,
        route_cancellation_epoch: task.route_cancellation_epoch,
    };
    assert!(matches!(
        store
            .submit_candidate(&project.project_id, submission, "late")
            .await,
        Err(StorageError::LateSubmission(_))
    ));
}

#[tokio::test]
async fn recovery_interrupts_unfinished_round_and_resets_runtime_state() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("arithmetic".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("begin");
    let events = store
        .recover_project(&project.project_id)
        .await
        .expect("recover");
    assert_eq!(events.len(), 1);
    let recovered = store
        .current_round(&project.project_id)
        .await
        .expect("round")
        .expect("exists");
    assert_eq!(recovered.round_id, round.round_id);
    assert_eq!(recovered.status.to_string(), "interrupted");
}

#[tokio::test]
async fn planning_stage_attempt_records_soft_budget_and_terminal_state() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("planning-attempt".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("begin");
    let (attempt_id, started) = store
        .begin_planning_stage_attempt(
            &project.project_id,
            &round.round_id,
            "route_generator",
            "input-hash",
            1,
            (1, 2),
        )
        .await
        .expect("attempt");
    assert!(started.is_some());
    let initial_heartbeat: String = sqlx::query_scalar(
        "SELECT last_heartbeat_at FROM planning_stage_attempts WHERE attempt_id=?",
    )
    .bind(&attempt_id)
    .fetch_one(store.pool())
    .await
    .expect("initial heartbeat");
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    store
        .heartbeat_planning_stage_attempt(&attempt_id)
        .await
        .expect("heartbeat");
    let refreshed_heartbeat: String = sqlx::query_scalar(
        "SELECT last_heartbeat_at FROM planning_stage_attempts WHERE attempt_id=?",
    )
    .bind(&attempt_id)
    .fetch_one(store.pool())
    .await
    .expect("refreshed heartbeat");
    assert!(refreshed_heartbeat > initial_heartbeat);
    assert!(
        store
            .mark_planning_stage_soft_budget_exceeded(&attempt_id)
            .await
            .expect("soft budget")
            .is_some()
    );
    store
        .complete_planning_stage_attempt(&attempt_id, true, None)
        .await
        .expect("complete");
    let row = sqlx::query(
        "SELECT status,soft_budget_exceeded,completed_at FROM planning_stage_attempts WHERE attempt_id=?",
    )
    .bind(&attempt_id)
    .fetch_one(store.pool())
    .await
    .expect("attempt row");
    assert_eq!(row.get::<String, _>("status"), "completed");
    assert_eq!(row.get::<i64, _>("soft_budget_exceeded"), 1);
    assert!(row.get::<Option<String>, _>("completed_at").is_some());
}

#[tokio::test]
async fn strategy_state_is_immutable_idempotent_and_queryable() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("strategy-state".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("begin");
    let strategy = StrategyDirectorOutput {
        verdict_summary: "The goal is open.".into(),
        fixed_goal: "1+1=2".into(),
        proof_skeleton: vec!["Close the exact target.".into()],
        route_portfolio: vec![StrategyRouteState {
            title: "direct".into(),
            mechanism: "definition".into(),
            mathematical_frontier: "target".into(),
            decisive_obstacle: "checked derivation".into(),
            evidence_for: vec![],
            evidence_against: vec![],
            status: "active".into(),
            revisit_condition: "if rejected".into(),
        }],
        interface_debts: vec![StrategyInterfaceDebt {
            interface_name: "definition-to-target".into(),
            input_required: "definition".into(),
            output_available: "goal".into(),
            missing_matches: vec!["derivation".into()],
            failure_if_ignored: "unsupported equality".into(),
            affected_goal_ids: vec![],
        }],
        central_missing_bridge: "checked derivation".into(),
        method_vs_proposition_failure: "undetermined".into(),
        dangerous_shortcuts: vec![],
        strategy_directives: vec!["prove the exact target".into()],
        literature_priorities: vec![],
        macro_replan_required: false,
    };
    let (first, first_event) = store
        .record_strategy_state(
            &project.project_id,
            &round.round_id,
            "strategy-input",
            "initial",
            &["initial_strategy_state".into()],
            &strategy,
        )
        .await
        .expect("first strategy state");
    let (replayed, replay_event) = store
        .record_strategy_state(
            &project.project_id,
            &round.round_id,
            "strategy-input",
            "initial",
            &["initial_strategy_state".into()],
            &strategy,
        )
        .await
        .expect("replayed strategy state");
    assert!(first_event.is_some());
    assert!(replay_event.is_none());
    assert_eq!(first["strategy_state_id"], replayed["strategy_state_id"]);
    assert_eq!(first["trigger_reasons"][0], "initial_strategy_state");
    assert_eq!(
        store
            .latest_strategy_state(&project.project_id)
            .await
            .expect("latest")
            .expect("state")["central_missing_bridge"],
        "checked derivation"
    );
    assert_eq!(
        store
            .list_strategy_states(&project.project_id)
            .await
            .expect("states")
            .len(),
        1
    );
}

#[tokio::test]
async fn source_leads_are_untrusted_and_repeated_failures_are_compressed() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("arithmetic".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("begin");
    let saved = store
        .save_plan(&project.project_id, &round, &plan())
        .await
        .expect("plan");
    let (running, _) = store
        .mark_task_running(&project.project_id, &saved.tasks[0].task_id)
        .await
        .expect("running");
    let failure = || FailureDraft {
        failure_type: "boundary_case".into(),
        summary: "the same boundary obstruction".into(),
        repairable: true,
    };
    let output_events = store
        .record_worker_output(
            &running,
            &WorkerOutput {
                summary: "reported evidence".into(),
                discoveries: vec![],
                candidates: vec![],
                failures: vec![failure(), failure(), failure()],
                uncertainties: vec![],
                sources: vec![
                    SourceDraft {
                        title: "A claimed reference".into(),
                        authors: vec!["A. Author".into()],
                        url: Some("https://example.invalid/paper".into()),
                        citation_key: None,
                        theorem_reference: Some("Theorem 1".into()),
                        statement_excerpt: Some("A statement".into()),
                        assumptions: vec![],
                        applicability: "possibly relevant".into(),
                        status: "reported".into(),
                        retrieval_query: None,
                        document_version: None,
                        fulltext_path: None,
                        fulltext_sha256: None,
                        fulltext_artifact_id: None,
                    },
                    SourceDraft {
                        title: "Unlocatable reference".into(),
                        authors: vec![],
                        url: None,
                        citation_key: None,
                        theorem_reference: None,
                        statement_excerpt: None,
                        assumptions: vec![],
                        applicability: "could not inspect".into(),
                        status: "reported".into(),
                        retrieval_query: None,
                        document_version: None,
                        fulltext_path: None,
                        fulltext_sha256: None,
                        fulltext_artifact_id: None,
                    },
                    SourceDraft {
                        title: "The same reference, reported twice".into(),
                        authors: vec!["A. Author".into()],
                        url: Some("https://example.invalid/paper/".into()),
                        citation_key: None,
                        theorem_reference: Some("Theorem 1".into()),
                        statement_excerpt: Some("A statement".into()),
                        assumptions: vec![],
                        applicability: "duplicate lead".into(),
                        status: "reported".into(),
                        retrieval_query: None,
                        document_version: None,
                        fulltext_path: None,
                        fulltext_sha256: None,
                        fulltext_artifact_id: None,
                    },
                    SourceDraft {
                        title: "Spoofed full text".into(),
                        authors: vec![],
                        url: Some("https://example.invalid/other".into()),
                        citation_key: None,
                        theorem_reference: Some("Theorem 2".into()),
                        statement_excerpt: Some("B".into()),
                        assumptions: vec![],
                        applicability: "claimed exact".into(),
                        status: "reported".into(),
                        retrieval_query: Some("B".into()),
                        document_version: Some("v1".into()),
                        fulltext_path: Some("other.pdf".into()),
                        fulltext_sha256: Some("00".repeat(32)),
                        fulltext_artifact_id: Some("artifact-spoof".into()),
                    },
                ],
                experiments: vec![ExperimentDraft {
                    language: "python".into(),
                    program_text: "print(2 + 2)".into(),
                    input: json!({}),
                    environment: json!({"python": "3.x"}),
                    stdout: "4\n".into(),
                    stderr: String::new(),
                    exit_code: Some(0),
                    artifacts: vec![],
                    conclusion_mapping: json!({"supports": "a finite sanity check only"}),
                    replay_command: vec!["python".into(), "capsule.py".into()],
                }],
            },
        )
        .await
        .expect("record output");
    assert!(
        output_events
            .iter()
            .any(|event| event.event_type == "source.draft.rejected")
    );
    let sources = store
        .list_sources(&project.project_id)
        .await
        .expect("sources");
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].status, "reported_unverified");
    assert_eq!(
        sources[0].normalized_url.as_deref(),
        Some("https://example.invalid/paper")
    );
    assert_eq!(
        sources[0].origin_task_id.as_deref(),
        Some(running.task_id.as_str())
    );
    assert_eq!(sources[0].provenance["trust"], "reported_unverified");
    let dispositions = sqlx::query_scalar::<_, String>(
        "SELECT disposition FROM source_ingestion_records WHERE project_id=? ORDER BY created_at,ingestion_id",
    )
    .bind(&project.project_id)
    .fetch_all(store.pool())
    .await
    .expect("source ingestion records");
    assert_eq!(dispositions.len(), 4);
    assert!(dispositions.iter().any(|value| value == "inserted"));
    assert!(dispositions.iter().any(|value| value == "rejected"));
    assert!(dispositions.iter().any(|value| value == "duplicate"));
    let capsules = store
        .list_experiment_capsules(&project.project_id)
        .await
        .expect("capsules");
    assert_eq!(capsules.len(), 1);
    assert_eq!(capsules[0].status, "reported_unverified");
    assert_eq!(capsules[0].stdout, "4\n");
    assert_eq!(capsules[0].content_hash.len(), 64);
    assert_eq!(
        store
            .get_experiment_capsule(&capsules[0].capsule_id)
            .await
            .expect("capsule")
            .content_hash,
        capsules[0].content_hash
    );
    let events = store
        .compress_failures(&project.project_id, 3)
        .await
        .expect("compress");
    assert_eq!(events.len(), 1);
    let patterns = store
        .list_failure_patterns(&project.project_id)
        .await
        .expect("patterns");
    assert_eq!(patterns.len(), 1);
    assert_eq!(
        patterns[0]
            .get("occurrences")
            .and_then(serde_json::Value::as_i64),
        Some(3)
    );
}

#[tokio::test]
async fn model_call_reservations_enforce_project_and_task_budgets() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("budget".into(), contract(), Budget::default())
        .await
        .expect("create");
    let first = store
        .reserve_model_call(ModelCallRequest {
            project_id: &project.project_id,
            round: 1,
            worker_id: Some("worker-1"),
            task_id: Some("task-1"),
            model: None,
            max_total_calls: 2,
            max_task_calls: 1,
        })
        .await
        .expect("first reservation");
    store
        .finish_model_call(&first.usage_id, 12)
        .await
        .expect("finish");
    assert!(matches!(
        store
            .reserve_model_call(ModelCallRequest {
                project_id: &project.project_id,
                round: 1,
                worker_id: Some("worker-1"),
                task_id: Some("task-1"),
                model: None,
                max_total_calls: 2,
                max_task_calls: 1,
            },)
            .await,
        Err(StorageError::BudgetExhausted(_))
    ));
    store
        .reserve_model_call(ModelCallRequest {
            project_id: &project.project_id,
            round: 1,
            worker_id: Some("worker-2"),
            task_id: Some("task-2"),
            model: None,
            max_total_calls: 2,
            max_task_calls: 1,
        })
        .await
        .expect("second project call");
    assert_eq!(
        store
            .total_model_calls(&project.project_id)
            .await
            .expect("usage"),
        2
    );
    assert!(matches!(
        store
            .reserve_model_call(ModelCallRequest {
                project_id: &project.project_id,
                round: 1,
                worker_id: Some("worker-3"),
                task_id: Some("task-3"),
                model: None,
                max_total_calls: 2,
                max_task_calls: 1,
            },)
            .await,
        Err(StorageError::BudgetExhausted(_))
    ));
}

#[tokio::test]
async fn fact_governance_is_idempotent_and_cascades_dependency_invalidation() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("governance".into(), contract(), Budget::default())
        .await
        .expect("create");
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("INSERT INTO candidates(candidate_id,project_id,submission_json,status,created_at) VALUES('candidate-a',?,?,'promoted_to_fact',?)")
        .bind(&project.project_id)
        .bind(json!({
            "task_id":"task-a","route_id":"route-source","target_goal_ids":[],"statement":"A",
            "assumptions":[],"proof_markdown":"proof A","dependency_fact_ids":[],
            "definitions_introduced":{},"external_source_ids":[],"candidate_type":"lemma",
            "task_revision":1,"route_cancellation_epoch":0
        }).to_string())
        .bind(&now)
        .execute(store.pool()).await.expect("candidate");
    sqlx::query("INSERT INTO verifications(verification_id,candidate_id,project_id,status,report_json,started_at,completed_at) VALUES('verification-a','candidate-a',?,'accepted',NULL,?,?)")
        .bind(&project.project_id).bind(&now).bind(&now).execute(store.pool()).await.expect("verification");
    sqlx::query("INSERT INTO facts(fact_id,project_id,statement,assumptions_json,proof_markdown,dependency_fact_ids_json,definitions_introduced_json,external_source_ids_json,verification_ids_json,evidence_level,created_by,status,content_hash,created_at) VALUES('fact-a',?,'A','[]','proof A','[]','{}','[]','[\"verification-a\"]','reviewed','test','active','hash-a',?)")
        .bind(&project.project_id).bind(&now).execute(store.pool()).await.expect("fact a");
    sqlx::query("INSERT INTO facts(fact_id,project_id,statement,assumptions_json,proof_markdown,dependency_fact_ids_json,definitions_introduced_json,external_source_ids_json,verification_ids_json,evidence_level,created_by,status,content_hash,created_at) VALUES('fact-b',?,'B','[]','proof B','[\"fact-a\"]','{}','[]','[]','reviewed','test','active','hash-b',?)")
        .bind(&project.project_id).bind(&now).execute(store.pool()).await.expect("fact b");
    sqlx::query("INSERT INTO fact_edges(edge_id,project_id,source_id,target_id,kind) VALUES('edge-a-b',?,'fact-a','fact-b','depends_on')")
        .bind(&project.project_id).execute(store.pool()).await.expect("edge");
    let goal_id: String =
        sqlx::query_scalar("SELECT goal_id FROM goals WHERE project_id=? LIMIT 1")
            .bind(&project.project_id)
            .fetch_one(store.pool())
            .await
            .expect("goal");
    sqlx::query("UPDATE goals SET status='solved',solved_by_fact_id='fact-b' WHERE goal_id=?")
        .bind(&goal_id)
        .execute(store.pool())
        .await
        .expect("solve goal");
    sqlx::query("INSERT INTO routes(route_id,project_id,title,method_summary,target_goal_ids_json,required_fact_ids_json,status,score,priority,cancellation_epoch,created_in_round,attributes_json) VALUES('route-dependent',?,'dependent','test',?, '[\"fact-b\"]','active',1.0,1.0,0,1,'{}')")
        .bind(&project.project_id).bind(json!([goal_id]).to_string()).execute(store.pool()).await.expect("route");

    let (challenged, challenge_events) = store
        .govern_fact("fact-a", "challenge", "audit", "reviewer", "challenge-1")
        .await
        .expect("challenge");
    assert_eq!(
        challenged.fact.status,
        research_domain::FactStatus::Challenged
    );
    assert!(challenged.verification_id.is_some());
    assert_eq!(challenge_events.len(), 2);
    let (same, repeated_events) = store
        .govern_fact("fact-a", "challenge", "audit", "reviewer", "challenge-1")
        .await
        .expect("idempotent challenge");
    assert_eq!(same.verification_id, challenged.verification_id);
    assert!(repeated_events.is_empty());
    let (formalization, formalization_events) = store
        .govern_fact(
            "fact-a",
            "request_formalization",
            "raise assurance",
            "reviewer",
            "formalize-1",
        )
        .await
        .expect("request formalization");
    assert!(formalization.verification_id.is_some());
    assert!(
        formalization_events
            .iter()
            .any(|event| { event.event_type == "fact.formalization_requested" })
    );
    let (independent, independent_events) = store
        .govern_fact(
            "fact-a",
            "request_independent_proof",
            "independent confirmation",
            "reviewer",
            "independent-proof-1",
        )
        .await
        .expect("request independent proof");
    assert!(independent.verification_id.is_some());
    assert!(
        independent_events
            .iter()
            .any(|event| { event.event_type == "fact.independent_proof_requested" })
    );
    let governance_history = store
        .list_fact_challenges("fact-a")
        .await
        .expect("challenge history");
    assert!(
        governance_history
            .iter()
            .any(|item| item.kind == "request_formalization")
    );
    assert!(
        governance_history
            .iter()
            .any(|item| item.kind == "request_independent_proof")
    );

    let (revoked, revoke_events) = store
        .govern_fact("fact-a", "revoke", "failed audit", "reviewer", "revoke-1")
        .await
        .expect("revoke");
    assert_eq!(revoked.fact.status, research_domain::FactStatus::Revoked);
    assert_eq!(revoked.impact.affected_fact_ids, vec!["fact-b"]);
    assert_eq!(
        store.get_fact("fact-b").await.expect("fact b").status,
        research_domain::FactStatus::Suspended
    );
    assert_eq!(
        store.list_goals(&project.project_id).await.expect("goals")[0].status,
        research_domain::GoalStatus::Open
    );
    assert_eq!(
        store
            .get_route(&project.project_id, "route-dependent")
            .await
            .expect("route")
            .status,
        research_domain::RouteStatus::Paused
    );
    assert_eq!(
        store
            .get_project(&project.project_id)
            .await
            .expect("project")
            .status,
        research_domain::ProjectStatus::NeedsHumanReview
    );
    assert_eq!(
        store
            .get_verification(
                challenged
                    .verification_id
                    .as_deref()
                    .expect("verification id")
            )
            .await
            .expect("cancelled reverification")
            .status,
        research_domain::CandidateStatus::Superseded
    );
    assert_eq!(revoke_events.len(), 1);
}

#[tokio::test]
async fn p1_control_plane_applies_steering_questions_and_budget_guards() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("control".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let saved = store
        .save_plan(&project.project_id, &round, &plan())
        .await
        .expect("plan");
    let (running, _) = store
        .mark_task_running(&project.project_id, &saved.tasks[0].task_id)
        .await
        .expect("running");
    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project")
        .revision;
    let (steer_command, _) = store.enqueue_command(&project.project_id, CommandDraft {
        command_type: "steer_task".into(), target_kind: "task".into(), target_id: running.task_id.clone(),
        mode: CommandMode::SafePoint,
        payload: json!({"content":"try the invariant first","expected_task_revision":running.revision,"expected_route_epoch":running.route_cancellation_epoch}),
        expected_project_revision: revision, idempotency_key: "steer-1".into(), reason: "human insight".into(), requested_by: "researcher".into(),
    }).await.expect("enqueue steer");
    let (waiting, events) = store
        .apply_command(&project.project_id, &steer_command.command_id)
        .await
        .expect("queue steer");
    assert_eq!(
        waiting.status,
        research_domain::CommandStatus::WaitingSafePoint
    );
    assert_eq!(events[0].event_type, "task.steer.queued");
    let (steers, events) = store
        .consume_pending_task_steers(
            &project.project_id,
            &running.task_id,
            running.revision,
            running.route_cancellation_epoch,
        )
        .await
        .expect("safe point");
    assert_eq!(steers.len(), 1);
    assert_eq!(steers[0].content, "try the invariant first");
    assert_eq!(events[0].event_type, "task.steer.applied");
    assert_eq!(
        store
            .get_command(&project.project_id, &steer_command.command_id)
            .await
            .expect("command")
            .status,
        research_domain::CommandStatus::Applied
    );

    let (question, _) = store
        .create_human_question(
            &project.project_id,
            "Which convention?",
            vec![json!("left"), json!("right")],
            vec![running.task_id.clone()],
            "alignment_reviewer",
            None,
        )
        .await
        .expect("question");
    assert_eq!(
        store
            .get_project(&project.project_id)
            .await
            .expect("project")
            .status,
        research_domain::ProjectStatus::NeedsHumanReview
    );
    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project")
        .revision;
    let (answer_command, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "answer_question".into(),
                target_kind: "human_question".into(),
                target_id: question.question_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({"answer":"left"}),
                expected_project_revision: revision,
                idempotency_key: "answer-1".into(),
                reason: "clarify".into(),
                requested_by: "researcher".into(),
            },
        )
        .await
        .expect("enqueue answer");
    store
        .apply_command(&project.project_id, &answer_command.command_id)
        .await
        .expect("answer");
    assert_eq!(
        store
            .list_human_questions(&project.project_id)
            .await
            .expect("questions")[0]
            .status,
        "answered"
    );

    let reservation = store
        .reserve_model_call(ModelCallRequest {
            project_id: &project.project_id,
            round: 1,
            worker_id: None,
            task_id: None,
            model: Some("test"),
            max_total_calls: 10,
            max_task_calls: 10,
        })
        .await
        .expect("usage");
    store
        .finish_model_call_with_tokens(&reservation.usage_id, 10, 123, 45)
        .await
        .expect("finish");
    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project")
        .revision;
    let (budget_command, _) = store.enqueue_command(&project.project_id, CommandDraft {
        command_type: "adjust_budget".into(), target_kind: "project".into(), target_id: project.project_id.clone(), mode: CommandMode::Immediate,
        payload: json!({"scope_kind":"project","scope_id":project.project_id,"limits":{"max_rounds":1,"max_parallel_workers":1,"max_minutes_per_task":1,"max_model_calls_per_task":1,"max_total_model_calls":0}}),
        expected_project_revision: revision, idempotency_key: "budget-too-low".into(), reason: "test".into(), requested_by: "operator".into(),
    }).await.expect("enqueue budget");
    assert!(matches!(
        store
            .apply_command(&project.project_id, &budget_command.command_id)
            .await,
        Err(StorageError::BudgetExhausted(_))
    ));
    let usage = store
        .usage_summary(&project.project_id)
        .await
        .expect("usage summary");
    assert_eq!(usage.model_calls, 1);
    assert_eq!(usage.input_tokens, 123);
    assert_eq!(usage.output_tokens, 45);
    assert_eq!(usage.estimated_cost_usd, None);
}

async fn proof_search_fixture(
    store: &SqliteStore,
) -> (
    research_domain::VerificationCase,
    research_domain::Formalization,
) {
    let (project, _) = store
        .create_project("proof search".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let saved = store
        .save_plan(&project.project_id, &round, &plan())
        .await
        .expect("plan");
    let (running, _) = store
        .mark_task_running(&project.project_id, &saved.tasks[0].task_id)
        .await
        .expect("running");
    store
        .record_worker_output(
            &running,
            &WorkerOutput {
                summary: "done".into(),
                discoveries: vec![],
                candidates: vec![],
                failures: vec![],
                uncertainties: vec![],
                sources: vec![],
                experiments: vec![],
            },
        )
        .await
        .expect("output");
    let task = store
        .get_task(&project.project_id, &running.task_id)
        .await
        .expect("task");
    let receipt = store
        .submit_candidate(
            &project.project_id,
            CandidateSubmission {
                task_id: task.task_id.clone(),
                route_id: task.route_id.clone(),
                target_goal_ids: task.goal_ids.clone(),
                statement: "1+1=2".into(),
                assumptions: vec![],
                proof_markdown: "calculation".into(),
                dependency_fact_ids: vec![],
                definitions_introduced: Default::default(),
                external_source_ids: vec![],
                candidate_type: CandidateType::Theorem,
                task_revision: task.revision,
                route_cancellation_epoch: task.route_cancellation_epoch,
            },
            "proof-search-candidate",
        )
        .await
        .expect("candidate");
    let (case, _) = store
        .ensure_verification_case(
            &receipt.verification.verification_id,
            VerificationCaseDraft {
                name: "formal".into(),
                profile: VerificationProfile::FormalRequired,
                required_acceptance: AcceptanceClass::FormallyVerified,
                required_checks: vec![],
                independent_reviewer_count: 2,
                require_citation_review: false,
                require_adversarial_review: true,
                require_alignment_review: true,
                require_fresh_replay: true,
                max_attempts: 2,
                risk_score: 0.9,
                risk_reasons: vec!["test".into()],
            },
        )
        .await
        .expect("case");
    let (semantic, _) = store
        .store_semantic_contract(
            &case.case_id,
            "1+1=2",
            SemanticContractDraft {
                variables: vec![],
                assumptions: vec![],
                conclusion: "1+1=2".into(),
                definitions: vec![],
                boundary_conditions: vec![],
                ambiguity_notes: vec![],
            },
        )
        .await
        .expect("semantic");
    let output = FormalizerOutput {
        semantic_contract: SemanticContractDraft {
            variables: vec![],
            assumptions: vec![],
            conclusion: "1+1=2".into(),
            definitions: vec![],
            boundary_conditions: vec![],
            ambiguity_notes: vec![],
        },
        theorem_name: "one_add_one".into(),
        lean_statement: "(1:Nat)+1=2".into(),
        lean_source: "import Mathlib\ntheorem one_add_one : (1:Nat)+1=2 := by norm_num".into(),
        mapping: vec![],
    };
    let (formalization, _) = store
        .store_formalization(&case.case_id, &semantic.contract_id, &output)
        .await
        .expect("formalization");
    (case, formalization)
}

#[tokio::test]
async fn proof_hint_is_consumed_by_the_next_matching_expansion() {
    let (store, _temp) = store().await;
    let (case, formalization) = proof_search_fixture(&store).await;
    let (search, root, _) = store
        .create_proof_search(
            &case.case_id,
            &formalization.formalization_id,
            "best_first",
            ProofSearchBudget::default(),
            ProofNodeDraft {
                parent_node_id: None,
                state_id: Some(0),
                goal: "⊢ (1:Nat)+1=2".into(),
                local_context: json!([]),
                tactic: None,
                score: 1.0,
                status: ProofNodeStatus::Open,
                diagnostic: None,
            },
        )
        .await
        .expect("search");
    let (hint, _) = store
        .add_proof_hint(
            &search.search_id,
            Some(&root.node_id),
            "use_lemma: Nat.add_comm",
            "human",
            "hint-1",
        )
        .await
        .expect("hint");
    assert_eq!(hint.effective_after_expansion, 1);
    let (deduplicated, duplicate_event) = store
        .add_proof_hint(
            &search.search_id,
            Some(&root.node_id),
            "use_lemma: Nat.add_comm",
            "human",
            "hint-1",
        )
        .await
        .expect("deduplicated hint");
    assert_eq!(deduplicated.hint_id, hint.hint_id);
    assert!(duplicate_event.is_none());
    assert!(matches!(
        store
            .add_proof_hint(
                &search.search_id,
                Some(&root.node_id),
                "avoid_tactic: simp",
                "human",
                "hint-1"
            )
            .await,
        Err(StorageError::InvalidTransition(_))
    ));
    let consumed = store
        .consume_proof_hints(&search.search_id, &root.node_id, 0)
        .await
        .expect("consume");
    assert_eq!(consumed.len(), 1);
    assert_eq!(consumed[0].content, "use_lemma: Nat.add_comm");
    assert!(
        store
            .consume_proof_hints(&search.search_id, &root.node_id, 0)
            .await
            .expect("consume again")
            .is_empty()
    );
}

#[tokio::test]
async fn pruned_branch_rejects_late_results_but_sibling_can_continue() {
    let (store, _temp) = store().await;
    let (case, formalization) = proof_search_fixture(&store).await;
    let (search, root, _) = store
        .create_proof_search(
            &case.case_id,
            &formalization.formalization_id,
            "best_first",
            ProofSearchBudget::default(),
            ProofNodeDraft {
                parent_node_id: None,
                state_id: Some(0),
                goal: "root".into(),
                local_context: json!([]),
                tactic: None,
                score: 1.0,
                status: ProofNodeStatus::Open,
                diagnostic: None,
            },
        )
        .await
        .expect("search");
    let (branch, _, _) = store
        .add_proof_node(
            &search.search_id,
            0,
            ProofNodeDraft {
                parent_node_id: Some(root.node_id.clone()),
                state_id: Some(1),
                goal: "branch".into(),
                local_context: json!([]),
                tactic: Some("intro".into()),
                score: 0.8,
                status: ProofNodeStatus::Open,
                diagnostic: None,
            },
        )
        .await
        .expect("branch");
    let (pruned, _) = store
        .prune_proof_branch(&search.search_id, &branch.node_id, 0, "human", "prune-1")
        .await
        .expect("prune");
    assert_eq!(pruned.cancellation_epoch, 1);
    assert!(matches!(
        store
            .add_proof_node(
                &search.search_id,
                0,
                ProofNodeDraft {
                    parent_node_id: Some(branch.node_id),
                    state_id: Some(2),
                    goal: "late".into(),
                    local_context: json!([]),
                    tactic: Some("simp".into()),
                    score: 0.7,
                    status: ProofNodeStatus::Open,
                    diagnostic: None,
                }
            )
            .await,
        Err(StorageError::LateSubmission(_))
    ));
    store
        .add_proof_node(
            &search.search_id,
            1,
            ProofNodeDraft {
                parent_node_id: Some(root.node_id),
                state_id: Some(3),
                goal: "sibling".into(),
                local_context: json!([]),
                tactic: Some("rfl".into()),
                score: 0.9,
                status: ProofNodeStatus::Open,
                diagnostic: None,
            },
        )
        .await
        .expect("sibling continues");
}

#[tokio::test]
async fn actor_tokens_authenticate_and_security_audit_is_append_only() {
    let (store, _temp) = store().await;
    let admin = store
        .bootstrap_admin("admin", "Administrator", "admin-token-123456789")
        .await
        .expect("bootstrap");
    assert_eq!(admin.role, "admin");
    assert!(matches!(
        store
            .bootstrap_admin("second", "Second", "second-token-123456")
            .await,
        Err(StorageError::InvalidTransition(_))
    ));
    let researcher = store
        .create_actor(
            "researcher",
            "Researcher",
            "researcher",
            "research-token-123456",
        )
        .await
        .expect("create actor");
    assert_eq!(
        store
            .authenticate_actor("researcher", "research-token-123456")
            .await
            .expect("authenticate")
            .actor_id,
        researcher.actor_id
    );
    assert!(matches!(
        store.authenticate_actor("researcher", "wrong-token").await,
        Err(StorageError::InvalidTransition(_))
    ));
    let (project, _) = store
        .create_project("audit".into(), contract(), Budget::default())
        .await
        .expect("project");
    store
        .record_security_audit(
            Some(&project.project_id),
            Some(&researcher.actor_id),
            "read_project",
            "project",
            &project.project_id,
            "allowed",
            None,
            Some("request-hash"),
        )
        .await
        .expect("audit");
    let audit = store
        .list_security_audit(&project.project_id)
        .await
        .expect("list audit");
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].decision, "allowed");
}

#[tokio::test]
async fn sqlite_state_writer_serializes_concurrent_domain_writes_without_busy_errors() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("writer-concurrency".into(), contract(), Budget::default())
        .await
        .expect("project");
    let mut writers = Vec::new();
    for index in 0..64 {
        let store = store.clone();
        let project_id = project.project_id.clone();
        writers.push(tokio::spawn(async move {
            store
                .record_security_audit(
                    Some(&project_id),
                    None,
                    "concurrent_write_probe",
                    "project",
                    &project_id,
                    "allowed",
                    Some(&format!("probe-{index}")),
                    None,
                )
                .await
        }));
    }
    for writer in writers {
        writer
            .await
            .expect("writer task")
            .expect("serialized write");
    }
    assert_eq!(
        store
            .list_security_audit(&project.project_id)
            .await
            .expect("audit rows")
            .len(),
        64
    );
    let health = store.storage_health().await.expect("storage health");
    assert_eq!(health.sqlite_busy_total, 0);
    assert_eq!(health.state_writer_queue_depth, 0);
    assert!(health.state_writer_admitted_total >= 65);
}

#[tokio::test]
async fn distributed_task_leases_reject_stale_epochs_and_commit_exactly_once() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("distributed".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let saved = store
        .save_plan(&project.project_id, &round, &plan())
        .await
        .expect("plan");
    let task_id = &saved.tasks[0].task_id;
    let node = store
        .register_worker_node(
            "node-1",
            "Test node",
            json!({"roles":["prover"],"runtime":"test"}),
            "worker-token-123456789",
        )
        .await
        .expect("register node");
    assert!(matches!(
        store
            .heartbeat_worker_node("node-1", "worker-token-123456789", node.node_epoch + 1)
            .await,
        Err(StorageError::InvalidTransition(_))
    ));
    let (leased, events) = store
        .lease_next_task(
            &project.project_id,
            "node-1",
            "worker-token-123456789",
            node.node_epoch,
            60,
        )
        .await
        .expect("lease");
    let (lease, task) = leased.expect("leased task");
    assert_eq!(task.task_id, *task_id);
    assert_eq!(
        events.last().expect("lease event").event_type,
        "task_lease.created"
    );
    assert!(matches!(
        store
            .renew_task_lease(
                &lease.lease_id,
                "node-1",
                "worker-token-123456789",
                node.node_epoch,
                lease.lease_epoch + 1,
                60,
            )
            .await,
        Err(StorageError::LateSubmission(_))
    ));
    let output = WorkerOutput {
        summary: "remote completion".into(),
        discoveries: vec![],
        candidates: vec![],
        failures: vec![],
        uncertainties: vec![],
        sources: vec![],
        experiments: vec![],
    };
    store
        .complete_task_lease(
            &lease.lease_id,
            "node-1",
            "worker-token-123456789",
            node.node_epoch,
            lease.lease_epoch,
            &output,
        )
        .await
        .expect("complete");
    assert_eq!(
        store
            .get_task(&project.project_id, task_id)
            .await
            .expect("task")
            .status,
        research_domain::TaskStatus::Completed
    );
    assert_eq!(
        store
            .get_task_lease(&lease.lease_id)
            .await
            .expect("lease")
            .status,
        "completed"
    );
    assert!(matches!(
        store
            .complete_task_lease(
                &lease.lease_id,
                "node-1",
                "worker-token-123456789",
                node.node_epoch,
                lease.lease_epoch,
                &output,
            )
            .await,
        Err(StorageError::LateSubmission(_))
    ));
}

#[tokio::test]
async fn catalog_import_copies_dependency_closure_and_revocation_cascades() {
    let (store, _temp) = store().await;
    let (case, _formalization) = proof_search_fixture(&store).await;
    let now = chrono::Utc::now().to_rfc3339();
    for (fact_id, statement, dependencies, fact_hash, catalog_hash) in [
        (
            "catalog-base",
            "Base",
            "[]",
            "fact-base-hash",
            "catalog-base-hash",
        ),
        (
            "catalog-derived",
            "Derived",
            "[\"catalog-base\"]",
            "fact-derived-hash",
            "catalog-derived-hash",
        ),
    ] {
        sqlx::query("INSERT INTO facts(fact_id,project_id,statement,assumptions_json,proof_markdown,dependency_fact_ids_json,definitions_introduced_json,external_source_ids_json,verification_ids_json,evidence_level,created_by,status,content_hash,created_at) VALUES(?,?,?,'[]','proof',?,'{}','[]',?,'fully_certified','test','active',?,?)")
            .bind(fact_id).bind(&case.project_id).bind(statement).bind(dependencies)
            .bind(json!([case.verification_id]).to_string()).bind(fact_hash).bind(&now)
            .execute(store.pool()).await.expect("fact");
        sqlx::query("INSERT INTO fact_assurances(assurance_id,fact_id,case_id,acceptance_class,snapshot_hash,package_id,replay_id,status,created_at,invalidated_at) VALUES(?,?,?,'fully_certified',?,NULL,NULL,'active',?,NULL)")
            .bind(format!("assurance-{fact_id}")).bind(fact_id).bind(&case.case_id)
            .bind(format!("snapshot-{fact_id}")).bind(&now).execute(store.pool()).await.expect("assurance");
        sqlx::query("INSERT INTO global_fact_catalog(content_hash,source_project_id,source_fact_id,statement,status,updated_at) VALUES(?,?,?,?, 'active',?)")
            .bind(catalog_hash).bind(&case.project_id).bind(fact_id).bind(statement).bind(&now)
            .execute(store.pool()).await.expect("catalog");
    }
    sqlx::query("INSERT INTO fact_edges(edge_id,project_id,source_id,target_id,kind) VALUES('catalog-edge',?,'catalog-base','catalog-derived','depends_on')")
        .bind(&case.project_id).execute(store.pool()).await.expect("edge");
    let (target, _) = store
        .create_project("catalog target".into(), contract(), Budget::default())
        .await
        .expect("target");
    let (root_import, event) = store
        .import_catalog_fact(&target.project_id, "catalog-derived-hash", "researcher")
        .await
        .expect("import closure");
    assert_eq!(root_import.source_fact_id, "catalog-derived");
    assert!(event.is_some());
    let imports = store
        .list_fact_imports(&target.project_id)
        .await
        .expect("imports");
    assert_eq!(imports.len(), 2);
    assert!(imports.iter().all(|item| item.status == "active"));
    assert!(
        imports
            .iter()
            .any(|item| item.source_fact_id == "catalog-base")
    );

    let (_, revocation_events) = store
        .govern_fact(
            "catalog-base",
            "revoke",
            "upstream proof failed",
            "operator",
            "catalog-revoke-1",
        )
        .await
        .expect("revoke source");
    assert!(revocation_events.iter().any(|event| {
        event.project_id == target.project_id && event.event_type == "fact_import.invalidated"
    }));
    let invalidated = store
        .list_fact_imports(&target.project_id)
        .await
        .expect("invalidated imports");
    assert!(invalidated.iter().all(|item| item.status == "invalidated"));
    assert_eq!(
        store
            .get_project(&target.project_id)
            .await
            .expect("target project")
            .status,
        research_domain::ProjectStatus::NeedsHumanReview
    );
}
