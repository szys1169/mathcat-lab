//! Scripted six-role integration fixture. No model or mathematical-quality claim.
use super::*;
use research_worker_runtime::research_v2::{TurnError, TurnOutput, TurnRequest, V2Backend};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct ScriptedLab {
    store: V2Store,
    main_turns: AtomicUsize,
    partner_turns: AtomicUsize,
    calls: Mutex<Vec<Value>>,
}

impl ScriptedLab {
    async fn wait_state(
        &self,
        project: &str,
        cancel: &CancellationToken,
        predicate: impl Fn(&Value) -> bool,
    ) -> Result<Value, TurnError> {
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                let state = self.store.read(project).await.map_err(|e| TurnError::new("FIXTURE_STORE", e.message))?;
                if predicate(&state) { return Ok(state); }
                tokio::select! {
                    () = cancel.cancelled() => return Err(TurnError::new("CANCELLED", "mock cancelled")),
                    () = tokio::time::sleep(Duration::from_millis(30)) => {}
                }
            }
        }).await.map_err(|_| TurnError::new("FIXTURE_TIMEOUT", "scripted lab prerequisite did not arrive"))?
    }

    async fn control(
        &self,
        request: &TurnRequest,
        cancel: &CancellationToken,
    ) -> Result<Value, TurnError> {
        let role = request.binding.role.as_str();
        let project = &request.binding.project_id;
        let checkpoint = json!({"proof_goal":"MOCK: check parity","local_assumptions":["n is an integer"],"symbols":[{"name":"n","scope":"this fixture"}],"unfinished_steps":["independent review pending"],"next_step":"read the next exact result","draft_refs":[]});
        let mut control = json!({"protocol_version":"2.2","actions":[],"checkpoint":checkpoint});
        match role {
            "main" => {
                let turn = self.main_turns.fetch_add(1, Ordering::SeqCst) + 1;
                if turn == 1 {
                    control["actions"] = json!([{"type":"request_partner","focus":"FIRST_FOCUS: independently check integer parity; full problem remains available"}]);
                    control["phase_result"] = json!({"next_phase":"waiting","reason":"MOCK: wait for the partner's normal research output","wait_for":"partner"});
                } else if turn == 2 {
                    if !request.prompt.contains("partner_output_ready") {
                        return Err(TurnError::new(
                            "FIXTURE_PARTNER_WAKE",
                            "waiting main was not supplied the partner's ordinary output notification",
                        ));
                    }
                    let state = self
                        .wait_state(project, cancel, |_| {
                            self.partner_turns.load(Ordering::SeqCst) >= 2
                        })
                        .await?;
                    let partner = rows(&state, "sessions")
                        .iter()
                        .find(|s| s["role"] == "partner")
                        .unwrap();
                    control["actions"] = json!([
                        {"type":"assign_partner","partner_session_id":partner["id"],"focus":"SECOND_FOCUS: check the negative-integer case without a fixed method","goal_relation":"test scope after the current derivation"},
                        {"type":"request_advisor"}
                    ]);
                    control["phase_result"] = json!({"next_phase":"research","reason":"MOCK: preserve main research while partners and background roles work"});
                } else {
                    self.wait_state(project, cancel, |state| {
                        self.partner_turns.load(Ordering::SeqCst) >= 3
                            && ["advisor", "memory", "display"].iter().all(|role| {
                                rows(state, "background_jobs")
                                    .iter()
                                    .any(|job| job["kind"] == *role && job["state"] == "completed")
                            })
                    })
                    .await?;
                    control["actions"] = json!([{"type":"submit_candidate","claim":"For every integer n, n(n+1) is even.","proof":"MOCK protocol fixture: either n=2k or n=2k+1, so n(n+1) has an explicit factor 2.","covers_goal":true,"dependency_ids":[],"declared_premises":[]}]);
                    control["phase_result"] = json!({"next_phase":"waiting","reason":"MOCK: wait for the independently bound review session","wait_for":"review"});
                }
            }
            "partner" => {
                let turn = self.partner_turns.fetch_add(1, Ordering::SeqCst) + 1;
                if turn == 2 {
                    let state = self
                        .wait_state(project, cancel, |state| {
                            rows(state, "pending_assignments").iter().any(|a| {
                                a["partner_session_id"] == request.binding.session_id
                                    && a["state"] == "pending"
                            })
                        })
                        .await?;
                    let session = rows(&state, "sessions")
                        .iter()
                        .find(|s| s["id"] == request.binding.session_id)
                        .unwrap();
                    if !session["focus"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("FIRST_FOCUS")
                    {
                        return Err(TurnError::new(
                            "FIXTURE_ASSIGNMENT",
                            "ordinary reassignment changed the current in-flight work",
                        ));
                    }
                }
                if turn >= 3 && !request.prompt.contains("SECOND_FOCUS") {
                    return Err(TurnError::new(
                        "FIXTURE_ASSIGNMENT",
                        "next dispatch did not receive the new assignment",
                    ));
                }
                control["partner_continuation"] = json!({"decision":if turn < 3 {"continue"} else {"complete"},"reason":"MOCK: preserve the old result then use the new assignment","next_focus":"integer parity"});
            }
            "reviewer" => {
                let packet: Value = serde_json::from_slice(
                    &tokio::fs::read(request.binding.workspace.join("verification-packet.json"))
                        .await
                        .map_err(|e| TurnError::new("FIXTURE_IO", e.to_string()))?,
                )
                .map_err(|e| TurnError::new("FIXTURE_JSON", e.to_string()))?;
                if request.binding.native_session_id.is_some()
                    || !request.prompt.contains("MOCK protocol fixture")
                {
                    return Err(TurnError::new(
                        "FIXTURE_REVIEW",
                        "review must be a fresh session reading the frozen proof",
                    ));
                }
                control = json!({"verification":{"snapshot_hash":packet["snapshot_hash"],"verdict":"correct","repair_hints":"","claim_coverage":true,"goal_coverage":true,"verification_report":{"summary":"Scripted protocol response; not a real mathematical audit","critical_errors":[],"gaps":[],"checked_items":["MOCK parity case split"],"unresolved_materials":[],"checked_dependency_ids":[],"repair_checks":[],"premise_audit_status":"complete","checked_premise_ids":[],"undeclared_premises":[],"applicability_gaps":[]}}});
            }
            "advisor" | "memory" | "display" => {
                let state = self
                    .store
                    .read(project)
                    .await
                    .map_err(|e| TurnError::new("FIXTURE_STORE", e.message))?;
                let job = rows(&state, "background_jobs")
                    .iter()
                    .find(|j| j["session_id"] == request.binding.session_id)
                    .ok_or_else(|| TurnError::new("FIXTURE_JOB", "missing background job"))?;
                let refs: Vec<Value> = rows(job, "source_refs").iter().take(1).cloned().collect();
                if role == "memory" && refs.is_empty() {
                    return Err(TurnError::new(
                        "FIXTURE_SOURCE",
                        "memory needs an actual source",
                    ));
                }
                control = json!({"protocol_version":"2.2","actions":[],"background_result":match role {
                    "advisor" => json!({"observation":"MOCK: review main/partner scope","evidence_refs":refs,"concern":"check duplicate assumptions","suggested_change":"read the exact proof","uncertainty":"scripted fixture, no actual research judgment","priority":"normal","repeat_assessment":"insufficient evidence"}),
                    "memory" => json!({"entries":[{"summary":"MOCK: parity attempt remains a source-linked experience","source_refs":refs,"attempt_outcome":"unfinished","tags":["mock"],"goal_relation":"scope check","prerequisite_refs":[]}]}),
                    _ => json!({"title":"MOCK background status","body":"A source-linked demonstration summary, not a certified proof.","source_refs":refs,"actor_refs":[],"route_refs":[],"evidence_refs":[],"statement_refs":[]})
                }});
            }
            _ => return Err(TurnError::new("FIXTURE_ROLE", "unexpected role")),
        }
        Ok(control)
    }
}

#[async_trait::async_trait]
impl V2Backend for ScriptedLab {
    async fn preflight(&self) -> Result<Value, TurnError> {
        Ok(
            json!({"adapter":"scripted-six-role-fixture","authentication":"not-needed","real_model_calls":false}),
        )
    }

    async fn run_turn(
        &self,
        request: TurnRequest,
        events: mpsc::Sender<Value>,
        cancel: CancellationToken,
    ) -> Result<TurnOutput, TurnError> {
        let call_index = {
            let mut calls = self.calls.lock().await;
            let index = calls.len();
            calls.push(json!({"role":request.binding.role,"session_id":request.binding.session_id,"resumed":request.binding.native_session_id.is_some(),"model":request.binding.model,"reasoning_effort":request.binding.reasoning_effort}));
            index
        };
        let control = self.control(&request, &cancel).await?;
        {
            let mut calls = self.calls.lock().await;
            calls[call_index]["action_types"] = json!(
                rows(&control, "actions")
                    .iter()
                    .map(|action| action["type"].clone())
                    .collect::<Vec<_>>()
            );
            calls[call_index]["phase_result"] = control["phase_result"].clone();
        }
        if let Some(path) = &request.control_path {
            tokio::fs::write(path, control.to_string())
                .await
                .map_err(|e| TurnError::new("FIXTURE_IO", e.to_string()))?;
        }
        let text = format!(
            "MOCK {} output: scripted lifecycle behavior; no model call.",
            request.binding.role
        );
        let _ = events
            .send(json!({"type":"message.completed","text":text}))
            .await;
        Ok(TurnOutput {
            native_session_id: Some(request.binding.native_session_id.unwrap_or_else(new_id)),
            text,
            input_tokens: Some(1),
            output_tokens: Some(1),
            ..TurnOutput::default()
        })
    }
}

#[tokio::test]
async fn six_role_mock_lifecycle_runs_through_authoritative_http_views() {
    let temporary = tempfile::tempdir().unwrap();
    let store = V2Store::connect(
        &temporary.path().join("mock.sqlite"),
        &temporary.path().join("workspaces"),
    )
    .await
    .unwrap();
    let project = store.create_project(json!({"title":"MOCK six-role lifecycle","problem":"For every integer n, n(n+1) is even. This is a scripted engineering fixture, not a research benchmark."}),"mock-create").await.unwrap();
    let backend = Arc::new(ScriptedLab {
        store: store.clone(),
        main_turns: AtomicUsize::new(0),
        partner_turns: AtomicUsize::new(0),
        calls: Mutex::new(Vec::new()),
    });
    let service = V2Service::with_backend(store.clone(), V2Config::default(), backend.clone());
    let f = Fixture {
        _temporary: temporary,
        store: store.clone(),
        app: router(service.clone(), TOKEN.into()),
        project,
    };
    // The advisor is explicitly requested on main turn two. Keep its periodic
    // timer beyond this test so advice cannot accidentally wake the first wait.
    // The six-role lifecycle has no human approval driver: explicitly use the
    // automatic organization mode, retaining its existing task-boundary checks.
    let (status, started) = f.request(Method::POST,&f.path("/runs"),Some(json!({"start_authorized":true,"mode":"delegated","duration_seconds":40,"model":"gpt-6-astra","reasoning_effort":"high","limits":{"max_invocations":40,"max_partners":5,"research_soft_seconds":10,"research_hard_seconds":30,"coordination_soft_seconds":10,"coordination_hard_seconds":15,"advisor_interval_seconds":40,"memory_interval_seconds":1,"display_interval_seconds":1}})),Some("mock-start")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    assert_eq!(started["run"]["model"], "gpt-6-astra");
    assert_eq!(started["run"]["reasoning_effort"], "high");
    assert_eq!(started["run"]["limits"]["max_partners"], 5);
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let snapshot = store.read(f.project["id"].as_str().unwrap()).await.unwrap();
            if snapshot["runs"][0]["state"] == "ended" && !rows(&snapshot, "reports").is_empty() {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await;
    let snapshot = store.read(f.project["id"].as_str().unwrap()).await.unwrap();
    service.shutdown().await.unwrap();
    let receipt_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
        .join("tests/results")
        .join(format!("mock-six-role-{}", new_id()));
    tokio::fs::create_dir_all(&receipt_root).await.unwrap();
    let receipt = json!({"version":"2.2.0","kind":"scripted-six-role-http-fixture","real_model_calls":false,"mathematical_quality_evidence":false,"passed":false,"completed":result.is_ok(),"calls":backend.calls.lock().await.clone(),"run":snapshot["runs"][0],"snapshot_file":"snapshot.json"});
    tokio::fs::write(
        receipt_root.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        receipt_root.join("snapshot.json"),
        serde_json::to_vec_pretty(&snapshot).unwrap(),
    )
    .await
    .unwrap();
    assert!(
        result.is_ok(),
        "Mock lifecycle timed out; receipt: {}",
        receipt_root.display()
    );
    assert_eq!(
        snapshot["runs"][0]["stop_reason"],
        "goal_satisfied",
        "{}",
        receipt_root.display()
    );
    assert!(backend.partner_turns.load(Ordering::SeqCst) >= 3);
    let main = rows(&snapshot, "sessions")
        .iter()
        .find(|session| session["role"] == "main")
        .unwrap();
    assert!(rows(&snapshot, "cycles").iter().any(|cycle| {
        cycle["session_id"] == main["id"]
            && cycle["from_phase"] == "waiting"
            && cycle["phase"] == "coordination"
            && cycle["reason"] == "new_input"
    }));
    assert!(rows(&snapshot, "messages").iter().any(|message| {
        message["kind"] == "partner_output_ready"
            && message["priority"] == "normal"
            && message["trust"] == "unreviewed"
            && rows(&snapshot, "artifacts")
                .iter()
                .any(|artifact| artifact["id"] == message["body"]["artifact_id"])
    }));
    assert!(backend.calls.lock().await.iter().all(|call| {
        call["role"] != "partner" || !rows(call, "action_types").contains(&json!("send_feedback"))
    }));
    for role in [
        "main", "partner", "reviewer", "advisor", "memory", "display",
    ] {
        assert!(
            backend.calls.lock().await.iter().any(|c| c["role"] == role),
            "role {role} was not executed"
        );
        for session in rows(&snapshot, "sessions")
            .iter()
            .filter(|s| s["role"] == role)
        {
            assert_eq!(session["model"], "gpt-6-astra");
            assert_eq!(session["reasoning_effort"], "high");
        }
    }
    for call in backend.calls.lock().await.iter() {
        assert_eq!(call["model"], "gpt-6-astra");
        assert_eq!(call["reasoning_effort"], "high");
    }
    assert!(
        rows(&snapshot, "pending_assignments")
            .iter()
            .any(|a| a["state"] == "applied")
    );
    assert!(
        rows(&snapshot, "reviews")
            .iter()
            .any(|r| r["report_validated"] == true)
    );
    for (endpoint, field) in [
        ("cycles", "cycles"),
        ("jobs", "background_jobs"),
        ("advisories", "advisories"),
        ("memory-entries", "memory_entries"),
        ("display-summaries", "display_summaries"),
        ("proof-checkpoints", "proof_checkpoints"),
    ] {
        let (status, value) = f
            .request(Method::GET, &f.path(&format!("/{endpoint}")), None, None)
            .await;
        assert_eq!(status, StatusCode::OK, "{endpoint}");
        assert!(!value[field].as_array().unwrap().is_empty(), "{endpoint}");
    }
    let (_, facts) = f
        .request(
            Method::GET,
            &f.path("/memories/search?mode=facts"),
            None,
            None,
        )
        .await;
    assert_eq!(facts["records"].as_array().unwrap().len(), 1);
    let mut receipt = receipt;
    receipt["passed"] = json!(true);
    receipt["ordinary_partner_output_wakes_waiting_main"] = json!(true);
    receipt["partner_send_feedback_actions"] = json!(0);
    receipt["periodic_advisor_before_wake"] = json!(false);
    receipt["http_model_configuration_reaches_all_six_roles"] = json!(true);
    tokio::fs::write(
        receipt_root.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .await
    .unwrap();
    println!(
        "MOCK six-role engineering receipt: {}",
        receipt_root.display()
    );
}
