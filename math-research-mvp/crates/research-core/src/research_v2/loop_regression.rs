#[cfg(test)]
mod loop_audit_tests {
    use crate::research_v2::*;
    use research_worker_runtime::research_v2::{TurnError, TurnOutput};
    use std::collections::HashSet;

    #[test]
    fn queued_human_command_should_wake_goal_review_wait() {
        let session = json!({"id":"main","role":"main","run_id":"run"});
        let state = json!({"problem_version":1,"commands":[{"run_id":"run","status":"queued","type":"steer_focus"}],"candidates":[{"id":"candidate","run_id":"run","author_session_id":"main","problem_version":1,"covers_goal":true}],"reviews":[{"candidate_id":"candidate","state":"running"}]});
        assert!(
            !enhancements::awaiting_goal_review(&state, &session, &HashSet::new()),
            "queued user command is incorrectly blocked by review wait"
        );
    }

    #[test]
    fn old_acknowledged_command_should_not_enable_empty_polling() {
        let session = json!({"id":"main","role":"main","run_id":"run"});
        let state = json!({"problem_version":1,"commands":[{"run_id":"run","status":"acknowledged","type":"suggest_idea"}],"candidates":[{"id":"candidate","run_id":"run","author_session_id":"main","problem_version":1,"covers_goal":true}],"reviews":[{"candidate_id":"candidate","state":"running"}]});
        assert!(
            enhancements::awaiting_goal_review(&state, &session, &HashSet::new()),
            "old acknowledged command permanently disables review waiting although execute_turn only delivers queued commands"
        );
    }

    struct AuditBackend {
        mode: &'static str,
    }
    #[async_trait::async_trait]
    impl V2Backend for AuditBackend {
        async fn preflight(&self) -> Result<Value, TurnError> {
            Ok(json!({"adapter":"audit-fake"}))
        }
        async fn run_turn(
            &self,
            request: TurnRequest,
            events: mpsc::Sender<Value>,
            _cancel: CancellationToken,
        ) -> Result<TurnOutput, TurnError> {
            if request.binding.role == "reviewer" {
                return Err(TurnError::new(
                    "PROVIDER_ERROR",
                    "simulated reviewer failure",
                ));
            }
            let control = if self.mode == "bad_path" {
                json!({"actions":[{"type":"submit_candidate","claim":"a claim","proof_path":"missing-proof.md","covers_goal":true,"declared_premises":[]}]})
            } else {
                json!({"actions":[{"type":"submit_candidate","claim":"a claim","proof":"a proof","covers_goal":true,"declared_premises":[]}]})
            };
            tokio::fs::write(request.control_path.as_ref().unwrap(), control.to_string())
                .await
                .unwrap();
            let _ = events
                .send(json!({"type":"activity.delta","text":"fixture"}))
                .await;
            Ok(TurnOutput {
                native_session_id: Some("12345678-1234-1234-1234-123456789abc".into()),
                text: "Fixture output".into(),
                input_tokens: Some(1),
                output_tokens: Some(1),
                cached_input_tokens: None,
                reconstructed: false,
            })
        }
    }
    async fn fixture(mode: &'static str) -> (tempfile::TempDir, V2Service, String) {
        let temp = tempfile::tempdir().unwrap();
        let store = V2Store::connect(&temp.path().join("audit.sqlite"), &temp.path().join("data"))
            .await
            .unwrap();
        let p = store
            .create_project(json!({"title":"audit","problem":"test problem"}), "create")
            .await
            .unwrap();
        let service =
            V2Service::with_backend(store, V2Config::default(), Arc::new(AuditBackend { mode }));
        let project = p["id"].as_str().unwrap().to_owned();
        service.start_run(&project,json!({"start_authorized":true,"duration_seconds":60,"limits":{"max_invocations":10}}),"start").await.unwrap();
        (temp, service, project)
    }
    async fn waiting(service: &V2Service, project: &str, role: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let state = service.store.read(project).await.unwrap();
                if (role == "reviewer"
                    && array(&state, "reviews")
                        .iter()
                        .any(|r| r["state"] == "failed"))
                    || (role == "main" && state["runs"][0]["state"] == "ended")
                {
                    return state;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn failed_reviewer_should_release_main_wait() {
        let (_temp, service, project) = fixture("reviewer_error").await;
        let state = waiting(&service, &project, "reviewer").await;
        let main = array(&state, "sessions")
            .iter()
            .find(|s| s["role"] == "main")
            .unwrap();
        let blocked = enhancements::awaiting_goal_review(&state, main, &HashSet::new());
        let review_state = state["reviews"][0]["state"].clone();
        tokio::time::sleep(Duration::from_millis(450)).await;
        let later = service.store.read(&project).await.unwrap();
        service.shutdown().await.unwrap();
        assert!(
            !blocked,
            "reviewer failed but review remains {review_state}; main blocked, run={}, invocations {} -> {}",
            later["runs"][0]["state"],
            array(&state, "usage").len(),
            array(&later, "usage").len()
        );
    }
    #[tokio::test]
    async fn invalid_candidate_path_should_not_strand_running_run() {
        let (_temp, service, project) = fixture("bad_path").await;
        let state = waiting(&service, &project, "main").await;
        tokio::time::sleep(Duration::from_millis(450)).await;
        let later = service.store.read(&project).await.unwrap();
        service.shutdown().await.unwrap();
        assert!(
            later["runs"][0]["state"] != "running"
                || array(&later, "sessions")
                    .iter()
                    .any(|s| s["state"] == "idle" || s["state"] == "active"),
            "invalid proof_path stranded run: state={}, error={}, invocations {} -> {}",
            later["runs"][0]["state"],
            later["sessions"][0]["last_error"],
            array(&state, "usage").len(),
            array(&later, "usage").len()
        );
    }

    #[tokio::test]
    async fn same_proof_resubmitted_as_full_goal_should_preserve_new_intent() {
        let temp = tempfile::tempdir().unwrap();
        let store = V2Store::connect(&temp.path().join("audit.sqlite"), &temp.path().join("data"))
            .await
            .unwrap();
        let p = store
            .create_project(json!({"title":"audit","problem":"a claim"}), "create")
            .await
            .unwrap();
        let project = p["id"].as_str().unwrap();
        let service = V2Service::with_backend(
            store,
            V2Config::default(),
            Arc::new(AuditBackend { mode: "unused" }),
        );
        let run = json!({"id":"run","state":"running","problem_version":1,"control_epoch":1,"deadline_at":(Utc::now()+chrono::Duration::seconds(60)).to_rfc3339()});
        let author = service
            .store
            .mutate(project, "audit.seed", None, |state| {
                push(state, "runs", run.clone());
                let author = new_session(state, &run, "main", None, &None);
                push(state, "sessions", author.clone());
                Ok(author)
            })
            .await
            .unwrap();
        let mut output_ids = Vec::new();
        for covers_goal in [false, true] {
            let control = json!({"actions":[{"type":"submit_candidate","claim":"a claim","proof":"identical proof","covers_goal":covers_goal,"declared_premises":[],"action_id":format!("submit-{covers_goal}")}]});
            // These are two model returns carrying distinct coverage intent,
            // while the submitted mathematical proof is byte-for-byte identical.
            let artifact = service
                .store
                .put_artifact(
                    project,
                    &format!("output-{covers_goal}.json"),
                    control.to_string().as_bytes(),
                    "application/json",
                )
                .await
                .unwrap();
            output_ids.push(artifact["id"].clone());
            service
                .apply_actions(
                    project,
                    "run",
                    author["id"].as_str().unwrap(),
                    1,
                    &control,
                    &artifact,
                    temp.path(),
                )
                .await
                .unwrap();
        }
        let state = service.store.read(project).await.unwrap();
        assert_ne!(output_ids[0], output_ids[1]);
        // Coverage changes the goal references in the mathematical scope, so
        // a distinct candidate is valid. Neither return may rewrite the proof.
        for candidate in array(&state, "candidates") {
            let proof_id = candidate["proof_artifact_id"].as_str().unwrap();
            let (_, proof) = service
                .store
                .read_artifact(project, proof_id)
                .await
                .unwrap();
            assert_eq!(proof, b"identical proof");
        }
        assert!(
            array(&state, "candidates")
                .iter()
                .any(|c| c["covers_goal"] == true),
            "second submission silently discarded: candidates={}, covers_goal={}",
            array(&state, "candidates").len(),
            state["candidates"][0]["covers_goal"]
        );
    }

    #[test]
    fn undisplayed_incremental_memory_should_remain_unread() {
        let memories=(0..13).map(|i|json!({"id":format!("m{i}"),"session_id":"partner","text":format!("DISTINCT_FEEDBACK_{i:02}")})).collect::<Vec<_>>();
        let mut prepared = json!({"session":{"id":"main","native_session_id":"native","turns_completed":1,"seen_memory_ids":[]},"memories":memories,"run":{}});
        let first = build_prompt(&prepared, "main", Path::new("control.json"));
        // Identical assignment to execute_turn's message.completed transaction.
        let mut session = prepared["session"].clone();
        loop_control::mark_memories_supplied(&mut session, &prepared);
        prepared["session"] = session;
        let second = build_prompt(&prepared, "main", Path::new("control2.json"));
        assert!(
            first.contains("DISTINCT_FEEDBACK_12") || second.contains("DISTINCT_FEEDBACK_12"),
            "undisplayed one of 13 messages never delivered through delta, but marked seen after first turn"
        );
    }

    #[tokio::test]
    async fn inner_submission_rechecks_session_epoch_after_outer_dispatch() {
        let temp = tempfile::tempdir().unwrap();
        let store = V2Store::connect(&temp.path().join("guard.sqlite"), &temp.path().join("data"))
            .await
            .unwrap();
        let p = store
            .create_project(json!({"title":"guard","problem":"precise goal"}), "create")
            .await
            .unwrap();
        let project = p["id"].as_str().unwrap();
        let service = V2Service::with_backend(
            store,
            V2Config::default(),
            Arc::new(AuditBackend { mode: "unused" }),
        );
        let capture=service.store.mutate(project,"test.seed",None,|state| {
            push(state,"runs",json!({"id":"r","state":"running","problem_version":1,"control_epoch":1,"deadline_at":(Utc::now()+chrono::Duration::seconds(60)).to_rfc3339(),"limits":{}}));
            let run=entity(state,"runs","r").unwrap().clone(); let mut author=new_session(state,&run,"main",None,&None);author["id"]=json!("a");
            push(state,"sessions",author);lab::prepare_turn_at(state,"r","a",&now())
        }).await.unwrap();
        let mut output = service
            .store
            .put_artifact(project, "output.md", b"mathematics", "text/markdown")
            .await
            .unwrap();
        output["execution_capture"] = capture;
        service
            .store
            .mutate(project, "test.urgent", None, |state| {
                entity_mut(state, "sessions", "a")?["control_epoch"] = json!(2);
                Ok(Value::Null)
            })
            .await
            .unwrap();
        let rejected=service.apply_actions_inner(project,"r","a",1,&json!({"actions":[{"type":"submit_candidate","claim":"precise claim","proof":"full proof","declared_premises":[]}]}),&output,temp.path()).await.unwrap_err();
        assert_eq!(rejected.code, "STALE_CONTROL_EPOCH");
        assert!(array(&service.store.read(project).await.unwrap(), "candidates").is_empty());
    }
    #[tokio::test]
    async fn reconstructed_main_receives_saved_mathematical_handoff() {
        let temp = tempfile::tempdir().unwrap();
        let store = V2Store::connect(
            &temp.path().join("handoff.sqlite"),
            &temp.path().join("data"),
        )
        .await
        .unwrap();
        let p = store
            .create_project(
                json!({"title":"handoff","problem":"fixed original goal"}),
                "create",
            )
            .await
            .unwrap();
        let project = p["id"].as_str().unwrap();
        let service = V2Service::with_backend(
            store,
            V2Config::default(),
            Arc::new(AuditBackend { mode: "unused" }),
        );
        service.store.mutate(project,"test.seed",None,|state| {
            push(state,"runs",json!({"id":"r","state":"running","problem_version":1,"control_epoch":2,"limits":{},"deadline_at":(Utc::now()+chrono::Duration::seconds(60)).to_rfc3339()}));
            push(state,"sessions",json!({"id":"old","role":"main","run_id":"r","problem_version":1,"state":"closed","latest_checkpoint_id":"cp","workspace_path":"old draft workspace"}));
            push(state,"proof_checkpoints",json!({"id":"cp","session_id":"old","proof_goal":"prove exact local lemma","local_assumptions":["epsilon > 0"],"symbols":{"n":"fixed integer"},"unfinished_steps":["bound remainder"],"next_step":"check uniformity","requires_recheck":true}));Ok(Value::Null)
        }).await.unwrap();
        service.ensure_main(project, "r").await.unwrap();
        let state = service.store.read(project).await.unwrap();
        let main = array(&state, "sessions")
            .iter()
            .find(|s| s["role"] == "main" && s["state"] == "idle")
            .unwrap();
        assert_eq!(main["latest_checkpoint_id"], "cp");
        assert_eq!(main["recovery_context"]["predecessor_session_id"], "old");
        let session_id = main["id"].as_str().unwrap();
        let capture = service
            .store
            .mutate(project, "test.prepare", None, |state| {
                lab::prepare_turn_at(state, "r", session_id, &now())
            })
            .await
            .unwrap();
        assert_eq!(
            capture["checkpoint"]["proof_goal"],
            "prove exact local lemma"
        );
        assert_eq!(capture["checkpoint"]["local_assumptions"][0], "epsilon > 0");
    }
}
