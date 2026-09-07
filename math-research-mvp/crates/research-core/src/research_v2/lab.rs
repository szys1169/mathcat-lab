//! `MathCat` 2.2 state transitions. This module never calls a model or certifies mathematics.
//! Background work is represented by ordinary executable sessions, not synthetic summaries.
// Keep the model argument compatible with the existing parent session factory.
#![allow(clippy::ref_option)]

use super::{array, entity, entity_mut, err, id, new_session, push, revision};
use chrono::{DateTime, Duration};
use research_storage::research_v2::V2Result;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn timestamp(at: &str) -> i64 {
    DateTime::parse_from_rfc3339(at).map_or(0, |date| date.timestamp())
}

fn later(at: &str, seconds: u64) -> String {
    DateTime::parse_from_rfc3339(at).map_or_else(
        |_| at.to_owned(),
        |date| {
            (date + Duration::seconds(i64::try_from(seconds).unwrap_or(i64::MAX / 2))).to_rfc3339()
        },
    )
}

fn elapsed(start: &Value, at: &str) -> u64 {
    start.as_str().map_or(0, |start| {
        u64::try_from((timestamp(at) - timestamp(start)).max(0)).unwrap_or(0)
    })
}

fn changed(value: &mut Value, at: &str) {
    revision(value);
    value["updated_at"] = json!(at);
}

fn text_field<'a>(value: &'a Value, key: &str) -> V2Result<&'a str> {
    value[key]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| err("LAB_INVALID_CONTROL", format!("缺少有效字段 {key}")))
}

fn enabled(run: &Value, at: &str) -> bool {
    run["state"] == "running"
        && run["verification_only"] != true
        && run["mode"] != "verification_only"
        && run["deadline_at"]
            .as_str()
            .is_none_or(|deadline| timestamp(deadline) > timestamp(at))
}

fn session_route(session: &Value) -> Option<&str> {
    session["route_id"].as_str()
}

fn source_refs(state: &Value, run_id: &str) -> Vec<String> {
    let mut refs = BTreeSet::new();
    for key in [
        "nodes",
        "candidates",
        "reviews",
        "proof_checkpoints",
        "pending_assignments",
    ] {
        for item in array(state, key).iter().filter(|item| {
            item["run_id"] == run_id
                || (key == "nodes"
                    && item["author"]
                        .as_str()
                        .and_then(|author| entity(state, "sessions", author))
                        .is_some_and(|session| {
                            session["run_id"] == run_id
                                && matches!(
                                    session["role"].as_str(),
                                    Some("main" | "partner" | "reviewer")
                                )
                        }))
        }) {
            if let Some(identifier) = item["id"].as_str() {
                refs.insert(identifier.to_owned());
            }
            for field in ["artifact_id", "body_artifact_id", "proof_artifact_id"] {
                if let Some(identifier) = item[field].as_str() {
                    refs.insert(identifier.to_owned());
                }
            }
        }
    }
    refs.into_iter().collect()
}

fn source_version(state: &Value, sources: &[String]) -> Value {
    json!(sources.iter().filter_map(|identifier|resolve_source(state,identifier)).map(|item|json!({"id":item["source"]["id"],"revision":item["source"]["revision"],"state":item["source"]["state"],"status":item["source"]["status"],"assurance":item["source"]["assurance"]})).collect::<Vec<_>>())
}

fn resolve_source(state: &Value, identifier: &str) -> Option<Value> {
    for key in [
        "nodes",
        "candidates",
        "reviews",
        "proof_checkpoints",
        "pending_assignments",
        "artifacts",
        "facts",
        "memories",
        "statements",
    ] {
        if let Some(source) = entity(state, key, identifier) {
            return Some(json!({"collection":key,"source":source}));
        }
    }
    None
}

/// Fill only absent controls. Invocation limits and run deadlines remain independent.
pub(super) fn normalize_limits(limits: &mut Value) -> V2Result<()> {
    if !limits.is_object() {
        return Err(err("LAB_INVALID_LIMITS", "运行配置必须是对象"));
    }
    for (key, default) in [
        ("research_soft_seconds", 1200),
        ("research_hard_seconds", 1800),
        ("coordination_soft_seconds", 120),
        ("coordination_hard_seconds", 300),
        ("advisor_interval_seconds", 3600),
        ("memory_interval_seconds", 300),
        ("display_interval_seconds", 30),
    ] {
        if limits[key].is_null() {
            limits[key] =
                if key == "advisor_interval_seconds" && limits["advisor_interval"].is_u64() {
                    limits["advisor_interval"].clone()
                } else {
                    json!(default)
                };
        }
        if limits[key]
            .as_u64()
            .is_none_or(|value| value == 0 || value > 31_536_000)
        {
            return Err(err(
                "LAB_INVALID_LIMITS",
                format!("{key} 必须是 1 至 31536000 的秒数"),
            ));
        }
    }
    for (soft, hard) in [
        ("research_soft_seconds", "research_hard_seconds"),
        ("coordination_soft_seconds", "coordination_hard_seconds"),
    ] {
        if limits[soft].as_u64() > limits[hard].as_u64() {
            return Err(err("LAB_INVALID_LIMITS", format!("{soft} 不能超过 {hard}")));
        }
    }
    Ok(())
}

pub(super) fn initialize_run_at(state: &mut Value, run_id: &str, at: &str) -> V2Result<Value> {
    let run = entity_mut(state, "runs", run_id)?;
    if run["lab_version"] == "2.2.0" {
        return Ok(json!({"changed":false}));
    }
    normalize_limits(&mut run["limits"])?;
    let start = run["started_at"].as_str().unwrap_or(at).to_owned();
    run["lab_version"] = json!("2.2.0");
    run["advisor_next_due_at"] = json!(later(
        &start,
        run["limits"]["advisor_interval_seconds"]
            .as_u64()
            .unwrap_or(3600)
    ));
    run["lab_source_refs"] = json!([]);
    run["lab_source_version"] = json!([]);
    run["memory_pending_since"] = Value::Null;
    run["display_pending_since"] = Value::Null;
    run["memory_source_refs"] = json!([]);
    run["display_source_refs"] = json!([]);
    run["memory_source_version"] = json!([]);
    run["display_source_version"] = json!([]);
    changed(run, at);
    Ok(json!({"changed":true}))
}

pub(super) fn transition(
    state: &mut Value,
    session_id: &str,
    phase: &str,
    reason: &str,
    at: &str,
) -> V2Result<()> {
    let session = entity_mut(state, "sessions", session_id)?;
    if session["phase"] == phase {
        return Ok(());
    }
    let previous = session["phase"].clone();
    let run_id = session["run_id"].clone();
    let checkpoint = session["latest_checkpoint_id"].clone();
    session["phase"] = json!(phase);
    session["phase_started_at"] = json!(at);
    session["phase_soft_due"] = json!(false);
    changed(session, at);
    push(
        state,
        "cycles",
        json!({"id":id(),"run_id":run_id,"session_id":session_id,"from_phase":previous,"phase":phase,"reason":reason,"checkpoint_id":checkpoint,"created_at":at}),
    );
    Ok(())
}

fn pending_urgent(state: &Value, session: &Value) -> bool {
    session["pending_urgent"] == true
        || array(state, "messages").iter().any(|message| {
            message["run_id"] == session["run_id"]
                && message["priority"] == "urgent"
                && message_pending(message)
                && (message["target_session_id"] == session["id"]
                    || message["recipient_session_id"] == session["id"]
                    || (session["role"] == "main" && message["target_role"] == "main"))
        })
}

fn message_pending(message: &Value) -> bool {
    matches!(
        message["state"]
            .as_str()
            .or_else(|| message["status"].as_str()),
        Some("queued" | "pending" | "delivered")
    )
}

fn messages_for_session(state: &Value, session: &Value) -> Vec<Value> {
    array(state, "messages")
        .iter()
        .filter(|message| {
            let target = message["target_session_id"] == session["id"]
                || message["recipient_session_id"] == session["id"]
                || message["target_role"] == session["role"];
            let timely = if session["role"] == "main" {
                matches!(
                    session["phase"].as_str(),
                    Some("coordination" | "urgent_attention")
                ) && (session["phase"] != "urgent_attention" || message["priority"] == "urgent")
            } else {
                true
            };
            let command_live = message["command_id"].as_str().is_none_or(|id| {
                entity(state, "commands", id).is_some_and(|c| {
                    !matches!(c["status"].as_str(), Some("cancelled" | "superseded"))
                })
            });
            message["run_id"] == session["run_id"]
                && message_pending(message)
                && target
                && timely
                && command_live
        })
        .cloned()
        .collect()
}

fn acknowledge_messages(
    state: &mut Value,
    session_id: &str,
    control: &Value,
    at: &str,
) -> V2Result<()> {
    let session = entity(state, "sessions", session_id).unwrap().clone();
    for identifier in array(&session, "inflight_message_ids")
        .iter()
        .filter_map(Value::as_str)
    {
        let receipt = array(control, "message_receipts")
            .iter()
            .find(|receipt| receipt["message_id"] == identifier);
        let message = entity_mut(state, "messages", identifier)?;
        message["state"] = json!("handled");
        message["status"] = json!("handled");
        message["receipt"]=receipt.cloned().unwrap_or_else(||json!({"message_id":identifier,"disposition":"received","reason":"已成功投递；模型未给出采纳决定，不表示建议已采纳或已审查"}));
        message["handled_by"] = json!(session_id);
        message["handled_at"] = json!(at);
        changed(message, at);
        if let Some(advisory_id) = message["advisory_id"].as_str().map(str::to_owned) {
            if let Ok(advisory) = entity_mut(state, "advisories", &advisory_id) {
                advisory["state"] = json!("delivered_to_main");
                changed(advisory, at);
            }
        }
    }
    entity_mut(state, "sessions", session_id)?["inflight_message_ids"] = json!([]);
    Ok(())
}

fn main_has_updates(state: &Value, run_id: &str) -> bool {
    array(state, "commands").iter().any(|item| {
        item["run_id"] == run_id
            && (item["state"] == "queued"
                || matches!(item["status"].as_str(), Some("pending" | "queued")))
    }) || array(state, "messages").iter().any(|item| {
        item["run_id"] == run_id
            && message_pending(item)
            && (item["target_role"] == "main"
                || item["recipient_session_id"]
                    .as_str()
                    .or_else(|| item["target_session_id"].as_str())
                    .and_then(|sid| entity(state, "sessions", sid))
                    .is_some_and(|session| session["role"] == "main"))
    })
}

fn background_session(
    state: &mut Value,
    run: &Value,
    role: &str,
    model: &Option<String>,
    sources: &[String],
    trigger: &str,
    at: &str,
) -> bool {
    let run_id = run["id"].as_str().unwrap_or_default();
    if array(state, "background_jobs").iter().any(|job| {
        job["run_id"] == run_id
            && job["kind"] == role
            && matches!(job["state"].as_str(), Some("queued" | "running"))
    }) {
        return false;
    }
    let previous = array(state, "background_jobs")
        .iter()
        .rev()
        .find(|job| job["run_id"] == run_id && job["kind"] == role);
    if previous.is_some_and(|job| {
        job["state"] == "failed"
            && job["source_refs"] == json!(sources)
            && job["attempt"].as_u64().unwrap_or(1) >= 2
            && trigger != "periodic"
    }) {
        return false;
    }
    let attempt = previous
        .filter(|job| job["state"] == "failed" && job["source_refs"] == json!(sources))
        .map_or(1, |job| job["attempt"].as_u64().unwrap_or(1) + 1);
    let job_id = id();
    let mut session = new_session(
        state,
        run,
        role,
        Some(match role {
            "advisor" => "独立检查路线与分工，并明确证据和不确定性",
            "memory" => "根据原始材料生成可检索记忆，保留准确来源",
            _ => "根据原始事件生成后台展示摘要",
        }),
        model,
    );
    session["background_job_id"] = json!(job_id);
    session["protocol_mode"] = json!("lab_2_2");
    session["created_at"] = json!(at);
    let snapshot: Vec<Value> = sources
        .iter()
        .filter_map(|source| resolve_source(state, source))
        .collect();
    let research_overview = json!({"problem":state["problem"],"problem_version":state["problem_version"],"researchers":array(state,"sessions").iter().filter(|session|session["run_id"]==run_id && matches!(session["role"].as_str(),Some("main"|"partner"))).map(|session|json!({"id":session["id"],"role":session["role"],"state":session["state"],"phase":session["phase"],"focus":session["focus"],"task_id":session["task_id"],"route_id":session["route_id"],"latest_checkpoint_id":session["latest_checkpoint_id"],"progress_signature":session["progress_signature"],"progress_warning":session["progress_warning"]})).collect::<Vec<_>>(),"routes":array(state,"routes").iter().filter(|route|route["run_id"]==run_id).collect::<Vec<_>>(),"previous_advisories":array(state,"advisories").iter().rev().filter(|advisory|advisory["run_id"]==run_id).take(8).collect::<Vec<_>>()});
    push(
        state,
        "background_jobs",
        json!({"id":job_id,"run_id":run_id,"kind":role,"session_id":session["id"],"state":"queued","trigger":trigger,"source_refs":sources,"source_snapshot":snapshot,"research_overview":research_overview,"snapshot_revision":state["revision"],"attempt":attempt,"created_at":at,"planned_at":if role=="advisor" {run["advisor_next_due_at"].clone()} else {json!(at)}}),
    );
    push(state, "sessions", session);
    true
}

/// Called on a cloned aggregate before committing, so idle ticks produce no writes.
pub(super) fn tick_at(
    state: &mut Value,
    run_id: &str,
    model: &Option<String>,
    at: &str,
) -> V2Result<Value> {
    let mut did_change = initialize_run_at(state, run_id, at)?["changed"] == true;
    let mut cancel_session_ids = Vec::new();
    let run = entity(state, "runs", run_id)
        .ok_or_else(|| err("NOT_FOUND", "找不到运行"))?
        .clone();
    if !enabled(&run, at) {
        return Ok(json!({"changed":did_change,"cancel_session_ids":[]}));
    }
    super::whiteboard24::tick(state, run_id);
    let sessions: Vec<Value> = array(state, "sessions")
        .iter()
        .filter(|session| session["run_id"] == run_id && session["role"] == "main")
        .cloned()
        .collect();
    for session in sessions {
        let sid = session["id"].as_str().unwrap_or_default();
        if super::whiteboard24::blocked(state, &session) {
            continue;
        }
        if session["phase"].is_null() {
            transition(state, sid, "research", "run_started", at)?;
            did_change = true;
        }
        if pending_urgent(state, &session)
            && (session["invocation_in_flight"] == true || session["state"] == "active")
        {
            let signal = json!(
                array(state, "messages")
                    .iter()
                    .filter(|message| message["run_id"] == run_id
                        && message["priority"] == "urgent"
                        && message_pending(message)
                        && (message["target_role"] == "main"
                            || message["recipient_session_id"] == session["id"]
                            || message["target_session_id"] == session["id"]))
                    .map(|message| message["id"].clone())
                    .collect::<Vec<_>>()
            );
            if session["urgent_cancel_requested_for"] != signal {
                let current = entity_mut(state, "sessions", sid)?;
                current["urgent_cancel_requested_for"] = signal;
                current["pending_urgent"] = json!(true);
                current["control_epoch"] =
                    json!(current["control_epoch"].as_u64().unwrap_or(1) + 1);
                changed(current, at);
                cancel_session_ids.push(sid.to_owned());
                did_change = true;
            }
        }
        if session["invocation_in_flight"] != true
            && session["state"] == "waiting"
            && session["phase"] == "waiting"
            && (main_has_updates(state, run_id)
                || (session["host_wait_targets"].is_array()
                    && session["route_blocked"] != true
                    && super::whiteboard24::waiting_targets(state, run_id).is_empty()))
        {
            transition(state, sid, "coordination", "new_input", at)?;
            entity_mut(state, "sessions", sid)?["state"] = json!("idle");
            did_change = true;
        }
        if pending_urgent(state, &session)
            && session["invocation_in_flight"] != true
            && session["state"] != "active"
            && session["phase"] != "urgent_attention"
        {
            let current = entity_mut(state, "sessions", sid)?;
            current["resume_phase"] = current["phase"].clone();
            transition(state, sid, "urgent_attention", "urgent_message", at)?;
            entity_mut(state, "sessions", sid)?["state"] = json!("idle");
            did_change = true;
        }
    }
    let sources = source_refs(state, run_id);
    let version = source_version(state, &sources);
    if run["lab_source_refs"] != json!(sources) || run["lab_source_version"] != version {
        let current = entity_mut(state, "runs", run_id)?;
        current["lab_source_refs"] = json!(sources);
        current["lab_source_version"] = version.clone();
        for field in ["memory_pending_since", "display_pending_since"] {
            if current[field].is_null() {
                current[field] = json!(at);
            }
        }
        changed(current, at);
        did_change = true;
    }
    let run = entity(state, "runs", run_id).unwrap().clone();
    let invocations = array(state, "usage")
        .iter()
        .filter(|usage| usage["run_id"] == run_id && usage["state"] != "not_dispatched")
        .count() as u64;
    let limit = run["limits"]["max_invocations"].as_u64().unwrap_or(100);
    if invocations >= limit {
        return Ok(json!({"changed":did_change,"cancel_session_ids":cancel_session_ids}));
    }
    for role in ["advisor", "memory", "display"] {
        let failed = array(state, "background_jobs")
            .iter()
            .rev()
            .find(|job| job["run_id"] == run_id && job["kind"] == role)
            .filter(|job| {
                job["state"] == "failed"
                    && job["attempt"].as_u64().unwrap_or(1) < 2
                    && elapsed(&job["ended_at"], at) >= 30
            })
            .cloned();
        if let Some(job) = failed {
            let retry_sources: Vec<String> = array(&job, "source_refs")
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            did_change |= background_session(state, &run, role, model, &retry_sources, "retry", at);
        }
    }
    if timestamp(run["advisor_next_due_at"].as_str().unwrap_or(at)) <= timestamp(at)
        && background_session(state, &run, "advisor", model, &sources, "periodic", at)
    {
        let current = entity_mut(state, "runs", run_id)?;
        current["advisor_next_due_at"] = json!(later(
            at,
            run["limits"]["advisor_interval_seconds"]
                .as_u64()
                .unwrap_or(3600)
        ));
        changed(current, at);
        did_change = true;
    }
    for role in ["memory", "display"] {
        let since = format!("{role}_pending_since");
        let cursor = format!("{role}_source_refs");
        let period = run["limits"][format!("{role}_interval_seconds")]
            .as_u64()
            .unwrap_or(300);
        let pending = sources
            .iter()
            .filter(|source| {
                !array(&run, &cursor)
                    .iter()
                    .any(|old| old == source.as_str())
            })
            .count();
        let cursor_version = format!("{role}_source_version");
        if !sources.is_empty()
            && (pending > 0 || run[&cursor_version] != version)
            && !run[&since].is_null()
            && (elapsed(&run[&since], at) >= period || (role == "memory" && pending >= 8))
            && background_session(state, &run, role, model, &sources, "source_events", at)
        {
            let current = entity_mut(state, "runs", run_id)?;
            current[&since] = Value::Null;
            current[&cursor] = json!(sources);
            current[&cursor_version] = version.clone();
            changed(current, at);
            did_change = true;
        }
    }
    Ok(json!({"changed":did_change,"cancel_session_ids":cancel_session_ids}))
}

fn apply_pending(state: &mut Value, session_id: &str, at: &str) -> V2Result<()> {
    let session = entity(state, "sessions", session_id)
        .ok_or_else(|| err("NOT_FOUND", "找不到研究员"))?
        .clone();
    let Some(pending_id) = session["pending_assignment_id"].as_str() else {
        return Ok(());
    };
    let assignment = entity(state, "pending_assignments", pending_id)
        .ok_or_else(|| err("LAB_PENDING_MISSING", "待生效安排记录不存在"))?
        .clone();
    if assignment["state"] != "pending" {
        entity_mut(state, "sessions", session_id)?["pending_assignment_id"] = Value::Null;
        return Ok(());
    }
    let route = assignment["route_id"].as_str();
    if route.is_some_and(|route| route_blocked(state, route)) {
        return Err(err("ROUTE_BLOCKED", "待生效任务所属路线已被禁止"));
    }
    if let Some(task_id) = session["task_id"].as_str() {
        let task = entity_mut(state, "tasks", task_id)?;
        task["state"] = json!("superseded");
        changed(task, at);
    }
    let epoch = session["assignment_epoch"].as_u64().unwrap_or(1) + 1;
    let task_id = id();
    let run = entity(
        state,
        "runs",
        session["run_id"].as_str().unwrap_or_default(),
    )
    .cloned()
    .unwrap_or(Value::Null);
    push(
        state,
        "tasks",
        json!({"id":task_id,"run_id":session["run_id"],"owner_session_id":session_id,"kind":"research","focus":assignment["focus"],"route_id":assignment["route_id"],"assignment_id":pending_id,"state":"queued","task_epoch":epoch,"assignment_epoch":epoch,"deadline_at":run["deadline_at"],"revision":1,"created_at":at}),
    );
    let current = entity_mut(state, "sessions", session_id)?;
    current["task_id"] = json!(task_id);
    current["assignment_epoch"] = json!(epoch);
    current["focus"] = assignment["focus"].clone();
    current["route_id"] = assignment["route_id"].clone();
    current["pending_assignment_id"] = Value::Null;
    current["continuation"] = Value::Null;
    current["pending_premise_ids"] = json!([]);
    current["premise_work_mode"] = Value::Null;
    current["pending_followup"] = json!(false);
    current["assignment_context"] = assignment;
    changed(current, at);
    let pending = entity_mut(state, "pending_assignments", pending_id)?;
    pending["state"] = json!("applied");
    pending["applied_at"] = json!(at);
    pending["applied_epoch"] = json!(epoch);
    changed(pending, at);
    Ok(())
}

/// A completed old task may yield its session to an authorized pending task at dispatch.
/// Pauses, cancellations and route prohibitions remain hard barriers.
pub(super) fn completed_task_has_pending_assignment(state: &Value, session: &Value) -> bool {
    session["role"] == "partner"
        && session["state"] == "idle"
        && session["invocation_in_flight"] != true
        && session["task_id"]
            .as_str()
            .and_then(|task_id| entity(state, "tasks", task_id))
            .is_some_and(|task| task["state"] == "completed")
        && session["pending_assignment_id"]
            .as_str()
            .and_then(|assignment_id| entity(state, "pending_assignments", assignment_id))
            .is_some_and(|assignment| {
                assignment["state"] == "pending"
                    && assignment["run_id"] == session["run_id"]
                    && assignment["partner_session_id"] == session["id"]
                    && assignment["route_id"].as_str().is_some_and(|route_id| {
                        entity(state, "routes", route_id)
                            .is_some_and(|route| route["run_id"] == session["run_id"])
                            && !route_blocked(state, route_id)
                    })
            })
}

/// A partner remains complete; the main researcher receives a sourced status notification.
pub(super) fn notify_partner_review_at(state: &mut Value, review_id: &str, at: &str) {
    let Some(review) = entity(state, "reviews", review_id).cloned() else {
        return;
    };
    if !matches!(
        review["state"].as_str(),
        Some("completed" | "failed" | "cancelled")
    ) {
        return;
    }
    let Some(candidate) = review["candidate_id"]
        .as_str()
        .and_then(|candidate_id| entity(state, "candidates", candidate_id))
        .cloned()
    else {
        return;
    };
    if !candidate["author_session_id"]
        .as_str()
        .and_then(|author_id| entity(state, "sessions", author_id))
        .is_some_and(|author| author["role"] == "partner")
    {
        return;
    }
    if array(state, "messages").iter().any(|message| {
        message["kind"] == "partner_review_ready"
            && message["review_id"] == review_id
            && message["review_revision"] == review["revision"]
    }) {
        return;
    }
    let mut refs = vec![json!(review_id), candidate["id"].clone()];
    for field in ["artifact_id", "report_artifact_id"] {
        if let Some(artifact_id) = review[field].as_str() {
            refs.push(json!(artifact_id));
        }
    }
    push(
        state,
        "messages",
        json!({"id":id(),"run_id":candidate["run_id"],"target_role":"main","kind":"partner_review_ready","priority":"normal","state":"queued","review_id":review_id,"review_revision":review["revision"],"candidate_id":candidate["id"],"author_session_id":candidate["author_session_id"],"body":{"state":review["state"],"verdict":review["verdict"],"issues":review["issues"],"failure_code":review["failure_code"],"goal_coverage":review["goal_coverage"]},"evidence_refs":refs,"trust":"review_status_only","problem_version":candidate["problem_version"],"created_at":at}),
    );
}

pub(super) fn prepare_turn_at(
    state: &mut Value,
    run_id: &str,
    session_id: &str,
    at: &str,
) -> V2Result<Value> {
    initialize_run_at(state, run_id, at)?;
    ensure_research_bindings(state, run_id, at)?;
    let session = entity(state, "sessions", session_id)
        .ok_or_else(|| err("NOT_FOUND", "找不到会话"))?
        .clone();
    if session["run_id"] != run_id {
        return Err(err("LAB_WRONG_RUN", "会话不属于此运行"));
    }
    if session["role"] == "partner" {
        if session["task_id"].is_null() {
            let task = array(state, "tasks")
                .iter()
                .rev()
                .find(|task| {
                    task["owner_session_id"] == session_id
                        && !matches!(task["state"].as_str(), Some("superseded" | "cancelled"))
                })
                .cloned();
            if let Some(task) = task {
                let current = entity_mut(state, "sessions", session_id)?;
                current["task_id"] = task["id"].clone();
                current["assignment_epoch"] = json!(task["task_epoch"].as_u64().unwrap_or(1));
            }
        }
        apply_pending(state, session_id, at)?;
    }
    if session["role"] == "main" && session["phase"].is_null() {
        transition(state, session_id, "research", "first_turn", at)?;
    }
    if session["role"] == "main" {
        let current = entity(state, "sessions", session_id).unwrap().clone();
        if pending_urgent(state, &current) && current["phase"] != "urgent_attention" {
            transition(
                state,
                session_id,
                "urgent_attention",
                "urgent_at_natural_boundary",
                at,
            )?;
        } else if main_has_updates(state, run_id)
            && matches!(current["phase"].as_str(), Some("research" | "waiting"))
        {
            transition(
                state,
                session_id,
                "coordination",
                "input_at_natural_boundary",
                at,
            )?;
        }
    }
    if let Some(job_id) = session["background_job_id"].as_str() {
        let job = entity_mut(state, "background_jobs", job_id)?;
        job["state"] = json!("running");
        job["started_at"] = json!(at);
        changed(job, at);
    }
    let session = entity(state, "sessions", session_id).unwrap().clone();
    let messages = messages_for_session(state, &session);
    let message_ids: Vec<Value> = messages
        .iter()
        .map(|message| message["id"].clone())
        .collect();
    for identifier in message_ids.iter().filter_map(Value::as_str) {
        let message = entity_mut(state, "messages", identifier)?;
        message["state"] = json!("delivered");
        message["delivered_at"] = json!(at);
        changed(message, at);
    }
    entity_mut(state, "sessions", session_id)?["inflight_message_ids"] = json!(message_ids);
    let session = entity(state, "sessions", session_id).unwrap();
    if session_route(session).is_some_and(|route| route_blocked(state, route)) {
        return Err(err("ROUTE_BLOCKED", "路线已被禁止"));
    }
    let run = entity(state, "runs", run_id).unwrap();
    let route_epochs: Vec<Value> = session_route(session)
        .into_iter()
        .filter_map(|route| entity(state, "routes", route))
        .map(|route| json!({"id":route["id"],"control_epoch":route["control_epoch"]}))
        .collect();
    Ok(
        json!({"run_id":run_id,"session_id":session_id,"phase":session["phase"],"phase_started_at":session["phase_started_at"],"run_control_epoch":run["control_epoch"],"session_control_epoch":session["control_epoch"],"assignment_epoch":session["assignment_epoch"],"task_id":session["task_id"],"route_epochs":route_epochs,"checkpoint":session["latest_checkpoint_id"].as_str().and_then(|identifier| entity(state,"proof_checkpoints",identifier)),"assignment":session["assignment_context"],"limits":run["limits"],"background_job":session["background_job_id"].as_str().and_then(|identifier| entity(state,"background_jobs",identifier)),"progress_warning":session["progress_warning"],"protocol_feedback":session["protocol_feedback"],"messages":messages,"message_ids":message_ids,"prepared_at":at}),
    )
}

pub(super) fn execution_valid(state: &Value, session_id: &str, capture: &Value) -> bool {
    let Some(session) = entity(state, "sessions", session_id) else {
        return false;
    };
    let Some(run) = entity(
        state,
        "runs",
        session["run_id"].as_str().unwrap_or_default(),
    ) else {
        return false;
    };
    run["control_epoch"] == capture["run_control_epoch"]
        && session["control_epoch"] == capture["session_control_epoch"]
        && session["assignment_epoch"] == capture["assignment_epoch"]
        && session["task_id"] == capture["task_id"]
        && !matches!(session["state"].as_str(), Some("cancelled" | "stopped"))
        && array(capture, "route_epochs").iter().all(|expected| {
            expected["id"]
                .as_str()
                .and_then(|route| entity(state, "routes", route))
                .is_some_and(|route| {
                    route["control_epoch"] == expected["control_epoch"]
                        && !route_blocked(state, route["id"].as_str().unwrap_or_default())
                })
        })
}

pub(super) fn turn_timeout_seconds(
    run: &Value,
    session: &Value,
    backend_limit: u64,
    at: &str,
) -> u64 {
    if session["role"] != "main" {
        return backend_limit.max(1);
    }
    let key = if matches!(
        session["phase"].as_str(),
        Some("coordination" | "urgent_attention")
    ) {
        "coordination_hard_seconds"
    } else {
        "research_hard_seconds"
    };
    let limit = run["limits"][key]
        .as_u64()
        .unwrap_or(if key == "research_hard_seconds" {
            1800
        } else {
            300
        });
    backend_limit
        .min(limit.saturating_sub(elapsed(&session["phase_started_at"], at)))
        .max(1)
}

pub(super) fn route_blocked(state: &Value, route_id: &str) -> bool {
    let mut todo = vec![route_id.to_owned()];
    let mut visited = BTreeSet::new();
    while let Some(route_id) = todo.pop() {
        if !visited.insert(route_id.clone()) {
            continue;
        }
        let Some(route) = entity(state, "routes", &route_id) else {
            return true;
        };
        if matches!(
            route["status"].as_str(),
            Some("blocked" | "banned" | "prohibited")
        ) || route["restriction"]["blocked"] == true
        {
            return true;
        }
        todo.extend(
            array(route, "parent_route_ids")
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    }
    false
}

/// User control only: no model action delegates to this function.
pub(super) fn ban_route_at(
    state: &mut Value,
    run_id: &str,
    route_id: &str,
    reason: &str,
    at: &str,
) -> V2Result<Value> {
    let target = entity(state, "routes", route_id).ok_or_else(|| err("NOT_FOUND", "路线不存在"))?;
    if target["run_id"] != run_id {
        return Err(err("LAB_WRONG_RUN", "路线不属于此运行"));
    }
    let route = entity_mut(state, "routes", route_id)?;
    route["status"] = json!("blocked");
    route["restriction"] =
        json!({"blocked":true,"authority":"human","reason":reason,"created_at":at});
    route["control_epoch"] = json!(route["control_epoch"].as_u64().unwrap_or(1) + 1);
    changed(route, at);
    let covered: Vec<String> = array(state, "routes")
        .iter()
        .filter(|route| {
            route["run_id"] == run_id
                && route["id"]
                    .as_str()
                    .is_some_and(|identifier| route_blocked(state, identifier))
        })
        .filter_map(|route| route["id"].as_str().map(str::to_owned))
        .collect();
    let affected: Vec<String> = array(state, "sessions")
        .iter()
        .filter(|session| {
            session["run_id"] == run_id
                && session_route(session)
                    .is_some_and(|route| covered.iter().any(|identifier| identifier == route))
        })
        .filter_map(|session| session["id"].as_str().map(str::to_owned))
        .collect();
    for sid in &affected {
        let session = entity_mut(state, "sessions", sid)?;
        session["control_epoch"] = json!(session["control_epoch"].as_u64().unwrap_or(1) + 1);
        session["state"] = json!("waiting");
        session["route_blocked"] = json!(true);
        session["continuation"] = Value::Null;
        if session["role"] == "main" {
            session["route_id"] = Value::Null;
            session["state"] = json!("idle");
        }
        changed(session, at);
        if session["role"] == "main" {
            transition(state, sid, "coordination", "route_banned", at)?;
        }
    }
    let tasks: Vec<String> = array(state, "tasks")
        .iter()
        .filter(|task| {
            task["run_id"] == run_id
                && (task["route_id"]
                    .as_str()
                    .is_some_and(|route| covered.iter().any(|identifier| identifier == route))
                    || task["owner_session_id"]
                        .as_str()
                        .is_some_and(|sid| affected.iter().any(|identifier| identifier == sid)))
                && !matches!(task["state"].as_str(), Some("completed" | "superseded"))
        })
        .filter_map(|task| task["id"].as_str().map(str::to_owned))
        .collect();
    for identifier in tasks {
        let task = entity_mut(state, "tasks", &identifier)?;
        task["state"] = json!("cancelled");
        task["task_epoch"] = json!(task["task_epoch"].as_u64().unwrap_or(1) + 1);
        changed(task, at);
    }
    for key in ["reviews", "background_jobs"] {
        let records: Vec<String> = array(state, key)
            .iter()
            .filter(|item| {
                item["run_id"] == run_id
                    && matches!(item["state"].as_str(), Some("queued" | "running"))
                    && item[if key == "reviews" {
                        "reviewer_session_id"
                    } else {
                        "session_id"
                    }]
                    .as_str()
                    .is_some_and(|sid| affected.iter().any(|identifier| identifier == sid))
            })
            .filter_map(|item| item["id"].as_str().map(str::to_owned))
            .collect();
        for identifier in records {
            let item = entity_mut(state, key, &identifier)?;
            item["state"] = json!("cancelled");
            changed(item, at);
        }
    }
    let pending: Vec<String> = array(state, "pending_assignments")
        .iter()
        .filter(|item| {
            item["state"] == "pending"
                && item["route_id"]
                    .as_str()
                    .is_some_and(|route| covered.iter().any(|identifier| identifier == route))
        })
        .filter_map(|item| item["id"].as_str().map(str::to_owned))
        .collect();
    for identifier in pending {
        let item = entity_mut(state, "pending_assignments", &identifier)?;
        item["state"] = json!("cancelled");
        changed(item, at);
    }
    Ok(json!({"cancel_session_ids":affected,"covered_route_ids":covered}))
}

pub(super) fn prohibit_route_at(
    state: &mut Value,
    run_id: &str,
    route_id: &str,
    reason: &str,
    at: &str,
) -> V2Result<Value> {
    ban_route_at(state, run_id, route_id, reason, at)
}

/// Removing a human restriction does not resume cancelled work or clear independent child bans.
pub(super) fn reopen_route_at(
    state: &mut Value,
    run_id: &str,
    route_id: &str,
    reason: &str,
    at: &str,
) -> V2Result<Value> {
    let route = entity_mut(state, "routes", route_id)?;
    if route["run_id"] != run_id {
        return Err(err("LAB_WRONG_RUN", "路线不属于此运行"));
    }
    let previous = route["restriction"].clone();
    route["status"] = json!("active");
    route["restriction"] = json!({"blocked":false,"authority":"human","reason":reason,"created_at":at,"previous":previous});
    route["control_epoch"] = json!(route["control_epoch"].as_u64().unwrap_or(1) + 1);
    changed(route, at);
    Ok(
        json!({"route_id":route_id,"blocked_by_ancestor":route_blocked(state,route_id),"cancel_session_ids":[]}),
    )
}

fn ensure_research_bindings(state: &mut Value, run_id: &str, at: &str) -> V2Result<()> {
    let sessions: Vec<Value> = array(state, "sessions")
        .iter()
        .filter(|session| {
            session["run_id"] == run_id
                && matches!(session["role"].as_str(), Some("main" | "partner"))
        })
        .cloned()
        .collect();
    for session in sessions {
        let sid = session["id"].as_str().unwrap_or_default();
        if session["role"] == "partner" && session["task_id"].is_null() {
            if let Some(task) = array(state, "tasks")
                .iter()
                .rev()
                .find(|task| {
                    task["owner_session_id"] == sid
                        && !matches!(task["state"].as_str(), Some("cancelled" | "superseded"))
                })
                .cloned()
            {
                let current = entity_mut(state, "sessions", sid)?;
                current["task_id"] = task["id"].clone();
                current["assignment_epoch"] = json!(task["task_epoch"].as_u64().unwrap_or(1));
            }
        }
        // A deliberately detached main session may perform one arrangement after a route ban.
        if session["route_id"].is_null() && session["route_blocked"] != true {
            let inherited = array(state, "sessions")
                .iter()
                .find(|other| other["run_id"] == run_id && other["role"] == "main")
                .and_then(session_route)
                .map(str::to_owned);
            let route_id = if session["role"] == "partner" && inherited.is_some() {
                inherited.unwrap_or_default()
            } else {
                let route_id = id();
                push(
                    state,
                    "routes",
                    json!({"id":route_id,"run_id":run_id,"problem_version":state["problem_version"],"title":session["focus"].as_str().unwrap_or("初始研究路线"),"goal_relation":"探索完整原始问题","parent_route_ids":[],"status":"active","restriction":null,"control_epoch":1,"revision":1,"created_at":at}),
                );
                route_id
            };
            entity_mut(state, "sessions", sid)?["route_id"] = json!(route_id);
        }
        let current = entity(state, "sessions", sid).unwrap().clone();
        if let Some(task_id) = current["task_id"].as_str() {
            entity_mut(state, "tasks", task_id)?["route_id"] = current["route_id"].clone();
        }
    }
    Ok(())
}

fn authorize(role: &str, action: &str) -> V2Result<()> {
    let allowed = match action {
        "register_route" | "set_route" => matches!(role, "main" | "partner"),
        "assign_partner" | "withdraw_assignment" | "stop_partner" | "request_advisor" => {
            role == "main"
        }
        _ => true,
    };
    if allowed {
        Ok(())
    } else {
        Err(err(
            "LAB_ROLE_FORBIDDEN",
            format!("{role} 无权执行 {action}"),
        ))
    }
}

/// Reassignment within an existing slot is free; reopening a closed partner needs a slot.
pub(super) fn ensure_partner_capacity(
    state: &Value,
    run_id: &str,
    reopening_id: Option<&str>,
) -> V2Result<()> {
    if reopening_id
        .and_then(|sid| entity(state, "sessions", sid))
        .is_some_and(|session| session["role"] != "partner" || session["state"] != "closed")
    {
        return Ok(());
    }
    let run = entity(state, "runs", run_id).ok_or_else(|| err("NOT_FOUND", "运行不存在"))?;
    let limit = run["limits"]["max_partners"].as_u64().unwrap_or(2).min(5);
    let active = array(state, "sessions")
        .iter()
        .filter(|session| {
            session["run_id"] == run_id
                && session["role"] == "partner"
                && session["state"] != "closed"
        })
        .count() as u64;
    if active >= limit {
        return Err(err(
            "PARTNER_LIMIT",
            "伙伴猫名额已满；先结束现有伙伴，再创建或恢复已关闭伙伴",
        ));
    }
    Ok(())
}

pub(super) fn apply_action_at(
    state: &mut Value,
    run_id: &str,
    session_id: &str,
    action: &Value,
    _artifact: &Value,
    model: &Option<String>,
    at: &str,
) -> V2Result<Option<Value>> {
    let kind = action["type"]
        .as_str()
        .or_else(|| action["action"].as_str())
        .unwrap_or_default();
    if kind == "request_partner"
        && entity(state, "sessions", session_id)
            .is_some_and(|session| session["route_blocked"] == true)
    {
        return Err(err(
            "ROUTE_BLOCKED",
            "禁止路线后的整体安排应先选择既有允许路线，不能通过新建伙伴绕过禁令",
        ));
    }
    if entity(state, "sessions", session_id).is_some_and(|session| {
        matches!(
            session["role"].as_str(),
            Some("advisor" | "memory" | "display")
        )
    }) {
        return Err(err(
            "LAB_ROLE_FORBIDDEN",
            "后台角色只能提交本角色的 background_result，无研究安排与事实写入权限",
        ));
    }
    if !matches!(
        kind,
        "register_route"
            | "set_route"
            | "assign_partner"
            | "withdraw_assignment"
            | "stop_partner"
            | "request_advisor"
    ) {
        return Ok(None);
    }
    let author = entity(state, "sessions", session_id)
        .ok_or_else(|| err("NOT_FOUND", "会话不存在"))?
        .clone();
    if author["run_id"] != run_id {
        return Err(err("LAB_WRONG_RUN", "会话不属于此运行"));
    }
    authorize(author["role"].as_str().unwrap_or_default(), kind)?;
    if matches!(
        kind,
        "assign_partner" | "withdraw_assignment" | "stop_partner" | "request_advisor"
    ) && author["phase"] == "research"
    {
        transition(state, session_id, "coordination", "arrangement_action", at)?;
    }
    match kind {
        "register_route" => {
            let title = text_field(action, "title")?;
            let parents: Vec<String> = if action["parent_route_ids"].is_array() {
                array(action, "parent_route_ids")
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| err("LAB_INVALID_ROUTE", "路线来源必须为ID"))
                    })
                    .collect::<V2Result<_>>()?
            } else {
                session_route(&author)
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            };
            for parent in &parents {
                if entity(state, "routes", parent).is_none_or(|route| route["run_id"] != run_id) {
                    return Err(err("LAB_INVALID_ROUTE", "来源路线不属于当前运行"));
                }
                if route_blocked(state, parent) {
                    return Err(err("ROUTE_BLOCKED", "禁止派生被禁路线"));
                }
            }
            if author["route_blocked"] == true && parents.is_empty() {
                return Err(err(
                    "ROUTE_BLOCKED",
                    "被禁路线不能通过改名创建新路线；需使用既有未禁路线或人类解除限制",
                ));
            }
            let route_id = action["route_id"]
                .as_str()
                .filter(|value| !value.is_empty())
                .map_or_else(id, str::to_owned);
            if let Some(route) = entity(state, "routes", &route_id) {
                if route["run_id"] == run_id
                    && route["title"] == title
                    && route["parent_route_ids"] == json!(parents)
                {
                    return Ok(Some(json!({"route_id":route_id,"duplicate":true})));
                }
                return Err(err("LAB_ROUTE_CONFLICT", "路线 ID 已用于不同内容"));
            }
            push(
                state,
                "routes",
                json!({"id":route_id,"run_id":run_id,"problem_version":state["problem_version"],"title":title,"goal_relation":action["goal_relation"],"intention":action["intention"],"obstacle":action["obstacle"],"evidence_refs":action["evidence_refs"],"parent_route_ids":parents,"status":"active","restriction":null,"control_epoch":1,"created_by":session_id,"revision":1,"created_at":at}),
            );
            Ok(Some(json!({"route_id":route_id,"cancel_session_ids":[]})))
        }
        "set_route" => {
            let route_id = text_field(action, "route_id")?;
            if entity(state, "routes", route_id).is_none_or(|route| route["run_id"] != run_id) {
                return Err(err("LAB_INVALID_ROUTE", "路线不属于此运行"));
            }
            if route_blocked(state, route_id) {
                return Err(err("ROUTE_BLOCKED", "路线被禁止"));
            }
            let current = entity_mut(state, "sessions", session_id)?;
            current["route_id"] = json!(route_id);
            current["route_blocked"] = json!(false);
            changed(current, at);
            if let Some(task_id) = current["task_id"].as_str().map(str::to_owned) {
                let task = entity_mut(state, "tasks", &task_id)?;
                task["route_id"] = json!(route_id);
                changed(task, at);
            }
            Ok(Some(json!({"route_id":route_id,"cancel_session_ids":[]})))
        }
        "assign_partner" | "withdraw_assignment" | "stop_partner" => {
            let target_id = action["partner_session_id"]
                .as_str()
                .or_else(|| action["session_id"].as_str())
                .ok_or_else(|| err("LAB_INVALID_CONTROL", "缺少 partner_session_id"))?;
            let target = entity(state, "sessions", target_id)
                .ok_or_else(|| err("NOT_FOUND", "伙伴猫不存在"))?
                .clone();
            if target["role"] != "partner" || target["run_id"] != run_id {
                return Err(err("LAB_WRONG_TARGET", "只能安排当前运行的伙伴猫"));
            }
            if kind == "assign_partner" {
                ensure_partner_capacity(state, run_id, Some(target_id))?;
                let focus = text_field(action, "focus")?;
                let route = action["route_id"]
                    .as_str()
                    .or_else(|| session_route(&target))
                    .or_else(|| session_route(&author));
                if route.is_some_and(|route| {
                    entity(state, "routes", route).is_none_or(|route| route["run_id"] != run_id)
                        || route_blocked(state, route)
                }) {
                    return Err(err(
                        "ROUTE_BLOCKED",
                        "安排路线不存在、不属于当前运行或被禁止",
                    ));
                }
                let old = target["pending_assignment_id"]
                    .as_str()
                    .and_then(|identifier| entity(state, "pending_assignments", identifier))
                    .cloned();
                if let Some(old) = old.as_ref().filter(|old| {
                    old["state"] == "pending"
                        && old["focus"] == focus
                        && old["route_id"] == json!(route)
                }) {
                    return Ok(Some(
                        json!({"pending_assignment_id":old["id"],"duplicate":true,"cancel_session_ids":[]}),
                    ));
                }
                if let Some(old) = old {
                    let pending = entity_mut(
                        state,
                        "pending_assignments",
                        old["id"].as_str().unwrap_or_default(),
                    )?;
                    pending["state"] = json!("superseded");
                    changed(pending, at);
                }
                let identifier = id();
                let assignment_revision = target["assignment_revision"].as_u64().unwrap_or(0) + 1;
                push(
                    state,
                    "pending_assignments",
                    json!({"id":identifier,"run_id":run_id,"partner_session_id":target_id,"author_session_id":session_id,"focus":focus,"route_id":route,"goal_relation":action["goal_relation"],"context_refs":action["context_refs"],"state":"pending","assignment_revision":assignment_revision,"replaces_task_id":target["task_id"],"revision":1,"created_at":at}),
                );
                let target = entity_mut(state, "sessions", target_id)?;
                target["pending_assignment_id"] = json!(identifier);
                target["assignment_revision"] = json!(assignment_revision);
                if target["invocation_in_flight"] != true && target["state"] != "active" {
                    target["state"] = json!("idle");
                }
                changed(target, at);
                Ok(Some(
                    json!({"pending_assignment_id":identifier,"assignment_revision":assignment_revision,"effective":"next_dispatch","cancel_session_ids":[]}),
                ))
            } else {
                if let Some(identifier) = target["pending_assignment_id"].as_str() {
                    let pending = entity_mut(state, "pending_assignments", identifier)?;
                    pending["state"] = json!(if kind == "withdraw_assignment" {
                        "withdrawn"
                    } else {
                        "cancelled"
                    });
                    changed(pending, at);
                }
                let current = entity_mut(state, "sessions", target_id)?;
                current["pending_assignment_id"] = Value::Null;
                if kind == "stop_partner" {
                    current["state"] = json!("closed");
                    current["control_epoch"] =
                        json!(current["control_epoch"].as_u64().unwrap_or(1) + 1);
                    current["continuation"] = Value::Null;
                }
                changed(current, at);
                if kind == "stop_partner" {
                    if let Some(task_id) = target["task_id"].as_str() {
                        let task = entity_mut(state, "tasks", task_id)?;
                        task["state"] = json!("cancelled");
                        task["task_epoch"] = json!(task["task_epoch"].as_u64().unwrap_or(1) + 1);
                        changed(task, at);
                    }
                }
                Ok(Some(
                    json!({"partner_session_id":target_id,"cancel_session_ids":if kind=="stop_partner" {vec![target_id]} else {vec![]}}),
                ))
            }
        }
        "request_advisor" => {
            let run = entity(state, "runs", run_id).unwrap().clone();
            let created = background_session(
                state,
                &run,
                "advisor",
                model,
                &source_refs(state, run_id),
                "requested",
                at,
            );
            Ok(Some(
                json!({"created":created,"coalesced":!created,"cancel_session_ids":[]}),
            ))
        }
        _ => unreachable!(),
    }
}

pub(super) fn checkpoint_at(
    state: &mut Value,
    session_id: &str,
    checkpoint: &Value,
    artifact: &Value,
    at: &str,
) -> V2Result<Option<String>> {
    if checkpoint.is_null() {
        return Ok(None);
    }
    for key in [
        "proof_goal",
        "local_assumptions",
        "symbols",
        "unfinished_steps",
        "next_step",
    ] {
        if !(checkpoint[key].is_string()
            || checkpoint[key].is_array()
            || checkpoint[key].is_object())
            || checkpoint[key]
                .as_str()
                .is_some_and(|text| text.trim().is_empty())
        {
            return Err(err(
                "LAB_INCOMPLETE_CHECKPOINT",
                format!("交接缺少 {key}；无法恢复时必须复核"),
            ));
        }
    }
    let session = entity(state, "sessions", session_id).unwrap().clone();
    if let Some(artifact_id) = artifact["id"].as_str() {
        if let Some(existing) = array(state, "proof_checkpoints").iter().find(|existing| {
            existing["session_id"] == session_id && existing["artifact_id"] == artifact_id
        }) {
            if [
                "proof_goal",
                "local_assumptions",
                "symbols",
                "unfinished_steps",
                "next_step",
                "draft_refs",
            ]
            .iter()
            .any(|field| existing[*field] != checkpoint[*field])
            {
                return Err(err("LAB_CHECKPOINT_CONFLICT", "同一输出的交接内容发生冲突"));
            }
            return Ok(existing["id"].as_str().map(str::to_owned));
        }
    }
    let identifier = id();
    push(
        state,
        "proof_checkpoints",
        json!({"id":identifier,"run_id":session["run_id"],"session_id":session_id,"task_id":session["task_id"],"assignment_epoch":session["assignment_epoch"],"route_id":session["route_id"],"proof_goal":checkpoint["proof_goal"],"local_assumptions":checkpoint["local_assumptions"],"symbols":checkpoint["symbols"],"unfinished_steps":checkpoint["unfinished_steps"],"next_step":checkpoint["next_step"],"draft_refs":checkpoint["draft_refs"],"artifact_id":artifact["id"],"review_status":"unreviewed","requires_recheck":true,"created_at":at}),
    );
    entity_mut(state, "sessions", session_id)?["latest_checkpoint_id"] = json!(identifier);
    Ok(Some(identifier))
}

fn background_result_at(
    state: &mut Value,
    session_id: &str,
    result: &Value,
    artifact: &Value,
    at: &str,
) -> V2Result<Value> {
    let session = entity(state, "sessions", session_id).unwrap().clone();
    let role = session["role"].as_str().unwrap_or_default();
    let job_id = text_field(&session, "background_job_id")?;
    let job = entity(state, "background_jobs", job_id)
        .ok_or_else(|| err("LAB_JOB_MISSING", "后台任务不存在"))?
        .clone();
    if job["state"] == "completed" {
        return Ok(json!({"duplicate":true}));
    }
    if !result.is_object() {
        return Err(err(
            "LAB_BACKGROUND_PROTOCOL",
            "后台角色必须返回 background_result 对象",
        ));
    }
    let sources = array(&job, "source_refs");
    let check_refs = |refs: &Value| -> V2Result<()> {
        if !refs.is_array() {
            return Err(err("LAB_BACKGROUND_PROTOCOL", "来源引用必须是数组"));
        }
        if refs
            .as_array()
            .unwrap()
            .iter()
            .any(|reference| !sources.contains(reference))
        {
            return Err(err("LAB_SOURCE_MISMATCH", "摘要引用了任务快照外的来源"));
        }
        Ok(())
    };
    match role {
        "advisor" => {
            text_field(result, "observation")?;
            text_field(result, "uncertainty")?;
            check_refs(&result["evidence_refs"])?;
            let identifier = id();
            let priority = if result["priority"] == "urgent" {
                "urgent"
            } else {
                "normal"
            };
            push(
                state,
                "advisories",
                json!({"id":identifier,"run_id":session["run_id"],"session_id":session_id,"job_id":job_id,"observation":result["observation"],"evidence_refs":result["evidence_refs"],"concern":result["concern"],"suggested_change":result["suggested_change"],"uncertainty":result["uncertainty"],"attempt_refs":result["attempt_refs"],"assumption_changes":result["assumption_changes"],"method_changes":result["method_changes"],"new_evidence_refs":result["new_evidence_refs"],"repeat_assessment":result["repeat_assessment"],"priority":priority,"snapshot_revision":job["snapshot_revision"],"artifact_id":artifact["id"],"state":"unhandled","created_at":at}),
            );
            push(
                state,
                "messages",
                json!({"id":id(),"run_id":session["run_id"],"sender_session_id":session_id,"target_role":"main","kind":"advisor_feedback","priority":priority,"state":"queued","advisory_id":identifier,"body":result,"trust":"unreviewed_advice","created_at":at}),
            );
        }
        "memory" => {
            if !result["entries"].is_array() {
                return Err(err("LAB_BACKGROUND_PROTOCOL", "记忆猫需返回 entries 数组"));
            }
            // Resolve the entire batch against the model's frozen input, never live revisions.
            let entries: Vec<(&Value, Vec<Value>)> = array(result, "entries")
                .iter()
                .map(|entry| {
                    text_field(entry, "summary")?;
                    check_refs(&entry["source_refs"])?;
                    if array(entry, "source_refs").is_empty() {
                        return Err(err("LAB_SOURCE_MISMATCH", "每条记忆必须有准确来源"));
                    }
                    let exact = array(entry, "source_refs")
                        .iter()
                        .map(|reference| {
                            let mut frozen = array(&job, "source_snapshot")
                                .iter()
                                .find(|source| source["source"]["id"] == *reference)
                                .cloned()
                                .ok_or_else(|| {
                                    err(
                                        "LAB_SOURCE_MISMATCH",
                                        "记忆引用缺少冻结原文，不能用实时节点补替",
                                    )
                                })?;
                            frozen["source_revision"] = frozen["source"]["revision"].clone();
                            frozen["source_sha256"] = frozen["source"]["sha256"].clone();
                            Ok(frozen)
                        })
                        .collect::<V2Result<Vec<_>>>()?;
                    Ok((entry, exact))
                })
                .collect::<V2Result<_>>()?;
            for (entry, exact) in entries {
                push(
                    state,
                    "memory_entries",
                    json!({"id":id(),"run_id":session["run_id"],"session_id":session_id,"job_id":job_id,"summary":entry["summary"],"source_refs":entry["source_refs"],"exact_sources":exact,"tags":entry["tags"],"attempt_outcome":entry["attempt_outcome"],"goal_relation":entry["goal_relation"],"prerequisite_refs":entry["prerequisite_refs"],"snapshot_revision":job["snapshot_revision"],"artifact_id":artifact["id"],"trust":"summary_only","requires_source_read":true,"created_at":at}),
                );
            }
        }
        "display" => {
            text_field(result, "title")?;
            text_field(result, "body")?;
            check_refs(&result["source_refs"])?;
            push(
                state,
                "display_summaries",
                json!({"id":id(),"run_id":session["run_id"],"session_id":session_id,"job_id":job_id,"title":result["title"],"body":result["body"],"source_refs":result["source_refs"],"event_range":result["event_range"],"actor_refs":result["actor_refs"],"route_refs":result["route_refs"],"evidence_refs":result["evidence_refs"],"statement_refs":result["statement_refs"],"source_revision":job["snapshot_revision"],"artifact_id":artifact["id"],"generated_at":at,"freshness":"snapshot","trust":"summary_only"}),
            );
        }
        _ => return Err(err("LAB_ROLE_FORBIDDEN", "此角色不能提交后台摘要")),
    }
    let job = entity_mut(state, "background_jobs", job_id)?;
    job["state"] = json!("completed");
    job["completed_at"] = json!(at);
    job["artifact_id"] = artifact["id"].clone();
    changed(job, at);
    let current = entity_mut(state, "sessions", session_id)?;
    current["state"] = json!("closed");
    changed(current, at);
    Ok(json!({"job_id":job_id,"state":"completed"}))
}

/// Runs after existing candidate/finding actions. It does not override a successful/ended run.
pub(super) fn after_turn_at(
    state: &mut Value,
    run_id: &str,
    session_id: &str,
    control: &Value,
    artifact: &Value,
    at: &str,
) -> V2Result<Value> {
    let session = entity(state, "sessions", session_id)
        .ok_or_else(|| err("NOT_FOUND", "会话不存在"))?
        .clone();
    let role = session["role"].as_str().unwrap_or_default();
    if matches!(role, "advisor" | "memory" | "display") {
        return background_result_at(
            state,
            session_id,
            &control["background_result"],
            artifact,
            at,
        );
    }
    if !matches!(role, "main" | "partner") {
        return Ok(json!({"changed":false}));
    }
    let run = entity(state, "runs", run_id)
        .ok_or_else(|| err("NOT_FOUND", "运行不存在"))?
        .clone();
    if run["state"] == "ended" || run["result_state"] == "solved" {
        return Ok(json!({"changed":false}));
    }
    ensure_research_bindings(state, run_id, at)?;
    let is_new = control["protocol_version"] == "2.2"
        || control["phase_result"].is_object()
        || control["partner_continuation"].is_object()
        || control["checkpoint"].is_object();
    let checkpoint_id = checkpoint_at(state, session_id, &control["checkpoint"], artifact, at)?;
    let actions = array(control, "actions");
    let meaningful = checkpoint_id.is_some()
        || !actions.is_empty()
        || control["phase_result"].is_object()
        || control["partner_continuation"].is_object();
    let current = entity_mut(state, "sessions", session_id)?;
    current["protocol_mode"] = json!(if is_new { "lab_2_2" } else { "legacy_2_1" });
    current["empty_control_turns"] = json!(if meaningful {
        0
    } else {
        current["empty_control_turns"].as_u64().unwrap_or(0) + 1
    });
    if meaningful {
        current["protocol_feedback"] = Value::Null;
    } else {
        current["protocol_feedback"] = json!(
            "本次没有可执行控制。请在下一次返回五项checkpoint，并明确phase_result或partner_continuation；旧版无控制输出仅兼容，不表示数学进展。"
        );
    }
    let empty_count = current["empty_control_turns"].as_u64().unwrap_or(0);
    // Progress is based on declared mathematical obstacles/evidence, never output size.
    if let Some(progress) = control["progress"].as_object() {
        let signature = json!({"obstacle":progress.get("obstacle"),"evidence_refs":progress.get("evidence_refs"),"method":progress.get("method"),"assumptions":progress.get("assumptions")});
        let repeated = current["progress_signature"] == signature;
        current["repeat_count"] = json!(if repeated {
            current["repeat_count"].as_u64().unwrap_or(0) + 1
        } else {
            0
        });
        current["progress_signature"] = signature;
        if current["repeat_count"].as_u64().unwrap_or(0) >= 2 {
            current["progress_warning"] = json!(
                "连续阶段报告相同障碍、方法和证据；请在整体安排中核对尝试记录，是否继续由研究员判断。"
            );
        } else {
            current["progress_warning"] = Value::Null;
        }
    }
    changed(current, at);
    if role == "partner" {
        let pending = entity(state, "sessions", session_id).unwrap()["pending_assignment_id"]
            .as_str()
            .and_then(|identifier| entity(state, "pending_assignments", identifier))
            .is_some_and(|assignment| assignment["state"] == "pending");
        let decision = control["partner_continuation"]["decision"].as_str();
        if pending || decision == Some("continue") {
            ensure_partner_capacity(state, run_id, Some(session_id))?;
        }
        if decision.is_some_and(|decision| !matches!(decision, "continue" | "wait" | "complete")) {
            return Err(err(
                "LAB_INVALID_CONTINUATION",
                "continuation 必须为 continue/wait/complete",
            ));
        }
        let current = entity_mut(state, "sessions", session_id)?;
        if pending {
            current["state"] = json!("idle");
            current["continuation"] = json!({"decision":"pending_assignment","supersedes":control["partner_continuation"]});
        } else if let Some(decision) = decision {
            current["state"] = json!(match decision {
                "continue" => "idle",
                "complete" => "closed",
                _ => "waiting",
            });
            current["continuation"] = control["partner_continuation"].clone();
        } else if is_new {
            current["state"] = json!("waiting");
            current["protocol_feedback"] =
                json!("伙伴猫需明确 continuation；原始成果已保存，请补齐继续/等待/完成决定。");
        }
        // Legacy continue_partner/close_partner remains authoritative when no new decision exists.
        if let Some(task_id) = current["task_id"].as_str().map(str::to_owned) {
            let state_name = current["state"].as_str().unwrap_or("waiting").to_owned();
            let task = entity_mut(state, "tasks", &task_id)?;
            if !matches!(task["state"].as_str(), Some("cancelled" | "superseded")) {
                task["state"] = json!(if state_name == "closed" {
                    "completed"
                } else {
                    "queued"
                });
                changed(task, at);
            }
        }
        acknowledge_messages(state, session_id, control, at)?;
        return Ok(
            json!({"changed":true,"pending_assignment":pending,"checkpoint_id":checkpoint_id}),
        );
    }
    let current = entity(state, "sessions", session_id).unwrap().clone();
    let phase = current["phase"].as_str().unwrap_or("research");
    let requested = control["phase_result"]["next_phase"].as_str();
    if requested.is_some_and(|phase| !matches!(phase, "research" | "coordination" | "waiting")) {
        return Err(err("LAB_INVALID_PHASE", "phase_result.next_phase 无效"));
    }
    if !super::whiteboard24::blocked(state, &current) && current["route_blocked"] != true {
        super::whiteboard24::validate_wait(state, run_id, &control["phase_result"]["next_phase"])?;
    }
    let research_soft = run["limits"]["research_soft_seconds"]
        .as_u64()
        .unwrap_or(1200);
    if current["route_blocked"] == true {
        transition(state, session_id, "waiting", "no_permitted_route", at)?;
        entity_mut(state, "sessions", session_id)?["state"] = json!("waiting");
    } else if phase == "research"
        && requested == Some("research")
        && elapsed(&current["phase_started_at"], at) >= research_soft
    {
        transition(
            state,
            session_id,
            "coordination",
            "soft_period_reached_at_natural_return",
            at,
        )?;
    } else if let Some(next) = requested {
        transition(
            state,
            session_id,
            next,
            control["phase_result"]["reason"]
                .as_str()
                .unwrap_or("researcher_choice"),
            at,
        )?;
        let targets = super::whiteboard24::waiting_targets(state, run_id);
        let current = entity_mut(state, "sessions", session_id)?;
        current["wait_for"] = control["phase_result"]["wait_for"].clone();
        current["host_wait_targets"] = if next == "waiting" {
            json!(targets)
        } else {
            Value::Null
        };
        current["state"] = json!(if next == "waiting" { "waiting" } else { "idle" });
    } else if phase == "urgent_attention" {
        let next = if current["resume_phase"] == "coordination" {
            "coordination"
        } else {
            "research"
        };
        transition(state, session_id, next, "urgent_attention_completed", at)?;
    } else {
        let key = if phase == "research" {
            "research_soft_seconds"
        } else {
            "coordination_soft_seconds"
        };
        let soft = run["limits"][key]
            .as_u64()
            .unwrap_or(if phase == "research" { 1200 } else { 120 });
        if elapsed(&current["phase_started_at"], at) >= soft {
            transition(
                state,
                session_id,
                if phase == "research" {
                    "coordination"
                } else {
                    "research"
                },
                "soft_period_reached_at_natural_return",
                at,
            )?;
        }
    }
    if empty_count >= 3 {
        transition(state, session_id, "waiting", "repeated_empty_control", at)?;
        let current = entity_mut(state, "sessions", session_id)?;
        current["state"] = json!("waiting");
        current["last_error"] = json!({"code":"LAB_PROTOCOL_STALLED","message":"连续三次缺少研究控制，保留原始成果并等待显式恢复"});
    } else if empty_count >= 2 {
        transition(
            state,
            session_id,
            "coordination",
            "empty_control_recovery",
            at,
        )?;
    }
    if phase == "urgent_attention" {
        let current = entity_mut(state, "sessions", session_id)?;
        current["pending_urgent"] = json!(false);
        current["urgent_cancel_requested_for"] = Value::Null;
    }
    acknowledge_messages(state, session_id, control, at)?;
    Ok(
        json!({"changed":true,"checkpoint_id":checkpoint_id,"phase":entity(state,"sessions",session_id).unwrap()["phase"]}),
    )
}

pub(super) fn fail_turn_at(
    state: &mut Value,
    session_id: &str,
    code: &str,
    at: &str,
) -> V2Result<Value> {
    super::collaboration23::failed_delivery(state, session_id, code)?;
    let session = entity(state, "sessions", session_id)
        .ok_or_else(|| err("NOT_FOUND", "会话不存在"))?
        .clone();
    for identifier in array(&session, "inflight_message_ids")
        .iter()
        .filter_map(Value::as_str)
    {
        if let Ok(message) = entity_mut(state, "messages", identifier) {
            if message["state"] == "delivered" {
                message["state"] = json!("queued");
                message["status"] = json!("queued");
                changed(message, at);
            }
        }
    }
    entity_mut(state, "sessions", session_id)?["inflight_message_ids"] = json!([]);
    if let Some(job_id) = session["background_job_id"].as_str() {
        let job = entity_mut(state, "background_jobs", job_id)?;
        job["state"] = json!(if code.contains("UNKNOWN") {
            "cancellation_unknown"
        } else {
            "failed"
        });
        job["error_code"] = json!(code);
        job["ended_at"] = json!(at);
        changed(job, at);
        let current = entity_mut(state, "sessions", session_id)?;
        current["state"] = json!("closed");
        changed(current, at);
    } else if session["role"] == "main" {
        if session["pending_urgent"] == true {
            let current = entity_mut(state, "sessions", session_id)?;
            if current["phase"] != "urgent_attention" {
                current["resume_phase"] = current["phase"].clone();
            }
            transition(
                state,
                session_id,
                "urgent_attention",
                "cancelled_for_urgent_delivery",
                at,
            )?;
            entity_mut(state, "sessions", session_id)?["state"] = json!("idle");
        } else if matches!(
            code,
            "TURN_TIMEOUT" | "TIMEOUT" | "PHASE_TIMEOUT" | "DEADLINE_REACHED"
        ) {
            let was_research = session["phase"] == "research";
            transition(
                state,
                session_id,
                if was_research {
                    "coordination"
                } else {
                    "waiting"
                },
                "hard_period_recovery",
                at,
            )?;
            let current = entity_mut(state, "sessions", session_id)?;
            current["state"] = json!(if was_research { "idle" } else { "waiting" });
            current["protocol_feedback"] = json!(
                "上次调用达到硬上限；只能从已保存的草稿和checkpoint恢复，未输出推导不可视为已完成。安排阶段超时需显式恢复，避免一秒超时空转。"
            );
        }
    }
    Ok(json!({"changed":true,"cancel_session_ids":[]}))
}

pub(super) fn prompt(prepared: &Value, role: &str) -> String {
    let lab = if prepared["lab"].is_object() {
        &prepared["lab"]
    } else {
        prepared
    };
    let shared = "你在 MathCat 2.3 中工作。所有数学结论仍需独立审核。候选可直接 submit_candidate，审核异步进行。输出控制文件保留既有 actions，新增 protocol_version=\"2.2\"。checkpoint 必须含 proof_goal、local_assumptions、symbols（含作用范围）、unfinished_steps、next_step，可另含 draft_refs；这是未审查的恢复线索，恢复时读取原文核对。不得把摘要当作证明。运行中无法注入新消息；紧急反馈通过取消后新调用续接。对本轮 messages 返回 message_receipts:[{message_id,disposition:accepted|rejected|deferred,reason}]；收到不等于采纳，紧急不等于可信。";
    let shared = format!(
        "{}\n{}\n{}\n先运行 node .mathcat-tools.mjs context-summary，再定向检索所需精确原文；不要整份输出上下文文件。",
        shared,
        super::collaboration23::prompt(),
        "MathCat 2.4 白板：以下动作都必须作为控制文件 actions 数组中的对象，使用 type 字段指定动作；不能把动作名写成控制文件的顶层字段。成果条目 {type:record_finding,kind:theorem|lemma|proposition|conjecture,statement,title,mathematical_statements:[{kind,title,statement,assumptions,scope}]} 每项写完整精确数学陈述，不写抽象工作进度。摘要不具有来源的数学可信保证。疑点条目 {type:record_open_question,title,statement,conditions,affected_refs,evidence_refs,route_id?} 保存具体障碍；解决条目 {type:resolve_open_question,question_id,expected_revision,resolution,evidence_refs} 保存解决依据（仅研究员报告）。失败尝试条目 {type:record_route_outcome,title,goal,method,conditions,reason,outcome:mathematical_failure|blocked|abandoned|human_stopped|runtime_failure|time_limit,scope,evidence_refs,useful_result_refs,route_id?} 保存失败尝试。字段示意需转换为合法 JSON，数学失败必须有已有依据ID；中断不表示数学错误。"
    );
    let instructions = match role {
        "main" => {
            "你是领研猫，承担研究与实验室整体安排，其他猫不能代替你作全局决定。研究期间持续推进完整问题，在有意义节点或软时限后保存checkpoint并返回 phase_result:{next_phase:research|coordination|waiting,reason,wait_for}。20分钟研究软期/30分钟硬限、2分钟安排软期/5分钟硬限是配置初值；每次自然调用结束不重置同一阶段时钟。协调时处理普通反馈、分工、顾问意见及待审结果；紧急意见也必须评估可靠性。assign_partner:{partner_session_id,focus,route_id?,goal_relation?,context_refs?} 只在伙伴猫下次派发时生效，不取消当前研究；withdraw_assignment 撤回待安排；stop_partner 明确停止；request_advisor 请求独立路线建议。新方向可用 register_route:{title,goal_relation,parent_route_ids?}，set_route:{route_id} 选择已有路线。提出开放目标，让伙伴猫自主选择数学方法。progress 可记录 obstacle/evidence_refs/method/assumptions 供重复检查。"
        }
        "partner" => {
            "你与领研猫拥有相同数学研究能力和完整原题，可以自行选择方法、提出异议及发现新方向，但不修改实验室全局分工。每次自然研究返回保存checkpoint，并给 partner_continuation:{decision:continue|wait|complete,reason,next_focus}；continue 无需领研猫批准。若已有待生效安排，当前成果仍按旧任务接受，下一调用使用新安排，旧continue不能覆盖它。允许直接送审候选，与其他研究员讨论。"
        }
        "advisor" => {
            "你是独立研究顾问猫。读取 background_job.source_snapshot，检查路线与最终目标的联系、重复分工、相同障碍是否有新证据。只给建议，无任务改派/事实认证权限。返回 background_result:{observation,evidence_refs:[],concern,suggested_change,uncertainty,priority:normal|urgent,attempt_refs:[],assumption_changes:[],method_changes:[],new_evidence_refs:[],repeat_assessment}。引用必须来自任务source_refs；没有新研究也要诚实指出观察限制。urgent 只表示投递优先，不代表意见已证实。"
        }
        "memory" => {
            "你是记忆猫。读取 background_job.source_snapshot，将成功/失败尝试、条件和依赖整理为便于检索的摘要，不能认证数学结论或补造缺失条件。返回 background_result:{entries:[{summary,source_refs:[],tags:[],attempt_outcome,goal_relation,prerequisite_refs:[]}]}。每条来源只能引用任务source_refs且不能为空。宿主保存准确原文与身份；摘要不会进入可信事实。"
        }
        "display" => {
            "你是展示猫，本阶段只生成后台摘要。读取 background_job.source_snapshot，概括实验室正在做什么、已有产出和安排，分清待审与已审，不替用户做数学判断。返回 background_result:{title,body,source_refs:[],event_range,actor_refs:[],route_refs:[],evidence_refs:[],statement_refs:[]}。引用必须来自任务source_refs；摘要只用于展示，不能成为证明依据。"
        }
        _ => "你是独立审核猫，只审核冻结候选版本，不修改研究分工。",
    };
    format!(
        "\n\n{shared}\n{instructions}\n领研猫专属：problem_spec.normalization_state=pending 时，首轮先完整阅读原始输入，在控制文件的 actions 数组中追加 {{\"type\":\"frame_problem\",\"expected_revision\":当前problem_spec.revision,\"math_statement\":完整数学题面字符串,\"research_description\":非数学说明字符串,\"problem_source_artifact_ids\":[已实际阅读并用于定义本题的导入材料ID]}}；problem_source_artifact_ids 的元素是 JSON 字符串；题面完全来自原始消息时显式填写 []。当前题目输入清单在 .mathcat-context.json 的 problem_input_manifest，只选择定义题目/假设的材料，不把无关参考文献当题面。多份未分类附件时必须明确选择；单一未分类启动附件仅作为未验证输入上下文保守保留。来源清单不赋予任何引理可信保证；这是字段示意，实际必须填写合法 JSON 值，不要创建顶层 frame_problem 字段。把任意混合输入整理为完整数学对象、假设、量词、范围和目标，以及非数学描述。不得增添假设、遗漏边界或把数学限制放入说明。无法可靠分离时保留相关原文并说明歧义，不猜测。原始问题仍为数学目标，整理内容是未独立核对的展示。之后仅人类明确改题才能重定义目标。reframe_goal 意见要求你检查现目标/路线是否有效并通过 actions 中的 command_response 条目回应，不授权自行改原题。人工 collaborative 模式下，任何本轮组织动作及安排完成后的下一阶段都会冻结待批；提交整批 actions 与 phase_result，等待批准，不要伪称已执行。可在 actions 中添加 {{\"type\":\"set_focus\",\"focus\":下一数学目标字符串}} 说明你自己的下一数学目标。普通停止伙伴在当前任务边界生效，禁令仍立即生效。\n当前阶段与可靠控制上下文：\n{}",
        serde_json::to_string_pretty(lab).unwrap_or_default()
    )
}

#[cfg(test)]
#[path = "lab_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "lab_dispatch_tests.rs"]
mod dispatch_tests;
