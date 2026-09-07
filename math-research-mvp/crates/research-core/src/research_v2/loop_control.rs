//! Small deterministic loop rules. None of these functions makes research decisions.
use super::{V2Result, Value, array, entity, entity_mut, err, id, json, now, push, revision};

pub(super) fn record_error(
    state: &mut Value,
    session_id: &str,
    code: &str,
    message: &str,
) -> V2Result<()> {
    let session = entity_mut(state, "sessions", session_id)?;
    if !session["error_history"].is_array() {
        session["error_history"] = json!([]);
    }
    // Legacy current errors may predate the history field. Preserve them before
    // replacement; usage/action receipts and event records remain untouched.
    if session["last_error"].is_object() && session["last_error_id"].is_null() {
        let old = json!({"id":id(),"error":session["last_error"],"recorded_at":now(),"status":"superseded","legacy":true});
        session["error_history"].as_array_mut().unwrap().push(old);
    }
    let error_id = id();
    let error = json!({"code":code,"message":message});
    let record = json!({"id":error_id,"error":error,"recorded_at":now(),"status":"current"});
    for previous in session["error_history"].as_array_mut().unwrap().iter_mut() {
        if previous["status"] == "current" {
            previous["status"] = json!("superseded");
        }
    }
    session["error_history"]
        .as_array_mut()
        .unwrap()
        .push(record);
    session["last_error"] = error;
    session["last_error_id"] = json!(error_id);
    revision(session);
    Ok(())
}

/// Only called after all action receipts and phase processing succeed. This
/// resolves an execution error, never a mathematical claim or rejected proof.
pub(super) fn complete_turn(
    state: &mut Value,
    session_id: &str,
    previous_error: &Value,
    artifact: &Value,
) -> V2Result<()> {
    let session = entity_mut(state, "sessions", session_id)?;
    if !previous_error.is_object()
        || session["last_error"] != *previous_error
        || session["empty_control_turns"].as_u64().unwrap_or(0) >= 3
    {
        return Ok(());
    }
    if !session["error_history"].is_array() {
        session["error_history"] = json!([]);
    }
    let error_id = session["last_error_id"].clone();
    let resolution = json!({"resolved_at":now(),"resolution":"later_control_completed","artifact_id":artifact["id"]});
    if let Some(record) = session["error_history"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|r| !error_id.is_null() && r["id"] == error_id)
    {
        record["status"] = json!("resolved");
        record["resolution"] = resolution;
    } else {
        let record = json!({"id":id(),"error":previous_error,"recorded_at":now(),"status":"resolved","resolution":resolution,"legacy":true});
        session["error_history"]
            .as_array_mut()
            .unwrap()
            .push(record);
    }
    session["last_error"] = Value::Null;
    session["last_error_id"] = Value::Null;
    revision(session);
    Ok(())
}

pub(super) fn current_task<'a>(state: &'a Value, session: &Value) -> Option<&'a Value> {
    if let Some(task_id) = session["task_id"].as_str() {
        return entity(state, "tasks", task_id);
    }
    array(state, "tasks")
        .iter()
        .rev()
        .find(|task| task["owner_session_id"] == session["id"] && task["state"] != "superseded")
}

pub(super) fn selected_memories(prepared: &Value) -> Vec<Value> {
    let session = &prepared["session"];
    // Oldest unseen first: a burst of new messages cannot starve earlier feedback.
    array(prepared, "memories")
        .iter()
        .filter(|m| {
            m["session_id"] != session["id"]
                && !array(session, "seen_memory_ids").contains(&m["id"])
        })
        .take(12)
        .cloned()
        .collect()
}

pub(super) fn mark_memories_supplied(session: &mut Value, prepared: &Value) {
    let mut seen = array(session, "seen_memory_ids").to_vec();
    for memory in selected_memories(prepared) {
        if !seen.contains(&memory["id"]) {
            seen.push(memory["id"].clone());
        }
    }
    session["seen_memory_ids"] = json!(seen);
}

pub(super) fn fail_review(state: &mut Value, session_id: &str, code: &str) {
    let mut affected = Vec::new();
    let mut finished_reviews = Vec::new();
    for review in state["reviews"].as_array_mut().into_iter().flatten() {
        if review["reviewer_session_id"] == session_id
            && matches!(review["state"].as_str(), Some("queued" | "running"))
        {
            review["state"] = json!(if code == "CANCELLED" {
                "cancelled"
            } else {
                "failed"
            });
            review["failure_code"] = json!(code);
            review["ended_at"] = json!(now());
            affected.push(review["candidate_id"].clone());
            if let Some(review_id) = review["id"].as_str() {
                finished_reviews.push(review_id.to_owned());
            }
            revision(review);
        }
    }
    for candidate_id in affected {
        let candidate = entity(
            state,
            "candidates",
            candidate_id.as_str().unwrap_or_default(),
        )
        .cloned();
        if let Some(candidate) = candidate {
            let author = candidate["author_session_id"].as_str().unwrap_or_default();
            if let Ok(session) = entity_mut(state, "sessions", author) {
                session["pending_followup"] = json!(true);
                if session["state"] == "waiting" {
                    session["state"] = json!("idle");
                }
            }
            push(
                state,
                "memories",
                json!({"id":id(),"run_id":candidate["run_id"],
                "recipient_session_id":author,"problem_version":candidate["problem_version"],
                "kind":"review_failed","candidate_id":candidate_id,"text":format!("审查调用未完成（{code}），没有数学通过/否定结论。可修复材料后重新提交或请求重审。"),"created_at":now()}),
            );
        }
    }
    for review_id in finished_reviews {
        super::lab::notify_partner_review_at(state, &review_id, &now());
    }
}

/// A malformed control turn receives one corrective turn, then an explicit fault.
pub(super) fn postprocess_failure(
    state: &mut Value,
    session_id: &str,
    code: &str,
    message: &str,
) -> V2Result<Value> {
    let snapshot = entity(state, "sessions", session_id)
        .cloned()
        .ok_or_else(|| err("NOT_FOUND", "会话不存在"))?;
    if entity(
        state,
        "runs",
        snapshot["run_id"].as_str().unwrap_or_default(),
    )
    .is_some_and(|r| matches!(r["state"].as_str(), Some("ended" | "stopping")))
    {
        entity_mut(state, "sessions", session_id)?["invocation_in_flight"] = json!(false);
        return Ok(json!({"fault":false}));
    }
    if snapshot["role"] == "main"
        && snapshot["last_output_artifact_id"].is_string()
        && array(state, "planning_proposals").iter().any(|plan| {
            plan["session_id"] == session_id
                && plan["source_artifact_id"] == snapshot["last_output_artifact_id"]
                && matches!(plan["state"].as_str(), Some("pending" | "deferred"))
        })
    {
        super::whiteboard24::invalidate(
            state,
            snapshot["run_id"].as_str().unwrap_or_default(),
            "source_output_failed",
        );
    }
    if matches!(
        snapshot["role"].as_str(),
        Some("advisor" | "memory" | "display")
    ) {
        // Auxiliary output processing has no authority to stop mathematical work.
        // Its job is already marked failed by fail_turn_at; don't reopen a closed
        // sidecar and retry its stale input as though it were a researcher.
        let session = entity_mut(state, "sessions", session_id)?;
        session["invocation_in_flight"] = json!(false);
        session["state"] = json!("closed");
        session["pending_followup"] = json!(false);
        revision(session);
        record_error(state, session_id, code, message)?;
        return Ok(json!({"fault":true,"main":false,"auxiliary":true,"run_id":snapshot["run_id"]}));
    }
    if code == "PREMISE_MATERIAL_BLOCKED" {
        let session = entity_mut(state, "sessions", session_id)?;
        session["invocation_in_flight"] = json!(false);
        session["state"] = json!("waiting");
        session["phase"] = json!("waiting");
        session["wait_for"] = json!("source_material_repair");
        record_error(state, session_id, code, message)?;
        push(
            state,
            "messages",
            json!({"id":id(),"run_id":snapshot["run_id"],"target_role":"main","recipient_session_id":session_id,"priority":"normal","kind":"material_blocked","state":"queued","body":message,"created_at":now()}),
        );
        if snapshot["role"] == "main" {
            let question = super::collaboration23::register_question(
                state,
                snapshot["run_id"].as_str().unwrap_or_default(),
                session_id,
                &json!({"question":message}),
                &json!({"id":null}),
            )?;
            let question = entity_mut(
                state,
                "human_questions",
                question["id"].as_str().unwrap_or_default(),
            )?;
            question["source_kind"] = json!("system_material_failure");
            question["failure_code"] = json!(code);
            question["source_artifact_id"] = Value::Null;
        }
        return Ok(json!({"fault":false}));
    }
    if matches!(
        code,
        "PREMISE_READ_REQUIRED" | "CONDITIONAL_LIMIT" | "CONDITIONAL_LIMIT_REQUIRED"
    ) {
        record_error(state, session_id, code, message)?;
        let session = entity_mut(state, "sessions", session_id)?;
        session["invocation_in_flight"] = json!(false);
        session["pending_followup"] = json!(true);
        session["state"] = json!(
            if code == "PREMISE_READ_REQUIRED" || snapshot["role"] == "main" {
                "idle"
            } else {
                "waiting"
            }
        );
        if code != "PREMISE_READ_REQUIRED" {
            session["conditional_wait"] = json!(true);
            if snapshot["role"] == "main" {
                session["phase"] = json!("coordination");
                session["phase_started_at"] = json!(now());
            }
        }
        push(
            state,
            "memories",
            json!({"id":id(),"run_id":snapshot["run_id"],"recipient_session_id":session_id,"kind":"premise_input_needed","text":message,"problem_version":snapshot["problem_version"],"created_at":now()}),
        );
        return Ok(json!({"fault":false}));
    }
    fail_review(state, session_id, code);
    let obsolete = matches!(
        code,
        "STALE_CONTROL_EPOCH" | "INVALID_STATE" | "BUDGET_LIMIT"
    );
    let session = entity_mut(state, "sessions", session_id)?;
    session["invocation_in_flight"] = json!(false);
    if obsolete {
        if session["state"] == "active" {
            session["state"] = json!("idle");
        }
        return Ok(json!({"fault":false}));
    }
    let failures = session["control_failures"].as_u64().unwrap_or(0) + 1;
    let fault = failures > 1 || snapshot["role"] == "reviewer";
    session["control_failures"] = json!(failures);
    session["state"] = json!(if fault { "faulted" } else { "idle" });
    session["pending_followup"] = json!(!fault);
    revision(session);
    record_error(state, session_id, code, message)?;
    push(
        state,
        "memories",
        json!({"id":id(),"run_id":snapshot["run_id"],"recipient_session_id":session_id,
        "problem_version":snapshot["problem_version"],"kind":"control_error", "text":format!("控制处理失败：{code}: {message}。数学输出已保存，已生效动作不要重复；只修正失败动作。连续两轮失败将显式暂停该会话。"),"created_at":now()}),
    );
    if fault && snapshot["role"] != "main" {
        push(
            state,
            "memories",
            json!({"id":id(),"run_id":snapshot["run_id"],"problem_version":snapshot["problem_version"],"kind":"session_fault","text":format!("会话 {session_id} 已停止自动重试：{code}: {message}"),"created_at":now()}),
        );
    }
    Ok(json!({"fault":fault,"main":snapshot["role"]=="main","run_id":snapshot["run_id"]}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn submission_recovery_success_resolves_current_error_and_preserves_history() {
        let receipt = json!({"id":"failed-submit","status":"rejected","error":{"code":"PREMISE_READ_REQUIRED","message":"read exact source"}});
        let mut s = json!({"sessions":[{"id":"main","revision":1}],"action_receipts":[receipt]});
        record_error(
            &mut s,
            "main",
            "ACTION_REJECTED",
            "source declaration missing",
        )
        .unwrap();
        let previous = s["sessions"][0]["last_error"].clone();
        record_error(&mut s, "main", "PREMISE_READ_REQUIRED", "read exact source").unwrap();
        complete_turn(&mut s, "main", &previous, &json!({"id":"unrelated"})).unwrap();
        assert_eq!(
            s["sessions"][0]["last_error"]["code"],
            "PREMISE_READ_REQUIRED"
        );
        let current = s["sessions"][0]["last_error"].clone();
        complete_turn(&mut s, "main", &current, &json!({"id":"successful-output"})).unwrap();
        assert!(s["sessions"][0]["last_error"].is_null());
        assert_eq!(
            s["sessions"][0]["error_history"].as_array().unwrap().len(),
            2
        );
        assert_eq!(s["sessions"][0]["error_history"][0]["status"], "superseded");
        assert_eq!(s["sessions"][0]["error_history"][1]["status"], "resolved");
        assert_eq!(
            s["sessions"][0]["error_history"][1]["error"]["code"],
            "PREMISE_READ_REQUIRED"
        );
        assert_eq!(s["action_receipts"][0], receipt);
    }

    #[test]
    fn submission_recovery_failed_output_releases_its_plan_without_erasing_decision_history() {
        let mut s = json!({"runs":[{"id":"r","state":"running"}],"sessions":[{"id":"main","run_id":"r","role":"main","phase":"waiting_plan","state":"waiting","last_output_artifact_id":"output","pending_plan_id":"plan"}],"planning_proposals":[{"id":"old","run_id":"r","session_id":"main","state":"rejected","source_artifact_id":"old-output"},{"id":"plan","run_id":"r","session_id":"main","state":"pending","actions":[],"source_artifact_id":"output"}]});
        postprocess_failure(
            &mut s,
            "main",
            "PREMISE_READ_REQUIRED",
            "read exact original",
        )
        .unwrap();
        assert_eq!(s["planning_proposals"][0]["state"], "rejected");
        assert_eq!(s["planning_proposals"][1]["state"], "stale");
        assert_eq!(
            s["planning_proposals"][1]["invalidation_reason"],
            "source_output_failed"
        );
        assert_eq!(s["sessions"][0]["state"], "idle");
        assert_eq!(s["sessions"][0]["phase"], "coordination");
        assert!(s["sessions"][0]["pending_plan_id"].is_null());
    }
    #[test]
    fn auxiliary_postprocessing_failure_does_not_reopen_sidecar_or_stop_main() {
        for role in ["display", "memory", "advisor"] {
            let mut state = json!({"runs":[{"id":"run","state":"running","control_epoch":7}],"sessions":[{"id":"main","run_id":"run","role":"main","state":"active","invocation_in_flight":true},{"id":"sidecar","run_id":"run","role":role,"state":"closed","invocation_in_flight":true,"background_job_id":"job"}],"background_jobs":[{"id":"job","state":"running"}],"reviews":[]});
            let main = state["sessions"][0].clone();
            let run = state["runs"][0].clone();
            super::super::lab::fail_turn_at(
                &mut state,
                "sidecar",
                "LAB_BACKGROUND_PROTOCOL",
                &now(),
            )
            .unwrap();
            let result = postprocess_failure(
                &mut state,
                "sidecar",
                "LAB_BACKGROUND_PROTOCOL",
                "missing source reference",
            )
            .unwrap();
            assert_eq!(result["main"], false);
            assert_eq!(state["runs"][0], run);
            assert_eq!(state["sessions"][0], main);
            assert_eq!(state["sessions"][1]["state"], "closed");
            assert_eq!(state["sessions"][1]["pending_followup"], false);
            assert_eq!(state["background_jobs"][0]["state"], "failed");
        }
    }
    #[test]
    fn material_failure_creates_an_answerable_system_question_without_fake_artifact() {
        let mut state = json!({"problem_version":1,"runs":[{"id":"run","state":"running"}],"sessions":[{"id":"main","run_id":"run","role":"main","state":"active"}],"human_questions":[],"messages":[]});
        postprocess_failure(
            &mut state,
            "main",
            "PREMISE_MATERIAL_BLOCKED",
            "需要补充前提原文",
        )
        .unwrap();
        assert_eq!(state["runs"][0]["state"], "waiting_human");
        let question = &state["human_questions"][0];
        assert_eq!(state["runs"][0]["human_question_id"], question["id"]);
        assert_eq!(question["state"], "open");
        assert_eq!(question["source_kind"], "system_material_failure");
        assert!(question["source_artifact_id"].is_null());
    }
    #[test]
    fn only_supplied_feedback_is_consumed_without_starvation() {
        let memories: Vec<Value> = (0..14)
            .map(|n| json!({"id":format!("m{n}"),"session_id":"other"}))
            .collect();
        let mut p = json!({"session":{"id":"main"},"memories":memories});
        let first = selected_memories(&p);
        assert_eq!(first.len(), 12);
        assert_eq!(first[0]["id"], "m0");
        let mut s = p["session"].clone();
        mark_memories_supplied(&mut s, &p);
        p["session"] = s;
        let second = selected_memories(&p);
        assert_eq!(second.len(), 2);
        assert_eq!(second[0]["id"], "m12");
    }
    #[test]
    fn superseded_task_cannot_block_reassigned_partner() {
        let state = json!({"tasks":[{"id":"old","owner_session_id":"p","state":"superseded"},{"id":"new","owner_session_id":"p","state":"queued"}]});
        assert_eq!(
            current_task(&state, &json!({"id":"p","task_id":"new"})).unwrap()["state"],
            "queued"
        );
    }
    #[test]
    fn review_failure_is_terminal_and_wakes_author() {
        let mut s = json!({"sessions":[{"id":"a","state":"waiting"}],"reviews":[{"id":"r","reviewer_session_id":"v","candidate_id":"c","state":"running"}],"candidates":[{"id":"c","author_session_id":"a"}]});
        fail_review(&mut s, "v", "PROVIDER_ERROR");
        assert_eq!(s["reviews"][0]["state"], "failed");
        assert_eq!(s["sessions"][0]["state"], "idle");
    }
}
