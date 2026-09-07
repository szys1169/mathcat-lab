//! Human approval is a dispatch barrier, not a global run pause. Mathematical
//! statements and presentation summaries retain independent provenance.
#![allow(clippy::ref_option)] // Match the established lab/new_session model configuration API.
use super::{
    V2Result, V2Service, Value, array, digest, entity, entity_mut, err, id, json, lab, new_session,
    now, push, remaining, revision,
};

pub(super) fn action_type(action: &Value) -> &str {
    action["type"]
        .as_str()
        .or_else(|| action["action"].as_str())
        .unwrap_or_default()
}

pub(super) fn organization(action: &Value) -> bool {
    matches!(
        action_type(action),
        "request_partner"
            | "assign_partner"
            | "withdraw_assignment"
            | "stop_partner"
            | "register_route"
            | "set_route"
            | "set_focus"
    )
}

fn pending<'a>(state: &'a Value, run: &str) -> Option<&'a Value> {
    array(state, "planning_proposals")
        .iter()
        .rev()
        .find(|p| p["run_id"] == run && matches!(p["state"].as_str(), Some("pending" | "deferred")))
}

/// Checked both by selection and by the dispatch transaction to close UI races.
pub(super) fn blocked(state: &Value, session: &Value) -> bool {
    let run = session["run_id"].as_str().unwrap_or_default();
    if session["role"] == "main" && pending(state, run).is_some() {
        return true;
    }
    session["role"] == "partner"
        && session["plan_boundary_reached"] == true
        && pending(state, run)
            .is_some_and(|p| array(p, "affected_session_ids").contains(&session["id"]))
}

/// Concrete work that can produce an update for the main researcher. A model's
/// prose claiming that it submitted a candidate is not a queue entry.
pub(super) fn waiting_targets(state: &Value, run: &str) -> Vec<Value> {
    let mut targets = Vec::new();
    for review in array(state, "reviews") {
        let candidate = review["candidate_id"]
            .as_str()
            .and_then(|id| entity(state, "candidates", id));
        if (review["run_id"] == run || candidate.is_some_and(|c| c["run_id"] == run))
            && matches!(review["state"].as_str(), Some("queued" | "running"))
        {
            targets.push(
                json!({"kind":"review","id":review["id"],"candidate_id":review["candidate_id"]}),
            );
        }
    }
    for session in array(state, "sessions") {
        if session["run_id"] != run || session["role"] != "partner" {
            continue;
        }
        let assigned = array(state, "pending_assignments")
            .iter()
            .any(|a| a["partner_session_id"] == session["id"] && a["state"] == "pending");
        let working = matches!(session["state"].as_str(), Some("idle" | "active"))
            && (session["invocation_in_flight"] == true
                || array(state, "tasks").iter().any(|t| {
                    t["owner_session_id"] == session["id"]
                        && matches!(t["state"].as_str(), Some("queued" | "running"))
                }));
        if assigned || working {
            targets.push(json!({"kind":"partner","id":session["id"]}));
        }
    }
    for job in array(state, "background_jobs") {
        if job["run_id"] == run
            && job["kind"] == "advisor"
            && matches!(job["state"].as_str(), Some("queued" | "running"))
        {
            targets.push(json!({"kind":"advisor","id":job["id"]}));
        }
    }
    for question in array(state, "human_questions") {
        if question["run_id"] == run
            && question["state"] == "open"
            && question["problem_version"] == state["problem_version"]
        {
            targets.push(json!({"kind":"human_question","id":question["id"]}));
        }
    }
    targets
}

pub(super) fn validate_wait(state: &Value, run: &str, next_phase: &Value) -> V2Result<()> {
    if next_phase == "waiting" && waiting_targets(state, run).is_empty() {
        return Err(err(
            "LAB_WAIT_UNAVAILABLE",
            "没有可等待的实际审查、伙伴任务、顾问任务或人类问题。请依据动作回执修正未成功提交的内容，不能宣称已进入复审等待。",
        ));
    }
    Ok(())
}

fn release_hold(state: &mut Value, run: &str, next_phase: &str) {
    for s in state["sessions"].as_array_mut().into_iter().flatten() {
        if s["run_id"] != run {
            continue;
        }
        s["pending_plan_id"] = Value::Null;
        if s["role"] == "main" && s["phase"] == "waiting_plan" {
            s["phase"] = json!(next_phase);
            s["phase_started_at"] = json!(now());
            if s["state"] == "waiting" && next_phase != "waiting" {
                s["state"] = json!("idle");
            }
        }
        if s["plan_boundary_reached"] == true {
            s["plan_boundary_reached"] = json!(false);
            if s["state"] == "waiting" {
                s["state"] = json!("idle");
            }
        }
    }
}

pub(super) fn invalidate(state: &mut Value, run: &str, reason: &str) {
    let plan_ids: Vec<Value> = array(state, "planning_proposals")
        .iter()
        .filter(|p| {
            p["run_id"] == run && matches!(p["state"].as_str(), Some("pending" | "deferred"))
        })
        .map(|p| p["id"].clone())
        .collect();
    for p in state["planning_proposals"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        if p["run_id"] == run && matches!(p["state"].as_str(), Some("pending" | "deferred")) {
            p["state"] = json!("stale");
            p["invalidation_reason"] = json!(reason);
            p["decided_at"] = json!(now());
            revision(p);
        }
    }
    for receipt in state["action_receipts"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        if plan_ids.contains(&receipt["planning_proposal_id"])
            && receipt["status"] == "pending_approval"
        {
            receipt["status"] = json!("stale");
            receipt["invalidation_reason"] = json!(reason);
        }
    }
    release_hold(state, run, "coordination");
}

pub(super) fn tick(state: &mut Value, run: &str) {
    let Some(plan) = pending(state, run).cloned() else {
        return;
    };
    let Some(r) = entity(state, "runs", run) else {
        return;
    };
    if r["state"] != "running" {
        return;
    }
    let urgent = array(state, "sessions")
        .iter()
        .any(|s| s["run_id"] == run && s["role"] == "main" && s["pending_urgent"] == true);
    if plan["problem_version"] != state["problem_version"]
        || plan["control_epoch"] != r["control_epoch"]
        || urgent
    {
        invalidate(
            state,
            run,
            if urgent {
                "urgent_feedback"
            } else {
                "control_changed"
            },
        );
    }
}

pub(super) fn stage(
    state: &mut Value,
    run: &str,
    session: &str,
    control: &Value,
    artifact: &Value,
    model: &Option<String>,
) -> V2Result<bool> {
    let r = entity(state, "runs", run)
        .cloned()
        .ok_or_else(|| err("NOT_FOUND", "运行不存在"))?;
    let s = entity(state, "sessions", session)
        .cloned()
        .ok_or_else(|| err("NOT_FOUND", "会话不存在"))?;
    if s["role"] != "main" {
        return Ok(false);
    }
    let batch_hash = digest(&control.to_string());
    if let Some(old) = array(state, "planning_batches")
        .iter()
        .find(|b| b["source_artifact_id"] == artifact["id"] && b["session_id"] == session)
    {
        if old["control_hash"] != batch_hash {
            return Err(err("CONTROL_INVALID", "同一输出对应不同分工内容"));
        }
        return Ok(old["staged"] == true);
    }
    let batch = json!({"id":id(),"run_id":run,"session_id":session,"source_artifact_id":artifact["id"],"control_hash":batch_hash,"mode":r["mode"],"accepted_at":now(),"staged":false});
    if r["mode"] != "collaborative" {
        push(state, "planning_batches", batch);
        return Ok(false);
    }
    let actions: Vec<Value> = array(control, "actions")
        .iter()
        .filter(|a| organization(a))
        .cloned()
        .collect();
    if control["phase_result"]["next_phase"]
        .as_str()
        .is_some_and(|p| !matches!(p, "research" | "coordination" | "waiting"))
    {
        return Err(err("LAB_INVALID_PHASE", "方案下一阶段无效"));
    }
    let completing_coordination = matches!(
        artifact["execution_capture"]["phase"].as_str(),
        Some("coordination" | "urgent_attention")
    ) && control["phase_result"]["next_phase"] != "coordination";
    if actions.is_empty() && !completing_coordination {
        push(state, "planning_batches", batch);
        return Ok(false);
    }
    if let Some(old) = pending(state, run) {
        if old["source_artifact_id"] == artifact["id"] {
            return Ok(true);
        }
        return Err(err(
            "PLAN_PENDING",
            "已有待批准分工，不能覆盖人工正在阅读的方案",
        ));
    }
    // Validate the complete batch on a clone. No partial organizational change
    // can escape when a later action is invalid or exceeds the partner limit.
    let mut preview = state.clone();
    for action in &actions {
        super::collaboration23::validate_source_commands(&preview, run, session, action)?;
        execute_organization(&mut preview, run, session, action, model)?;
    }
    validate_wait(&preview, run, &control["phase_result"]["next_phase"])?;
    let affected: Vec<Value> = actions
        .iter()
        .filter_map(|a| {
            a["partner_session_id"]
                .as_str()
                .or_else(|| a["session_id"].as_str())
                .map(|v| json!(v))
        })
        .collect();
    let affected_versions: Vec<Value> = affected.iter().filter_map(Value::as_str).filter_map(|sid|entity(state,"sessions",sid)).map(|s|json!({"id":s["id"],"control_epoch":s["control_epoch"],"assignment_revision":s["assignment_revision"],"task_id":s["task_id"],"task_epoch":s["task_id"].as_str().and_then(|tid|entity(state,"tasks",tid)).map(|t|t["task_epoch"].clone())})).collect();
    let plan = json!({"id":id(),"run_id":run,"session_id":session,"revision":1,"state":"pending",
        "actions":actions,"affected_session_ids":affected,"source_artifact_id":artifact["id"],
        "affected_versions":affected_versions,
        "problem_version":state["problem_version"],"control_epoch":r["control_epoch"],
        "main_next_phase":control["phase_result"]["next_phase"].as_str().unwrap_or("research"),
        "wait_for":control["phase_result"]["wait_for"],
        "reason":control["phase_result"]["reason"],"created_at":now(),"decided_at":null});
    for affected_id in affected.iter().filter_map(Value::as_str) {
        let target = entity_mut(state, "sessions", affected_id)?;
        target["pending_plan_id"] = plan["id"].clone();
        target["plan_boundary_reached"] =
            json!(target["invocation_in_flight"] != true && target["state"] != "active");
    }
    push(state, "planning_proposals", plan.clone());
    let mut batch = batch;
    batch["staged"] = json!(true);
    batch["planning_proposal_id"] = plan["id"].clone();
    push(state, "planning_batches", batch);
    let main = entity_mut(state, "sessions", session)?;
    main["pending_plan_id"] = plan["id"].clone();
    main["state"] = json!("waiting");
    main["phase"] = json!("waiting_plan");
    main["wait_for"] = json!("planning_approval");
    for (index, action) in actions.iter().enumerate() {
        push(
            state,
            "action_receipts",
            json!({"id":format!("plan:{}:{index}",plan["id"].as_str().unwrap_or_default()),"run_id":run,"session_id":session,"planning_proposal_id":plan["id"],"action":action,"source_command_ids":action.get("source_command_ids").cloned().unwrap_or_else(||json!([])),"status":"pending_approval","result_refs":[],"output_artifact_id":artifact["id"],"created_at":now()}),
        );
    }
    Ok(true)
}

pub(super) fn after_turn(state: &mut Value, run: &str, session: &str) -> V2Result<()> {
    let plan = pending(state, run).cloned();
    let s = entity_mut(state, "sessions", session)?;
    if let Some(plan) = plan {
        if s["role"] == "main" {
            s["state"] = json!("waiting");
            s["phase"] = json!("waiting_plan");
            s["wait_for"] = json!("planning_approval");
            s["pending_plan_id"] = plan["id"].clone();
        } else if array(&plan, "affected_session_ids").contains(&s["id"]) {
            s["plan_boundary_reached"] = json!(true);
            if s["state"] != "closed" {
                s["state"] = json!("waiting");
            }
        }
    }
    if s["stop_after_current_task"] == true {
        s["state"] = json!("closed");
        s["stop_after_current_task"] = json!(false);
        let task = s["task_id"].as_str().map(str::to_owned);
        if let Some(t) = task {
            entity_mut(state, "tasks", &t)?["state"] = json!("completed");
        }
    }
    Ok(())
}

pub(super) fn execute_organization(
    state: &mut Value,
    run: &str,
    session: &str,
    action: &Value,
    model: &Option<String>,
) -> V2Result<Value> {
    let r = entity(state, "runs", run)
        .cloned()
        .ok_or_else(|| err("NOT_FOUND", "运行不存在"))?;
    let author = entity(state, "sessions", session)
        .cloned()
        .ok_or_else(|| err("NOT_FOUND", "领研猫不存在"))?;
    if author["role"] != "main" || author["run_id"] != run {
        return Err(err("INVALID_ROLE", "仅领研猫可安排分工"));
    }
    let kind = action_type(action);
    if kind == "set_focus" {
        let focus = text(action, "focus")?;
        let s = entity_mut(state, "sessions", session)?;
        s["focus"] = json!(focus);
        revision(s);
        return Ok(json!({"session_id":session,"focus":focus}));
    }
    if kind == "request_partner" {
        if author["route_blocked"] == true {
            return Err(err("ROUTE_BLOCKED", "被禁路线不能通过新增伙伴继续"));
        }
        lab::ensure_partner_capacity(state, run, None)?;
        let focus = text(action, "focus")?;
        let route = action["route_id"]
            .as_str()
            .or_else(|| author["route_id"].as_str());
        if let Some(route) = route {
            if entity(state, "routes", route).is_none_or(|v| v["run_id"] != run)
                || lab::route_blocked(state, route)
            {
                return Err(err("ROUTE_BLOCKED", "伙伴路线不存在或已被禁止"));
            }
        }
        let mut partner = new_session(state, &r, "partner", Some(focus), model);
        let task = json!({"id":id(),"run_id":run,"owner_session_id":partner["id"],"kind":"research","focus":focus,
            "route_id":route,"state":"queued","task_epoch":1,"assignment_epoch":1,"deadline_at":r["deadline_at"],"revision":1,"created_at":now()});
        partner["task_id"] = task["id"].clone();
        partner["route_id"] = json!(route);
        partner["assignment_epoch"] = json!(1);
        push(state, "tasks", task.clone());
        push(state, "sessions", partner);
        return Ok(task);
    }
    if kind == "stop_partner" {
        let sid = action["partner_session_id"]
            .as_str()
            .or_else(|| action["session_id"].as_str())
            .ok_or_else(|| err("CONTROL_INVALID", "缺少伙伴ID"))?;
        let s = entity_mut(state, "sessions", sid)?;
        if s["role"] != "partner" || s["run_id"] != run {
            return Err(err("INVALID_ROLE", "只能停止本轮伙伴"));
        }
        if s["invocation_in_flight"] == true || s["state"] == "active" {
            s["stop_after_current_task"] = json!(true);
            revision(s);
            return Ok(
                json!({"partner_session_id":sid,"effective":"after_current_task","cancel_session_ids":[]}),
            );
        }
    }
    lab::apply_action_at(state, run, session, action, &Value::Null, model, &now())?
        .ok_or_else(|| err("CONTROL_INVALID", "不支持的分工动作"))
}

fn text<'a>(value: &'a Value, field: &str) -> V2Result<&'a str> {
    value[field]
        .as_str()
        .filter(|v| !v.trim().is_empty() && v.len() <= 1_000_000)
        .ok_or_else(|| err("INVALID_INPUT", format!("{field} 不能为空或过长")))
}

pub(super) fn replay(state: &Value, key: &str, hash: &str) -> V2Result<Option<Value>> {
    if key.trim().is_empty() {
        return Err(err("IDEMPOTENCY_REQUIRED", "需要幂等键"));
    }
    if let Some(old) = array(state, "whiteboard_operations")
        .iter()
        .find(|v| v["key"] == key)
    {
        if old["hash"] != hash {
            return Err(err("IDEMPOTENCY_CONFLICT", "相同键对应不同修改"));
        }
        return Ok(Some(old["result"].clone()));
    }
    Ok(None)
}

pub(super) fn receipt(state: &mut Value, key: &str, hash: &str, result: &Value) {
    push(
        state,
        "whiteboard_operations",
        json!({"id":id(),"key":key,"hash":hash,"result":result,"created_at":now()}),
    );
}

fn decide(
    state: &mut Value,
    plan_id: &str,
    input: &Value,
    model: &Option<String>,
) -> V2Result<Value> {
    let plan = entity(state, "planning_proposals", plan_id)
        .cloned()
        .ok_or_else(|| err("NOT_FOUND", "分工方案不存在"))?;
    if input["expected_revision"].as_u64().is_none()
        || input["expected_revision"] != plan["revision"]
    {
        return Err(err("REVISION_CONFLICT", "请刷新分工版本后重试"));
    }
    if !matches!(plan["state"].as_str(), Some("pending" | "deferred")) {
        return Err(err("PLAN_NOT_PENDING", "方案已处理或过期"));
    }
    let run = plan["run_id"].as_str().unwrap_or_default();
    let r = entity(state, "runs", run)
        .cloned()
        .ok_or_else(|| err("NOT_FOUND", "运行不存在"))?;
    if r["state"] != "running" || remaining(&r) == 0 || super::unknown_run(state, run) {
        return Err(err(
            "RUN_NOT_RUNNING",
            "暂停、停止、等待具体答复、期限届满或取消未知时不能批准分工",
        ));
    }
    if plan["problem_version"] != state["problem_version"]
        || plan["control_epoch"] != r["control_epoch"]
    {
        return Err(err("REVISION_CONFLICT", "题面或运行控制已改变，请重新安排"));
    }
    let decision = input["decision"].as_str().unwrap_or_default();
    if !matches!(decision, "approve" | "reject" | "defer") {
        return Err(err(
            "INVALID_DECISION",
            "decision 必须是 approve/reject/defer",
        ));
    }
    let session = plan["session_id"].as_str().unwrap_or_default();
    let mut results = Vec::new();
    if decision == "approve" {
        for expected in array(&plan, "affected_versions") {
            let target = entity(
                state,
                "sessions",
                expected["id"].as_str().unwrap_or_default(),
            )
            .ok_or_else(|| err("REVISION_CONFLICT", "待安排伙伴已不存在"))?;
            let task = target["task_id"]
                .as_str()
                .and_then(|tid| entity(state, "tasks", tid));
            if target["control_epoch"] != expected["control_epoch"]
                || target["assignment_revision"] != expected["assignment_revision"]
                || target["task_id"] != expected["task_id"]
                || task.map_or(Value::Null, |t| t["task_epoch"].clone()) != expected["task_epoch"]
            {
                return Err(err("REVISION_CONFLICT", "伙伴任务控制已变化，需要重新安排"));
            }
        }
        for (index, action) in array(&plan, "actions").iter().enumerate() {
            super::collaboration23::validate_source_commands(state, run, session, action)?;
            if array(action, "source_command_ids")
                .iter()
                .filter_map(Value::as_str)
                .any(|key| {
                    entity(state, "commands", key).is_some_and(|c| {
                        matches!(c["status"].as_str(), Some("cancelled" | "superseded"))
                    })
                })
            {
                return Err(err(
                    "REVISION_CONFLICT",
                    "方案依赖的人类意见已经撤回或替代，请重新安排",
                ));
            }
            let before = state.clone();
            let result = execute_organization(state, run, session, action, model)?;
            let refs = super::collaboration23::result_refs(&before, state, &result);
            let record = entity_mut(state, "action_receipts", &format!("plan:{plan_id}:{index}"))?;
            record["status"] = json!("applied");
            record["result"] = result.clone();
            record["result_refs"] = json!(refs);
            record["applied_at"] = json!(now());
            results.push(result);
        }
        entity_mut(state, "sessions", session)?["approved_plan_id"] = json!(plan_id);
    } else if decision == "reject" {
        for receipt in state["action_receipts"]
            .as_array_mut()
            .into_iter()
            .flatten()
        {
            if receipt["planning_proposal_id"] == plan_id {
                receipt["status"] = json!("rejected_by_human");
            }
        }
    }
    let p = entity_mut(state, "planning_proposals", plan_id)?;
    p["state"] = json!(match decision {
        "approve" => "approved",
        "reject" => "rejected",
        _ => "deferred",
    });
    p["decision_reason"] = input["reason"].clone();
    p["decided_at"] = json!(now());
    p["execution_results"] = json!(results);
    revision(p);
    let mut result = p.clone();
    if decision != "defer" {
        // A real review/partner may finish while the human reads the proposal.
        // Approval releases the barrier, but must not create an orphan wait.
        let requested = plan["main_next_phase"].as_str().unwrap_or("research");
        let next = if decision == "approve"
            && (requested != "waiting" || !waiting_targets(state, run).is_empty())
        {
            requested
        } else {
            "coordination"
        };
        release_hold(state, run, next);
        if decision == "approve" {
            let targets = waiting_targets(state, run);
            let main = entity_mut(state, "sessions", session)?;
            main["host_wait_targets"] = if next == "waiting" {
                json!(targets)
            } else {
                Value::Null
            };
            main["wait_for"] = if next == "waiting" {
                plan["wait_for"].clone()
            } else {
                Value::Null
            };
        }
        entity_mut(state, "planning_proposals", plan_id)?["effective_next_phase"] = json!(next);
        result["effective_next_phase"] = json!(next);
        if decision == "reject" {
            push(
                state,
                "messages",
                json!({"id":id(),"run_id":run,"target_role":"main","kind":"planning_decision","priority":"normal","state":"queued","body":result,"created_at":now()}),
            );
        }
    }
    Ok(result)
}

impl V2Service {
    pub async fn planning_decision24(
        &self,
        project: &str,
        plan: &str,
        input: &Value,
        key: &str,
    ) -> V2Result<Value> {
        let hash =
            digest(&json!({"operation":"planning_decision","plan":plan,"input":input}).to_string());
        self.store
            .mutate(project, "planning.decided", None, |state| {
                if let Some(old) = replay(state, key, &hash)? {
                    return Ok(old);
                }
                let result = decide(state, plan, input, &self.config.model)?;
                receipt(state, key, &hash, &result);
                Ok(result)
            })
            .await
    }

    pub async fn set_mode24(
        &self,
        project: &str,
        run: &str,
        input: &Value,
        key: &str,
    ) -> V2Result<Value> {
        let hash = digest(&json!({"operation":"set_mode","run":run,"input":input}).to_string());
        self.store
            .mutate(project, "run.mode_changed", None, |state| {
                if let Some(old) = replay(state, key, &hash)? {
                    return Ok(old);
                }
                let r = entity_mut(state, "runs", run)?;
                if r["revision"] != input["expected_revision"]
                    || input["expected_revision"].as_u64().is_none()
                {
                    return Err(err("REVISION_CONFLICT", "运行版本已变化"));
                }
                if matches!(r["state"].as_str(), Some("ended" | "stopping")) || remaining(r) == 0 {
                    return Err(err("RUN_ENDED", "已结束或期限届满的运行不能切换"));
                }
                let mode = text(input, "mode")?;
                if !matches!(mode, "collaborative" | "delegated") {
                    return Err(err("INVALID_MODE", "mode 必须是 collaborative/delegated"));
                }
                r["mode"] = json!(mode);
                revision(r);
                let result = r.clone();
                // Switching to automatic does not silently execute an old proposal.
                // Replan under current controls; the run state is never resumed here.
                if mode == "delegated" {
                    invalidate(state, run, "mode_changed_to_automatic");
                }
                receipt(state, key, &hash, &result);
                Ok(result)
            })
            .await
    }
}

#[cfg(test)]
#[path = "whiteboard24_tests.rs"]
mod tests;
