use research_domain::{
    AcceptanceClass, AssignmentDraft, BoardCapabilities, Budget, CandidateDraft,
    CandidateSubmission, CandidateType, CommandMode, ExperimentDraft, FailureDraft,
    FormalizerOutput, HumanRouteProposalRequest, PlannerOutput, ProblemContract, ProofNodeStatus,
    ProofSearchBudget, RouteProposal, SemanticContractDraft, SourceDraft, StrategyDirectorOutput,
    StrategyInterfaceDebt, StrategyRouteState, SuggestionDecision, SuggestionDisposition,
    VerificationProfile, VerificationReport, VerificationStage, VerificationVerdict, WorkerOutput,
};
use serde_json::json;
use sqlx::Row;
use tempfile::TempDir;

use crate::{
    BoardInclude, CommandDraft, LocalResultSubmission, ModelCallPurpose, ModelCallRequest,
    ProofNodeDraft, SqliteStore, StorageError, TaskLeaseCompletionRequest, VerificationCaseDraft,
    VerificationSnapshotDraft, merged_route_attributes,
    reliability_v2::STARTUP_ARTIFACT_AUDIT_BATCH_SIZE,
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
async fn route_presentation_fields_round_trip_into_board_and_graph() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("route-presentation".into(), contract(), Budget::default())
        .await
        .expect("project");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    store
        .save_plan_v2(
            &project.project_id,
            &round,
            &plan(),
            &plan_scores(),
            &delta,
            "presentation round trip",
            None,
        )
        .await
        .expect("plan");
    let board = store
        .research_board(
            &project.project_id,
            20,
            BoardInclude {
                graph: true,
                ..BoardInclude::default()
            },
            BoardCapabilities {
                can_edit_problem: true,
                can_propose_route: true,
                can_create_route: true,
                can_approve_route: true,
                can_force_goal_review: true,
                can_manage_settings: true,
                can_control_project: true,
                can_control_tasks: true,
                can_govern_facts: true,
            },
        )
        .await
        .expect("board");
    let route = board
        .routes
        .iter()
        .find(|route| route.approach_kind == "direct_proof")
        .expect("direct proof route");
    assert_eq!(route.user_title, "直接证明：建立关键引理");
    assert_eq!(route.route_role, "primary");
    assert_eq!(route.steps.len(), 2);
    let graph_route = board
        .graph
        .expect("graph")
        .nodes
        .into_iter()
        .find(|node| node.id == route.route_id)
        .expect("route graph node");
    assert_eq!(graph_route.label, route.user_title);
    assert_eq!(
        graph_route
            .attributes
            .get("plain_language_summary")
            .and_then(serde_json::Value::as_str),
        Some(route.plain_language_summary.as_str())
    );
}

#[tokio::test]
async fn route_approval_releases_only_its_own_planned_tasks() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project_with_route_approval(
            "approval-task-gate".into(),
            contract(),
            Budget::default(),
            true,
        )
        .await
        .expect("project");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    let mut reviewed_plan = plan();
    reviewed_plan.assignments.push(AssignmentDraft {
        route_index: 1,
        worker_role: "counterexample_hunter".into(),
        strategic_role: "adversarial".into(),
        addresses_interface_debt: false,
        goal_ids: vec![],
        objective: "search boundary counterexamples".into(),
        completion_contract: "counterexample or exclusion report".into(),
        priority: 0.8,
    });
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &reviewed_plan,
            &reviewed_plan
                .routes
                .iter()
                .map(RouteProposal::score)
                .collect::<Vec<_>>(),
            &delta,
            "approval task gate",
            None,
        )
        .await
        .expect("plan");

    assert_eq!(saved.tasks.len(), 2, "review must not erase planned work");
    assert!(
        saved
            .tasks
            .iter()
            .all(|task| task.status == research_domain::TaskStatus::Queued)
    );
    assert_eq!(
        store
            .get_project(&project.project_id)
            .await
            .expect("project after plan")
            .status,
        research_domain::ProjectStatus::NeedsHumanReview
    );
    for task in &saved.tasks {
        let blocked = store
            .offer_local_task(
                task,
                "mock",
                None,
                "runtime/approval-gate-blocked",
                json!({}),
            )
            .await;
        assert!(
            matches!(blocked, Err(StorageError::InvalidTransition(message)) if message.contains("needs_human_review") && message.contains("pending")),
            "every unreviewed route must remain unexecutable"
        );
    }

    let approved_route = saved
        .routes
        .iter()
        .find(|route| route.title == "direct")
        .expect("direct route");
    let pending_route = saved
        .routes
        .iter()
        .find(|route| route.title == "attack")
        .expect("pending route");
    let approved_task = saved
        .tasks
        .iter()
        .find(|task| task.route_id == approved_route.route_id)
        .expect("direct task");
    let pending_task = saved
        .tasks
        .iter()
        .find(|task| task.route_id == pending_route.route_id)
        .expect("pending task");
    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project before approval")
        .revision;
    let (command, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "approve_route".into(),
                target_kind: "route".into(),
                target_id: approved_route.route_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: revision,
                idempotency_key: "approve-only-direct-route".into(),
                reason: "release only the direct route".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue approval");
    store
        .apply_command(&project.project_id, &command.command_id)
        .await
        .expect("apply approval");

    assert_eq!(
        store
            .get_project(&project.project_id)
            .await
            .expect("project after partial approval")
            .status,
        research_domain::ProjectStatus::NeedsHumanReview
    );
    let released = store
        .list_released_queued_tasks(&project.project_id, round.number)
        .await
        .expect("released tasks");
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].task_id, approved_task.task_id);

    let pending_offer = store
        .offer_local_task(
            pending_task,
            "mock",
            None,
            "runtime/pending-route-stays-blocked",
            json!({}),
        )
        .await;
    assert!(
        matches!(pending_offer, Err(StorageError::InvalidTransition(message)) if message.contains("route review is pending")),
        "a sibling route must not inherit another route's approval"
    );
    let offer = store
        .offer_local_task(
            approved_task,
            "mock",
            None,
            "runtime/approved-route-released",
            json!({}),
        )
        .await
        .expect("approved task offer");
    assert_eq!(offer.task.status, research_domain::TaskStatus::Offered);

    let reservation = store
        .reserve_model_call(ModelCallRequest {
            project_id: &project.project_id,
            round: round.number,
            worker_id: approved_task.worker_id.as_deref(),
            task_id: Some(&approved_task.task_id),
            model: None,
            purpose: ModelCallPurpose::RouteScopedResearch(&approved_route.route_id),
            max_total_calls: 10,
            max_task_calls: 2,
        })
        .await
        .expect("approved route model call");
    store
        .finish_model_call(&reservation.usage_id, 1)
        .await
        .expect("finish approved route model call");
    let pending_reservation = store
        .reserve_model_call(ModelCallRequest {
            project_id: &project.project_id,
            round: round.number,
            worker_id: pending_task.worker_id.as_deref(),
            task_id: Some(&pending_task.task_id),
            model: None,
            purpose: ModelCallPurpose::RouteScopedResearch(&pending_route.route_id),
            max_total_calls: 10,
            max_task_calls: 2,
        })
        .await;
    assert!(matches!(
        pending_reservation,
        Err(StorageError::InvalidTransition(message))
            if message.contains("needs_human_review")
    ));

    let node = store
        .register_worker_node(
            "approval-gate-remote-node",
            "Approval gate remote node",
            json!({"roles":["counterexample_hunter"]}),
            "approval-gate-remote-token",
        )
        .await
        .expect("register remote node");
    assert!(
        store
            .lease_next_task(
                &project.project_id,
                &node.node_id,
                "approval-gate-remote-token",
                node.node_epoch,
                60,
            )
            .await
            .expect("distributed approval gate")
            .0
            .is_none(),
        "a distributed worker must not lease the only remaining pending-route task"
    );

    // Simulate a durable candidate left behind by a stale process. Candidate
    // ingestion is task-revision scoped, but verification must independently
    // enforce the current route approval gate before trusting any such result.
    sqlx::query("UPDATE tasks SET status='completed',revision=revision+1 WHERE task_id=?")
        .bind(&pending_task.task_id)
        .execute(store.pool())
        .await
        .expect("stage pending-route candidate fixture");
    let pending_completed_task = store
        .get_task(&project.project_id, &pending_task.task_id)
        .await
        .expect("pending route task fixture");
    let pending_submission = CandidateSubmission {
        task_id: pending_completed_task.task_id.clone(),
        route_id: pending_completed_task.route_id.clone(),
        target_goal_ids: vec![],
        statement: "A stale result on the pending route must remain untrusted.".into(),
        assumptions: vec![],
        proof_markdown: "This fixture exists only to exercise the approval boundary.".into(),
        dependency_fact_ids: vec![],
        definitions_introduced: Default::default(),
        external_source_ids: vec![],
        candidate_type: CandidateType::Lemma,
        task_revision: pending_completed_task.revision,
        route_cancellation_epoch: pending_completed_task.route_cancellation_epoch,
    };
    let pending_candidate_id = "candidate-pending-route-verification-gate";
    let pending_verification_id = "verification-pending-route-verification-gate";
    let fixture_now = chrono::Utc::now().to_rfc3339();
    // Bypass the now-stricter ingestion boundary deliberately: this row models
    // data written by an older process, so the verification gate itself must
    // still reject it independently.
    sqlx::query("INSERT INTO candidates(candidate_id,project_id,submission_json,status,created_at,idempotency_key) VALUES(?,?,?,'submitted',?,?)")
        .bind(pending_candidate_id)
        .bind(&project.project_id)
        .bind(serde_json::to_string(&pending_submission).expect("pending submission JSON"))
        .bind(&fixture_now)
        .bind("pending-route-verification-gate")
        .execute(store.pool())
        .await
        .expect("stage pending-route candidate");
    sqlx::query("INSERT INTO verifications(verification_id,candidate_id,project_id,status) VALUES(?,?,?,'submitted')")
        .bind(pending_verification_id)
        .bind(pending_candidate_id)
        .bind(&project.project_id)
        .execute(store.pool())
        .await
        .expect("stage pending-route verification");
    assert!(
        store
            .try_mark_verification_started(pending_verification_id)
            .await
            .expect("pending verification gate")
            .is_none(),
        "a pending sibling's candidate must not enter verification"
    );
}

#[test]
fn retained_route_attribute_merge_backfills_presentation_without_erasing_provenance() {
    let mut proposal = plan().routes.remove(0);
    proposal.user_title = "直接证明：更新后的可读标题".into();
    proposal.steps.clear();
    let merged = merged_route_attributes(
        r#"{"provenance":"legacy","steps":["保留步骤一","保留步骤二"]}"#,
        &proposal,
    )
    .expect("merged attributes");
    let value: serde_json::Value = serde_json::from_str(&merged).expect("attributes json");
    assert_eq!(value["provenance"], "legacy");
    assert_eq!(value["user_title"], "直接证明：更新后的可读标题");
    assert_eq!(value["steps"], json!(["保留步骤一", "保留步骤二"]));
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
            &plan_scores(),
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
        approach_kind: if title == "attack" {
            "counterexample"
        } else {
            "direct_proof"
        }
        .into(),
        route_role: if title == "attack" {
            "adversarial"
        } else {
            "primary"
        }
        .into(),
        user_title: if title == "attack" {
            "寻找反例：检查最小对象"
        } else {
            "直接证明：建立关键引理"
        }
        .into(),
        plain_language_summary: "对当前目标执行一条可核验的数学路线。".into(),
        why_this_route: "它直接检验目标的一端。".into(),
        expected_output: "一个可独立检查的证明或反例候选。".into(),
        relation_to_goal: "成功时直接推进主目标，失败时记录边界。".into(),
        steps: vec!["明确要操作的数学对象".into(), "提交可核验结果".into()],
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

fn plan_scores() -> Vec<f64> {
    plan().routes.iter().map(RouteProposal::score).collect()
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

async fn add_test_suggestion(
    store: &SqliteStore,
    project_id: &str,
    content: &str,
    idempotency_key: &str,
) -> (String, String) {
    let revision = store
        .get_project(project_id)
        .await
        .expect("project")
        .revision;
    let (command, _) = store
        .enqueue_command(
            project_id,
            CommandDraft {
                command_type: "add_suggestion".into(),
                target_kind: "project".into(),
                target_id: project_id.into(),
                mode: CommandMode::NextRound,
                payload: json!({"content":content}),
                expected_project_revision: revision,
                idempotency_key: idempotency_key.into(),
                reason: "suggestion decision regression".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue suggestion");
    store
        .apply_command(project_id, &command.command_id)
        .await
        .expect("validate suggestion");
    let suggestion_id: String = sqlx::query_scalar(
        "SELECT suggestion_id FROM suggestions WHERE command_id=? AND project_id=?",
    )
    .bind(&command.command_id)
    .bind(project_id)
    .fetch_one(store.pool())
    .await
    .expect("suggestion id");
    (suggestion_id, command.command_id)
}

async fn suggestion_case(
    name: &str,
    count: usize,
) -> (
    SqliteStore,
    TempDir,
    String,
    research_domain::ResearchRound,
    research_domain::ResearchDelta,
    Vec<(String, String)>,
) {
    let (store, temp) = store().await;
    let (project, _) = store
        .create_project(name.into(), contract(), Budget::default())
        .await
        .expect("project");
    let mut suggestions = Vec::new();
    for index in 0..count {
        suggestions.push(
            add_test_suggestion(
                &store,
                &project.project_id,
                &format!("suggestion {index}"),
                &format!("{name}-suggestion-{index}"),
            )
            .await,
        );
    }
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
    (store, temp, project.project_id, round, delta, suggestions)
}

async fn suggestion_state(
    store: &SqliteStore,
    suggestion_id: &str,
) -> (String, Option<String>, String) {
    let row = sqlx::query("SELECT s.status,s.decision,c.status AS command_status FROM suggestions s JOIN human_commands c ON c.command_id=s.command_id WHERE s.suggestion_id=?")
        .bind(suggestion_id)
        .fetch_one(store.pool())
        .await
        .expect("suggestion state");
    (
        row.try_get("status").expect("suggestion status"),
        row.try_get("decision").expect("suggestion decision"),
        row.try_get("command_status").expect("command status"),
    )
}

async fn save_v2_suggestion_plan(
    store: &SqliteStore,
    project_id: &str,
    round: &research_domain::ResearchRound,
    delta: &research_domain::ResearchDelta,
    decisions: Vec<SuggestionDecision>,
) -> crate::PlanSaveResult {
    let mut proposed_plan = plan();
    proposed_plan.suggestion_decisions = decisions;
    store
        .save_plan_v2(
            project_id,
            round,
            &proposed_plan,
            &plan_scores(),
            delta,
            "suggestion decision test",
            None,
        )
        .await
        .expect("save suggestion plan")
}

#[tokio::test]
async fn multiple_suggestions_receive_only_their_exact_typed_decisions() {
    let (store, _temp, project_id, round, delta, suggestions) =
        suggestion_case("multiple-suggestion-decisions", 2).await;
    let saved = save_v2_suggestion_plan(
        &store,
        &project_id,
        &round,
        &delta,
        vec![
            SuggestionDecision {
                suggestion_id: suggestions[1].0.clone(),
                disposition: SuggestionDisposition::Rejected,
                rationale: "conflicts with the fixed problem contract".into(),
            },
            SuggestionDecision {
                suggestion_id: suggestions[0].0.clone(),
                disposition: SuggestionDisposition::Applied,
                rationale: "implemented in the selected exact-target route".into(),
            },
        ],
    )
    .await;

    let first = suggestion_state(&store, &suggestions[0].0).await;
    let second = suggestion_state(&store, &suggestions[1].0).await;
    assert_eq!((first.0.as_str(), first.2.as_str()), ("applied", "applied"));
    assert_eq!(
        (second.0.as_str(), second.2.as_str()),
        ("rejected", "rejected")
    );
    assert!(first.1.expect("first decision").contains(&suggestions[0].0));
    assert!(
        second
            .1
            .expect("second decision")
            .contains(&suggestions[1].0)
    );
    assert_eq!(
        saved
            .events
            .iter()
            .filter(|event| event.event_type == "human_command.applied")
            .count(),
        1
    );
    assert_eq!(
        saved
            .events
            .iter()
            .filter(|event| event.event_type == "human_command.rejected")
            .count(),
        1
    );
}

#[tokio::test]
async fn missing_suggestion_decision_leaves_only_that_suggestion_pending() {
    let (store, _temp, project_id, round, delta, suggestions) =
        suggestion_case("missing-suggestion-decision", 2).await;
    let saved = save_v2_suggestion_plan(
        &store,
        &project_id,
        &round,
        &delta,
        vec![SuggestionDecision {
            suggestion_id: suggestions[0].0.clone(),
            disposition: SuggestionDisposition::Applied,
            rationale: "implemented in the selected route".into(),
        }],
    )
    .await;

    assert_eq!(
        suggestion_state(&store, &suggestions[0].0).await.0,
        "applied"
    );
    let missing = suggestion_state(&store, &suggestions[1].0).await;
    assert_eq!(
        (missing.0.as_str(), missing.2.as_str()),
        ("pending", "validated")
    );
    assert!(missing.1.is_none());
    assert!(saved.events.iter().any(|event| {
        event.event_type == "suggestion.decision_unresolved"
            && event.data["reason"] == "missing_decision"
            && event.data["suggestion_id"] == suggestions[1].0
    }));
}

#[tokio::test]
async fn duplicate_suggestion_decisions_leave_suggestion_and_command_pending() {
    let (store, _temp, project_id, round, delta, suggestions) =
        suggestion_case("duplicate-suggestion-decision", 1).await;
    let saved = save_v2_suggestion_plan(
        &store,
        &project_id,
        &round,
        &delta,
        vec![
            SuggestionDecision {
                suggestion_id: suggestions[0].0.clone(),
                disposition: SuggestionDisposition::Applied,
                rationale: "first answer".into(),
            },
            SuggestionDecision {
                suggestion_id: suggestions[0].0.clone(),
                disposition: SuggestionDisposition::Rejected,
                rationale: "conflicting second answer".into(),
            },
        ],
    )
    .await;

    let unresolved = suggestion_state(&store, &suggestions[0].0).await;
    assert_eq!(
        (unresolved.0.as_str(), unresolved.2.as_str()),
        ("pending", "validated")
    );
    assert!(unresolved.1.is_none());
    assert!(saved.events.iter().any(|event| {
        event.event_type == "suggestion.decision_unresolved"
            && event.data["reason"] == "duplicate_decision"
    }));
}

#[tokio::test]
async fn deferred_suggestion_stays_pending_for_the_next_round() {
    let (store, _temp, project_id, round, delta, suggestions) =
        suggestion_case("deferred-suggestion-decision", 1).await;
    let saved = save_v2_suggestion_plan(
        &store,
        &project_id,
        &round,
        &delta,
        vec![SuggestionDecision {
            suggestion_id: suggestions[0].0.clone(),
            disposition: SuggestionDisposition::Deferred,
            rationale: "requires a fact that is not active in this round".into(),
        }],
    )
    .await;

    let deferred = suggestion_state(&store, &suggestions[0].0).await;
    assert_eq!(
        (deferred.0.as_str(), deferred.2.as_str()),
        ("pending", "validated")
    );
    assert!(deferred.1.expect("deferred decision").contains("deferred"));
    let effective_round: i64 =
        sqlx::query_scalar("SELECT effective_round FROM suggestions WHERE suggestion_id=?")
            .bind(&suggestions[0].0)
            .fetch_one(store.pool())
            .await
            .expect("effective round");
    assert_eq!(effective_round, round.number + 1);
    assert!(saved.events.iter().any(|event| {
        event.event_type == "suggestion.deferred" && event.data["suggestion_id"] == suggestions[0].0
    }));
}

#[tokio::test]
async fn legacy_plan_unknown_suggestion_id_cannot_apply_a_pending_command() {
    let (store, _temp, project_id, round, _delta, suggestions) =
        suggestion_case("legacy-unknown-suggestion", 1).await;
    let mut proposed_plan = plan();
    proposed_plan.suggestion_decisions = vec![SuggestionDecision {
        suggestion_id: "suggestion-does-not-exist".into(),
        disposition: SuggestionDisposition::Applied,
        rationale: "must not fall back to the first pending suggestion".into(),
    }];
    let saved = store
        .save_plan(&project_id, &round, &proposed_plan)
        .await
        .expect("save legacy plan");

    let unresolved = suggestion_state(&store, &suggestions[0].0).await;
    assert_eq!(
        (unresolved.0.as_str(), unresolved.2.as_str()),
        ("pending", "validated")
    );
    assert!(unresolved.1.is_none());
    assert!(saved.events.iter().any(|event| {
        event.event_type == "suggestion.decision_unresolved"
            && event.data["reason"] == "unknown_suggestion_id"
    }));
}

async fn record_passed_verification_checks(store: &SqliteStore, case_id: &str, kinds: &[&str]) {
    for kind in kinds {
        store
            .record_verification_check(
                case_id,
                crate::CheckDraft {
                    attempt_id: None,
                    kind: (*kind).into(),
                    status: research_domain::CheckStatus::Passed,
                    mandatory: true,
                    summary: format!("{kind} passed in fixture"),
                    details: json!({}),
                },
            )
            .await
            .expect("required verification check");
    }
}

#[tokio::test]
async fn verification_policy_rejects_a_projection_that_disagrees_with_required_checks() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("policy consistency".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("INSERT INTO candidates(candidate_id,project_id,submission_json,status,created_at) VALUES('candidate-policy-drift',?,'{}','under_review',?)")
        .bind(&project.project_id)
        .bind(&now)
        .execute(store.pool())
        .await
        .expect("candidate");
    sqlx::query("INSERT INTO verifications(verification_id,candidate_id,project_id,status,started_at) VALUES('verification-policy-drift','candidate-policy-drift',?,'verifying',?)")
        .bind(&project.project_id)
        .bind(&now)
        .execute(store.pool())
        .await
        .expect("verification");

    let error = store
        .ensure_verification_case(
            "verification-policy-drift",
            VerificationCaseDraft {
                name: "inconsistent".into(),
                profile: VerificationProfile::StandardReview,
                required_acceptance: AcceptanceClass::Reviewed,
                required_checks: vec![
                    "deterministic_precheck".into(),
                    "math_review_1".into(),
                    "adversarial_review".into(),
                ],
                independent_reviewer_count: 2,
                require_citation_review: false,
                require_adversarial_review: true,
                require_alignment_review: false,
                require_fresh_replay: false,
                max_attempts: 2,
                risk_score: 0.5,
                risk_reasons: Vec::new(),
            },
        )
        .await
        .expect_err("projection drift must fail closed");
    assert!(
        matches!(error, StorageError::InvalidTransition(reason) if reason.contains("independent_reviewer_count"))
    );
    let policy_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM verification_policies WHERE project_id=?")
            .bind(&project.project_id)
            .fetch_one(store.pool())
            .await
            .expect("policy count");
    assert_eq!(policy_count, 0);
}

#[tokio::test]
async fn stopped_project_cannot_create_or_advance_verification_cases() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project(
            "verification stop fence".into(),
            contract(),
            Budget::default(),
        )
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let now = chrono::Utc::now().to_rfc3339();
    for ordinal in ["existing", "late"] {
        sqlx::query("INSERT INTO candidates(candidate_id,project_id,submission_json,status,created_at) VALUES(?,?,?,'verifying',?)")
            .bind(format!("candidate-stop-{ordinal}"))
            .bind(&project.project_id)
            .bind("{}")
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("candidate");
        sqlx::query("INSERT INTO verifications(verification_id,candidate_id,project_id,status,started_at) VALUES(?,?,?,'verifying',?)")
            .bind(format!("verification-stop-{ordinal}"))
            .bind(format!("candidate-stop-{ordinal}"))
            .bind(&project.project_id)
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("verification");
    }
    let draft = || VerificationCaseDraft {
        name: "stop-fenced-review".into(),
        profile: VerificationProfile::StandardReview,
        required_acceptance: AcceptanceClass::Reviewed,
        required_checks: vec!["deterministic_precheck".into(), "math_review_1".into()],
        independent_reviewer_count: 1,
        require_citation_review: false,
        require_adversarial_review: false,
        require_alignment_review: false,
        require_fresh_replay: false,
        max_attempts: 1,
        risk_score: 0.1,
        risk_reasons: Vec::new(),
    };
    let (existing_case, _) = store
        .ensure_verification_case("verification-stop-existing", draft())
        .await
        .expect("case before stop");

    apply_test_command(
        &store,
        &project.project_id,
        "stop_project",
        "project",
        &project.project_id,
        json!({}),
        "verification-case-stop-fence",
        "stop verification race",
    )
    .await;

    let creation_error = store
        .ensure_verification_case("verification-stop-late", draft())
        .await
        .expect_err("terminal verification must not create a late case");
    assert!(matches!(creation_error, StorageError::LateSubmission(_)));
    let late_case_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM verification_cases WHERE verification_id='verification-stop-late'",
    )
    .fetch_one(store.pool())
    .await
    .expect("late case count");
    assert_eq!(late_case_count, 0);

    assert!(
        store
            .create_verification_snapshot(
                &existing_case.case_id,
                VerificationSnapshotDraft {
                    toolchain_hash: None,
                    extra_payload: json!({}),
                },
            )
            .await
            .is_err(),
        "a case cancelled by stop must not advance to snapshotting"
    );
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
            &plan_scores(),
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
                required_checks: vec![
                    "deterministic_precheck".into(),
                    "math_review_1".into(),
                    "citation_review".into(),
                ],
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
    record_passed_verification_checks(
        &store,
        &case.case_id,
        &["deterministic_precheck", "math_review_1"],
    )
    .await;
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
async fn reviewed_semantic_equivalence_promotes_fact_without_closing_goal() {
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
            &plan_scores(),
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
    record_passed_verification_checks(
        &store,
        &case.case_id,
        &[
            "deterministic_precheck",
            "math_review_1",
            "math_review_2",
            "reviewer_independence",
            "adversarial_review",
        ],
    )
    .await;
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
    let commit = store
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
    let fact = commit.fact.expect("reviewed candidate promoted to fact");
    let assurance: String = sqlx::query_scalar(
        "SELECT acceptance_class FROM fact_assurances WHERE fact_id=? AND status='active'",
    )
    .bind(&fact.fact_id)
    .fetch_one(store.pool())
    .await
    .expect("active reviewed assurance");
    assert_eq!(assurance, "reviewed");
    store
        .complete_round(
            &project.project_id,
            &round.round_id,
            "reviewed semantic fact",
        )
        .await
        .expect("complete round");

    let snapshot = store.snapshot(&project.project_id).await.expect("snapshot");
    assert_eq!(snapshot.project.status.to_string(), "running");
    assert_eq!(snapshot.goals[0].status.to_string(), "open");
    assert!(snapshot.goals[0].solved_by_fact_id.is_none());
    let unresolved = snapshot
        .uncertainties
        .iter()
        .find(|item| item.uncertainty_id == "uncertainty-obsolete-on-closure")
        .expect("unresolved uncertainty retained for audit");
    assert_eq!(unresolved.status, research_domain::UncertaintyStatus::Open);
    assert!(unresolved.resolved_by.is_none());
    let closures = store
        .list_goal_closures(&project.project_id, &snapshot.goals[0].goal_id)
        .await
        .expect("goal closures");
    assert!(closures.is_empty());
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

async fn leased_v2_task(
    store: &SqliteStore,
    project_name: &str,
    worker_role: Option<&str>,
) -> (
    research_domain::Project,
    research_domain::ResearchRound,
    research_domain::Task,
    crate::LocalTaskLease,
) {
    let (project, _) = store
        .create_project(project_name.into(), contract(), Budget::default())
        .await
        .expect("create project");
    start_project(store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    let mut test_plan = plan();
    if let Some(worker_role) = worker_role {
        test_plan.assignments[0].worker_role = worker_role.into();
    }
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &test_plan,
            &plan_scores(),
            &delta,
            "source reference test",
            None,
        )
        .await
        .expect("plan");
    let task = saved.tasks[0].clone();
    let offer = store
        .offer_local_task(&task, "mock", None, "runtime/source-ref", json!({}))
        .await
        .expect("offer");
    let lease = store
        .accept_local_handshake(
            &offer,
            Some("source-ref-worker"),
            json!({"context_hash":offer.context_packet.content_hash}),
            60,
        )
        .await
        .expect("lease");
    (project, round, task, lease)
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
            &plan_scores(),
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
    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project")
        .revision;
    let (steer_command, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "steer_task".into(),
                target_kind: "task".into(),
                target_id: lease.task.task_id.clone(),
                mode: CommandMode::SafePoint,
                payload: json!({
                    "content":"preserve the sharp obstruction",
                    "expected_task_revision":lease.task.revision,
                    "expected_route_epoch":lease.task.route_cancellation_epoch,
                }),
                expected_project_revision: revision,
                idempotency_key: "atomic-steer-result-seal".into(),
                reason: "test atomic steer acknowledgement".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue steer");
    store
        .apply_command(&project.project_id, &steer_command.command_id)
        .await
        .expect("queue steer");
    let (steers, _) = store
        .pending_task_steers(
            &project.project_id,
            &lease.task.task_id,
            lease.task.revision,
            lease.task.route_cancellation_epoch,
        )
        .await
        .expect("pending steer");
    assert_eq!(steers.len(), 1);
    let output = WorkerOutput {
        summary: "recorded a precise obstruction".into(),
        discoveries: vec![],
        candidates: vec![
            CandidateDraft {
                statement: "First candidate".into(),
                assumptions: vec![],
                proof_markdown: "A complete first proof.".into(),
                dependency_fact_ids: vec![],
                definitions_introduced: Default::default(),
                external_source_ids: vec![],
                candidate_type: CandidateType::Lemma,
                target_goal_ids: task.goal_ids.clone(),
            },
            CandidateDraft {
                statement: "Second candidate".into(),
                assumptions: vec![],
                proof_markdown: "A complete second proof.".into(),
                dependency_fact_ids: vec![],
                definitions_introduced: Default::default(),
                external_source_ids: vec![],
                candidate_type: CandidateType::Lemma,
                target_goal_ids: task.goal_ids.clone(),
            },
        ],
        failures: vec![FailureDraft {
            failure_type: "missing_bridge".into(),
            summary: "the named bridge lemma is still missing".into(),
            repairable: true,
        }],
        uncertainties: vec![],
        sources: vec![],
        experiments: vec![],
    };
    assert!(matches!(
        store.submit_local_result_envelope(&lease, &output).await,
        Err(StorageError::InvalidTransition(_))
    ));
    assert_eq!(
        store
            .get_command(&project.project_id, &steer_command.command_id)
            .await
            .expect("still pending command")
            .status,
        research_domain::CommandStatus::WaitingSafePoint
    );
    let incorporated_ids = vec![steers[0].steer_id.clone()];
    let submission = store
        .submit_local_result_envelope_after_steers(&lease, &output, &incorporated_ids)
        .await
        .expect("submit envelope and acknowledge steer");
    let envelope_id = match submission {
        LocalResultSubmission::Submitted {
            result_envelope_id,
            events,
        } => {
            assert!(
                events
                    .iter()
                    .any(|event| event.event_type == "task.steer.applied")
            );
            result_envelope_id
        }
        LocalResultSubmission::SteeringPending { .. } => panic!("steer was incorporated"),
    };
    assert_eq!(
        store
            .get_command(&project.project_id, &steer_command.command_id)
            .await
            .expect("applied command")
            .status,
        research_domain::CommandStatus::Applied
    );
    let (duplicate_id, duplicate_events) = store
        .submit_local_result_envelope(&lease, &output)
        .await
        .expect("idempotent envelope");
    assert_eq!(duplicate_id, envelope_id);
    assert!(duplicate_events.is_empty());
    let ingested = store
        .ingest_local_result_envelope(&envelope_id)
        .await
        .expect("ingest envelope");
    assert_eq!(ingested.output.summary, output.summary);
    assert_eq!(ingested.verifications.len(), 2);
    let registered_candidates: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM candidates WHERE project_id=?")
            .bind(&project.project_id)
            .fetch_one(store.pool())
            .await
            .expect("candidate count");
    assert_eq!(registered_candidates, 2);
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
    assert_eq!(pending_outbox, 0);
    let delivered_outbox: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM event_outbox WHERE project_id=? AND status='delivered'",
    )
    .bind(&project.project_id)
    .fetch_one(store.pool())
    .await
    .expect("delivered outbox count");
    assert!(delivered_outbox > 0);
    let writer = store.state_writer_status();
    assert_eq!(writer.status, "running");
    assert!(writer.last_command_kind.is_some());
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
            &plan_scores(),
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
    let lead = SourceDraft {
        title: "search-only-lead".into(),
        authors: vec![],
        url: Some("https://example.test/search-only-lead".into()),
        citation_key: None,
        theorem_reference: None,
        statement_excerpt: None,
        assumptions: vec![],
        applicability: "query hit; full text not inspected".into(),
        status: "lead_unverified".into(),
        retrieval_query: Some("possible theorem".into()),
        document_version: None,
        fulltext_path: None,
        fulltext_sha256: None,
        fulltext_artifact_id: None,
    };
    let promotable_lead = SourceDraft {
        title: "promotable".into(),
        url: Some("https://example.test/promotable".into()),
        ..lead.clone()
    };
    let not_applicable = SourceDraft {
        title: "inspected-but-irrelevant".into(),
        authors: vec![],
        url: None,
        citation_key: Some("Irrelevant2026".into()),
        theorem_reference: None,
        statement_excerpt: None,
        assumptions: vec![],
        applicability: "wrong ambient category".into(),
        status: "not_applicable".into(),
        retrieval_query: Some("possible theorem".into()),
        document_version: None,
        fulltext_path: None,
        fulltext_sha256: None,
        fulltext_artifact_id: None,
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
            lead,
            not_applicable,
            promotable_lead,
            source("promotable", Some(artifact.artifact_id.clone())),
        ],
        experiments: vec![],
    };
    let (envelope_id, _) = store
        .submit_local_result_envelope(&lease, &output)
        .await
        .expect("submit");
    let ingestion = store
        .ingest_local_result_envelope(&envelope_id)
        .await
        .expect("ingest");
    let events = ingestion.events;
    let sources = store
        .list_sources(&project.project_id)
        .await
        .expect("sources");
    assert_eq!(sources.len(), 4);
    assert!(
        sources
            .iter()
            .any(|source| source.status == "reported_unverified")
    );
    let lead = sources
        .iter()
        .find(|source| source.status == "lead_unverified")
        .expect("lead retained");
    assert!(lead.theorem_reference.is_none());
    assert!(lead.statement_excerpt.is_none());
    assert!(lead.fulltext_artifact_id.is_none());
    assert!(
        sources
            .iter()
            .any(|source| source.status == "not_applicable")
    );
    let promoted = sources
        .iter()
        .find(|source| source.title == "promotable")
        .expect("lead promoted in place");
    assert_eq!(promoted.status, "reported_unverified");
    assert!(promoted.fulltext_artifact_id.is_some());
    let dispositions = sqlx::query_scalar::<_, String>(
        "SELECT disposition FROM source_ingestion_records WHERE project_id=?",
    )
    .bind(&project.project_id)
    .fetch_all(store.pool())
    .await
    .expect("ingestion dispositions");
    assert!(dispositions.iter().any(|value| value == "upgraded"));
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "source.reported")
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "source.lead.recorded")
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "source.not_applicable.recorded")
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

    let completed = store
        .get_task(&project.project_id, &task.task_id)
        .await
        .expect("completed task");
    let result = store
        .submit_candidate(
            &project.project_id,
            CandidateSubmission {
                task_id: completed.task_id,
                route_id: completed.route_id,
                target_goal_ids: vec![],
                statement: "A lead proves the claim".into(),
                assumptions: vec![],
                proof_markdown: "Unsupported citation.".into(),
                dependency_fact_ids: vec![],
                definitions_introduced: Default::default(),
                external_source_ids: vec![lead.source_id.clone()],
                candidate_type: CandidateType::Lemma,
                task_revision: completed.revision,
                route_cancellation_epoch: completed.route_cancellation_epoch,
            },
            "lead-cannot-support-candidate",
        )
        .await;
    assert!(matches!(result, Err(StorageError::InvalidDependency(_))));
}

#[tokio::test]
async fn worker_result_normalizes_unique_source_aliases_and_quarantines_bad_ones() {
    let (store, _temp) = store().await;
    let (project, round, task, lease) = leased_v2_task(
        &store,
        "source-reference-normalization",
        Some("literature_researcher"),
    )
    .await;
    let bytes = b"trusted source body";
    let (artifact, _) = store
        .store_artifact(
            &project.project_id,
            "source_fulltext",
            "source.txt",
            bytes,
            round.number,
            vec![task.task_id.clone()],
        )
        .await
        .expect("source artifact");
    let source = |title: &str, url: &str, citation_key: &str| SourceDraft {
        title: title.into(),
        authors: vec!["A. Author".into()],
        url: Some(url.into()),
        citation_key: Some(citation_key.into()),
        theorem_reference: Some("Theorem 1".into()),
        statement_excerpt: Some("A implies A".into()),
        assumptions: vec!["A".into()],
        applicability: "exactly the cited step".into(),
        status: "reported".into(),
        retrieval_query: Some("A theorem".into()),
        document_version: Some("v1".into()),
        fulltext_path: None,
        fulltext_sha256: Some(artifact.sha256.clone()),
        fulltext_artifact_id: Some(artifact.artifact_id.clone()),
    };
    let candidate = |statement: &str, source_reference: &str| CandidateDraft {
        statement: statement.into(),
        assumptions: vec![],
        proof_markdown: "Complete proof for the fixture.".into(),
        dependency_fact_ids: vec![],
        definitions_introduced: Default::default(),
        external_source_ids: vec![source_reference.into()],
        candidate_type: CandidateType::Lemma,
        target_goal_ids: task.goal_ids.clone(),
    };
    let output = WorkerOutput {
        summary: "source reference normalization".into(),
        discoveries: vec![],
        candidates: vec![
            candidate("unique citation key", "Stacks-0BNH"),
            candidate("canonical URL", "https://example.test/stacks/"),
            candidate("missing citation key", "Missing-Key"),
            candidate("ambiguous citation key", "Duplicate-Key"),
        ],
        failures: vec![],
        uncertainties: vec![],
        sources: vec![
            source(
                "Stacks completion",
                "https://example.test/stacks",
                "Stacks-0BNH",
            ),
            source(
                "Duplicate A",
                "https://example.test/duplicate-a",
                "Duplicate-Key",
            ),
            source(
                "Duplicate B",
                "https://example.test/duplicate-b",
                "Duplicate-Key",
            ),
        ],
        experiments: vec![],
    };
    let (envelope_id, _) = store
        .submit_local_result_envelope(&lease, &output)
        .await
        .expect("submit result");
    let ingested = store
        .ingest_local_result_envelope(&envelope_id)
        .await
        .expect("bad source aliases must not poison the whole envelope");
    assert_eq!(ingested.verifications.len(), 4);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM result_envelopes WHERE result_envelope_id=?",
        )
        .bind(&envelope_id)
        .fetch_one(store.pool())
        .await
        .expect("envelope status"),
        "ingested"
    );

    let rows = sqlx::query(
        "SELECT submission_json,status FROM candidates WHERE project_id=? ORDER BY created_at,candidate_id",
    )
    .bind(&project.project_id)
    .fetch_all(store.pool())
    .await
    .expect("candidates");
    let candidates = rows
        .iter()
        .map(|row| {
            Ok((
                serde_json::from_str::<CandidateSubmission>(row.try_get("submission_json")?)?,
                row.try_get::<String, _>("status")?,
            ))
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()
        .expect("candidate rows");
    let stored_sources = store
        .list_sources(&project.project_id)
        .await
        .expect("sources");
    let stacks_source_id = stored_sources
        .iter()
        .find(|source| source.citation_key.as_deref() == Some("Stacks-0BNH"))
        .expect("Stacks source")
        .source_id
        .clone();
    for statement in ["unique citation key", "canonical URL"] {
        let (submission, status) = candidates
            .iter()
            .find(|(submission, _)| submission.statement == statement)
            .expect("normalized candidate");
        assert_eq!(status, "submitted");
        assert_eq!(
            submission.external_source_ids,
            vec![stacks_source_id.clone()]
        );
    }
    for statement in ["missing citation key", "ambiguous citation key"] {
        let (_, status) = candidates
            .iter()
            .find(|(submission, _)| submission.statement == statement)
            .expect("rejected candidate");
        assert_eq!(status, "unknown");
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM failures WHERE project_id=? AND failure_type='invalid_candidate_source_reference'")
            .bind(&project.project_id).fetch_one(store.pool()).await.expect("failures"),
        2
    );
    assert_eq!(
        ingested
            .events
            .iter()
            .filter(|event| event.event_type == "claim.source_references.normalized")
            .count(),
        2
    );
    assert_eq!(
        ingested
            .events
            .iter()
            .filter(|event| event.event_type == "claim.rejected_untrusted_source_reference")
            .count(),
        2
    );
}

#[tokio::test]
async fn worker_result_uses_envelope_alias_when_source_deduplicates_by_url() {
    let (store, _temp) = store().await;
    let (project, round, task, lease) = leased_v2_task(
        &store,
        "source-reference-envelope-alias",
        Some("literature_researcher"),
    )
    .await;
    let bytes = b"already archived source";
    let (artifact, _) = store
        .store_artifact(
            &project.project_id,
            "source_fulltext",
            "existing-source.txt",
            bytes,
            round.number,
            vec![task.task_id.clone()],
        )
        .await
        .expect("source artifact");
    sqlx::query("INSERT INTO sources(source_id,project_id,title,authors_json,url,normalized_url,citation_key,theorem_reference,statement_excerpt,assumptions_json,applicability,status,fulltext_artifact_id,retrieved_at) VALUES('source-existing',?,'Existing source','[]','https://example.test/same','https://example.test/same','Old-Key','Theorem 1','A implies A','[]','existing evidence','reported_unverified',?,?)")
        .bind(&project.project_id).bind(&artifact.artifact_id).bind(chrono::Utc::now().to_rfc3339())
        .execute(store.pool()).await.expect("existing source");
    let output = WorkerOutput {
        summary: "same source reported with a new local citation key".into(),
        discoveries: vec![],
        candidates: vec![CandidateDraft {
            statement: "candidate using new envelope key".into(),
            assumptions: vec![],
            proof_markdown: "Complete fixture proof.".into(),
            dependency_fact_ids: vec![],
            definitions_introduced: Default::default(),
            external_source_ids: vec!["New-Key".into()],
            candidate_type: CandidateType::Lemma,
            target_goal_ids: task.goal_ids.clone(),
        }],
        failures: vec![],
        uncertainties: vec![],
        sources: vec![SourceDraft {
            title: "Same source, new envelope key".into(),
            authors: vec!["A. Author".into()],
            url: Some("https://example.test/same/".into()),
            citation_key: Some("New-Key".into()),
            theorem_reference: Some("Theorem 1".into()),
            statement_excerpt: Some("A implies A".into()),
            assumptions: vec![],
            applicability: "same evidence".into(),
            status: "reported".into(),
            retrieval_query: Some("same theorem".into()),
            document_version: Some("v2".into()),
            fulltext_path: None,
            fulltext_sha256: Some(artifact.sha256.clone()),
            fulltext_artifact_id: Some(artifact.artifact_id.clone()),
        }],
        experiments: vec![],
    };
    let (envelope_id, _) = store
        .submit_local_result_envelope(&lease, &output)
        .await
        .expect("submit result");
    let ingested = store
        .ingest_local_result_envelope(&envelope_id)
        .await
        .expect("new envelope alias must resolve to existing source");
    assert_eq!(
        ingested.verifications[0].status,
        research_domain::CandidateStatus::Submitted
    );
    let candidate = store
        .get_candidate(&ingested.verifications[0].candidate_id)
        .await
        .expect("candidate");
    assert_eq!(
        candidate.submission.external_source_ids,
        vec!["source-existing"]
    );
    let (source_count, citation_key): (i64, String) =
        sqlx::query_as("SELECT COUNT(*),MIN(citation_key) FROM sources WHERE project_id=?")
            .bind(&project.project_id)
            .fetch_one(store.pool())
            .await
            .expect("deduplicated source");
    assert_eq!(source_count, 1);
    assert_eq!(
        citation_key, "Old-Key",
        "the durable source key is immutable"
    );
    assert!(ingested.events.iter().any(|event| {
        event.event_type == "claim.source_references.normalized"
            && event.data["source_reference_audit"]["references"][0]["matched_by"]
                == "envelope_alias"
    }));
}

#[tokio::test]
async fn worker_result_rejects_envelope_alias_for_ambiguous_canonical_sources() {
    let (store, _temp) = store().await;
    let (project, round, task, lease) = leased_v2_task(
        &store,
        "source-reference-envelope-alias-ambiguous",
        Some("literature_researcher"),
    )
    .await;
    let bytes = b"canonical duplicate source fixture";
    let (artifact, _) = store
        .store_artifact(
            &project.project_id,
            "source_fulltext",
            "canonical-duplicate.txt",
            bytes,
            round.number,
            vec![task.task_id.clone()],
        )
        .await
        .expect("source artifact");
    let now = chrono::Utc::now().to_rfc3339();
    for (source_id, title, citation_key, status, fulltext_artifact_id) in [
        (
            "source-canonical-a",
            "Canonical source A",
            "Old-A",
            "reported_unverified",
            Some(artifact.artifact_id.as_str()),
        ),
        (
            "source-canonical-b",
            "Canonical source B",
            "Old-B",
            "lead_unverified",
            None,
        ),
    ] {
        sqlx::query("INSERT INTO sources(source_id,project_id,title,authors_json,url,normalized_url,citation_key,theorem_reference,statement_excerpt,assumptions_json,applicability,status,fulltext_artifact_id,retrieved_at) VALUES(?,?,?,'[]','https://example.test/ambiguous','https://example.test/ambiguous',?,'Theorem 1','A implies A','[]','canonical ambiguity fixture',?,?,?)")
            .bind(source_id)
            .bind(&project.project_id)
            .bind(title)
            .bind(citation_key)
            .bind(status)
            .bind(fulltext_artifact_id)
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("canonical source");
    }
    let output = WorkerOutput {
        summary: "ambiguous canonical source reported with an envelope-local key".into(),
        discoveries: vec![],
        candidates: vec![CandidateDraft {
            statement: "candidate using ambiguous envelope key".into(),
            assumptions: vec![],
            proof_markdown: "Complete fixture proof.".into(),
            dependency_fact_ids: vec![],
            definitions_introduced: Default::default(),
            external_source_ids: vec!["New-Ambiguous-Key".into()],
            candidate_type: CandidateType::Lemma,
            target_goal_ids: task.goal_ids.clone(),
        }],
        failures: vec![],
        uncertainties: vec![],
        sources: vec![SourceDraft {
            title: "Ambiguous canonical source".into(),
            authors: vec!["A. Author".into()],
            url: Some("https://example.test/ambiguous/".into()),
            citation_key: Some("New-Ambiguous-Key".into()),
            theorem_reference: Some("Theorem 1".into()),
            statement_excerpt: Some("A implies A".into()),
            assumptions: vec![],
            applicability: "ambiguous evidence".into(),
            status: "reported".into(),
            retrieval_query: Some("ambiguous theorem".into()),
            document_version: Some("v2".into()),
            fulltext_path: None,
            fulltext_sha256: Some(artifact.sha256.clone()),
            fulltext_artifact_id: Some(artifact.artifact_id.clone()),
        }],
        experiments: vec![],
    };
    let (envelope_id, _) = store
        .submit_local_result_envelope(&lease, &output)
        .await
        .expect("submit result");
    let ingested = store
        .ingest_local_result_envelope(&envelope_id)
        .await
        .expect("ambiguous alias must fail closed per candidate");
    assert_eq!(
        ingested.verifications[0].status,
        research_domain::CandidateStatus::Unknown
    );
    let rejection_event = ingested
        .events
        .iter()
        .find(|event| event.event_type == "claim.rejected_untrusted_source_reference")
        .expect("candidate rejection event");
    let issue = &rejection_event.data["source_reference_audit"]["issues"][0];
    assert_eq!(issue["reason"], "ambiguous_source_reference");
    assert_eq!(
        issue["matching_source_ids"],
        json!(["source-canonical-a", "source-canonical-b"])
    );
    assert!(
        !ingested
            .events
            .iter()
            .any(|event| event.event_type == "route.material_progress")
    );
    let source_states: Vec<(String, String)> = sqlx::query_as(
        "SELECT source_id,status FROM sources WHERE project_id=? ORDER BY source_id",
    )
    .bind(&project.project_id)
    .fetch_all(store.pool())
    .await
    .expect("source states");
    assert_eq!(
        source_states,
        vec![
            ("source-canonical-a".into(), "reported_unverified".into()),
            ("source-canonical-b".into(), "lead_unverified".into()),
        ],
        "ambiguous canonical matches must not be upgraded or otherwise mutated"
    );
    let ambiguous_ingestion_targets: Vec<String> = sqlx::query_scalar("SELECT source_id FROM source_ingestion_records WHERE project_id=? AND disposition='duplicate' AND json_extract(reasons_json,'$[0]')='ambiguous_normalized_identifier' ORDER BY source_id")
        .bind(&project.project_id)
        .fetch_all(store.pool())
        .await
        .expect("ambiguous ingestion audit");
    assert_eq!(
        ambiguous_ingestion_targets,
        vec!["source-canonical-a", "source-canonical-b"]
    );
}

#[tokio::test]
async fn rejected_source_reference_alone_is_not_material_route_progress() {
    let (store, _temp) = store().await;
    let (project, _round, task, lease) =
        leased_v2_task(&store, "source-reference-no-progress", None).await;
    let output = WorkerOutput {
        summary: "only an invalid cited candidate".into(),
        discoveries: vec![],
        candidates: vec![CandidateDraft {
            statement: "unsupported candidate".into(),
            assumptions: vec![],
            proof_markdown: "This proof cites a source that was never ingested.".into(),
            dependency_fact_ids: vec![],
            definitions_introduced: Default::default(),
            external_source_ids: vec!["missing-source".into()],
            candidate_type: CandidateType::Lemma,
            target_goal_ids: task.goal_ids.clone(),
        }],
        failures: vec![],
        uncertainties: vec![],
        sources: vec![],
        experiments: vec![],
    };
    let (envelope_id, _) = store
        .submit_local_result_envelope(&lease, &output)
        .await
        .expect("submit");
    let ingested = store
        .ingest_local_result_envelope(&envelope_id)
        .await
        .expect("ingest terminally untrusted candidate");
    assert_eq!(
        ingested.verifications[0].status,
        research_domain::CandidateStatus::Unknown
    );
    assert!(
        !ingested
            .events
            .iter()
            .any(|event| event.event_type == "route.material_progress")
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM route_progress_entries WHERE project_id=? AND route_id=?",
        )
        .bind(&project.project_id)
        .bind(&task.route_id)
        .fetch_one(store.pool())
        .await
        .expect("route progress count"),
        0
    );
}

#[tokio::test]
async fn reconciliation_never_rewrites_candidate_covered_by_immutable_snapshot() {
    let (store, _temp) = store().await;
    let (project, _round, _task, lease) =
        leased_v2_task(&store, "snapshot-source-reference-recovery", None).await;
    sqlx::query("INSERT INTO sources(source_id,project_id,title,authors_json,citation_key,assumptions_json,applicability,status,retrieved_at) VALUES('source-snapshot',?,'Snapshot source','[]','Snapshot-Key','[]','snapshot fixture','reported_unverified',?)")
        .bind(&project.project_id).bind(chrono::Utc::now().to_rfc3339())
        .execute(store.pool()).await.expect("source");
    let receipt = store
        .submit_candidate(
            &project.project_id,
            CandidateSubmission {
                task_id: lease.task.task_id.clone(),
                route_id: lease.task.route_id.clone(),
                target_goal_ids: lease.task.goal_ids.clone(),
                statement: "snapshot candidate with duplicate exact source IDs".into(),
                assumptions: vec![],
                proof_markdown: "Complete fixture proof.".into(),
                dependency_fact_ids: vec![],
                definitions_introduced: Default::default(),
                external_source_ids: vec!["source-snapshot".into(), "source-snapshot".into()],
                candidate_type: CandidateType::Lemma,
                task_revision: lease.task.revision,
                route_cancellation_epoch: lease.task.route_cancellation_epoch,
            },
            "snapshot-source-reference-candidate",
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
                name: "snapshot immutability".into(),
                profile: VerificationProfile::StandardReview,
                required_acceptance: AcceptanceClass::Reviewed,
                required_checks: vec!["deterministic_precheck".into(), "math_review_1".into()],
                independent_reviewer_count: 1,
                require_citation_review: false,
                require_adversarial_review: false,
                require_alignment_review: false,
                require_fresh_replay: false,
                max_attempts: 1,
                risk_score: 0.1,
                risk_reasons: vec![],
            },
        )
        .await
        .expect("case");
    let (snapshot, _) = store
        .create_verification_snapshot(
            &case.case_id,
            VerificationSnapshotDraft {
                toolchain_hash: None,
                extra_payload: json!({}),
            },
        )
        .await
        .expect("snapshot");
    let original_submission: String =
        sqlx::query_scalar("SELECT submission_json FROM candidates WHERE candidate_id=?")
            .bind(&receipt.candidate.candidate_id)
            .fetch_one(store.pool())
            .await
            .expect("original submission");
    let original_snapshot: (String, String, String) = sqlx::query_as(
        "SELECT candidate_hash,content_hash,payload_json FROM verification_snapshots WHERE snapshot_id=?",
    )
    .bind(&snapshot.snapshot_id)
    .fetch_one(store.pool())
    .await
    .expect("original snapshot");

    for trigger in ["service_startup", "watchdog_tick"] {
        let result = store
            .run_reconciliation(Some(&project.project_id), trigger)
            .await
            .expect("reconciliation");
        assert!(
            !result["repairs"]
                .as_array()
                .expect("repairs")
                .iter()
                .any(|repair| {
                    matches!(
                        repair["kind"].as_str(),
                        Some(
                            "candidate_source_references_normalized"
                                | "candidate_source_references_rejected_untrusted"
                        )
                    )
                })
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT submission_json FROM candidates WHERE candidate_id=?",
            )
            .bind(&receipt.candidate.candidate_id)
            .fetch_one(store.pool())
            .await
            .expect("unchanged submission"),
            original_submission
        );
        assert_eq!(
            sqlx::query_as::<_, (String, String, String)>(
                "SELECT candidate_hash,content_hash,payload_json FROM verification_snapshots WHERE snapshot_id=?",
            )
            .bind(&snapshot.snapshot_id)
            .fetch_one(store.pool())
            .await
            .expect("unchanged snapshot"),
            original_snapshot
        );
    }
    assert_eq!(
        store
            .get_candidate(&receipt.candidate.candidate_id)
            .await
            .expect("candidate")
            .status,
        research_domain::CandidateStatus::Submitted
    );
    assert_eq!(
        store
            .get_verification(&receipt.verification.verification_id)
            .await
            .expect("verification")
            .status,
        research_domain::CandidateStatus::Submitted
    );
    assert_eq!(
        store
            .get_verification_case(&case.case_id)
            .await
            .expect("case")
            .stage,
        VerificationStage::Precheck
    );
}

#[tokio::test]
async fn reconciliation_terminalizes_snapshot_candidate_hash_mismatch_once() {
    let (store, _temp) = store().await;
    let (case, _formalization) = proof_search_fixture(&store).await;
    let original_snapshot: (String, String, String) = sqlx::query_as(
        "SELECT candidate_hash,content_hash,payload_json FROM verification_snapshots WHERE case_id=?",
    )
    .bind(&case.case_id)
    .fetch_one(store.pool())
    .await
    .expect("immutable snapshot");
    let offer = store
        .offer_verification_worker(
            &case.case_id,
            "math_review_1",
            "mock",
            None,
            "runtime/snapshot-hash-mismatch",
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
            &json!({"verdict":"accepted","summary":"pending poisoned result"}),
        )
        .await
        .expect("submit pending verification result");

    let mut poisoned_submission = store
        .get_candidate(&case.candidate_id)
        .await
        .expect("candidate")
        .submission;
    poisoned_submission
        .proof_markdown
        .push_str("\nlegacy reconciliation mutation");
    sqlx::query("UPDATE candidates SET submission_json=?,status='submitted' WHERE candidate_id=?")
        .bind(serde_json::to_string(&poisoned_submission).expect("poisoned submission json"))
        .bind(&case.candidate_id)
        .execute(store.pool())
        .await
        .expect("emulate legacy candidate mutation");
    sqlx::query("UPDATE verifications SET status='submitted' WHERE verification_id=?")
        .bind(&case.verification_id)
        .execute(store.pool())
        .await
        .expect("emulate resumable poisoned verification");

    let first = store
        .run_reconciliation(Some(&case.project_id), "watchdog_tick")
        .await
        .expect("terminalize poisoned snapshot candidate");
    assert!(first["repairs"].as_array().is_some_and(|repairs| {
        repairs.iter().any(|repair| {
            repair["kind"] == "verification_snapshot_candidate_hash_mismatch_terminalized"
                && repair["source_reference_audit"]["reason"]
                    == "immutable_verification_snapshot_candidate_hash_mismatch"
        })
    }));
    assert_eq!(
        store
            .get_candidate(&case.candidate_id)
            .await
            .expect("terminal candidate")
            .status,
        research_domain::CandidateStatus::Unknown
    );
    assert_eq!(
        store
            .get_verification(&case.verification_id)
            .await
            .expect("terminal verification")
            .status,
        research_domain::CandidateStatus::Unknown
    );
    assert_eq!(
        store
            .get_verification_case(&case.case_id)
            .await
            .expect("terminal case")
            .stage,
        VerificationStage::Unknown
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM verification_task_leases WHERE verification_lease_id=?",
        )
        .bind(&lease.verification_lease_id)
        .fetch_one(store.pool())
        .await
        .expect("terminal lease"),
        "cancelled"
    );
    let (attempt_status, error_kind): (String, Option<String>) =
        sqlx::query_as("SELECT status,error_kind FROM verification_attempts WHERE attempt_id=?")
            .bind(&offer.attempt_id)
            .fetch_one(store.pool())
            .await
            .expect("terminal attempt");
    assert_eq!(attempt_status, "failed");
    assert_eq!(
        error_kind.as_deref(),
        Some("verification_snapshot_candidate_hash_mismatch")
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM verification_result_envelopes WHERE verification_result_envelope_id=?",
        )
        .bind(&envelope_id)
        .fetch_one(store.pool())
        .await
        .expect("terminal envelope"),
        "stale"
    );
    assert_eq!(
        sqlx::query_as::<_, (String, String, String)>(
            "SELECT candidate_hash,content_hash,payload_json FROM verification_snapshots WHERE case_id=?",
        )
        .bind(&case.case_id)
        .fetch_one(store.pool())
        .await
        .expect("unchanged immutable snapshot"),
        original_snapshot
    );

    let second = store
        .run_reconciliation(Some(&case.project_id), "watchdog_tick")
        .await
        .expect("idempotent second reconciliation");
    assert!(!second["repairs"].as_array().is_some_and(|repairs| {
        repairs.iter().any(|repair| {
            repair["kind"] == "verification_snapshot_candidate_hash_mismatch_terminalized"
        })
    }));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM failures WHERE project_id=? AND failure_type='verification_snapshot_candidate_hash_mismatch'")
            .bind(&case.project_id)
            .fetch_one(store.pool())
            .await
            .expect("single integrity failure"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM events WHERE project_id=? AND type='verification.snapshot_candidate_hash_mismatch' AND json_extract(entity_json,'$.id')=?")
            .bind(&case.project_id)
            .bind(&case.candidate_id)
            .fetch_one(store.pool())
            .await
            .expect("single integrity event"),
        1
    );
}

#[tokio::test]
async fn reconciliation_repairs_or_terminalizes_legacy_candidate_source_aliases() {
    let (store, _temp) = store().await;
    let (project, _round, task, lease) =
        leased_v2_task(&store, "legacy-source-reference-recovery", None).await;
    let empty_output = WorkerOutput {
        summary: "legacy task completed before candidate migration".into(),
        discoveries: vec![],
        candidates: vec![],
        failures: vec![],
        uncertainties: vec![],
        sources: vec![],
        experiments: vec![],
    };
    let (envelope_id, _) = store
        .submit_local_result_envelope(&lease, &empty_output)
        .await
        .expect("submit completion");
    store
        .ingest_local_result_envelope(&envelope_id)
        .await
        .expect("complete task");
    let completed = store
        .get_task(&project.project_id, &task.task_id)
        .await
        .expect("completed task");
    let now = chrono::Utc::now().to_rfc3339();
    for (source_id, title, citation_key) in [
        ("source-legacy-unique", "Unique source", "Legacy-Key"),
        (
            "source-legacy-ambiguous-a",
            "Ambiguous A",
            "Legacy-Duplicate",
        ),
        (
            "source-legacy-ambiguous-b",
            "Ambiguous B",
            "Legacy-Duplicate",
        ),
    ] {
        sqlx::query("INSERT INTO sources(source_id,project_id,title,authors_json,citation_key,assumptions_json,applicability,status,retrieved_at) VALUES(?,?,?,'[]',?,'[]','legacy fixture','reported_unverified',?)")
            .bind(source_id).bind(&project.project_id).bind(title).bind(citation_key).bind(&now)
            .execute(store.pool()).await.expect("legacy source");
    }
    let submission = |statement: &str, source_reference: &str| CandidateSubmission {
        task_id: completed.task_id.clone(),
        route_id: completed.route_id.clone(),
        target_goal_ids: completed.goal_ids.clone(),
        statement: statement.into(),
        assumptions: vec![],
        proof_markdown: "Legacy proof payload.".into(),
        dependency_fact_ids: vec![],
        definitions_introduced: Default::default(),
        external_source_ids: vec![source_reference.into()],
        candidate_type: CandidateType::Lemma,
        task_revision: completed.revision,
        route_cancellation_epoch: completed.route_cancellation_epoch,
    };
    for (candidate_id, verification_id, status, submission) in [
        (
            "candidate-legacy-unique-submitted",
            "verification-legacy-unique-submitted",
            "submitted",
            submission("unique submitted", "Legacy-Key"),
        ),
        (
            "candidate-legacy-unique-verifying",
            "verification-legacy-unique-verifying",
            "verifying",
            submission("unique interrupted verifying", "Legacy-Key"),
        ),
        (
            "candidate-legacy-missing",
            "verification-legacy-missing",
            "submitted",
            submission("missing legacy", "Legacy-Missing"),
        ),
        (
            "candidate-legacy-ambiguous",
            "verification-legacy-ambiguous",
            "submitted",
            submission("ambiguous legacy", "Legacy-Duplicate"),
        ),
    ] {
        sqlx::query("INSERT INTO candidates(candidate_id,project_id,submission_json,status,created_at,idempotency_key) VALUES(?,?,?,?,?,?)")
            .bind(candidate_id).bind(&project.project_id).bind(serde_json::to_string(&submission).expect("submission json"))
            .bind(status).bind(&now).bind(format!("legacy-{candidate_id}"))
            .execute(store.pool()).await.expect("legacy candidate");
        sqlx::query("INSERT INTO verifications(verification_id,candidate_id,project_id,status,started_at) VALUES(?,?,?,?,?)")
            .bind(verification_id).bind(candidate_id).bind(&project.project_id).bind(status)
            .bind((status == "verifying").then_some(now.as_str()))
            .execute(store.pool()).await.expect("legacy verification");
    }

    let reconciliation = store
        .run_reconciliation(Some(&project.project_id), "service_startup")
        .await
        .expect("legacy poison reconciliation");
    let repairs = reconciliation["repairs"].as_array().expect("repairs");
    assert_eq!(
        repairs
            .iter()
            .filter(|repair| repair["kind"] == "candidate_source_references_normalized")
            .count(),
        2
    );
    assert!(repairs.iter().any(|repair| {
        repair["kind"] == "verification_reset_for_restart"
            && repair["verification_id"] == "verification-legacy-unique-verifying"
    }));
    assert_eq!(
        repairs
            .iter()
            .filter(|repair| { repair["kind"] == "candidate_source_references_rejected_untrusted" })
            .count(),
        2
    );
    for candidate_id in [
        "candidate-legacy-unique-submitted",
        "candidate-legacy-unique-verifying",
    ] {
        let candidate = store.get_candidate(candidate_id).await.expect("candidate");
        assert_eq!(
            candidate.submission.external_source_ids,
            vec!["source-legacy-unique"]
        );
        assert_eq!(
            candidate.status,
            research_domain::CandidateStatus::Submitted
        );
        let status: String =
            sqlx::query_scalar("SELECT status FROM verifications WHERE candidate_id=?")
                .bind(candidate_id)
                .fetch_one(store.pool())
                .await
                .expect("verification status");
        assert_eq!(status, "submitted");
    }
    for candidate_id in ["candidate-legacy-missing", "candidate-legacy-ambiguous"] {
        let candidate = store.get_candidate(candidate_id).await.expect("candidate");
        assert_eq!(candidate.status, research_domain::CandidateStatus::Unknown);
        let (status, report_json): (String, Option<String>) =
            sqlx::query_as("SELECT status,report_json FROM verifications WHERE candidate_id=?")
                .bind(candidate_id)
                .fetch_one(store.pool())
                .await
                .expect("terminal verification");
        assert_eq!(status, "unknown");
        assert!(
            report_json
                .as_deref()
                .is_some_and(|report| report.contains("untrusted_source_reference"))
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM failures WHERE project_id=? AND failure_type='invalid_candidate_source_reference'")
            .bind(&project.project_id).fetch_one(store.pool()).await.expect("legacy failures"),
        2
    );
    let recovery_events = store
        .list_events_after(&project.project_id, 0, 1_000)
        .await
        .expect("recovery audit events");
    assert_eq!(
        recovery_events
            .iter()
            .filter(|event| event.event_type == "claim.source_references.normalized")
            .count(),
        2
    );
    assert_eq!(
        recovery_events
            .iter()
            .filter(|event| event.event_type == "claim.rejected_untrusted_source_reference")
            .count(),
        2
    );
    let failure_audits = sqlx::query_scalar::<_, String>(
        "SELECT raw_json FROM failures WHERE project_id=? AND failure_type='invalid_candidate_source_reference' ORDER BY failure_id",
    )
    .bind(&project.project_id)
    .fetch_all(store.pool())
    .await
    .expect("failure audit payloads")
    .join("\n");
    assert!(failure_audits.contains("Legacy-Missing"));
    assert!(failure_audits.contains("Legacy-Duplicate"));
    store
        .run_reconciliation(Some(&project.project_id), "watchdog_tick")
        .await
        .expect("second reconciliation is idempotent");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM failures WHERE project_id=? AND failure_type='invalid_candidate_source_reference'")
            .bind(&project.project_id).fetch_one(store.pool()).await.expect("idempotent legacy failures"),
        2
    );
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
            &plan_scores(),
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
            &plan_scores(),
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
    let completed_at: Option<String> =
        sqlx::query_scalar("SELECT completed_at FROM task_leases WHERE lease_id=?")
            .bind(&lease.lease_id)
            .fetch_one(store.pool())
            .await
            .expect("expired lease completion timestamp");
    assert!(completed_at.is_some());
    let attempt_status: String =
        sqlx::query_scalar("SELECT status FROM task_attempts WHERE attempt_id=?")
            .bind(&offer.attempt.attempt_id)
            .fetch_one(store.pool())
            .await
            .expect("orphaned attempt");
    assert_eq!(attempt_status, "orphaned");
    let instance_status: String =
        sqlx::query_scalar("SELECT status FROM worker_instances WHERE worker_instance_id=?")
            .bind(&offer.worker_instance.worker_instance_id)
            .fetch_one(store.pool())
            .await
            .expect("unhealthy worker instance");
    assert_eq!(instance_status, "unhealthy");
    let worker_state: (String, Option<String>) =
        sqlx::query_as("SELECT status,current_task_id FROM workers WHERE worker_id=?")
            .bind(task.worker_id.as_deref().expect("assigned worker"))
            .fetch_one(store.pool())
            .await
            .expect("logical worker backoff");
    assert_eq!(worker_state, ("backoff".into(), None));
    let requeued = store
        .get_task(&project.project_id, &task.task_id)
        .await
        .expect("requeued task");
    assert_eq!(requeued.status, research_domain::TaskStatus::Queued);
    let resumed_offer = store
        .offer_local_task(&requeued, "mock", None, "runtime/restart-2", json!({}))
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
            &plan_scores(),
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
    let requeued = store
        .get_task(&project.project_id, &task.task_id)
        .await
        .expect("requeued task");
    assert!(matches!(
        store
            .offer_local_task(
                &requeued,
                "mock",
                None,
                "runtime/duplicate-blocked",
                json!({}),
            )
            .await,
        Err(StorageError::InvalidTransition(_))
    ));
    sqlx::query("UPDATE task_attempts SET status='orphaned' WHERE attempt_id=?")
        .bind(&first_offer.attempt.attempt_id)
        .execute(store.pool())
        .await
        .expect("temporarily orphan first attempt");
    let second_offer = store
        .offer_local_task(&requeued, "mock", None, "runtime/duplicate-2", json!({}))
        .await
        .expect("second offer");
    store
        .accept_local_handshake(&second_offer, Some("mock-2"), json!({}), 3_600)
        .await
        .expect("second lease");
    sqlx::query("UPDATE task_attempts SET status='running' WHERE attempt_id=?")
        .bind(&first_offer.attempt.attempt_id)
        .execute(store.pool())
        .await
        .expect("inject duplicate active attempt");

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
    let initial_audit_status: String =
        sqlx::query_scalar("SELECT status FROM artifact_integrity_audit_state WHERE artifact_id=?")
            .bind(&artifact.artifact_id)
            .fetch_one(store.pool())
            .await
            .expect("initial audit status");
    assert_eq!(initial_audit_status, "unverified");
    store
        .run_reconciliation(Some(&project.project_id), "manual")
        .await
        .expect("initial full integrity reconciliation");
    let verified_state = sqlx::query("SELECT status,last_verified_sha256,last_verified_at FROM artifact_integrity_audit_state WHERE artifact_id=?")
        .bind(&artifact.artifact_id)
        .fetch_one(store.pool())
        .await
        .expect("verified audit state");
    assert_eq!(
        verified_state
            .try_get::<String, _>("status")
            .expect("status"),
        "verified"
    );
    assert_eq!(
        verified_state
            .try_get::<Option<String>, _>("last_verified_sha256")
            .expect("verified hash")
            .as_deref(),
        Some(artifact.sha256.as_str())
    );
    assert!(
        verified_state
            .try_get::<Option<String>, _>("last_verified_at")
            .expect("verified time")
            .is_some()
    );
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
    let failed_state = sqlx::query("SELECT status,last_checked_sha256,last_checked_at,last_verified_sha256,last_verified_at,last_error,check_count FROM artifact_integrity_audit_state WHERE artifact_id=?")
        .bind(&artifact.artifact_id)
        .fetch_one(store.pool())
        .await
        .expect("failed audit state");
    assert_eq!(
        failed_state.try_get::<String, _>("status").expect("status"),
        "failed"
    );
    assert_eq!(
        failed_state
            .try_get::<Option<String>, _>("last_verified_sha256")
            .expect("last verified hash")
            .as_deref(),
        Some(artifact.sha256.as_str())
    );
    let last_checked_sha256 = failed_state
        .try_get::<Option<String>, _>("last_checked_sha256")
        .expect("last checked hash")
        .expect("tampered file digest");
    assert_ne!(last_checked_sha256, artifact.sha256);
    assert!(
        failed_state
            .try_get::<Option<String>, _>("last_checked_at")
            .expect("last checked time")
            .is_some()
    );
    assert!(
        failed_state
            .try_get::<Option<String>, _>("last_verified_at")
            .expect("last verified time")
            .is_some()
    );
    assert!(
        failed_state
            .try_get::<Option<String>, _>("last_error")
            .expect("last error")
            .is_some_and(|error| error.starts_with("sha256_mismatch:"))
    );
    assert_eq!(
        failed_state
            .try_get::<i64, _>("check_count")
            .expect("check count"),
        2
    );
}

#[tokio::test]
async fn startup_artifact_integrity_audit_is_bounded_prioritized_and_eventually_complete() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project(
            "incremental-artifact-audit".into(),
            contract(),
            Budget::default(),
        )
        .await
        .expect("project");
    let artifact_count = STARTUP_ARTIFACT_AUDIT_BATCH_SIZE + 3;
    for index in 0..artifact_count {
        let filename = format!("artifact-{index}.txt");
        let content = format!("immutable artifact {index}");
        store
            .store_artifact(
                &project.project_id,
                "audit_fixture",
                &filename,
                content.as_bytes(),
                0,
                vec![],
            )
            .await
            .expect("artifact");
    }

    let initial_unverified: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM artifact_integrity_audit_state WHERE project_id=? AND status='unverified' AND last_verified_at IS NULL",
    )
    .bind(&project.project_id)
    .fetch_one(store.pool())
    .await
    .expect("initial unverified count");
    assert_eq!(initial_unverified, artifact_count);

    let first = store
        .run_reconciliation(Some(&project.project_id), "service_startup")
        .await
        .expect("first startup batch");
    assert_eq!(
        first["artifact_integrity_audit"]["mode"].as_str(),
        Some("incremental_startup")
    );
    assert_eq!(
        first["artifact_integrity_audit"]["checked_count"].as_i64(),
        Some(STARTUP_ARTIFACT_AUDIT_BATCH_SIZE)
    );
    assert_eq!(
        first["artifact_integrity_audit"]["priority_unverified_checked"].as_i64(),
        Some(STARTUP_ARTIFACT_AUDIT_BATCH_SIZE)
    );
    assert_eq!(
        first["artifact_integrity_audit"]["priority_queue_saturated"].as_bool(),
        Some(true)
    );
    let verified_after_first: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM artifact_integrity_audit_state WHERE project_id=? AND status='verified' AND last_verified_sha256 IS NOT NULL AND last_verified_at IS NOT NULL",
    )
    .bind(&project.project_id)
    .fetch_one(store.pool())
    .await
    .expect("first verified count");
    assert_eq!(verified_after_first, STARTUP_ARTIFACT_AUDIT_BATCH_SIZE);
    let still_unverified: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM artifact_integrity_audit_state WHERE project_id=? AND status='unverified'",
    )
    .bind(&project.project_id)
    .fetch_one(store.pool())
    .await
    .expect("remaining unverified count");
    assert_eq!(still_unverified, 3);

    let second = store
        .run_reconciliation(Some(&project.project_id), "service_startup")
        .await
        .expect("second startup batch");
    assert_eq!(
        second["artifact_integrity_audit"]["priority_unverified_checked"].as_i64(),
        Some(3)
    );
    let unverified_after_second: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM artifact_integrity_audit_state WHERE project_id=? AND status<>'verified'",
    )
    .bind(&project.project_id)
    .fetch_one(store.pool())
    .await
    .expect("eventual verification count");
    assert_eq!(unverified_after_second, 0);
    let cursor_row = sqlx::query("SELECT last_artifact_id,last_batch_checked_count,last_batch_started_at,last_batch_completed_at FROM artifact_integrity_audit_cursors WHERE scope_key=?")
        .bind(format!("project:{}", project.project_id))
        .fetch_one(store.pool())
        .await
        .expect("persistent audit cursor");
    assert!(
        cursor_row
            .try_get::<Option<String>, _>("last_artifact_id")
            .expect("cursor")
            .is_some()
    );
    assert_eq!(
        cursor_row
            .try_get::<i64, _>("last_batch_checked_count")
            .expect("batch count"),
        STARTUP_ARTIFACT_AUDIT_BATCH_SIZE
    );
    assert!(
        cursor_row
            .try_get::<Option<String>, _>("last_batch_started_at")
            .expect("batch started")
            .is_some()
    );
    assert!(
        cursor_row
            .try_get::<Option<String>, _>("last_batch_completed_at")
            .expect("batch completed")
            .is_some()
    );

    let (fresh, _) = store
        .store_artifact(
            &project.project_id,
            "audit_fixture",
            "fresh.txt",
            b"fresh unverified artifact",
            0,
            vec![],
        )
        .await
        .expect("fresh artifact");
    let third = store
        .run_reconciliation(Some(&project.project_id), "service_startup")
        .await
        .expect("startup with fresh artifact");
    assert_eq!(
        third["artifact_integrity_audit"]["priority_unverified_checked"].as_i64(),
        Some(1)
    );
    let fresh_state: String =
        sqlx::query_scalar("SELECT status FROM artifact_integrity_audit_state WHERE artifact_id=?")
            .bind(&fresh.artifact_id)
            .fetch_one(store.pool())
            .await
            .expect("fresh artifact audit state");
    assert_eq!(fresh_state, "verified");

    let full = store
        .run_reconciliation(Some(&project.project_id), "manual_full_audit")
        .await
        .expect("manual full audit");
    assert_eq!(
        full["artifact_integrity_audit"]["mode"].as_str(),
        Some("full")
    );
    assert_eq!(
        full["artifact_integrity_audit"]["checked_count"].as_i64(),
        Some(artifact_count + 1)
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
            &plan_scores(),
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
            &plan_scores(),
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
    let first_events = store
        .fail_local_attempt(None, &first, "schema mismatch")
        .await
        .expect("first failure");
    assert_eq!(first_events.len(), 1);
    assert!(
        store
            .fail_local_attempt(None, &first, "schema mismatch")
            .await
            .expect("idempotent first failure")
            .is_empty()
    );
    let failed_attempt_count: i64 =
        sqlx::query_scalar("SELECT failed_attempt_count FROM routes WHERE route_id=?")
            .bind(&task.route_id)
            .fetch_one(store.pool())
            .await
            .expect("failed attempt count");
    assert_eq!(failed_attempt_count, 1);
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
async fn reliability_v2_late_failure_cannot_reset_a_newer_attempt() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project(
            "stale-attempt-failure".into(),
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
            &plan_scores(),
            &delta,
            "primary",
            None,
        )
        .await
        .expect("v2 plan");
    let task = &saved.tasks[0];
    let first = store
        .offer_local_task(task, "mock", None, "runtime/stale-1", json!({}))
        .await
        .expect("first offer");
    let first_lease = store
        .accept_local_handshake(&first, Some("mock-1"), json!({}), 3_600)
        .await
        .expect("first lease");
    sqlx::query("UPDATE task_leases SET status='expired' WHERE lease_id=?")
        .bind(&first_lease.lease_id)
        .execute(store.pool())
        .await
        .expect("expire old lease fixture");
    sqlx::query("UPDATE task_attempts SET status='orphaned' WHERE attempt_id=?")
        .bind(&first.attempt.attempt_id)
        .execute(store.pool())
        .await
        .expect("orphan old attempt fixture");
    sqlx::query("UPDATE tasks SET status='queued',revision=revision+1 WHERE task_id=?")
        .bind(&task.task_id)
        .execute(store.pool())
        .await
        .expect("requeue fixture");
    let queued = store
        .get_task(&project.project_id, &task.task_id)
        .await
        .expect("queued task");
    let second = store
        .offer_local_task(&queued, "mock", None, "runtime/stale-2", json!({}))
        .await
        .expect("second offer");

    assert!(matches!(
        store
            .fail_local_attempt(Some(&first_lease), &first, "late worker failure")
            .await,
        Err(StorageError::LateSubmission(_))
    ));
    let current = store
        .get_task(&project.project_id, &task.task_id)
        .await
        .expect("current task");
    assert_eq!(current.status, research_domain::TaskStatus::Offered);
    assert_eq!(current.revision, second.task.revision);
    let second_attempt_status: String =
        sqlx::query_scalar("SELECT status FROM task_attempts WHERE attempt_id=?")
            .bind(&second.attempt.attempt_id)
            .fetch_one(store.pool())
            .await
            .expect("second attempt status");
    assert_eq!(second_attempt_status, "offered");
    let failed_attempt_count: i64 =
        sqlx::query_scalar("SELECT failed_attempt_count FROM routes WHERE route_id=?")
            .bind(&task.route_id)
            .fetch_one(store.pool())
            .await
            .expect("failed attempt count");
    assert_eq!(failed_attempt_count, 0);
}

#[tokio::test]
async fn cancelling_a_task_closes_its_entire_active_execution() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project(
            "cancel-active-execution".into(),
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
            &plan_scores(),
            &delta,
            "primary",
            None,
        )
        .await
        .expect("plan");
    let mut stale_task = saved.tasks[0].clone();
    stale_task.revision -= 1;
    assert!(matches!(
        store
            .offer_local_task(&stale_task, "mock", None, "runtime/stale-offer", json!({}),)
            .await,
        Err(StorageError::InvalidTransition(_))
    ));
    let offer = store
        .offer_local_task(
            &saved.tasks[0],
            "mock",
            None,
            "runtime/cancel-active",
            json!({}),
        )
        .await
        .expect("offer");
    let lease = store
        .accept_local_handshake(&offer, Some("mock-1"), json!({}), 3_600)
        .await
        .expect("lease");
    let output = WorkerOutput {
        summary: "awaiting ingestion".into(),
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
        .expect("submit envelope");
    let submitted_task = store
        .get_task(&project.project_id, &offer.task.task_id)
        .await
        .expect("submitted task");
    apply_test_command(
        &store,
        &project.project_id,
        "cancel_task",
        "task",
        &submitted_task.task_id,
        json!({"expected_task_revision":submitted_task.revision}),
        "cancel-submitted-task",
        "operator cancelled before ingestion",
    )
    .await;

    assert_eq!(
        store
            .get_task(&project.project_id, &submitted_task.task_id)
            .await
            .expect("cancelled task")
            .status,
        research_domain::TaskStatus::Cancelled
    );
    assert_eq!(
        store
            .get_task_lease(&lease.lease_id)
            .await
            .expect("cancelled lease")
            .status,
        "cancelled"
    );
    let attempt_status: String =
        sqlx::query_scalar("SELECT status FROM task_attempts WHERE attempt_id=?")
            .bind(&offer.attempt.attempt_id)
            .fetch_one(store.pool())
            .await
            .expect("attempt status");
    assert_eq!(attempt_status, "cancelled");
    let instance_status: String =
        sqlx::query_scalar("SELECT status FROM worker_instances WHERE worker_instance_id=?")
            .bind(&offer.worker_instance.worker_instance_id)
            .fetch_one(store.pool())
            .await
            .expect("instance status");
    assert_eq!(instance_status, "exited");
    let envelope_status: String =
        sqlx::query_scalar("SELECT status FROM result_envelopes WHERE result_envelope_id=?")
            .bind(&envelope_id)
            .fetch_one(store.pool())
            .await
            .expect("envelope status");
    assert_eq!(envelope_status, "stale");
    assert!(matches!(
        store
            .fail_local_attempt(Some(&lease), &offer, "late cancellation callback")
            .await,
        Err(StorageError::LateSubmission(_))
    ));
}

#[tokio::test]
async fn offer_persists_dead_letter_state_when_attempt_budget_is_already_exhausted() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("offer-attempt-budget".into(), contract(), Budget::default())
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
            &plan_scores(),
            &delta,
            "primary",
            None,
        )
        .await
        .expect("plan");
    let task = &saved.tasks[0];
    for attempt_no in 1_i64..=3 {
        sqlx::query("INSERT INTO task_attempts(attempt_id,project_id,task_id,attempt_no,status,lease_epoch,plan_revision_id,route_cancellation_epoch,context_packet_id,failure_signature,failure_reason,completed_at,created_at) VALUES(?,?,?,?, 'failed',?,?,?,?,?,?,?,?)")
            .bind(format!("attempt-budget-fixture-{attempt_no}"))
            .bind(&project.project_id)
            .bind(&task.task_id)
            .bind(attempt_no)
            .bind(attempt_no)
            .bind(&task.plan_revision_id)
            .bind(task.route_cancellation_epoch)
            .bind(&task.context_packet_id)
            .bind(format!("failure-{attempt_no}"))
            .bind("fixture failure")
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(store.pool())
            .await
            .expect("attempt fixture");
    }
    let prior_revision = task.revision;
    assert!(matches!(
        store
            .offer_local_task(task, "mock", None, "runtime/budget", json!({}))
            .await,
        Err(StorageError::InvalidTransition(_))
    ));
    let dead_lettered = store
        .get_task(&project.project_id, &task.task_id)
        .await
        .expect("dead-letter task");
    assert_eq!(
        dead_lettered.status,
        research_domain::TaskStatus::DeadLettered
    );
    assert_eq!(dead_lettered.revision, prior_revision + 1);
    let terminal_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE project_id=? AND type='task.dead_lettered' AND json_extract(entity_json,'$.id')=?",
    )
    .bind(&project.project_id)
    .bind(&task.task_id)
    .fetch_one(store.pool())
    .await
    .expect("dead-letter event");
    assert_eq!(terminal_events, 1);
}

#[tokio::test]
async fn startup_reconciliation_recovers_an_offer_without_a_lease() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("abandoned-offer".into(), contract(), Budget::default())
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
            &plan_scores(),
            &delta,
            "primary",
            None,
        )
        .await
        .expect("plan");
    let offer = store
        .offer_local_task(
            &saved.tasks[0],
            "mock",
            None,
            "runtime/abandoned",
            json!({}),
        )
        .await
        .expect("offer");

    let report = store
        .run_reconciliation(Some(&project.project_id), "service_startup")
        .await
        .expect("startup reconciliation");
    assert!(report["repairs"].as_array().is_some_and(|repairs| {
        repairs
            .iter()
            .any(|repair| repair["kind"] == "abandoned_task_offer_recovered")
    }));
    assert_eq!(
        store
            .get_task(&project.project_id, &offer.task.task_id)
            .await
            .expect("requeued task")
            .status,
        research_domain::TaskStatus::Queued
    );
    let attempt_status: String =
        sqlx::query_scalar("SELECT status FROM task_attempts WHERE attempt_id=?")
            .bind(&offer.attempt.attempt_id)
            .fetch_one(store.pool())
            .await
            .expect("attempt status");
    assert_eq!(attempt_status, "orphaned");
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
            &plan_scores(),
            &delta,
            "primary",
            None,
        )
        .await
        .expect("plan");
    let route_id = saved.routes[0].route_id.clone();
    let task = saved.tasks[0].clone();
    let offer = store
        .offer_local_task(&task, "mock", None, "runtime/prune", json!({}))
        .await
        .expect("offer");
    let lease = store
        .accept_local_handshake(&offer, Some("mock-1"), json!({}), 90)
        .await
        .expect("lease");
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
    assert_eq!(
        store
            .get_task(&project.project_id, &task.task_id)
            .await
            .expect("cancelled task")
            .status,
        research_domain::TaskStatus::Cancelled
    );
    let attempt_status: String =
        sqlx::query_scalar("SELECT status FROM task_attempts WHERE attempt_id=?")
            .bind(&lease.attempt_id)
            .fetch_one(store.pool())
            .await
            .expect("cancelled attempt");
    assert_eq!(attempt_status, "cancelled");
    let lease_status: String =
        sqlx::query_scalar("SELECT status FROM task_leases WHERE lease_id=?")
            .bind(&lease.lease_id)
            .fetch_one(store.pool())
            .await
            .expect("cancelled lease");
    assert_eq!(lease_status, "cancelled");
    let worker_instance_status: String =
        sqlx::query_scalar("SELECT status FROM worker_instances WHERE worker_instance_id=?")
            .bind(&lease.worker_instance_id)
            .fetch_one(store.pool())
            .await
            .expect("exited worker instance");
    assert_eq!(worker_instance_status, "exited");
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
    let (replayed, duplicate_events) = store
        .ingest_verification_result(&envelope_id)
        .await
        .expect("idempotent verification result ingestion");
    assert_eq!(replayed, result);
    assert!(duplicate_events.is_empty());
    let status: String =
        sqlx::query_scalar("SELECT status FROM verification_attempts WHERE attempt_id=?")
            .bind(&offer.attempt_id)
            .fetch_one(store.pool())
            .await
            .expect("attempt status");
    assert_eq!(status, "completed");
}

#[tokio::test]
async fn late_verification_failure_cannot_cross_a_project_stop_boundary() {
    let (store, _temp) = store().await;
    let (case, _formalization) = proof_search_fixture(&store).await;
    let offer = store
        .offer_verification_worker(
            &case.case_id,
            "math_review_1",
            "mock",
            None,
            "runtime/verifier-stop-fence",
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
    apply_test_command(
        &store,
        &case.project_id,
        "stop_project",
        "project",
        &case.project_id,
        json!({}),
        "verification-failure-stop-fence",
        "operator stop",
    )
    .await;
    let stopped_revision = store
        .get_project(&case.project_id)
        .await
        .expect("stopped project")
        .revision;

    assert!(matches!(
        store
            .fail_verification_worker(&offer, Some(&lease), "late worker failure")
            .await,
        Err(StorageError::LateSubmission(_))
    ));
    let attempt_status: String =
        sqlx::query_scalar("SELECT status FROM verification_attempts WHERE attempt_id=?")
            .bind(&offer.attempt_id)
            .fetch_one(store.pool())
            .await
            .expect("attempt status");
    assert_eq!(attempt_status, "cancelled");
    let lease_status: String = sqlx::query_scalar(
        "SELECT status FROM verification_task_leases WHERE verification_lease_id=?",
    )
    .bind(&lease.verification_lease_id)
    .fetch_one(store.pool())
    .await
    .expect("lease status");
    assert_eq!(lease_status, "cancelled");
    assert_eq!(
        store
            .get_project(&case.project_id)
            .await
            .expect("project after late failure")
            .revision,
        stopped_revision,
        "a fenced late failure must not append an event or bump revision"
    );
    let failure_events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE project_id=? AND type='verification.worker.failed' AND json_extract(entity_json,'$.id')=?")
        .bind(&case.project_id)
        .bind(&offer.attempt_id)
        .fetch_one(store.pool())
        .await
        .expect("failure event count");
    assert_eq!(failure_events, 0);
}

#[tokio::test]
async fn verification_failure_only_changes_the_latest_attempt_of_its_kind() {
    let (store, _temp) = store().await;
    let (case, _formalization) = proof_search_fixture(&store).await;
    let content = json!({"candidate":"1+1=2","active_facts":[]});
    let contract = json!({"success":"one independent verdict"});
    let first = store
        .offer_verification_worker(
            &case.case_id,
            "math_review_1",
            "mock",
            None,
            "runtime/verifier-first",
            &content,
            &[],
            &contract,
        )
        .await
        .expect("first verification offer");
    let second = store
        .offer_verification_worker(
            &case.case_id,
            "math_review_1",
            "mock",
            None,
            "runtime/verifier-second",
            &json!({"candidate":"1+1=2","active_facts":[],"retry":2}),
            &[],
            &contract,
        )
        .await
        .expect("second verification offer");

    assert!(matches!(
        store
            .fail_verification_worker(&first, None, "stale first failure")
            .await,
        Err(StorageError::LateSubmission(_))
    ));
    let first_status: String =
        sqlx::query_scalar("SELECT status FROM verification_attempts WHERE attempt_id=?")
            .bind(&first.attempt_id)
            .fetch_one(store.pool())
            .await
            .expect("first attempt status");
    assert_eq!(first_status, "offered");

    let events = store
        .fail_verification_worker(&second, None, "latest attempt failed")
        .await
        .expect("record latest failure");
    assert_eq!(events.len(), 1);
    assert!(
        store
            .fail_verification_worker(&second, None, "latest   attempt FAILED")
            .await
            .expect("same normalized failure is idempotent")
            .is_empty()
    );
    assert!(matches!(
        store
            .fail_verification_worker(&second, None, "different failure")
            .await,
        Err(StorageError::LateSubmission(_))
    ));
}

#[tokio::test]
async fn failed_verification_case_cannot_offer_another_worker() {
    let (store, _temp) = store().await;
    let (case, _formalization) = proof_search_fixture(&store).await;
    sqlx::query("UPDATE verification_cases SET stage='failed' WHERE case_id=?")
        .bind(&case.case_id)
        .execute(store.pool())
        .await
        .expect("mark case failed");

    assert!(matches!(
        store
            .offer_verification_worker(
                &case.case_id,
                "math_review_1",
                "mock",
                None,
                "runtime/verifier-terminal-case",
                &json!({"candidate":"1+1=2"}),
                &[],
                &json!({"success":"one independent verdict"}),
            )
            .await,
        Err(StorageError::InvalidTransition(_))
    ));
    let attempts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM verification_attempts WHERE case_id=?")
            .bind(&case.case_id)
            .fetch_one(store.pool())
            .await
            .expect("attempt count");
    assert_eq!(attempts, 0);
}

#[tokio::test]
async fn startup_reconciliation_ingests_submitted_verification_result_before_resuming() {
    let (store, _temp) = store().await;
    let (case, _formalization) = proof_search_fixture(&store).await;
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
            &json!({"verdict":"accepted","summary":"durable process output"}),
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
    assert_eq!(recovered_case.cancellation_epoch, case.cancellation_epoch);
    assert_eq!(recovered_case.stage, VerificationStage::Precheck);
    let envelope_status: String = sqlx::query_scalar(
        "SELECT status FROM verification_result_envelopes WHERE verification_result_envelope_id=?",
    )
    .bind(&envelope_id)
    .fetch_one(store.pool())
    .await
    .expect("envelope status");
    assert_eq!(envelope_status, "ingested");
    let attempt_status: String =
        sqlx::query_scalar("SELECT status FROM verification_attempts WHERE attempt_id=?")
            .bind(&offer.attempt_id)
            .fetch_one(store.pool())
            .await
            .expect("attempt status");
    assert_eq!(attempt_status, "completed");
    let replayed = store
        .replay_ingested_verification_result(
            &case.case_id,
            "math_review_1",
            &json!({"candidate":"1+1=2","active_facts":[]}),
        )
        .await
        .expect("replay lookup")
        .expect("recovered review");
    assert_eq!(replayed.0, offer.attempt_id);
    assert_eq!(replayed.1["summary"], "durable process output");
    assert!(
        store
            .replay_ingested_verification_result(
                &case.case_id,
                "math_review_1",
                &json!({"candidate":"different","active_facts":[]}),
            )
            .await
            .expect("mismatched context lookup")
            .is_none(),
        "an ingested result must only replay for the exact original context"
    );
    assert!(
        store
            .pending_verification_result_envelopes(&case.project_id)
            .await
            .expect("pending envelopes")
            .is_empty()
    );
    let facts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM facts WHERE project_id=?")
        .bind(&case.project_id)
        .fetch_one(store.pool())
        .await
        .expect("fact count");
    assert_eq!(facts, 0, "envelope ingestion must not bypass the Fact Gate");
    store
        .run_reconciliation(Some(&case.project_id), "service_startup")
        .await
        .expect("idempotent second startup reconciliation");
    let ingestion_events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE project_id=? AND type='verification.worker.result_ingested' AND json_extract(entity_json,'$.id')=?")
        .bind(&case.project_id).bind(&envelope_id).fetch_one(store.pool()).await.expect("ingestion event count");
    assert_eq!(ingestion_events, 1);
}

#[tokio::test]
async fn startup_reconciliation_stales_verification_result_with_changed_epoch() {
    let (store, _temp) = store().await;
    let (case, _formalization) = proof_search_fixture(&store).await;
    let offer = store
        .offer_verification_worker(
            &case.case_id,
            "math_review_1",
            "mock",
            None,
            "runtime/verifier-stale-restart",
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
            &json!({"verdict":"accepted","summary":"old epoch output"}),
        )
        .await
        .expect("submit pending verification result");
    sqlx::query(
        "UPDATE verification_cases SET cancellation_epoch=cancellation_epoch+1 WHERE case_id=?",
    )
    .bind(&case.case_id)
    .execute(store.pool())
    .await
    .expect("advance cancellation epoch");

    store
        .run_reconciliation(Some(&case.project_id), "service_startup")
        .await
        .expect("startup reconciliation");

    let envelope_status: String = sqlx::query_scalar(
        "SELECT status FROM verification_result_envelopes WHERE verification_result_envelope_id=?",
    )
    .bind(&envelope_id)
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
async fn verification_result_ingestion_rejects_tampered_content_hash() {
    let (store, _temp) = store().await;
    let (case, _formalization) = proof_search_fixture(&store).await;
    let offer = store
        .offer_verification_worker(
            &case.case_id,
            "math_review_1",
            "mock",
            None,
            "runtime/verifier-tamper",
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
            &json!({"verdict":"accepted","summary":"original"}),
        )
        .await
        .expect("submit verification result");
    sqlx::query("UPDATE verification_result_envelopes SET envelope_json=? WHERE verification_result_envelope_id=?")
        .bind(json!({"verdict":"accepted","summary":"tampered"}).to_string())
        .bind(&envelope_id)
        .execute(store.pool())
        .await
        .expect("tamper envelope payload");

    assert!(matches!(
        store.ingest_verification_result(&envelope_id).await,
        Err(StorageError::LateSubmission(_))
    ));
    let status: String = sqlx::query_scalar(
        "SELECT status FROM verification_result_envelopes WHERE verification_result_envelope_id=?",
    )
    .bind(envelope_id)
    .fetch_one(store.pool())
    .await
    .expect("envelope status");
    assert_eq!(status, "stale");
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
    let obsolete_generic_dedup_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM domain_command_dedup WHERE project_id=?")
            .bind(&project.project_id)
            .fetch_one(store.pool())
            .await
            .expect("legacy generic dedup table");
    assert_eq!(obsolete_generic_dedup_rows, 0);
}

#[tokio::test]
async fn durable_command_recovery_only_replays_queued_commands_once() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project(
            "durable command recovery".into(),
            contract(),
            Budget::default(),
        )
        .await
        .expect("create");
    let (command, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "add_suggestion".into(),
                target_kind: "project".into(),
                target_id: project.project_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({"content": "try a contradiction argument"}),
                expected_project_revision: project.revision,
                idempotency_key: "durable-suggestion".into(),
                reason: "exercise process-restart recovery".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue");
    assert_eq!(
        store
            .pending_command_ids(Some(&project.project_id))
            .await
            .expect("pending commands"),
        vec![(project.project_id.clone(), command.command_id.clone())]
    );

    let (validated, first_events) = store
        .apply_command(&project.project_id, &command.command_id)
        .await
        .expect("first dispatch");
    assert_eq!(validated.status, research_domain::CommandStatus::Validated);
    assert_eq!(first_events.len(), 1);
    let (replayed, replay_events) = store
        .apply_command(&project.project_id, &command.command_id)
        .await
        .expect("replayed dispatch");
    assert_eq!(replayed.status, research_domain::CommandStatus::Validated);
    assert!(replay_events.is_empty());
    assert!(
        store
            .pending_command_ids(Some(&project.project_id))
            .await
            .expect("pending commands after validation")
            .is_empty()
    );
    let suggestion_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM suggestions WHERE command_id=?")
            .bind(&command.command_id)
            .fetch_one(store.pool())
            .await
            .expect("suggestion count");
    assert_eq!(suggestion_count, 1);
}

#[tokio::test]
async fn plan_fact_decisions_are_exposed_in_the_context_digest() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project(
            "fact-decision-read-path".into(),
            contract(),
            Budget::default(),
        )
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("round");
    let snapshot = store.snapshot(&project.project_id).await.expect("snapshot");
    let goal_id = &snapshot.goals[0].goal_id;
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("INSERT INTO facts(fact_id,project_id,statement,assumptions_json,proof_markdown,dependency_fact_ids_json,definitions_introduced_json,external_source_ids_json,verification_ids_json,evidence_level,created_by,status,content_hash,created_at) VALUES('fact-decision-test',?,'test fact','[]','verified fixture','[]','{}','[]','[]','reviewed','test','active','fact-decision-test-hash',?)")
        .bind(&project.project_id)
        .bind(&now)
        .execute(store.pool())
        .await
        .expect("fact fixture");
    sqlx::query("INSERT INTO fact_impact_records(impact_id,project_id,fact_id,source_revision,closed_goal_ids_json,unblocked_route_ids_json,invalidated_task_ids_json,newly_enabled_task_templates_json,dominated_route_ids_json,resolved_uncertainty_ids_json,planner_disposition,created_at) VALUES('impact-decision-test',?,'fact-decision-test',?,?,'[]','[]','[]','[]','[]','pending',?)")
        .bind(&project.project_id)
        .bind(snapshot.project_revision)
        .bind(json!([goal_id]).to_string())
        .bind(&now)
        .execute(store.pool())
        .await
        .expect("impact fixture");
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    store
        .save_plan_v2(
            &project.project_id,
            &round,
            &plan(),
            &plan_scores(),
            &delta,
            "fact decision read path",
            None,
        )
        .await
        .expect("plan");

    let digest = store
        .context_digest(&project.project_id)
        .await
        .expect("context digest");
    let decisions = digest["recent_plan_fact_decisions"]
        .as_array()
        .expect("plan fact decisions");
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0]["fact_id"], "fact-decision-test");
    assert_eq!(decisions[0]["disposition"], "consumed");
    assert_eq!(decisions[0]["affected_entity_ids"], json!([goal_id]));
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
async fn verification_claim_is_atomic_and_idempotent() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("verification-claim".into(), contract(), Budget::default())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store.begin_round(&project.project_id).await.expect("begin");
    let saved = store
        .save_plan(&project.project_id, &round, &plan())
        .await
        .expect("plan");
    let (task, _) = store
        .mark_task_running(&project.project_id, &saved.tasks[0].task_id)
        .await
        .expect("running");
    let receipt = store
        .submit_candidate(
            &project.project_id,
            CandidateSubmission {
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
            },
            "verification-claim-candidate",
        )
        .await
        .expect("candidate");

    let verification_id = &receipt.verification.verification_id;
    let (first, second) = tokio::join!(
        store.try_mark_verification_started(verification_id),
        store.try_mark_verification_started(verification_id),
    );
    let claims = [first.expect("first claim"), second.expect("second claim")]
        .into_iter()
        .flatten()
        .count();
    assert_eq!(claims, 1);
    assert!(
        store
            .try_mark_verification_started(verification_id)
            .await
            .expect("idempotent retry")
            .is_none()
    );
    assert!(
        store
            .release_verification_claim(verification_id, "simulated orchestrator failure")
            .await
            .expect("release claim")
            .is_some()
    );
    assert!(
        store
            .try_mark_verification_started(verification_id)
            .await
            .expect("claim after release")
            .is_some()
    );
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
async fn candidate_cannot_forge_the_current_route_epoch_for_an_old_completed_task() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project(
            "candidate-epoch-fence".into(),
            contract(),
            Budget::default(),
        )
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
                summary: "completed before the contract boundary moved".into(),
                discoveries: vec![],
                candidates: vec![],
                failures: vec![],
                uncertainties: vec![],
                sources: vec![],
                experiments: vec![],
            },
        )
        .await
        .expect("complete task");
    let completed = store
        .get_task(&project.project_id, &running.task_id)
        .await
        .expect("completed task");

    // A problem revision advances the route epoch but cannot rewrite the
    // immutable epoch captured by a task that already completed. A caller must
    // not be able to forge the new route epoch in its CandidateSubmission.
    sqlx::query("UPDATE routes SET cancellation_epoch=cancellation_epoch+1 WHERE route_id=?")
        .bind(&completed.route_id)
        .execute(store.pool())
        .await
        .expect("advance route epoch");
    let forged_epoch = completed.route_cancellation_epoch + 1;
    let error = store
        .submit_candidate(
            &project.project_id,
            CandidateSubmission {
                task_id: completed.task_id.clone(),
                route_id: completed.route_id.clone(),
                target_goal_ids: completed.goal_ids.clone(),
                statement: "stale result".into(),
                assumptions: vec![],
                proof_markdown: "proof produced for the superseded contract".into(),
                dependency_fact_ids: vec![],
                definitions_introduced: Default::default(),
                external_source_ids: vec![],
                candidate_type: CandidateType::Lemma,
                task_revision: completed.revision,
                route_cancellation_epoch: forged_epoch,
            },
            "forged-current-route-epoch",
        )
        .await
        .expect_err("the task epoch and current route epoch must both match");
    assert!(matches!(error, StorageError::LateSubmission(_)));
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
    let (source_artifact, _) = store
        .store_artifact(
            &project.project_id,
            "source_fulltext",
            "legacy-source.txt",
            b"archived source",
            round.number,
            vec![running.task_id.clone()],
        )
        .await
        .expect("source artifact");
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
                        fulltext_sha256: Some(source_artifact.sha256.clone()),
                        fulltext_artifact_id: Some(source_artifact.artifact_id.clone()),
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
                        fulltext_sha256: Some(source_artifact.sha256.clone()),
                        fulltext_artifact_id: Some(source_artifact.artifact_id.clone()),
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
                        fulltext_sha256: Some(source_artifact.sha256.clone()),
                        fulltext_artifact_id: Some(source_artifact.artifact_id.clone()),
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
    start_project(&store, &project.project_id, project.revision).await;
    let first = store
        .reserve_model_call(ModelCallRequest {
            project_id: &project.project_id,
            round: 1,
            worker_id: Some("worker-1"),
            task_id: Some("task-1"),
            model: None,
            purpose: ModelCallPurpose::Research,
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
                purpose: ModelCallPurpose::Research,
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
            purpose: ModelCallPurpose::Research,
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
                purpose: ModelCallPurpose::Research,
                max_total_calls: 2,
                max_task_calls: 1,
            },)
            .await,
        Err(StorageError::BudgetExhausted(_))
    ));
}

#[tokio::test]
async fn model_call_reservations_reject_stale_limits_after_budget_is_lowered() {
    let (store, _temp) = store().await;
    let initial_budget = Budget {
        max_total_model_calls: 4,
        max_model_calls_per_task: 4,
        ..Budget::default()
    };
    let (project, _) = store
        .create_project("stale budget".into(), contract(), initial_budget.clone())
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let stale_request = |task_id| ModelCallRequest {
        project_id: &project.project_id,
        round: 1,
        worker_id: None,
        task_id: Some(task_id),
        model: None,
        purpose: ModelCallPurpose::Research,
        max_total_calls: initial_budget.max_total_model_calls,
        max_task_calls: initial_budget.max_model_calls_per_task,
    };

    let first = store
        .reserve_model_call(stale_request("task-1"))
        .await
        .expect("first reservation");
    store
        .finish_model_call(&first.usage_id, 1)
        .await
        .expect("finish first reservation");

    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project")
        .revision;
    let lowered_budget = Budget {
        max_total_model_calls: 2,
        max_model_calls_per_task: 1,
        ..initial_budget
    };
    let (command, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "adjust_budget".into(),
                target_kind: "project".into(),
                target_id: project.project_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({
                    "scope_kind": "project",
                    "scope_id": project.project_id,
                    "limits": lowered_budget,
                }),
                expected_project_revision: revision,
                idempotency_key: "lower-model-call-budget".into(),
                reason: "exercise stale caller protection".into(),
                requested_by: "operator".into(),
            },
        )
        .await
        .expect("enqueue budget adjustment");
    store
        .apply_command(&project.project_id, &command.command_id)
        .await
        .expect("lower budget");

    assert!(matches!(
        store.reserve_model_call(stale_request("task-1")).await,
        Err(StorageError::BudgetExhausted(_))
    ));
    let second = store
        .reserve_model_call(stale_request("task-2"))
        .await
        .expect("a different task may consume the final current-budget call");
    store
        .finish_model_call(&second.usage_id, 1)
        .await
        .expect("finish second reservation");
    assert!(matches!(
        store.reserve_model_call(stale_request("task-3")).await,
        Err(StorageError::BudgetExhausted(_))
    ));
    assert_eq!(
        store
            .total_model_calls(&project.project_id)
            .await
            .expect("usage"),
        2
    );
}

#[tokio::test]
async fn model_call_request_limit_can_tighten_a_database_task_override() {
    let (store, _temp) = store().await;
    let budget = Budget {
        max_total_model_calls: 5,
        max_model_calls_per_task: 5,
        ..Budget::default()
    };
    let (project, _) = store
        .create_project("caller budget ceiling".into(), contract(), budget)
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("started project")
        .revision;
    let (command, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "adjust_budget".into(),
                target_kind: "task".into(),
                target_id: "task-1".into(),
                mode: CommandMode::Immediate,
                payload: json!({
                    "scope_kind": "task",
                    "scope_id": "task-1",
                    "limits": {"max_model_calls": 3},
                }),
                expected_project_revision: revision,
                idempotency_key: "task-model-call-override".into(),
                reason: "allow a larger database task allowance".into(),
                requested_by: "operator".into(),
            },
        )
        .await
        .expect("enqueue task budget override");
    store
        .apply_command(&project.project_id, &command.command_id)
        .await
        .expect("apply task budget override");

    let request = || ModelCallRequest {
        project_id: &project.project_id,
        round: 1,
        worker_id: None,
        task_id: Some("task-1"),
        model: None,
        purpose: ModelCallPurpose::Research,
        max_total_calls: 5,
        max_task_calls: 1,
    };
    let first = store
        .reserve_model_call(request())
        .await
        .expect("first caller-limited reservation");
    store
        .finish_model_call(&first.usage_id, 1)
        .await
        .expect("finish reservation");
    assert!(matches!(
        store.reserve_model_call(request()).await,
        Err(StorageError::BudgetExhausted(_))
    ));
}

#[tokio::test]
async fn publication_model_calls_have_a_narrow_terminal_project_exception() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project(
            "publication model purpose".into(),
            contract(),
            Budget::default(),
        )
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    sqlx::query("UPDATE projects SET status='success' WHERE project_id=?")
        .bind(&project.project_id)
        .execute(store.pool())
        .await
        .expect("terminal publication fixture");
    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project")
        .revision;
    let (publication, _) = store
        .begin_publication(
            &project.project_id,
            "publication-purpose",
            false,
            revision,
            "packet-hash",
        )
        .await
        .expect("begin publication");

    let publication_call = store
        .reserve_model_call(ModelCallRequest {
            project_id: &project.project_id,
            round: 1,
            worker_id: None,
            task_id: None,
            model: Some("paper-writer"),
            purpose: ModelCallPurpose::Publication(&publication.publication_id),
            max_total_calls: 10,
            max_task_calls: 10,
        })
        .await
        .expect("active publication may call its writer after success");
    store
        .finish_model_call(&publication_call.usage_id, 1)
        .await
        .expect("finish publication call");
    assert!(matches!(
        store
            .reserve_model_call(ModelCallRequest {
                project_id: &project.project_id,
                round: 1,
                worker_id: None,
                task_id: None,
                model: Some("research-worker"),
                purpose: ModelCallPurpose::Research,
                max_total_calls: 10,
                max_task_calls: 10,
            })
            .await,
        Err(StorageError::InvalidTransition(_))
    ));
    assert!(matches!(
        store
            .reserve_model_call(ModelCallRequest {
                project_id: &project.project_id,
                round: 1,
                worker_id: None,
                task_id: None,
                model: Some("paper-writer"),
                purpose: ModelCallPurpose::Publication("unknown-publication"),
                max_total_calls: 10,
                max_task_calls: 10,
            })
            .await,
        Err(StorageError::InvalidTransition(_))
    ));
}

#[tokio::test]
async fn exhausted_project_distinguishes_backend_failure_from_partial_research() {
    let (store, _temp) = store().await;
    let (failed_project, _) = store
        .create_project("unavailable backend".into(), contract(), Budget::default())
        .await
        .expect("create failed project");
    start_project(&store, &failed_project.project_id, failed_project.revision).await;
    let failed_call = store
        .reserve_model_call(ModelCallRequest {
            project_id: &failed_project.project_id,
            round: 1,
            worker_id: None,
            task_id: None,
            model: Some("missing-model"),
            purpose: ModelCallPurpose::Research,
            max_total_calls: 1,
            max_task_calls: 1,
        })
        .await
        .expect("reserve failed call");
    store
        .fail_model_call(
            &failed_call.usage_id,
            5,
            "process",
            "HTTP 404 model route not found",
        )
        .await
        .expect("record failed call");
    let failed_event = store
        .mark_budget_exhausted(&failed_project.project_id)
        .await
        .expect("mark environment failure");
    assert_eq!(failed_event.event_type, "project.environment_failed");
    assert_eq!(
        store
            .get_project(&failed_project.project_id)
            .await
            .expect("failed project")
            .status,
        research_domain::ProjectStatus::EnvironmentFailed
    );

    let (partial_project, _) = store
        .create_project("partial work".into(), contract(), Budget::default())
        .await
        .expect("create partial project");
    start_project(
        &store,
        &partial_project.project_id,
        partial_project.revision,
    )
    .await;
    let successful_call = store
        .reserve_model_call(ModelCallRequest {
            project_id: &partial_project.project_id,
            round: 1,
            worker_id: None,
            task_id: None,
            model: Some("working-model"),
            purpose: ModelCallPurpose::Research,
            max_total_calls: 1,
            max_task_calls: 1,
        })
        .await
        .expect("reserve successful call");
    store
        .finish_model_call_with_tokens(&successful_call.usage_id, 5, 10, 2)
        .await
        .expect("record successful call");
    let partial_event = store
        .mark_budget_exhausted(&partial_project.project_id)
        .await
        .expect("mark partial result");
    assert_eq!(partial_event.event_type, "project.budget_exhausted");
    assert_eq!(
        store
            .get_project(&partial_project.project_id)
            .await
            .expect("partial project")
            .status,
        research_domain::ProjectStatus::PartialSuccess
    );
}

#[tokio::test]
async fn completing_the_last_round_atomically_persists_partial_success() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project(
            "last configured round".into(),
            contract(),
            Budget {
                max_rounds: 1,
                ..Budget::default()
            },
        )
        .await
        .expect("create project");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store
        .begin_round(&project.project_id)
        .await
        .expect("begin last round");
    sqlx::query("UPDATE rounds SET status='running' WHERE round_id=?")
        .bind(&round.round_id)
        .execute(store.pool())
        .await
        .expect("round entered execution");

    let events = store
        .complete_round(
            &project.project_id,
            &round.round_id,
            "bounded partial result",
        )
        .await
        .expect("complete and terminalize last round");

    let snapshot = store
        .snapshot(&project.project_id)
        .await
        .expect("terminal snapshot");
    assert_eq!(
        snapshot.project.status,
        research_domain::ProjectStatus::PartialSuccess
    );
    assert_eq!(
        snapshot.current_round.expect("round").status,
        research_domain::RoundStatus::Completed
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "project.budget_exhausted")
    );

    let (call_limited, _) = store
        .create_project(
            "last configured call".into(),
            contract(),
            Budget {
                max_rounds: 3,
                max_total_model_calls: 1,
                ..Budget::default()
            },
        )
        .await
        .expect("create call-limited project");
    start_project(&store, &call_limited.project_id, call_limited.revision).await;
    let (call_limited_round, _) = store
        .begin_round(&call_limited.project_id)
        .await
        .expect("begin call-limited round");
    sqlx::query("UPDATE rounds SET status='running' WHERE round_id=?")
        .bind(&call_limited_round.round_id)
        .execute(store.pool())
        .await
        .expect("call-limited round entered execution");
    let usage = store
        .reserve_model_call(ModelCallRequest {
            project_id: &call_limited.project_id,
            round: call_limited_round.number,
            worker_id: None,
            task_id: None,
            model: Some("test"),
            purpose: ModelCallPurpose::Research,
            max_total_calls: 1,
            max_task_calls: 1,
        })
        .await
        .expect("reserve final call");
    store
        .finish_model_call(&usage.usage_id, 1)
        .await
        .expect("finish final call");

    let call_events = store
        .complete_round(
            &call_limited.project_id,
            &call_limited_round.round_id,
            "call-bounded partial result",
        )
        .await
        .expect("complete round at call limit");
    assert_eq!(
        store
            .get_project(&call_limited.project_id)
            .await
            .expect("call-limited terminal project")
            .status,
        research_domain::ProjectStatus::PartialSuccess
    );
    assert!(call_events.iter().any(|event| {
        event.event_type == "project.budget_exhausted"
            && event.data["round_limit_reached"] == false
            && event.data["model_call_limit_reached"] == true
    }));
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
    for (content_hash, fact_id, statement) in [("hash-a", "fact-a", "A"), ("hash-b", "fact-b", "B")]
    {
        sqlx::query("INSERT INTO global_fact_catalog(content_hash,source_project_id,source_fact_id,statement,status,updated_at) VALUES(?,?,?,?, 'active',?)")
            .bind(content_hash)
            .bind(&project.project_id)
            .bind(fact_id)
            .bind(statement)
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("catalog fixture");
    }
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
    // A Fact intentionally has at most one open governance review. Close the
    // first review through the public control path before exercising the next
    // review kind; this keeps the legacy coverage aligned with that invariant.
    store
        .govern_fact(
            "fact-a",
            "suspend",
            "close the first audit fixture",
            "reviewer",
            "close-challenge-1",
        )
        .await
        .expect("close first challenge");
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
    store
        .govern_fact(
            "fact-a",
            "suspend",
            "close the formalization review fixture",
            "reviewer",
            "close-formalization-1",
        )
        .await
        .expect("close formalization challenge");
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
    let catalog_states = sqlx::query_as::<_, (String, String)>(
        "SELECT source_fact_id,status FROM global_fact_catalog WHERE source_fact_id IN ('fact-a','fact-b') ORDER BY source_fact_id",
    )
    .fetch_all(store.pool())
    .await
    .expect("catalog states");
    assert_eq!(
        catalog_states,
        vec![
            ("fact-a".into(), "revoked".into()),
            ("fact-b".into(), "suspended".into()),
        ]
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
async fn p1_control_plane_holds_steering_until_result_commit_and_guards_budgets() {
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
        .pending_task_steers(
            &project.project_id,
            &running.task_id,
            running.revision,
            running.route_cancellation_epoch,
        )
        .await
        .expect("safe point");
    assert_eq!(steers.len(), 1);
    assert_eq!(steers[0].content, "try the invariant first");
    assert!(events.is_empty());
    assert_eq!(steers[0].status, "pending");
    assert_eq!(
        store
            .get_command(&project.project_id, &steer_command.command_id)
            .await
            .expect("command")
            .status,
        research_domain::CommandStatus::WaitingSafePoint
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
            purpose: ModelCallPurpose::Research,
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
        Err(StorageError::InvalidTransition(message)) if message.contains("greater than zero")
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
    store
        .mark_verification_started(&receipt.verification.verification_id)
        .await
        .expect("verification started");
    let (case, _) = store
        .ensure_verification_case(
            &receipt.verification.verification_id,
            VerificationCaseDraft {
                name: "formal".into(),
                profile: VerificationProfile::FormalRequired,
                required_acceptance: AcceptanceClass::FormallyVerified,
                required_checks: vec![
                    "deterministic_precheck".into(),
                    "math_review_1".into(),
                    "math_review_2".into(),
                    "reviewer_independence".into(),
                    "adversarial_review".into(),
                    "semantic_contract".into(),
                    "alignment_review".into(),
                    "lean_kernel".into(),
                    "package_integrity".into(),
                    "fresh_replay".into(),
                ],
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
    store
        .create_verification_snapshot(
            &case.case_id,
            VerificationSnapshotDraft {
                toolchain_hash: None,
                extra_payload: json!({"fixture":"proof_search"}),
            },
        )
        .await
        .expect("verification snapshot");
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
async fn stopping_a_project_persists_a_complete_execution_fence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp
        .path()
        .join("hard-stop.sqlite")
        .to_string_lossy()
        .replace('\\', "/");
    let database_url = format!("sqlite://{path}");
    let store = SqliteStore::connect(&database_url, temp.path().join("artifacts"))
        .await
        .expect("file-backed store");
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
                goal: "open goal".into(),
                local_context: json!([]),
                tactic: None,
                score: 1.0,
                status: ProofNodeStatus::Open,
                diagnostic: None,
            },
        )
        .await
        .expect("proof search");
    let revision = store
        .get_project(&case.project_id)
        .await
        .expect("project")
        .revision;
    let (pending, _) = store
        .enqueue_command(
            &case.project_id,
            CommandDraft {
                command_type: "trigger_replan".into(),
                target_kind: "project".into(),
                target_id: case.project_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: revision,
                idempotency_key: "pending-before-stop".into(),
                reason: "durability fixture".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("pending command");
    let active_usage = store
        .reserve_model_call(ModelCallRequest {
            project_id: &case.project_id,
            round: 1,
            worker_id: None,
            task_id: None,
            model: None,
            purpose: ModelCallPurpose::Research,
            max_total_calls: 10,
            max_task_calls: 10,
        })
        .await
        .expect("active model call");
    apply_test_command(
        &store,
        &case.project_id,
        "stop_project",
        "project",
        &case.project_id,
        json!({}),
        "hard-stop",
        "operator stop",
    )
    .await;
    let current_round = store
        .current_round(&case.project_id)
        .await
        .expect("round")
        .expect("current round");
    assert!(matches!(
        store
            .complete_round(&case.project_id, &current_round.round_id, "late completion")
            .await,
        Err(StorageError::InvalidTransition(_))
    ));
    store
        .finish_model_call_with_tokens(&active_usage.usage_id, 50, 20, 10)
        .await
        .expect("late usage completion is idempotently fenced");
    let usage_outcome: String =
        sqlx::query_scalar("SELECT outcome FROM usage_records WHERE usage_id=?")
            .bind(&active_usage.usage_id)
            .fetch_one(store.pool())
            .await
            .expect("usage outcome");
    assert_eq!(usage_outcome, "cancelled");
    assert!(matches!(
        store
            .reserve_model_call(ModelCallRequest {
                project_id: &case.project_id,
                round: 1,
                worker_id: None,
                task_id: None,
                model: None,
                purpose: ModelCallPurpose::Research,
                max_total_calls: 10,
                max_task_calls: 10,
            })
            .await,
        Err(StorageError::InvalidTransition(_))
    ));
    assert!(
        store
            .try_mark_verification_started(&case.verification_id)
            .await
            .expect("verification claim")
            .is_none()
    );
    assert_eq!(
        store
            .get_command(&case.project_id, &pending.command_id)
            .await
            .expect("superseded command")
            .status,
        research_domain::CommandStatus::Superseded
    );
    drop(store);

    let reopened = SqliteStore::connect(&database_url, temp.path().join("artifacts"))
        .await
        .expect("reopen store");
    assert_eq!(
        reopened
            .get_project(&case.project_id)
            .await
            .expect("stopped project")
            .status,
        research_domain::ProjectStatus::StoppedByHuman
    );
    assert_eq!(
        reopened
            .get_verification(&case.verification_id)
            .await
            .expect("interrupted verification")
            .status,
        research_domain::CandidateStatus::Unknown
    );
    assert_eq!(
        reopened
            .get_proof_search(&search.search_id)
            .await
            .expect("cancelled search")
            .status,
        research_domain::CheckStatus::Cancelled
    );
    let node_status: String = sqlx::query_scalar("SELECT status FROM proof_nodes WHERE node_id=?")
        .bind(&root.node_id)
        .fetch_one(reopened.pool())
        .await
        .expect("cancelled node");
    assert_eq!(node_status, "cancelled");
    assert!(
        reopened
            .pending_command_ids(Some(&case.project_id))
            .await
            .expect("pending commands")
            .is_empty()
    );
}

#[tokio::test]
async fn project_stop_closes_planning_and_publication_subsystems() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project(
            "stop planning and publication".into(),
            contract(),
            Budget::default(),
        )
        .await
        .expect("create");
    start_project(&store, &project.project_id, project.revision).await;
    let (round, _) = store
        .begin_round(&project.project_id)
        .await
        .expect("planning round");
    let (planning_attempt_id, _) = store
        .begin_planning_stage_attempt(
            &project.project_id,
            &round.round_id,
            "route_generator",
            "stop-fixture-input",
            1,
            (30, 60),
        )
        .await
        .expect("planning attempt");
    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project")
        .revision;
    let (publication, _) = store
        .begin_publication(
            &project.project_id,
            "stop-publication",
            true,
            revision,
            "stop-packet-hash",
        )
        .await
        .expect("publication");

    apply_test_command(
        &store,
        &project.project_id,
        "stop_project",
        "project",
        &project.project_id,
        json!({}),
        "stop-planning-publication",
        "operator stop",
    )
    .await;

    let planning_status: String =
        sqlx::query_scalar("SELECT status FROM planning_stage_attempts WHERE attempt_id=?")
            .bind(&planning_attempt_id)
            .fetch_one(store.pool())
            .await
            .expect("planning attempt status");
    assert_eq!(planning_status, "failed");
    assert_eq!(
        store
            .get_publication(&publication.publication_id)
            .await
            .expect("publication")
            .status,
        "failed"
    );
    let artifact_count_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM artifacts WHERE project_id=?")
            .bind(&project.project_id)
            .fetch_one(store.pool())
            .await
            .expect("artifact count before late publication write");
    assert!(matches!(
        store
            .store_publication_artifact(
                &publication.publication_id,
                &project.project_id,
                "paper_markdown",
                "paper.md",
                b"late publication output",
                0,
            )
            .await,
        Err(StorageError::LateSubmission(_))
    ));
    let artifact_count_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM artifacts WHERE project_id=?")
            .bind(&project.project_id)
            .fetch_one(store.pool())
            .await
            .expect("artifact count after late publication write");
    assert_eq!(artifact_count_after, artifact_count_before);
    let (authoritative, completion_event) = store
        .complete_publication(
            &publication.publication_id,
            "ready",
            Some(&json!({"status":"ready"})),
            None,
        )
        .await
        .expect("late completion resolves to the existing terminal publication");
    assert_eq!(authoritative.status, "failed");
    assert!(completion_event.is_none());
    let (replayed, replay_event) = store
        .begin_publication(
            &project.project_id,
            "stop-publication",
            true,
            revision,
            "stop-packet-hash",
        )
        .await
        .expect("terminal publication replay");
    assert_eq!(replayed.status, "failed");
    assert!(replay_event.is_none());
    assert!(matches!(
        store
            .begin_publication(
                &project.project_id,
                "post-stop-publication",
                true,
                revision,
                "post-stop-packet-hash",
            )
            .await,
        Err(StorageError::InvalidTransition(_))
    ));
    assert!(matches!(
        store
            .begin_planning_stage_attempt(
                &project.project_id,
                &round.round_id,
                "route_generator",
                "post-stop-input",
                2,
                (30, 60),
            )
            .await,
        Err(StorageError::InvalidTransition(_))
    ));
}

#[tokio::test]
async fn explicit_resume_after_stop_preserves_cancelled_planning() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project("explicit stop resume".into(), contract(), Budget::default())
        .await
        .expect("create");
    let id = &project.project_id;
    start_project(&store, id, project.revision).await;
    let (round, _) = store.begin_round(id).await.expect("round");
    let (attempt, _) = store
        .begin_planning_stage_attempt(
            id,
            &round.round_id,
            "route_generator",
            "resume-input",
            1,
            (30, 60),
        )
        .await
        .expect("attempt");
    apply_test_command(
        &store,
        id,
        "stop_project",
        "project",
        id,
        json!({}),
        "stop-for-resume",
        "stop",
    )
    .await;
    assert_eq!(
        store.get_project(id).await.expect("stopped").status,
        research_domain::ProjectStatus::StoppedByHuman
    );
    apply_test_command(
        &store,
        id,
        "resume_project",
        "project",
        id,
        json!({}),
        "explicit-resume",
        "user authorized recovery",
    )
    .await;
    assert_eq!(
        store.get_project(id).await.expect("resumed").status,
        research_domain::ProjectStatus::Running
    );
    let status: String =
        sqlx::query_scalar("SELECT status FROM planning_stage_attempts WHERE attempt_id=?")
            .bind(&attempt)
            .fetch_one(store.pool())
            .await
            .expect("attempt status");
    assert_eq!(status, "failed", "resume must not resurrect old attempts");
    store
        .begin_round(id)
        .await
        .expect("fresh planning is possible");
}

#[tokio::test]
async fn budget_terminal_resume_requires_headroom_and_preserves_round_history() {
    let (store, _temp) = store().await;
    let budget = Budget {
        max_rounds: 1,
        ..Budget::default()
    };
    let (project, _) = store
        .create_project("budget resume".into(), contract(), budget.clone())
        .await
        .expect("create");
    let id = &project.project_id;
    start_project(&store, id, project.revision).await;
    let (round, _) = store.begin_round(id).await.expect("round");
    sqlx::query("UPDATE rounds SET status='running' WHERE round_id=?")
        .bind(&round.round_id)
        .execute(store.pool())
        .await
        .expect("fixture");
    store
        .complete_round(id, &round.round_id, "partial work")
        .await
        .expect("budget terminal");
    let current = store.get_project(id).await.expect("project");
    let (resume, _) = store
        .enqueue_command(
            id,
            CommandDraft {
                command_type: "resume_project".into(),
                target_kind: "project".into(),
                target_id: id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: current.revision,
                idempotency_key: "no-headroom".into(),
                reason: "test".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue");
    assert!(store.apply_command(id, &resume.command_id).await.is_err());
    assert_eq!(
        store.get_project(id).await.expect("unchanged").status,
        research_domain::ProjectStatus::PartialSuccess
    );
    let increased = Budget {
        max_rounds: 5,
        ..budget
    };
    apply_test_command(
        &store,
        id,
        "adjust_budget",
        "project",
        id,
        json!({"scope_kind":"project","scope_id":id,"limits":increased}),
        "add-headroom",
        "continue research",
    )
    .await;
    apply_test_command(
        &store,
        id,
        "resume_project",
        "project",
        id,
        json!({}),
        "resume-budget",
        "explicit recovery",
    )
    .await;
    let resumed = store.get_project(id).await.expect("resumed");
    assert_eq!(resumed.status, research_domain::ProjectStatus::Running);
    assert_eq!(resumed.current_round, 1);
    let old_status: String = sqlx::query_scalar("SELECT status FROM rounds WHERE round_id=?")
        .bind(&round.round_id)
        .fetch_one(store.pool())
        .await
        .expect("old round");
    assert_eq!(old_status, "completed");
    assert_eq!(store.begin_round(id).await.expect("new round").0.number, 2);
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
    assert_eq!(health.state_writer_status, "running");
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
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("delta");
    let plan = plan();
    let route_scores = plan
        .routes
        .iter()
        .map(RouteProposal::score)
        .collect::<Vec<_>>();
    let saved = store
        .save_plan_v2(
            &project.project_id,
            &round,
            &plan,
            &route_scores,
            &delta,
            "distributed worker test",
            None,
        )
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
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "task.offered")
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "task.lease_acquired")
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
    sqlx::query("UPDATE task_leases SET expires_at='1970-01-01T00:00:00+00:00' WHERE lease_id=?")
        .bind(&lease.lease_id)
        .execute(store.pool())
        .await
        .expect("expire first distributed lease");
    let (recovered, recovery_events) = store
        .lease_next_task(
            &project.project_id,
            "node-1",
            "worker-token-123456789",
            node.node_epoch,
            60,
        )
        .await
        .expect("recover expired distributed lease");
    assert!(
        recovery_events
            .iter()
            .any(|event| event.event_type == "task.lease_expired")
    );
    assert_eq!(
        store
            .get_task_lease(&lease.lease_id)
            .await
            .expect("expired first lease")
            .status,
        "expired"
    );
    let expired_state: (Option<String>, String, String) = sqlx::query_as(
        "SELECT l.completed_at,a.status,i.status FROM task_leases l JOIN task_attempts a ON a.attempt_id=l.attempt_id JOIN worker_instances i ON i.worker_instance_id=l.worker_instance_id WHERE l.lease_id=?",
    )
    .bind(&lease.lease_id)
    .fetch_one(store.pool())
    .await
    .expect("expired distributed execution state");
    assert!(expired_state.0.is_some());
    assert_eq!(expired_state.1, "orphaned");
    assert_eq!(expired_state.2, "unhealthy");
    let (lease, task) = recovered.expect("re-leased queued task");
    assert_eq!(task.task_id, *task_id);
    assert_eq!(lease.lease_epoch, 2);
    let project_revision = store
        .get_project(&project.project_id)
        .await
        .expect("project before steer")
        .revision;
    let (steer_command, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "steer_task".into(),
                target_kind: "task".into(),
                target_id: task.task_id.clone(),
                mode: CommandMode::SafePoint,
                payload: json!({
                    "content":"use the remote invariant",
                    "expected_task_revision":task.revision,
                    "expected_route_epoch":task.route_cancellation_epoch,
                }),
                expected_project_revision: project_revision,
                idempotency_key: "distributed-safe-point-steer".into(),
                reason: "test remote steer delivery".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue remote steer");
    store
        .apply_command(&project.project_id, &steer_command.command_id)
        .await
        .expect("queue remote steer");
    let pending_steers = store
        .pending_distributed_task_steers(
            &lease.lease_id,
            "node-1",
            "worker-token-123456789",
            node.node_epoch,
            lease.lease_epoch,
        )
        .await
        .expect("poll remote steer");
    assert_eq!(pending_steers.len(), 1);
    let incorporated_steer_ids = vec![pending_steers[0].steer_id.clone()];
    let output = WorkerOutput {
        summary: "remote completion".into(),
        discoveries: vec![],
        candidates: vec![],
        failures: vec![],
        uncertainties: vec![],
        sources: vec![],
        experiments: vec![],
    };
    assert!(matches!(
        store
            .complete_task_lease(TaskLeaseCompletionRequest {
                lease_id: &lease.lease_id,
                node_id: "node-1",
                token: "worker-token-123456789",
                node_epoch: node.node_epoch,
                lease_epoch: lease.lease_epoch + 1,
                output: &output,
                incorporated_steer_ids: &[],
            })
            .await,
        Err(StorageError::LateSubmission(_))
    ));
    assert!(matches!(
        store
            .complete_task_lease(TaskLeaseCompletionRequest {
                lease_id: &lease.lease_id,
                node_id: "node-1",
                token: "worker-token-123456789",
                node_epoch: node.node_epoch,
                lease_epoch: lease.lease_epoch,
                output: &output,
                incorporated_steer_ids: &[],
            })
            .await,
        Err(StorageError::InvalidTransition(_))
    ));
    store
        .complete_task_lease(TaskLeaseCompletionRequest {
            lease_id: &lease.lease_id,
            node_id: "node-1",
            token: "worker-token-123456789",
            node_epoch: node.node_epoch,
            lease_epoch: lease.lease_epoch,
            output: &output,
            incorporated_steer_ids: &incorporated_steer_ids,
        })
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
    let replay_events = store
        .complete_task_lease(TaskLeaseCompletionRequest {
            lease_id: &lease.lease_id,
            node_id: "node-1",
            token: "worker-token-123456789",
            node_epoch: node.node_epoch,
            lease_epoch: lease.lease_epoch,
            output: &output,
            incorporated_steer_ids: &incorporated_steer_ids,
        })
        .await
        .expect("idempotent completion replay");
    assert!(replay_events.events.is_empty());
    assert_eq!(
        store
            .get_command(&project.project_id, &steer_command.command_id)
            .await
            .expect("remote steer command")
            .status,
        research_domain::CommandStatus::Applied
    );
    let envelope_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM result_envelopes WHERE lease_id=? AND status='ingested'",
    )
    .bind(&lease.lease_id)
    .fetch_one(store.pool())
    .await
    .expect("result envelope count");
    assert_eq!(envelope_count, 1);
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

    sqlx::query("INSERT INTO facts(fact_id,project_id,statement,assumptions_json,proof_markdown,dependency_fact_ids_json,definitions_introduced_json,external_source_ids_json,verification_ids_json,evidence_level,created_by,status,content_hash,created_at) VALUES('target-derived',?,'Target derived','[]','proof','[\"catalog-base\"]','{}','[]','[]','reviewed','test','active','target-derived-hash',?)")
        .bind(&target.project_id)
        .bind(&now)
        .execute(store.pool())
        .await
        .expect("target derived fact");
    sqlx::query("INSERT INTO fact_edges(edge_id,project_id,source_id,target_id,kind) VALUES('target-import-edge',?,'catalog-base','target-derived','depends_on')")
        .bind(&target.project_id)
        .execute(store.pool())
        .await
        .expect("target import edge");
    sqlx::query("INSERT INTO fact_assurances(assurance_id,fact_id,case_id,acceptance_class,snapshot_hash,package_id,replay_id,status,created_at,invalidated_at) VALUES('target-derived-assurance','target-derived',?,'fully_certified','target-derived-snapshot',NULL,NULL,'active',?,NULL)")
        .bind(&case.case_id)
        .bind(&now)
        .execute(store.pool())
        .await
        .expect("target assurance");
    let target_goal_id: String =
        sqlx::query_scalar("SELECT goal_id FROM goals WHERE project_id=? LIMIT 1")
            .bind(&target.project_id)
            .fetch_one(store.pool())
            .await
            .expect("target goal");
    sqlx::query(
        "UPDATE goals SET status='solved',solved_by_fact_id='target-derived' WHERE goal_id=?",
    )
    .bind(&target_goal_id)
    .execute(store.pool())
    .await
    .expect("solve target goal");
    sqlx::query("INSERT INTO routes(route_id,project_id,title,method_summary,target_goal_ids_json,required_fact_ids_json,status,score,priority,cancellation_epoch,created_in_round,attributes_json) VALUES('target-import-route',?,'import dependent','test',?,'[\"target-derived\"]','active',1.0,1.0,0,1,'{}')")
        .bind(&target.project_id)
        .bind(json!([target_goal_id]).to_string())
        .execute(store.pool())
        .await
        .expect("target route");
    sqlx::query("INSERT INTO workers(worker_id,project_id,role,backend,status,current_task_id,current_route_id) VALUES('target-worker',?,'prover','codex','running','target-task-running','target-import-route')")
        .bind(&target.project_id)
        .execute(store.pool())
        .await
        .expect("target worker");

    let live_task_statuses = [
        "open",
        "queued",
        "assigned",
        "offered",
        "leased",
        "running",
        "checkpointed",
        "result_submitted",
        "ingesting",
        "paused",
        "blocked",
    ];
    for status in live_task_statuses {
        let task_id = format!("target-task-{status}");
        let worker_id = (status == "running").then_some("target-worker");
        sqlx::query("INSERT INTO tasks(task_id,project_id,route_id,worker_id,worker_role,goal_ids_json,objective,completion_contract,status,priority,revision,route_cancellation_epoch,round) VALUES(?,?,?,?,?,'[]','test','test',?,1.0,1,0,1)")
            .bind(&task_id)
            .bind(&target.project_id)
            .bind("target-import-route")
            .bind(worker_id)
            .bind("prover")
            .bind(status)
            .execute(store.pool())
            .await
            .expect("live target task");
    }
    let terminal_task_statuses = [
        "completed",
        "rejected",
        "cancelled",
        "dead_lettered",
        "human_stopped",
    ];
    for status in terminal_task_statuses {
        sqlx::query("INSERT INTO tasks(task_id,project_id,route_id,worker_role,goal_ids_json,objective,completion_contract,status,priority,revision,route_cancellation_epoch,round) VALUES(?,?,?,'prover','[]','test','test',?,1.0,1,0,1)")
            .bind(format!("target-terminal-task-{status}"))
            .bind(&target.project_id)
            .bind("target-import-route")
            .bind(status)
            .execute(store.pool())
            .await
            .expect("terminal target task");
    }
    sqlx::query("INSERT INTO worker_nodes(node_id,display_name,capabilities_json,token_hash,status,node_epoch,registered_at,last_heartbeat) VALUES('target-node','target node','{}','target-node-token','active',1,?,?)")
        .bind(&now)
        .bind(&now)
        .execute(store.pool())
        .await
        .expect("target node");
    sqlx::query("INSERT INTO worker_instances(worker_instance_id,project_id,worker_id,backend,status,capability_json,started_at) VALUES('target-worker-instance',?,'target-worker','codex','running','{}',?)")
        .bind(&target.project_id)
        .bind(&now)
        .execute(store.pool())
        .await
        .expect("target worker instance");
    sqlx::query("INSERT INTO task_attempts(attempt_id,project_id,task_id,worker_instance_id,attempt_no,status,lease_epoch,route_cancellation_epoch,created_at) VALUES('target-attempt',?,'target-task-running','target-worker-instance',1,'running',1,0,?)")
        .bind(&target.project_id)
        .bind(&now)
        .execute(store.pool())
        .await
        .expect("target task attempt");
    sqlx::query("INSERT INTO task_leases(lease_id,project_id,task_id,node_id,task_revision,route_epoch,lease_epoch,status,leased_at,expires_at,attempt_id,worker_instance_id,lease_token_hash,offered_at,last_heartbeat_at) VALUES('target-lease',?,'target-task-running','target-node',1,0,1,'active',?,?, 'target-attempt','target-worker-instance','target-lease-token',?,?)")
        .bind(&target.project_id)
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .execute(store.pool())
        .await
        .expect("target task lease");

    for (packet_id, known_fact_ids, content_hash) in [
        (
            "target-root-context",
            "[\"catalog-base\"]",
            "target-root-context-hash",
        ),
        (
            "target-derived-context",
            "[\"target-derived\"]",
            "target-derived-context-hash",
        ),
        (
            "target-safe-context",
            "[\"unrelated-fact\"]",
            "target-safe-context-hash",
        ),
    ] {
        sqlx::query("INSERT INTO context_packets(context_packet_id,project_id,route_id,packet_kind,source_revision,problem_contract_excerpt,objective,known_fact_ids_json,route_progress_json,relevant_failure_pattern_ids_json,relevant_uncertainty_ids_json,source_refs_json,omitted_sections_json,token_estimate,content_json,content_hash,status,created_at) VALUES(?,?,'target-import-route','task',1,'problem','objective',?,'{}','[]','[]','[]','[]',1,'{}',?,'active',?)")
            .bind(packet_id)
            .bind(&target.project_id)
            .bind(known_fact_ids)
            .bind(content_hash)
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("target context packet");
    }
    for (summary_id, input_ids, content_hash) in [
        (
            "target-root-summary",
            "[\"catalog-base\"]",
            "target-root-summary-hash",
        ),
        (
            "target-derived-summary",
            "[\"target-derived\"]",
            "target-derived-summary-hash",
        ),
        (
            "target-safe-summary",
            "[\"unrelated-fact\"]",
            "target-safe-summary-hash",
        ),
    ] {
        sqlx::query("INSERT INTO context_summaries(summary_id,project_id,summary_kind,scope_kind,scope_id,input_entity_ids_json,input_revision,summarizer_version,content,content_hash,omitted_categories_json,status,created_at) VALUES(?,?,'test','project',?,?,1,'test','summary',?,'[]','active',?)")
            .bind(summary_id)
            .bind(&target.project_id)
            .bind(summary_id)
            .bind(input_ids)
            .bind(content_hash)
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("target context summary");
    }

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
            .get_fact("target-derived")
            .await
            .expect("target derived fact")
            .status,
        research_domain::FactStatus::Suspended
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM fact_assurances WHERE assurance_id='target-derived-assurance'",
        )
        .fetch_one(store.pool())
        .await
        .expect("target assurance status"),
        "invalidated"
    );
    assert_eq!(
        store
            .list_goals(&target.project_id)
            .await
            .expect("target goals")[0]
            .status,
        research_domain::GoalStatus::Open
    );
    let target_route = store
        .get_route(&target.project_id, "target-import-route")
        .await
        .expect("target route");
    assert_eq!(target_route.status, research_domain::RouteStatus::Paused);
    assert_eq!(target_route.cancellation_epoch, 1);
    for original_status in live_task_statuses {
        let row = sqlx::query("SELECT status,revision FROM tasks WHERE task_id=?")
            .bind(format!("target-task-{original_status}"))
            .fetch_one(store.pool())
            .await
            .expect("cancelled live task");
        assert_eq!(
            row.try_get::<String, _>("status").expect("task status"),
            "cancelled"
        );
        assert_eq!(row.try_get::<i64, _>("revision").expect("task revision"), 2);
    }
    for original_status in terminal_task_statuses {
        let row = sqlx::query("SELECT status,revision FROM tasks WHERE task_id=?")
            .bind(format!("target-terminal-task-{original_status}"))
            .fetch_one(store.pool())
            .await
            .expect("preserved terminal task");
        assert_eq!(
            row.try_get::<String, _>("status").expect("task status"),
            original_status
        );
        assert_eq!(row.try_get::<i64, _>("revision").expect("task revision"), 1);
    }
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM task_attempts WHERE attempt_id='target-attempt'",
        )
        .fetch_one(store.pool())
        .await
        .expect("attempt status"),
        "cancelled"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM task_leases WHERE lease_id='target-lease'",
        )
        .fetch_one(store.pool())
        .await
        .expect("lease status"),
        "cancelled"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM worker_instances WHERE worker_instance_id='target-worker-instance'",
        )
        .fetch_one(store.pool())
        .await
        .expect("worker instance status"),
        "exited"
    );
    let worker = sqlx::query("SELECT status,current_task_id,current_route_id FROM workers WHERE worker_id='target-worker'")
        .fetch_one(store.pool())
        .await
        .expect("worker state");
    assert_eq!(
        worker
            .try_get::<String, _>("status")
            .expect("worker status"),
        "idle"
    );
    assert!(
        worker
            .try_get::<Option<String>, _>("current_task_id")
            .expect("worker task")
            .is_none()
    );
    assert!(
        worker
            .try_get::<Option<String>, _>("current_route_id")
            .expect("worker route")
            .is_none()
    );
    for packet_id in ["target-root-context", "target-derived-context"] {
        let row = sqlx::query(
            "SELECT status,invalidation_reason FROM context_packets WHERE context_packet_id=?",
        )
        .bind(packet_id)
        .fetch_one(store.pool())
        .await
        .expect("invalidated context packet");
        assert_eq!(
            row.try_get::<String, _>("status").expect("packet status"),
            "invalidated"
        );
        assert!(
            row.try_get::<Option<String>, _>("invalidation_reason")
                .expect("packet reason")
                .is_some()
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM context_packets WHERE context_packet_id='target-safe-context'",
        )
        .fetch_one(store.pool())
        .await
        .expect("safe packet"),
        "active"
    );
    for summary_id in ["target-root-summary", "target-derived-summary"] {
        let row =
            sqlx::query("SELECT status,invalidated_at FROM context_summaries WHERE summary_id=?")
                .bind(summary_id)
                .fetch_one(store.pool())
                .await
                .expect("invalidated context summary");
        assert_eq!(
            row.try_get::<String, _>("status").expect("summary status"),
            "invalidated"
        );
        assert!(
            row.try_get::<Option<String>, _>("invalidated_at")
                .expect("summary time")
                .is_some()
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM context_summaries WHERE summary_id='target-safe-summary'",
        )
        .fetch_one(store.pool())
        .await
        .expect("safe summary"),
        "active"
    );
    assert_eq!(
        store
            .get_project(&target.project_id)
            .await
            .expect("target project")
            .status,
        research_domain::ProjectStatus::NeedsHumanReview
    );
    let target_event = revocation_events
        .iter()
        .find(|event| {
            event.project_id == target.project_id
                && event.event_type == "fact_import.invalidated"
                && event.data["source_fact_id"] == "catalog-base"
        })
        .expect("target base invalidation event");
    let target_revision = store
        .get_project(&target.project_id)
        .await
        .expect("target project revision")
        .revision;
    assert_eq!(target_event.project_revision, target_revision);
    let persisted_event =
        sqlx::query("SELECT cursor,project_revision FROM events WHERE event_id=?")
            .bind(&target_event.event_id)
            .fetch_one(store.pool())
            .await
            .expect("persisted invalidation event");
    assert_eq!(
        persisted_event
            .try_get::<i64, _>("cursor")
            .expect("event cursor"),
        target_event.cursor
    );
    assert_eq!(
        persisted_event
            .try_get::<i64, _>("project_revision")
            .expect("event revision"),
        target_revision
    );
}
