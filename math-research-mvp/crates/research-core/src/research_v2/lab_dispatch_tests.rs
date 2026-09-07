use super::{completed_task_has_pending_assignment, notify_partner_review_at};
use crate::research_v2::{V2Config, V2Service, V2Store, array, entity, loop_control};
use research_worker_runtime::research_v2::{TurnError, TurnOutput, TurnRequest, V2Backend};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

struct HandoffBackend {
    store: V2Store,
    project: String,
    main_calls: AtomicUsize,
    partner_calls: AtomicUsize,
    partner_id: Mutex<Option<String>>,
    second_assignment: Mutex<Option<Value>>,
}

fn checkpoint() -> Value {
    json!({"proof_goal":"finish one independent task","local_assumptions":[],"symbols":[],"unfinished_steps":[],"next_step":"await another task"})
}

#[async_trait::async_trait]
impl V2Backend for HandoffBackend {
    async fn preflight(&self) -> Result<Value, TurnError> {
        Ok(json!({"adapter":"handoff-mock"}))
    }

    async fn run_turn(
        &self,
        request: TurnRequest,
        _events: mpsc::Sender<Value>,
        cancel: CancellationToken,
    ) -> Result<TurnOutput, TurnError> {
        let control = match request.binding.role.as_str() {
            "main" => {
                match self.main_calls.fetch_add(1, Ordering::SeqCst) {
                    0 => {
                        json!({"actions":[{"type":"request_partner","focus":"first independent task"}],"phase_result":{"next_phase":"waiting","reason":"wait for partner result"}})
                    }
                    1 => {
                        let partner_id = self.partner_id.lock().await.clone().unwrap();
                        // The notification can arrive before postprocessing ends. Deliberately
                        // wait for actual completion to reproduce the scheduler's old barrier.
                        tokio::time::timeout(Duration::from_secs(5), async {
                            loop {
                                let state = self.store.read(&self.project).await.unwrap();
                                let partner = entity(&state,"sessions",&partner_id).unwrap();
                                if partner["state"]=="closed" && partner["invocation_in_flight"]!=true {
                                    assert_eq!(entity(&state,"tasks",partner["task_id"].as_str().unwrap()).unwrap()["state"],"completed");
                                    break;
                                }
                                tokio::select! {()=cancel.cancelled()=>break,()=tokio::time::sleep(Duration::from_millis(10))=>{}}
                            }
                        }).await.expect("partner completed its first task");
                        json!({"actions":[{"type":"assign_partner","partner_session_id":partner_id,"focus":"second independent task"}],"phase_result":{"next_phase":"waiting","reason":"wait for reassigned partner"}})
                    }
                    _ => {
                        json!({"phase_result":{"next_phase":"waiting","reason":"fixture finished"}})
                    }
                }
            }
            "partner" => {
                *self.partner_id.lock().await = Some(request.binding.session_id.clone());
                let call = self.partner_calls.fetch_add(1, Ordering::SeqCst);
                if call == 1 {
                    let state = self.store.read(&self.project).await.unwrap();
                    let partner = entity(&state, "sessions", &request.binding.session_id).unwrap();
                    *self.second_assignment.lock().await = Some(partner.clone());
                    assert_eq!(partner["assignment_epoch"], 2);
                    assert_eq!(partner["focus"], "second independent task");
                    assert!(request.prompt.contains("second independent task"));
                }
                json!({"checkpoint":checkpoint(),"partner_continuation":{"decision":"complete","reason":"task finished"}})
            }
            "memory" => json!({"background_result":{"entries":[]}}),
            "display" => {
                json!({"background_result":{"title":"Fixture","body":"Fixture progress","source_refs":[]}})
            }
            _ => panic!("unexpected mock role"),
        };
        tokio::fs::write(request.control_path.as_ref().unwrap(), control.to_string())
            .await
            .unwrap();
        Ok(TurnOutput {
            native_session_id: Some("12345678-1234-1234-1234-123456789abc".into()),
            text: "Mock research task finished".into(),
            input_tokens: Some(1),
            output_tokens: Some(1),
            ..TurnOutput::default()
        })
    }
}

#[tokio::test]
async fn completed_partner_is_reassigned_and_dispatched_by_real_scheduler() {
    let temp = tempfile::tempdir().unwrap();
    let store = V2Store::connect(
        &temp.path().join("handoff.sqlite"),
        &temp.path().join("data"),
    )
    .await
    .unwrap();
    let project=store.create_project(json!({"title":"handoff regression","problem":"independently investigate two simple lemmas"}),"create").await.unwrap()["id"].as_str().unwrap().to_owned();
    let backend = Arc::new(HandoffBackend {
        store: store.clone(),
        project: project.clone(),
        main_calls: AtomicUsize::new(0),
        partner_calls: AtomicUsize::new(0),
        partner_id: Mutex::new(None),
        second_assignment: Mutex::new(None),
    });
    let service = V2Service::with_backend(store, V2Config::default(), backend.clone());
    service
        .start_run(
            &project,
            json!({"start_authorized":true,"mode":"delegated","duration_seconds":60,"limits":{"max_invocations":20}}),
            "start",
        )
        .await
        .unwrap();
    let observed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = service.store.read(&project).await.unwrap();
            if backend.partner_calls.load(Ordering::SeqCst) >= 2
                && array(&state, "sessions").iter().any(|session| {
                    session["role"] == "partner"
                        && session["assignment_epoch"] == 2
                        && session["state"] == "closed"
                        && session["invocation_in_flight"] != true
                })
            {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    service.shutdown().await.unwrap();
    let state = observed.expect("completed partner must dispatch its newly assigned task");
    assert_eq!(backend.partner_calls.load(Ordering::SeqCst), 2);
    let assignment = backend.second_assignment.lock().await.clone().unwrap();
    let old_task = array(&state, "tasks")
        .iter()
        .find(|task| task["focus"] == "first independent task")
        .unwrap();
    assert_eq!(old_task["state"], "superseded");
    assert_ne!(old_task["id"], assignment["task_id"]);
    assert_eq!(state["pending_assignments"][0]["state"], "applied");
    assert!(
        array(&state, "messages")
            .iter()
            .any(|message| message["kind"] == "partner_output_ready"
                && message["state"] == "handled")
    );
}

fn handoff_state() -> Value {
    json!({"sessions":[{"id":"partner","run_id":"run","role":"partner","state":"idle","task_id":"task","pending_assignment_id":"pending"}],"tasks":[{"id":"task","state":"completed"}],"routes":[{"id":"route","run_id":"run","status":"active","parent_route_ids":[]}],"pending_assignments":[{"id":"pending","run_id":"run","partner_session_id":"partner","state":"pending","route_id":"route"}]})
}

#[test]
fn pending_dispatch_does_not_bypass_stops_pauses_routes_or_inflight_work() {
    let mut state = handoff_state();
    assert!(completed_task_has_pending_assignment(
        &state,
        &state["sessions"][0]
    ));
    for task_state in ["paused", "cancelled", "superseded", "running"] {
        state["tasks"][0]["state"] = json!(task_state);
        assert!(!completed_task_has_pending_assignment(
            &state,
            &state["sessions"][0]
        ));
    }
    state["tasks"][0]["state"] = json!("completed");
    state["sessions"][0]["invocation_in_flight"] = json!(true);
    assert!(!completed_task_has_pending_assignment(
        &state,
        &state["sessions"][0]
    ));
    state["sessions"][0]["invocation_in_flight"] = json!(false);
    state["routes"][0]["status"] = json!("blocked");
    assert!(!completed_task_has_pending_assignment(
        &state,
        &state["sessions"][0]
    ));
    state["routes"][0]["status"] = json!("active");
    state["pending_assignments"][0]["state"] = json!("withdrawn");
    assert!(!completed_task_has_pending_assignment(
        &state,
        &state["sessions"][0]
    ));
}

#[test]
fn closed_partner_review_notifies_waiting_main_without_reopening_partner() {
    for verdict in ["accepted", "rejected", "changes_requested", "inconclusive"] {
        let mut state = json!({"runs":[{"id":"run","state":"running","limits":{"max_invocations":100}}],"sessions":[{"id":"main","run_id":"run","role":"main","state":"waiting","phase":"waiting"},{"id":"partner","run_id":"run","role":"partner","state":"closed"}],"candidates":[{"id":"candidate","run_id":"run","author_session_id":"partner","problem_version":1}],"reviews":[{"id":"review","candidate_id":"candidate","run_id":"run","state":"completed","verdict":verdict,"revision":2,"artifact_id":"review-artifact"}]});
        super::tick_at(&mut state, "run", &None, "2026-09-07T00:00:00+00:00").unwrap();
        notify_partner_review_at(&mut state, "review", "2026-09-07T00:00:01+00:00");
        notify_partner_review_at(&mut state, "review", "2026-09-07T00:00:01+00:00");
        assert_eq!(array(&state, "messages").len(), 1);
        assert_eq!(state["messages"][0]["priority"], "normal");
        assert_eq!(
            state["messages"][0]["evidence_refs"],
            json!(["review", "candidate", "review-artifact"])
        );
        super::tick_at(&mut state, "run", &None, "2026-09-07T00:00:02+00:00").unwrap();
        assert_eq!(state["sessions"][0]["state"], "idle");
        assert_eq!(state["sessions"][0]["phase"], "coordination");
        assert_eq!(state["sessions"][1]["state"], "closed");
        state["candidates"][0]["author_session_id"] = json!("main");
        state["reviews"][0]["revision"] = json!(3);
        notify_partner_review_at(&mut state, "review", "2026-09-07T00:00:03+00:00");
        assert_eq!(
            array(&state, "messages").len(),
            1,
            "main's own review retains its original gate/wake path"
        );
    }
}

#[test]
fn failed_partner_review_also_notifies_main_without_reopening_partner() {
    let mut state = json!({"sessions":[{"id":"partner","role":"partner","state":"closed"}],"candidates":[{"id":"candidate","run_id":"run","author_session_id":"partner"}],"reviews":[{"id":"review","run_id":"run","candidate_id":"candidate","reviewer_session_id":"reviewer","state":"running","revision":1}]});
    loop_control::fail_review(&mut state, "reviewer", "PROVIDER_ERROR");
    assert_eq!(state["sessions"][0]["state"], "closed");
    assert_eq!(state["messages"][0]["kind"], "partner_review_ready");
    assert_eq!(
        state["messages"][0]["body"]["failure_code"],
        "PROVIDER_ERROR"
    );
    assert_eq!(state["messages"][0]["trust"], "review_status_only");
}
