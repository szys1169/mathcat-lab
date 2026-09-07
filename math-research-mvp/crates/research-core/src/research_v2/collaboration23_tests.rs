use super::super::{V2Config, V2Store, new_session};
use super::*;
use tokio_util::sync::CancellationToken;

async fn fixture() -> (tempfile::TempDir, V2Service, String, String) {
    let temp = tempfile::tempdir().unwrap();
    let store = V2Store::connect(&temp.path().join("state.sqlite"), &temp.path().join("data"))
        .await
        .unwrap();
    let project = store
        .create_project(
            json!({"title":"collaboration regression","problem":"Prove P under A"}),
            "create",
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let service = V2Service::new(store, V2Config::default());
    // This test owns the runner slot: absolutely no backend invocation is permitted.
    service
        .runners
        .lock()
        .await
        .insert("run".into(), CancellationToken::new());
    let main=service.store.mutate(&project,"seed",None,|state|{
        let run=json!({"id":"run","state":"running","problem_version":1,"control_epoch":1,"revision":1,"deadline_at":null,"limits":{"max_partners":5,"max_invocations":100}});
        push(state,"runs",run.clone());
        let session=new_session(state,&run,"main",None,&None);push(state,"sessions",session.clone());Ok(session)
    }).await.unwrap()["id"].as_str().unwrap().to_owned();
    (temp, service, project, main)
}
async fn feedback(service: &V2Service, project: &str, key: &str) -> Value {
    service
        .feedback(
            project,
            key,
            &json!({"run_id":"run","body":"Check the parameter dependence","priority":"normal"}),
        )
        .await
        .unwrap()
}
async fn prepare(service: &V2Service, project: &str, main: &str) -> Value {
    service
        .store
        .mutate(project, "prepare", None, |state| {
            let mut capture = json!({"phase":"coordination"});
            prepare_commands(state, "run", main, "invocation-1", &mut capture)?;
            Ok(capture)
        })
        .await
        .unwrap()
}
async fn deliver(service: &V2Service, project: &str, main: &str) -> Value {
    let capture = prepare(service, project, main).await;
    service
        .store
        .mutate(project, "delivery", None, |state| {
            confirm_delivery(
                state,
                &array(&capture, "command_ids")
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
                "invocation-1",
            )?;
            Ok(Value::Null)
        })
        .await
        .unwrap();
    capture
}

#[tokio::test]
async fn ordinary_feedback_preserves_question_wait_and_atomic_message_dedup() {
    let (_temp, service, project, main) = fixture().await;
    let question = service
        .store
        .mutate(&project, "question", None, |state| {
            register_question(
                state,
                "run",
                &main,
                &json!({"question":"Is A assumed?"}),
                &json!({"id":"question-output"}),
            )
        })
        .await
        .unwrap();
    let first = feedback(&service, &project, "same").await;
    let replay = feedback(&service, &project, "same").await;
    assert_eq!(first["id"], replay["id"]);
    let state = service.store.read(&project).await.unwrap();
    assert_eq!(
        entity(&state, "runs", "run").unwrap()["state"],
        "waiting_human"
    );
    assert_eq!(array(&state, "messages").len(), 1);
    assert_eq!(
        entity(&state, "human_questions", question["id"].as_str().unwrap()).unwrap()["state"],
        "open"
    );
    assert!(
        service
            .command(
                &project,
                json!({"type":"resume_run","run_id":"run"}),
                "resume"
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn question_answer_requires_specific_revision_and_cannot_bypass_pause() {
    let (_temp, service, project, main) = fixture().await;
    let question = service
        .store
        .mutate(&project, "question", None, |state| {
            register_question(
                state,
                "run",
                &main,
                &json!({"question":"Which domain?"}),
                &json!({"id":"output"}),
            )
        })
        .await
        .unwrap();
    let qid = question["id"].as_str().unwrap();
    let mut answer = json!({"run_id":"run","expected_revision":99,"body":"All real x"});
    assert_eq!(
        service
            .answer_question23(&project, qid, &answer, "bad")
            .await
            .unwrap_err()
            .code,
        "REVISION_CONFLICT"
    );
    for status in ["pausing", "paused"] {
        service
            .store
            .mutate(&project, "pause", None, |state| {
                entity_mut(state, "runs", "run")?["state"] = json!(status);
                Ok(Value::Null)
            })
            .await
            .unwrap();
    }
    answer["expected_revision"] = json!(1);
    assert!(
        service
            .answer_question23(&project, qid, &answer, "paused-answer")
            .await
            .is_err()
    );
    service
        .command(
            &project,
            json!({"type":"resume_run","run_id":"run"}),
            "resume-pause",
        )
        .await
        .unwrap();
    assert_eq!(
        entity(&service.store.read(&project).await.unwrap(), "runs", "run").unwrap()["state"],
        "waiting_human"
    );
    let command = service
        .answer_question23(&project, qid, &answer, "answer")
        .await
        .unwrap();
    assert_eq!(
        service
            .answer_question23(&project, qid, &answer, "answer")
            .await
            .unwrap()["id"],
        command["id"]
    );
    let state = service.store.read(&project).await.unwrap();
    assert_eq!(entity(&state, "runs", "run").unwrap()["state"], "running");
    assert_eq!(
        entity(&state, "human_questions", qid).unwrap()["state"],
        "answered"
    );
    assert!(
        service
            .answer_question23(&project, qid, &answer, "second-answer")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn withdrawal_before_cutoff_removes_both_queued_records_atomically() {
    let (_temp, service, project, main) = fixture().await;
    let command = feedback(&service, &project, "opinion").await;
    let input = json!({"expected_revision":1});
    let id = command["id"].as_str().unwrap();
    let withdrawn = service
        .change_feedback23(&project, id, "withdraw", &input, "withdraw")
        .await
        .unwrap();
    assert_eq!(withdrawn["status"], "cancelled");
    assert_eq!(
        service
            .change_feedback23(&project, id, "withdraw", &input, "withdraw")
            .await
            .unwrap()["id"],
        command["id"]
    );
    let capture = prepare(&service, &project, &main).await;
    assert!(array(&capture, "command_ids").is_empty());
    let state = service.store.read(&project).await.unwrap();
    assert_eq!(array(&state, "commands").len(), 1);
    assert_eq!(state["messages"][0]["state"], "cancelled");
}

#[tokio::test]
async fn prepared_withdrawal_keeps_delivery_history_and_sends_one_correction() {
    let (_temp, service, project, main) = fixture().await;
    let command = feedback(&service, &project, "opinion").await;
    prepare(&service, &project, &main).await;
    let input = json!({"expected_revision":2});
    let id = command["id"].as_str().unwrap();
    let correction = service
        .change_feedback23(&project, id, "withdraw", &input, "withdraw")
        .await
        .unwrap();
    assert_eq!(correction["replaces_command_id"], command["id"]);
    assert_eq!(
        service
            .change_feedback23(&project, id, "withdraw", &input, "withdraw")
            .await
            .unwrap()["id"],
        correction["id"]
    );
    let state = service.store.read(&project).await.unwrap();
    let old = entity(&state, "commands", id).unwrap();
    assert_eq!(old["status"], "superseded");
    assert_eq!(
        old["delivery_attempts"][0]["certainty"],
        "prepared_not_confirmed"
    );
    assert_eq!(array(&state, "commands").len(), 2);
}

#[tokio::test]
async fn escalation_reuses_unprepared_command_but_replaces_prepared_command() {
    let (_temp, service, project, main) = fixture().await;
    let command = feedback(&service, &project, "opinion").await;
    let id = command["id"].as_str().unwrap();
    let urgent = service
        .change_feedback23(
            &project,
            id,
            "escalate",
            &json!({"expected_revision":1}),
            "urgent",
        )
        .await
        .unwrap();
    assert_eq!(urgent["id"], command["id"]);
    let state = service.store.read(&project).await.unwrap();
    assert_eq!(array(&state, "messages").len(), 1);
    assert_eq!(state["messages"][0]["priority"], "urgent");
    prepare(&service, &project, &main).await;
    let replacement = service
        .change_feedback23(
            &project,
            id,
            "escalate",
            &json!({"expected_revision":3}),
            "urgent-after",
        )
        .await
        .unwrap();
    assert_ne!(replacement["id"], command["id"]);
    assert_eq!(replacement["priority"], "urgent");
    assert_eq!(replacement["replaces_command_id"], command["id"]);
}

#[tokio::test]
async fn cutoff_and_urgent_intake_do_not_claim_later_normal_opinions() {
    let (_temp, service, project, main) = fixture().await;
    let first = feedback(&service, &project, "first").await;
    let capture = prepare(&service, &project, &main).await;
    let second = feedback(&service, &project, "later").await;
    assert_eq!(capture["command_ids"], json!([first["id"]]));
    assert!(!array(&capture, "command_ids").contains(&second["id"]));
    let urgent = service
        .feedback(
            &project,
            "urgent",
            &json!({"run_id":"run","body":"urgent correction","priority":"urgent"}),
        )
        .await
        .unwrap();
    let next = service
        .store
        .mutate(&project, "prepare-urgent", None, |state| {
            let mut capture = json!({"phase":"urgent_attention"});
            prepare_commands(state, "run", &main, "urgent-invocation", &mut capture)?;
            Ok(capture)
        })
        .await
        .unwrap();
    assert_eq!(next["command_ids"], json!([urgent["id"]]));
}

#[tokio::test]
async fn response_requires_confirmed_delivery_and_execution_links_need_successful_host_actions() {
    let (temp, service, project, main) = fixture().await;
    let command = feedback(&service, &project, "opinion").await;
    let cid = command["id"].as_str().unwrap();
    let response = json!({"type":"command_response","command_id":cid,"disposition":"partially_adopted","response":"Check assumptions with a partner"});
    let mut state = service.store.read(&project).await.unwrap();
    assert!(
        command_response(&mut state, "run", &main, &response, &json!({"id":"output"})).is_err()
    );
    deliver(&service, &project, &main).await;
    let artifact = service
        .store
        .put_artifact(&project, "out.md", b"host output", "text/markdown")
        .await
        .unwrap();
    service.apply_actions(&project,"run",&main,1,&json!({"actions":[response,{"type":"request_partner","focus":"Check exact assumptions","source_command_ids":[cid]}]}),&artifact,temp.path()).await.unwrap();
    let trace = service
        .feedback_traces23(&project, Some("run"))
        .await
        .unwrap()
        .remove(0);
    assert_eq!(trace["disposition"], "partially_adopted");
    assert!(
        array(&trace, "result_refs")
            .iter()
            .any(|r| r["collection"] == "tasks")
    );
    let artifact2 = service
        .store
        .put_artifact(&project, "bad.md", b"rejected host action", "text/markdown")
        .await
        .unwrap();
    service
        .apply_actions(
            &project,
            "run",
            &main,
            1,
            &json!({"actions":[{"type":"request_partner","focus":"","source_command_ids":[cid]}]}),
            &artifact2,
            temp.path(),
        )
        .await
        .unwrap_err();
    let trace = service
        .feedback_traces23(&project, Some("run"))
        .await
        .unwrap()
        .remove(0);
    let rejected = array(&trace, "action_receipts")
        .iter()
        .find(|r| r["status"] == "rejected")
        .unwrap();
    assert!(array(rejected, "result_refs").is_empty());
}

#[test]
fn received_and_unattributed_actions_never_imply_adoption() {
    let state = json!({"commands":[{"id":"c","run_id":"r","type":"suggest_idea","status":"delivered","payload":{"text":"idea"}}],"messages":[{"id":"m","command_id":"c","state":"handled","receipt":{"disposition":"received"}}],"action_receipts":[{"id":"a","status":"applied","result_refs":[{"collection":"tasks","id":"t"}]}]});
    let trace = traces(&state, None).remove(0);
    assert!(trace["disposition"].is_null());
    assert!(array(&trace, "result_refs").is_empty());
}

#[test]
fn result_receipts_exclude_unchanged_goal_and_author_but_include_new_edge() {
    let before =
        json!({"nodes":[{"id":"root","revision":1}],"sessions":[{"id":"main","revision":1}]});
    let mut state = before.clone();
    state["candidates"] = json!([{"id":"candidate","revision":1}]);
    state["edges"] = json!([{"id":"edge","revision":1}]);
    let refs = result_refs(
        &before,
        &state,
        &json!([{"id":"candidate","author_session_id":"main","goal_refs":[{"ref":{"id":"root"}}]},{"id":"edge","from":{"id":"root"},"to":{"id":"candidate"}}]),
    );
    assert_eq!(refs.len(), 2);
    assert!(refs.iter().any(|r| r["collection"] == "edges"));
    assert!(!refs.iter().any(|r| r["id"] == "root" || r["id"] == "main"));
}

#[tokio::test]
async fn uncertain_delivery_keeps_unknown_attempt_and_confirmation_never_regresses() {
    let (_temp, service, project, main) = fixture().await;
    let command = feedback(&service, &project, "opinion").await;
    prepare(&service, &project, &main).await;
    service
        .store
        .mutate(&project, "unknown", None, |state| {
            failed_delivery(state, &main, "DELIVERY_UNCERTAIN")?;
            Ok(Value::Null)
        })
        .await
        .unwrap();
    let state = service.store.read(&project).await.unwrap();
    let c = entity(&state, "commands", command["id"].as_str().unwrap()).unwrap();
    assert_eq!(c["delivery_state"], "unknown");
    assert_eq!(c["delivery_attempts"][0]["certainty"], "unknown");
    assert_eq!(c["status"], "prepared");
    service
        .store
        .mutate(&project, "late-confirmation", None, |state| {
            confirm_delivery(
                state,
                &[command["id"].as_str().unwrap().to_owned()],
                "invocation-1",
            )?;
            failed_delivery(state, &main, "CANCELLED")?;
            Ok(Value::Null)
        })
        .await
        .unwrap();
    let state = service.store.read(&project).await.unwrap();
    let c = entity(&state, "commands", command["id"].as_str().unwrap()).unwrap();
    assert_eq!(c["delivery_state"], "confirmed");
    assert_eq!(c["delivery_attempts"][0]["certainty"], "confirmed");
}

struct NoDispatchBackend(std::sync::atomic::AtomicUsize);
#[async_trait::async_trait]
impl super::super::V2Backend for NoDispatchBackend {
    async fn preflight(&self) -> Result<Value, research_worker_runtime::research_v2::TurnError> {
        Ok(json!({"fixture":true}))
    }
    async fn run_turn(
        &self,
        _request: super::super::TurnRequest,
        _events: tokio::sync::mpsc::Sender<Value>,
        _cancel: CancellationToken,
    ) -> Result<
        research_worker_runtime::research_v2::TurnOutput,
        research_worker_runtime::research_v2::TurnError,
    > {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(research_worker_runtime::research_v2::TurnError::new(
            "UNEXPECTED_DISPATCH",
            "missing frozen proof must fail before any backend invocation",
        ))
    }
}

#[tokio::test]
async fn verification_only_missing_frozen_proof_ends_without_dispatch_or_empty_loop() {
    let (_temp, initial, project, main) = fixture().await;
    let backend = std::sync::Arc::new(NoDispatchBackend(std::sync::atomic::AtomicUsize::new(0)));
    let service =
        V2Service::with_backend(initial.store.clone(), V2Config::default(), backend.clone());
    service.store.mutate(&project,"verification-fixture",None,|state|{
        let run=entity_mut(state,"runs","run")?;run["verification_only"]=json!(true);
        entity_mut(state,"sessions",&main)?["state"]=json!("closed");
        let candidate=json!({"id":"broken-candidate","run_id":"run","author_session_id":main,"problem_version":1,"control_epoch":1,"claim":"P","exact_statement":"P","proof_artifact_id":"missing-frozen-proof","snapshot_hash":"snapshot","status":"submitted","covers_goal":true,"dependency_ids":[],"source_artifact_ids":[],"declared_premises":[],"revision":1});
        push(state,"candidates",candidate.clone());super::super::queue_review(state,&candidate,"candidate_submission")?;Ok(Value::Null)
    }).await.unwrap();
    service.launch(&project, "run").await;
    let state = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let state = service.store.read(&project).await.unwrap();
            if state["runs"][0]["state"] == "ended" {
                break state;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(backend.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(state["runs"][0]["stop_reason"], "environment_error");
    assert_eq!(state["reviews"][0]["state"], "failed");
    assert!(
        array(&state, "usage")
            .iter()
            .all(|usage| usage["state"] == "not_dispatched")
    );
    assert_eq!(array(&state, "usage").len(), 1);
    service.shutdown().await.unwrap();
}
