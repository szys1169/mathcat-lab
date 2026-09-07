use research_domain::{
    AssignmentDraft, BoardCapabilities, Budget, CommandMode, HumanRouteCreateRequest,
    PlannerOutput, ProblemContract, ProjectStatus, ReviewMode, RouteProposal, TaskStatus,
};
use research_storage::{BoardInclude, CommandDraft, SqliteStore, StorageError};
use serde_json::json;

async fn store() -> (SqliteStore, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
        .await
        .expect("store");
    (store, temp)
}

fn contract() -> ProblemContract {
    ProblemContract {
        original_problem: "Prove the exact target P.".into(),
        target_statement: "P".into(),
        assumptions: vec!["A".into()],
        success_criteria: "An accepted candidate with active dependencies.".into(),
        version: 1,
    }
}

const fn board_capabilities() -> BoardCapabilities {
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
    }
}

fn human_route(expected_revision: i64, suffix: &str) -> HumanRouteCreateRequest {
    HumanRouteCreateRequest {
        expected_revision,
        title: format!("Human route {suffix}"),
        method_summary: format!("Use the human-selected invariant {suffix}"),
        approach_kind: "direct_proof".into(),
        route_role: "primary".into(),
        plain_language_summary: format!("Test invariant {suffix} directly."),
        steps: vec![
            "State the invariant precisely.".into(),
            "Prove the bridge.".into(),
        ],
        target_goal_ids: vec![],
        required_fact_ids: vec![],
        known_risks: vec!["The bridge may need a narrower lemma.".into()],
        worker_role: format!("human_collaborator_{suffix}"),
        objective: format!("Establish the exact bridge using invariant {suffix}."),
        completion_contract: "A checkable proof, counterexample, or exact named blocker.".into(),
        priority: 0.95,
        reason: "The researcher requested immediate execution from the whiteboard.".into(),
    }
}

fn planner_plan() -> PlannerOutput {
    let route = |title: &str, method: &str, kind: &str| RouteProposal {
        title: title.into(),
        method_summary: method.into(),
        approach_kind: kind.into(),
        route_role: "primary".into(),
        user_title: title.into(),
        plain_language_summary: method.into(),
        why_this_route: "It independently probes the exact target.".into(),
        expected_output: "A checkable candidate or blocker.".into(),
        relation_to_goal: "Directly targets the main goal.".into(),
        steps: vec![
            "Set up the argument.".into(),
            "Check the conclusion.".into(),
        ],
        target_goal_ids: vec![],
        required_fact_ids: vec![],
        expected_subgoals: vec![],
        expected_goal_progress: 0.5,
        uncertainty_reduction: 0.5,
        human_suggestion_alignment: 0.0,
        evidence_support: 0.5,
        route_diversity: 0.5,
        verifiability: 1.0,
        novelty: 0.2,
        failure_similarity_penalty: 0.0,
        cost_penalty: 0.1,
        risks: vec![],
    };
    PlannerOutput {
        rationale_summary: "Keep a proof route and a falsification route.".into(),
        routes: vec![
            route("Planner proof", "Derive P by induction", "direct_proof"),
            route(
                "Planner counterexample",
                "Search minimal boundary cases",
                "counterexample",
            ),
        ],
        assignments: vec![AssignmentDraft {
            route_index: 0,
            worker_role: "prover".into(),
            strategic_role: "central_bridge".into(),
            addresses_interface_debt: true,
            goal_ids: vec![],
            objective: "Prove the exact target.".into(),
            completion_contract: "A candidate or exact blocker.".into(),
            priority: 1.0,
        }],
        targeted_uncertainty_ids: vec![],
        suggestion_decisions: vec![],
    }
}

async fn start_project(store: &SqliteStore, project_id: &str, revision: i64, key: &str) {
    let (command, _) = store
        .enqueue_command(
            project_id,
            CommandDraft {
                command_type: "start_project".into(),
                target_kind: "project".into(),
                target_id: project_id.into(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: revision,
                idempotency_key: key.into(),
                reason: "test".into(),
                requested_by: "researcher".into(),
            },
        )
        .await
        .expect("enqueue start");
    store
        .apply_command(project_id, &command.command_id)
        .await
        .expect("start project");
}

async fn route_review(store: &SqliteStore, route_id: &str) -> String {
    sqlx::query_scalar("SELECT human_review FROM routes WHERE route_id=?")
        .bind(route_id)
        .fetch_one(store.pool())
        .await
        .expect("route review")
}

async fn strict_planned_project(
    store: &SqliteStore,
) -> (research_domain::Project, research_domain::ResearchRound) {
    let (created, _) = store
        .create_project_with_review_mode(
            "strict collaboration".into(),
            contract(),
            Budget::default(),
            ReviewMode::Strict,
        )
        .await
        .expect("strict project");
    start_project(store, &created.project_id, created.revision, "strict-start").await;
    let (round, _) = store.begin_round(&created.project_id).await.expect("round");
    let delta = store
        .collect_research_delta(&created.project_id)
        .await
        .expect("delta");
    let plan = planner_plan();
    store
        .save_plan_v2(
            &created.project_id,
            &round,
            &plan,
            &[0.8, 0.7],
            &delta,
            "strict planner fixture",
            None,
        )
        .await
        .expect("strict plan");
    (
        store
            .get_project(&created.project_id)
            .await
            .expect("planned project"),
        round,
    )
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn human_route_from_created_project_is_atomic_runnable_and_idempotent() {
    let (store, _temp) = store().await;
    let budget = Budget {
        max_minutes_per_task: 9,
        ..Budget::default()
    };
    let (project, _) = store
        .create_project_with_review_mode(
            "direct human route".into(),
            contract(),
            budget,
            ReviewMode::Strict,
        )
        .await
        .expect("project");
    let request = human_route(project.revision, "created");

    let first = store
        .create_human_route(
            &project.project_id,
            &request,
            "researcher",
            "human-route-created-1",
        )
        .await
        .expect("human route");
    assert!(first.data.execution_started);
    assert_eq!(first.data.status, "queued");

    let route = store
        .get_route(&project.project_id, &first.data.route_id)
        .await
        .expect("route");
    assert_eq!(route_review(&store, &route.route_id).await, "approved");
    assert_eq!(route.attributes["source"], "human");
    assert_eq!(route.attributes["mathematical_endorsement"], false);
    let task = store
        .get_task(&project.project_id, &first.data.task_id)
        .await
        .expect("task");
    assert_eq!(task.status, TaskStatus::Queued);
    assert_eq!(task.round, 1);
    assert_eq!(
        task.plan_revision_id.as_deref(),
        Some(first.data.plan_revision_id.as_str())
    );
    assert!(task.context_packet_id.is_some());
    assert!(task.task_signature.is_some());
    let immutable_contract = store
        .get_task_contract(&project.project_id, &task.task_id)
        .await
        .expect("task contract");
    assert_eq!(immutable_contract.budget["max_minutes"], 9);
    assert_eq!(immutable_contract.task_id, task.task_id);
    assert_eq!(immutable_contract.route_id, route.route_id);
    let packet = store
        .get_context_packet_for_task(&task.task_id)
        .await
        .expect("context packet");
    assert_eq!(packet.content["route"]["source"], "human");
    assert!(
        packet.source_refs.iter().any(|reference| {
            reference.kind == "command" && reference.id == first.data.command_id
        })
    );
    let plan_revision = store
        .get_plan_revision(&project.project_id, &first.data.plan_revision_id)
        .await
        .expect("plan revision");
    assert_eq!(plan_revision.status, "committed");
    assert_eq!(plan_revision.planner_mode, "human_direct");
    assert_eq!(
        plan_revision.task_contract_ids,
        vec![immutable_contract.task_contract_id]
    );
    let current = store
        .get_project(&project.project_id)
        .await
        .expect("project after route");
    assert_eq!(current.current_round, 1);
    assert_eq!(current.status, ProjectStatus::Running);
    assert_eq!(current.review_mode, ReviewMode::Strict);
    let rounds = store
        .list_rounds(&project.project_id)
        .await
        .expect("rounds");
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].status.to_string(), "running");
    let released = store
        .list_released_queued_tasks(&project.project_id, 1)
        .await
        .expect("released tasks");
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].task_id, task.task_id);

    let replay = store
        .create_human_route(
            &project.project_id,
            &request,
            "researcher",
            "human-route-created-1",
        )
        .await
        .expect("idempotent replay");
    assert_eq!(replay.data.route_id, first.data.route_id);
    assert_eq!(replay.data.task_id, first.data.task_id);
    assert_eq!(replay.project_revision, first.project_revision);
    assert_eq!(replay.event_cursor, first.event_cursor);
    assert!(replay.events.is_empty());
    let counts: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM routes WHERE project_id=?),\
                (SELECT COUNT(*) FROM tasks WHERE project_id=?),\
                (SELECT COUNT(*) FROM task_contracts WHERE project_id=?),\
                (SELECT COUNT(*) FROM board_write_requests WHERE project_id=?)",
    )
    .bind(&project.project_id)
    .bind(&project.project_id)
    .bind(&project.project_id)
    .bind(&project.project_id)
    .fetch_one(store.pool())
    .await
    .expect("atomic row counts");
    assert_eq!(counts, (1, 1, 1, 1));
}

#[tokio::test]
async fn strict_mode_runs_the_human_route_without_releasing_pending_siblings() {
    let (store, _temp) = store().await;
    let (project, round) = strict_planned_project(&store).await;
    let pending_before = store
        .list_routes(&project.project_id)
        .await
        .expect("routes before human route");
    assert_eq!(pending_before.len(), 2);
    for route in &pending_before {
        assert_eq!(route_review(&store, &route.route_id).await, "pending");
    }

    let created = store
        .create_human_route(
            &project.project_id,
            &human_route(project.revision, "strict"),
            "researcher",
            "human-route-strict-1",
        )
        .await
        .expect("human route under strict policy");
    let routes = store
        .list_routes(&project.project_id)
        .await
        .expect("routes after human route");
    let direct = routes
        .iter()
        .find(|route| route.route_id == created.data.route_id)
        .expect("direct route");
    assert_eq!(route_review(&store, &direct.route_id).await, "approved");
    for route in routes
        .iter()
        .filter(|route| route.route_id != direct.route_id)
    {
        assert_eq!(route_review(&store, &route.route_id).await, "pending");
    }
    assert_eq!(
        store
            .get_project(&project.project_id)
            .await
            .expect("project")
            .status,
        ProjectStatus::NeedsHumanReview
    );
    let released = store
        .list_released_queued_tasks(&project.project_id, round.number)
        .await
        .expect("released tasks");
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].task_id, created.data.task_id);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn goal_review_preserves_history_and_exposes_focus_to_the_next_planner_delta() {
    let (store, _temp) = store().await;
    let (project, round) = strict_planned_project(&store).await;
    let pending_route_ids = store
        .list_routes(&project.project_id)
        .await
        .expect("routes")
        .into_iter()
        .map(|route| route.route_id)
        .collect::<Vec<_>>();
    let old_task_id = store.list_tasks(&project.project_id).await.expect("tasks")[0]
        .task_id
        .clone();
    let artifact = store
        .store_artifact(
            &project.project_id,
            "test",
            "preserved.txt",
            b"keep me",
            round.number,
            vec![],
        )
        .await
        .expect("artifact");
    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project")
        .revision;
    let focus = "重新梳理主目标，并优先讨论交换代数中的局部化与深度方法";
    let (command, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "goal_review".into(),
                target_kind: "project".into(),
                target_id: project.project_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({"focus": focus}),
                expected_project_revision: revision,
                idempotency_key: "goal-review-focus-1".into(),
                reason: "Human requested an immediate whiteboard discussion.".into(),
                requested_by: "researcher".into(),
            },
        )
        .await
        .expect("enqueue goal review");
    let (applied, events) = store
        .apply_command(&project.project_id, &command.command_id)
        .await
        .expect("apply goal review");
    assert_eq!(applied.status.to_string(), "applied");
    assert!(events.iter().any(|event| {
        event.event_type == "goal_review.requested" && event.data["focus"] == focus
    }));
    assert_eq!(
        store
            .get_task(&project.project_id, &old_task_id)
            .await
            .expect("old task")
            .status,
        TaskStatus::Cancelled
    );
    assert_eq!(
        store
            .current_round(&project.project_id)
            .await
            .expect("current round")
            .expect("round")
            .status
            .to_string(),
        "interrupted"
    );
    let restarted = store
        .get_project(&project.project_id)
        .await
        .expect("restarted project");
    assert_eq!(restarted.status, ProjectStatus::Running);
    assert!(
        store
            .has_pending_route_reviews(&project.project_id)
            .await
            .expect("pending reviews")
    );
    for route_id in &pending_route_ids {
        assert_eq!(route_review(&store, route_id).await, "pending");
    }
    assert_eq!(
        store
            .get_artifact(&project.project_id, &artifact.0.artifact_id)
            .await
            .expect("preserved artifact")
            .filename,
        "preserved.txt"
    );
    let delta = store
        .collect_research_delta(&project.project_id)
        .await
        .expect("goal review delta");
    let directive = delta
        .human_commands
        .iter()
        .find(|value| value["command_id"] == command.command_id)
        .expect("auditable goal review directive");
    assert_eq!(directive["type"], "goal_review");
    assert_eq!(directive["payload"]["focus"], focus);
    let next_round = store
        .begin_round(&project.project_id)
        .await
        .expect("pending reviews must not block fresh round")
        .0;
    assert_eq!(next_round.number, round.number + 1);
    assert_eq!(next_round.status.to_string(), "planning");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn combined_settings_are_atomic_release_only_existing_pending_routes_and_preserve_contracts()
{
    let (store, _temp) = store().await;
    let (project, _) = strict_planned_project(&store).await;
    let old_task = store.list_tasks(&project.project_id).await.expect("tasks")[0].clone();
    let old_contract = store
        .get_task_contract(&project.project_id, &old_task.task_id)
        .await
        .expect("old immutable contract");
    assert_eq!(
        old_contract.budget["max_minutes"],
        Budget::default().max_minutes_per_task
    );
    let limits = Budget {
        max_rounds: 20,
        max_parallel_workers: 5,
        max_minutes_per_task: 17,
        max_model_calls_per_task: 6,
        max_total_model_calls: 200,
    };
    let (settings, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "research_settings".into(),
                target_kind: "project".into(),
                target_id: project.project_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({"limits": limits, "review_mode": "automatic"}),
                expected_project_revision: project.revision,
                idempotency_key: "combined-settings-1".into(),
                reason: "Update whiteboard controls atomically.".into(),
                requested_by: "operator".into(),
            },
        )
        .await
        .expect("enqueue settings");
    store
        .apply_command(&project.project_id, &settings.command_id)
        .await
        .expect("apply settings");
    let updated = store
        .get_project(&project.project_id)
        .await
        .expect("updated project");
    assert_eq!(updated.review_mode, ReviewMode::Automatic);
    assert!(!updated.human_route_approval);
    assert_eq!(updated.budget.max_parallel_workers, 5);
    assert_eq!(updated.budget.max_minutes_per_task, 17);
    for route in store
        .list_routes(&project.project_id)
        .await
        .expect("released routes")
    {
        assert_eq!(route_review(&store, &route.route_id).await, "not_required");
    }
    assert_eq!(
        store
            .get_task_contract(&project.project_id, &old_task.task_id)
            .await
            .expect("contract after settings")
            .content_hash,
        old_contract.content_hash,
        "settings must not mutate an already issued task contract"
    );
    let board = store
        .research_board(
            &project.project_id,
            20,
            BoardInclude::default(),
            board_capabilities(),
        )
        .await
        .expect("board settings projection");
    assert_eq!(board.settings.review_mode, ReviewMode::Automatic);
    assert_eq!(board.settings.budget.max_minutes_per_task, 17);
    assert!(
        board
            .settings
            .running_task_policy
            .contains("newly created immutable task contracts")
    );

    let before_invalid = updated;
    let invalid_limits = Budget {
        max_parallel_workers: 17,
        ..before_invalid.budget.clone()
    };
    let (invalid, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "research_settings".into(),
                target_kind: "project".into(),
                target_id: project.project_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({"limits": invalid_limits, "review_mode": "strict"}),
                expected_project_revision: before_invalid.revision,
                idempotency_key: "combined-settings-invalid".into(),
                reason: "Reject the entire invalid settings write.".into(),
                requested_by: "operator".into(),
            },
        )
        .await
        .expect("enqueue invalid settings");
    assert!(matches!(
        store
            .apply_command(&project.project_id, &invalid.command_id)
            .await,
        Err(StorageError::InvalidTransition(message)) if message.contains("max_parallel_workers")
    ));
    let after_invalid = store
        .get_project(&project.project_id)
        .await
        .expect("project after rejected settings");
    assert_eq!(
        after_invalid.revision,
        before_invalid.revision + 1,
        "the durable command enqueue is audited, while its rejected settings transaction is rolled back"
    );
    assert_eq!(after_invalid.review_mode, ReviewMode::Automatic);
    assert_eq!(after_invalid.budget.max_parallel_workers, 5);

    let (strict, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "review_policy".into(),
                target_kind: "project".into(),
                target_id: project.project_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({"review_mode": "strict", "human_route_approval": true}),
                expected_project_revision: after_invalid.revision,
                idempotency_key: "future-strict-only".into(),
                reason: "Require review only for future planner routes.".into(),
                requested_by: "operator".into(),
            },
        )
        .await
        .expect("enqueue strict policy");
    store
        .apply_command(&project.project_id, &strict.command_id)
        .await
        .expect("apply strict policy");
    assert_eq!(
        store
            .get_project(&project.project_id)
            .await
            .expect("strict project")
            .review_mode,
        ReviewMode::Strict
    );
    for route in store
        .list_routes(&project.project_id)
        .await
        .expect("existing routes")
    {
        assert_eq!(route_review(&store, &route.route_id).await, "not_required");
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn suggestion_rejects_a_proposal_or_foreign_identifier_as_target_route() {
    let (store, _temp) = store().await;
    let (project, _) = store
        .create_project(
            "suggestion route validation".into(),
            contract(),
            Budget::default(),
        )
        .await
        .expect("project");
    let (command, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "add_suggestion".into(),
                target_kind: "project".into(),
                target_id: project.project_id.clone(),
                mode: CommandMode::NextRound,
                payload: json!({
                    "content": "Use more commutative algebra.",
                    "target_route_id": "routeproposal_not_a_route"
                }),
                expected_project_revision: project.revision,
                idempotency_key: "invalid-suggestion-route".into(),
                reason: "UI regression fixture".into(),
                requested_by: "researcher".into(),
            },
        )
        .await
        .expect("enqueue suggestion");
    assert!(matches!(
        store
            .apply_command(&project.project_id, &command.command_id)
            .await,
        Err(StorageError::InvalidTransition(message)) if message.contains("not a route in this project")
    ));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM suggestions WHERE project_id=?")
        .bind(&project.project_id)
        .fetch_one(store.pool())
        .await
        .expect("suggestion count");
    assert_eq!(count, 0);

    let current = store
        .get_project(&project.project_id)
        .await
        .expect("project after rejected suggestion");
    let route = store
        .create_human_route(
            &project.project_id,
            &human_route(current.revision, "suggestion"),
            "researcher",
            "suggestion-target-route",
        )
        .await
        .expect("real route");
    let current = store
        .get_project(&project.project_id)
        .await
        .expect("project with route");
    let (valid, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "add_suggestion".into(),
                target_kind: "project".into(),
                target_id: project.project_id.clone(),
                mode: CommandMode::NextRound,
                payload: json!({
                    "content": "Use more commutative algebra.",
                    "target_route_id": route.data.route_id
                }),
                expected_project_revision: current.revision,
                idempotency_key: "valid-suggestion-route".into(),
                reason: "Persist the planning suggestion on the board.".into(),
                requested_by: "researcher".into(),
            },
        )
        .await
        .expect("enqueue valid suggestion");
    store
        .apply_command(&project.project_id, &valid.command_id)
        .await
        .expect("validate suggestion");
    let board = store
        .research_board(
            &project.project_id,
            20,
            BoardInclude::default(),
            board_capabilities(),
        )
        .await
        .expect("board suggestions");
    assert_eq!(board.planning_suggestions.len(), 1);
    assert_eq!(
        board.planning_suggestions[0].target_route_id.as_deref(),
        Some(route.data.route_id.as_str())
    );
    assert_eq!(board.planning_suggestions[0].status, "pending");
    assert_eq!(
        board.planning_suggestions[0].content,
        "Use more commutative algebra."
    );
    let current_round = store
        .get_project(&project.project_id)
        .await
        .expect("project with pending suggestion")
        .current_round;
    assert!(
        store
            .pending_suggestions(&project.project_id, current_round)
            .await
            .expect("suggestions for current round")
            .is_empty(),
        "a next-round suggestion must not leak into the active planning round"
    );
    assert_eq!(
        store
            .pending_suggestions(&project.project_id, current_round + 1)
            .await
            .expect("suggestions for next round")
            .len(),
        1
    );
    sqlx::query("UPDATE suggestions SET status='applied',decision=? WHERE suggestion_id=?")
        .bind(
            json!({
                "suggestion_id":board.planning_suggestions[0].suggestion_id,
                "disposition":"applied",
                "rationale":"The next plan uses the requested algebraic viewpoint."
            })
            .to_string(),
        )
        .bind(&board.planning_suggestions[0].suggestion_id)
        .execute(store.pool())
        .await
        .expect("resolved suggestion fixture");
    let resolved = store
        .research_board(
            &project.project_id,
            20,
            BoardInclude::default(),
            board_capabilities(),
        )
        .await
        .expect("resolved board suggestion");
    assert_eq!(
        resolved.planning_suggestions[0]
            .decision
            .as_ref()
            .and_then(|value| value.get("rationale"))
            .and_then(serde_json::Value::as_str),
        Some("The next plan uses the requested algebraic viewpoint.")
    );
}

#[tokio::test]
async fn lowering_parallelism_changes_future_admission_without_stopping_active_work() {
    let (store, _temp) = store().await;
    let initial_budget = Budget {
        max_parallel_workers: 2,
        ..Budget::default()
    };
    let (project, _) = store
        .create_project_with_review_mode(
            "parallel admission".into(),
            contract(),
            initial_budget.clone(),
            ReviewMode::Automatic,
        )
        .await
        .expect("project");
    let first = store
        .create_human_route(
            &project.project_id,
            &human_route(project.revision, "parallel-one"),
            "researcher",
            "parallel-route-one",
        )
        .await
        .expect("first route");
    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project after first route")
        .revision;
    let second = store
        .create_human_route(
            &project.project_id,
            &human_route(revision, "parallel-two"),
            "researcher",
            "parallel-route-two",
        )
        .await
        .expect("second route");
    let first_task = store
        .get_task(&project.project_id, &first.data.task_id)
        .await
        .expect("first task");
    store
        .offer_local_task(&first_task, "mock", None, "runtime/parallel-one", json!({}))
        .await
        .expect("first offer");
    let revision = store
        .get_project(&project.project_id)
        .await
        .expect("project after offer")
        .revision;
    let lowered = Budget {
        max_parallel_workers: 1,
        ..initial_budget
    };
    let (settings, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "research_settings".into(),
                target_kind: "project".into(),
                target_id: project.project_id.clone(),
                mode: CommandMode::Immediate,
                payload: json!({"limits":lowered,"review_mode":"automatic"}),
                expected_project_revision: revision,
                idempotency_key: "parallel-limit-one".into(),
                reason: "Lower future scheduling concurrency.".into(),
                requested_by: "operator".into(),
            },
        )
        .await
        .expect("enqueue parallel setting");
    store
        .apply_command(&project.project_id, &settings.command_id)
        .await
        .expect("lower parallelism");
    assert_eq!(
        store
            .get_task(&project.project_id, &first.data.task_id)
            .await
            .expect("active task retained")
            .status,
        TaskStatus::Offered
    );
    let second_task = store
        .get_task(&project.project_id, &second.data.task_id)
        .await
        .expect("second task");
    assert!(matches!(
        store
            .offer_local_task(
                &second_task,
                "mock",
                None,
                "runtime/parallel-two",
                json!({})
            )
            .await,
        Err(StorageError::InvalidTransition(message)) if message.contains("active executions") && message.contains("limit 1")
    ));
}

#[tokio::test]
async fn project_budget_bounds_are_enforced_at_creation() {
    let (store, _temp) = store().await;
    for (name, budget) in [
        (
            "zero parallel",
            Budget {
                max_parallel_workers: 0,
                ..Budget::default()
            },
        ),
        (
            "too many parallel",
            Budget {
                max_parallel_workers: 17,
                ..Budget::default()
            },
        ),
        (
            "zero minutes",
            Budget {
                max_minutes_per_task: 0,
                ..Budget::default()
            },
        ),
        (
            "too many minutes",
            Budget {
                max_minutes_per_task: 1_441,
                ..Budget::default()
            },
        ),
        (
            "zero other limit",
            Budget {
                max_model_calls_per_task: 0,
                ..Budget::default()
            },
        ),
    ] {
        assert!(matches!(
            store.create_project(name.into(), contract(), budget).await,
            Err(StorageError::InvalidTransition(_))
        ));
    }
}
