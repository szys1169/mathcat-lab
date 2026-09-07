//! Precise problem presentation and sourced unresolved/abandoned work.
use super::{
    V2Result, V2Service, Value, array, digest, entity, entity_mut, err, id, json, now, push,
    revision, whiteboard24,
};

pub(super) fn spec(state: &Value) -> Value {
    if state["problem_spec"].is_object() {
        return state["problem_spec"].clone();
    }
    json!({"math_statement":state["problem"],"research_description":"","original_input":array(state,"problem_versions").first().and_then(|v|v.get("problem")).unwrap_or(&state["problem"]),
        "revision":1,"problem_version":state["problem_version"],"normalization_state":"pending",
        "source":"original_input","updated_at":state["created_at"]})
}

fn text<'a>(value: &'a Value, field: &str) -> V2Result<&'a str> {
    value[field]
        .as_str()
        .filter(|v| !v.trim().is_empty() && v.len() <= 1_000_000)
        .ok_or_else(|| err("INVALID_INPUT", format!("{field} 不能为空或过长")))
}

pub(super) fn statements(action: &Value, node: &Value) -> V2Result<Value> {
    let Some(value) = action.get("mathematical_statements") else {
        return Ok(json!([]));
    };
    let items = value
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 32)
        .ok_or_else(|| err("INVALID_FINDING", "数学陈述列表需要1至32项"))?;
    let mut result = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let kind = text(item, "kind")?;
        if !matches!(
            kind,
            "theorem"
                | "lemma"
                | "proposition"
                | "conjecture"
                | "definition"
                | "example"
                | "counterexample"
        ) {
            return Err(err("INVALID_FINDING", "陈述的数学类型无效"));
        }
        let statement = text(item, "statement")?;
        result.push(json!({"id":format!("{}:r{}:statement:{}",node["id"].as_str().unwrap_or_default(),node["revision"],index+1),"kind":kind,"title":item["title"],"statement":statement,"assumptions":item["assumptions"],"scope":item["scope"],"assurance":"unreviewed","source_artifact_id":node["body_artifact_id"],"statement_sha256":digest(statement)}));
    }
    Ok(json!(result))
}

fn save_spec(state: &mut Value, mut next: Value) {
    let before = spec(state);
    if before["problem_version"] != state["problem_version"] {
        next["problem_source_refs"] = json!([]);
        next["source_artifact_id"] = Value::Null;
    }
    push(state, "problem_spec_history", before.clone());
    next["revision"] = json!(before["revision"].as_u64().unwrap_or(1) + 1);
    next["original_input"] = before["original_input"].clone();
    next["problem_version"] = state["problem_version"].clone();
    next["updated_at"] = json!(now());
    state["problem_spec"] = next;
    super::proof_tree23::sync_problem_spec(state);
}

pub(super) fn apply_replacement(state: &mut Value, preview: &Value) {
    let mut next = if preview["problem_spec"].is_object() {
        preview["problem_spec"].clone()
    } else {
        spec(state)
    };
    next["math_statement"] = state["problem"].clone();
    next["normalization_state"] = json!("human_edited");
    save_spec(state, next);
}

fn references(state: &Value, action: &Value) -> V2Result<Vec<Value>> {
    let refs = action
        .get("evidence_refs")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let refs = refs
        .as_array()
        .ok_or_else(|| err("INVALID_INPUT", "evidence_refs 必须为数组"))?;
    for r in refs {
        let key = r
            .as_str()
            .or_else(|| r["id"].as_str())
            .ok_or_else(|| err("INVALID_INPUT", "依据需要已有ID"))?;
        if ![
            "artifacts",
            "nodes",
            "candidates",
            "facts",
            "reviews",
            "proof_checkpoints",
        ]
        .iter()
        .any(|c| entity(state, c, key).is_some())
        {
            return Err(err("MATERIAL_MISSING", "依据不存在"));
        }
    }
    Ok(refs.clone())
}

pub(super) fn apply_action(
    state: &mut Value,
    run: &str,
    session: &str,
    action: &Value,
    artifact: &Value,
) -> V2Result<Option<Value>> {
    let kind = action["type"].as_str().unwrap_or_default();
    if !matches!(
        kind,
        "frame_problem" | "record_open_question" | "resolve_open_question" | "record_route_outcome"
    ) {
        return Ok(None);
    }
    let actor = entity(state, "sessions", session)
        .cloned()
        .ok_or_else(|| err("NOT_FOUND", "会话不存在"))?;
    if actor["run_id"] != run || !matches!(actor["role"].as_str(), Some("main" | "partner")) {
        return Err(err("INVALID_ROLE", "仅研究猫可提交研究记录"));
    }
    if kind == "frame_problem" {
        let current = spec(state);
        if actor["role"] != "main" {
            return Err(err("INVALID_ROLE", "只有领研猫整理原始题面"));
        }
        if current["normalization_state"] != "pending"
            || action["expected_revision"] != current["revision"]
        {
            return Err(err("REVISION_CONFLICT", "题面已整理或已由人类修改"));
        }
        let math = text(action, "math_statement")?;
        let description = action["research_description"]
            .as_str()
            .filter(|v| v.len() <= 1_000_000)
            .ok_or_else(|| err("INVALID_INPUT", "research_description 必须是文本"))?;
        let mut next = current;
        next["problem_source_refs"] =
            json!(super::problem_context::framing_sources(state, action)?);
        next["math_statement"] = json!(math);
        next["research_description"] = if matches!(
            next["source"].as_str(),
            Some("direct_edit" | "conversation_edit")
        ) && next["research_description"]
            .as_str()
            .is_some_and(|v| !v.trim().is_empty() && v != description)
        {
            json!(format!(
                "{}\n\n原始输入中的其他说明：\n{description}",
                next["research_description"].as_str().unwrap_or_default()
            ))
        } else {
            json!(description)
        };
        next["normalization_state"] = json!("model_structured");
        next["source"] = json!("main_cat");
        next["source_artifact_id"] = artifact["id"].clone();
        next["mathematical_equivalence"] = json!("not_independently_verified");
        // This is presentation normalization, not permission to redefine the
        // original research target. Both are supplied to every later turn.
        save_spec(state, next);
        return Ok(Some(state["problem_spec"].clone()));
    }
    if kind == "resolve_open_question" {
        let qid = text(action, "question_id")?;
        let refs = references(state, action)?;
        if refs.is_empty() {
            return Err(err("MATERIAL_MISSING", "解决疑点需要已保存依据"));
        }
        let reason = text(action, "resolution")?;
        let q = entity_mut(state, "open_questions", qid)?;
        if q["run_id"] != run || q["revision"] != action["expected_revision"] {
            return Err(err("REVISION_CONFLICT", "疑点版本或运行已变化"));
        }
        q["state"] = json!("resolved");
        q["resolution"] = json!(reason);
        q["resolution_refs"] = json!(refs);
        q["resolved_by"] = json!(session);
        q["resolved_at"] = json!(now());
        q["resolution_assurance"] = json!("researcher_reported");
        revision(q);
        return Ok(Some(q.clone()));
    }
    let statement = text(
        action,
        if kind == "record_open_question" {
            "statement"
        } else {
            "goal"
        },
    )?;
    let refs = references(state, action)?;
    let route = action["route_id"]
        .as_str()
        .or_else(|| actor["route_id"].as_str());
    if route.is_some_and(|r| entity(state, "routes", r).is_none_or(|r| r["run_id"] != run)) {
        return Err(err("INVALID_TARGET", "路线不属于当前研究"));
    }
    let mut row = json!({"id":id(),"run_id":run,"problem_version":state["problem_version"],"revision":1,
        "title":action["title"],"statement":statement,"conditions":action["conditions"],"route_id":route,
        "owner_session_id":session,"affected_refs":action["affected_refs"],"evidence_refs":refs,
        "source_artifact_id":artifact["id"],"state":"open","assurance":"unreviewed","created_at":now()});
    let collection = if kind == "record_open_question" {
        "open_questions"
    } else {
        let outcome = text(action, "outcome")?;
        if !matches!(
            outcome,
            "mathematical_failure"
                | "blocked"
                | "abandoned"
                | "human_stopped"
                | "runtime_failure"
                | "time_limit"
        ) {
            return Err(err("INVALID_OUTCOME", "不能识别的路线结局"));
        }
        if outcome == "mathematical_failure" && refs.is_empty() {
            return Err(err("MATERIAL_MISSING", "数学失败需引用反例或论证原文"));
        }
        row["goal"] = json!(statement);
        row["method"] = action["method"].clone();
        row["reason"] = json!(text(action, "reason")?);
        row["outcome"] = json!(outcome);
        row["scope"] = action["scope"].clone();
        row["useful_result_refs"] = action["useful_result_refs"].clone();
        row["state"] = json!("recorded");
        "route_outcomes"
    };
    push(state, collection, row.clone());
    Ok(Some(row))
}

pub(super) fn project(state: &mut Value) {
    state["problem_spec"] = spec(state);
    if state["planning_proposals"].is_null() {
        state["planning_proposals"] = json!([]);
    }
    let mut questions = array(state, "open_questions").to_vec();
    let mut outcomes = array(state, "route_outcomes").to_vec();
    for command in array(state, "commands") {
        if command["type"] == "prohibit_route" && command["status"] == "completed" {
            let route_id = command["target"]["id"]
                .as_str()
                .or_else(|| command["payload"]["route_id"].as_str())
                .unwrap_or_default();
            let route = entity(state, "routes", route_id)
                .cloned()
                .unwrap_or(Value::Null);
            outcomes.push(json!({"id":format!("control:{}",command["id"].as_str().unwrap_or_default()),"run_id":command["run_id"],"problem_version":route["problem_version"],"goal":route["title"],"route_id":route_id,"outcome":"human_stopped","reason":command["payload"]["reason"],"state":"recorded","source_kind":"human_route_control","evidence_refs":[command["id"]],"assurance":"execution_status_only","created_at":command["created_at"]}));
        }
    }
    for review in array(state, "reviews") {
        let Some(candidate) = review["candidate_id"]
            .as_str()
            .and_then(|c| entity(state, "candidates", c))
        else {
            continue;
        };
        if review["state"] == "completed"
            && matches!(
                review["verdict"].as_str(),
                Some(
                    "reject"
                        | "rejected"
                        | "needs_revision"
                        | "uncertain"
                        | "inconclusive"
                        | "wrong"
                        | "incorrect"
                )
            )
        {
            let mut issues = array(review, "issues").to_vec();
            for field in [
                "gaps",
                "critical_errors",
                "applicability_gaps",
                "unresolved_materials",
            ] {
                issues.extend_from_slice(array(&review["verification_report"], field));
            }
            questions.push(json!({"id":format!("review:{}",review["id"].as_str().unwrap_or_default()),"run_id":candidate["run_id"],"problem_version":candidate["problem_version"],"title":"审核指出的证明疑点","statement":candidate.get("exact_statement").unwrap_or(&candidate["claim"]),"issues":issues,"state":if candidate["status"]=="accepted"{"resolved"}else{"open"},"owner_session_id":candidate["author_session_id"],"candidate_id":candidate["id"],"evidence_refs":[review["id"],candidate["id"]],"source_artifact_id":review["report_artifact_id"],"source_kind":"review","assurance":"review_observation","created_at":review["created_at"]}));
        }
    }
    for s in array(state, "sessions") {
        if matches!(s["role"].as_str(), Some("main" | "partner")) && s["last_error"].is_object() {
            outcomes.push(json!({"id":format!("runtime:{}",s["id"].as_str().unwrap_or_default()),"run_id":s["run_id"],"problem_version":s["problem_version"],"goal":s["focus"],"route_id":s["route_id"],"owner_session_id":s["id"],"outcome":if s["last_error"]["code"]=="TIMEOUT"{"time_limit"}else{"runtime_failure"},"reason":s["last_error"],"state":"recorded","source_kind":"session_error","assurance":"execution_status_only","created_at":s["updated_at"]}));
        }
    }
    state["open_questions"] = json!(questions);
    state["route_outcomes"] = json!(outcomes);
}

impl V2Service {
    #[must_use]
    pub fn whiteboard_from_snapshot24(state: &Value) -> Value {
        let mut result = state.clone();
        project(&mut result);
        result
    }
    pub async fn problem_spec24(&self, project: &str) -> V2Result<Value> {
        Ok(spec(&self.store.read(project).await?))
    }

    pub async fn update_problem_spec24(
        &self,
        project: &str,
        input: &Value,
        key: &str,
    ) -> V2Result<Value> {
        let hash = digest(&json!({"operation":"problem_spec","input":input}).to_string());
        let result=self.store.mutate(project,"problem.edit_requested",None,|state|{
            if let Some(old)=whiteboard24::replay(state,key,&hash)? {return Ok(old);}
            let mut next=spec(state);
            if input["expected_revision"]!=next["revision"] || input["expected_revision"].as_u64().is_none(){return Err(err("REVISION_CONFLICT","题面版本已变化"));}
            for field in ["math_statement","research_description"] {
                if let Some(v)=input.get(field) {
                    if v.as_str().is_none_or(|v|v.len()>1_000_000 || field=="math_statement"&&v.trim().is_empty()){return Err(err("INVALID_PROBLEM","问题或说明格式无效"));}
                    next[field]=v.clone();
                }
            }
            if input["source"].as_str().is_some_and(|s|!matches!(s,"direct_edit"|"conversation_edit")){return Err(err("INVALID_INPUT","source 必须为 direct_edit/conversation_edit"));}
            next["source"]=input.get("source").cloned().unwrap_or_else(||json!("direct_edit"));
            let math_changed=input.get("math_statement").is_some_and(|m|*m!=spec(state)["math_statement"]);
            next["normalization_state"]=if math_changed{json!("human_edited")}else{spec(state)["normalization_state"].clone()};
            let active=array(state,"runs").iter().rev().find(|r|r["state"]!="ended").cloned();
            let result=if let Some(run)=active.filter(|_|math_changed) {
                if !matches!(run["state"].as_str(),Some("running"|"waiting_human"|"paused")){return Err(err("INVALID_STATE","请等待运行完成启动或停止后再修改数学题面"));}
                let affected:Vec<Value>=["tasks","candidates","reviews","facts"].iter().flat_map(|name|array(state,name).iter().map(move|item|json!({"kind":name,"id":item["id"],"revision":item["revision"],"state":item["state"],"status":item["status"]}))).collect();
                let preview=json!({"id":id(),"actor_id":"local-owner","base_problem_version":state["problem_version"],"base_control_epoch":run["control_epoch"],"base_run_id":run["id"],"problem_spec":next,"expected_spec_revision":input["expected_revision"],"new_problem":next["math_statement"],"affected_refs":affected,"impact_set_hash":digest(&json!(affected).to_string()),"expires_at":(chrono::Utc::now()+chrono::Duration::minutes(10)).to_rfc3339()});
                push(state,"previews",preview.clone());
                let command=json!({"id":id(),"run_id":run["id"],"type":"replace_problem","priority":"normal","payload":{"confirmed_impact_preview_id":preview["id"]},"status":"queued","revision":1,"idempotency_key":format!("problem-spec:{key}"),"created_at":now()});
                push(state,"commands",command.clone());json!({"command":command})
            } else {
                if math_changed {state["problem"]=next["math_statement"].clone();state["problem_version"]=json!(state["problem_version"].as_u64().unwrap_or(1)+1);push(state,"problem_versions",json!({"version":state["problem_version"],"problem":state["problem"],"created_at":now()}));super::proof_tree23::record_problem_version(state);super::refresh_result_states(state);}
                save_spec(state,next);json!({"problem_spec":state["problem_spec"]})
            };
            whiteboard24::receipt(state,key,&hash,&result);Ok(result)
        }).await?;
        if result["problem_spec"].is_object() {
            return Ok(result["problem_spec"].clone());
        }
        if let Err(error) = self.apply_control(project, &result["command"]).await {
            self.store
                .mutate(project, "command.failed", None, |state| {
                    let c = entity_mut(
                        state,
                        "commands",
                        result["command"]["id"].as_str().unwrap_or_default(),
                    )?;
                    c["status"] = json!("failed");
                    c["error"] = json!({"code":error.code,"message":error.message});
                    revision(c);
                    Ok(Value::Null)
                })
                .await?;
            return Err(error);
        }
        let state = self.store.read(project).await?;
        entity(
            &state,
            "commands",
            result["command"]["id"].as_str().unwrap_or_default(),
        )
        .and_then(|c| c.get("applied_problem_spec"))
        .filter(|v| v.is_object())
        .cloned()
        .ok_or_else(|| {
            err(
                "REVISION_CONFLICT",
                "改题未产生已绑定的版本回执，请刷新后检查命令状态",
            )
        })
    }
}
