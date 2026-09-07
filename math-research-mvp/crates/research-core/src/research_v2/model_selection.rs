//! Project execution preferences change future invocations, never an in-flight request.
use super::{
    V2Result, V2Service, Value, array, digest, entity, entity_mut, err, id, json, now, push,
    revision,
};

pub(super) fn selection(state: &Value) -> Value {
    if state["model_selection"].is_object() {
        state["model_selection"].clone()
    } else {
        json!({"model":null,"reasoning_effort":null,"revision":0,"updated_at":null})
    }
}

/// A Run's explicit startup choice wins until the project preference is changed again.
pub(super) fn effective(
    state: &Value,
    run: Option<&Value>,
    default_model: Option<&str>,
    default_effort: Option<&str>,
) -> Value {
    let project = selection(state);
    let changed = project["revision"].as_u64().unwrap_or(0)
        > run
            .and_then(|r| r["model_selection_revision"].as_u64())
            .unwrap_or(0);
    let value = |field: &str, fallback: Option<&str>| {
        let project_value = project[field].as_str();
        let run_value = run.and_then(|r| r[field].as_str());
        json!(
            if changed {
                project_value.or(run_value)
            } else {
                run_value.or(project_value)
            }
            .or(fallback)
        )
    };
    json!({"model":value("model",default_model),"reasoning_effort":value("reasoning_effort",default_effort),"revision":project["revision"],"source":if changed {"project"} else if run.is_some() {"run"} else {"project_or_default"}})
}

pub(super) fn capture(
    state: &mut Value,
    session_id: &str,
    invocation_id: &str,
    selected: &Value,
) -> V2Result<()> {
    let session = entity_mut(state, "sessions", session_id)?;
    session["model"] = selected["model"].clone();
    session["reasoning_effort"] = selected["reasoning_effort"].clone();
    session["current_model_selection"] = selected.clone();
    session["pending_model_selection"] = Value::Null;
    let usage = entity_mut(state, "usage", invocation_id)?;
    usage["model"] = selected["model"].clone();
    usage["reasoning_effort"] = selected["reasoning_effort"].clone();
    usage["model_selection_revision"] = selected["revision"].clone();
    Ok(())
}

impl V2Service {
    #[must_use]
    pub fn model_selection_from_snapshot(state: &Value) -> Value {
        selection(state)
    }

    pub async fn set_model_selection(
        &self,
        project: &str,
        input: Value,
        key: &str,
    ) -> V2Result<Value> {
        if key.trim().is_empty() || key.len() > 200 {
            return Err(err("IDEMPOTENCY_REQUIRED", "模型选择需要幂等键"));
        }
        if input.as_object().is_none_or(|o| {
            o.keys().any(|k| {
                !matches!(
                    k.as_str(),
                    "model" | "reasoning_effort" | "expected_revision"
                )
            })
        }) {
            return Err(err(
                "INVALID_REQUEST",
                "模型选择仅支持 model、reasoning_effort 和 expected_revision",
            ));
        }
        let model = input["model"]
            .as_str()
            .ok_or_else(|| err("INVALID_MODEL_CONFIG", "model 必须为字符串"))?;
        let effort = input["reasoning_effort"]
            .as_str()
            .ok_or_else(|| err("INVALID_MODEL_CONFIG", "reasoning_effort 必须为字符串"))?;
        if !input["expected_revision"].is_null() && input["expected_revision"].as_u64().is_none() {
            return Err(err("INVALID_REQUEST", "expected_revision 必须为非负整数"));
        }
        research_worker_runtime::research_v2::execution_overrides(Some(model), Some(effort))
            .map_err(|e| err(&e.code, e.message))?;
        let hash = digest(&input.to_string());
        self.store.mutate(project,"project.model_selection_updated",None,|state| {
            if let Some(prior)=array(state,"model_selection_operations").iter().find(|o|o["idempotency_key"]==key) {
                if prior["request_hash"]!=hash {return Err(err("IDEMPOTENCY_CONFLICT","同一模型选择键对应不同请求"));}
                return Ok(prior["result"].clone());
            }
            let prior=selection(state);
            if input["expected_revision"].as_u64().is_some_and(|v|Some(v)!=prior["revision"].as_u64()) {
                return Err(err("REVISION_CONFLICT","项目模型选择已改变，请刷新后再修改"));
            }
            let value=json!({"model":model,"reasoning_effort":effort,"revision":prior["revision"].as_u64().unwrap_or(0)+1,"updated_at":now()});
            state["model_selection"]=value.clone();
            let updates=array(state,"sessions").iter().filter(|s| !matches!(s["state"].as_str(),Some("closed"|"lost")) && matches!(s["role"].as_str(),Some("main"|"partner"|"reviewer"|"advisor"|"memory"|"display"|"discussion"))).map(|s| {
                let run=s["run_id"].as_str().and_then(|r|entity(state,"runs",r)).or_else(||array(state,"runs").last());
                (s["id"].as_str().unwrap_or_default().to_owned(),effective(state,run,self.config.model.as_deref(),self.config.reasoning_effort.as_deref()))
            }).collect::<Vec<_>>();
            for (session_id,next) in updates {
                let session=entity_mut(state,"sessions",&session_id)?;
                session["pending_model_selection"]=next;
                revision(session);
            }
            push(state,"model_selection_operations",json!({"id":id(),"idempotency_key":key,"request_hash":hash,"result":value,"created_at":now()}));
            Ok(value)
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::research_v2::{V2Config, V2Store};
    use tokio_util::sync::CancellationToken;

    #[test]
    fn explicit_run_choice_wins_until_a_later_project_revision() {
        let mut state =
            json!({"model_selection":{"model":"project-a","reasoning_effort":"high","revision":3}});
        let run =
            json!({"model":"explicit-run","reasoning_effort":"low","model_selection_revision":3});
        assert_eq!(
            effective(&state, Some(&run), None, None)["model"],
            "explicit-run"
        );
        state["model_selection"] =
            json!({"model":"project-b","reasoning_effort":"ultra","revision":4});
        let selected = effective(&state, Some(&run), None, None);
        assert_eq!(selected["model"], "project-b");
        assert_eq!(selected["reasoning_effort"], "ultra");
        assert_eq!(
            effective(&json!({}), None, Some("default"), Some("medium"))["model"],
            "default"
        );
        assert_eq!(
            effective(&state, None, None, None),
            effective(&state, Some(&json!({"model":"old-schema"})), None, None)
        );
    }

    #[tokio::test]
    async fn preference_update_preserves_active_request_history_and_applies_to_next_capture() {
        let tmp = tempfile::tempdir().unwrap();
        let store = V2Store::connect(&tmp.path().join("models.sqlite"), &tmp.path().join("data"))
            .await
            .unwrap();
        let project = store
            .create_project(json!({"problem":"model fixture"}), "create")
            .await
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let service = V2Service::new(store, V2Config::default());
        let cancel = CancellationToken::new();
        service
            .turns
            .lock()
            .await
            .insert("main".into(), cancel.clone());
        service.store.mutate(&project,"fixture.models",None,|state| {
            push(state,"runs",json!({"id":"run","state":"running","model":"old","reasoning_effort":"low","model_selection_revision":0,"control_epoch":7}));
            push(state,"sessions",json!({"id":"main","run_id":"run","role":"main","state":"active","model":"old","reasoning_effort":"low","revision":1}));
            push(state,"usage",json!({"id":"old-call","session_id":"main","state":"running","model":"old","reasoning_effort":"low"}));Ok(Value::Null)
        }).await.unwrap();
        let before = service.store.read(&project).await.unwrap();
        let input = json!({"model":"new","reasoning_effort":"max","expected_revision":0});
        let result = service
            .set_model_selection(&project, input.clone(), "change")
            .await
            .unwrap();
        assert_eq!(result["revision"], 1);
        assert_eq!(
            service
                .set_model_selection(&project, input, "change")
                .await
                .unwrap(),
            result
        );
        let changed = service.store.read(&project).await.unwrap();
        assert_eq!(changed["usage"], before["usage"]);
        assert_eq!(changed["runs"], before["runs"]);
        assert_eq!(changed["sessions"][0]["model"], "old");
        assert_eq!(
            changed["sessions"][0]["pending_model_selection"]["model"],
            "new"
        );
        assert!(!cancel.is_cancelled());
        assert_eq!(
            service
                .set_model_selection(
                    &project,
                    json!({"model":"third","reasoning_effort":"high","expected_revision":0}),
                    "stale"
                )
                .await
                .unwrap_err()
                .code,
            "REVISION_CONFLICT"
        );
        service
            .store
            .mutate(&project, "fixture.next_call", None, |state| {
                let selected = effective(state, array(state, "runs").last(), None, None);
                push(
                    state,
                    "usage",
                    json!({"id":"next-call","session_id":"main","state":"reserved"}),
                );
                capture(state, "main", "next-call", &selected)?;
                Ok(Value::Null)
            })
            .await
            .unwrap();
        let next = service.store.read(&project).await.unwrap();
        assert_eq!(next["usage"][0], before["usage"][0]);
        assert_eq!(next["usage"][1]["model"], "new");
        assert_eq!(next["usage"][1]["reasoning_effort"], "max");
        assert_eq!(next["sessions"][0]["model"], "new");
        assert!(next["sessions"][0]["pending_model_selection"].is_null());
    }
}
