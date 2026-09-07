//! Real router + real scheduler + deterministic offline model stand-in.
use super::*;
use research_worker_runtime::research_v2::{TurnError, TurnOutput, TurnRequest, V2Backend};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

struct ApprovalBackend {
    store: V2Store,
    project: String,
    main_calls: AtomicUsize,
    partner_calls: AtomicUsize,
    release: Notify,
}
#[async_trait::async_trait]
impl V2Backend for ApprovalBackend {
    async fn preflight(&self) -> Result<Value, TurnError> {
        Ok(json!({"adapter":"whiteboard24-offline"}))
    }
    async fn run_turn(
        &self,
        request: TurnRequest,
        _events: mpsc::Sender<Value>,
        cancel: CancellationToken,
    ) -> Result<TurnOutput, TurnError> {
        let state = self.store.read(&self.project).await.unwrap();
        let control = match request.binding.role.as_str() {
            "main" => {
                self.main_calls.fetch_add(1, Ordering::SeqCst);
                if let Some(partner) = rows(&state, "sessions")
                    .iter()
                    .find(|s| s["role"] == "partner")
                {
                    json!({"actions":[{"type":"assign_partner","partner_session_id":partner["id"],"focus":"new independent goal"}],"phase_result":{"next_phase":"waiting","reason":"propose next task"}})
                } else {
                    json!({"actions":[{"type":"request_partner","focus":"original independent goal"}],"phase_result":{"next_phase":"waiting","reason":"wait for partner"}})
                }
            }
            "partner" => {
                let call = self.partner_calls.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    tokio::select! {()=self.release.notified()=>{},()=cancel.cancelled()=>return Err(TurnError::new("CANCELLED","fixture stopped"))}
                }
                json!({"partner_continuation":{"decision":if call==0{"continue"}else{"complete"},"reason":"task boundary"}})
            }
            "memory" => json!({"background_result":{"entries":[]}}),
            "display" => {
                json!({"background_result":{"title":"Fixture","body":"Offline fixture","source_refs":[]}})
            }
            _ => json!({"actions":[]}),
        };
        tokio::fs::write(request.control_path.unwrap(), control.to_string())
            .await
            .unwrap();
        Ok(TurnOutput {
            native_session_id: Some("12345678-1234-1234-1234-123456789abc".into()),
            text: "Offline approval fixture".into(),
            input_tokens: Some(1),
            output_tokens: Some(1),
            ..TurnOutput::default()
        })
    }
}

async fn observed(store: &V2Store, project: &str, predicate: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let s = store.read(project).await.unwrap();
            if predicate(&s) {
                break s;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("offline scheduler reached checkpoint")
}

async fn http(app: &Router, path: &str, input: Value, key: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method(Method::POST)
                .uri(path)
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("content-type", "application/json")
                .header("idempotency-key", key)
                .body(axum::body::Body::from(input.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 2_000_000)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn pending_plan(state: &Value) -> Option<&Value> {
    rows(state, "planning_proposals")
        .iter()
        .rev()
        .find(|p| p["state"] == "pending")
}

#[tokio::test]
async fn http_approval_rejection_and_worker_boundary_gate_real_dispatch() {
    let temp = tempfile::tempdir().unwrap();
    let store = V2Store::connect(&temp.path().join("state.sqlite"), &temp.path().join("data"))
        .await
        .unwrap();
    let project = store
        .create_project(
            json!({"problem":"Prove a theorem","title":"Offline approval"}),
            "create",
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let backend = Arc::new(ApprovalBackend {
        store: store.clone(),
        project: project.clone(),
        main_calls: AtomicUsize::new(0),
        partner_calls: AtomicUsize::new(0),
        release: Notify::new(),
    });
    let service = V2Service::with_backend(store.clone(), V2Config::default(), backend.clone());
    let app = router(service.clone(), TOKEN.into());
    let base = format!("/api/v2/research/projects/{project}");
    let (status,run)=http(&app,&format!("{base}/runs"),json!({"start_authorized":true,"mode":"collaborative","duration_seconds":60,"limits":{"max_partners":5,"max_invocations":20}}),"start").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run}");
    let first = observed(&store, &project, |s| {
        pending_plan(s).is_some()
            && rows(s, "sessions")
                .iter()
                .all(|s| s["invocation_in_flight"] != true)
    })
    .await;
    assert_eq!(backend.partner_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.main_calls.load(Ordering::SeqCst), 1);
    let plan = pending_plan(&first).unwrap();
    let (status,response)=http(&app,&format!("{base}/planning-proposals/{}/decision",plan["id"].as_str().unwrap()),json!({"expected_revision":plan["revision"],"decision":"reject","reason":"Please reconsider the route"}),"reject").await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let second = observed(&store, &project, |s| {
        pending_plan(s).is_some_and(|p| p["id"] != plan["id"])
            && rows(s, "sessions")
                .iter()
                .all(|s| s["invocation_in_flight"] != true)
    })
    .await;
    assert_eq!(backend.partner_calls.load(Ordering::SeqCst), 0);
    let second_plan = pending_plan(&second).unwrap();
    let path = format!(
        "{base}/planning-proposals/{}/decision",
        second_plan["id"].as_str().unwrap()
    );
    let decision = json!({"expected_revision":second_plan["revision"],"decision":"approve"});
    let (status, result) = http(&app, &path, decision.clone(), "approve").await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let (status, replay) = http(&app, &path, decision, "approve").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, result);
    let working = observed(&store, &project, |s| {
        rows(s, "sessions")
            .iter()
            .any(|s| s["role"] == "partner" && s["state"] == "active")
    })
    .await;
    let run_id = working["runs"][0]["id"].clone();
    let (status, response) = http(
        &app,
        &format!("{base}/feedback"),
        json!({"run_id":run_id,"kind":"reframe_goal","body":"Review the partner next task"}),
        "feedback",
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    let proposed = observed(&store, &project, |s| {
        pending_plan(s).is_some_and(|p| p["actions"][0]["type"] == "assign_partner")
    })
    .await;
    assert_eq!(backend.partner_calls.load(Ordering::SeqCst), 1);
    backend.release.notify_one();
    let held = observed(&store, &project, |s| {
        rows(s, "sessions").iter().any(|s| {
            s["role"] == "partner"
                && s["plan_boundary_reached"] == true
                && s["invocation_in_flight"] != true
        })
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(backend.partner_calls.load(Ordering::SeqCst), 1);
    assert!(
        rows(&held, "tasks")
            .iter()
            .all(|t| t["focus"] != "new independent goal")
    );
    let plan = pending_plan(&proposed).unwrap();
    let (status, result) = http(
        &app,
        &format!(
            "{base}/planning-proposals/{}/decision",
            plan["id"].as_str().unwrap()
        ),
        json!({"expected_revision":plan["revision"],"decision":"approve"}),
        "approve-next",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    observed(&store, &project, |s| {
        rows(s, "sessions").iter().any(|s| {
            s["role"] == "partner" && s["focus"] == "new independent goal" && s["state"] == "closed"
        })
    })
    .await;
    assert_eq!(backend.partner_calls.load(Ordering::SeqCst), 2);
    service.shutdown().await.unwrap();
}
