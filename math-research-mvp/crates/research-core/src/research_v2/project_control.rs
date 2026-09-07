//! Durable project-wide model-call gate. Research decisions and Fact Gate remain separate.
use super::{
    V2Result, V2Service, Value, array, digest, entity, entity_mut, err, id, json, now, push,
    remaining, revision,
};

pub(super) fn control(state: &Value) -> Value {
    if state["project_control"].is_object() {
        state["project_control"].clone()
    } else {
        json!({"state":"running","revision":1,"epoch":1,"paused_run_ids":[],"pending_session_ids":[],"unknown_invocation_ids":[],"outstanding_cancellation":false})
    }
}

pub(super) fn blocked(state: &Value) -> bool {
    !matches!(
        state["project_control"]["state"].as_str(),
        None | Some("running")
    )
}

pub(super) fn ensure_open(state: &Value) -> V2Result<()> {
    match state["project_control"]["state"].as_str() {
        Some("pausing" | "paused") => Err(err(
            "PROJECT_PAUSED",
            "当前项目已暂停；请明确恢复后再启动模型任务",
        )),
        Some("stopping" | "stopped") => Err(err(
            "PROJECT_STOPPED",
            "当前项目已停止；结束收束后可明确开始新一轮研究",
        )),
        _ => Ok(()),
    }
}

fn pending(state: &Value, registered: &[String]) -> Vec<Value> {
    let mut values = array(state, "sessions")
        .iter()
        .filter(|s| {
            registered.iter().any(|id| s["id"] == *id)
                || s["state"] == "active"
                || array(state, "usage").iter().any(|u| {
                    u["session_id"] == s["id"]
                        && matches!(u["state"].as_str(), Some("reserved" | "running"))
                })
        })
        .map(|s| s["id"].clone())
        .collect::<Vec<_>>();
    values.sort_by_key(Value::to_string);
    values.dedup();
    values
}

fn unknown(state: &Value) -> (Vec<Value>, bool) {
    let ids = array(state, "usage")
        .iter()
        .filter(|u| u["state"] == "unknown")
        .map(|u| u["id"].clone())
        .collect::<Vec<_>>();
    let unresolved = !ids.is_empty()
        || array(state, "runs")
            .iter()
            .any(|r| r["outstanding_cancellation"] == true)
        || array(state, "interactions")
            .iter()
            .any(|i| i["outstanding_cancellation"] == true);
    (ids, unresolved)
}

fn refreshed(state: &Value, registered: &[String]) -> Value {
    let mut value = control(state);
    if !matches!(
        value["state"].as_str(),
        Some("pausing" | "paused" | "stopping" | "stopped")
    ) {
        return value;
    }
    let waiting = pending(state, registered);
    let (uncertain, unresolved) = unknown(state);
    value["pending_session_ids"] = json!(waiting);
    value["unknown_invocation_ids"] = json!(uncertain);
    value["outstanding_cancellation"] = json!(unresolved);
    let settled = waiting.is_empty() && !unresolved;
    value["state"] = json!(
        if matches!(value["state"].as_str(), Some("stopping" | "stopped")) {
            if settled
                && array(state, "runs").iter().all(|r| r["state"] == "ended")
                && array(state, "interactions")
                    .iter()
                    .all(|i| matches!(i["state"].as_str(), Some("ended")))
            {
                "stopped"
            } else {
                "stopping"
            }
        } else if settled {
            "paused"
        } else {
            "pausing"
        }
    );
    value
}

/// Cancellation is a suspension of a live cat's work, not a negative review or failed job.
pub(super) fn settle_interrupted(state: &mut Value, session_id: &str) -> V2Result<bool> {
    if !matches!(
        state["project_control"]["state"].as_str(),
        Some("pausing" | "paused")
    ) {
        return Ok(false);
    }
    let Some(session) = entity(state, "sessions", session_id).cloned() else {
        return Ok(false);
    };
    if session["project_pause_epoch"] != state["project_control"]["epoch"] {
        return Ok(false);
    }
    super::collaboration23::failed_delivery(state, session_id, "CANCELLED")?;
    for message_id in array(&session, "inflight_message_ids")
        .iter()
        .filter_map(Value::as_str)
    {
        if let Ok(message) = entity_mut(state, "messages", message_id) {
            if message["state"] == "delivered" {
                message["state"] = json!("queued");
                message["status"] = json!("queued");
                revision(message);
            }
        }
    }
    for key in ["reviews", "background_jobs", "tasks"] {
        for item in state[key].as_array_mut().into_iter().flatten() {
            let owns = item["reviewer_session_id"] == session_id
                || item["session_id"] == session_id
                || item["owner_session_id"] == session_id;
            if owns && item["state"] == "running" {
                item["state"] = json!("queued");
                item["suspended_by_project"] = json!(true);
                revision(item);
            }
        }
    }
    let current = entity_mut(state, "sessions", session_id)?;
    current["inflight_message_ids"] = json!([]);
    current["invocation_in_flight"] = json!(false);
    current["state"] = json!(if current["route_blocked"] == true {
        "waiting"
    } else {
        "idle"
    });
    current["suspended_by_project"] = json!(true);
    revision(current);
    Ok(true)
}

impl V2Service {
    #[must_use]
    pub fn project_control_from_snapshot(state: &Value) -> Value {
        control(state)
    }

    pub async fn project_control(&self, project: &str, input: Value, key: &str) -> V2Result<Value> {
        if key.trim().is_empty() || key.len() > 200 {
            return Err(err("IDEMPOTENCY_REQUIRED", "项目控制需要幂等键"));
        }
        let action = input["type"].as_str().unwrap_or_default();
        if !matches!(action, "pause" | "resume" | "stop")
            || input
                .as_object()
                .is_none_or(|o| o.keys().any(|k| k != "type"))
        {
            return Err(err(
                "INVALID_REQUEST",
                "项目控制仅支持 type: pause、resume 或 stop",
            ));
        }
        let hash = digest(&input.to_string());
        // The same map protects explanation registration and catches queued cat dispatches.
        let turns = self.turns.lock().await;
        let registered = turns.keys().cloned().collect::<Vec<_>>();
        let outcome = self.store.mutate(project, "project.control_requested", None, |state| {
            if let Some(prior) = array(state, "project_control_operations").iter().find(|o| o["idempotency_key"] == key) {
                if prior["request_hash"] != hash { return Err(err("IDEMPOTENCY_CONFLICT", "同一项目控制键对应不同请求")); }
                return Ok(json!({"control":control(state),"cancel_session_ids":[],"replay":true}));
            }
            let mut value = refreshed(state, &registered);
            let current = value["state"].as_str().unwrap_or("running").to_owned();
            let waiting = pending(state, &registered);
            let (_, unresolved) = unknown(state);
            let mut cancel = Vec::<Value>::new();
            if action == "resume" {
                if unresolved { return Err(err("DELIVERY_UNCERTAIN", "项目仍有未确认停止的模型调用，不能恢复")); }
                if !waiting.is_empty() { return Err(err("CONTROL_PENDING", "模型调用仍在收束，请等待暂停确认")); }
                if current != "paused" { return Err(err("INVALID_STATE", "只有已暂停项目可以恢复；已停止研究须明确开始新Run")); }
                let owned = array(&value, "paused_run_ids").to_vec();
                for run_id in owned.iter().filter_map(Value::as_str) {
                    let Some(prior) = entity(state, "runs", run_id).cloned() else { continue; };
                    if matches!(prior["state"].as_str(), Some("ended" | "stopping")) { continue; }
                    let awaits = super::collaboration23::open_question(state, run_id);
                    let run = entity_mut(state, "runs", run_id)?;
                    if !matches!(run["state"].as_str(), Some("created" | "preflighting")) {
                        if remaining(run) == 0 {
                            run["state"] = json!("stopping");
                            run["requested_stop_reason"] = json!("time_limit");
                        } else {
                            run["state"] = json!(if awaits { "waiting_human" } else { "running" });
                        }
                    }
                    run["project_paused"] = json!(false);
                    revision(run);
                    super::collaboration23::resume_pending_commands(state, run_id);
                }
                value["state"] = json!("running");
                value["paused_run_ids"] = json!([]);
                value["resumed_at"] = json!(now());
            } else {
                if action == "pause" && matches!(current.as_str(), "stopping" | "stopped") {
                    return Err(err("INVALID_STATE", "项目正在停止或已经停止，不能改记为暂停"));
                }
                let first_pause = action == "pause" && current == "running";
                value["epoch"] = json!(value["epoch"].as_u64().unwrap_or(1) + 1);
                if first_pause { value["paused_run_ids"] = json!([]); }
                value["state"] = json!(if action == "stop" { "stopping" } else { "pausing" });
                value["requested_at"] = json!(now());
                let mut owned = array(&value, "paused_run_ids").to_vec();
                for run in state["runs"].as_array_mut().into_iter().flatten() {
                    if run["state"] == "ended" { continue; }
                    if action == "stop" {
                        run["state"] = json!("stopping");
                        run["requested_stop_reason"] = json!("user_stop");
                        run["resume_after_pause"] = Value::Null;
                        revision(run);
                    } else if !matches!(run["state"].as_str(), Some("paused" | "stopping")) || run["project_paused"] == true {
                        if !owned.contains(&run["id"]) { owned.push(run["id"].clone()); }
                        run["project_paused"] = json!(true);
                        run["resume_after_pause"] = Value::Null;
                        if !matches!(run["state"].as_str(), Some("created" | "preflighting" | "paused")) { run["state"] = json!("pausing"); }
                        revision(run);
                    }
                }
                value["paused_run_ids"] = json!(owned);
                for session in state["sessions"].as_array_mut().into_iter().flatten() {
                    if waiting.contains(&session["id"]) {
                        cancel.push(session["id"].clone());
                        if action == "pause" { session["project_pause_epoch"] = value["epoch"].clone(); }
                    }
                }
                for interaction in state["interactions"].as_array_mut().into_iter().flatten() {
                    if interaction["state"] == "ended" { continue; }
                    if interaction["state"] == "created" && action == "pause" { continue; }
                    let running = interaction["state"] != "created";
                    interaction["state"] = json!(if running { "stopping" } else { "ended" });
                    interaction["requested_stop_reason"] = json!(if action == "pause" { "project_pause" } else { "project_stop" });
                    interaction["epoch"] = json!(interaction["epoch"].as_u64().unwrap_or(1) + 1);
                    if !running {
                        interaction["stop_reason"] = interaction["requested_stop_reason"].clone();
                        interaction["ended_at"] = json!(now());
                    }
                    revision(interaction);
                }
            }
            revision(&mut value);
            state["project_control"] = value.clone();
            push(state, "project_control_operations", json!({"id":id(),"type":action,"idempotency_key":key,"request_hash":hash,"control_revision":value["revision"],"created_at":now()}));
            Ok(json!({"control":value,"cancel_session_ids":cancel,"replay":false}))
        }).await?;
        for session in array(&outcome, "cancel_session_ids")
            .iter()
            .filter_map(Value::as_str)
        {
            if let Some(token) = turns.get(session) {
                token.cancel();
            }
        }
        drop(turns);
        self.refresh_project_control(project).await?;
        if outcome["replay"] != true {
            for run in array(&self.store.read(project).await?, "runs") {
                if run["state"] != "ended" {
                    self.launch(project, run["id"].as_str().unwrap_or_default())
                        .await;
                }
            }
        }
        Ok(control(&self.store.read(project).await?))
    }

    pub(super) async fn refresh_project_control(&self, project: &str) -> V2Result<()> {
        let snapshot = self.store.read(project).await?;
        if !blocked(&snapshot) {
            return Ok(());
        }
        let turns = self.turns.lock().await;
        let registered = turns.keys().cloned().collect::<Vec<_>>();
        let next = refreshed(&snapshot, &registered);
        if next == control(&snapshot) {
            return Ok(());
        }
        self.store
            .mutate(project, "project.control_updated", None, |state| {
                let mut next = refreshed(state, &registered);
                if next["state"] == "paused" {
                    let owned = array(&next, "paused_run_ids").to_vec();
                    for run in state["runs"].as_array_mut().into_iter().flatten() {
                        if owned.contains(&run["id"]) && run["state"] == "pausing" {
                            run["state"] = json!("paused");
                            revision(run);
                        }
                    }
                }
                revision(&mut next);
                state["project_control"] = next.clone();
                Ok(next)
            })
            .await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "project_control_tests.rs"]
mod tests;
