//! Human controls and runtime limits; mathematical organization remains with the main cat.
use super::{
    DateTime, Utc, V2Result, V2Service, Value, array, digest, entity, entity_mut, err, evidence22,
    id, json, lab, now, push, revision,
};

pub(super) fn feedback_action(
    state: &mut Value,
    run_id: &str,
    session_id: &str,
    action: &Value,
) -> V2Result<Option<Value>> {
    if action["type"] != "send_feedback" {
        return Ok(None);
    }
    let session =
        entity(state, "sessions", session_id).ok_or_else(|| err("NOT_FOUND", "会话不存在"))?;
    if session["run_id"] != run_id || !matches!(session["role"].as_str(), Some("main" | "partner"))
    {
        return Err(err("INVALID_ROLE", "仅研究猫可发送研究反馈"));
    }
    let summary = action["summary"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| err("CONTROL_INVALID", "反馈需 summary"))?;
    let priority = action["priority"].as_str().unwrap_or("normal");
    if !matches!(priority, "normal" | "urgent") {
        return Err(err("CONTROL_INVALID", "反馈优先级错误"));
    }
    if priority == "urgent"
        && action["urgency_reason"]
            .as_str()
            .is_none_or(|s| s.trim().is_empty())
    {
        return Err(err("CONTROL_INVALID", "紧急反馈需具体 urgency_reason"));
    }
    let refs = action["evidence_refs"]
        .as_array()
        .ok_or_else(|| err("CONTROL_INVALID", "反馈需 evidence_refs 数组"))?;
    for reference in refs {
        let key = reference
            .as_str()
            .or_else(|| reference["id"].as_str())
            .ok_or_else(|| err("CONTROL_INVALID", "引用需已有来源 ID"))?;
        if ![
            "artifacts",
            "candidates",
            "facts",
            "nodes",
            "proof_checkpoints",
            "reviews",
        ]
        .iter()
        .any(|collection| entity(state, collection, key).is_some())
        {
            return Err(err("MATERIAL_MISSING", format!("反馈来源 {key} 不存在")));
        }
    }
    let message = json!({"id":id(),"run_id":run_id,"session_id":session_id,"target_role":"main","state":"queued","priority":priority,"summary":summary,"body":action["body"],"evidence_refs":refs,"urgency_reason":action["urgency_reason"],"trust":"unreviewed","problem_version":state["problem_version"],"created_at":now()});
    push(state, "messages", message.clone());
    Ok(Some(message))
}

impl V2Service {
    pub(super) async fn cancel_sessions(&self, result: &Value, field: &str) {
        let turns = self.turns.lock().await;
        for session in array(result, field).iter().filter_map(Value::as_str) {
            if let Some(token) = turns.get(session) {
                token.cancel();
            }
        }
    }

    pub async fn update_limits(
        &self,
        project: &str,
        run_id: &str,
        key: &str,
        input: &Value,
    ) -> V2Result<Value> {
        if key.trim().is_empty() {
            return Err(err("IDEMPOTENCY_REQUIRED", "修改预算需要幂等键"));
        }
        let hash = digest(&input.to_string());
        self.store.mutate(project,"run.limits_updated",None,|state| {
            if let Some(old)=array(state,"limit_updates").iter().find(|v|v["key"]==key) {
                if old["hash"]!=hash {return Err(err("IDEMPOTENCY_CONFLICT","相同键对应不同预算修改"));}
                return Ok(old["result"].clone());
            }
            let run=entity_mut(state,"runs",run_id)?;
            if run["state"]=="ended" {return Err(err("RUN_ENDED","已结束运行不能扩展预算"));}
            if !input["expected_revision"].is_null() && input["expected_revision"]!=run["revision"] {return Err(err("REVISION_CONFLICT","运行版本已变化"));}
            let updates=input.get("limits").unwrap_or(input).as_object().ok_or_else(||err("INVALID_LIMITS","需要预算对象"))?;
            let mut limits=run["limits"].clone();
            for (field,value) in updates {
                if field=="expected_revision" {continue;}
                if !["duration_seconds","max_invocations","max_partners","token_limit","allow_early_unresolved","research_soft_seconds","research_hard_seconds","coordination_soft_seconds","coordination_hard_seconds","advisor_interval_seconds","advisor_interval","memory_interval_seconds","display_interval_seconds","conditional_max_invocations"].contains(&field.as_str()) {
                    return Err(err("INVALID_LIMITS",format!("不支持动态修改 {field}")));
                }
                limits[field]=value.clone();
            }
            for (field,min,max) in [("max_invocations",1,1000),("max_partners",0,5)] {
                if limits[field].as_u64().is_none_or(|v|v<min||v>max) {return Err(err("INVALID_LIMITS",format!("{field} 超出范围")));}
            }
            if !limits["token_limit"].is_null() && limits["token_limit"].as_u64().is_none_or(|v|v==0) {return Err(err("INVALID_LIMITS","token_limit 必须为正整数或 null"));}
            if !limits["allow_early_unresolved"].is_boolean() {return Err(err("INVALID_LIMITS","allow_early_unresolved 必须为布尔值"));}
            lab::normalize_limits(&mut limits)?;
            if updates.contains_key("duration_seconds") {
                run["deadline_at"]=if limits["duration_seconds"].is_null() {Value::Null} else {
                    let secs=limits["duration_seconds"].as_i64().filter(|v|(1..=31_536_000).contains(v)).ok_or_else(||err("INVALID_DURATION","时长须为1–31536000秒或null"))?;
                    if let Some(start)=run["started_at"].as_str().and_then(|s|DateTime::parse_from_rfc3339(s).ok()) {json!((start+chrono::Duration::seconds(secs)).to_rfc3339())} else {Value::Null}
                };
            }
            if updates.contains_key("advisor_interval_seconds") || updates.contains_key("advisor_interval") {
                if let Some(seconds)=limits["advisor_interval_seconds"].as_i64() {run["advisor_next_due_at"]=json!((Utc::now()+chrono::Duration::seconds(seconds)).to_rfc3339());}
            }
            if limits["conditional_max_invocations"].as_u64().is_none_or(|v|v==0||v>1000) {return Err(err("INVALID_LIMITS","conditional_max_invocations 须为1–1000"));}
            run["limits"]=limits;run["config_revision"]=json!(run["config_revision"].as_u64().unwrap_or(0)+1);run["config_effective_at"]=json!(now()); revision(run); let result=run.clone();
            push(state,"limit_updates",json!({"id":id(),"key":key,"hash":hash,"run_id":run_id,"result":result,"created_at":now()}));
            Ok(result)
        }).await
    }

    pub async fn feedback(&self, project: &str, key: &str, input: &Value) -> V2Result<Value> {
        if key.trim().is_empty() {
            return Err(err("IDEMPOTENCY_REQUIRED", "建议需要幂等键"));
        }
        let priority = input["priority"].as_str().unwrap_or("normal");
        if !matches!(priority, "normal" | "urgent") {
            return Err(err("INVALID_FEEDBACK", "priority 必须是 normal 或 urgent"));
        }
        let body = input["body"]
            .as_str()
            .or_else(|| input["summary"].as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| err("INVALID_FEEDBACK", "建议正文不能为空"))?;
        let kind = if input["kind"] == "reframe_goal" {
            "reframe_goal"
        } else {
            "suggest_idea"
        };
        let command=self.command(project,json!({"type":kind,"run_id":input["run_id"],"priority":priority,"payload":{"text":body,"feedback":input},"expected_versions":input["expected_versions"]}),key).await?;
        Ok(command)
    }

    pub(super) async fn route_command(
        &self,
        project: &str,
        input: &Value,
        key: &str,
    ) -> V2Result<Value> {
        let hash = digest(&input.to_string());
        let result=self.store.mutate(project,"route.controlled",None,|state| {
            if let Some(old)=array(state,"commands").iter().find(|c|c["idempotency_key"]==key) {
                if old["request_hash"]!=hash {return Err(err("IDEMPOTENCY_CONFLICT","命令键冲突"));} return Ok(json!({"command":old,"cancel_session_ids":[]}));
            }
            let run_id=input["run_id"].as_str().or_else(||array(state,"runs").iter().rev().find(|r|r["state"]!="ended").and_then(|r|r["id"].as_str())).ok_or_else(||err("RUN_NOT_ACTIVE","没有活动运行"))?.to_owned();
            let run=entity(state,"runs",&run_id).ok_or_else(||err("NOT_FOUND","运行不存在"))?;
            if run["state"]=="ended" {return Err(err("RUN_ENDED","运行已结束"));}
            for (field,current) in [("problem_version",&state["problem_version"]),("control_epoch",&run["control_epoch"])] {
                if !input["expected_versions"][field].is_null() && input["expected_versions"][field]!=*current {return Err(err("REVISION_CONFLICT","版本已改变"));}
            }
            let route_id=input["target"]["id"].as_str().or_else(||input["payload"]["route_id"].as_str()).ok_or_else(||err("INVALID_TARGET","路线命令需要 route_id"))?;
            if !input["target"]["revision"].is_null() && entity(state,"routes",route_id).is_none_or(|r|r["revision"]!=input["target"]["revision"]) {return Err(err("REVISION_CONFLICT","路线版本已改变"));}
            let result=if input["type"]=="prohibit_route" {lab::prohibit_route_at(state,&run_id,route_id,input["payload"]["reason"].as_str().unwrap_or("human prohibition"),&now())?} else {lab::reopen_route_at(state,&run_id,route_id,input["payload"]["reason"].as_str().unwrap_or("human reopening"),&now())?};
            let command=json!({"id":id(),"run_id":run_id,"type":input["type"],"target":input["target"],"payload":input["payload"],"status":"completed","idempotency_key":key,"request_hash":hash,"revision":1,"created_at":now(),"effect_refs":result});
            push(state,"commands",command.clone());
            super::whiteboard24::invalidate(state,&run_id,"route_restriction_changed");
            Ok(json!({"command":command,"cancel_session_ids":result["cancel_session_ids"]}))
        }).await?;
        self.cancel_sessions(&result, "cancel_session_ids").await;
        Ok(result["command"].clone())
    }

    pub async fn memory_search22(
        &self,
        project: &str,
        query: &str,
        mode: &str,
    ) -> V2Result<Vec<Value>> {
        if !matches!(mode, "facts" | "experience") {
            return Err(err("INVALID_QUERY", "mode 为 facts 或 experience"));
        }
        let state = self.store.read(project).await?;
        let prepared = json!({"session":{"id":null}});
        let mut context = json!({});
        evidence22::augment_context(&state, &prepared, &mut context);
        let key = if mode == "facts" {
            "fact_records"
        } else {
            "experience_records"
        };
        let needle = query.to_lowercase();
        Ok(array(&context, key)
            .iter()
            .filter(|r| needle.is_empty() || r.to_string().to_lowercase().contains(&needle))
            .cloned()
            .collect())
    }
}
