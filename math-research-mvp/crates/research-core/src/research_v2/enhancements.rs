//! Research capability deployment, frozen verification packets, and board checkpoints.
use super::{array, entity, entity_mut, err, id, now, push, revision};
use research_storage::research_v2::{V2Result, V2Store};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

macro_rules! asset {
    ($path:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../capabilities/",
            $path
        ))
    };
}

pub(super) async fn deploy(workspace: &Path, prepared: &Value, context: &Value) -> V2Result<()> {
    let files = [
        (
            ".agents/skills/mathcat-research/SKILL.md",
            asset!("mathcat-research/SKILL.md"),
        ),
        (
            ".agents/skills/mathcat-research/references/direct-proof.md",
            asset!("mathcat-research/references/direct-proof.md"),
        ),
        (
            ".agents/skills/mathcat-research/references/counterexamples.md",
            asset!("mathcat-research/references/counterexamples.md"),
        ),
        (
            ".agents/skills/mathcat-research/references/sources.md",
            asset!("mathcat-research/references/sources.md"),
        ),
        (
            ".agents/skills/mathcat-research/references/repair.md",
            asset!("mathcat-research/references/repair.md"),
        ),
        (
            ".agents/skills/mathcat-proof-verification/SKILL.md",
            asset!("mathcat-proof-verification/SKILL.md"),
        ),
        (
            ".agents/skills/mathcat-proof-verification/references/sequential.md",
            asset!("mathcat-proof-verification/references/sequential.md"),
        ),
        (
            ".agents/skills/mathcat-proof-verification/references/references.md",
            asset!("mathcat-proof-verification/references/references.md"),
        ),
        (
            ".agents/skills/mathcat-proof-verification/references/synthesis.md",
            asset!("mathcat-proof-verification/references/synthesis.md"),
        ),
        (
            ".mathcat-tools.mjs",
            asset!("mathcat-research/scripts/research-tools.mjs"),
        ),
    ];
    for (name, body) in files {
        write_session_file(workspace, &workspace.join(name), body).await?;
    }
    write_session_file(
        workspace,
        &workspace.join(".mathcat-context.json"),
        &context.to_string(),
    )
    .await?;
    if !prepared["verification_packet"].is_null() {
        write_session_file(
            workspace,
            &workspace.join("verification-packet.json"),
            &prepared["verification_packet"].to_string(),
        )
        .await?;
    }
    Ok(())
}

async fn write_session_file(root: &Path, file: &Path, text: &str) -> V2Result<()> {
    let root = tokio::fs::canonicalize(root).await?;
    let relative = file
        .strip_prefix(&root)
        .map_err(|_| err("WORKSPACE_DENIED", "Session file escaped root"))?;
    let mut parent = root.clone();
    for component in relative
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .components()
    {
        parent.push(component);
        if !tokio::fs::try_exists(&parent).await? {
            tokio::fs::create_dir(&parent).await?;
        }
        if !tokio::fs::canonicalize(&parent).await?.starts_with(&root) {
            return Err(err(
                "WORKSPACE_DENIED",
                "Linked session directory escaped root",
            ));
        }
    }
    if let Ok(meta) = tokio::fs::symlink_metadata(file).await {
        if meta.file_type().is_symlink() || !tokio::fs::canonicalize(file).await?.starts_with(&root)
        {
            return Err(err("WORKSPACE_DENIED", "Refusing linked session output"));
        }
    }
    tokio::fs::write(file, text).await?;
    Ok(())
}

#[allow(clippy::items_after_statements)]
pub(super) async fn proof_packet(
    store: &V2Store,
    project: &str,
    session: &Value,
) -> V2Result<Value> {
    let state = store.read(project).await?;
    let candidate = &session["candidate_snapshot"];
    let mut missing = Vec::new();
    let mut dependencies = Vec::new();
    let mut visited = HashSet::new();
    let mut pending: Vec<Value> = array(candidate, "dependency_ids").to_vec();
    let mut graph = HashMap::<String, Vec<String>>::new();
    while let Some(dep) = pending.pop() {
        let key = dep.as_str().unwrap_or_default().to_owned();
        if !visited.insert(key.clone()) {
            continue;
        }
        if visited.len() > 1000 {
            missing.push("Dependency closure exceeds 1000 entries; not silently truncated".into());
            break;
        }
        let Some(fact) = entity(&state, "facts", &key) else {
            missing.push(format!("Missing dependency {key}"));
            continue;
        };
        if fact["validity"] != "current" || fact["problem_version"] != candidate["problem_version"]
        {
            missing.push(format!("Stale or challenged dependency {key}"));
        }
        // Facts normally preserve their proof forever. If imported or repaired
        // data breaks that invariant, never substitute the latest proof silently.
        if let Some(pin) = array(candidate, "declared_premises")
            .iter()
            .find(|p| p["ref_id"] == key && p["kind"] == "fact")
        {
            for field in [
                "revision",
                "proof_artifact_id",
                "proof_sha256",
                "snapshot_hash",
            ] {
                if !pin[field].is_null() && pin[field] != fact[field] {
                    missing.push(format!(
                        "Frozen dependency {key}: {field} no longer matches the submitted version"
                    ));
                }
            }
            if let Some(statement) = pin["statement"].as_str() {
                if fact["claim"]
                    .as_str()
                    .is_some_and(|claim| claim != statement)
                {
                    missing.push(format!("Frozen dependency {key}: exact statement changed"));
                }
            }
        }
        let children = array(fact, "dependency_ids").to_vec();
        graph.insert(
            key.clone(),
            children
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
        );
        pending.extend(children);
        let proof_id = fact["proof_artifact_id"].as_str().unwrap_or_default();
        match store.read_artifact(project, proof_id).await {
            Ok((artifact, bytes)) => match String::from_utf8(bytes) {
                Ok(proof) => dependencies.push(json!({"id":key,"claim":fact["claim"],"proof":proof,"proof_artifact_id":proof_id,"sha256":artifact["sha256"],"dependency_ids":fact["dependency_ids"]})),
                Err(_) => missing.push(format!("Non-text dependency proof {key}")),
            },
            Err(_) => missing.push(format!("Missing/corrupt dependency proof {key}")),
        }
    }
    fn cyclic(
        key: &str,
        graph: &HashMap<String, Vec<String>>,
        path: &mut HashSet<String>,
        done: &mut HashSet<String>,
    ) -> bool {
        if done.contains(key) {
            return false;
        }
        if !path.insert(key.into()) {
            return true;
        }
        if graph
            .get(key)
            .is_some_and(|children| children.iter().any(|c| cyclic(c, graph, path, done)))
        {
            return true;
        }
        path.remove(key);
        done.insert(key.into());
        false
    }
    let mut done = HashSet::new();
    if graph
        .keys()
        .any(|k| cyclic(k, &graph, &mut HashSet::new(), &mut done))
    {
        missing.push("Circular dependency closure".into());
    }
    let mut sources = Vec::new();
    for source in array(candidate, "source_artifact_ids") {
        let key = source.as_str().unwrap_or_default();
        match store.read_artifact(project, key).await {
            Ok((a, bytes)) => match String::from_utf8(bytes) {
                Ok(text) => sources
                    .push(json!({"id":key,"name":a["name"],"sha256":a["sha256"],"text":text})),
                Err(_) => missing.push(format!(
                    "Source {key} needs text extraction; original retained"
                )),
            },
            Err(_) => missing.push(format!("Missing/corrupt source {key}")),
        }
    }
    let problem_context = super::problem_context::for_candidate(&state, candidate);
    let mut problem_sources = Vec::new();
    if problem_context["source_selection_pending"] == true {
        missing.push("Problem input sources were ambiguous when this candidate was frozen; explicit target-source selection is required.".into());
    }
    for source in array(&problem_context, "source_refs") {
        let key = source["artifact_id"].as_str().unwrap_or_default();
        match store.read_artifact(project, key).await {
            Ok((artifact, bytes)) if artifact["sha256"] == source["sha256"] => {
                match String::from_utf8(bytes) {
                    Ok(text) => {
                        let input = json!({"id":key,"name":source["name"],"sha256":source["sha256"],"text":text,
                            "problem_version":problem_context["problem_version"],"source_ref":source,
                            "source_role":"problem_input_context","assurance":"unreviewed_input",
                            "use":"target_identification_and_assumptions_only_not_a_verified_lemma"});
                        if !sources.iter().any(|item| item["id"] == key) {
                            sources.push(input.clone());
                        }
                        problem_sources.push(input);
                    }
                    Err(_) => missing.push(format!(
                        "Frozen problem source {key} needs UTF-8 extraction; original retained"
                    )),
                }
            }
            Ok(_) => missing.push(format!(
                "Frozen problem source {key} content identity changed"
            )),
            Err(_) => missing.push(format!("Missing/corrupt frozen problem source {key}")),
        }
    }
    let mut obligations = Vec::new();
    let mut previous = candidate["repair_of"].as_str();
    let mut ancestors = HashSet::new();
    while let Some(key) = previous {
        if !ancestors.insert(key) {
            missing.push("Circular repair lineage".into());
            break;
        }
        for review in array(&state, "reviews").iter().filter(|r| {
            r["candidate_id"] == key
                && r["state"] == "completed"
                && matches!(
                    r["verdict"].as_str(),
                    Some("rejected" | "changes_requested")
                )
        }) {
            for (i, issue) in array(review, "issues").iter().enumerate() {
                obligations.push(json!({"id":format!("{}:{i}",review["id"].as_str().unwrap_or_default()),"issue":issue}));
            }
        }
        previous = entity(&state, "candidates", key).and_then(|c| c["repair_of"].as_str());
    }
    let (_, bytes) = store
        .read_artifact(
            project,
            candidate["proof_artifact_id"].as_str().unwrap_or_default(),
        )
        .await?;
    let proof =
        String::from_utf8(bytes).map_err(|_| err("MATERIAL_MISSING", "证明不是UTF-8正文"))?;
    Ok(
        json!({"schema_version":1,"engine":"rethlas-adapted/2.1.0","candidate_id":candidate["id"],"snapshot_hash":candidate["snapshot_hash"],"problem_version":candidate["problem_version"],"problem":problem_context["original_problem"],"problem_context":problem_context,"problem_sources":problem_sources,"claim":candidate["claim"],"covers_goal":candidate["covers_goal"],"proof":proof,"dependencies":dependencies,"sources":sources,"repair_obligations":obligations,"unresolved_materials":missing}),
    )
}

pub(super) fn context(state: &Value, prepared: &Value) -> Value {
    let reviewer = prepared["session"]["role"] == "reviewer";
    let records = if reviewer {
        json!([])
    } else {
        json!(
            ["facts", "memories", "nodes", "candidates"]
                .iter()
                .flat_map(|key| array(state, key)
                    .iter()
                    .filter(|v| v["problem_version"].is_null()
                        || v["problem_version"] == state["problem_version"])
                    .map(move |v| {
                        let mut r = v.clone();
                        r["collection"] = json!(key);
                        r
                    }))
                .collect::<Vec<_>>()
        )
    };
    json!({"schema_version":1,"project_id":state["id"],"problem_version":state["problem_version"],"deadline_at":prepared["run"]["deadline_at"],"role":prepared["session"]["role"],"records":records,"artifacts":if reviewer{json!([])}else{state["artifacts"].clone()},"artifact_root":PathBuf::from(state["workspace_path"].as_str().unwrap_or_default()).join(".mathcat/artifacts"),"verification_packet":prepared["verification_packet"],"mathematical_record_protocol":super::proof_tree23::prompt(),"context_reading":"Start with node .mathcat-tools.mjs context-summary and targeted search. Do not dump the entire .mathcat-context.json; read exact statements and frozen evidence only for selected relevant objects."})
}

pub(super) fn compact_memory(memory: &Value) -> Value {
    let mut copy = memory.clone();
    if let Some(text) = memory["text"].as_str() {
        copy["text"] = json!(text.chars().take(1600).collect::<String>());
        copy["excerpt"] = json!(text.chars().count() > 1600);
    }
    copy
}

pub(super) fn record_finding(
    state: &mut Value,
    action: &Value,
    session: &str,
    artifact: &Value,
    run: &str,
) -> V2Result<Value> {
    let statement = action["statement"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 100_000)
        .ok_or_else(|| err("INVALID_FINDING", "需要非空数学陈述"))?;
    let kind = action["kind"].as_str().unwrap_or("finding");
    if ![
        "finding",
        "lemma",
        "theorem",
        "proposition",
        "conjecture",
        "obstruction",
        "source",
        "focus",
        "goal",
        "definition",
        "example",
        "counterexample",
    ]
    .contains(&kind)
    {
        return Err(err("INVALID_FINDING", "不支持的发现类型"));
    }
    let old = action["node_id"]
        .as_str()
        .and_then(|key| entity(state, "nodes", key))
        .cloned();
    if action["node_id"].is_string() && old.is_none() {
        return Err(err("NOT_FOUND", "发现节点不存在"));
    }
    if let Some(node) = &old {
        if node["author"] != session
            || node["problem_version"] != state["problem_version"]
            || node["assurance"] != "unreviewed"
            || node["validity"] != "current"
        {
            return Err(err(
                "INVALID_FINDING",
                "不能覆写其他作者、旧版本或可信/争议证据",
            ));
        }
        if node["revision"] != action["expected_revision"] {
            return Err(err("REVISION_CONFLICT", "数学对象版本已变化"));
        }
    }
    let node_id = old
        .as_ref()
        .and_then(|n| n["id"].as_str())
        .map_or_else(id, str::to_owned);
    let rev = old
        .as_ref()
        .and_then(|n| n["revision"].as_u64())
        .unwrap_or(0)
        + 1;
    let detail = json!({"statement":statement,"assumptions":action["assumptions"],"scope":action["scope"],"attempted_method":action["attempted_method"],"exact_obstruction":action["exact_obstruction"],"failure_kind":action["failure_kind"],"reopen_condition":action["reopen_condition"]});
    let body = format!(
        "{statement}\n\n假设：{}\n\n适用范围：{}\n\n{}\n\n证据：{}",
        action["assumptions"],
        action["scope"],
        action["detail"].as_str().unwrap_or_default(),
        artifact["id"]
    );
    let node = json!({"id":node_id,"node_type":if kind=="source"{"source"}else if kind=="goal"{"goal"}else{"claim"},"math_kind":kind,"title":action["title"].as_str().unwrap_or(kind),"body":body,"body_artifact_id":artifact["id"],"exact_statement":statement,"statement_sha256":super::digest(statement),"draft_artifact_refs":action.get("draft_artifact_refs").cloned().unwrap_or_else(||json!([])),"original_output_artifact_id":action["original_output_artifact_id"],"assumptions":action["assumptions"],"symbols":action["symbols"],"scope":action["scope"],"goal_kind":action["goal_kind"],"checkpoint":action["checkpoint"],"finding":detail,"problem_version":state["problem_version"],"run_id":run,"author":session,"revision":rev,"work_state":"open","assurance":"unreviewed","validity":"current","created_at":old.as_ref().map_or_else(||json!(now()),|n|n["created_at"].clone()),"updated_at":now()});
    let mut node = node;
    node["mathematical_statements"] = super::problem24::statements(action, &node)?;
    if let Some(previous) = old {
        let mut version = previous;
        version["node_id"] = json!(node_id);
        push(state, "node_versions", version);
        *entity_mut(state, "nodes", &node_id)? = node.clone();
    } else {
        push(state, "nodes", node.clone());
    }
    push(
        state,
        "memories",
        json!({"id":id(),"run_id":run,"session_id":session,"kind":kind,"text":body,"finding":detail,"node_id":node_id,"artifact_id":artifact["id"],"problem_version":state["problem_version"],"created_at":now()}),
    );
    Ok(node)
}

pub(super) fn message_partner(
    state: &mut Value,
    action: &Value,
    author: &str,
    run_id: &str,
) -> V2Result<Value> {
    let target = action["session_id"]
        .as_str()
        .ok_or_else(|| err("INVALID_TARGET", "需要伙伴猫会话ID"))?;
    let sender = entity(state, "sessions", author).ok_or_else(|| err("NOT_FOUND", "作者不存在"))?;
    let receiver =
        entity(state, "sessions", target).ok_or_else(|| err("NOT_FOUND", "伙伴猫不存在"))?;
    if sender["role"] != "main"
        || receiver["role"] != "partner"
        || receiver["run_id"] != run_id
        || receiver["problem_version"] != state["problem_version"]
        || receiver["state"] == "closed"
    {
        return Err(err("INVALID_TARGET", "只能向本次运行当前版本的伙伴猫投递"));
    }
    if super::loop_control::current_task(state, receiver).is_some_and(|t| {
        matches!(
            t["state"].as_str(),
            Some("paused" | "cancelled" | "superseded")
        )
    }) {
        return Err(err("INVALID_STATE", "伙伴猫任务已暂停或取消"));
    }
    let text = action["text"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| err("INVALID_INPUT", "伙伴猫消息为空"))?;
    let message = json!({"id":id(),"run_id":run_id,"session_id":author,"recipient_session_id":target,"kind":"partner_message","text":text,"problem_version":state["problem_version"],"created_at":now()});
    push(state, "memories", message.clone());
    let receiver = entity_mut(state, "sessions", target)?;
    if receiver["state"] == "waiting" {
        receiver["state"] = json!("idle");
    }
    receiver["pending_followup"] = json!(true);
    revision(receiver);
    Ok(message)
}

/// Waiting for one's own complete solution review must not consume model calls.
pub(super) fn awaiting_goal_review(
    state: &Value,
    session: &Value,
    owned: &HashSet<String>,
) -> bool {
    if session["role"] != "main"
        || session["pending_urgent"] == true
        || array(state, "messages").iter().any(|m| {
            m["run_id"] == session["run_id"]
                && m["kind"] != "review_result"
                && (m["state"] == "queued" || m["status"] == "queued")
                && (m["target_role"] == "main"
                    || m["recipient_session_id"] == session["id"]
                    || m["target_session_id"] == session["id"])
        })
        || array(state, "commands").iter().any(|c| {
            c["run_id"] == session["run_id"]
                && matches!(c["status"].as_str(), Some("pending" | "queued"))
        })
    {
        return false;
    }
    array(state, "candidates").iter().any(|c| {
        c["run_id"] == session["run_id"]
            && c["author_session_id"] == session["id"]
            && c["problem_version"] == state["problem_version"]
            && c["covers_goal"] == true
            && c["continue_while_reviewing"] != true
            && array(state, "reviews").iter().any(|r| {
                r["candidate_id"] == c["id"]
                    && (matches!(r["state"].as_str(), Some("queued" | "running"))
                        || r["reviewer_session_id"]
                            .as_str()
                            .is_some_and(|s| owned.contains(s)))
            })
    })
}

/// Preserve historical evidence while retracting its authority transitively.
pub(super) fn invalidate_candidate_facts(state: &mut Value, candidate_id: &str) {
    let mut affected: HashSet<String> = array(state, "facts")
        .iter()
        .filter(|f| f["candidate_id"] == candidate_id)
        .filter_map(|f| f["id"].as_str().map(str::to_owned))
        .collect();
    loop {
        let additions: Vec<String> = array(state, "facts")
            .iter()
            .filter(|f| {
                array(f, "dependency_ids")
                    .iter()
                    .any(|d| d.as_str().is_some_and(|s| affected.contains(s)))
            })
            .filter_map(|f| {
                f["id"]
                    .as_str()
                    .filter(|s| !affected.contains(*s))
                    .map(str::to_owned)
            })
            .collect();
        if additions.is_empty() {
            break;
        }
        affected.extend(additions);
    }
    let runs: Vec<Value> = array(state, "facts")
        .iter()
        .filter(|f| f["id"].as_str().is_some_and(|s| affected.contains(s)))
        .map(|f| f["run_id"].clone())
        .filter(|r| !r.is_null())
        .collect();
    for collection in ["facts", "nodes"] {
        if let Some(items) = state[collection].as_array_mut() {
            for item in items {
                if ["id", "fact_id"]
                    .iter()
                    .any(|key| item[*key].as_str().is_some_and(|s| affected.contains(s)))
                    && item["validity"] == "current"
                {
                    item["validity"] = json!("challenged");
                    revision(item);
                }
            }
        }
    }
    if let Some(reports) = state["reports"].as_array_mut() {
        for report in reports {
            if runs.contains(&report["run_id"]) && report["validity"] != "challenged" {
                report["validity"] = json!("challenged");
                report["warning"] = json!(
                    "本报告是历史快照；其中使用的证据在后续复审中被质疑，请查看当前证据状态。"
                );
                revision(report);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn whole_goal_wait_is_event_driven_and_allows_human_or_explicit_continuation() {
        let session = json!({"id":"m","role":"main","run_id":"run"});
        let mut state = json!({"problem_version":1,"candidates":[{"id":"c","author_session_id":"m","run_id":"run","problem_version":1,"covers_goal":true}],"reviews":[{"candidate_id":"c","state":"running","reviewer_session_id":"v"}],"commands":[]});
        let empty = HashSet::new();
        assert!(awaiting_goal_review(&state, &session, &empty));
        state["reviews"][0]["state"] = json!("completed");
        assert!(!awaiting_goal_review(&state, &session, &empty));
        assert!(awaiting_goal_review(
            &state,
            &session,
            &HashSet::from(["v".into()])
        ));
        state["reviews"][0]["state"] = json!("running");
        state["candidates"][0]["continue_while_reviewing"] = json!(true);
        assert!(!awaiting_goal_review(&state, &session, &empty));
        state["candidates"][0]["continue_while_reviewing"] = json!(false);
        state["candidates"][0]["covers_goal"] = json!(false);
        assert!(!awaiting_goal_review(&state, &session, &empty));
        state["candidates"][0]["covers_goal"] = json!(true);
        state["commands"] = json!([{"run_id":"run","status":"queued"}]);
        assert!(!awaiting_goal_review(&state, &session, &empty));
    }
    #[test]
    fn negative_rereview_challenges_transitive_facts_and_reports_without_deleting_history() {
        let mut state = json!({"facts":[
            {"id":"a","candidate_id":"c","run_id":"r","validity":"current","revision":1},
            {"id":"b","candidate_id":"d","run_id":"s","validity":"current","dependency_ids":["a"],"revision":1},
            {"id":"other","validity":"current","revision":1}],
            "nodes":[{"id":"b","validity":"current","revision":1}],
            "reports":[{"run_id":"s","validity":"current","revision":1}]});
        invalidate_candidate_facts(&mut state, "c");
        assert_eq!(state["facts"][0]["validity"], "challenged");
        assert_eq!(state["facts"][1]["validity"], "challenged");
        assert_eq!(state["facts"][2]["validity"], "current");
        assert_eq!(state["nodes"][0]["validity"], "challenged");
        assert_eq!(state["reports"][0]["validity"], "challenged");
        invalidate_candidate_facts(&mut state, "c");
        assert_eq!(state["facts"][0]["revision"], 2);
        assert_eq!(array(&state, "facts").len(), 3);
    }
    #[test]
    fn checkpoints_are_versioned_untrusted_and_cannot_overwrite_other_authors() {
        let mut s = json!({"problem_version":1,"nodes":[],"memories":[]});
        let action = json!({"kind":"lemma","statement":"紧致性引理","assumptions":["紧致"],"scope":"当前空间"});
        let a = json!({"id":"evidence"});
        let first = record_finding(&mut s, &action, "main", &a, "run").unwrap();
        assert_eq!(first["assurance"], "unreviewed");
        let mut update = action;
        update["node_id"] = first["id"].clone();
        update["expected_revision"] = json!(1);
        update["statement"] = json!("修订引理");
        assert!(record_finding(&mut s, &update, "other", &a, "run").is_err());
        let second = record_finding(&mut s, &update, "main", &a, "run").unwrap();
        assert_eq!(second["revision"], 2);
        assert_eq!(array(&s, "node_versions").len(), 1);
        assert_eq!(array(&s, "nodes").len(), 1);
        assert!(record_finding(&mut s, &update, "main", &a, "run").is_err());
    }
    #[test]
    fn partner_messages_wake_existing_session_without_restarting_or_cancelled_task() {
        let mut s = json!({"problem_version":1,"sessions":[{"id":"m","role":"main"},{"id":"p","role":"partner","run_id":"r","problem_version":1,"state":"waiting","native_session_id":"native"}],"tasks":[{"owner_session_id":"p","state":"queued"}],"memories":[]});
        let action = json!({"session_id":"p","text":"继续处理同一个引理"});
        message_partner(&mut s, &action, "m", "r").unwrap();
        assert_eq!(s["sessions"][1]["state"], "idle");
        assert_eq!(s["sessions"][1]["native_session_id"], "native");
        assert_eq!(s["memories"][0]["recipient_session_id"], "p");
        s["tasks"][0]["state"] = json!("cancelled");
        assert!(message_partner(&mut s, &action, "m", "r").is_err());
    }
    #[tokio::test]
    async fn packet_contains_full_dependency_proofs_and_repair_obligations() {
        let tmp = tempfile::tempdir().unwrap();
        let store = V2Store::connect(&tmp.path().join("state.sqlite"), &tmp.path().join("data"))
            .await
            .unwrap();
        let p = store
            .create_project(json!({"problem":"A goal"}), "create")
            .await
            .unwrap();
        let project = p["id"].as_str().unwrap();
        let proof = store
            .put_artifact(project, "proof.md", b"FULL PROOF TEXT", "text/markdown")
            .await
            .unwrap();
        let aid = proof["id"].clone();
        store.mutate(project,"fact.admitted",None,|s|{push(s,"facts",json!({"id":"dep","claim":"needed lemma","proof_artifact_id":aid,"validity":"current","problem_version":1,"assurance":"model_reviewed","dependency_ids":[]}));push(s,"candidates",json!({"id":"old","repair_of":null}));push(s,"reviews",json!({"id":"rev","candidate_id":"old","state":"completed","verdict":"changes_requested","issues":[{"location":"L1","issue":"missing argument"}]}));Ok(Value::Null)}).await.unwrap();
        let session = json!({"candidate_snapshot":{"id":"c","claim":"goal","snapshot_hash":"h","proof_artifact_id":aid,"problem_version":1,"dependency_ids":["dep"],"repair_of":"old"}});
        let packet = proof_packet(&store, project, &session).await.unwrap();
        assert_eq!(packet["dependencies"][0]["proof"], "FULL PROOF TEXT");
        assert_eq!(packet["repair_obligations"][0]["id"], "rev:0");
        assert!(array(&packet, "unresolved_materials").is_empty());
        store
            .mutate(project, "dependency.challenged", None, |s| {
                s["facts"][0]["validity"] = json!("challenged");
                Ok(Value::Null)
            })
            .await
            .unwrap();
        assert!(
            !array(
                &proof_packet(&store, project, &session).await.unwrap(),
                "unresolved_materials"
            )
            .is_empty()
        );
    }
}
