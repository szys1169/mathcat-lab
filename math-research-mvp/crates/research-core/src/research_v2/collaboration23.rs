//! Human intent, delivery evidence and execution receipts. Summaries never certify actions.
use super::{
    V2Result, V2Service, Value, array, digest, entity, entity_mut, err, id, json, lab, now, push,
    remaining, revision,
};

pub(super) fn feedback_kind(kind: &str) -> bool {
    matches!(
        kind,
        "suggest_idea"
            | "steer_focus"
            | "restrict_method"
            | "human_answer"
            | "feedback_correction"
            | "reframe_goal"
    )
}

fn check_revision(item: &Value, expected: &Value) -> V2Result<()> {
    if expected.as_u64().is_none() || *expected != item["revision"] {
        return Err(err("REVISION_CONFLICT", "请刷新对象版本后重试"));
    }
    Ok(())
}

fn check_versions(state: &Value, run: &Value, input: &Value) -> V2Result<()> {
    for (field, current) in [
        ("problem_version", &state["problem_version"]),
        ("control_epoch", &run["control_epoch"]),
    ] {
        if !input["expected_versions"][field].is_null()
            && input["expected_versions"][field] != *current
        {
            return Err(err("REVISION_CONFLICT", format!("{field} 已改变")));
        }
    }
    Ok(())
}

fn text_field<'a>(input: &'a Value, name: &str) -> V2Result<&'a str> {
    input[name]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 1_000_000)
        .ok_or_else(|| err("INVALID_FEEDBACK", format!("{name} 不能为空或超过大小限制")))
}

fn replay(state: &Value, key: &str, hash: &str) -> V2Result<Option<Value>> {
    if key.trim().is_empty() {
        return Err(err("IDEMPOTENCY_REQUIRED", "需要幂等键"));
    }
    if let Some(old) = array(state, "commands")
        .iter()
        .find(|c| c["idempotency_key"] == key)
    {
        if old["request_hash"] != hash {
            return Err(err("IDEMPOTENCY_CONFLICT", "相同键对应不同内容"));
        }
        return Ok(Some(old.clone()));
    }
    if let Some(old) = array(state, "feedback_operations")
        .iter()
        .find(|c| c["idempotency_key"] == key)
    {
        if old["request_hash"] != hash {
            return Err(err("IDEMPOTENCY_CONFLICT", "相同键对应不同操作"));
        }
        return Ok(Some(old["result"].clone()));
    }
    Ok(None)
}

fn active_run(state: &Value, input: &Value) -> V2Result<Value> {
    let run_id = input["run_id"]
        .as_str()
        .or_else(|| {
            array(state, "runs")
                .iter()
                .rev()
                .find(|r| r["state"] != "ended")
                .and_then(|r| r["id"].as_str())
        })
        .ok_or_else(|| err("RUN_NOT_ACTIVE", "没有可操作的运行"))?;
    let run = entity(state, "runs", run_id).ok_or_else(|| err("NOT_FOUND", "运行不存在"))?;
    if matches!(run["state"].as_str(), Some("ended" | "stopping")) {
        return Err(err("RUN_ENDED", "运行已结束或正在停止"));
    }
    check_versions(state, run, input)?;
    Ok(run.clone())
}

fn queue_message(state: &mut Value, command: &Value) -> V2Result<Vec<String>> {
    if array(state, "messages")
        .iter()
        .any(|m| m["command_id"] == command["id"])
    {
        return Ok(vec![]);
    }
    let run_id = command["run_id"].as_str().unwrap_or_default();
    let target = array(state, "sessions")
        .iter()
        .find(|s| {
            s["run_id"] == run_id
                && s["role"] == "main"
                && !matches!(s["state"].as_str(), Some("closed" | "lost"))
        })
        .map(|s| s["id"].clone());
    push(
        state,
        "messages",
        json!({"id":id(),"command_id":command["id"],"run_id":run_id,"target_role":"main","recipient_session_id":target,"state":"queued","status":"queued","author_kind":"human","priority":command["priority"],"summary":command["payload"]["text"],"body":command["payload"],"problem_version":command["problem_version"],"created_at":now()}),
    );
    let running = entity(state, "runs", run_id).is_some_and(|r| r["state"] == "running");
    if command["priority"] == "urgent"
        && let Some(sid) = target.as_ref().and_then(Value::as_str)
    {
        let session = entity_mut(state, "sessions", sid)?;
        session["pending_urgent"] = json!(true);
        session["pending_followup"] = json!(true);
        // Waiting for a human answer, pauses and cancellation uncertainty are not unlocked here.
        if running {
            session["control_epoch"] = json!(session["control_epoch"].as_u64().unwrap_or(1) + 1);
            revision(session);
            return Ok(vec![sid.to_owned()]);
        }
        revision(session);
    }
    Ok(vec![])
}

#[allow(clippy::needless_pass_by_value)] // The JSON payload is moved into a newly owned command.
fn new_command(
    state: &Value,
    run: &Value,
    kind: &str,
    priority: &str,
    payload: Value,
    key: &str,
    hash: &str,
) -> Value {
    json!({"id":id(),"run_id":run["id"],"problem_version":state["problem_version"],"type":kind,"priority":priority,"payload":payload,"status":"queued","disposition":null,"delivery_state":"queued","idempotency_key":key,"request_hash":hash,"revision":1,"effect_refs":[],"created_at":now()})
}

pub(super) fn register_question(
    state: &mut Value,
    run_id: &str,
    session_id: &str,
    action: &Value,
    artifact: &Value,
) -> V2Result<Value> {
    let body = text_field(action, "question")?;
    if let Some(old) = array(state, "human_questions").iter().find(|q| {
        q["run_id"] == run_id && q["source_artifact_id"] == artifact["id"] && q["body"] == body
    }) {
        return Ok(old.clone());
    }
    for question in state["human_questions"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        if question["run_id"] == run_id && question["state"] == "open" {
            question["state"] = json!("superseded");
            revision(question);
        }
    }
    let question = json!({"id":id(),"run_id":run_id,"session_id":session_id,"problem_version":state["problem_version"],"body":body,"state":"open","revision":1,"source_artifact_id":artifact["id"],"created_at":now()});
    push(state, "human_questions", question.clone());
    let run = entity_mut(state, "runs", run_id)?;
    run["state"] = json!("waiting_human");
    run["question"] = json!(body);
    run["human_question_id"] = question["id"].clone();
    revision(run);
    Ok(question)
}

pub(super) fn open_question(state: &Value, run_id: &str) -> bool {
    array(state, "human_questions").iter().any(|q| {
        q["run_id"] == run_id
            && q["state"] == "open"
            && q["problem_version"] == state["problem_version"]
    })
}

/// This transaction is the opinion cutoff. A prepared input cannot be erased by a later UI write.
pub(super) fn prepare_commands(
    state: &mut Value,
    run_id: &str,
    session_id: &str,
    invocation: &str,
    capture: &mut Value,
) -> V2Result<Vec<Value>> {
    if !entity(state, "sessions", session_id).is_some_and(|s| s["role"] == "main")
        || !matches!(
            capture["phase"].as_str(),
            Some("coordination" | "urgent_attention")
        )
    {
        return Ok(vec![]);
    }
    let commands: Vec<Value> = array(state, "commands")
        .iter()
        .filter(|c| {
            c["run_id"] == run_id
                && c["status"] == "queued"
                && (capture["phase"] != "urgent_attention" || c["priority"] == "urgent")
        })
        .cloned()
        .collect();
    let cutoff = now();
    capture["opinion_cutoff_at"] = json!(cutoff);
    capture["command_ids"] = json!(commands.iter().map(|c| c["id"].clone()).collect::<Vec<_>>());
    capture["invocation_id"] = json!(invocation);
    for command in &commands {
        let current = entity_mut(
            state,
            "commands",
            command["id"].as_str().unwrap_or_default(),
        )?;
        current["status"] = json!("prepared");
        current["delivery_state"] = json!("prepared");
        push(
            current,
            "delivery_attempts",
            json!({"invocation_id":invocation,"session_id":session_id,"prepared_at":cutoff,"certainty":"prepared_not_confirmed"}),
        );
        revision(current);
    }
    let session = entity_mut(state, "sessions", session_id)?;
    session["opinion_cutoff_at"] = json!(cutoff);
    session["inflight_command_ids"] = capture["command_ids"].clone();
    Ok(commands)
}

pub(super) fn confirm_delivery(
    state: &mut Value,
    ids: &[String],
    invocation: &str,
) -> V2Result<()> {
    for id in ids {
        let command = entity_mut(state, "commands", id)?;
        if let Some(attempt) = command["delivery_attempts"]
            .as_array_mut()
            .and_then(|a| a.iter_mut().find(|a| a["invocation_id"] == invocation))
        {
            attempt["certainty"] = json!("confirmed");
            attempt["confirmed_at"] = json!(now());
        }
        command["delivery_state"] = json!("confirmed");
        command["delivery_record"] = json!({"invocation_id":invocation,"certainty":"confirmed"});
        if matches!(command["status"].as_str(), Some("prepared" | "queued")) {
            command["status"] = json!("delivered");
        }
        revision(command);
    }
    Ok(())
}

pub(super) fn failed_delivery(state: &mut Value, session_id: &str, code: &str) -> V2Result<()> {
    let session = entity(state, "sessions", session_id)
        .cloned()
        .unwrap_or(Value::Null);
    let ids = array(&session, "inflight_command_ids").to_vec();
    let unknown = code == "DELIVERY_UNCERTAIN" || code.contains("UNKNOWN");
    for id in ids.iter().filter_map(Value::as_str) {
        let command = entity_mut(state, "commands", id)?;
        if let Some(attempt) = command["delivery_attempts"]
            .as_array_mut()
            .and_then(|items| {
                items
                    .iter_mut()
                    .rev()
                    .find(|item| item["session_id"] == session_id)
            })
        {
            attempt["failure_code"] = json!(code);
            attempt["ended_at"] = json!(now());
            // The invocation ID joins authoritative usage; a failed output is not proof of no delivery.
            if attempt["certainty"] != "confirmed" {
                attempt["certainty"] = json!(if unknown { "unknown" } else { "unconfirmed" });
            }
        }
        if command["status"] == "prepared" {
            command["status"] = json!(if unknown { "prepared" } else { "queued" });
            command["delivery_state"] = json!(if unknown { "unknown" } else { "unconfirmed" });
        }
        revision(command);
    }
    Ok(())
}

pub(super) fn resume_pending_commands(state: &mut Value, run_id: &str) {
    for command in state["commands"].as_array_mut().into_iter().flatten() {
        if command["run_id"] == run_id
            && feedback_kind(command["type"].as_str().unwrap_or_default())
            && command["disposition"].is_null()
            && matches!(command["status"].as_str(), Some("prepared" | "delivered"))
        {
            command["status"] = json!("queued");
            revision(command);
        }
    }
}

pub(super) fn validate_source_commands(
    state: &Value,
    run_id: &str,
    session_id: &str,
    action: &Value,
) -> V2Result<Vec<String>> {
    let refs = action
        .get("source_command_ids")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let refs = refs
        .as_array()
        .filter(|r| r.len() <= 32)
        .ok_or_else(|| err("CONTROL_INVALID", "source_command_ids 须为最多32个命令ID"))?;
    let mut ids = Vec::new();
    for reference in refs {
        let id = reference
            .as_str()
            .ok_or_else(|| err("CONTROL_INVALID", "命令引用须为ID"))?;
        let command =
            entity(state, "commands", id).ok_or_else(|| err("INVALID_TARGET", "引用意见不存在"))?;
        if command["run_id"] != run_id
            || !array(command, "delivery_attempts")
                .iter()
                .any(|d| d["session_id"] == session_id && d["certainty"] == "confirmed")
        {
            return Err(err("INVALID_TARGET", "不能关联未向本会话确认提供的意见"));
        }
        if !ids.iter().any(|old| old == id) {
            ids.push(id.to_owned());
        }
    }
    Ok(ids)
}

pub(super) fn command_response(
    state: &mut Value,
    run_id: &str,
    session_id: &str,
    action: &Value,
    artifact: &Value,
) -> V2Result<Value> {
    if !entity(state, "sessions", session_id).is_some_and(|s| s["role"] == "main") {
        return Err(err("INVALID_ROLE", "只有领研猫处置人类意见"));
    }
    let command_id = text_field(action, "command_id")?;
    validate_source_commands(
        state,
        run_id,
        session_id,
        &json!({"source_command_ids":[command_id]}),
    )?;
    let disposition = action["disposition"].as_str().unwrap_or_default();
    if !matches!(
        disposition,
        "adopted" | "partially_adopted" | "declined" | "needs_clarification" | "deferred"
    ) {
        return Err(err("CONTROL_INVALID", "需明确意见处置"));
    }
    let reason = action["response"]
        .as_str()
        .or_else(|| action["reason"].as_str())
        .filter(|r| !r.trim().is_empty())
        .ok_or_else(|| err("CONTROL_INVALID", "意见处置需要理由"))?;
    let command = entity_mut(state, "commands", command_id)?;
    if matches!(command["status"].as_str(), Some("cancelled" | "superseded")) {
        return Err(err("INVALID_STATE", "该意见已撤回或被替代，请处理关联通知"));
    }
    command["status"] = json!(
        if matches!(disposition, "needs_clarification" | "deferred") {
            "acknowledged"
        } else {
            "completed"
        }
    );
    command["disposition"] = json!(disposition);
    command["response"] = json!(reason);
    command["handled_by"] = json!(session_id);
    command["response_artifact_id"] = artifact["id"].clone();
    command["handled_at"] = json!(now());
    revision(command);
    Ok(command.clone())
}

/// IDs come only from host mutation results. Explicit intent attribution is kept separate.
pub(super) fn result_refs(before: &Value, state: &Value, result: &Value) -> Vec<Value> {
    fn walk(value: &Value, ids: &mut Vec<String>) {
        match value {
            Value::Array(items) => {
                for item in items {
                    walk(item, ids);
                }
            }
            Value::Object(fields) => {
                for key in [
                    "id",
                    "task_id",
                    "session_id",
                    "partner_session_id",
                    "pending_assignment_id",
                    "candidate_id",
                    "review_id",
                    "node_id",
                    "edge_id",
                    "route_id",
                    "job_id",
                    "question_id",
                ] {
                    if let Some(identifier) = fields.get(key).and_then(Value::as_str) {
                        ids.push(identifier.to_owned());
                    }
                }
                // Only host result wrappers are traversed. Goal/source/from/to refs are inputs.
                for key in [
                    "result",
                    "task",
                    "session",
                    "assignment",
                    "candidate",
                    "review",
                    "node",
                    "edge",
                    "route",
                    "question",
                ] {
                    if let Some(value) = fields.get(key) {
                        walk(value, ids);
                    }
                }
            }
            _ => {}
        }
    }
    let mut ids = Vec::<String>::new();
    walk(result, &mut ids);
    let mut refs = Vec::new();
    for collection in [
        "tasks",
        "sessions",
        "pending_assignments",
        "candidates",
        "reviews",
        "nodes",
        "edges",
        "routes",
        "human_questions",
        "background_jobs",
    ] {
        for item in array(state, collection).iter().filter(|i| {
            i["id"]
                .as_str()
                .is_some_and(|id| ids.iter().any(|v| v == id))
        }) {
            if entity(before, collection, item["id"].as_str().unwrap_or_default()) == Some(item) {
                continue;
            }
            refs.push(json!({"collection":collection,"id":item["id"],"revision":item["revision"],"state":item.get("state").unwrap_or(&item["status"])}));
        }
    }
    refs
}

pub(super) fn traces(state: &Value, run_id: Option<&str>) -> Vec<Value> {
    array(state, "commands").iter().filter(|c| feedback_kind(c["type"].as_str().unwrap_or_default()) && run_id.is_none_or(|r| c["run_id"] == r)).map(|c| {
        let messages: Vec<Value> = array(state, "messages").iter().filter(|m| m["command_id"] == c["id"]).cloned().collect();
        let receipts: Vec<Value> = array(state, "action_receipts").iter().filter(|r| array(r,"source_command_ids").contains(&c["id"]) || r["action"]["command_id"] == c["id"]).cloned().collect();
        let refs: Vec<Value> = receipts.iter().filter(|r| r["status"] == "applied" && r["action"]["type"] != "command_response").flat_map(|r| array(r,"result_refs").iter().cloned()).map(|mut r| {if let Some(current)=entity(state,r["collection"].as_str().unwrap_or_default(),r["id"].as_str().unwrap_or_default()){r["current_state"]=current.get("state").unwrap_or(&current["status"]).clone(); r["current_revision"]=current["revision"].clone();r["applied_at"]=current["applied_at"].clone();}r}).collect();
        let delivery:Vec<Value>=array(c,"delivery_attempts").iter().cloned().map(|mut attempt|{attempt["usage"]=entity(state,"usage",attempt["invocation_id"].as_str().unwrap_or_default()).cloned().unwrap_or(Value::Null);attempt}).collect();
        json!({"command_id":c["id"],"kind":c["type"],"target":c["payload"]["feedback"]["target"],"target_session_id":c["payload"]["feedback"]["target_session_id"],"run_id":c["run_id"],"problem_version":c["problem_version"],"body":c["payload"]["text"],"priority":c["priority"],"status":c["status"],"revision":c["revision"],"created_at":c["created_at"],"delivery":delivery,"delivery_state":c["delivery_state"],"disposition":c["disposition"],"response":c["response"],"response_artifact_id":c["response_artifact_id"],"messages":messages,"message_ids":messages.iter().map(|m|m["id"].clone()).collect::<Vec<_>>(),"action_receipts":receipts,"result_refs":refs,"annotation_id":c["payload"]["feedback"]["annotation_id"],"replaces_command_id":c["replaces_command_id"],"replacement_command_id":c["replacement_command_id"],"withdrawal":c["withdrawal"],"association":"model_declared_intent_with_host_action_receipts"})
    }).collect()
}

impl V2Service {
    pub(super) async fn feedback_command23(
        &self,
        project: &str,
        input: &Value,
        key: &str,
    ) -> V2Result<Value> {
        let hash = digest(&input.to_string());
        let result = self
            .store
            .mutate(project, "feedback.queued", None, |state| {
                if let Some(old) = replay(state, key, &hash)? {
                    return Ok(json!({"command":old,"cancel_session_ids":[]}));
                }
                let run = active_run(state, input)?;
                let priority = input["priority"].as_str().unwrap_or("normal");
                if !matches!(priority, "normal" | "urgent") {
                    return Err(err("INVALID_FEEDBACK", "优先级错误"));
                }
                text_field(&input["payload"], "text")?;
                let feedback = &input["payload"]["feedback"];
                if !feedback["selection_ref"].is_null() {
                    super::interactions::selection23(state, &feedback["selection_ref"])?;
                }
                if let Some(refs) = feedback.get("evidence_refs") {
                    for reference in refs
                        .as_array()
                        .ok_or_else(|| err("INVALID_FEEDBACK", "evidence_refs 须为数组"))?
                    {
                        let source = reference
                            .as_str()
                            .or_else(|| reference["id"].as_str())
                            .ok_or_else(|| err("INVALID_FEEDBACK", "引用需已有来源ID"))?;
                        if ![
                            "nodes",
                            "candidates",
                            "facts",
                            "artifacts",
                            "reviews",
                            "proof_checkpoints",
                        ]
                        .iter()
                        .any(|collection| entity(state, collection, source).is_some())
                        {
                            return Err(err("MATERIAL_MISSING", "意见引用来源不存在"));
                        }
                        if reference.is_object()
                            && reference.get("revision").is_some()
                            && reference["kind"].is_string()
                        {
                            super::interactions::selection23(state, reference)?;
                        }
                    }
                }
                if let Some(annotation_id) = feedback["annotation_id"].as_str() {
                    let annotation = entity(state, "annotations", annotation_id)
                        .ok_or_else(|| err("NOT_FOUND", "批注不存在"))?;
                    check_revision(annotation, &feedback["annotation_revision"])?;
                }
                let command = new_command(
                    state,
                    &run,
                    input["type"].as_str().unwrap_or("suggest_idea"),
                    priority,
                    input["payload"].clone(),
                    key,
                    &hash,
                );
                push(state, "commands", command.clone());
                let cancelled = queue_message(state, &command)?;
                if let Some(aid) = feedback["annotation_id"].as_str() {
                    let annotation = entity_mut(state, "annotations", aid)?;
                    push(annotation, "linked_command_ids", command["id"].clone());
                    revision(annotation);
                }
                Ok(json!({"command":command,"cancel_session_ids":cancelled}))
            })
            .await?;
        self.cancel_sessions(&result, "cancel_session_ids").await;
        self.launch(
            project,
            result["command"]["run_id"].as_str().unwrap_or_default(),
        )
        .await;
        Ok(result["command"].clone())
    }

    #[must_use]
    pub fn feedback_traces_from_snapshot23(state: &Value, run_id: Option<&str>) -> Vec<Value> {
        traces(state, run_id)
    }

    pub async fn feedback_traces23(
        &self,
        project: &str,
        run_id: Option<&str>,
    ) -> V2Result<Vec<Value>> {
        Ok(traces(&self.store.read(project).await?, run_id))
    }

    #[allow(clippy::if_not_else)] // Keep the before/after delivery boundary in chronological order.
    pub async fn change_feedback23(
        &self,
        project: &str,
        command_id: &str,
        operation: &str,
        input: &Value,
        key: &str,
    ) -> V2Result<Value> {
        if !matches!(operation, "withdraw" | "escalate") {
            return Err(err("INVALID_REQUEST", "未知意见操作"));
        }
        let hash = digest(
            &json!({"command_id":command_id,"operation":operation,"input":input}).to_string(),
        );
        let result=self.store.mutate(project,"feedback.changed",None,|state|{
            if let Some(old)=replay(state,key,&hash)?{return Ok(json!({"command":old,"cancel_session_ids":[]}));}
            let old=entity(state,"commands",command_id).cloned().ok_or_else(||err("NOT_FOUND","意见不存在"))?;
            if !feedback_kind(old["type"].as_str().unwrap_or_default()) || old["type"]=="human_answer"{return Err(err("INVALID_TARGET","该命令不是可修改建议"));}
            check_revision(&old,&input["expected_revision"])?;
            if matches!(old["status"].as_str(),Some("cancelled"|"superseded")){return Err(err("INVALID_STATE","意见已撤回或替代"));}
            let run=active_run(state,&json!({"run_id":old["run_id"]}))?;
            let prepared=!array(&old,"delivery_attempts").is_empty() || array(state,"messages").iter().any(|m|m["command_id"]==command_id&&m["state"]!="queued");
            let mut cancelled=vec![];
            let result;
            if !prepared {
                let command=entity_mut(state,"commands",command_id)?;
                if operation=="withdraw"{command["status"]=json!("cancelled");command["withdrawal"]=json!({"state":"withdrawn_before_preparation","at":now()});}else{command["priority"]=json!("urgent");}
                revision(command);result=command.clone();
                for message in state["messages"].as_array_mut().into_iter().flatten(){if message["command_id"]==command_id{if operation=="withdraw"{message["state"]=json!("cancelled");message["status"]=json!("cancelled");}else{message["priority"]=json!("urgent");}revision(message);}}
                if operation=="escalate"{cancelled=signal_urgent(state,run["id"].as_str().unwrap_or_default())?;}
            } else {
                let reason=input["reason"].as_str().unwrap_or(if operation=="withdraw"{"用户撤回此前意见；历史上下文无法删除。"}else{"用户将此前意见升级为紧急。"});
                let text=if operation=="withdraw"{format!("撤回通知：意见 {command_id} 不再是当前建议。{reason}")}else{format!("紧急替代意见 {command_id}：{}\n{reason}",old["payload"]["text"].as_str().unwrap_or_default())};
                let mut next=new_command(state,&run,"feedback_correction",if operation=="escalate"{"urgent"}else{"normal"},json!({"text":text,"operation":operation,"original_command_id":command_id,"feedback":old["payload"]["feedback"]}),key,&hash);
                next["replaces_command_id"]=json!(command_id);push(state,"commands",next.clone());
                let command=entity_mut(state,"commands",command_id)?;command["status"]=json!("superseded");command["replacement_command_id"]=next["id"].clone();
                if operation=="withdraw"{command["withdrawal"]=json!({"state":"notification_queued_after_preparation","notification_command_id":next["id"],"at":now()});}revision(command);
                for message in state["messages"].as_array_mut().into_iter().flatten(){if message["command_id"]==command_id&&matches!(message["state"].as_str(),Some("queued"|"pending")){message["state"]=json!("superseded");revision(message);}}
                cancelled=queue_message(state,&next)?;result=next;
            }
            push(state,"feedback_operations",json!({"id":id(),"command_id":command_id,"operation":operation,"idempotency_key":key,"request_hash":hash,"result":result,"created_at":now()}));
            Ok(json!({"command":result,"cancel_session_ids":cancelled}))
        }).await?;
        self.cancel_sessions(&result, "cancel_session_ids").await;
        self.launch(
            project,
            result["command"]["run_id"].as_str().unwrap_or_default(),
        )
        .await;
        Ok(result["command"].clone())
    }

    pub async fn answer_question23(
        &self,
        project: &str,
        question_id: &str,
        input: &Value,
        key: &str,
    ) -> V2Result<Value> {
        let hash = digest(&json!({"question_id":question_id,"input":input}).to_string());
        let command=self.store.mutate(project,"human_question.answered",None,|state|{
            if let Some(old)=replay(state,key,&hash)?{return Ok(old);}
            let question=entity(state,"human_questions",question_id).cloned().ok_or_else(||err("NOT_FOUND","待答问题不存在"))?;
            check_revision(&question,&input["expected_revision"])?;
            if question["state"]!="open"||question["problem_version"]!=state["problem_version"]||input["run_id"]!=question["run_id"]{return Err(err("INVALID_STATE","问题已答复、已替代或不属于该运行"));}
            let run=active_run(state,input)?;
            if run["state"]!="waiting_human"||run["human_question_id"]!=question_id{return Err(err("INVALID_STATE","当前并非等待此问题；暂停或停止不能被答复绕过"));}
            if remaining(&run)==0{return Err(err("DEADLINE_REACHED","期限已到"));}
            if run["outstanding_cancellation"]==true||super::unknown_interaction(state){return Err(err("DELIVERY_UNCERTAIN","存在未确认执行，不能恢复"));}
            let body=text_field(input,"body")?;
            let command=new_command(state,&run,"human_answer","normal",json!({"text":body,"question_id":question_id,"question_revision":question["revision"]}),key,&hash);
            push(state,"commands",command.clone());queue_message(state,&command)?;
            let q=entity_mut(state,"human_questions",question_id)?;q["state"]=json!("answered");q["answer"]=json!({"body":body,"command_id":command["id"],"created_at":now()});revision(q);
            let r=entity_mut(state,"runs",run["id"].as_str().unwrap_or_default())?;r["state"]=json!("running");r["question"]=Value::Null;revision(r);
            if let Some(sid)=array(state,"sessions").iter().rev().find(|s|s["run_id"]==run["id"]&&s["role"]=="main"&&!matches!(s["state"].as_str(),Some("closed"|"lost"))).and_then(|s|s["id"].as_str()).map(str::to_owned){
                lab::transition(state,&sid,"coordination","human_answer",&now())?;
                let session=entity_mut(state,"sessions",&sid)?;if session["invocation_in_flight"]!=true{session["state"]=json!("idle");}session["pending_followup"]=json!(true);
            }
            Ok(command)
        }).await?;
        self.launch(project, command["run_id"].as_str().unwrap_or_default())
            .await;
        Ok(command)
    }
}

fn signal_urgent(state: &mut Value, run_id: &str) -> V2Result<Vec<String>> {
    let running = entity(state, "runs", run_id).is_some_and(|r| r["state"] == "running");
    let Some(sid) = array(state, "sessions")
        .iter()
        .find(|s| {
            s["run_id"] == run_id
                && s["role"] == "main"
                && !matches!(s["state"].as_str(), Some("closed" | "lost"))
        })
        .and_then(|s| s["id"].as_str())
        .map(str::to_owned)
    else {
        return Ok(vec![]);
    };
    let s = entity_mut(state, "sessions", &sid)?;
    s["pending_urgent"] = json!(true);
    if running {
        s["control_epoch"] = json!(s["control_epoch"].as_u64().unwrap_or(1) + 1);
    }
    revision(s);
    Ok(if running { vec![sid] } else { vec![] })
}

pub(super) fn prompt() -> &'static str {
    "2.3 人类意见：仅对实际提供的 command_id 用 command_response 返回 adopted|partially_adopted|declined|deferred|needs_clarification 及非空 response。收到不等于采纳。由某意见产生的实际 actions 明确带 source_command_ids:[该意见ID]；宿主只关联实际动作回执，不凭回复文字声称执行。每轮意见取入截止时间已固定；晚到意见留待后续安排。wait_for_user:{question} 创建带版本待答问题，普通建议不会解除等待。讨论/解释是只读通道，不是研究指令。"
}

#[cfg(test)]
#[path = "collaboration23_tests.rs"]
mod tests;
