//! Frozen target inputs. These identify the question; they never certify a premise.
use super::{V2Result, Value, array, entity, err, json, problem24};

fn current_import(state: &Value, artifact: &Value) -> bool {
    if artifact["source_type"] != "user_import" {
        return false;
    }
    if !artifact["problem_version"].is_null() {
        return artifact["problem_version"] == state["problem_version"];
    }
    // Older imports had no version. Only the first problem's startup inputs
    // may use this compatibility path; never carry them across a changed goal.
    state["problem_version"] == 1
        && array(state, "runs")
            .iter()
            .filter(|run| run["problem_version"] == 1)
            .filter_map(|run| run["created_at"].as_str())
            .filter_map(|time| chrono::DateTime::parse_from_rfc3339(time).ok())
            .min()
            .is_none_or(|start| {
                artifact["created_at"]
                    .as_str()
                    .and_then(|time| chrono::DateTime::parse_from_rfc3339(time).ok())
                    .is_some_and(|created| created <= start)
            })
}

fn input_ref(artifact: &Value, version: &Value) -> Value {
    json!({"id":artifact["id"],"artifact_id":artifact["id"],"sha256":artifact["sha256"],
        "name":artifact["name"],"problem_version":version,"imported_problem_version":artifact["problem_version"],
        "material_role":artifact.get("material_role").cloned().unwrap_or_else(||json!("unclassified_input")),
        "provenance":artifact["provenance"],"assurance":"unreviewed_input",
        "use":"target_identification_and_assumptions_only_not_a_verified_lemma"})
}

pub(super) fn manifest(state: &Value) -> Vec<Value> {
    array(state, "artifacts")
        .iter()
        .filter(|artifact| current_import(state, artifact))
        .map(|artifact| input_ref(artifact, &state["problem_version"]))
        .collect()
}

pub(super) fn framing_sources(state: &Value, action: &Value) -> V2Result<Vec<Value>> {
    if let Some(selected) = action.get("problem_source_artifact_ids") {
        let selected = selected
            .as_array()
            .filter(|items| items.len() <= 64)
            .ok_or_else(|| {
                err(
                    "INVALID_PROBLEM_SOURCE",
                    "problem_source_artifact_ids 必须是最多64项的已有材料ID数组",
                )
            })?;
        let mut result = Vec::new();
        for source in selected {
            let artifact = source
                .as_str()
                .and_then(|key| entity(state, "artifacts", key))
                .filter(|artifact| current_import(state, artifact))
                .ok_or_else(|| {
                    err(
                        "INVALID_PROBLEM_SOURCE",
                        "题面来源必须属于当前题目版本的已导入材料",
                    )
                })?;
            if !result
                .iter()
                .any(|reference: &Value| reference["id"] == artifact["id"])
            {
                result.push(input_ref(artifact, &state["problem_version"]));
            }
        }
        return Ok(result);
    }
    let inputs = manifest(state);
    let marked: Vec<Value> = inputs
        .iter()
        .filter(|item| item["material_role"] == "problem_statement")
        .cloned()
        .collect();
    if !marked.is_empty() {
        return Ok(marked);
    }
    if inputs.len() == 1 && inputs[0]["material_role"] == "unclassified_input" {
        return Ok(inputs);
    }
    if inputs
        .iter()
        .any(|item| item["material_role"] == "unclassified_input")
    {
        return Err(err(
            "PROBLEM_SOURCE_SELECTION_REQUIRED",
            "有多份未分类输入。请阅读当前题目材料清单，在 frame_problem 中明确填写 problem_source_artifact_ids；题面完全来自原始消息时显式填写 []。",
        ));
    }
    Ok(Vec::new())
}

pub(super) fn snapshot(state: &Value) -> Value {
    let spec = problem24::spec(state);
    let (sources, selection_pending) = if let Some(sources) = spec["problem_source_refs"].as_array()
    {
        (sources.clone(), false)
    } else {
        match framing_sources(state, &Value::Null) {
            Ok(sources) => (sources, false),
            Err(_) => (Vec::new(), true),
        }
    };
    json!({"schema_version":1,"problem_version":state["problem_version"],
        "original_input":spec["original_input"],"original_problem":state["problem"],
        "problem_spec":spec,"source_refs":sources,"source_selection_pending":selection_pending,
        "assurance":"unreviewed_target_input",
        "instruction":"Compare the normalized mathematical statement with the original input and frozen source texts, including all assumptions, quantifiers and scope. Normalization is not independently verified. These inputs identify the target; they are not verified lemmas and do not replace declared premises or exact premise-read requirements."})
}

pub(super) fn for_candidate(state: &Value, candidate: &Value) -> Value {
    if candidate["problem_context"].is_object() {
        return candidate["problem_context"].clone();
    }
    // A pre-upgrade candidate has no captured normalized target. Do not bind
    // its old proof to a later normalization or a later imported attachment.
    let original = array(state, "problem_versions")
        .iter()
        .find(|version| version["version"] == candidate["problem_version"])
        .map_or_else(
            || {
                if state["problem_version"] == candidate["problem_version"] {
                    state["problem"].clone()
                } else {
                    Value::Null
                }
            },
            |version| version["problem"].clone(),
        );
    json!({"schema_version":1,"problem_version":candidate["problem_version"],
        "original_input":original,"original_problem":original,
        "problem_spec":{"math_statement":original,"normalization_state":"legacy_not_captured"},
        "source_refs":[],"capture_kind":"legacy_original_target_only","assurance":"unreviewed_target_input",
        "instruction":"Legacy candidate: only its original problem version was retained. Do not substitute newer normalization or attachments. Missing target identification evidence must remain unresolved; imported inputs are not verified lemmas."})
}

#[cfg(test)]
#[path = "problem_context_tests.rs"]
mod tests;
