use research_domain::{
    BoardCapabilities, Budget, CommandMode, ProblemContract, ProblemRevisionRequest,
};
use research_storage::{BoardInclude, CommandDraft, SqliteStore};
use serde_json::json;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn problem_revision_atomically_invalidates_stale_tasks_and_preserves_original_problem() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path())
        .await
        .expect("store");
    let (project, _) = store
        .create_project(
            "board revision".into(),
            ProblemContract {
                original_problem: "original immutable problem".into(),
                target_statement: "prove P under A".into(),
                assumptions: vec!["A".into()],
                success_criteria: "accepted".into(),
                version: 1,
            },
            Budget::default(),
        )
        .await
        .expect("project");
    let goal_id = store.list_goals(&project.project_id).await.expect("goals")[0]
        .goal_id
        .clone();
    sqlx::query("INSERT INTO routes(route_id,project_id,title,method_summary,target_goal_ids_json,required_fact_ids_json,status,score,priority,cancellation_epoch,created_in_round,attributes_json) VALUES('board-route',?,'route','method',?,'[]','active',1.0,1.0,0,1,'{}')")
        .bind(&project.project_id)
        .bind(json!([goal_id]).to_string())
        .execute(store.pool())
        .await
        .expect("route");
    sqlx::query("INSERT INTO workers(worker_id,project_id,role,backend,status,current_task_id,current_route_id) VALUES('board-worker',?,'prover','mock','running','board-task','board-route')")
        .bind(&project.project_id)
        .execute(store.pool())
        .await
        .expect("worker");
    sqlx::query("INSERT INTO tasks(task_id,project_id,route_id,worker_id,worker_role,goal_ids_json,objective,completion_contract,status,priority,revision,route_cancellation_epoch,round) VALUES('board-task',?,'board-route','board-worker','prover',?,'prove','candidate or gap','running',1.0,1,0,1)")
        .bind(&project.project_id)
        .bind(json!([goal_id]).to_string())
        .execute(store.pool())
        .await
        .expect("task");
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("INSERT INTO facts(fact_id,project_id,statement,assumptions_json,proof_markdown,dependency_fact_ids_json,definitions_introduced_json,external_source_ids_json,verification_ids_json,evidence_level,created_by,status,content_hash,created_at) VALUES('board-fact',?,'lemma under A','[\"A\"]','proof','[]','{}','[]','[]','independent_llm_check','test','active','board-fact-hash',?)")
        .bind(&project.project_id)
        .bind(&now)
        .execute(store.pool())
        .await
        .expect("fact");

    let request = ProblemRevisionRequest {
        expected_revision: project.revision,
        target_statement: "prove P under B".into(),
        assumptions: vec!["B".into()],
        success_criteria: "accepted with active closure".into(),
        change_reason: "replace invalid assumption".into(),
        replan: true,
    };
    let mutation = store
        .revise_problem_contract(
            &project.project_id,
            &request,
            "researcher",
            "problem-revision-test",
        )
        .await
        .expect("revision");
    assert_eq!(mutation.data.contract_version, 2);
    assert!(
        mutation
            .events
            .iter()
            .any(|event| event.event_type == "problem_contract.revised")
    );
    let revised = store
        .get_project(&project.project_id)
        .await
        .expect("project");
    assert_eq!(
        revised.contract.original_problem,
        "original immutable problem"
    );
    assert_eq!(revised.contract.target_statement, "prove P under B");
    let task = store
        .get_task(&project.project_id, "board-task")
        .await
        .expect("task");
    assert_eq!(task.status.to_string(), "cancelled");
    assert_eq!(task.revision, 2);
    let route = store
        .get_route(&project.project_id, "board-route")
        .await
        .expect("route");
    assert_eq!(route.cancellation_epoch, 1);
    let fact = store.get_fact("board-fact").await.expect("fact");
    assert_eq!(fact.status.to_string(), "suspended");
    let board = store
        .research_board(
            &project.project_id,
            100,
            BoardInclude {
                tasks: true,
                ..BoardInclude::default()
            },
            BoardCapabilities {
                can_edit_problem: true,
                can_propose_route: true,
                can_approve_route: true,
                can_control_project: true,
                can_control_tasks: true,
                can_govern_facts: true,
            },
        )
        .await
        .expect("board");
    assert_eq!(board.revision, revised.revision);
    assert_eq!(board.problem.version, 2);
    assert!(!board.capabilities.can_approve_route);
}

#[tokio::test]
async fn optional_route_approval_changes_budget_eligibility_without_endorsing_mathematics() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path())
        .await
        .expect("store");
    let (project, _) = store
        .create_project_with_route_approval(
            "approval".into(),
            ProblemContract {
                original_problem: "P".into(),
                target_statement: "P".into(),
                assumptions: vec![],
                success_criteria: "accepted".into(),
                version: 1,
            },
            Budget::default(),
            true,
        )
        .await
        .expect("project");
    sqlx::query("INSERT INTO routes(route_id,project_id,title,method_summary,target_goal_ids_json,required_fact_ids_json,status,score,priority,cancellation_epoch,created_in_round,attributes_json,human_review) VALUES('approval-route',?,'route','method','[]','[]','active',1.0,1.0,0,0,'{}','pending')")
        .bind(&project.project_id)
        .execute(store.pool())
        .await
        .expect("route");
    let (command, _) = store
        .enqueue_command(
            &project.project_id,
            CommandDraft {
                command_type: "approve_route".into(),
                target_kind: "route".into(),
                target_id: "approval-route".into(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: project.revision,
                idempotency_key: "approve-route-once".into(),
                reason: "allocate research budget".into(),
                requested_by: "researcher".into(),
            },
        )
        .await
        .expect("enqueue");
    let (_, events) = store
        .apply_command(&project.project_id, &command.command_id)
        .await
        .expect("approve");
    let human_review: String =
        sqlx::query_scalar("SELECT human_review FROM routes WHERE route_id='approval-route'")
            .fetch_one(store.pool())
            .await
            .expect("review state");
    assert_eq!(human_review, "approved");
    let event = events
        .iter()
        .find(|event| event.event_type == "route.approved")
        .expect("approval event");
    assert_eq!(
        event
            .data
            .get("mathematical_endorsement")
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );
}

#[tokio::test]
async fn problem_revision_without_replan_pauses_a_running_project() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path())
        .await
        .expect("store");
    let (project, _) = store
        .create_project(
            "pause on revision".into(),
            ProblemContract {
                original_problem: "P".into(),
                target_statement: "P".into(),
                assumptions: vec![],
                success_criteria: "accepted".into(),
                version: 1,
            },
            Budget::default(),
        )
        .await
        .expect("project");
    sqlx::query("UPDATE projects SET status='running' WHERE project_id=?")
        .bind(&project.project_id)
        .execute(store.pool())
        .await
        .expect("running project");

    let mutation = store
        .revise_problem_contract(
            &project.project_id,
            &ProblemRevisionRequest {
                expected_revision: project.revision,
                target_statement: "Q".into(),
                assumptions: vec![],
                success_criteria: "accepted".into(),
                change_reason: "correct the target".into(),
                replan: false,
            },
            "researcher",
            "pause-on-revision",
        )
        .await
        .expect("revision");

    let revised = store
        .get_project(&project.project_id)
        .await
        .expect("project");
    assert_eq!(revised.status.to_string(), "paused");
    assert!(
        mutation
            .events
            .iter()
            .any(|event| event.event_type == "project.paused")
    );
}
