use super::super::{
    Utc, V2Config, V2Service, V2Store, enhancements, evidence22, interactions, new_session,
    problem24, proof_tree23, push,
};
use super::*;

const SHORT: &str = "请研究材料中的猜想3.6。";
const EXACT: &str = "Let k have characteristic zero, n >= 2, R=k[[x_1,...,x_n]], m=(x_1,...,x_n), f in m, and J_f m-primary. Prove J_f:f is not contained in (f,J_f).";

fn input_state() -> Value {
    json!({"id":"p","problem":SHORT,"problem_version":1,"problem_versions":[{"version":1,"problem":SHORT}],
        "artifacts":[{"id":"input","name":"problem.md","source_type":"user_import","sha256":"hash","created_at":"2026-09-08T10:00:00Z"}],
        "runs":[{"problem_version":1,"created_at":"2026-09-08T10:01:00Z"}]})
}

#[test]
fn problem_context_selection_preserves_unclassified_legacy_inputs_and_excludes_other_versions() {
    let mut state = input_state();
    let legacy = framing_sources(&state, &Value::Null).unwrap();
    assert_eq!(legacy[0]["material_role"], "unclassified_input");
    state["artifacts"][0]["problem_version"] = json!(1);
    state["artifacts"][0]["material_role"] = json!("unclassified_input");
    assert_eq!(
        framing_sources(&state, &Value::Null).unwrap()[0]["id"],
        "input"
    );
    push(
        &mut state,
        "artifacts",
        json!({"id":"unrelated","source_type":"user_import","problem_version":2,"material_role":"problem_statement"}),
    );
    assert_eq!(manifest(&state).len(), 1);
    assert!(
        framing_sources(
            &state,
            &json!({"problem_source_artifact_ids":["unrelated"]})
        )
        .is_err()
    );
    state["problem_version"] = json!(2);
    assert_eq!(manifest(&state)[0]["id"], "unrelated");
    assert!(framing_sources(&state, &json!({"problem_source_artifact_ids":["input"]})).is_err());
    state["artifacts"][0]
        .as_object_mut()
        .unwrap()
        .remove("problem_version");
    assert_eq!(
        manifest(&state).len(),
        1,
        "legacy imports cannot cross a replaced problem"
    );
}

#[test]
fn problem_context_multiple_inputs_require_explicit_selection_not_filename_guessing() {
    let mut state = input_state();
    push(
        &mut state,
        "artifacts",
        json!({"id":"reference","source_type":"user_import","problem_version":1,"material_role":"reference"}),
    );
    assert!(framing_sources(&state, &Value::Null).is_err());
    assert_eq!(snapshot(&state)["source_selection_pending"], true);
    assert_eq!(
        framing_sources(&state, &json!({"problem_source_artifact_ids":["input"]}))
            .unwrap()
            .len(),
        1
    );
    assert!(
        framing_sources(&state, &json!({"problem_source_artifact_ids":[]}))
            .unwrap()
            .is_empty()
    );
    state["artifacts"][0]["material_role"] = json!("problem_statement");
    assert_eq!(
        framing_sources(&state, &Value::Null).unwrap()[0]["id"],
        "input"
    );
}

#[test]
fn problem_context_does_not_turn_input_materials_into_read_or_verified_premises() {
    let mut state = input_state();
    push(&mut state, "sessions", json!({"id":"main"}));
    push(
        &mut state,
        "facts",
        json!({"id":"lemma","claim":"A nontrivial lemma","problem_version":1,"validity":"current","revision":1,"assurance":"model_reviewed"}),
    );
    let target = snapshot(&state);
    assert_eq!(target["assurance"], "unreviewed_target_input");
    assert!(array(&state, "evidence_reads").is_empty());
    let checked = evidence22::submission_premises(&mut state, &json!({"dependency_ids":["lemma"],"declared_premises":[{"kind":"fact","ref_id":"lemma","usage_location":"proof step 2"}]}), "main").unwrap();
    assert_eq!(
        checked["ready"], false,
        "automatic target context must not bypass the premise read gate"
    );
    assert_eq!(array(&state, "facts").len(), 1);
}

#[tokio::test]
async fn problem_context_frame_and_submit_same_output_freezes_sources_and_old_root_refs() {
    let temp = tempfile::tempdir().unwrap();
    let store = V2Store::connect(&temp.path().join("audit.sqlite"), &temp.path().join("data"))
        .await
        .unwrap();
    let project = store
        .create_project(json!({"title":"frozen target","problem":SHORT}), "create")
        .await
        .unwrap();
    let project_id = project["id"].as_str().unwrap();
    let source = store
        .put_artifact(project_id, "problem.md", EXACT.as_bytes(), "text/plain")
        .await
        .unwrap();
    let run = json!({"id":"run","state":"running","mode":"delegated","problem_version":1,"control_epoch":1,"created_at":Utc::now().to_rfc3339(),"deadline_at":(Utc::now()+chrono::Duration::seconds(60)).to_rfc3339()});
    let author = store
        .mutate(project_id, "fixture.seed", None, |state| {
            let input =
                super::super::entity_mut(state, "artifacts", source["id"].as_str().unwrap())?;
            input["source_type"] = json!("user_import");
            input["material_role"] = json!("unclassified_input");
            input["problem_version"] = json!(1);
            push(state, "runs", run.clone());
            let author = new_session(state, &run, "main", None, &None);
            push(state, "sessions", author.clone());
            Ok(author)
        })
        .await
        .unwrap();
    let before = store.read(project_id).await.unwrap();
    let root = array(&before, "nodes")
        .iter()
        .find(|node| node["node_type"] == "problem")
        .unwrap();
    let old_ref =
        json!({"kind":"node","id":root["id"],"revision":root["revision"],"problem_version":1});
    let control = json!({"actions":[
        {"type":"frame_problem","expected_revision":1,"math_statement":EXACT,"research_description":""},
        {"type":"submit_candidate","claim":EXACT,"proof":"Fixture proof; no mathematical validity asserted by this regression.","covers_goal":true,"goal_refs":[{"ref":old_ref,"relation":"covers"}],"declared_premises":[]}
    ]});
    let output = store
        .put_artifact(
            project_id,
            "output.json",
            control.to_string().as_bytes(),
            "application/json",
        )
        .await
        .unwrap();
    let service = V2Service::new(store.clone(), V2Config::default());
    service
        .apply_actions(
            project_id,
            "run",
            author["id"].as_str().unwrap(),
            1,
            &control,
            &output,
            temp.path(),
        )
        .await
        .unwrap();
    let state = store.read(project_id).await.unwrap();
    let candidate = &state["candidates"][0];
    assert!(array(candidate, "source_artifact_ids").is_empty());
    assert_eq!(
        candidate["problem_context"]["problem_spec"]["math_statement"],
        EXACT
    );
    assert_eq!(
        candidate["problem_context"]["source_refs"][0]["sha256"],
        source["sha256"]
    );
    let root = array(&state, "nodes")
        .iter()
        .find(|node| node["node_type"] == "problem")
        .unwrap();
    assert_eq!(root["exact_statement"], EXACT);
    assert_eq!(root["revision"], 2);
    let old = interactions::selection23(&state, &old_ref).unwrap();
    assert!(
        old.to_string().contains(SHORT),
        "old root remains readable: {old}"
    );
    assert!(array(&state, "facts").is_empty());
    assert!(array(&state, "evidence_reads").is_empty());
    let reviewer = json!({"candidate_snapshot":candidate});
    let mut packet = enhancements::proof_packet(&store, project_id, &reviewer)
        .await
        .unwrap();
    evidence22::augment_packet(&state, &reviewer, &mut packet);
    assert_eq!(packet["problem_sources"][0]["text"], EXACT);
    assert_eq!(packet["sources"][0]["assurance"], "unreviewed_input");
    assert!(
        array(&packet, "unresolved_materials").is_empty(),
        "{packet}"
    );
    assert!(array(&packet, "dependencies").is_empty());
    let target_before = packet["problem_context"].clone();
    let sources_before = packet["problem_sources"].clone();
    store.mutate(project_id,"fixture.new_problem",None,|state| {
        state["problem"]=json!("A later unrelated question"); state["problem_version"]=json!(2);
        problem24::apply_replacement(state,&Value::Null);
        push(state,"artifacts",json!({"id":"later","source_type":"user_import","problem_version":2,"material_role":"problem_statement","sha256":"later"}));
        Ok(json!({}))
    }).await.unwrap();
    let later = store.read(project_id).await.unwrap();
    let frozen = enhancements::proof_packet(&store, project_id, &reviewer)
        .await
        .unwrap();
    assert_eq!(frozen["problem_context"], target_before);
    assert_eq!(frozen["problem_sources"], sources_before);
    assert_eq!(frozen["problem"], SHORT);
    assert!(interactions::selection23(&later, &old_ref).is_ok());
    assert!(
        proof_tree23::candidate_metadata(
            &later,
            &json!({"claim":"new","goal_refs":[{"ref":old_ref,"relation":"covers"}]}),
            author["id"].as_str().unwrap()
        )
        .is_err(),
        "real problem-version changes remain strict"
    );
}
