use super::*;

fn state() -> Value {
    json!({"id":"p","problem_version":1,"revision":1,"event_cursor":1,
        "nodes":[{"id":"root","node_type":"problem","body":"Every A is B","revision":1,"problem_version":1,"validity":"current"}],
        "sessions":[{"id":"main","role":"main","run_id":"run","problem_version":1},{"id":"partner","role":"partner","run_id":"run","problem_version":1}],
        "edges":[],"candidates":[],"facts":[],"reviews":[],"artifacts":[],"memories":[]})
}

fn finding(state: &mut Value, statement: &str) -> Value {
    super::super::enhancements::record_finding(
        state,
        &json!({"statement":statement,"kind":"lemma"}),
        "main",
        &json!({"id":"draft-v1"}),
        "run",
    )
    .unwrap()
}

fn candidate(state: &mut Value, key: &str, statement: &str, metadata: &Value) -> Value {
    let mut c = json!({"id":key,"revision":1,"lineage_id":key,"exact_statement":statement,"claim":statement,"problem_version":1,
        "author_session_id":"main","snapshot_hash":format!("hash-{key}"),"status":"submitted","proof_artifact_id":format!("proof-{key}"),"declared_premises":[]});
    c["math_node_ref"] = metadata["math_node_ref"].clone();
    c["goal_refs"] = metadata["goal_refs"].clone();
    push(state, "candidates", c.clone());
    record_candidate(state, &c).unwrap();
    c
}

fn accept(state: &mut Value, c: &Value, goal_coverage: bool) {
    push(
        state,
        "reviews",
        json!({"id":format!("review-{}",c["id"].as_str().unwrap()),"candidate_id":c["id"],"snapshot_hash":c["snapshot_hash"],"reviewer_session_id":"reviewer","state":"completed","verdict":"accepted","report_validated":true,"goal_coverage":goal_coverage,
        "raw_verification":{"verification_report":{"checked_premise_ids":["f0"]}}}),
    );
    push(
        state,
        "facts",
        json!({"id":format!("fact-{}",c["id"].as_str().unwrap()),"candidate_id":c["id"],"snapshot_hash":c["snapshot_hash"],"problem_version":1,"revision":1,"assurance":"model_reviewed","validity":"current","claim":c["claim"],"proof_artifact_id":c["proof_artifact_id"]}),
    );
}

#[test]
fn revisions_preserve_old_statement_and_proof_binding() {
    let mut s = state();
    let first = finding(&mut s, "A implies B");
    let meta = candidate_metadata(
        &s,
        &json!({"claim":"A implies B","math_node_ref":reference("node",&first)}),
        "main",
    )
    .unwrap();
    candidate(&mut s, "c1", "A implies B", &meta);
    let next = super::super::enhancements::record_finding(
        &mut s,
        &json!({"node_id":first["id"],"expected_revision":1,"statement":"A and C imply B"}),
        "main",
        &json!({"id":"draft-v2"}),
        "run",
    )
    .unwrap();
    assert_eq!(next["revision"], 2);
    assert_eq!(
        node_version(&s, first["id"].as_str().unwrap(), 1).unwrap()["exact_statement"],
        "A implies B"
    );
    let tree = project_tree(&s);
    assert_eq!(tree["proofs"][0]["conclusion_ref"]["revision"], 1);
    let current = array(&tree, "nodes")
        .iter()
        .find(|n| n["ref"]["id"] == first["id"] && n["ref"]["revision"] == 2)
        .unwrap();
    assert_eq!(current["historical_versions"][0]["artifact_id"], "draft-v1");
    assert!(array(current, "proof_refs").is_empty());
    assert!(
        candidate_metadata(
            &s,
            &json!({"claim":"A implies B","math_node_ref":reference("node",&next)}),
            "main"
        )
        .is_err()
    );
}

#[test]
fn shared_nodes_keep_alternative_proofs_and_and_groups_separate() {
    let mut s = state();
    let node = finding(&mut s, "L");
    let meta = candidate_metadata(
        &s,
        &json!({"claim":"L","math_node_ref":reference("node",&node)}),
        "main",
    )
    .unwrap();
    let a = candidate(&mut s, "a", "L", &meta);
    let b = candidate(&mut s, "b", "L", &meta);
    accept(&mut s, &a, false);
    let tree = project_tree(&s);
    let projected = array(&tree, "nodes")
        .iter()
        .find(|n| n["ref"]["id"] == node["id"])
        .unwrap();
    assert_eq!(array(projected, "proof_refs").len(), 2);
    assert_eq!(array(&tree, "nodes").len(), 2); // root plus one shared lemma
    assert_eq!(tree["proofs"][0]["admission_state"], "accepted");
    assert_eq!(tree["proofs"][1]["admission_state"], "not_admitted");
    assert_ne!(
        tree["proofs"][0]["premise_group_id"],
        tree["proofs"][1]["premise_group_id"]
    );
    assert_eq!(b["exact_statement"], "L");
}

#[test]
fn a_fact_or_supporting_claim_does_not_complete_the_problem() {
    let mut s = state();
    let root = reference("node", &s["nodes"][0]);
    let meta = candidate_metadata(
        &s,
        &json!({"claim":"A special case","goal_refs":[{"ref":root,"relation":"supports"}]}),
        "main",
    )
    .unwrap();
    let c = candidate(&mut s, "c", "A special case", &meta);
    accept(&mut s, &c, true);
    assert_eq!(project_tree(&s)["goals"][0]["status"], "open");
    let meta = candidate_metadata(
        &s,
        &json!({"claim":"Every A is B","covers_goal":true}),
        "main",
    )
    .unwrap();
    let c = candidate(&mut s, "d", "Every A is B", &meta);
    accept(&mut s, &c, false);
    assert_eq!(project_tree(&s)["goals"][0]["status"], "open");
    s["reviews"][1]["goal_coverage"] = json!(true);
    assert_eq!(project_tree(&s)["goals"][0]["status"], "reviewed_coverage");
    s["facts"][1]["validity"] = json!("challenged");
    assert_eq!(project_tree(&s)["goals"][0]["status"], "open");
    s["problem_version"] = json!(2);
    assert_eq!(project_tree(&s)["goals"][0]["status"], "historical");
}

#[test]
fn stage_and_extension_are_pinned_and_only_exact_reviewed_scope_covers_stage() {
    let mut s = state();
    let goal = apply_action(
        &mut s,
        "run",
        "main",
        &json!({"type":"record_goal","statement":"Local lemma","goal_kind":"stage"}),
        &json!({"id":"out"}),
    )
    .unwrap()
    .unwrap();
    assert!(
        apply_action(
            &mut s,
            "run",
            "partner",
            &json!({"type":"record_goal","statement":"New whole goal"}),
            &json!({})
        )
        .is_err()
    );
    let input = json!({"claim":"Different claim","goal_refs":[{"ref":reference("node",&goal),"relation":"covers"}]});
    let meta = candidate_metadata(&s, &input, "main").unwrap();
    let c = candidate(&mut s, "a", "Different claim", &meta);
    accept(&mut s, &c, false);
    assert_eq!(project_tree(&s)["goals"][1]["status"], "open");
    let c = candidate(&mut s, "b", "Local lemma", &meta);
    accept(&mut s, &c, false);
    assert_eq!(project_tree(&s)["goals"][1]["status"], "reviewed_coverage");
    assert_eq!(project_tree(&s)["goals"][0]["status"], "open");
}

#[test]
fn proposed_relations_cannot_be_self_certified_and_cycles_are_explicit() {
    let mut s = state();
    let a = finding(&mut s, "A");
    let b = finding(&mut s, "B");
    let mut action = json!({"type":"record_relation","from":reference("node",&a),"to":reference("node",&b),"status":"checked"});
    assert!(apply_action(&mut s, "run", "main", &action, &json!({})).is_err());
    action["status"] = json!("proposed");
    apply_action(&mut s, "run", "main", &action, &json!({})).unwrap();
    action["from"] = reference("node", &b);
    action["to"] = reference("node", &a);
    apply_action(&mut s, "run", "main", &action, &json!({})).unwrap();
    let tree = project_tree(&s);
    assert_eq!(array(&tree, "warnings").len(), 2);
    assert!(
        array(&tree, "edges")
            .iter()
            .all(|e| e["status"] == "proposed")
    );
    let na = array(&tree, "nodes")
        .iter()
        .find(|n| n["ref"]["id"] == a["id"])
        .unwrap();
    assert_eq!(na["successor_refs"][0]["id"], b["id"]);
}

#[test]
fn declared_premise_versions_survive_validity_change_and_review_stays_separate() {
    let mut s = state();
    push(
        &mut s,
        "facts",
        json!({"id":"f0","revision":1,"problem_version":1,"validity":"current","claim":"P","proof_artifact_id":"pf0"}),
    );
    let mut c = candidate(&mut s, "c", "Q", &json!({"goal_refs":[]}));
    c["declared_premises"] = json!([{"id":"f0","ref_id":"f0","kind":"fact","revision":1,"sha256":"exact","proof_artifact_id":"pf0","usage_location":"line 2"}]);
    s["candidates"][0] = c.clone();
    record_candidate(&mut s, &c).unwrap();
    assert_eq!(project_tree(&s)["edges"][0]["status"], "declared");
    accept(&mut s, &c, false);
    assert_eq!(project_tree(&s)["edges"][0]["status"], "checked");
    s["facts"][0]["validity"] = json!("challenged");
    s["facts"][0]["revision"] = json!(2);
    let tree = project_tree(&s);
    assert_eq!(tree["edges"][0]["from"]["revision"], 1);
    assert_eq!(tree["edges"][0]["premise_validity"], "challenged");
    s["reviews"][0]["verdict"] = json!("inconclusive");
    assert_eq!(project_tree(&s)["edges"][0]["status"], "declared");
}

#[test]
fn source_pins_and_problem_versions_cannot_silently_follow_new_content() {
    let mut s = state();
    let n = finding(&mut s, "A");
    let mut r = reference("node", &n);
    r["sha256"] = json!("wrong");
    assert!(resolve_ref(&s, &r).is_err());
    r = reference("node", &n);
    r["revision"] = Value::Null;
    assert!(resolve_ref(&s, &r).is_err());
    s["problem_version"] = json!(2);
    assert!(resolve_ref(&s, &reference("node", &n)).is_err());
}

#[test]
fn old_proposed_edges_keep_their_historical_endpoint_without_a_candidate() {
    let mut s = state();
    let a = finding(&mut s, "A");
    let b = finding(&mut s, "B");
    apply_action(
        &mut s,
        "run",
        "main",
        &json!({"type":"record_relation","from":reference("node",&a),"to":reference("node",&b)}),
        &json!({}),
    )
    .unwrap();
    super::super::enhancements::record_finding(
        &mut s,
        &json!({"node_id":a["id"],"expected_revision":1,"statement":"A revised"}),
        "main",
        &json!({"id":"draft-v2"}),
        "run",
    )
    .unwrap();
    let tree = project_tree(&s);
    let old = array(&tree, "nodes")
        .iter()
        .find(|n| n["ref"]["id"] == a["id"] && n["ref"]["revision"] == 1)
        .unwrap();
    assert_eq!(old["exact_statement"], "A");
    assert_eq!(old["successor_refs"][0]["id"], b["id"]);
    assert_eq!(tree["edges"][0]["from_node_ref"]["revision"], 1);
}

#[test]
fn review_state_revisions_do_not_retarget_frozen_mathematics() {
    let mut s = state();
    let c = candidate(&mut s, "c", "Q", &json!({"goal_refs":[]}));
    let pinned = reference("candidate", &c);
    s["candidates"][0]["revision"] = json!(4);
    s["candidates"][0]["status"] = json!("changes_requested");
    let resolved = resolve_ref(&s, &pinned).unwrap();
    assert_eq!(resolved["revision"], 1);
    assert_eq!(resolved["state_revision"], 4);
    assert_eq!(resolved["artifact_id"], pinned["artifact_id"]);
    assert_eq!(resolved["sha256"], pinned["sha256"]);
    let tree = project_tree(&s);
    assert_eq!(tree["proofs"][0]["ref"]["revision"], 1);
    assert_eq!(tree["proofs"][0]["review_state"], "changes_requested");
    s["candidates"][0]["proof_artifact_id"] = json!("illegally-replaced");
    assert!(resolve_ref(&s, &pinned).is_err());
}

#[test]
fn changing_problem_keeps_old_goal_and_creates_a_distinct_current_target() {
    let mut s = state();
    let old = reference("node", &s["nodes"][0]);
    s["problem_version"] = json!(2);
    s["problem"] = json!("Every A and C is B");
    record_problem_version(&mut s);
    record_problem_version(&mut s);
    assert_eq!(array(&s, "nodes").len(), 2);
    let meta =
        candidate_metadata(&s, &json!({"claim":"new proof","covers_goal":true}), "main").unwrap();
    assert_ne!(meta["goal_refs"][0]["ref"]["id"], old["id"]);
    assert_eq!(meta["goal_refs"][0]["ref"]["problem_version"], 2);
    let tree = project_tree(&s);
    assert_eq!(tree["goals"][0]["status"], "historical");
    assert_eq!(tree["goals"][1]["status"], "open");
}

#[tokio::test]
async fn frozen_draft_and_statement_survive_workspace_edits_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = tokio::fs::canonicalize(temp.path()).await.unwrap();
    let db = workspace.join("state.sqlite");
    let data = workspace.join("data");
    let store = research_storage::research_v2::V2Store::connect(&db, &data)
        .await
        .unwrap();
    let project = store
        .create_project(json!({"title":"P","problem":"P"}), "create")
        .await
        .unwrap();
    let project_id = project["id"].as_str().unwrap();
    store
        .mutate(project_id, "fixture", None, |s| {
            push(
                s,
                "runs",
                json!({"id":"run","state":"running","control_epoch":1}),
            );
            push(
                s,
                "sessions",
                json!({"id":"main","role":"main","run_id":"run","problem_version":1}),
            );
            Ok(Value::Null)
        })
        .await
        .unwrap();
    tokio::fs::write(
        workspace.join("draft.md"),
        "Original proof with exact quantifiers",
    )
    .await
    .unwrap();
    let service = V2Service::new(store.clone(), super::super::V2Config::default());
    let node = service
        .record_finding23(
            project_id,
            "run",
            "main",
            1,
            &json!({"statement":"For all n, P(n)","draft_path":"draft.md"}),
            &json!({"id":"output"}),
            &workspace,
        )
        .await
        .unwrap();
    tokio::fs::write(workspace.join("draft.md"), "Changed later")
        .await
        .unwrap();
    let reopened = research_storage::research_v2::V2Store::connect(&db, &data)
        .await
        .unwrap();
    let (_, bytes) = reopened
        .read_artifact(
            project_id,
            node["draft_artifact_refs"][0]["id"].as_str().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        "Original proof with exact quantifiers"
    );
    let saved = reopened.read(project_id).await.unwrap();
    assert_eq!(
        service
            .read_statement22(project_id, node["id"].as_str().unwrap())
            .await
            .unwrap()["text"],
        "For all n, P(n)"
    );
    assert_eq!(
        entity(&saved, "nodes", node["id"].as_str().unwrap()).unwrap()["exact_statement"],
        "For all n, P(n)"
    );
    assert!(array(&saved, "facts").is_empty());
    assert!(
        service
            .record_finding23(
                project_id,
                "run",
                "main",
                2,
                &json!({"statement":"stale"}),
                &json!({"id":"out"}),
                &workspace
            )
            .await
            .is_err()
    );
}
