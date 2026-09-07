use super::*;
use crate::research_v2::{V2Config, V2Store, new_session};
use research_worker_runtime::research_v2::{TurnError, TurnOutput, TurnRequest, V2Backend};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

struct HeldBackend {
    calls: AtomicUsize,
    uncertain: bool,
}

#[async_trait::async_trait]
impl V2Backend for HeldBackend {
    async fn preflight(&self) -> Result<Value, TurnError> {
        Ok(json!({"adapter":"pause-fixture"}))
    }
    async fn run_turn(
        &self,
        _request: TurnRequest,
        _events: mpsc::Sender<Value>,
        cancel: CancellationToken,
    ) -> Result<TurnOutput, TurnError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        cancel.cancelled().await;
        Err(TurnError::new(
            if self.uncertain {
                "DELIVERY_UNCERTAIN"
            } else {
                "CANCELLED"
            },
            "controlled fixture cancellation",
        ))
    }
}

async fn fixture(uncertain: bool) -> (tempfile::TempDir, V2Service, String, Arc<HeldBackend>) {
    let tmp = tempfile::tempdir().unwrap();
    let store = V2Store::connect(&tmp.path().join("pause.sqlite"), &tmp.path().join("data"))
        .await
        .unwrap();
    let backend = Arc::new(HeldBackend {
        calls: AtomicUsize::new(0),
        uncertain,
    });
    let service = V2Service::with_backend(store, V2Config::default(), backend.clone());
    let project = service
        .store
        .create_project(json!({"problem":"Prove the fixture goal"}), "create")
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (tmp, service, project, backend)
}

async fn wait_for(service: &V2Service, project: &str, predicate: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let state = service.store.read(project).await.unwrap();
            if predicate(&state) {
                return state;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

async fn discussion(service: &V2Service, project: &str) -> Value {
    service.store.mutate(project,"fixture.discussion",None,|s| {
        push(s,"discussions",json!({"id":"discussion","messages":[],"node_version_ref":null,"selection":null,"revision":1}));Ok(Value::Null)
    }).await.unwrap();
    service.create_interaction(project,json!({"explicit_authorization":true,"discussion_id":"discussion","expected_run_state":"none"}),"interaction").await.unwrap()
}

#[tokio::test]
async fn independent_explanation_is_cancelled_and_project_gate_cannot_be_bypassed() {
    let (_tmp, service, project, backend) = fixture(false).await;
    let interaction = discussion(&service, &project).await;
    service.explain_discussion(&project,"discussion",json!({"text":"Explain the goal","execution_owner":{"kind":"interaction","id":interaction["id"]}}),"explain").await.unwrap();
    wait_for(&service, &project, |s| {
        s["usage"][0]["dispatch_started"] == true && backend.calls.load(Ordering::SeqCst) == 1
    })
    .await;
    service
        .project_control(&project, json!({"type":"pause"}), "pause")
        .await
        .unwrap();
    let paused = wait_for(&service, &project, |s| {
        s["project_control"]["state"] == "paused"
    })
    .await;
    assert_eq!(paused["interactions"][0]["state"], "ended");
    assert_eq!(paused["interactions"][0]["stop_reason"], "project_pause");
    assert_eq!(paused["usage"][0]["state"], "cancelled");
    assert_eq!(
        service
            .start_run(&project, json!({"start_authorized":true}), "start")
            .await
            .unwrap_err()
            .code,
        "PROJECT_PAUSED"
    );
    assert_eq!(service.create_interaction(&project,json!({"explicit_authorization":true,"discussion_id":"discussion","expected_run_state":"none"}),"another").await.unwrap_err().code,"PROJECT_PAUSED");
    assert_eq!(service.explain_discussion(&project,"discussion",json!({"text":"Again","execution_owner":{"kind":"interaction","id":interaction["id"]}}),"another").await.unwrap_err().code,"PROJECT_PAUSED");
    service
        .project_control(&project, json!({"type":"resume"}), "resume")
        .await
        .unwrap();
    assert_eq!(
        service
            .project_control(&project, json!({"type":"pause"}), "pause")
            .await
            .unwrap()["state"],
        "running",
        "replaying an old control cannot reapply it"
    );
    assert_eq!(
        backend.calls.load(Ordering::SeqCst),
        1,
        "resume must not resend a consumed one-shot explanation"
    );
}

#[tokio::test]
async fn queued_explanation_never_dispatches_after_pause() {
    let (_tmp, mut service, project, backend) = fixture(false).await;
    service.lanes = Arc::new(Semaphore::new(0));
    let interaction = discussion(&service, &project).await;
    service.explain_discussion(&project,"discussion",json!({"text":"Explain","execution_owner":{"kind":"interaction","id":interaction["id"]}}),"explain").await.unwrap();
    service
        .project_control(&project, json!({"type":"pause"}), "pause")
        .await
        .unwrap();
    wait_for(&service, &project, |s| {
        s["project_control"]["state"] == "paused"
    })
    .await;
    service.lanes.add_permits(4);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn uncertain_explanation_keeps_pausing_and_blocks_resume_or_restart() {
    let (_tmp, service, project, backend) = fixture(true).await;
    let interaction = discussion(&service, &project).await;
    service.explain_discussion(&project,"discussion",json!({"text":"Explain","execution_owner":{"kind":"interaction","id":interaction["id"]}}),"explain").await.unwrap();
    wait_for(&service, &project, |_| {
        backend.calls.load(Ordering::SeqCst) == 1
    })
    .await;
    service
        .project_control(&project, json!({"type":"pause"}), "pause")
        .await
        .unwrap();
    let state = wait_for(&service, &project, |s| {
        s["project_control"]["outstanding_cancellation"] == true
    })
    .await;
    assert_eq!(state["project_control"]["state"], "pausing");
    assert_eq!(
        service
            .project_control(&project, json!({"type":"resume"}), "resume")
            .await
            .unwrap_err()
            .code,
        "DELIVERY_UNCERTAIN"
    );
    assert_eq!(
        service
            .project_control(&project, json!({"type":"stop"}), "stop")
            .await
            .unwrap()["state"],
        "stopping"
    );
    assert_eq!(
        service
            .start_run(&project, json!({"start_authorized":true}), "start")
            .await
            .unwrap_err()
            .code,
        "PROJECT_STOPPED"
    );
}

#[tokio::test]
async fn pause_suspends_all_cat_roles_without_changing_proof_epochs_or_human_approval() {
    let (_tmp, service, project, _backend) = fixture(false).await;
    // Hold the coordinator so this test exercises state transitions without dispatching any model.
    service
        .runners
        .lock()
        .await
        .insert("run".into(), CancellationToken::new());
    service.store.mutate(&project,"fixture.run",None,|s| {
        let run=json!({"id":"run","state":"waiting_human","control_epoch":7,"problem_version":1,"deadline_at":(chrono::Utc::now()+chrono::Duration::seconds(100)).to_rfc3339(),"limits":{"max_invocations":100},"revision":1});
        push(s,"runs",run.clone());
        push(s,"human_questions",json!({"id":"question","run_id":"run","state":"open","problem_version":1}));
        push(s,"planning_proposals",json!({"id":"approval","run_id":"run","state":"pending"}));
        push(s,"candidates",json!({"id":"proof","run_id":"run","control_epoch":7,"status":"submitted","proof_artifact_id":"frozen"}));
        for role in ["main","partner","reviewer","advisor","memory","display"] {
            let mut session=new_session(s,&run,role,None,&None);session["id"]=json!(role);session["state"]=json!("active");session["invocation_in_flight"]=json!(true);
            push(s,"sessions",session);push(s,"usage",json!({"id":role,"session_id":role,"run_id":"run","state":"running"}));
        }
        push(s,"reviews",json!({"id":"review","reviewer_session_id":"reviewer","state":"running"}));
        push(s,"tasks",json!({"id":"work","owner_session_id":"partner","state":"running"}));
        push(s,"tasks",json!({"id":"manually-paused","owner_session_id":"partner","state":"paused"}));
        push(s,"background_jobs",json!({"id":"job","session_id":"advisor","state":"running"}));
        Ok(Value::Null)
    }).await.unwrap();
    let before = service.store.read(&project).await.unwrap();
    for role in [
        "main", "partner", "reviewer", "advisor", "memory", "display",
    ] {
        service
            .turns
            .lock()
            .await
            .insert(role.into(), CancellationToken::new());
    }
    assert_eq!(
        service
            .project_control(&project, json!({"type":"pause"}), "pause")
            .await
            .unwrap()["state"],
        "pausing"
    );
    assert!(
        service
            .turns
            .lock()
            .await
            .values()
            .all(CancellationToken::is_cancelled)
    );
    service
        .store
        .mutate(&project, "fixture.cancel_confirmed", None, |s| {
            for role in [
                "main", "partner", "reviewer", "advisor", "memory", "display",
            ] {
                assert!(settle_interrupted(s, role)?);
                entity_mut(s, "usage", role)?["state"] = json!("cancelled");
            }
            Ok(Value::Null)
        })
        .await
        .unwrap();
    service.turns.lock().await.clear();
    service.refresh_project_control(&project).await.unwrap();
    assert_eq!(
        service
            .project_control(&project, json!({"type":"pause"}), "pause-again")
            .await
            .unwrap()["state"],
        "paused"
    );
    service
        .project_control(&project, json!({"type":"resume"}), "resume")
        .await
        .unwrap();
    let after = service.store.read(&project).await.unwrap();
    assert_eq!(after["runs"][0]["state"], "waiting_human");
    assert_eq!(
        after["runs"][0]["deadline_at"],
        before["runs"][0]["deadline_at"]
    );
    assert_eq!(after["runs"][0]["control_epoch"], 7);
    assert_eq!(after["candidates"], before["candidates"]);
    assert_eq!(after["planning_proposals"], before["planning_proposals"]);
    assert_eq!(after["reviews"][0]["state"], "queued");
    assert_eq!(after["tasks"][0]["state"], "queued");
    assert_eq!(after["tasks"][1]["state"], "paused");
    assert_eq!(after["background_jobs"][0]["state"], "queued");
}

#[test]
fn pending_and_unknown_are_project_scoped_and_independent_of_budget() {
    let state = json!({"project_control":{"state":"pausing"},"sessions":[{"id":"local","state":"idle"}],"usage":[]});
    assert_eq!(
        refreshed(&state, &["other-project-session".into()])["state"],
        "paused"
    );
    assert_eq!(refreshed(&state, &["local".into()])["state"], "pausing");
    let state = json!({"project_control":{"state":"pausing"},"sessions":[],"usage":[{"id":"unknown","state":"unknown"}]});
    assert_eq!(refreshed(&state, &[])["state"], "pausing");
    assert_eq!(
        refreshed(&state, &[])["unknown_invocation_ids"],
        json!(["unknown"])
    );
}

#[tokio::test]
async fn project_resume_preserves_preexisting_pause_and_expired_deadline() {
    let (_tmp, service, project, _) = fixture(false).await;
    service
        .runners
        .lock()
        .await
        .insert("run".into(), CancellationToken::new());
    service.store.mutate(&project,"fixture.paused_run",None,|s| {
        push(s,"runs",json!({"id":"run","state":"paused","control_epoch":3,"problem_version":1,"deadline_at":(chrono::Utc::now()-chrono::Duration::seconds(1)).to_rfc3339(),"revision":1}));Ok(Value::Null)
    }).await.unwrap();
    service
        .project_control(&project, json!({"type":"pause"}), "pause")
        .await
        .unwrap();
    service
        .project_control(&project, json!({"type":"resume"}), "resume")
        .await
        .unwrap();
    assert_eq!(
        service.store.read(&project).await.unwrap()["runs"][0]["state"],
        "paused",
        "project resume must not revive a separately paused run"
    );
    service
        .store
        .mutate(&project, "fixture.owned_expired_pause", None, |s| {
            s["project_control"]["state"] = json!("paused");
            s["project_control"]["paused_run_ids"] = json!(["run"]);
            Ok(Value::Null)
        })
        .await
        .unwrap();
    service
        .project_control(&project, json!({"type":"resume"}), "resume-expired")
        .await
        .unwrap();
    let state = service.store.read(&project).await.unwrap();
    assert_eq!(state["runs"][0]["state"], "stopping");
    assert_eq!(state["runs"][0]["requested_stop_reason"], "time_limit");
}

#[tokio::test]
async fn stopped_gate_requires_explicit_authorized_new_run() {
    let (_tmp, service, project, _) = fixture(false).await;
    assert_eq!(
        service
            .project_control(&project, json!({"type":"stop"}), "stop")
            .await
            .unwrap()["state"],
        "stopped"
    );
    assert_eq!(
        service
            .project_control(&project, json!({"type":"resume"}), "resume")
            .await
            .unwrap_err()
            .code,
        "INVALID_STATE"
    );
    assert_eq!(
        service
            .start_run(&project, json!({}), "unauthorized")
            .await
            .unwrap_err()
            .code,
        "AUTHORIZATION_REQUIRED"
    );
    service
        .start_run(
            &project,
            json!({"start_authorized":true,"duration_seconds":2,"limits":{"max_invocations":1}}),
            "authorized",
        )
        .await
        .unwrap();
    assert_eq!(
        service.store.read(&project).await.unwrap()["project_control"]["state"],
        "running"
    );
    service
        .project_control(&project, json!({"type":"stop"}), "stop-again")
        .await
        .unwrap();
    wait_for(&service, &project, |s| {
        s["project_control"]["state"] == "stopped"
    })
    .await;
}
