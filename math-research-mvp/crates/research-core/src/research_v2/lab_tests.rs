use super::*;

const START: &str = "2026-09-07T00:00:00+00:00";

fn state() -> Value {
    json!({"id":"project","revision":1,"problem_version":1,"workspace_path":"lab-test","runs":[{"id":"r","state":"running","mode":"collaborative","started_at":START,"deadline_at":null,"control_epoch":1,"limits":{"max_invocations":100},"revision":1}],"sessions":[{"id":"main","run_id":"r","role":"main","state":"idle","control_epoch":1,"revision":1},{"id":"partner","run_id":"r","role":"partner","state":"idle","control_epoch":1,"revision":1}],"tasks":[{"id":"old-task","run_id":"r","owner_session_id":"partner","state":"queued","task_epoch":1,"focus":"old target","revision":1}],"usage":[],"facts":[]})
}

fn checkpoint() -> Value {
    json!({"proof_goal":"prove lemma A","local_assumptions":["n >= 2"],"symbols":[{"name":"n","scope":"lemma A","domain":"integer"}],"unfinished_steps":["bound remainder"],"next_step":"inspect inequality B","draft_refs":[{"artifact_id":"draft-v1","version":1}]})
}

#[test]
fn submission_recovery_wait_requires_queue_and_wakes_when_work_finishes() {
    let mut s = state();
    s["sessions"] = json!([s["sessions"][0]]);
    s["tasks"] = json!([]);
    prepare_turn_at(&mut s, "r", "main", START).unwrap();
    let control =
        json!({"actions":[],"phase_result":{"next_phase":"waiting","reason":"review is pending"}});
    assert_eq!(
        after_turn_at(&mut s, "r", "main", &control, &json!({"id":"out"}), START)
            .unwrap_err()
            .code,
        "LAB_WAIT_UNAVAILABLE"
    );
    s["reviews"] = json!([{"id":"review","run_id":"r","state":"running"}]);
    after_turn_at(&mut s, "r", "main", &control, &json!({"id":"out"}), START).unwrap();
    assert_eq!(s["sessions"][0]["state"], "waiting");
    assert_eq!(s["sessions"][0]["host_wait_targets"][0]["id"], "review");
    s["reviews"][0]["state"] = json!("completed");
    tick_at(&mut s, "r", &None, START).unwrap();
    assert_eq!(s["sessions"][0]["state"], "idle");
    assert_eq!(s["sessions"][0]["phase"], "coordination");
}

#[test]
fn closed_partner_reassignment_cannot_create_a_sixth_slot() {
    let mut state = state();
    state["runs"][0]["limits"]["max_partners"] = json!(5);
    state["sessions"][0]["phase"] = json!("coordination");
    for n in 0..4 {
        push(
            &mut state,
            "sessions",
            json!({"id":format!("active-{n}"),"run_id":"r","role":"partner","state":"waiting"}),
        );
    }
    push(
        &mut state,
        "sessions",
        json!({"id":"closed","run_id":"r","role":"partner","state":"closed","revision":1}),
    );
    let action = json!({"type":"assign_partner","partner_session_id":"closed","focus":"New independent target"});
    assert_eq!(
        apply_action_at(&mut state, "r", "main", &action, &Value::Null, &None, START)
            .unwrap_err()
            .code,
        "PARTNER_LIMIT"
    );
    assert_eq!(
        entity(&state, "sessions", "closed").unwrap()["state"],
        "closed"
    );
    assert!(entity(&state, "sessions", "closed").unwrap()["pending_assignment_id"].is_null());
    let continuing = json!({"type":"assign_partner","partner_session_id":"partner","focus":"Changed open target"});
    apply_action_at(
        &mut state,
        "r",
        "main",
        &continuing,
        &Value::Null,
        &None,
        START,
    )
    .unwrap();
    entity_mut(&mut state, "sessions", "active-0").unwrap()["state"] = json!("closed");
    apply_action_at(&mut state, "r", "main", &action, &Value::Null, &None, START).unwrap();
    assert_eq!(
        entity(&state, "sessions", "closed").unwrap()["state"],
        "idle"
    );
    assert_eq!(
        array(&state, "sessions")
            .iter()
            .filter(|s| s["role"] == "partner" && s["state"] != "closed")
            .count(),
        5
    );
}

#[test]
fn limits_are_independent_of_run_duration_and_validated() {
    let mut limits = json!({"duration_seconds":86_400,"max_invocations":750});
    normalize_limits(&mut limits).unwrap();
    assert_eq!(limits["research_soft_seconds"], 1200);
    assert_eq!(limits["research_hard_seconds"], 1800);
    assert_eq!(limits["coordination_soft_seconds"], 120);
    assert_eq!(limits["coordination_hard_seconds"], 300);
    assert_eq!(limits["duration_seconds"], 86_400);
    assert_eq!(limits["max_invocations"], 750);
    limits["research_soft_seconds"] = json!(1801);
    assert!(normalize_limits(&mut limits).is_err());
}

#[test]
fn main_phase_clock_survives_natural_turns_and_enforces_soft_boundary() {
    let mut state = state();
    prepare_turn_at(&mut state, "r", "main", START).unwrap();
    let control = json!({"checkpoint":checkpoint(),"phase_result":{"next_phase":"research"}});
    after_turn_at(
        &mut state,
        "r",
        "main",
        &control,
        &json!({"id":"out1"}),
        "2026-09-07T00:10:00+00:00",
    )
    .unwrap();
    assert_eq!(state["sessions"][0]["phase_started_at"], START);
    assert_eq!(
        turn_timeout_seconds(
            &state["runs"][0],
            &state["sessions"][0],
            2700,
            "2026-09-07T00:25:00+00:00"
        ),
        300
    );
    after_turn_at(
        &mut state,
        "r",
        "main",
        &control,
        &json!({"id":"out2"}),
        "2026-09-07T00:21:00+00:00",
    )
    .unwrap();
    assert_eq!(state["sessions"][0]["phase"], "coordination");
    assert_eq!(array(&state, "proof_checkpoints").len(), 2);
    let saved = &state["proof_checkpoints"][1];
    assert_eq!(
        saved["local_assumptions"],
        checkpoint()["local_assumptions"]
    );
    assert_eq!(saved["symbols"], checkpoint()["symbols"]);
    assert_eq!(saved["requires_recheck"], true);
}

#[test]
fn ordinary_reassignment_preserves_current_capture_and_wins_next_dispatch() {
    let mut state = state();
    let capture = prepare_turn_at(&mut state, "r", "partner", START).unwrap();
    state["sessions"][1]["state"] = json!("active");
    state["sessions"][1]["invocation_in_flight"] = json!(true);
    let result = apply_action_at(
        &mut state,
        "r",
        "main",
        &json!({"type":"assign_partner","partner_session_id":"partner","focus":"new open target"}),
        &Value::Null,
        &None,
        START,
    )
    .unwrap()
    .unwrap();
    assert!(array(&result, "cancel_session_ids").is_empty());
    assert_eq!(state["sessions"][1]["assignment_epoch"], 1);
    assert!(execution_valid(&state, "partner", &capture));
    after_turn_at(
        &mut state,
        "r",
        "partner",
        &json!({"checkpoint":checkpoint(),"partner_continuation":{"decision":"continue"}}),
        &json!({"id":"old-result"}),
        START,
    )
    .unwrap();
    assert_eq!(state["proof_checkpoints"][0]["task_id"], "old-task");
    state["sessions"][1]["invocation_in_flight"] = json!(false);
    let next = prepare_turn_at(&mut state, "r", "partner", START).unwrap();
    assert_eq!(state["sessions"][1]["focus"], "new open target");
    assert_eq!(next["assignment_epoch"], 2);
    assert_eq!(state["tasks"][0]["state"], "superseded");
    assert!(!execution_valid(&state, "partner", &capture));
    assert!(execution_valid(&state, "partner", &next));
    let current_task = state["sessions"][1]["task_id"].clone();
    prepare_turn_at(&mut state, "r", "partner", START).unwrap();
    assert_eq!(state["sessions"][1]["task_id"], current_task);
    assert_eq!(array(&state, "tasks").len(), 2);
}

#[test]
fn pending_update_and_withdraw_survive_aggregate_reload() {
    let mut state = state();
    prepare_turn_at(&mut state, "r", "partner", START).unwrap();
    for focus in ["first", "latest"] {
        apply_action_at(
            &mut state,
            "r",
            "main",
            &json!({"type":"assign_partner","partner_session_id":"partner","focus":focus}),
            &Value::Null,
            &None,
            START,
        )
        .unwrap();
    }
    let mut restored: Value = serde_json::from_str(&state.to_string()).unwrap();
    prepare_turn_at(&mut restored, "r", "partner", START).unwrap();
    assert_eq!(restored["sessions"][1]["focus"], "latest");
    assert_eq!(restored["pending_assignments"][0]["state"], "superseded");
    apply_action_at(
        &mut state,
        "r",
        "main",
        &json!({"type":"withdraw_assignment","partner_session_id":"partner"}),
        &Value::Null,
        &None,
        START,
    )
    .unwrap();
    prepare_turn_at(&mut state, "r", "partner", START).unwrap();
    assert_eq!(state["sessions"][1]["task_id"], "old-task");
    assert_eq!(state["sessions"][1]["assignment_epoch"], 1);
}

#[test]
fn autonomous_partner_and_legacy_wait_are_distinct() {
    let mut state = state();
    prepare_turn_at(&mut state, "r", "partner", START).unwrap();
    state["sessions"][1]["state"] = json!("waiting");
    after_turn_at(&mut state, "r", "partner", &json!({}), &Value::Null, START).unwrap();
    assert_eq!(state["sessions"][1]["state"], "waiting");
    assert_eq!(state["sessions"][1]["protocol_mode"], "legacy_2_1");
    after_turn_at(&mut state,"r","partner",&json!({"checkpoint":checkpoint(),"partner_continuation":{"decision":"continue","reason":"next lemma"}}),&Value::Null,START).unwrap();
    assert_eq!(state["sessions"][1]["state"], "idle");
    assert_eq!(state["sessions"][1]["protocol_mode"], "lab_2_2");
}

#[test]
fn route_bans_inherit_and_human_reopen_does_not_resume_cancelled_work() {
    let mut state = state();
    prepare_turn_at(&mut state, "r", "partner", START).unwrap();
    let parent = state["sessions"][0]["route_id"]
        .as_str()
        .unwrap()
        .to_owned();
    apply_action_at(&mut state,"r","partner",&json!({"type":"register_route","route_id":"child","title":"child method","parent_route_ids":[parent]}),&Value::Null,&None,START).unwrap();
    apply_action_at(
        &mut state,
        "r",
        "partner",
        &json!({"type":"set_route","route_id":"child"}),
        &Value::Null,
        &None,
        START,
    )
    .unwrap();
    let capture = prepare_turn_at(&mut state, "r", "partner", START).unwrap();
    let result = prohibit_route_at(&mut state, "r", &parent, "human forbids", START).unwrap();
    assert!(array(&result, "cancel_session_ids").contains(&json!("partner")));
    assert!(route_blocked(&state, "child"));
    assert!(!execution_valid(&state, "partner", &capture));
    assert_eq!(state["tasks"][0]["state"], "cancelled");
    assert!(
        apply_action_at(
            &mut state,
            "r",
            "partner",
            &json!({"type":"register_route","title":"renamed"}),
            &Value::Null,
            &None,
            START
        )
        .is_err()
    );
    reopen_route_at(&mut state, "r", &parent, "human restores", START).unwrap();
    assert!(!route_blocked(&state, "child"));
    assert_eq!(state["sessions"][1]["state"], "waiting");
}

#[test]
fn advisor_is_a_real_session_and_period_does_not_reset_on_unrelated_tick() {
    let mut state = state();
    tick_at(&mut state, "r", &None, START).unwrap();
    assert_eq!(
        state["runs"][0]["advisor_next_due_at"],
        "2026-09-07T01:00:00+00:00"
    );
    tick_at(&mut state, "r", &None, "2026-09-07T00:10:00+00:00").unwrap();
    assert_eq!(
        state["runs"][0]["advisor_next_due_at"],
        "2026-09-07T01:00:00+00:00"
    );
    tick_at(
        &mut state,
        "r",
        &Some("configured-model".into()),
        "2026-09-07T01:00:00+00:00",
    )
    .unwrap();
    assert_eq!(array(&state, "background_jobs").len(), 1);
    assert_eq!(state["background_jobs"][0]["state"], "queued");
    assert!(array(&state, "advisories").is_empty());
    let sid = state["background_jobs"][0]["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        entity(&state, "sessions", &sid).unwrap()["model"],
        "configured-model"
    );
    prepare_turn_at(&mut state, "r", &sid, "2026-09-07T01:01:00+00:00").unwrap();
    let response = json!({"background_result":{"observation":"No new result observed","evidence_refs":[],"uncertainty":"Available observation is limited","priority":"urgent"}});
    after_turn_at(
        &mut state,
        "r",
        &sid,
        &response,
        &json!({"id":"real-model-output"}),
        "2026-09-07T01:02:00+00:00",
    )
    .unwrap();
    assert_eq!(state["background_jobs"][0]["state"], "completed");
    assert_eq!(state["advisories"][0]["artifact_id"], "real-model-output");
    assert_eq!(state["messages"][0]["trust"], "unreviewed_advice");
    assert!(array(&state, "facts").is_empty());
}

#[test]
fn paused_runs_do_not_schedule_paid_background_jobs() {
    let mut state = state();
    tick_at(&mut state, "r", &None, START).unwrap();
    state["runs"][0]["state"] = json!("paused");
    tick_at(&mut state, "r", &None, "2026-09-07T03:00:00+00:00").unwrap();
    assert!(array(&state, "background_jobs").is_empty());
    state["runs"][0]["state"] = json!("running");
    tick_at(&mut state, "r", &None, "2026-09-07T03:01:00+00:00").unwrap();
    assert_eq!(array(&state, "background_jobs").len(), 1);
    let before = state.clone();
    assert_eq!(
        tick_at(&mut state, "r", &None, "2026-09-07T03:01:01+00:00").unwrap()["changed"],
        false
    );
    assert_eq!(state, before);
}

#[test]
fn memory_preserves_sources_and_background_roles_cannot_plan_or_certify() {
    let mut state = state();
    prepare_turn_at(&mut state, "r", "main", START).unwrap();
    after_turn_at(
        &mut state,
        "r",
        "main",
        &json!({"checkpoint":checkpoint(),"phase_result":{"next_phase":"coordination"}}),
        &json!({"id":"output"}),
        START,
    )
    .unwrap();
    tick_at(&mut state, "r", &None, START).unwrap();
    tick_at(&mut state, "r", &None, "2026-09-07T00:05:00+00:00").unwrap();
    let job = array(&state, "background_jobs")
        .iter()
        .find(|job| job["kind"] == "memory")
        .unwrap()
        .clone();
    let sid = job["session_id"].as_str().unwrap();
    assert!(
        apply_action_at(
            &mut state,
            "r",
            sid,
            &json!({"type":"submit_candidate"}),
            &Value::Null,
            &None,
            START
        )
        .is_err()
    );
    let source = state["proof_checkpoints"][0]["id"].clone();
    let response = json!({"background_result":{"entries":[{"summary":"We need to bound a remainder","source_refs":[source],"assurance":"formal_verified"}]}});
    after_turn_at(
        &mut state,
        "r",
        sid,
        &response,
        &json!({"id":"memory-model-output"}),
        "2026-09-07T00:06:00+00:00",
    )
    .unwrap();
    assert_eq!(state["memory_entries"][0]["trust"], "summary_only");
    assert_eq!(
        state["memory_entries"][0]["exact_sources"][0]["source"]["local_assumptions"],
        checkpoint()["local_assumptions"]
    );
    assert!(array(&state, "facts").is_empty());
    let invalid = json!({"background_result":{"title":"Invented","body":"invented","source_refs":["unknown-source"]}});
    let display = array(&state, "background_jobs")
        .iter()
        .find(|job| job["kind"] == "display")
        .unwrap()["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(after_turn_at(&mut state, "r", &display, &invalid, &Value::Null, START).is_err());
    assert!(array(&state, "display_summaries").is_empty());
}

#[test]
fn urgent_phase_never_changes_while_invocation_is_in_flight() {
    let mut state = state();
    prepare_turn_at(&mut state, "r", "main", START).unwrap();
    state["sessions"][0]["pending_urgent"] = json!(true);
    state["sessions"][0]["invocation_in_flight"] = json!(true);
    tick_at(&mut state, "r", &None, "2026-09-07T00:01:00+00:00").unwrap();
    assert_eq!(state["sessions"][0]["phase"], "research");
    fail_turn_at(&mut state, "main", "CANCELLED", "2026-09-07T00:01:01+00:00").unwrap();
    assert_eq!(state["sessions"][0]["phase"], "urgent_attention");
    assert_eq!(state["sessions"][0]["state"], "idle");
}

#[test]
fn empty_protocol_recovery_is_bounded_and_never_claims_progress() {
    let mut state = state();
    prepare_turn_at(&mut state, "r", "main", START).unwrap();
    for _ in 0..3 {
        after_turn_at(
            &mut state,
            "r",
            "main",
            &json!({}),
            &json!({"id":"raw-output"}),
            START,
        )
        .unwrap();
    }
    assert_eq!(state["sessions"][0]["phase"], "waiting");
    assert_eq!(state["sessions"][0]["state"], "waiting");
    assert_eq!(
        state["sessions"][0]["last_error"]["code"],
        "LAB_PROTOCOL_STALLED"
    );
    assert!(array(&state, "proof_checkpoints").is_empty());
    assert!(array(&state, "facts").is_empty());
}

#[test]
fn repeat_warning_does_not_stop_valid_research() {
    let mut state = state();
    prepare_turn_at(&mut state, "r", "main", START).unwrap();
    let control = json!({"checkpoint":checkpoint(),"phase_result":{"next_phase":"research"},"progress":{"obstacle":"need bound","evidence_refs":[],"method":"induction","assumptions":["n>=2"]}});
    for _ in 0..3 {
        after_turn_at(&mut state, "r", "main", &control, &Value::Null, START).unwrap();
    }
    assert!(state["sessions"][0]["progress_warning"].is_string());
    assert_eq!(state["sessions"][0]["phase"], "research");
    assert_eq!(state["sessions"][0]["state"], "idle");
}

#[test]
fn normal_messages_wait_for_natural_boundary_and_ack_only_captured_ids() {
    let mut state = state();
    prepare_turn_at(&mut state, "r", "main", START).unwrap();
    state["sessions"][0]["invocation_in_flight"] = json!(true);
    push(
        &mut state,
        "messages",
        json!({"id":"m1","run_id":"r","target_role":"main","priority":"normal","status":"queued","body":"try another method"}),
    );
    tick_at(&mut state, "r", &None, START).unwrap();
    assert_eq!(state["sessions"][0]["phase"], "research");
    state["sessions"][0]["invocation_in_flight"] = json!(false);
    let prepared = prepare_turn_at(&mut state, "r", "main", START).unwrap();
    assert_eq!(prepared["phase"], "coordination");
    assert_eq!(prepared["message_ids"], json!(["m1"]));
    push(
        &mut state,
        "messages",
        json!({"id":"m2","run_id":"r","target_role":"main","priority":"normal","state":"queued"}),
    );
    after_turn_at(&mut state,"r","main",&json!({"phase_result":{"next_phase":"research"},"message_receipts":[{"message_id":"m1","disposition":"deferred","reason":"finish current lemma"}]}),&Value::Null,START).unwrap();
    assert_eq!(state["messages"][0]["receipt"]["disposition"], "deferred");
    assert_eq!(state["messages"][0]["state"], "handled");
    assert_eq!(state["messages"][1]["state"], "queued");
}

#[test]
fn urgent_feedback_requests_one_cancellation_and_is_reliable_on_failure() {
    let mut state = state();
    let capture = prepare_turn_at(&mut state, "r", "main", START).unwrap();
    state["sessions"][0]["invocation_in_flight"] = json!(true);
    // Output has landed, but actions/checkpoint processing still owns this invocation.
    state["sessions"][0]["state"] = json!("idle");
    push(
        &mut state,
        "messages",
        json!({"id":"urgent","run_id":"r","target_role":"main","priority":"urgent","state":"queued"}),
    );
    let first = tick_at(&mut state, "r", &None, START).unwrap();
    assert_eq!(first["cancel_session_ids"], json!(["main"]));
    assert!(!execution_valid(&state, "main", &capture));
    assert!(
        array(
            &tick_at(&mut state, "r", &None, START).unwrap(),
            "cancel_session_ids"
        )
        .is_empty()
    );
    fail_turn_at(&mut state, "main", "CANCELLED", START).unwrap();
    state["sessions"][0]["invocation_in_flight"] = json!(false);
    let urgent = prepare_turn_at(&mut state, "r", "main", START).unwrap();
    assert_eq!(urgent["phase"], "urgent_attention");
    assert_eq!(urgent["message_ids"], json!(["urgent"]));
    fail_turn_at(&mut state, "main", "TEMPORARY_ERROR", START).unwrap();
    assert_eq!(state["messages"][0]["state"], "queued");
    prepare_turn_at(&mut state, "r", "main", START).unwrap();
    after_turn_at(
        &mut state,
        "r",
        "main",
        &json!({"phase_result":{"next_phase":"research"}}),
        &Value::Null,
        START,
    )
    .unwrap();
    assert_eq!(state["messages"][0]["state"], "handled");
    assert_eq!(state["sessions"][0]["pending_urgent"], false);
    tick_at(&mut state, "r", &None, START).unwrap();
    assert_eq!(state["sessions"][0]["phase"], "research");
}

#[test]
fn hard_timeout_does_not_repeat_one_second_arrangement_calls() {
    let mut state = state();
    prepare_turn_at(&mut state, "r", "main", START).unwrap();
    fail_turn_at(
        &mut state,
        "main",
        "DEADLINE_REACHED",
        "2026-09-07T00:30:00+00:00",
    )
    .unwrap();
    assert_eq!(state["sessions"][0]["phase"], "coordination");
    fail_turn_at(
        &mut state,
        "main",
        "DEADLINE_REACHED",
        "2026-09-07T00:35:00+00:00",
    )
    .unwrap();
    assert_eq!(state["sessions"][0]["phase"], "waiting");
    assert_eq!(state["sessions"][0]["state"], "waiting");
}

#[test]
fn frozen_review_status_change_triggers_fresh_background_summary() {
    let mut state = state();
    state["reviews"] = json!([{"id":"review","run_id":"r","state":"queued","revision":1}]);
    tick_at(&mut state, "r", &None, START).unwrap();
    tick_at(&mut state, "r", &None, "2026-09-07T00:00:30+00:00").unwrap();
    let display = state["background_jobs"][0]["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    after_turn_at(&mut state,"r",&display,&json!({"background_result":{"title":"Review pending","body":"Waiting for review","source_refs":["review"]}}),&Value::Null,"2026-09-07T00:00:31+00:00").unwrap();
    state["reviews"][0]["state"] = json!("completed");
    state["reviews"][0]["revision"] = json!(2);
    tick_at(&mut state, "r", &None, "2026-09-07T00:00:32+00:00").unwrap();
    tick_at(&mut state, "r", &None, "2026-09-07T00:01:02+00:00").unwrap();
    assert_eq!(
        array(&state, "background_jobs")
            .iter()
            .filter(|job| job["kind"] == "display")
            .count(),
        2
    );
}

#[test]
fn checkpoint_saved_before_actions_is_idempotent_in_after_turn() {
    let mut state = state();
    prepare_turn_at(&mut state, "r", "main", START).unwrap();
    let artifact = json!({"id":"actual-output"});
    let first = checkpoint_at(&mut state, "main", &checkpoint(), &artifact, START).unwrap();
    after_turn_at(
        &mut state,
        "r",
        "main",
        &json!({"checkpoint":checkpoint(),"phase_result":{"next_phase":"coordination"}}),
        &artifact,
        START,
    )
    .unwrap();
    assert_eq!(array(&state, "proof_checkpoints").len(), 1);
    assert_eq!(
        first,
        checkpoint_at(&mut state, "main", &checkpoint(), &artifact, START).unwrap()
    );
    let mut incompatible = checkpoint();
    incompatible["local_assumptions"] = json!(["n>=100"]);
    assert!(checkpoint_at(&mut state, "main", &incompatible, &artifact, START).is_err());
}

#[test]
fn verification_only_boolean_disables_background_model_calls() {
    let mut state = state();
    state["runs"][0]["verification_only"] = json!(true);
    tick_at(&mut state, "r", &None, START).unwrap();
    tick_at(&mut state, "r", &None, "2026-09-07T03:00:00+00:00").unwrap();
    assert!(array(&state, "background_jobs").is_empty());
    assert!(
        array(&state, "sessions")
            .iter()
            .all(|session| session["role"] != "advisor")
    );
}

#[test]
fn memory_exact_sources_keep_frozen_node_and_artifact_versions() {
    let mut state = state();
    state["nodes"] = json!([{"id":"node","author":"main","body":"original statement under assumption A","revision":1,"sha256":"node-v1-hash","body_artifact_id":"source-artifact"}]);
    state["artifacts"] = json!([{"id":"source-artifact","revision":1,"sha256":"artifact-v1-hash"}]);
    let run = state["runs"][0].clone();
    assert!(background_session(
        &mut state,
        &run,
        "memory",
        &None,
        &["node".into(), "source-artifact".into()],
        "source_events",
        START,
    ));
    let session_id = state["background_jobs"][0]["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    state["nodes"][0]["body"] = json!("different statement under assumption B");
    state["nodes"][0]["revision"] = json!(2);
    state["nodes"][0]["sha256"] = json!("node-v2-hash");
    state["artifacts"][0]["revision"] = json!(2);
    state["artifacts"][0]["sha256"] = json!("artifact-v2-hash");
    after_turn_at(
        &mut state,
        "r",
        &session_id,
        &json!({"background_result":{"entries":[{"summary":"The source assumes A","source_refs":["node","source-artifact"]}]}}),
        &json!({"id":"actual-memory-output"}),
        START,
    ).unwrap();
    let exact = &state["memory_entries"][0]["exact_sources"];
    assert_eq!(
        exact[0]["source"]["body"],
        "original statement under assumption A"
    );
    assert_eq!(exact[0]["source_revision"], 1);
    assert_eq!(exact[0]["source_sha256"], "node-v1-hash");
    assert_eq!(exact[1]["collection"], "artifacts");
    assert_eq!(exact[1]["source_revision"], 1);
    assert_eq!(exact[1]["source_sha256"], "artifact-v1-hash");
    assert_eq!(state["nodes"][0]["revision"], 2);
}
