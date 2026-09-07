//! A versioned mathematical view over the existing evidence ledger. This module
//! never admits a Fact: claims, review outcomes and current validity stay separate.
use super::{V2Service, array, digest, entity, err, id, now, push};
use research_storage::research_v2::V2Result;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

fn exact(value: &Value) -> &str {
    value["exact_statement"]
        .as_str()
        .or_else(|| value["finding"]["statement"].as_str())
        .or_else(|| value["claim"].as_str())
        .or_else(|| value["body"].as_str())
        .unwrap_or_default()
}

fn node_version<'a>(state: &'a Value, key: &str, revision: u64) -> Option<&'a Value> {
    entity(state, "nodes", key)
        .filter(|n| n["revision"] == revision)
        .or_else(|| {
            array(state, "node_versions")
                .iter()
                .find(|n| (n["id"] == key || n["node_id"] == key) && n["revision"] == revision)
        })
}

/// A changed problem is a new mathematical target, not a revision of evidence
/// already reviewed against the old target. Called in the change transaction.
pub(super) fn record_problem_version(state: &mut Value) {
    if array(state, "nodes")
        .iter()
        .any(|n| n["node_type"] == "problem" && n["problem_version"] == state["problem_version"])
    {
        return;
    }
    let node = json!({"id":id(),"node_type":"problem","math_kind":"problem","title":state["title"],"body":state["problem"],"exact_statement":state["problem"],
        "statement_sha256":digest(state["problem"].as_str().unwrap_or_default()),"problem_version":state["problem_version"],"revision":1,"work_state":"open","assurance":"unreviewed","validity":"current","created_at":now()});
    push(state, "nodes", node);
}

/// Normalize the visible root without rewriting any already pinned reference.
/// Description-only edits keep the mathematical node revision unchanged.
pub(super) fn sync_problem_spec(state: &mut Value) {
    let spec = super::problem24::spec(state);
    let Some(statement) = spec["math_statement"].as_str().filter(|s| !s.is_empty()) else {
        return;
    };
    record_problem_version(state);
    let Some(before) = array(state, "nodes")
        .iter()
        .find(|node| {
            node["node_type"] == "problem" && node["problem_version"] == state["problem_version"]
        })
        .cloned()
    else {
        return;
    };
    let mut next = before.clone();
    if exact(&before) != statement {
        push(state, "node_versions", before);
        next["revision"] = json!(next["revision"].as_u64().unwrap_or(1) + 1);
        next["body"] = json!(statement);
        next["exact_statement"] = json!(statement);
        next["statement_sha256"] = json!(digest(statement));
        next["work_state"] = json!("open");
        next["assurance"] = json!("unreviewed");
    }
    next["problem_spec_revision"] = spec["revision"].clone();
    next["problem_source_refs"] = spec["problem_source_refs"].clone();
    next["normalization_state"] = spec["normalization_state"].clone();
    next["mathematical_equivalence"] = spec["mathematical_equivalence"].clone();
    next["updated_at"] = spec["updated_at"].clone();
    if let Some(nodes) = state["nodes"].as_array_mut() {
        if let Some(node) = nodes.iter_mut().find(|node| node["id"] == next["id"]) {
            *node = next;
        }
    }
}

fn reference(kind: &str, value: &Value) -> Value {
    let artifact = value["body_artifact_id"]
        .as_str()
        .or_else(|| value["proof_artifact_id"].as_str());
    // Candidate/fact IDs denote immutable mathematical content. Their ordinary
    // revision increments on review/validity updates, so keep it separate.
    let content_revision = if matches!(kind, "candidate" | "fact" | "artifact") {
        1
    } else {
        value["revision"].as_u64().unwrap_or(1)
    };
    json!({"kind":kind,"id":value["id"],"revision":content_revision,"state_revision":value["revision"].as_u64().unwrap_or(1),
        "artifact_id":artifact,"sha256":digest(exact(value)),"snapshot_hash":value["snapshot_hash"],
        "problem_version":value["problem_version"]})
}

fn resolve_ref(state: &Value, input: &Value) -> V2Result<Value> {
    let kind = input["kind"]
        .as_str()
        .ok_or_else(|| err("INVALID_MATH_REF", "需要引用类型"))?;
    let key = input["id"]
        .as_str()
        .ok_or_else(|| err("INVALID_MATH_REF", "需要引用 ID"))?;
    let rev = input["revision"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| err("INVALID_MATH_REF", "数学引用必须指定版本"))?;
    let value = match kind {
        "node" => node_version(state, key, rev),
        "candidate" => entity(state, "candidates", key).filter(|_| rev == 1),
        "fact" => entity(state, "facts", key).filter(|_| rev == 1),
        "artifact" => entity(state, "artifacts", key).filter(|_| rev == 1),
        _ => None,
    }
    .ok_or_else(|| err("MATH_VERSION_MISSING", "引用的精确版本不存在或已变化"))?;
    if kind != "artifact" && value["problem_version"] != state["problem_version"] {
        return Err(err(
            "STALE_PROBLEM_VERSION",
            "不能将旧题面成果绑定到当前题目",
        ));
    }
    let mut result = reference(kind, value);
    if kind == "artifact" {
        result["artifact_id"] = json!(key);
        result["sha256"] = value["sha256"].clone();
    }
    for field in ["sha256", "artifact_id", "snapshot_hash", "problem_version"] {
        if !input[field].is_null() && input[field] != result[field] {
            return Err(err("MATH_VERSION_CONFLICT", "引用的内容或题面版本不匹配"));
        }
    }
    Ok(result)
}

fn researcher(state: &Value, run: &str, session: &str) -> V2Result<()> {
    let actor =
        entity(state, "sessions", session).ok_or_else(|| err("NOT_FOUND", "研究会话不存在"))?;
    if !matches!(actor["role"].as_str(), Some("main" | "partner"))
        || actor["run_id"] != run
        || actor["problem_version"] != state["problem_version"]
    {
        return Err(err("ROLE_DENIED", "只有当前题面研究会话可提交数学成果"));
    }
    Ok(())
}

impl V2Service {
    /// Freeze workspace drafts before committing the node and its versioned refs.
    pub(super) async fn record_finding23(
        &self,
        project: &str,
        run: &str,
        session: &str,
        epoch: u64,
        action: &Value,
        output: &Value,
        workspace: &Path,
    ) -> V2Result<Value> {
        let before = self.store.read(project).await?;
        super::validate_action_capture(&before, run, session, epoch, output)?;
        researcher(&before, run, session)?;
        let mut paths: Vec<String> = Vec::new();
        for field in ["draft_path", "proof_path"] {
            if let Some(path) = action[field].as_str() {
                paths.push(path.to_owned());
            }
        }
        for draft in array(action, "draft_refs") {
            let path = draft
                .as_str()
                .or_else(|| draft["path"].as_str())
                .ok_or_else(|| err("INVALID_FINDING", "草稿引用需要工作区相对路径"))?;
            paths.push(path.to_owned());
        }
        paths.sort();
        paths.dedup();
        if paths.len() > 16 {
            return Err(err("RESOURCE", "一次成果最多冻结 16 份原稿"));
        }
        let mut frozen = Vec::new();
        for path in paths {
            let body = super::read_workspace_text(workspace, &path).await?;
            let artifact = self
                .store
                .put_artifact(
                    project,
                    "finding-draft.md",
                    body.as_bytes(),
                    "text/markdown",
                )
                .await?;
            frozen.push(json!({"kind":"artifact","id":artifact["id"],"artifact_id":artifact["id"],"revision":1,"sha256":artifact["sha256"],"original_path":path}));
        }
        let mut enriched = action.clone();
        enriched["draft_artifact_refs"] = json!(frozen);
        enriched["original_output_artifact_id"] = output["id"].clone();
        // Preserve the exact submission even without a separate draft file.
        let body = serde_json::to_vec_pretty(&enriched)
            .map_err(|e| err("INVALID_FINDING", e.to_string()))?;
        let snapshot = self
            .store
            .put_artifact(project, "finding-snapshot.json", &body, "application/json")
            .await?;
        self.store
            .mutate(project, "math.finding_saved", None, |state| {
                super::validate_action_capture(state, run, session, epoch, output)?;
                researcher(state, run, session)?;
                let node =
                    super::enhancements::record_finding(state, &enriched, session, &snapshot, run)?;
                record_finding_relations(state, &node, &enriched, session)?;
                Ok(node)
            })
            .await
    }

    pub async fn proof_tree23(&self, project: &str) -> V2Result<Value> {
        Ok(project_tree(&self.store.read(project).await?))
    }

    #[must_use]
    pub fn proof_tree_from_snapshot23(state: &Value) -> Value {
        project_tree(state)
    }
}

fn add_edge(
    state: &mut Value,
    from: Value,
    to: Value,
    relation: &str,
    status: &str,
    proof: Value,
    author: &str,
) -> Value {
    let group = proof["id"].as_str().map(|v| format!("proof:{v}"));
    let mut edge = json!({"id":id(),"relation":relation,"status":status,
        "premise_group_id":group,"author_session_id":author,
        "problem_version":state["problem_version"],"created_at":now(),"schema_version":"2.3"});
    edge["from"] = from;
    edge["to"] = to;
    edge["proof_ref"] = proof;
    if let Some(existing) = array(state, "edges").iter().find(|e| {
        e["from"] == edge["from"]
            && e["to"] == edge["to"]
            && e["relation"] == edge["relation"]
            && e["status"] == edge["status"]
            && e["proof_ref"] == edge["proof_ref"]
    }) {
        return existing.clone();
    }
    push(state, "edges", edge.clone());
    edge
}

fn record_finding_relations(
    state: &mut Value,
    node: &Value,
    action: &Value,
    session: &str,
) -> V2Result<()> {
    let refs = array(action, "proposed_dependency_refs");
    if refs.len() > 128 {
        return Err(err("INVALID_RELATION", "拟依赖数量过多"));
    }
    for input in refs {
        let from = resolve_ref(state, input)?;
        add_edge(
            state,
            from,
            reference("node", node),
            "dependency",
            "proposed",
            Value::Null,
            session,
        );
    }
    Ok(())
}

/// Only proposed research relations are writable by researchers. Checked use is
/// a read projection of a matching independent accepted review, never an action.
pub(super) fn apply_action(
    state: &mut Value,
    run: &str,
    session: &str,
    action: &Value,
    artifact: &Value,
) -> V2Result<Option<Value>> {
    let kind = action["type"]
        .as_str()
        .or_else(|| action["action"].as_str())
        .unwrap_or_default();
    if !matches!(kind, "record_goal" | "record_relation") {
        return Ok(None);
    }
    researcher(state, run, session)?;
    if kind == "record_goal" {
        if entity(state, "sessions", session).is_none_or(|s| s["role"] != "main") {
            return Err(err("ROLE_DENIED", "研究目标由领研猫安排"));
        }
        let goal_kind = action["goal_kind"].as_str().unwrap_or("stage");
        if !matches!(goal_kind, "stage" | "extension") {
            return Err(err("INVALID_GOAL", "目标类型只能为 stage 或 extension"));
        }
        let parent = if action["parent_goal_ref"].is_null() {
            None
        } else {
            Some(goal_ref(state, &action["parent_goal_ref"])?)
        };
        let mut finding = action.clone();
        finding["kind"] = json!("goal");
        finding["goal_kind"] = json!(goal_kind);
        let node = super::enhancements::record_finding(state, &finding, session, artifact, run)?;
        if let Some(parent) = parent {
            add_edge(
                state,
                reference("node", &node),
                parent,
                "goal_membership",
                "proposed",
                Value::Null,
                session,
            );
        }
        return Ok(Some(node));
    }
    if action["status"].as_str().is_some_and(|s| s != "proposed") {
        return Err(err("ROLE_DENIED", "研究者不能声明关系已审核"));
    }
    let relation = action["relation"].as_str().unwrap_or("dependency");
    if !matches!(relation, "dependency" | "goal_membership") {
        return Err(err("INVALID_RELATION", "未知关系类型"));
    }
    let from = resolve_ref(state, &action["from"])?;
    let to = if relation == "goal_membership" {
        goal_ref(state, &action["to"])?
    } else {
        resolve_ref(state, &action["to"])?
    };
    Ok(Some(add_edge(
        state,
        from,
        to,
        relation,
        "proposed",
        Value::Null,
        session,
    )))
}

fn goal_ref(state: &Value, input: &Value) -> V2Result<Value> {
    let resolved = resolve_ref(state, input)?;
    let target = node_version(
        state,
        resolved["id"].as_str().unwrap_or_default(),
        resolved["revision"].as_u64().unwrap_or_default(),
    );
    if resolved["kind"] != "node"
        || target.is_none_or(|v| !matches!(v["node_type"].as_str(), Some("problem" | "goal")))
    {
        return Err(err(
            "INVALID_GOAL",
            "覆盖目标必须引用具体题面或阶段目标版本",
        ));
    }
    Ok(resolved)
}

pub(super) fn candidate_metadata(state: &Value, action: &Value, _session: &str) -> V2Result<Value> {
    let math_ref = if action["math_node_ref"].is_null() {
        Value::Null
    } else {
        let resolved = resolve_ref(state, &action["math_node_ref"])?;
        let node = node_version(
            state,
            resolved["id"].as_str().unwrap_or_default(),
            resolved["revision"].as_u64().unwrap_or_default(),
        );
        let statement = action["exact_statement"]
            .as_str()
            .or_else(|| action["claim"].as_str())
            .unwrap_or_default();
        if resolved["kind"] != "node" || node.is_none_or(|n| exact(n) != statement) {
            return Err(err(
                "MATH_STATEMENT_MISMATCH",
                "候选必须精确证明所绑定数学节点的陈述；更改命题请先保存新版本",
            ));
        }
        resolved
    };
    let mut goals = Vec::new();
    if array(action, "goal_refs").len() > 64 {
        return Err(err("INVALID_GOAL", "单个候选覆盖目标过多"));
    }
    for input in array(action, "goal_refs") {
        let relation = input["relation"].as_str().unwrap_or("supports");
        if !matches!(relation, "supports" | "covers") {
            return Err(err("INVALID_GOAL", "目标关系只能为 supports 或 covers"));
        }
        let target = goal_ref(state, input.get("ref").unwrap_or(input))?;
        goals.push(json!({"ref":target,"relation":relation}));
    }
    // Legacy covers_goal remains supported, but is now pinned to this exact root.
    if action["covers_goal"] == true {
        if let Some(root) = array(state, "nodes").iter().find(|n| {
            n["node_type"] == "problem" && n["problem_version"] == state["problem_version"]
        }) {
            let root_ref = reference("node", root);
            if !goals
                .iter()
                .any(|g| g["ref"] == root_ref && g["relation"] == "covers")
            {
                goals.push(json!({"ref":root_ref,"relation":"covers"}));
            }
        }
    }
    Ok(
        json!({"math_node_ref":math_ref,"goal_refs":goals,"problem_context":super::problem_context::snapshot(state)}),
    )
}

pub(super) fn augment_verification_packet(state: &Value, candidate: &Value, packet: &mut Value) {
    let bound_node = |r: &Value| {
        node_version(
            state,
            r["id"].as_str().unwrap_or_default(),
            r["revision"].as_u64().unwrap_or_default(),
        )
        .cloned()
        .unwrap_or(Value::Null)
    };
    packet["bound_mathematical_node"] = bound_node(&candidate["math_node_ref"]);
    packet["exact_goal_targets"] = json!(
        array(candidate, "goal_refs")
            .iter()
            .map(
                |g| json!({"ref":g["ref"],"relation":g["relation"],"target":bound_node(&g["ref"])})
            )
            .collect::<Vec<_>>()
    );
    packet["scope_audit_instruction"] = json!(
        "Audit the frozen claim against all assumptions, symbol scopes and quantifiers of bound_mathematical_node. Goal support is not coverage. Root goal_coverage must cover the full frozen problem_context for the candidate problem_version, including the original target and pinned problem_sources; never substitute a later problem or normalization. A stage or extension alone does not cover the root. Different goal_refs do not expand claim_coverage automatically."
    );
}

pub(super) fn record_candidate(state: &mut Value, candidate: &Value) -> V2Result<()> {
    let conclusion = if candidate["math_node_ref"].is_object() {
        candidate["math_node_ref"].clone()
    } else {
        reference("candidate", candidate)
    };
    let proof = reference("candidate", candidate);
    let author = candidate["author_session_id"].as_str().unwrap_or_default();
    for premise in array(candidate, "declared_premises") {
        let Some(key) = premise["ref_id"].as_str() else {
            continue;
        };
        let kind = match premise["kind"].as_str() {
            Some("fact") => "fact",
            Some("conditional") => "candidate",
            Some("source") => "artifact",
            _ => continue,
        };
        let from = json!({"kind":kind,"id":key,"revision":1,"state_revision":premise["revision"],"sha256":premise["sha256"],"artifact_id":if kind=="artifact"{json!(key)}else{premise["proof_artifact_id"].clone()},"snapshot_hash":premise["snapshot_hash"],"problem_version":candidate["problem_version"]});
        let mut edge = add_edge(
            state,
            from,
            conclusion.clone(),
            "dependency",
            "declared",
            proof.clone(),
            author,
        );
        edge["premise_id"] = premise["id"].clone();
        edge["premise_statement"] = premise["statement"].clone();
        edge["usage_location"] = premise["usage_location"].clone();
        let key = edge["id"].as_str().unwrap_or_default();
        *super::entity_mut(state, "edges", key)? = edge.clone();
    }
    for target in array(candidate, "goal_refs") {
        let mut edge = add_edge(
            state,
            conclusion.clone(),
            target["ref"].clone(),
            "goal_membership",
            "declared",
            proof.clone(),
            author,
        );
        edge["coverage_claim"] = target["relation"].clone();
        let key = edge["id"].as_str().unwrap_or_default();
        *super::entity_mut(state, "edges", key)? = edge.clone();
    }
    Ok(())
}

fn checked_review<'a>(state: &'a Value, candidate: &Value) -> Option<&'a Value> {
    array(state, "reviews")
        .iter()
        .rev()
        .find(|r| {
            r["candidate_id"] == candidate["id"]
                && r["snapshot_hash"] == candidate["snapshot_hash"]
                && r["state"] == "completed"
        })
        .filter(|r| {
            r["verdict"] == "accepted"
                && r["report_validated"] == true
                && r["reviewer_session_id"] != candidate["author_session_id"]
        })
}

fn admitted_fact<'a>(state: &'a Value, candidate: &Value) -> Option<&'a Value> {
    array(state, "facts").iter().find(|f| {
        f["candidate_id"] == candidate["id"]
            && f["snapshot_hash"] == candidate["snapshot_hash"]
            && matches!(
                f["assurance"].as_str(),
                Some("model_reviewed" | "formally_checked")
            )
    })
}

fn ref_key(value: &Value) -> String {
    format!(
        "{}:{}@{}",
        value["kind"].as_str().unwrap_or_default(),
        value["id"].as_str().unwrap_or_default(),
        value["revision"].as_u64().unwrap_or(1)
    )
}

fn visible_node(kind: &str, value: &Value, state: &Value) -> Value {
    let history: Vec<Value> = if kind == "node" {
        array(state, "node_versions")
            .iter()
            .filter(|n| n["id"] == value["id"] || n["node_id"] == value["id"])
            .map(|n| reference("node", n))
            .collect()
    } else {
        Vec::new()
    };
    let math_kind = value["math_kind"]
        .as_str()
        .or_else(|| value["node_type"].as_str())
        .unwrap_or("claim");
    json!({"id":format!("{kind}:{}",value["id"].as_str().unwrap_or_default()),"ref":reference(kind,value),
        "title":value["title"].as_str().unwrap_or_else(|| exact(value)),"exact_statement":exact(value),"math_kind":math_kind,
        "mathematical_statements":value.get("mathematical_statements").cloned().unwrap_or_else(||json!([])),
        "problem_version":value["problem_version"],"assumptions":value["assumptions"],"symbols":value["symbols"],"scope":value["scope"],
        "work_state":value["work_state"].as_str().unwrap_or("recorded"),"review_state":"unreviewed","admission_state":"not_submitted",
        "validity":if value["problem_version"] == state["problem_version"] {value.get("validity").cloned().unwrap_or_else(||json!("current"))} else {json!("historical_problem")},
        "body_artifact_id":value["body_artifact_id"],"proof_artifact_id":value["proof_artifact_id"],"draft_artifact_refs":value.get("draft_artifact_refs").cloned().unwrap_or_else(||json!([])),
        "candidate_id":value["candidate_id"],"historical_versions":history,"proof_refs":[],"fact_refs":[],"predecessor_refs":[],"successor_refs":[],
        "updated_at":value["updated_at"],"checkpoint":value["checkpoint"],"summary_is_evidence":false})
}

/// No independent store and no inferred mathematical certification. Every
/// projected proof and edge points back to its frozen candidate or node version.
pub(super) fn project_tree(state: &Value) -> Value {
    let mut nodes: Vec<Value> = array(state, "nodes")
        .iter()
        .filter(|n| {
            matches!(
                n["node_type"].as_str(),
                Some("problem" | "claim" | "goal" | "source")
            )
        })
        .map(|n| visible_node("node", n, state))
        .collect();
    let mut proofs = Vec::new();
    let mut aliases = HashMap::<String, Value>::new();
    for candidate in array(state, "candidates") {
        let review = checked_review(state, candidate);
        let fact = admitted_fact(state, candidate);
        let latest_review = array(state, "reviews").iter().rev().find(|r| {
            r["candidate_id"] == candidate["id"] && r["snapshot_hash"] == candidate["snapshot_hash"]
        });
        let review_state = latest_review.map_or_else(
            || candidate["status"].as_str().unwrap_or("unreviewed"),
            |r| {
                if r["state"] == "completed" {
                    r["verdict"].as_str().unwrap_or("inconclusive")
                } else {
                    r["state"].as_str().unwrap_or("unreviewed")
                }
            },
        );
        let validity = if candidate["problem_version"] != state["problem_version"] {
            "historical_problem"
        } else if let Some(fact) = fact {
            fact["validity"].as_str().unwrap_or("unknown")
        } else if matches!(
            candidate["status"].as_str(),
            Some("superseded" | "withdrawn")
        ) {
            "superseded"
        } else {
            "current"
        };
        let proof_ref = reference("candidate", candidate);
        let target = if candidate["math_node_ref"].is_object() {
            candidate["math_node_ref"].clone()
        } else {
            proof_ref.clone()
        };
        aliases.insert(
            format!("candidate:{}", candidate["id"].as_str().unwrap_or_default()),
            target.clone(),
        );
        if let Some(f) = fact {
            aliases.insert(
                format!("fact:{}", f["id"].as_str().unwrap_or_default()),
                target.clone(),
            );
        }
        let proof = json!({"ref":proof_ref,"conclusion_ref":target,"candidate_id":candidate["id"],"lineage_id":candidate["lineage_id"],"repair_of":candidate["repair_of"],
            "exact_statement":exact(candidate),"proof_artifact_id":candidate["proof_artifact_id"],"snapshot_hash":candidate["snapshot_hash"],
            "declared_premises":candidate["declared_premises"],"goal_refs":candidate["goal_refs"],"review_state":review_state,
            "admission_state":if fact.is_some(){"accepted"}else{"not_admitted"},"validity":validity,"review_id":review.map(|r|r["id"].clone()),"fact_ref":fact.map(|f|reference("fact",f)),"premise_group_id":format!("proof:{}",candidate["id"].as_str().unwrap_or_default())});
        let index = nodes.iter().position(|n| {
            n["ref"]["kind"] == target["kind"]
                && n["ref"]["id"] == target["id"]
                && n["ref"]["revision"] == target["revision"]
        });
        let index = index.unwrap_or_else(|| {
            let source = if target["kind"] == "node" {
                node_version(
                    state,
                    target["id"].as_str().unwrap_or_default(),
                    target["revision"].as_u64().unwrap_or(1),
                )
                .unwrap_or(candidate)
            } else {
                candidate
            };
            let mut node = visible_node(
                target["kind"].as_str().unwrap_or("candidate"),
                source,
                state,
            );
            if target["kind"] == "node" {
                node["id"] = json!(ref_key(&target));
                node["is_historical_version"] = json!(true);
            }
            nodes.push(node);
            nodes.len() - 1
        });
        let node = &mut nodes[index];
        push(node, "proof_refs", proof_ref);
        if node["admission_state"] != "accepted" || (fact.is_some() && validity == "current") {
            node["review_state"] = json!(review_state);
            node["admission_state"] = proof["admission_state"].clone();
            node["validity"] = json!(validity);
            node["candidate_id"] = candidate["id"].clone();
            node["proof_artifact_id"] = candidate["proof_artifact_id"].clone();
        }
        if let Some(fact) = fact {
            push(node, "fact_refs", reference("fact", fact));
        }
        proofs.push(proof);
    }
    let mut edges: Vec<Value> = array(state, "edges")
        .iter()
        .filter(|e| e["schema_version"] == "2.3")
        .cloned()
        .collect();
    for edge in &mut edges {
        if edge["status"] == "declared" && edge["relation"] == "dependency" {
            if let Some(candidate) = entity(
                state,
                "candidates",
                edge["proof_ref"]["id"].as_str().unwrap_or_default(),
            ) {
                if let Some(review) = checked_review(state, candidate) {
                    if edge["proof_ref"]["snapshot_hash"] == candidate["snapshot_hash"]
                        && array(
                            &review["raw_verification"]["verification_report"],
                            "checked_premise_ids",
                        )
                        .contains(&edge["premise_id"])
                    {
                        edge["status"] = json!("checked");
                        edge["review_id"] = review["id"].clone();
                    }
                }
            }
        }
        for side in ["from", "to"] {
            let r = &edge[side];
            let key = format!(
                "{}:{}",
                r["kind"].as_str().unwrap_or_default(),
                r["id"].as_str().unwrap_or_default()
            );
            edge[format!("{side}_node_ref")] =
                aliases.get(&key).cloned().unwrap_or_else(|| r.clone());
        }
        edge["premise_validity"] = match edge["from"]["kind"].as_str() {
            Some("fact") => entity(
                state,
                "facts",
                edge["from"]["id"].as_str().unwrap_or_default(),
            )
            .map_or_else(|| json!("missing"), |f| f["validity"].clone()),
            _ => json!("unreviewed"),
        };
    }
    // A historical premise or a raw source remains inspectable even if it is
    // only mentioned by an edge and is not one of today's current node rows.
    for edge in &edges {
        for side in ["from", "to"] {
            let target = &edge[format!("{side}_node_ref")];
            if nodes.iter().any(|n| ref_key(&n["ref"]) == ref_key(target)) {
                continue;
            }
            let key = target["id"].as_str().unwrap_or_default();
            let kind = target["kind"].as_str().unwrap_or_default();
            let source = match kind {
                "node" => node_version(state, key, target["revision"].as_u64().unwrap_or(1)),
                "artifact" => entity(state, "artifacts", key),
                "fact" => entity(state, "facts", key),
                "candidate" => entity(state, "candidates", key),
                _ => None,
            };
            if let Some(source) = source {
                let mut node = visible_node(kind, source, state);
                node["ref"] = target.clone();
                node["id"] = json!(ref_key(target));
                if kind == "artifact" {
                    node["math_kind"] = json!("source");
                    node["title"] = source["name"].clone();
                    node["body_artifact_id"] = json!(key);
                    node["validity"] = json!("source_only");
                    node["exact_statement"] = json!("");
                } else if source["revision"] != target["revision"] {
                    node["is_historical_version"] = json!(true);
                    if side == "from" && edge["premise_statement"].is_string() {
                        node["exact_statement"] = edge["premise_statement"].clone();
                    }
                }
                nodes.push(node);
            }
        }
    }
    let mut warnings = Vec::new();
    mark_cycles(&mut edges, &mut warnings);
    for node in &mut nodes {
        let key = ref_key(&node["ref"]);
        for edge in &edges {
            if edge["relation"] != "dependency" {
                continue;
            }
            if ref_key(&edge["to_node_ref"]) == key {
                push(node, "predecessor_refs", edge["from"].clone());
            }
            if ref_key(&edge["from_node_ref"]) == key {
                push(node, "successor_refs", edge["to"].clone());
            }
        }
    }
    let goals = goal_projection(state, &proofs);
    json!({"schema_version":"2.3","project_id":state["id"],"problem_version":state["problem_version"],"revision":state["revision"],"event_cursor":state["event_cursor"],
        "nodes":nodes,"edges":edges,"proofs":proofs,"goals":goals,"warnings":warnings,
        "direction":"premise_to_conclusion","premise_group_semantics":"all premises in one proof are required; different proofs are alternatives",
        "summary_is_evidence":false})
}

fn goal_projection(state: &Value, proofs: &[Value]) -> Vec<Value> {
    array(state,"nodes").iter().filter(|g| matches!(g["node_type"].as_str(),Some("problem" | "goal"))).map(|goal| {
        let target=reference("node",goal); let mut evidence=Vec::new();
        for proof in proofs {
            for claim in array(proof,"goal_refs").iter().filter(|g| g["ref"] == target) {
                let candidate=entity(state,"candidates",proof["candidate_id"].as_str().unwrap_or_default());
                let review=candidate.and_then(|c|checked_review(state,c));
                // A root coverage decision is explicitly audited. A stage can
                // inherit claim coverage only when its exact statement matches.
                let covered = claim["relation"] == "covers" && proof["admission_state"] == "accepted" && proof["validity"] == "current" && goal["problem_version"] == state["problem_version"]
                    && if goal["node_type"] == "problem" {review.is_some_and(|r|r["goal_coverage"] == true)} else {proof["exact_statement"] == exact(goal) && review.is_some()};
                evidence.push(json!({"proof_ref":proof["ref"],"fact_ref":proof["fact_ref"],"review_id":proof["review_id"],"relation":claim["relation"],"status":if covered{"reviewed_coverage"}else{"claimed"},"exact_goal_ref":target}));
            }
        }
        json!({"ref":target,"goal_kind":if goal["node_type"] == "problem" {json!("root")} else {goal["goal_kind"].clone()},"exact_statement":exact(goal),"problem_version":goal["problem_version"],
            "status":if goal["problem_version"] != state["problem_version"] {"historical"}else if evidence.iter().any(|e|e["status"] == "reviewed_coverage"){"reviewed_coverage"}else{"open"},"coverage_evidence":evidence})
    }).collect()
}

fn mark_cycles(edges: &mut [Value], warnings: &mut Vec<Value>) {
    let mut graph = HashMap::<String, Vec<String>>::new();
    for e in edges.iter().filter(|e| e["relation"] == "dependency") {
        graph
            .entry(ref_key(&e["from_node_ref"]))
            .or_default()
            .push(ref_key(&e["to_node_ref"]));
    }
    for edge in edges.iter_mut().filter(|e| e["relation"] == "dependency") {
        let source = ref_key(&edge["from_node_ref"]);
        let mut pending = vec![ref_key(&edge["to_node_ref"])];
        let mut seen = HashSet::new();
        while let Some(key) = pending.pop() {
            if key == source {
                edge["in_potential_cycle"] = json!(true);
                warnings.push(json!({"code":"POTENTIAL_DEPENDENCY_CYCLE","edge_id":edge["id"],"message":"存在回路；不能把该回路当作完整证明路径，需按所选备选证明核对"}));
                break;
            }
            if seen.insert(key.clone()) {
                if let Some(children) = graph.get(&key) {
                    pending.extend(children.iter().cloned());
                }
            }
        }
    }
}

pub(super) fn prompt() -> &'static str {
    "MathCat 2.3: At meaningful checkpoints use record_finding with exact statement, assumptions, symbols, scope and optional draft_path/draft_refs (workspace-relative files are frozen). For revision use node_id + expected_revision. No long report or per-step plan is required. proposed_dependency_refs use {kind,id,revision}; they do not confer truth. Main may record_goal {statement,goal_kind:stage|extension,parent_goal_ref?}. Use record_relation {from,to,relation:dependency|goal_membership,status:proposed} for tentative relations. Submission may include math_node_ref {kind:node,id,revision} only for an identical exact_statement and goal_refs [{ref:{kind:node,id,revision},relation:supports|covers}]. Supporting a goal does not complete it. All nontrivial premises still require exact reads and declared_premises; include revision and sha256 when available. Each submitted proof is a separate AND group of premises; alternative proofs may share a mathematical node. Never claim checked relations or Fact admission yourself. Read frozen drafts/precise statements before proof reuse; summaries have no inherited assurance."
}

#[cfg(test)]
#[path = "proof_tree23_tests.rs"]
mod tests;
