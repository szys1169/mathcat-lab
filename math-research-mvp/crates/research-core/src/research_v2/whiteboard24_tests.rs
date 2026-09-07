use super::*;

#[tokio::test]
async fn submission_recovery_failed_submit_does_not_stage_empty_wait_plan() {
    let temp = tempfile::tempdir().unwrap();
    let store = super::super::V2Store::connect(
        &temp.path().join("recovery.sqlite"),
        &temp.path().join("data"),
    )
    .await
    .unwrap();
    let project = store
        .create_project(json!({"problem":"Prove P"}), "create")
        .await
        .unwrap();
    let pid = project["id"].as_str().unwrap();
    let service = V2Service::new(store, super::super::V2Config::default());
    service
        .store
        .mutate(pid, "fixture", None, |p| {
            let seed = state();
            p["runs"] = seed["runs"].clone();
            p["runs"][0]["problem_version"] = json!(1);
            p["sessions"] = json!([seed["sessions"][0]]);
            p["sessions"][0]["phase"] = json!("coordination");
            Ok(Value::Null)
        })
        .await
        .unwrap();
    let source = service
        .store
        .put_artifact(pid, "problem.txt", b"Prove P", "text/plain")
        .await
        .unwrap();
    let mut artifact = service
        .store
        .put_artifact(
            pid,
            "output.md",
            b"Candidate is awaiting review",
            "text/markdown",
        )
        .await
        .unwrap();
    artifact["execution_capture"] =
        json!({"phase":"coordination","run_control_epoch":1,"session_control_epoch":1});
    let control = json!({"actions":[{"type":"submit_candidate","claim":"P","proof":"A self-contained fixture proof.","covers_goal":true,"source_artifact_ids":[source["id"]],"declared_premises":[{"kind":"source","ref_id":source["id"],"sha256":source["sha256"],"usage_location":"target","applicability":"same target"}]}],"phase_result":{"next_phase":"waiting","reason":"v2 submitted; await independent review","wait_for":[]}});
    let error = service
        .apply_actions(pid, "r", "main", 1, &control, &artifact, temp.path())
        .await
        .unwrap_err();
    assert_eq!(error.code, "PREMISE_READ_REQUIRED");
    service
        .store
        .mutate(pid, "session.failed", None, |p| {
            super::super::loop_control::postprocess_failure(p, "main", &error.code, &error.message)
        })
        .await
        .unwrap();
    let p = service.store.read(pid).await.unwrap();
    assert!(
        pending(&p, "r").is_none(),
        "failed mathematical action must not freeze an approval barrier"
    );
    assert!(array(&p, "planning_proposals").is_empty());
    assert!(array(&p, "reviews").is_empty());
    assert_eq!(p["sessions"][0]["state"], "idle");
    assert_eq!(
        p["sessions"][0]["last_error"]["code"],
        "PREMISE_READ_REQUIRED"
    );
    assert!(!blocked(&p, &p["sessions"][0]));
    assert_eq!(p["action_receipts"][0]["status"], "rejected");
}

#[test]
fn submission_recovery_wait_proposals_follow_real_queue_and_completion() {
    for finished_before_approval in [false, true] {
        let mut s = state();
        s["sessions"] = json!([s["sessions"][0]]);
        s["tasks"] = json!([]);
        let output = json!({"id":"wait-output","execution_capture":{"phase":"coordination"}});
        let control = json!({"actions":[],"phase_result":{"next_phase":"waiting","reason":"review submitted"}});
        assert_eq!(
            stage(&mut s, "r", "main", &control, &output, &None)
                .unwrap_err()
                .code,
            "LAB_WAIT_UNAVAILABLE"
        );
        assert!(pending(&s, "r").is_none());
        s["reviews"] =
            json!([{"id":"review","run_id":"r","candidate_id":"candidate","state":"queued"}]);
        assert!(stage(&mut s, "r", "main", &control, &output, &None).unwrap());
        let plan = pending(&s, "r").unwrap().clone();
        if finished_before_approval {
            s["reviews"][0]["state"] = json!("completed");
        }
        let result = decide(
            &mut s,
            plan["id"].as_str().unwrap(),
            &json!({"expected_revision":1,"decision":"approve"}),
            &None,
        )
        .unwrap();
        let expected = if finished_before_approval {
            "coordination"
        } else {
            "waiting"
        };
        assert_eq!(s["sessions"][0]["phase"], expected);
        assert_eq!(result["effective_next_phase"], expected);
        assert!(!blocked(&s, &s["sessions"][0]));
    }
}

fn state() -> Value {
    json!({"id":"p","problem":"For every n, prove P(n).","problem_version":1,"revision":1,
        "runs":[{"id":"r","state":"running","mode":"collaborative","control_epoch":1,"revision":1,"deadline_at":null,"limits":{"max_partners":5,"max_invocations":100}}],
        "sessions":[{"id":"main","run_id":"r","role":"main","state":"idle","phase":"research","revision":1,"problem_version":1,"control_epoch":1},
        {"id":"partner","run_id":"r","role":"partner","state":"active","invocation_in_flight":true,"task_id":"t","revision":1,"problem_version":1,"control_epoch":1}],
        "tasks":[{"id":"t","run_id":"r","owner_session_id":"partner","state":"running","task_epoch":1,"revision":1}],"routes":[],"facts":[],"usage":[],"commands":[]})
}

fn submit(state: &mut Value, actions: Value) -> Value {
    let mut control =
        json!({"phase_result":{"next_phase":"research","reason":"Explore independent lemma"}});
    control["actions"] = actions;
    stage(
        state,
        "r",
        "main",
        &control,
        &json!({"id":"output","execution_capture":{"phase":"research"}}),
        &None,
    )
    .unwrap();
    pending(state, "r").unwrap().clone()
}

#[test]
fn research_phase_assignment_batch_waits_and_approval_applies_once() {
    let mut s = state();
    let plan = submit(
        &mut s,
        json!([{"type":"request_partner","focus":"Prove lemma L"},{"type":"set_focus","focus":"Prove global theorem"}]),
    );
    assert_eq!(array(&s, "sessions").len(), 2);
    assert!(blocked(&s, entity(&s, "sessions", "main").unwrap()));
    assert!(!blocked(&s, entity(&s, "sessions", "partner").unwrap()));
    let input = json!({"expected_revision":1,"decision":"approve"});
    decide(&mut s, plan["id"].as_str().unwrap(), &input, &None).unwrap();
    assert_eq!(array(&s, "sessions").len(), 3);
    assert_eq!(
        entity(&s, "sessions", "main").unwrap()["focus"],
        "Prove global theorem"
    );
    assert!(!blocked(&s, entity(&s, "sessions", "main").unwrap()));
    assert!(decide(&mut s, plan["id"].as_str().unwrap(), &input, &None).is_err());
}

#[test]
fn adjusted_partner_finishes_before_waiting_and_stop_never_interrupts() {
    let mut s = state();
    let plan = submit(
        &mut s,
        json!([{"type":"stop_partner","partner_session_id":"partner"}]),
    );
    assert_eq!(
        entity(&s, "sessions", "partner").unwrap()["state"],
        "active"
    );
    decide(
        &mut s,
        plan["id"].as_str().unwrap(),
        &json!({"expected_revision":1,"decision":"approve"}),
        &None,
    )
    .unwrap();
    assert_eq!(
        entity(&s, "sessions", "partner").unwrap()["state"],
        "active"
    );
    assert_eq!(
        entity(&s, "sessions", "partner").unwrap()["stop_after_current_task"],
        true
    );
    after_turn(&mut s, "r", "partner").unwrap();
    assert_eq!(
        entity(&s, "sessions", "partner").unwrap()["state"],
        "closed"
    );
}

#[test]
fn paused_deadline_stale_and_new_capacity_block_approval() {
    for barrier in ["paused", "deadline", "epoch", "capacity"] {
        let mut s = state();
        let plan = submit(&mut s, json!([{"type":"request_partner","focus":"Lemma"}]));
        match barrier {
            "paused" => s["runs"][0]["state"] = json!("paused"),
            "deadline" => s["runs"][0]["deadline_at"] = json!("2000-01-01T00:00:00Z"),
            "epoch" => s["runs"][0]["control_epoch"] = json!(2),
            _ => s["runs"][0]["limits"]["max_partners"] = json!(1),
        }
        assert!(
            decide(
                &mut s,
                plan["id"].as_str().unwrap(),
                &json!({"expected_revision":1,"decision":"approve"}),
                &None
            )
            .is_err()
        );
        assert_eq!(array(&s, "sessions").len(), 2);
    }
}

#[test]
fn rejected_and_deferred_plans_have_distinct_dispatch_effects() {
    let mut s = state();
    let plan = submit(&mut s, json!([{"type":"set_focus","focus":"New"}]));
    after_turn(&mut s, "r", "main").unwrap();
    let deferred = decide(
        &mut s,
        plan["id"].as_str().unwrap(),
        &json!({"expected_revision":1,"decision":"defer","reason":"Need time"}),
        &None,
    )
    .unwrap();
    assert!(blocked(&s, entity(&s, "sessions", "main").unwrap()));
    decide(&mut s,plan["id"].as_str().unwrap(),&json!({"expected_revision":deferred["revision"],"decision":"reject","reason":"Try another route"}),&None).unwrap();
    assert_eq!(
        entity(&s, "sessions", "main").unwrap()["phase"],
        "coordination"
    );
    assert!(!blocked(&s, entity(&s, "sessions", "main").unwrap()));
}

#[test]
fn framing_never_redefines_authoritative_problem_and_records_provenance() {
    let mut s = state();
    let original = s["problem"].clone();
    super::super::problem24::apply_action(&mut s,"r","main",&json!({"type":"frame_problem","expected_revision":1,"math_statement":"For all n, P(n)","research_description":"Use at most five partners"}),&json!({"id":"output"})).unwrap();
    assert_eq!(s["problem"], original);
    assert_eq!(s["problem_version"], 1);
    assert_eq!(s["problem_spec"]["original_input"], original);
    assert_eq!(s["problem_spec"]["revision"], 2);
    assert_eq!(s["problem_spec"]["source_artifact_id"], "output");
}

#[test]
fn actual_negative_reviews_and_runtime_errors_do_not_conflate_math_failure() {
    let mut s = state();
    s["candidates"] = json!([{"id":"c","claim":"P(n)","status":"submitted","run_id":"r","author_session_id":"main","problem_version":1}]);
    s["reviews"] = json!([{"id":"review","candidate_id":"c","state":"completed","verdict":"inconclusive","issues":[{"location":"Step 2","issue":"Missing n=0 case"}]}]);
    s["sessions"][1]["last_error"] = json!({"code":"PROCESS_FAILED","message":"CLI stopped"});
    super::super::problem24::project(&mut s);
    assert_eq!(s["open_questions"][0]["statement"], "P(n)");
    assert_eq!(
        s["open_questions"][0]["issues"][0]["issue"],
        "Missing n=0 case"
    );
    assert_eq!(s["route_outcomes"][0]["outcome"], "runtime_failure");
}

#[tokio::test]
async fn description_edit_is_idempotent_and_does_not_change_problem_or_facts() {
    let temp = tempfile::tempdir().unwrap();
    let store = super::super::V2Store::connect(
        &temp.path().join("state.sqlite"),
        &temp.path().join("data"),
    )
    .await
    .unwrap();
    let project = store
        .create_project(json!({"title":"Test","problem":"Prove P"}), "create")
        .await
        .unwrap();
    let service = V2Service::new(store, super::super::V2Config::default());
    let pid = project["id"].as_str().unwrap();
    let input =
        json!({"expected_revision":1,"research_description":"Write all assumptions explicitly"});
    let first = service
        .update_problem_spec24(pid, &input, "edit")
        .await
        .unwrap();
    assert_eq!(first["revision"], 2);
    assert_eq!(first["normalization_state"], "pending");
    assert_eq!(
        service
            .update_problem_spec24(pid, &input, "edit")
            .await
            .unwrap(),
        first
    );
    assert!(
        service
            .update_problem_spec24(pid, &input, "other-edit")
            .await
            .is_err()
    );
    let state = service.store.read(pid).await.unwrap();
    assert_eq!(state["problem_version"], 1);
    assert_eq!(state["problem"], "Prove P");
}

#[test]
fn aliases_cannot_bypass_approval_or_immediately_interrupt_partner() {
    let mut s = state();
    let plan = submit(
        &mut s,
        json!([{"action":"stop_partner","partner_session_id":"partner"}]),
    );
    assert_eq!(plan["actions"].as_array().unwrap().len(), 1);
    assert_eq!(
        entity(&s, "sessions", "partner").unwrap()["state"],
        "active"
    );
    assert!(entity(&s, "sessions", "partner").unwrap()["stop_after_current_task"].is_null());
    invalidate(&mut s, "r", "fixture switch");
    execute_organization(
        &mut s,
        "r",
        "main",
        &json!({"action":"stop_partner","partner_session_id":"partner"}),
        &None,
    )
    .unwrap();
    assert_eq!(
        entity(&s, "sessions", "partner").unwrap()["state"],
        "active"
    );
    assert_eq!(
        entity(&s, "sessions", "partner").unwrap()["stop_after_current_task"],
        true
    );
}

#[test]
fn intake_mode_is_persisted_and_replay_uses_the_original_batch_boundary() {
    let mut s = state();
    s["runs"][0]["mode"] = json!("delegated");
    let control = json!({"actions":[{"action":"assign_partner","partner_session_id":"partner","focus":"Next task"}]});
    let artifact = json!({"id":"auto-output"});
    assert!(!stage(&mut s, "r", "main", &control, &artifact, &None).unwrap());
    s["runs"][0]["mode"] = json!("collaborative");
    assert!(!stage(&mut s, "r", "main", &control, &artifact, &None).unwrap());
    assert_eq!(s["planning_batches"][0]["mode"], "delegated");
    assert!(pending(&s, "r").is_none());
    assert!(
        stage(
            &mut s,
            "r",
            "main",
            &control,
            &json!({"id":"next-output"}),
            &None
        )
        .unwrap()
    );
    assert_eq!(s["planning_batches"][1]["mode"], "collaborative");
}

#[tokio::test]
async fn failed_second_action_rolls_back_entire_approved_batch() {
    let temp = tempfile::tempdir().unwrap();
    let store = super::super::V2Store::connect(
        &temp.path().join("state.sqlite"),
        &temp.path().join("data"),
    )
    .await
    .unwrap();
    let project = store
        .create_project(json!({"problem":"Prove P"}), "create")
        .await
        .unwrap();
    let pid = project["id"].as_str().unwrap();
    let service = V2Service::new(store, super::super::V2Config::default());
    let plan=service.store.mutate(pid,"fixture",None,|p|{let source=state();for k in ["runs","sessions","tasks"]{p[k]=source[k].clone();}let plan=submit(p,json!([{"type":"request_partner","focus":"A"},{"type":"request_partner","focus":"B"}]));p["runs"][0]["limits"]["max_partners"]=json!(2);Ok(plan)}).await.unwrap();
    assert!(
        service
            .planning_decision24(
                pid,
                plan["id"].as_str().unwrap(),
                &json!({"expected_revision":1,"decision":"approve"}),
                "approve"
            )
            .await
            .is_err()
    );
    let after = service.store.read(pid).await.unwrap();
    assert_eq!(array(&after, "sessions").len(), 2);
    assert_eq!(after["planning_proposals"][0]["state"], "pending");
    assert!(
        array(&after, "action_receipts")
            .iter()
            .all(|r| r["status"] == "pending_approval")
    );
}

#[tokio::test]
async fn human_math_edit_keeps_pause_and_old_input_and_rejects_expired_preview() {
    let temp = tempfile::tempdir().unwrap();
    let store = super::super::V2Store::connect(
        &temp.path().join("state.sqlite"),
        &temp.path().join("data"),
    )
    .await
    .unwrap();
    let project = store
        .create_project(json!({"problem":"Original P"}), "create")
        .await
        .unwrap();
    let pid = project["id"].as_str().unwrap();
    let service = V2Service::new(store, super::super::V2Config::default());
    service.store.mutate(pid,"fixture",None,|p|{p["runs"]=json!([{"id":"r","state":"paused","problem_version":1,"control_epoch":1,"revision":1,"deadline_at":null,"limits":{}}]);Ok(Value::Null)}).await.unwrap();
    let spec = service
        .update_problem_spec24(
            pid,
            &json!({"expected_revision":1,"math_statement":"New Q","source":"conversation_edit"}),
            "math-edit",
        )
        .await
        .unwrap();
    assert_eq!(spec["original_input"], "Original P");
    assert_eq!(spec["math_statement"], "New Q");
    assert_eq!(spec["problem_version"], 2);
    let before = service.store.read(pid).await.unwrap();
    assert_eq!(before["runs"][0]["state"], "paused");
    service
        .update_problem_spec24(
            pid,
            &json!({"expected_revision":2,"research_description":"A later description edit"}),
            "later-description",
        )
        .await
        .unwrap();
    let replay = service
        .update_problem_spec24(
            pid,
            &json!({"expected_revision":1,"math_statement":"New Q","source":"conversation_edit"}),
            "math-edit",
        )
        .await
        .unwrap();
    assert_eq!(replay, spec);
    // Simulate a recovered receipt missing the shortcut snapshot: the completed
    // command itself still owns the exact result of this edit.
    service
        .store
        .mutate(pid, "fixture-recovery", None, |p| {
            for op in p["whiteboard_operations"].as_array_mut().unwrap() {
                if op["key"] == "math-edit" {
                    op["result"].as_object_mut().unwrap().remove("problem_spec");
                }
            }
            Ok(Value::Null)
        })
        .await
        .unwrap();
    assert_eq!(service.update_problem_spec24(pid,&json!({"expected_revision":1,"math_statement":"New Q","source":"conversation_edit"}),"math-edit").await.unwrap(),spec);
    let command=service.store.mutate(pid,"fixture-expired",None,|p|{let mut preview=p["previews"][0].clone();preview["id"]=json!("expired");preview["expected_spec_revision"]=json!(3);preview["base_problem_version"]=json!(2);preview["base_control_epoch"]=p["runs"][0]["control_epoch"].clone();preview["expires_at"]=json!("2000-01-01T00:00:00Z");push(p,"previews",preview);let command=json!({"id":"expired-command","run_id":"r","type":"replace_problem","status":"queued","payload":{"confirmed_impact_preview_id":"expired"}});push(p,"commands",command.clone());Ok(command)}).await.unwrap();
    assert_eq!(
        service.apply_control(pid, &command).await.unwrap_err().code,
        "REVISION_CONFLICT"
    );
    let after = service.store.read(pid).await.unwrap();
    assert_eq!(after["problem"], "New Q");
    assert_eq!(after["runs"][0]["state"], "paused");
}
