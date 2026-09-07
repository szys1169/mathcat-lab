use super::*;

fn push(state: &mut Value, collection: &str, value: Value) {
    if !state[collection].is_array() {
        state[collection] = json!([]);
    }
    state[collection].as_array_mut().unwrap().push(value);
}

async fn waiting_fixture() -> Fixture {
    let f = Fixture::new().await;
    f.store.mutate(f.project["id"].as_str().unwrap(),"seed",None,|state|{
        push(state,"runs",json!({"id":"r","state":"waiting_human","problem_version":1,"control_epoch":1,"human_question_id":"q","deadline_at":null,"revision":1,"limits":{"max_invocations":1}}));
        push(state,"human_questions",json!({"id":"q","run_id":"r","state":"open","body":"Which assumptions?","problem_version":1,"revision":1}));
        Ok(Value::Null)
    }).await.unwrap();
    f
}

#[tokio::test]
async fn http_feedback_withdraw_and_snapshot_share_honest_receipts() {
    let f = waiting_fixture().await;
    let body = json!({"run_id":"r","body":"Check A first","priority":"normal"});
    let (status, response) = f
        .request(
            Method::POST,
            &f.path("/feedback"),
            Some(body.clone()),
            Some("feedback"),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    let id = response["command"]["id"].as_str().unwrap();
    let (_, replay) = f
        .request(
            Method::POST,
            &f.path("/feedback"),
            Some(body),
            Some("feedback"),
        )
        .await;
    assert_eq!(replay["command"]["id"], id);
    let (status, result) = f
        .request(
            Method::POST,
            &f.path(&format!("/feedback/{id}/withdraw")),
            Some(json!({"expected_revision":1})),
            Some("withdraw"),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let (_, snapshot) = f
        .request(Method::GET, &f.path("/snapshot"), None, None)
        .await;
    assert_eq!(snapshot["runs"][0]["state"], "waiting_human");
    assert_eq!(snapshot["feedback_traces"][0]["status"], "cancelled");
    assert_eq!(
        snapshot["feedback_traces"][0]["messages"][0]["state"],
        "cancelled"
    );
    assert!(snapshot["proof_tree"].is_object());
    assert_eq!(snapshot["human_questions"][0]["id"], "q");
}

#[tokio::test]
async fn http_explicit_answer_rejects_stale_question_and_wrong_run() {
    let f = waiting_fixture().await;
    for (key, input) in [
        (
            "stale",
            json!({"run_id":"r","expected_revision":9,"body":"A"}),
        ),
        (
            "wrong-run",
            json!({"run_id":"other","expected_revision":1,"body":"A"}),
        ),
    ] {
        let (status, _) = f
            .request(
                Method::POST,
                &f.path("/human-questions/q/answer"),
                Some(input),
                Some(key),
            )
            .await;
        assert!(status.is_client_error());
    }
    let (_, questions) = f
        .request(
            Method::GET,
            &f.path("/human-questions?run_id=r"),
            None,
            None,
        )
        .await;
    assert_eq!(questions["human_questions"][0]["state"], "open");
}

#[tokio::test]
async fn http_urgent_upgrade_reuses_record_without_unlocking_human_wait() {
    let f = waiting_fixture().await;
    let (_, response) = f
        .request(
            Method::POST,
            &f.path("/feedback"),
            Some(json!({"run_id":"r","body":"Check A"})),
            Some("feedback"),
        )
        .await;
    let id = response["command"]["id"].as_str().unwrap();
    let (status, response) = f
        .request(
            Method::POST,
            &f.path(&format!("/feedback/{id}/escalate")),
            Some(json!({"expected_revision":1})),
            Some("escalate"),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["command"]["id"], id);
    assert_eq!(response["command"]["priority"], "urgent");
    let state = f
        .store
        .read(f.project["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(rows(&state, "messages").len(), 1);
    assert_eq!(state["runs"][0]["state"], "waiting_human");
}

#[tokio::test]
async fn http_readonly_selection_and_annotation_conversion_preserve_separate_channels() {
    let f = waiting_fixture().await;
    let artifact = f
        .store
        .put_artifact(
            f.project["id"].as_str().unwrap(),
            "source.md",
            b"Exact original A implies P",
            "text/markdown",
        )
        .await
        .unwrap();
    let selection = json!({"kind":"artifact","id":artifact["id"],"quote":"A implies P"});
    let (status, discussion) = f
        .request(
            Method::POST,
            &f.path("/discussions"),
            Some(json!({"initial_message":"Read the source","selection_ref":selection})),
            Some("discussion"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{discussion}");
    assert_eq!(
        discussion["discussion"]["selection"]["artifact_id"],
        artifact["id"]
    );
    let (_, state) = f
        .request(Method::GET, &f.path("/snapshot"), None, None)
        .await;
    assert!(rows(&state, "commands").is_empty());
    assert!(rows(&state, "messages").is_empty());
    assert!(rows(&state, "facts").is_empty());
    let (status,note)=f.request(Method::POST,&f.path("/annotations"),Some(json!({"anchor":{"node_revision":1},"selection_ref":{"kind":"artifact","id":artifact["id"],"revision":1},"kind":"comment","body":"Check A"})),Some("annotation")).await;
    assert_eq!(status, StatusCode::CREATED, "{note}");
    let (status,response)=f.request(Method::POST,&f.path("/feedback"),Some(json!({"run_id":"r","body":"User approved Check A","summary":"User approved Check A","priority":"normal","expected_versions":{"problem_version":1,"control_epoch":1},"annotation_id":note["annotation"]["id"],"annotation_revision":1,"selection_ref":{"kind":"artifact","id":artifact["id"],"revision":1},"evidence_refs":[{"kind":"artifact","id":artifact["id"],"revision":1}]})),Some("convert")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    let state = f
        .store
        .read(f.project["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(
        state["annotations"][0]["linked_command_ids"],
        json!([response["command"]["id"]])
    );
}

struct CollaborationBackend {
    store: V2Store,
    main_turns: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl research_worker_runtime::research_v2::V2Backend for CollaborationBackend {
    async fn preflight(&self) -> Result<Value, research_worker_runtime::research_v2::TurnError> {
        Ok(json!({"real_model_calls":false,"adapter":"collaboration23-script"}))
    }
    async fn run_turn(
        &self,
        request: research_worker_runtime::research_v2::TurnRequest,
        events: tokio::sync::mpsc::Sender<Value>,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<
        research_worker_runtime::research_v2::TurnOutput,
        research_worker_runtime::research_v2::TurnError,
    > {
        use research_worker_runtime::research_v2::{TurnError, TurnOutput};
        if request.binding.role != "main" {
            return Err(TurnError::new(
                "UNEXPECTED_ROLE",
                "fixture should pause after creating the task",
            ));
        }
        let turn = self
            .main_turns
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut control = json!({"protocol_version":"2.2","checkpoint":{"proof_goal":"P under A","local_assumptions":[],"symbols":[],"unfinished_steps":["read human answer"],"next_step":"check A"},"phase_result":{"next_phase":"waiting","reason":"scripted question","wait_for":"human"}});
        if turn == 0 {
            control["actions"] = json!([{"type":"wait_for_user","question":"Should we assume A?"}]);
        } else {
            let state = self
                .store
                .read(&request.binding.project_id)
                .await
                .map_err(|e| TurnError::new("STORE", e.message))?;
            let opinion = rows(&state, "commands")
                .iter()
                .find(|c| c["type"] == "suggest_idea")
                .ok_or_else(|| TurnError::new("MISSING_OPINION", "normal opinion was lost"))?;
            let answer = rows(&state, "commands")
                .iter()
                .find(|c| c["type"] == "human_answer")
                .ok_or_else(|| TurnError::new("MISSING_ANSWER", "answer was lost"))?;
            if !request.prompt.contains(opinion["id"].as_str().unwrap())
                || !request.prompt.contains(answer["id"].as_str().unwrap())
            {
                return Err(TurnError::new(
                    "MISSING_DELIVERY",
                    "actual invocation prompt lacks opinions",
                ));
            }
            control["actions"] = json!([
                {"type":"command_response","command_id":opinion["id"],"disposition":"adopted","response":"Assign an independent scope check"},
                {"type":"command_response","command_id":answer["id"],"disposition":"adopted","response":"Use the clarified assumption"},
                {"type":"request_partner","focus":"Independently inspect A without restricting methods","source_command_ids":[opinion["id"]]},
                {"type":"wait_for_user","question":"The engineering fixture reached its final checkpoint."}
            ]);
        }
        tokio::fs::write(request.control_path.as_ref().unwrap(), control.to_string())
            .await
            .map_err(|e| TurnError::new("IO", e.to_string()))?;
        let _=events.send(json!({"type":"message.completed","text":"Scripted collaboration checkpoint; no real model"})).await;
        Ok(TurnOutput {
            native_session_id: Some(request.binding.native_session_id.unwrap_or_else(new_id)),
            text: "Scripted collaboration checkpoint".into(),
            input_tokens: Some(1),
            output_tokens: Some(1),
            ..TurnOutput::default()
        })
    }
}

#[tokio::test]
async fn scripted_http_collaboration_reaches_actual_disposition_and_task_receipt() {
    let temporary = tempfile::tempdir().unwrap();
    let store = V2Store::connect(
        &temporary.path().join("state.sqlite"),
        &temporary.path().join("workspaces"),
    )
    .await
    .unwrap();
    let project=store.create_project(json!({"title":"scripted collaboration","problem":"Prove P under explicitly supplied assumptions. Engineering fixture only."}),"create").await.unwrap();
    let backend = Arc::new(CollaborationBackend {
        store: store.clone(),
        main_turns: std::sync::atomic::AtomicUsize::new(0),
    });
    let service = V2Service::with_backend(store.clone(), V2Config::default(), backend.clone());
    let f = Fixture {
        _temporary: temporary,
        store: store.clone(),
        app: router(service, TOKEN.into()),
        project,
    };
    // This fixture checks explicit question/answer delivery and the resulting
    // automatic assignment receipt. Per-round human approval is exercised by
    // whiteboard24_tests instead; it must not stall this legacy receipt check.
    let (status,started)=f.request(Method::POST,&f.path("/runs"),Some(json!({"start_authorized":true,"mode":"delegated","duration_seconds":60,"limits":{"max_invocations":8,"max_partners":2,"advisor_interval_seconds":3600,"memory_interval_seconds":3600,"display_interval_seconds":3600}})),Some("start")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let run = started["run"]["id"].as_str().unwrap();
    let question = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let state = store.read(f.project["id"].as_str().unwrap()).await.unwrap();
            if let Some(q) = rows(&state, "human_questions")
                .iter()
                .find(|q| q["state"] == "open")
            {
                break q.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let (status, opinion) = f
        .request(
            Method::POST,
            &f.path("/feedback"),
            Some(json!({"run_id":run,"body":"Please independently check the scope of A"})),
            Some("opinion"),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{opinion}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        backend.main_turns.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let state = store.read(f.project["id"].as_str().unwrap()).await.unwrap();
    assert_eq!(state["runs"][0]["state"], "waiting_human");
    let (status,answer)=f.request(Method::POST,&f.path(&format!("/human-questions/{}/answer",question["id"].as_str().unwrap())),Some(json!({"run_id":run,"expected_revision":question["revision"],"body":"Yes, assume A"})),Some("answer")).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let snapshot = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let (_, state) = f
                .request(Method::GET, &f.path("/snapshot"), None, None)
                .await;
            if state["feedback_traces"].as_array().is_some_and(|traces| {
                traces.iter().any(|t| {
                    t["command_id"] == opinion["command"]["id"]
                        && t["disposition"] == "adopted"
                        && !rows(t, "result_refs").is_empty()
                })
            }) {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let trace = rows(&snapshot, "feedback_traces")
        .iter()
        .find(|t| t["command_id"] == opinion["command"]["id"])
        .unwrap();
    assert_eq!(trace["delivery_state"], "confirmed");
    assert!(
        rows(trace, "delivery")
            .iter()
            .any(|d| d["certainty"] == "confirmed")
    );
    assert!(
        rows(trace, "result_refs")
            .iter()
            .any(|r| r["collection"] == "tasks")
    );
    assert_eq!(snapshot["revision"], snapshot["project"]["revision"]);
    assert_eq!(snapshot["proof_tree"]["revision"], snapshot["revision"]);
    let (status, stop) = f
        .request(
            Method::POST,
            &f.path("/commands"),
            Some(json!({"type":"stop_run","run_id":run})),
            Some("cleanup"),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{stop}");
}

#[tokio::test]
async fn http_fact_challenge_uses_actual_fact_state_revision() {
    let f = waiting_fixture().await;
    let project = f.project["id"].as_str().unwrap();
    let proof = f
        .store
        .put_artifact(
            project,
            "fixture-proof.md",
            b"Scripted protocol proof; no mathematical certification.",
            "text/markdown",
        )
        .await
        .unwrap();
    let packet = f
        .store
        .put_artifact(
            project,
            "fixture-review-packet.json",
            b"{}",
            "application/json",
        )
        .await
        .unwrap();
    f.store.mutate(project,"fixture.independent_review",None,|state|{
        state["runs"][0]["limits"]["duration_seconds"]=Value::Null;
        push(state,"candidates",json!({"id":"candidate","run_id":"r","problem_version":1,"control_epoch":1,"author_session_id":"fixture-author","snapshot_hash":"fixture-frozen","proof_artifact_id":proof["id"],"claim":"A","exact_statement":"A","status":"submitted","covers_goal":false,"verification_engine":"rethlas-adapted/2.2.0","dependency_ids":[],"declared_premises":[],"revision":1}));
        push(state,"reviews",json!({"id":"independent-review","run_id":"r","candidate_id":"candidate","reviewer_session_id":"fixture-independent-reviewer","snapshot_hash":"fixture-frozen","state":"completed","verdict":"accepted","report_validated":true,"engine":"rethlas-adapted/2.2.0","packet_artifact_id":packet["id"],"revision":1}));
        Ok(Value::Null)
    }).await.unwrap();
    let fact = f.store.admit_candidate(project, "candidate").await.unwrap();
    assert_eq!(fact["assurance"], "model_reviewed");
    // Advance the real state version using the human control API, never by inserting a Fact.
    for revision in [1, 2] {
        let (status,response)=f.request(Method::POST,&f.path("/commands"),Some(json!({"type":"challenge_evidence","run_id":"r","target":{"kind":"fact","id":fact["id"],"revision":revision}})),Some(&format!("prior-challenge-{revision}"))).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    }
    let (status,_)=f.request(Method::POST,&f.path("/commands"),Some(json!({"type":"challenge_evidence","run_id":"r","target":{"kind":"fact","id":fact["id"],"revision":1}})),Some("old-body-version")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status,response)=f.request(Method::POST,&f.path("/commands"),Some(json!({"type":"challenge_evidence","run_id":"r","target":{"kind":"fact","id":fact["id"],"revision":3}})),Some("challenge")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    let state = f.store.read(project).await.unwrap();
    assert_eq!(state["facts"][0]["validity"], "challenged");
    assert_eq!(state["facts"][0]["revision"], 4);
}
