//! Host-owned exact evidence, premise declarations and conditional-work accounting.
//! A local file-tool journal is never proof that a model received or used a premise.
use super::{array, digest, entity, entity_mut, err, id, now, push};
use research_storage::research_v2::{V2Result, V2Store};
use serde_json::{Value, json};
use std::collections::HashSet;

const MAX_SUPPLY_CHARACTERS: usize = 180_000;

fn current_fact(state: &Value, fact: &Value, visited: &mut HashSet<String>) -> bool {
    let key = fact["id"].as_str().unwrap_or_default();
    if key.is_empty()
        || !visited.insert(key.into())
        || fact["validity"] != "current"
        || fact["assurance"] != "model_reviewed"
        || fact["problem_version"] != state["problem_version"]
    {
        return false;
    }
    let valid = array(fact, "dependency_ids").iter().all(|dependency| {
        entity(state, "facts", dependency.as_str().unwrap_or_default())
            .is_some_and(|parent| current_fact(state, parent, visited))
    });
    visited.remove(key);
    valid
}

fn statement_for(state: &Value, reference: &str) -> Option<Value> {
    if let Some(fact) = entity(state, "facts", reference) {
        let candidate = entity(
            state,
            "candidates",
            fact["candidate_id"].as_str().unwrap_or_default(),
        );
        let text = candidate
            .and_then(|item| item["exact_statement"].as_str())
            .or_else(|| fact["claim"].as_str())?;
        return Some(json!({"id":reference,"kind":"fact","text":text,
            "sha256":digest(text),"revision":fact["revision"].as_u64().unwrap_or(1),
            "problem_version":fact["problem_version"],"candidate_id":fact["candidate_id"],
            "source_artifact_id":candidate.map(|item|item["statement_artifact_id"].clone()),
            "proof_artifact_id":fact["proof_artifact_id"],
            "proof_sha256":fact["proof_sha256"],"snapshot_hash":fact["snapshot_hash"],
            "validity":if current_fact(state,fact,&mut HashSet::new()){"current"}else{"unavailable"},
            "assurance":fact["assurance"],"exact":true}));
    }
    if let Some(candidate) = entity(state, "candidates", reference) {
        let text = candidate["exact_statement"]
            .as_str()
            .or_else(|| candidate["claim"].as_str())?;
        return Some(json!({"id":reference,"kind":"candidate","text":text,
            "sha256":digest(text),"revision":candidate["revision"].as_u64().unwrap_or(1),
            "problem_version":candidate["problem_version"],
            "source_artifact_id":candidate["statement_artifact_id"],
            "proof_artifact_id":candidate["proof_artifact_id"],
            "snapshot_hash":candidate["snapshot_hash"],
            "validity":candidate["status"],"assurance":"unreviewed","exact":true}));
    }
    if let Some(node) = entity(state, "nodes", reference) {
        let text = node["exact_statement"]
            .as_str()
            .or_else(|| node["finding"]["statement"].as_str())
            .or_else(|| node["body"].as_str())?;
        return Some(
            json!({"id":reference,"kind":"node","text":text,"sha256":digest(text),
            "revision":node["revision"],"problem_version":node["problem_version"],
            "source_artifact_id":node["statement_artifact_id"],"body_artifact_id":node["body_artifact_id"],"draft_artifact_refs":node["draft_artifact_refs"],
            "validity":node["validity"],"assurance":"unreviewed","exact":true}),
        );
    }
    entity(state, "artifacts", reference).map(|artifact| {
        json!({"id":reference,"kind":"source","artifact_id":reference,
            "sha256":artifact["sha256"],"revision":1,"name":artifact["name"],
            "assurance":"unreviewed","exact":true})
    })
}

fn statement_catalog(state: &Value) -> Vec<Value> {
    ["facts", "candidates", "nodes"]
        .iter()
        .flat_map(|collection| array(state, collection))
        .filter_map(|item| {
            item["id"]
                .as_str()
                .and_then(|key| statement_for(state, key))
        })
        .collect()
}

fn unavailable_statement(
    key: &str,
    statement: &Value,
    code: &str,
    reason: &str,
    blocked: bool,
) -> Value {
    json!({"id":key,"code":code,"reason":reason,"blocked":blocked,
        "revision":statement["revision"],"sha256":statement["sha256"]})
}

/// Materialize exact, untruncated statements for the actual next model input.
/// Pending submission reads are served before ordinary current facts. Large inputs
/// require an explicit extraction/version, never a fabricated successful receipt.
pub(super) async fn prepare_turn(
    store: &V2Store,
    project: &str,
    prepared: &mut Value,
    invocation_id: &str,
) -> V2Result<()> {
    let state = store.read(project).await?;
    let session_id = prepared["session"]["id"].as_str().unwrap_or_default();
    let session = entity(&state, "sessions", session_id).unwrap_or(&prepared["session"]);
    let prior_epoch = session["evidence_context_epoch"].as_u64().unwrap_or(1);
    let has_reads = array(&state, "evidence_reads")
        .iter()
        .any(|read| read["session_id"] == session_id);
    let rebound = has_reads
        && (session["native_session_id"].is_null()
            || (session["evidence_native_binding"].is_string()
                && session["evidence_native_binding"] != session["native_session_id"]));
    let context_epoch = if rebound {
        prior_epoch.saturating_add(1)
    } else {
        prior_epoch
    };
    if entity(&state, "sessions", session_id).is_some() {
        store
            .mutate(project, "evidence.context_prepared", None, |state| {
                let current = entity_mut(state, "sessions", session_id)?;
                current["evidence_context_epoch"] = json!(context_epoch);
                current["evidence_native_binding"] = session["native_session_id"].clone();
                Ok(Value::Null)
            })
            .await?;
    }
    let pending: Vec<String> = array(session, "pending_statement_ids")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let mut keys = pending.clone();
    keys.extend(
        array(session, "pending_premise_ids")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned),
    );
    if session["role"] == "reviewer" {
        keys.clear();
        keys.extend(
            array(&session["candidate_snapshot"], "dependency_ids")
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    } else {
        keys.extend(
            array(&state, "facts")
                .iter()
                .rev()
                .take(12)
                .filter_map(|fact| fact["id"].as_str())
                .map(str::to_owned),
        );
    }
    let mut visited = HashSet::new();
    let mut supplied = Vec::new();
    let mut unavailable = Vec::new();
    let mut total = 0;
    for key in keys {
        if !visited.insert(key.clone()) {
            continue;
        }
        let Some(mut statement) = statement_for(&state, &key) else {
            unavailable.push(unavailable_statement(
                &key,
                &Value::Null,
                "STATEMENT_MISSING",
                "Reference is no longer available",
                true,
            ));
            continue;
        };
        let source_id = if statement["kind"] == "source" {
            Some(key.clone())
        } else {
            statement["source_artifact_id"].as_str().map(str::to_owned)
        };
        if let Some(source_id) = source_id {
            match store.read_artifact(project, &source_id).await {
                Ok((artifact, bytes)) => {
                    if let Ok(text) = String::from_utf8(bytes) {
                        if statement["kind"] != "source"
                            && (statement["text"] != text || statement["sha256"] != digest(&text))
                        {
                            unavailable.push(unavailable_statement(
                                &key,
                                &statement,
                                "STATEMENT_MISMATCH",
                                "Original statement artifact disagrees with the frozen statement",
                                true,
                            ));
                            continue;
                        }
                        statement["text"] = json!(text);
                        statement["sha256"] = artifact["sha256"].clone();
                    } else {
                        unavailable.push(unavailable_statement(
                            &key,
                            &statement,
                            "SOURCE_TEXT_EXTRACTION_REQUIRED",
                            "Source needs a UTF-8 text extraction before premise use",
                            true,
                        ));
                        continue;
                    }
                }
                Err(error) => {
                    unavailable.push(unavailable_statement(
                        &key,
                        &statement,
                        &error.code,
                        &error.message,
                        true,
                    ));
                    continue;
                }
            }
        }
        let size = statement["text"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .count();
        if size == 0 || size > MAX_SUPPLY_CHARACTERS {
            unavailable.push(unavailable_statement(&key, &statement,
                if size == 0 {"STATEMENT_EMPTY"} else {"STATEMENT_TOO_LARGE"},
                "Exact text is empty or exceeds the per-statement allowance; provide a scoped immutable extraction, not a summary", true));
            continue;
        }
        if total + size > MAX_SUPPLY_CHARACTERS {
            if pending.contains(&key) {
                unavailable.push(unavailable_statement(
                    &key,
                    &statement,
                    "EVIDENCE_BATCH_FULL",
                    "Exact text will be supplied in a later input after the current batch",
                    false,
                ));
            }
            continue;
        }
        total += size;
        statement["supplied_range"] =
            json!({"start":0,"end":size,"unit":"unicode_scalars","complete":true});
        supplied.push(statement);
    }
    prepared["evidence_context_epoch"] = json!(context_epoch);
    prepared["invocation_id"] = json!(invocation_id);
    prepared["exact_statements"] = json!({"notice":"Exact source statements follow separately from summaries. Their source status is not a certification of any free summary. Verify all hypotheses, quantifiers, parameter ranges and constant dependence before use.","statements":supplied,"unavailable":unavailable});
    prepared["statement_supply"] = prepared["exact_statements"]["statements"].clone();
    prepared["statement_unavailable"] = prepared["exact_statements"]["unavailable"].clone();
    Ok(())
}

/// Call only after the backend accepted/completed the exact prepared input.
/// Neither model actions nor a writable tool journal may call this trust path.
pub(super) fn record_supplied(state: &mut Value, prepared: &Value, invocation_id: &str) {
    settle_conditional(state, invocation_id, Some(true));
    let session_id = prepared["session"]["id"].clone();
    for statement in array(prepared, "statement_supply") {
        if array(state, "evidence_reads").iter().any(|read| {
            read["invocation_id"] == invocation_id && read["statement_id"] == statement["id"]
        }) {
            continue;
        }
        push(
            state,
            "evidence_reads",
            json!({"id":id(),"session_id":session_id,
            "invocation_id":invocation_id,"statement_id":statement["id"],
            "source_kind":statement["kind"],"revision":statement["revision"],
            "sha256":statement["sha256"],"supplied_range":statement["supplied_range"],
            "context_epoch":prepared["evidence_context_epoch"].as_u64().unwrap_or(1),
            "origin":"host_prompt","supplied_at":now()}),
        );
    }
    let ids: HashSet<String> = array(prepared, "statement_supply")
        .iter()
        .filter_map(|s| s["id"].as_str())
        .map(str::to_owned)
        .collect();
    if let Ok(session) = entity_mut(state, "sessions", session_id.as_str().unwrap_or_default()) {
        session["pending_statement_ids"] = json!(
            array(session, "pending_statement_ids")
                .iter()
                .filter(|key| !ids.contains(key.as_str().unwrap_or_default()))
                .cloned()
                .collect::<Vec<_>>()
        );
        let checked_ids: HashSet<&str> = array(prepared, "statement_unavailable")
            .iter()
            .filter_map(|item| item["id"].as_str())
            .collect();
        let mut blockers: Vec<Value> = array(session, "statement_blockers")
            .iter()
            .filter(|item| {
                let key = item["id"].as_str().unwrap_or_default();
                !ids.contains(key) && !checked_ids.contains(key)
            })
            .cloned()
            .collect();
        blockers.extend(
            array(prepared, "statement_unavailable")
                .iter()
                .filter(|item| item["blocked"] == true)
                .cloned(),
        );
        session["statement_blockers"] = json!(blockers);
    }
}

/// `Some(false)` is allowed only when the host confirms no backend dispatch.
/// Unknown delivery continues to consume the shared premise allowance.
pub(super) fn settle_conditional(state: &mut Value, invocation_id: &str, dispatched: Option<bool>) {
    if let Some(reservations) = state["conditional_reservations"].as_array_mut() {
        for reservation in reservations
            .iter_mut()
            .filter(|r| r["invocation_id"] == invocation_id)
        {
            if reservation["state"] == "used" && dispatched != Some(true) {
                continue;
            }
            reservation["state"] = json!(match dispatched {
                Some(true) => "used",
                Some(false) => "not_dispatched",
                None => "delivery_unknown",
            });
            reservation["settled_at"] = json!(now());
        }
    }
}

/// Facts and historical experience use distinct retrieval paths and status rules.
pub(super) fn augment_context(state: &Value, prepared: &Value, context: &mut Value) {
    context["schema_version"] = json!(2);
    context["snapshot_revision"] = state["revision"].clone();
    context["session_id"] = prepared["session"]["id"].clone();
    context["invocation_id"] = prepared["invocation_id"].clone();
    let reviewer = prepared["session"]["role"] == "reviewer";
    if reviewer {
        context["statements"] = json!([]);
        context["fact_records"] = json!([]);
        context["experience_records"] = json!([]);
        return;
    }
    context["problem_spec"] = super::problem24::spec(state);
    context["problem_input_manifest"] = json!(super::problem_context::manifest(state));
    context["planning_proposals"] = json!(
        array(state, "planning_proposals")
            .iter()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
    );
    context["open_questions"] = state
        .get("open_questions")
        .cloned()
        .unwrap_or_else(|| json!([]));
    context["route_outcomes"] = state
        .get("route_outcomes")
        .cloned()
        .unwrap_or_else(|| json!([]));
    context["statements"] = json!(statement_catalog(state));
    context["fact_records"] = json!(
        array(state, "facts")
            .iter()
            .filter(|fact| current_fact(state, fact, &mut HashSet::new()))
            .cloned()
            .map(|mut fact| {
                fact["collection"] = json!("facts");
                fact
            })
            .collect::<Vec<_>>()
    );
    let experiences: Vec<Value> = ["memories", "memory_entries", "candidates", "facts", "nodes"]
        .iter()
        .flat_map(|collection| {
            array(state, collection)
                .iter()
                .map(move |item| (*collection, item))
        })
        .filter(|(_, item)| {
            item["recipient_session_id"].is_null()
                || item["recipient_session_id"] == prepared["session"]["id"]
        })
        .map(|(collection, item)| {
            let mut item = item.clone();
            item["collection"] = json!(collection);
            item["retrieval_use"] = json!("experience_only_not_a_proof_premise");
            if collection == "facts" {
                item["currently_usable"] = json!(current_fact(state, &item, &mut HashSet::new()));
            }
            if matches!(collection, "memories" | "memory_entries") {
                item["summary_assurance"] = json!("unreviewed_summary");
                item["assurance"] = json!("unreviewed_summary");
                let invalid: Vec<Value> = array(&item, "source_refs")
                    .iter()
                    .filter_map(|reference| {
                        let key = reference.as_str().or_else(|| reference["id"].as_str())?;
                        let source = statement_for(state, key)?;
                        (source["kind"] == "fact" && source["validity"] != "current")
                            .then(|| json!(key))
                    })
                    .collect();
                item["invalid_source_ids"] = json!(invalid);
            }
            item
        })
        .collect();
    context["experience_records"] = json!(experiences);
    context["records"] = context["experience_records"].clone();
}

/// Return ready=false (not Err) when reads are missing so the transaction can
/// persist the next-input request alongside a recoverable action receipt.
pub(super) fn submission_premises(
    state: &mut Value,
    action: &Value,
    session_id: &str,
) -> V2Result<Value> {
    let context_epoch = entity(state, "sessions", session_id)
        .and_then(|session| session["evidence_context_epoch"].as_u64())
        .unwrap_or(1);
    let declared = action["declared_premises"].as_array().ok_or_else(|| {
        err(
            "PREMISE_DECLARATION_REQUIRED",
            "正式提交必须提供 declared_premises 数组；无非平凡前提时明确 []",
        )
    })?;
    if declared.len() > 128 {
        return Err(err(
            "PREMISE_DECLARATION_INVALID",
            "前提过多，请拆分为独立可审查成果",
        ));
    }
    let mut checked = Vec::new();
    let mut missing = Vec::new();
    let mut seen = HashSet::new();
    for (index, premise) in declared.iter().enumerate() {
        let location = premise["usage_location"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                err(
                    "PREMISE_DECLARATION_INVALID",
                    "每项非平凡前提必须标注 usage_location",
                )
            })?;
        let kind = premise["kind"].as_str().unwrap_or("fact");
        if matches!(kind, "assumption" | "background") {
            let statement = premise["statement"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| {
                    err(
                        "PREMISE_DECLARATION_INVALID",
                        "假设/背景约定必须有精确 statement",
                    )
                })?;
            checked.push(json!({"id":format!("local:{index}"),"kind":kind,"statement":statement,"usage_location":location,"applicability":premise["applicability"]}));
            continue;
        }
        if !matches!(kind, "fact" | "source" | "conditional") {
            return Err(err("PREMISE_DECLARATION_INVALID", "未知前提类型"));
        }
        let key = premise["ref_id"]
            .as_str()
            .ok_or_else(|| err("PREMISE_DECLARATION_INVALID", "引用前提需要 ref_id"))?;
        if !seen.insert(key.to_owned()) {
            return Err(err("PREMISE_DECLARATION_INVALID", "重复前提请合并使用位置"));
        }
        let statement = statement_for(state, key)
            .ok_or_else(|| err("MATERIAL_MISSING", format!("前提 {key} 不存在")))?;
        for field in ["revision", "sha256", "proof_artifact_id", "snapshot_hash"] {
            if !premise[field].is_null() && premise[field] != statement[field] {
                return Err(err(
                    "PREMISE_VERSION_CONFLICT",
                    format!("前提 {key} 的 {field} 已变化，需重新读取精确版本"),
                ));
            }
        }
        if (kind == "fact" && (statement["kind"] != "fact" || statement["validity"] != "current"))
            || (kind == "source" && statement["kind"] != "source")
            || (kind == "conditional" && statement["kind"] != "candidate")
        {
            return Err(err(
                "PREMISE_DECLARATION_INVALID",
                format!("前提 {key} 类型或有效性不符"),
            ));
        }
        let supplied = array(state, "evidence_reads").iter().any(|read| {
            read["session_id"] == session_id
                && read["statement_id"] == key
                && read["sha256"] == statement["sha256"]
                && read["revision"] == statement["revision"]
                && read["origin"] == "host_prompt"
                && read["context_epoch"].as_u64().unwrap_or(1) == context_epoch
                && read["supplied_range"]["complete"] == true
        });
        if !supplied {
            missing.push(json!(key));
        }
        checked.push(json!({"id":key,"ref_id":key,"kind":kind,"revision":statement["revision"],"sha256":statement["sha256"],"statement":statement["text"],"proof_artifact_id":statement["proof_artifact_id"],"proof_sha256":statement["proof_sha256"],"snapshot_hash":statement["snapshot_hash"],"usage_location":location,"applicability":premise["applicability"]}));
    }
    for (field, kind) in [
        ("dependency_ids", "fact"),
        ("dependencies", "fact"),
        ("source_artifact_ids", "source"),
    ] {
        for reference in array(action, field) {
            if !checked
                .iter()
                .any(|p| p["ref_id"] == *reference && p["kind"] == kind)
            {
                return Err(err(
                    "PREMISE_DECLARATION_INVALID",
                    format!("{field} 中的 {reference} 未在非平凡前提清单核对"),
                ));
            }
        }
    }
    for premise in &checked {
        if premise["kind"] == "fact"
            && !array(action, "dependency_ids")
                .iter()
                .chain(array(action, "dependencies"))
                .any(|key| *key == premise["ref_id"])
        {
            return Err(err(
                "PREMISE_DECLARATION_INVALID",
                "事实前提同时必须出现在 dependency_ids 中，避免漏报依赖",
            ));
        }
        if premise["kind"] == "source"
            && !array(action, "source_artifact_ids").contains(&premise["ref_id"])
        {
            return Err(err(
                "PREMISE_DECLARATION_INVALID",
                "来源前提同时必须出现在 source_artifact_ids 中",
            ));
        }
    }
    let session = entity_mut(state, "sessions", session_id)?;
    let abandoned: HashSet<String> = array(session, "statement_blockers")
        .iter()
        .filter_map(|blocker| blocker["id"].as_str())
        .filter(|key| !seen.contains(*key))
        .map(str::to_owned)
        .collect();
    let blockers: Vec<Value> = array(session, "statement_blockers")
        .iter()
        .filter(|blocker| {
            missing.contains(&blocker["id"])
                && checked.iter().any(|premise| {
                    premise["id"] == blocker["id"]
                        && premise["revision"] == blocker["revision"]
                        && premise["sha256"] == blocker["sha256"]
                })
        })
        .cloned()
        .collect();
    session["statement_blockers"] = json!(blockers);
    let mut requested: Vec<Value> = array(session, "pending_statement_ids")
        .iter()
        .filter(|key| !abandoned.contains(key.as_str().unwrap_or_default()))
        .cloned()
        .collect();
    if !missing.is_empty() {
        for key in &missing {
            if !requested.contains(key) {
                requested.push(key.clone());
            }
        }
    }
    session["pending_statement_ids"] = json!(requested);
    Ok(
        json!({"ready":missing.is_empty(),"blocked":!blockers.is_empty(),"statement_blockers":blockers,"declared_premises":checked,"missing_statement_ids":missing,"message":if missing.is_empty(){"前提引用已核对；数学适用性仍须独立审核"}else if !blockers.is_empty(){"部分原文无法由普通重试提供；请修复材料或修改前提引用，候选尚未提交"}else{"宿主将在下一轮提供精确原文；阅读后检查适用条件再提交，当前尚未提交候选"}}),
    )
}

pub(super) fn augment_packet(state: &Value, session: &Value, packet: &mut Value) {
    let candidate = &session["candidate_snapshot"];
    super::proof_tree23::augment_verification_packet(state, candidate, packet);
    packet["declared_premises"] = candidate
        .get("declared_premises")
        .cloned()
        .unwrap_or_else(|| json!([]));
    packet["math_node_ref"] = candidate["math_node_ref"].clone();
    packet["goal_refs"] = candidate["goal_refs"].clone();
    packet["premise_audit_required"] = json!(true);
    packet["premise_audit_instruction"] = json!(
        "Check every declared nontrivial premise and actively identify undeclared premises, quantifier/scope/parameter mismatches and circular reasoning. Read receipts do not establish applicability. Missing or unresolved premise audits cannot pass."
    );
    let mut missing = array(packet, "unresolved_materials").to_vec();
    for premise in array(packet, "declared_premises") {
        if premise["kind"] == "conditional" {
            let key = premise["ref_id"].as_str().unwrap_or_default();
            missing.push(json!(format!("Conditional premise {key}: resubmit with the corresponding current fact ID as an explicit dependency, or supply an independently checked proof without this premise before admission")));
        }
    }
    packet["unresolved_materials"] = json!(missing);
}

fn pending_family(state: &Value, reference: &str) -> V2Result<Option<String>> {
    if let Some(fact) = entity(state, "facts", reference) {
        if current_fact(state, fact, &mut HashSet::new()) {
            return Ok(None);
        }
        return Err(err("PREMISE_INVALID", "已失效事实不能继续作为工作前提"));
    }
    let candidate = entity(state, "candidates", reference).ok_or_else(|| {
        err(
            "PREMISE_INVALID",
            "条件性前提必须引用已有候选，先提交精确陈述",
        )
    })?;
    if candidate["problem_version"] != state["problem_version"] {
        return Err(err("PREMISE_INVALID", "条件性前提属于旧题目版本"));
    }
    if array(state, "facts").iter().any(|fact| {
        fact["candidate_id"] == reference && current_fact(state, fact, &mut HashSet::new())
    }) {
        return Ok(None);
    }
    if matches!(
        candidate["status"].as_str(),
        Some("rejected" | "withdrawn" | "challenged" | "stale")
    ) {
        return Err(err(
            "PREMISE_INVALID",
            "已否定或失效前提应修补或寻找反例，不能继续条件性推进",
        ));
    }
    Ok(Some(
        candidate["lineage_id"].as_str().unwrap_or(reference).into(),
    ))
}

pub(super) fn apply_action(
    state: &mut Value,
    run_id: &str,
    session_id: &str,
    action: &Value,
) -> V2Result<Option<Value>> {
    if action["type"] != "set_conditional_premises" {
        return Ok(None);
    }
    let session =
        entity(state, "sessions", session_id).ok_or_else(|| err("NOT_FOUND", "会话不存在"))?;
    if session["run_id"] != run_id || !matches!(session["role"].as_str(), Some("main" | "partner"))
    {
        return Err(err(
            "INVALID_ROLE",
            "仅当前运行中的研究者可设置自身工作前提",
        ));
    }
    let refs = action["premise_ids"]
        .as_array()
        .ok_or_else(|| err("PREMISE_INVALID", "premise_ids 必须是数组"))?;
    if refs.len() > 64 {
        return Err(err(
            "PREMISE_INVALID",
            "一次研究不能隐式展开超过64项待审前提",
        ));
    }
    let mode = action["work_mode"].as_str().unwrap_or("conditional");
    if !matches!(mode, "conditional" | "premise_repair" | "counterexample") {
        return Err(err("PREMISE_INVALID", "work_mode 不支持"));
    }
    for reference in refs {
        let key = reference
            .as_str()
            .ok_or_else(|| err("PREMISE_INVALID", "前提必须是ID"))?;
        if mode == "conditional" {
            pending_family(state, key)?;
        } else if statement_for(state, key).is_none() {
            return Err(err("PREMISE_INVALID", "修补或反例目标不存在"));
        }
    }
    let reason = action["reason"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| err("PREMISE_INVALID", "工作前提改变须给出 reason"))?;
    let session = entity_mut(state, "sessions", session_id)?;
    session["pending_premise_ids"] = json!(refs);
    session["premise_work_mode"] = json!(mode);
    session["premise_change_reason"] = json!(reason);
    Ok(Some(
        json!({"accepted":true,"premise_ids":refs,"work_mode":mode,"notice":"此登记不认证前提；下一次派发受共享累计限额约束"}),
    ))
}

/// Must be called inside the same state transaction as Run invocation reservation.
pub(super) fn reserve_conditional(
    state: &mut Value,
    run_id: &str,
    session_id: &str,
    invocation_id: &str,
) -> V2Result<()> {
    let session =
        entity(state, "sessions", session_id).ok_or_else(|| err("NOT_FOUND", "会话不存在"))?;
    if !matches!(session["role"].as_str(), Some("main" | "partner"))
        || matches!(
            session["phase"].as_str(),
            Some("coordination" | "urgent_attention" | "recovery")
        )
        || matches!(
            session["premise_work_mode"].as_str(),
            Some("premise_repair" | "counterexample")
        )
    {
        return Ok(());
    }
    let mut families = HashSet::new();
    for reference in array(session, "pending_premise_ids") {
        if let Some(family) = pending_family(state, reference.as_str().unwrap_or_default())? {
            families.insert(family);
        }
    }
    if families.is_empty() {
        return Ok(());
    }
    let run = entity(state, "runs", run_id).ok_or_else(|| err("NOT_FOUND", "运行不存在"))?;
    let limit = run["limits"]["conditional_max_invocations"]
        .as_u64()
        .filter(|v| *v > 0)
        .ok_or_else(|| {
            err(
                "CONDITIONAL_LIMIT_REQUIRED",
                "请配置正整数 conditional_max_invocations，未配置时不启动条件性研究",
            )
        })?;
    for family in &families {
        if array(state, "conditional_reservations")
            .iter()
            .any(|r| r["invocation_id"] == invocation_id && r["premise_family_id"] == *family)
        {
            continue;
        }
        let used = array(state, "conditional_reservations")
            .iter()
            .filter(|r| {
                r["run_id"] == run_id
                    && r["premise_family_id"] == *family
                    && r["state"] != "not_dispatched"
            })
            .count();
        if u64::try_from(used).unwrap_or(u64::MAX) >= limit {
            return Err(err(
                "CONDITIONAL_LIMIT",
                "共同未审查前提的累计研究额度已满；等待审查，或明确改为修补/反例/独立研究",
            ));
        }
    }
    for family in families {
        if array(state, "conditional_reservations")
            .iter()
            .any(|r| r["invocation_id"] == invocation_id && r["premise_family_id"] == family)
        {
            continue;
        }
        push(
            state,
            "conditional_reservations",
            json!({"id":id(),"run_id":run_id,"session_id":session_id,"invocation_id":invocation_id,"premise_family_id":family,"state":"reserved","created_at":now()}),
        );
        let candidate_ids: HashSet<String> = array(state, "candidates")
            .iter()
            .filter(|candidate| candidate["lineage_id"] == family || candidate["id"] == family)
            .filter_map(|c| c["id"].as_str())
            .map(str::to_owned)
            .collect();
        if let Some(reviews) = state["reviews"].as_array_mut() {
            for review in reviews {
                if candidate_ids.contains(review["candidate_id"].as_str().unwrap_or_default())
                    && matches!(review["state"].as_str(), Some("queued" | "running"))
                {
                    review["priority"] = json!("critical_premise");
                    review["priority_reason"] =
                        json!("Current research explicitly depends on this unreviewed premise");
                }
            }
        }
    }
    Ok(())
}

pub(super) fn notify_withdrawals(state: &mut Value) {
    let reads = array(state, "evidence_reads").to_vec();
    for read in reads {
        let key = read["statement_id"].as_str().unwrap_or_default();
        let Some(statement) = statement_for(state, key) else {
            continue;
        };
        let invalid = (statement["kind"] == "fact" && statement["validity"] != "current")
            || (statement["kind"] == "candidate"
                && matches!(
                    statement["validity"].as_str(),
                    Some("rejected" | "withdrawn" | "challenged" | "stale")
                ));
        if !invalid {
            continue;
        }
        let notification_key = format!(
            "withdrawal:{}:{}:{}",
            read["session_id"].as_str().unwrap_or_default(),
            key,
            statement["revision"]
        );
        if array(state, "memories")
            .iter()
            .any(|m| m["dedupe_key"] == notification_key)
        {
            continue;
        }
        push(
            state,
            "memories",
            json!({"id":id(),"kind":"evidence_self_check","dedupe_key":notification_key,"recipient_session_id":read["session_id"],"problem_version":state["problem_version"],"text":format!("来源 {key} 当前已失效。此会话曾收到其原文，请检查是否实际采用；只读过不等于依赖，不自动撤销无关成果。"),"source_refs":[key],"created_at":now()}),
        );
    }
}

impl super::V2Service {
    /// Read exact authoritative source content for the platform. HTTP reads do
    /// not establish delivery to any research model or create `EvidenceRead` rows.
    pub async fn read_statement22(&self, project: &str, reference: &str) -> V2Result<Value> {
        let state = self.store.read(project).await?;
        let mut statement =
            statement_for(&state, reference).ok_or_else(|| err("NOT_FOUND", "精确陈述不存在"))?;
        let source = if statement["kind"] == "source" {
            Some(reference.to_owned())
        } else {
            statement["source_artifact_id"].as_str().map(str::to_owned)
        };
        if let Some(source) = source {
            let (artifact, bytes) = self.store.read_artifact(project, &source).await?;
            let text = String::from_utf8(bytes)
                .map_err(|_| err("MATERIAL_MISSING", "精确陈述需要UTF-8文本提取"))?;
            if statement["kind"] != "source"
                && (statement["sha256"] != digest(&text) || statement["text"] != text)
            {
                return Err(err("EVIDENCE_MISMATCH", "原始陈述artifact与冻结陈述不一致"));
            }
            statement["text"] = json!(text);
            statement["sha256"] = artifact["sha256"].clone();
        }
        statement["delivery_scope"] = json!("platform_read_not_model_delivery");
        Ok(statement)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declared_premise_content_pins_reject_changed_versions_before_read_retry() {
        let mut state = state();
        let action = json!({"dependency_ids":["f"],"declared_premises":[{"kind":"fact","ref_id":"f","revision":2,"usage_location":"step 1"}]});
        assert!(submission_premises(&mut state, &action, "s").is_err());
        let mut action = action;
        action["declared_premises"][0]["revision"] = json!(1);
        action["declared_premises"][0]["sha256"] = json!("stale statement hash");
        assert!(submission_premises(&mut state, &action, "s").is_err());
        action["declared_premises"][0]["sha256"] = json!(digest("For each a there exists C(a)"));
        assert_eq!(
            submission_premises(&mut state, &action, "s").unwrap()["ready"],
            false
        );
    }
    fn state() -> Value {
        json!({"problem_version":1,"sessions":[{"id":"s","role":"main","run_id":"r"},{"id":"p","role":"partner","run_id":"r"}],"runs":[{"id":"r","limits":{"conditional_max_invocations":1}}],"facts":[{"id":"f","claim":"For each a there exists C(a)","problem_version":1,"revision":1,"validity":"current","assurance":"model_reviewed","dependency_ids":[]}],"candidates":[{"id":"c","lineage_id":"family","claim":"Pending lemma","problem_version":1,"revision":1,"status":"submitted"},{"id":"c2","lineage_id":"family","claim":"Repaired lemma","problem_version":1,"revision":1,"status":"submitted"}],"reviews":[],"memories":[],"artifacts":[]})
    }
    #[test]
    fn submission_requires_real_host_supply_and_keeps_request() {
        let mut s = state();
        let action = json!({"dependency_ids":["f"],"declared_premises":[{"kind":"fact","ref_id":"f","usage_location":"step 2"}]});
        let rejected = submission_premises(&mut s, &action, "s").unwrap();
        assert_eq!(rejected["ready"], false);
        assert_eq!(s["sessions"][0]["pending_statement_ids"], json!(["f"]));
        let mut statement = statement_for(&s, "f").unwrap();
        statement["supplied_range"] = json!({"complete":true});
        record_supplied(
            &mut s,
            &json!({"session":{"id":"p"},"statement_supply":[statement.clone()]}),
            "wrong-session",
        );
        assert_eq!(
            submission_premises(&mut s, &action, "s").unwrap()["ready"],
            false
        );
        record_supplied(
            &mut s,
            &json!({"session":{"id":"s"},"statement_supply":[statement]}),
            "i",
        );
        assert_eq!(
            submission_premises(&mut s, &action, "s").unwrap()["ready"],
            true
        );
        s["sessions"][0]["evidence_context_epoch"] = json!(2);
        assert_eq!(
            submission_premises(&mut s, &action, "s").unwrap()["ready"],
            false
        );
        s["sessions"][0]["evidence_context_epoch"] = json!(1);
        s["facts"][0]["claim"] = json!("Changed exact statement");
        assert_eq!(
            submission_premises(&mut s, &action, "s").unwrap()["ready"],
            false
        );
    }
    #[test]
    fn omitted_dependency_declarations_do_not_pass() {
        let mut s = state();
        assert!(
            submission_premises(
                &mut s,
                &json!({"dependency_ids":["f"],"declared_premises":[]}),
                "s"
            )
            .is_err()
        );
        assert!(submission_premises(&mut s, &json!({}), "s").is_err());
        assert_eq!(
            submission_premises(&mut s, &json!({"declared_premises":[]}), "s").unwrap()["ready"],
            true
        );
    }
    #[tokio::test]
    async fn preparing_original_text_does_not_claim_delivery_and_detects_statement_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let store = V2Store::connect(&tmp.path().join("db.sqlite"), &tmp.path().join("data"))
            .await
            .unwrap();
        let project = store
            .create_project(
                json!({"title":"Evidence test","problem":"Check a parameter-dependent bound"}),
                "p1",
            )
            .await
            .unwrap();
        let project_id = project["id"].as_str().unwrap();
        let exact = "For every fixed a, there exists C(a); no uniform constant is asserted.";
        let artifact = store
            .put_artifact(project_id, "exact.md", exact.as_bytes(), "text/markdown")
            .await
            .unwrap();
        store.mutate(project_id,"test.evidence",None,|state|{
            push(state,"sessions",json!({"id":"s","role":"main","pending_statement_ids":["c"]}));
            push(state,"candidates",json!({"id":"c","claim":exact,"statement_artifact_id":artifact["id"],"problem_version":1,"revision":1}));
            Ok(Value::Null)
        }).await.unwrap();
        let mut prepared = json!({"session":{"id":"s","role":"main"}});
        prepare_turn(&store, project_id, &mut prepared, "i")
            .await
            .unwrap();
        assert_eq!(prepared["exact_statements"]["statements"][0]["text"], exact);
        assert_eq!(prepared["statement_supply"][0]["sha256"], digest(exact));
        assert!(array(&store.read(project_id).await.unwrap(), "evidence_reads").is_empty());
        store
            .mutate(project_id, "test.bad_statement", None, |state| {
                state["candidates"][0]["claim"] = json!("There exists a uniform constant C");
                Ok(Value::Null)
            })
            .await
            .unwrap();
        prepare_turn(&store, project_id, &mut prepared, "i2")
            .await
            .unwrap();
        assert!(array(&prepared, "statement_supply").is_empty());
        assert_eq!(array(&prepared["exact_statements"], "unavailable").len(), 1);
        assert_eq!(
            prepared["statement_unavailable"][0]["code"],
            "STATEMENT_MISMATCH"
        );
        assert!(
            array(
                &store.read(project_id).await.unwrap()["sessions"][0],
                "statement_blockers"
            )
            .is_empty()
        );
        store
            .mutate(project_id, "test.delivered_mismatch", None, |state| {
                record_supplied(state, &prepared, "i2");
                Ok(Value::Null)
            })
            .await
            .unwrap();
        assert_eq!(
            store.read(project_id).await.unwrap()["sessions"][0]["statement_blockers"][0]["blocked"],
            true
        );
        store
            .mutate(project_id, "test.source_repaired", None, |state| {
                state["candidates"][0]["claim"] = json!(exact);
                Ok(Value::Null)
            })
            .await
            .unwrap();
        prepare_turn(&store, project_id, &mut prepared, "i3")
            .await
            .unwrap();
        store.mutate(project_id, "test.delivered_repaired", None, |state| {
            record_supplied(state, &prepared, "i3");
            assert!(array(&state["sessions"][0], "statement_blockers").is_empty());
            let result = submission_premises(state, &json!({"declared_premises":[{"kind":"conditional","ref_id":"c","usage_location":"step 1"}]}), "s")?;
            assert_eq!(result["ready"], true);
            Ok(Value::Null)
        }).await.unwrap();
    }
    #[tokio::test]
    async fn permanent_source_blockers_do_not_confuse_full_batches_and_can_be_abandoned() {
        let tmp = tempfile::tempdir().unwrap();
        let store = V2Store::connect(&tmp.path().join("db.sqlite"), &tmp.path().join("data"))
            .await
            .unwrap();
        let project = store
            .create_project(
                json!({"title":"Source blockers", "problem":"Read exact premises"}),
                "p",
            )
            .await
            .unwrap();
        let project_id = project["id"].as_str().unwrap();
        let oversized = store
            .put_artifact(
                project_id,
                "large.md",
                "x".repeat(MAX_SUPPLY_CHARACTERS + 1).as_bytes(),
                "text/markdown",
            )
            .await
            .unwrap();
        let binary = store
            .put_artifact(
                project_id,
                "binary.bin",
                &[0xff],
                "application/octet-stream",
            )
            .await
            .unwrap();
        let first = store
            .put_artifact(
                project_id,
                "first.md",
                "a".repeat(100_000).as_bytes(),
                "text/markdown",
            )
            .await
            .unwrap();
        let second = store
            .put_artifact(
                project_id,
                "second.md",
                "b".repeat(100_000).as_bytes(),
                "text/markdown",
            )
            .await
            .unwrap();
        let ids = vec![
            oversized["id"].clone(),
            binary["id"].clone(),
            first["id"].clone(),
            second["id"].clone(),
        ];
        store.mutate(project_id, "test.session", None, |state| {
            push(state, "sessions", json!({"id":"s","role":"main","native_session_id":"native","pending_statement_ids":ids}));
            Ok(Value::Null)
        }).await.unwrap();
        let mut prepared = json!({"session":{"id":"s","role":"main"}});
        prepare_turn(&store, project_id, &mut prepared, "i")
            .await
            .unwrap();
        assert_eq!(array(&prepared, "statement_supply").len(), 1);
        let unavailable = array(&prepared, "statement_unavailable");
        assert_eq!(
            unavailable
                .iter()
                .filter(|item| item["blocked"] == true)
                .count(),
            2
        );
        assert_eq!(
            unavailable
                .iter()
                .find(|item| item["id"] == second["id"])
                .unwrap()["blocked"],
            false
        );
        store.mutate(project_id, "test.supplied", None, |state| {
            record_supplied(state, &prepared, "i");
            let declared: Vec<Value> = ids.iter().map(|key| json!({"kind":"source","ref_id":key,"usage_location":"proof"})).collect();
            let blocked = submission_premises(state, &json!({"source_artifact_ids":ids,"declared_premises":declared}), "s")?;
            assert_eq!(blocked["blocked"], true);
            assert_eq!(array(&blocked, "statement_blockers").len(), 2);
            let changed = submission_premises(state, &json!({"source_artifact_ids":[second["id"]],"declared_premises":[{"kind":"source","ref_id":second["id"],"usage_location":"replacement proof"}]}), "s")?;
            assert_eq!(changed["blocked"], false);
            assert_eq!(state["sessions"][0]["pending_statement_ids"], json!([second["id"]]));
            Ok(Value::Null)
        }).await.unwrap();
        prepare_turn(&store, project_id, &mut prepared, "i2")
            .await
            .unwrap();
        assert_eq!(prepared["statement_supply"][0]["id"], second["id"]);
        assert!(array(&prepared, "statement_unavailable").is_empty());
    }
    #[test]
    fn revisions_and_partners_share_conditional_allowance() {
        let mut s = state();
        for (session, premise) in [("s", "c"), ("p", "c2")] {
            apply_action(&mut s,"r",session,&json!({"type":"set_conditional_premises","premise_ids":[premise],"reason":"Explore consequence"})).unwrap();
        }
        reserve_conditional(&mut s, "r", "s", "i1").unwrap();
        reserve_conditional(&mut s, "r", "s", "i1").unwrap();
        assert_eq!(array(&s, "conditional_reservations").len(), 1);
        assert_eq!(
            reserve_conditional(&mut s, "r", "p", "i2")
                .unwrap_err()
                .code,
            "CONDITIONAL_LIMIT"
        );
        apply_action(&mut s,"r","p",&json!({"type":"set_conditional_premises","premise_ids":[],"reason":"Independent work"})).unwrap();
        reserve_conditional(&mut s, "r", "p", "i3").unwrap();
    }
    #[test]
    fn uncertain_dispatch_keeps_allowance_but_confirmed_unstarted_can_release_it() {
        let mut s = state();
        s["sessions"][0]["pending_premise_ids"] = json!(["c"]);
        reserve_conditional(&mut s, "r", "s", "i").unwrap();
        settle_conditional(&mut s, "i", None);
        assert!(reserve_conditional(&mut s, "r", "s", "i2").is_err());
        settle_conditional(&mut s, "i", Some(false));
        reserve_conditional(&mut s, "r", "s", "i2").unwrap();
        settle_conditional(&mut s, "i2", Some(true));
        settle_conditional(&mut s, "i2", Some(false));
        assert_eq!(s["conditional_reservations"][1]["state"], "used");
        assert!(reserve_conditional(&mut s, "r", "s", "i3").is_err());
        s["sessions"][0]["phase"] = json!("coordination");
        reserve_conditional(&mut s, "r", "s", "arrange").unwrap();
        assert_eq!(array(&s, "conditional_reservations").len(), 2);
    }
    #[test]
    fn withdrawn_facts_remain_experience_and_readers_get_only_self_check() {
        let mut s = state();
        let mut statement = statement_for(&s, "f").unwrap();
        statement["supplied_range"] = json!({"complete":true});
        record_supplied(
            &mut s,
            &json!({"session":{"id":"s"},"statement_supply":[statement]}),
            "i",
        );
        s["facts"][0]["validity"] = json!("challenged");
        notify_withdrawals(&mut s);
        notify_withdrawals(&mut s);
        assert_eq!(array(&s, "memories").len(), 1);
        assert_eq!(s["candidates"][0]["status"], "submitted");
        let mut ctx = json!({});
        augment_context(&s, &json!({"session":{"id":"s","role":"main"}}), &mut ctx);
        assert!(array(&ctx, "fact_records").is_empty());
        assert!(
            array(&ctx, "experience_records")
                .iter()
                .any(|r| r["id"] == "f")
        );
    }
}
