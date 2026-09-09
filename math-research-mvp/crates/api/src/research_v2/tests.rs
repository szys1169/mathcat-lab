//! Offline HTTP contract tests: the service is never given a real model request.
#![allow(clippy::unwrap_used)]
use super::*;
use axum::http::Method;
use research_core::research_v2::V2Config;
use research_storage::research_v2::V2Store;
use tower::ServiceExt;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

#[path = "lab22_mock.rs"]
mod lab22_mock;
#[path = "whiteboard24_tests.rs"]
mod whiteboard24_tests;

#[tokio::test]
async fn model_selection_api_is_idempotent_revisioned_and_available_while_paused() {
    let f = Fixture::new().await;
    f.request(
        Method::POST,
        &f.path("/project-control"),
        Some(json!({"type":"pause"})),
        Some("pause-model"),
    )
    .await;
    let choice = json!({"model":"gpt-6-astra","reasoning_effort":"ultra","expected_revision":0});
    let (status, result) = f
        .request(
            Method::POST,
            &f.path("/model-selection"),
            Some(choice.clone()),
            Some("model-1"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["model_selection"]["revision"], 1);
    let (_, replay) = f
        .request(
            Method::POST,
            &f.path("/model-selection"),
            Some(choice),
            Some("model-1"),
        )
        .await;
    assert_eq!(replay, result);
    let (status, _) = f
        .request(
            Method::POST,
            &f.path("/model-selection"),
            Some(json!({"model":"next","reasoning_effort":"none","expected_revision":0})),
            Some("stale-model"),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, snapshot) = f
        .request(Method::GET, &f.path("/snapshot"), None, None)
        .await;
    assert_eq!(snapshot["model_selection"], result["model_selection"]);
    assert_eq!(snapshot["project_control"]["state"], "paused");
    assert!(snapshot["usage"].as_array().unwrap().is_empty());
    let (status, _) = f
        .request(
            Method::POST,
            &f.path("/model-selection"),
            Some(json!({"model":"valid","reasoning_effort":"high\";invalid"})),
            Some("invalid-model"),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn project_pause_api_blocks_model_entries_but_keeps_notes_and_snapshot_available() {
    let f = Fixture::new().await;
    let (status, paused) = f
        .request(
            Method::POST,
            &f.path("/project-control"),
            Some(json!({"type":"pause"})),
            Some("pause"),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(paused["project_control"]["state"], "paused");
    let (_, snapshot) = f
        .request(Method::GET, &f.path("/snapshot"), None, None)
        .await;
    assert_eq!(snapshot["project_control"]["state"], "paused");
    assert_eq!(
        snapshot["project"]["project_control"],
        snapshot["project_control"]
    );
    let (status, denied) = f
        .request(
            Method::POST,
            &f.path("/runs"),
            Some(json!({"start_authorized":true})),
            Some("start"),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(denied["error"]["code"], "PROJECT_PAUSED");
    let (_, thread) = f
        .request(
            Method::POST,
            &f.path("/discussions"),
            Some(json!({"initial_message":"Paused notes","reply_mode":"record_only"})),
            Some("discussion"),
        )
        .await;
    let path = f.path(&format!(
        "/discussions/{}/messages",
        thread["discussion"]["id"].as_str().unwrap()
    ));
    let (status,_)=f.request(Method::POST,&path,Some(json!({"text":"Keep this suggestion for the next round","reply_mode":"record_only"})),Some("note")).await;
    assert!(status.is_success());
    let (_, resumed) = f
        .request(
            Method::POST,
            &f.path("/project-control"),
            Some(json!({"type":"resume"})),
            Some("resume"),
        )
        .await;
    assert_eq!(resumed["project_control"]["state"], "running");
    let (status, _) = f
        .request(
            Method::POST,
            &f.path("/project-control"),
            Some(json!({"type":"stop"})),
            Some("pause"),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        f.store
            .read(f.project["id"].as_str().unwrap())
            .await
            .unwrap()["usage"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn exact_statement_api_keeps_full_artifact_text_without_model_read_receipts() {
    let f = Fixture::new().await;
    let p = f.project["id"].as_str().unwrap();
    let text = "对每个固定参数 a 存在 C(a)。这里没有声称对全部 a 存在统一 C。";
    let artifact = f
        .store
        .put_artifact(p, "statement.md", text.as_bytes(), "text/markdown")
        .await
        .unwrap();
    f.store.mutate(p, "fixture", None, |project| {
        project["candidates"] = json!([{"id":"candidate-a","claim":text,"exact_statement":text,"statement_artifact_id":artifact["id"],"revision":3,"status":"proposed","problem_version":1}]);
        Ok(Value::Null)
    }).await.unwrap();
    let before = f.store.read(p).await.unwrap();
    let (status, result) = f
        .request(Method::GET, &f.path("/statements/candidate-a"), None, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["statement"]["text"], text);
    assert_eq!(result["statement"]["revision"], 3);
    assert_eq!(result["statement"]["assurance"], "unreviewed");
    assert_eq!(before, f.store.read(p).await.unwrap());
}

#[tokio::test]
async fn memory_api_keeps_unverified_and_withdrawn_attempts_out_of_fact_search() {
    let f = Fixture::new().await;
    f.store.mutate(f.project["id"].as_str().unwrap(), "fixture", None, |p| {
        p["candidates"] = json!([
            {"id":"proposed","claim":"ZXcompact proposed","status":"proposed","problem_version":1},
            {"id":"withdrawn","claim":"ZXcompact failed","status":"withdrawn","problem_version":1}
        ]);
        p["memory_entries"] = json!([{"id":"unfinished","free_summary":"ZXcompact 未完成尝试","status":"unfinished"}]);
        Ok(Value::Null)
    }).await.unwrap();
    let (status, facts) = f
        .request(
            Method::GET,
            &f.path("/memories/search?q=ZXcompact&mode=facts"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(facts["records"].as_array().unwrap().is_empty());
    let (_, experience) = f
        .request(
            Method::GET,
            &f.path("/memories/search?q=ZXcompact"),
            None,
            None,
        )
        .await;
    assert_eq!(experience["mode"], "experience");
    assert!(
        experience["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == "withdrawn")
    );
    assert!(
        experience["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == "unfinished")
    );
    assert_eq!(
        f.request(
            Method::GET,
            &f.path("/memories/search?mode=trusted-summary"),
            None,
            None
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn lifecycle_read_views_filter_authoritative_rows_without_consuming_messages() {
    let f = Fixture::new().await;
    let collections = [
        ("cycles", "cycles"),
        ("routes", "routes"),
        ("messages", "messages"),
        ("advisories", "advisories"),
        ("memory-entries", "memory_entries"),
        ("display-summaries", "display_summaries"),
        ("jobs", "background_jobs"),
        ("proof-checkpoints", "proof_checkpoints"),
        ("pending-assignments", "pending_assignments"),
    ];
    f.store.mutate(f.project["id"].as_str().unwrap(), "fixture", None, |p| {
        p["runs"] = json!([{"id":"run-a","state":"ended"},{"id":"run-b","state":"ended"}]);
        p["sessions"] = json!([{"id":"author-a","run_id":"run-a","state":"closed"}]);
        for (_, collection) in collections {
            p[collection] = json!([
                {"id":"current","run_id":"run-a","session_id":"author-a","state":"queued","summary":"原文入口保留","evidence_refs":["artifact-a"],"native_session_id":"private-binding"},
                {"id":"other","run_id":"run-b","state":"queued"},
                {"id":"handled","run_id":"run-a","state":"handled"}
            ]);
        }
        Ok(Value::Null)
    }).await.unwrap();
    let before = f
        .store
        .read(f.project["id"].as_str().unwrap())
        .await
        .unwrap();
    for (endpoint, field) in collections {
        let (status, response) = f
            .request(
                Method::GET,
                &f.path(&format!(
                    "/{endpoint}?run_id=run-a&state=queued&session_id=author-a"
                )),
                None,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{endpoint}");
        assert_eq!(response[field].as_array().unwrap().len(), 1, "{endpoint}");
        assert_eq!(response[field][0]["id"], "current");
        assert_eq!(response[field][0]["state"], "queued");
        assert_eq!(response[field][0]["evidence_refs"][0], "artifact-a");
        assert!(response[field][0].get("native_session_id").is_none());
        assert_eq!(response["revision"], before["revision"]);
    }
    let after = f
        .store
        .read(f.project["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(
        before, after,
        "Viewing records must not consume inbox messages or invoke models"
    );
    assert_eq!(
        f.request(Method::GET, &f.path("/cycles?run_id=missing"), None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn lifecycle_feedback_rejects_external_role_spoofing_before_service_dispatch() {
    let f = Fixture::new().await;
    let before = f
        .store
        .read(f.project["id"].as_str().unwrap())
        .await
        .unwrap();
    for identity in ["role", "author", "source_session_id", "source"] {
        let mut body = json!({"summary":"不要把人工反馈伪装成审核回执"});
        body[identity] = json!("reviewer");
        assert_eq!(
            f.request(
                Method::POST,
                &f.path("/feedback"),
                Some(body),
                Some(identity)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        before,
        f.store
            .read(f.project["id"].as_str().unwrap())
            .await
            .unwrap()
    );
    assert_eq!(
        f.request(
            Method::PATCH,
            &f.path("/runs/missing/limits"),
            Some(json!({"max_invocations":200})),
            None
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn imported_material_retries_are_idempotent_and_provenance_is_retained() {
    let f = Fixture::new().await;
    let body = json!({"filename":"paper.txt","content":"Theorem text","media_type":"text/plain","provenance":{"original_filename":"paper.pdf","extraction_method":"pdftotext"}});
    let (status, first) = f
        .request(
            Method::POST,
            &f.path("/material-imports"),
            Some(body.clone()),
            Some("import-once"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(first["artifact"]["problem_version"], 1);
    assert_eq!(first["artifact"]["material_role"], "unclassified_input");
    let before = f
        .store
        .read(f.project["id"].as_str().unwrap())
        .await
        .unwrap();
    let (_, again) = f
        .request(
            Method::POST,
            &f.path("/material-imports"),
            Some(body),
            Some("import-once"),
        )
        .await;
    assert_eq!(first, again);
    let after = f
        .store
        .read(f.project["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(before["event_cursor"], after["event_cursor"]);
    assert_eq!(
        first["artifact"]["provenance"]["original_filename"],
        "paper.pdf"
    );
    assert_eq!(
        f.request(
            Method::POST,
            &f.path("/material-imports"),
            Some(json!({"filename":"paper.txt","content":"changed"})),
            Some("import-once")
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn imported_material_preserves_original_whitespace_bytes_and_hash_over_http() {
    use sha2::{Digest, Sha256};
    let f = Fixture::new().await;
    let content = "\u{feff}  \r\n\t原始定理：对每个 ε > 0，存在 C(ε)。\r\n\n  \t";
    let (status, result) = f
        .request(
            Method::POST,
            &f.path("/material-imports"),
            Some(json!({"filename":"  paper.txt  ","content":content,"media_type":"text/plain"})),
            Some("exact-material"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{result}");
    assert_eq!(result["artifact"]["name"], "paper.txt");
    assert_eq!(
        result["artifact"]["sha256"],
        format!("{:x}", Sha256::digest(content.as_bytes()))
    );
    let artifact_id = result["artifact"]["id"].as_str().unwrap();
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(f.path(&format!("/artifacts/{artifact_id}/content")))
                .header(header::HOST, "127.0.0.1:8890")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), MAX_TEXT_BYTES + 1)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), content.as_bytes());
    assert_ne!(bytes.as_ref(), content.trim().as_bytes());
}

#[tokio::test]
async fn imported_material_validates_raw_size_and_rejects_blank_content() {
    let f = Fixture::new().await;
    for (index, content) in [
        json!(" \r\n\t "),
        json!(7),
        json!(format!("x{}", " ".repeat(MAX_TEXT_BYTES))),
    ]
    .into_iter()
    .enumerate()
    {
        let (status, _) = f
            .request(
                Method::POST,
                &f.path("/material-imports"),
                Some(json!({"filename":"paper.txt","content":content})),
                Some(&format!("invalid-material-{index}")),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert!(
        f.store
            .read(f.project["id"].as_str().unwrap())
            .await
            .unwrap()["artifacts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn artifact_only_problem_preview_preserves_full_statement() {
    let f = Fixture::new().await;
    let (_, material) = f
        .request(
            Method::POST,
            &f.path("/material-imports"),
            Some(json!({"filename":"problem.md","content":"证明对任意整数 n 有 n+0=n。"})),
            Some("problem"),
        )
        .await;
    let body = json!({"base_problem_version":1,"new_statement_artifact_id":material["artifact"]["id"],"change_reason":"clarify"});
    let (status, preview) = f
        .request(
            Method::POST,
            &f.path("/problem-revision-previews"),
            Some(body.clone()),
            Some("preview"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        preview["preview"]["new_problem"],
        "证明对任意整数 n 有 n+0=n。"
    );
    let mut unsupported = body;
    unsupported["goal_spec"] = json!({"extra_goal":true});
    assert_eq!(
        f.request(
            Method::POST,
            &f.path("/problem-revision-previews"),
            Some(unsupported),
            Some("extra")
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn independent_interaction_requires_authorization_and_has_cancel_status() {
    let f = Fixture::new().await;
    let (_, discussion) = f
        .request(
            Method::POST,
            &f.path("/discussions"),
            Some(json!({"initial_message":"记录"})),
            Some("d"),
        )
        .await;
    let body = json!({"discussion_id":discussion["discussion"]["id"],"explicit_authorization":true,"expected_run_state":"none","limits":{"duration_seconds":300}});
    let (status, created) = f
        .request(
            Method::POST,
            &f.path("/interaction-executions"),
            Some(body),
            Some("i"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, listed) = f
        .request(Method::GET, &f.path("/interaction-executions"), None, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        listed["interactions"][0]["id"],
        created["interaction"]["id"]
    );
    let path = f.path(&format!(
        "/interaction-executions/{}/cancel",
        created["interaction"]["id"].as_str().unwrap()
    ));
    let (status, cancelled) = f
        .request(
            Method::POST,
            &path,
            Some(json!({"expected_revision":1})),
            Some("cancel"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cancelled["interaction"]["state"], "ended");
}

struct Fixture {
    _temporary: tempfile::TempDir,
    store: V2Store,
    app: Router,
    project: Value,
}

impl Fixture {
    async fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let store = V2Store::connect(
            &temporary.path().join("state.sqlite"),
            &temporary.path().join("data"),
        )
        .await
        .unwrap();
        let project = store
            .create_project(
                json!({"title":"离线样例","problem":"证明 n + 0 = n。"}),
                "fixture",
            )
            .await
            .unwrap();
        let service = V2Service::new(
            store.clone(),
            V2Config {
                codex_command: temporary.path().join("intentionally-missing-codex"),
                ..V2Config::default()
            },
        );
        Self {
            _temporary: temporary,
            store,
            app: router(service, TOKEN.to_owned()),
            project,
        }
    }
    fn path(&self, suffix: &str) -> String {
        format!(
            "/api/v2/research/projects/{}{suffix}",
            self.project["id"].as_str().unwrap()
        )
    }
    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        key: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "127.0.0.1:8890")
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key);
        }
        if body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        let response = self
            .app
            .clone()
            .oneshot(
                request
                    .body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| json!({"raw":String::from_utf8_lossy(&bytes)})),
        )
    }
}

#[tokio::test]
async fn health_is_public_but_project_requires_server_auth() {
    let f = Fixture::new().await;
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["version"], "2.5.3");
    assert_eq!(body["contract"], CONTRACT);
    assert!(!String::from_utf8_lossy(&bytes).contains(TOKEN));
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v2/research/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn current_version_browser_origins_are_accepted_and_previous_ports_are_rejected() {
    let f = Fixture::new().await;
    for (origin, expected) in [
        ("http://127.0.0.1:4335", StatusCode::OK),
        ("http://localhost:4335", StatusCode::OK),
        ("http://[::1]:4335", StatusCode::OK),
        ("http://127.0.0.1:8900", StatusCode::OK),
        ("http://127.0.0.1:4333", StatusCode::FORBIDDEN),
        ("http://127.0.0.1:8898", StatusCode::FORBIDDEN),
    ] {
        let response = f
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v2/research/projects")
                    .header(header::HOST, "127.0.0.1:8900")
                    .header(header::ORIGIN, origin)
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "origin {origin}");
    }
}

#[tokio::test]
async fn hostile_origin_and_rebinding_host_are_rejected_even_with_token() {
    let f = Fixture::new().await;
    for (host, origin) in [
        ("127.0.0.1:8890", "https://attacker.example"),
        ("attacker.example:8890", "http://localhost:4335"),
        ("127.0.0.1:8890", "null"),
    ] {
        let response = f
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v2/research/projects")
                    .header(header::HOST, host)
                    .header(header::ORIGIN, origin)
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn writes_are_explicit_idempotent_and_reject_unknown_fields() {
    let f = Fixture::new().await;
    let input = json!({"title":"另一题","problem":"证明1=1"});
    assert_eq!(
        f.request(
            Method::POST,
            "/api/v2/research/projects",
            Some(input.clone()),
            None
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (status, a) = f
        .request(
            Method::POST,
            "/api/v2/research/projects",
            Some(input.clone()),
            Some("new-project"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, b) = f
        .request(
            Method::POST,
            "/api/v2/research/projects",
            Some(input),
            Some("new-project"),
        )
        .await;
    assert_eq!(a["project"]["id"], b["project"]["id"]);
    assert_eq!(
        f.request(
            Method::POST,
            "/api/v2/research/projects",
            Some(json!({"problem":"changed"})),
            Some("new-project")
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.request(
            Method::POST,
            "/api/v2/research/projects",
            Some(json!({"problem":"test","admin":true})),
            Some("unknown-field")
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(f.store.list_projects().await.unwrap().len(), 2);
}

#[tokio::test]
async fn snapshot_and_event_history_do_not_expose_native_controls() {
    let f = Fixture::new().await;
    let id = f.project["id"].as_str().unwrap();
    f.store.mutate(id,"session.fixture",None,|p|{p["sessions"]=json!([{"id":"session-demo","role":"main","native_session_ref":"private-native","auth_profile_ref":"private-auth","token":"private-token","state":"idle"}]);Ok(p["sessions"].clone())}).await.unwrap();
    let (_, value) = f
        .request(Method::GET, &f.path("/snapshot"), None, None)
        .await;
    let encoded = value.to_string();
    assert!(!encoded.contains("private-native"));
    assert!(!encoded.contains("private-token"));
    assert!(!encoded.contains("_api_receipts"));
    assert_eq!(value["project"]["sessions"][0]["role"], "main");
    let (_, events) = f
        .request(
            Method::GET,
            &f.path("/events?format=json&after=0"),
            None,
            None,
        )
        .await;
    assert!(!events.to_string().contains("private-native"));
    assert!(!events["events"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn annotation_is_versioned_idempotent_and_never_upgrades_assurance() {
    let f = Fixture::new().await;
    let node = &f.project["nodes"][0];
    let input = json!({"node_id":node["id"],"anchor":{"node_revision":1},"kind":"human_endorsement","body":"我认可这个方向，但还需要证明。"});
    let (status, a) = f
        .request(
            Method::POST,
            &f.path("/annotations"),
            Some(input.clone()),
            Some("annotation-1"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let before = f
        .store
        .read(f.project["id"].as_str().unwrap())
        .await
        .unwrap();
    let (_, repeat) = f
        .request(
            Method::POST,
            &f.path("/annotations"),
            Some(input),
            Some("annotation-1"),
        )
        .await;
    assert_eq!(a, repeat);
    let after = f
        .store
        .read(f.project["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(before["event_cursor"], after["event_cursor"]);
    assert_eq!(after["nodes"][0]["assurance"], "unreviewed");
    assert!(rows(&after, "facts").is_empty());
    let path = f.path(&format!(
        "/annotations/{}",
        a["annotation"]["id"].as_str().unwrap()
    ));
    let (status, changed) = f
        .request(
            Method::PATCH,
            &path,
            Some(json!({"body":"更新批注","expected_revision":1})),
            Some("edit-annotation"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(changed["annotation"]["revision"], 2);
    assert_eq!(
        changed["annotation"]["versions"][0]["body"],
        "我认可这个方向，但还需要证明。"
    );
    assert_eq!(
        f.request(
            Method::PATCH,
            &path,
            Some(json!({"body":"stale","expected_revision":1})),
            Some("edit-stale")
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn record_only_discussion_does_not_launch_or_acknowledge_research() {
    let f = Fixture::new().await;
    let input = json!({"node_version_ref":{"node_id":f.project["nodes"][0]["id"],"revision":1},"initial_message":"为什么这样处理？"});
    let (status, discussion) = f
        .request(
            Method::POST,
            &f.path("/discussions"),
            Some(input),
            Some("discussion-1"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let path = f.path(&format!(
        "/discussions/{}/messages",
        discussion["discussion"]["id"].as_str().unwrap()
    ));
    assert_eq!(
        f.request(
            Method::POST,
            &path,
            Some(json!({"text":"先留下笔记","reply_mode":"record_only"})),
            Some("message-1")
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        f.request(
            Method::POST,
            &path,
            Some(json!({"text":"请回答","reply_mode":"explain"})),
            Some("explain-unsupported")
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let project = f
        .store
        .read(f.project["id"].as_str().unwrap())
        .await
        .unwrap();
    assert!(rows(&project, "runs").is_empty());
    assert!(rows(&project, "commands").is_empty());
    assert_eq!(
        project["discussions"][0]["messages"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn view_layout_does_not_change_research_cursor() {
    let f = Fixture::new().await;
    let before = f
        .store
        .read(f.project["id"].as_str().unwrap())
        .await
        .unwrap();
    let (status, value) = f
        .request(
            Method::PUT,
            &f.path("/view-preferences"),
            Some(json!({"positions":{},"selected_view":"board","expected_revision":0})),
            Some("view-1"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["view_preferences"]["revision"], 1);
    let after = f
        .store
        .read(f.project["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(before["research_revision"], after["research_revision"]);
    assert_eq!(before["event_cursor"], after["event_cursor"]);
}

#[tokio::test]
async fn material_content_is_safe_and_cannot_cross_projects() {
    let f = Fixture::new().await;
    let (status, artifact) = f
        .request(
            Method::POST,
            &f.path("/material-imports"),
            Some(json!({"filename":"lemma.md","content":"研究材料","media_type":"text/markdown"})),
            Some("import-1"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let aid = artifact["artifact"]["id"].as_str().unwrap();
    let content_path = f.path(&format!("/artifacts/{aid}/content"));
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(content_path)
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "attachment"
    );
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    let other = f
        .store
        .create_project(json!({"problem":"另一项目"}), "other-project")
        .await
        .unwrap();
    assert_eq!(
        f.request(
            Method::GET,
            &format!(
                "/api/v2/research/projects/{}/artifacts/{aid}/content",
                other["id"].as_str().unwrap()
            ),
            None,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request(
            Method::POST,
            &f.path("/material-imports"),
            Some(json!({"filename":"../escape.md","content":"x"})),
            Some("escape")
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn preview_is_read_only_with_respect_to_problem_and_run() {
    let f = Fixture::new().await;
    let (status,value)=f.request(Method::POST,&f.path("/problem-revision-previews"),Some(json!({"new_problem":"证明 2 + 0 = 2。","base_problem_version":1,"change_reason":"缩小范围"})),Some("preview-1")).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(value["preview"]["proposal_payload_hash"].is_string());
    assert!(value["preview"]["proposal"]["new_statement_artifact_id"].is_string());
    let project = f
        .store
        .read(f.project["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(project["problem"], f.project["problem"]);
    assert_eq!(project["problem_version"], 1);
    assert!(rows(&project, "runs").is_empty());
}

#[tokio::test]
async fn bindings_reject_silent_cross_project_reassignment() {
    let f = Fixture::new().await;
    let input = json!({"platform_conversation_id":"chat-1","kind":"main"});
    assert_eq!(
        f.request(
            Method::POST,
            &f.path("/conversation-bindings"),
            Some(input.clone()),
            Some("bind-1")
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let other = f
        .store
        .create_project(json!({"problem":"另一项目"}), "other-project")
        .await
        .unwrap();
    let path = format!(
        "/api/v2/research/projects/{}/conversation-bindings",
        other["id"].as_str().unwrap()
    );
    assert_eq!(
        f.request(Method::POST, &path, Some(input), Some("bind-other"))
            .await
            .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn unknown_node_revision_is_not_silently_reanchored() {
    let f = Fixture::new().await;
    let path = f.path(&format!(
        "/nodes/{}?revision=99",
        f.project["nodes"][0]["id"].as_str().unwrap()
    ));
    assert_eq!(
        f.request(Method::GET, &path, None, None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn events_replay_is_finite_when_requested_and_cursor_conflicts_fail() {
    let f = Fixture::new().await;
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(f.path("/events?once=true&after=0"))
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let content = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let content = String::from_utf8_lossy(&content);
    assert!(content.contains("id: 1"));
    assert!(content.contains("event: project.created"));
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(f.path("/events?after=0"))
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header("Last-Event-ID", "10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[path = "collaboration23_tests.rs"]
mod collaboration23_tests;
