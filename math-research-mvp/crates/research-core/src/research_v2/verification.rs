//! Strict Rethlas-derived report contract. Natural-language review is never formal proof.
use super::{V2Result, array, entity, entity_mut, queue_review, remaining, revision};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Finding {
    location: String,
    issue: String,
}
#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum MaterialIssue {
    Text(String),
    Located(Finding),
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RepairCheck {
    id: String,
    resolved: bool,
    explanation: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Report {
    summary: String,
    critical_errors: Vec<Finding>,
    gaps: Vec<Finding>,
    checked_items: Vec<String>,
    unresolved_materials: Vec<MaterialIssue>,
    checked_dependency_ids: Vec<String>,
    repair_checks: Vec<RepairCheck>,
    premise_audit_status: String,
    checked_premise_ids: Vec<String>,
    undeclared_premises: Vec<Finding>,
    applicability_gaps: Vec<Finding>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    snapshot_hash: String,
    verdict: String,
    repair_hints: String,
    claim_coverage: bool,
    goal_coverage: bool,
    verification_report: Report,
}

pub(super) fn schema_example(hash: &Value) -> Value {
    json!({"verification":{"snapshot_hash":hash,"verdict":"correct|wrong|inconclusive","repair_hints":"","claim_coverage":false,"goal_coverage":false,"verification_report":{"summary":"","critical_errors":[],"gaps":[],"checked_items":["location and actual checked inference"],"unresolved_materials":[],"checked_dependency_ids":[],"repair_checks":[{"id":"prior issue id when supplied; otherwise omit this entry","resolved":false,"explanation":""}],"premise_audit_status":"complete|incomplete","checked_premise_ids":[],"undeclared_premises":[],"applicability_gaps":[]}}})
}

pub(super) fn normalize(control: &Value, packet: &Value) -> Value {
    match validate(control, packet) {
        Ok(value) => value,
        Err(message) => {
            json!({"review":{"verdict":"inconclusive","issues":[{"location":"report contract","issue":message}],"goal_coverage":false,"engine":"rethlas-adapted/2.2.0","report_validated":false},"raw_verification":control.get("verification").cloned().unwrap_or(Value::Null)})
        }
    }
}

/// A positive report with a contract failure may receive one bounded correction.
/// Mathematical objections must go through ordinary proof repair instead.
pub(super) fn queue_contract_repair(
    state: &mut Value,
    reviewer_session_id: &str,
) -> V2Result<bool> {
    let Some(session) = entity(state, "sessions", reviewer_session_id).cloned() else {
        return Ok(false);
    };
    let review_id = session["review_id"].as_str().unwrap_or_default();
    let Some(review) = entity(state, "reviews", review_id).cloned() else {
        return Ok(false);
    };
    if review["state"] != "completed" || review["report_validated"] != false {
        return Ok(false);
    }
    let candidate_id = review["candidate_id"].as_str().unwrap_or_default();
    let Some(candidate) = entity(state, "candidates", candidate_id).cloned() else {
        return Ok(false);
    };
    let original_raw = &review["raw_verification"];
    let report = &original_raw["verification_report"];
    let only_positive_findings = original_raw["verdict"] == "correct"
        && original_raw["snapshot_hash"] == candidate["snapshot_hash"]
        && original_raw["claim_coverage"] == true
        && report["premise_audit_status"] == "complete"
        && [
            "critical_errors",
            "gaps",
            "undeclared_premises",
            "applicability_gaps",
            "unresolved_materials",
        ]
        .iter()
        .all(|field| report[field].as_array().is_some_and(Vec::is_empty))
        && report["repair_checks"]
            .as_array()
            .is_some_and(|checks| checks.iter().all(|check| check["resolved"] == true));
    if !only_positive_findings {
        entity_mut(state, "reviews", review_id)?["report_repair_status"] =
            json!("requires_research_repair_or_material_check");
        return Ok(false);
    }
    let run_id = candidate["run_id"].as_str().unwrap_or_default();
    let Some(run) = entity(state, "runs", run_id).cloned() else {
        return Ok(false);
    };
    let prior = array(state, "reviews").iter().find(|r| {
        r["candidate_id"] == candidate["id"]
            && r["snapshot_hash"] == candidate["snapshot_hash"]
            && r["trigger"] == "report_contract_repair"
    });
    if let Some(prior) = prior {
        return Ok(matches!(
            prior["state"].as_str(),
            Some("queued" | "running")
        ));
    }
    let unavailable = if !matches!(run["state"].as_str(), Some("running" | "waiting_human"))
        || run["outstanding_cancellation"] == true
        || remaining(&run) == 0
    {
        Some("run_not_available")
    } else if candidate["problem_version"] != state["problem_version"]
        || candidate["snapshot_hash"] != review["snapshot_hash"]
        || candidate["route_id"]
            .as_str()
            .is_some_and(|route| super::lab::route_blocked(state, route))
    {
        Some("candidate_not_current")
    } else if run["limits"]["max_invocations"]
        .as_u64()
        .is_some_and(|limit| {
            u64::try_from(
                array(state, "usage")
                    .iter()
                    .filter(|u| u["run_id"] == run_id && u["state"] != "not_dispatched")
                    .count(),
            )
            .unwrap_or(u64::MAX)
                >= limit
        })
    {
        Some("invocation_budget_unavailable")
    } else {
        None
    };
    if let Some(reason) = unavailable {
        entity_mut(state, "reviews", review_id)?["report_repair_status"] = json!(reason);
        return Ok(false);
    }
    queue_review(state, &candidate, "report_contract_repair")?;
    let queued = array(state, "reviews")
        .iter()
        .find(|r| r["candidate_id"] == candidate["id"] && r["trigger"] == "report_contract_repair")
        .cloned();
    let Some(queued) = queued else {
        return Ok(false);
    };
    let repair_session_id = queued["reviewer_session_id"].as_str().unwrap_or_default();
    entity_mut(state, "sessions", repair_session_id)?["report_repair"] = json!({
        "original_review_id":review_id,
        "original_report_artifact_id":review["report_artifact_id"],
        "original_raw_verification":original_raw,
        "contract_issues":review["issues"],
        "candidate_id":candidate["id"],
        "snapshot_hash":candidate["snapshot_hash"],
        "attempt":1,
        "max_automatic_attempts":1,
        "instruction":"此前正面意见未通过报告契约，原始报告完整附后。重新核对同一冻结候选并输出合规报告；不得改写命题、证明、依赖或掩盖数学缺口。仅当全部检查确实通过才能 correct；correct 时 repair_hints 必须为空，范围说明放 summary。若发现实际数学缺口，应如实给出负面或不确定意见，不能为修复格式而改成 correct。"
    });
    let original = entity_mut(state, "reviews", review_id)?;
    original["report_repair_status"] = json!("queued");
    original["report_repair_review_id"] = queued["id"].clone();
    revision(original);
    Ok(true)
}

fn validate(control: &Value, packet: &Value) -> Result<Value, String> {
    let e: Envelope = serde_json::from_value(control["verification"].clone())
        .map_err(|e| format!("Missing or invalid verification schema: {e}"))?;
    let r = &e.verification_report;
    if packet["snapshot_hash"] != e.snapshot_hash || e.snapshot_hash.is_empty() {
        return Err("Frozen snapshot mismatch".into());
    }
    if r.summary.trim().is_empty()
        || r.unresolved_materials.iter().any(|m| match m {
            MaterialIssue::Text(s) => s.trim().is_empty(),
            MaterialIssue::Located(f) => f.location.trim().is_empty() || f.issue.trim().is_empty(),
        })
        || r.critical_errors
            .iter()
            .chain(&r.gaps)
            .chain(&r.undeclared_premises)
            .chain(&r.applicability_gaps)
            .any(|f| f.location.trim().is_empty() || f.issue.trim().is_empty())
    {
        return Err("Missing summary or finding location/issue".into());
    }
    if !["correct", "wrong", "inconclusive"].contains(&e.verdict.as_str()) {
        return Err("Unknown verdict".into());
    }
    if !["complete", "incomplete"].contains(&r.premise_audit_status.as_str()) {
        return Err("Premise audit must explicitly be complete or incomplete".into());
    }
    let findings = r.critical_errors.len()
        + r.gaps.len()
        + r.undeclared_premises.len()
        + r.applicability_gaps.len();
    if e.verdict == "wrong" && (findings == 0 || e.repair_hints.trim().is_empty()) {
        return Err("Wrong verdict requires findings and repair hints".into());
    }
    let missing =
        !r.unresolved_materials.is_empty() || !array(packet, "unresolved_materials").is_empty();
    let dependency_ids: Vec<_> = array(packet, "dependencies")
        .iter()
        .filter_map(|d| d["id"].as_str())
        .collect();
    let dependencies_checked = dependency_ids
        .iter()
        .all(|id| r.checked_dependency_ids.iter().any(|v| v == id));
    let premises_checked = array(packet, "declared_premises").iter().all(|premise| {
        premise["id"]
            .as_str()
            .is_some_and(|id| r.checked_premise_ids.iter().any(|checked| checked == id))
    });
    let repairs_checked = array(packet, "repair_obligations").iter().all(|issue| {
        r.repair_checks
            .iter()
            .any(|c| issue["id"] == c.id && c.resolved && !c.explanation.trim().is_empty())
    });
    if e.verdict == "correct"
        && (findings > 0
            || !e.repair_hints.is_empty()
            || !e.claim_coverage
            || r.checked_items.is_empty()
            || r.checked_items.iter().any(|s| s.trim().is_empty())
            || missing
            || !dependencies_checked
            || !premises_checked
            || r.premise_audit_status != "complete"
            || !repairs_checked
            || r.repair_checks.iter().any(|c| !c.resolved))
    {
        return Err("Acceptance requires complete claim/step/dependency/repair/premise coverage, no undeclared premises or applicability gaps, and no unresolved material".into());
    }
    if e.goal_coverage && (!e.claim_coverage || e.verdict != "correct") {
        return Err("Goal coverage cannot bypass claim verification".into());
    }
    let verdict = if e.verdict == "correct" {
        "accepted"
    } else if e.verdict == "wrong" && !r.critical_errors.is_empty() {
        "rejected"
    } else if e.verdict == "wrong" {
        "changes_requested"
    } else {
        "inconclusive"
    };
    let mut issues: Vec<Value> = r
        .critical_errors
        .iter()
        .chain(&r.gaps)
        .chain(&r.undeclared_premises)
        .chain(&r.applicability_gaps)
        .map(|f| json!({"location":f.location,"issue":f.issue}))
        .collect();
    issues.extend(r.unresolved_materials.iter().map(|s| match s {
        MaterialIssue::Text(s) => json!({"location":"materials","issue":s}),
        MaterialIssue::Located(f) => json!({"location":f.location,"issue":f.issue}),
    }));
    if verdict == "inconclusive" && issues.is_empty() {
        issues.push(json!({"location":"review","issue":r.summary}));
    }
    Ok(
        json!({"review":{"verdict":verdict,"issues":issues,"goal_coverage":e.goal_coverage,"engine":"rethlas-adapted/2.2.0","report_validated":true,"premise_audit_status":r.premise_audit_status,"checked_premise_ids":r.checked_premise_ids,"undeclared_premises":r.undeclared_premises,"applicability_gaps":r.applicability_gaps},"raw_verification":serde_json::to_value(&e).map_err(|e|e.to_string())?}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn report_repair_fixture() -> Value {
        let mut state = json!({"id":"p","workspace_path":"work","problem_version":1,
            "runs":[{"id":"run","state":"running","problem_version":1,"control_epoch":1,"deadline_at":null,"limits":{"duration_seconds":null,"max_invocations":5}}],
            "sessions":[{"id":"author","role":"main","run_id":"run"},{"id":"reviewer","role":"reviewer","review_id":"review","run_id":"run"}],
            "reviews":[{"id":"review","candidate_id":"candidate","snapshot_hash":"hash","reviewer_session_id":"reviewer","state":"completed","verdict":"inconclusive","report_validated":false,"revision":1,"report_artifact_id":"raw-report","issues":[{"location":"report contract","issue":"correct requires empty repair hints"}]}],
            "candidates":[{"id":"candidate","run_id":"run","author_session_id":"author","problem_version":1,"snapshot_hash":"hash","proof_artifact_id":"frozen-proof","claim":"exact statement","status":"changes_requested","covers_goal":true}],
            "usage":[{"id":"u","run_id":"run","state":"succeeded"}],"facts":[],"routes":[]});
        let mut raw = good()["verification"].clone();
        raw["snapshot_hash"] = json!("hash");
        raw["repair_hints"] = json!("Scope explanation belongs in summary.");
        state["reviews"][0]["raw_verification"] = raw;
        state
    }
    #[test]
    fn contract_repair_is_once_for_the_same_frozen_proof_and_keeps_original_receipt() {
        let mut state = report_repair_fixture();
        let candidate = state["candidates"][0].clone();
        assert!(queue_contract_repair(&mut state, "reviewer").unwrap());
        assert_eq!(state["reviews"].as_array().unwrap().len(), 2);
        assert_eq!(state["reviews"][0]["report_validated"], false);
        assert_eq!(state["reviews"][0]["report_artifact_id"], "raw-report");
        assert_eq!(state["candidates"][0], candidate);
        assert!(state["facts"].as_array().unwrap().is_empty());
        let next = state["reviews"][1]["reviewer_session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            entity(&state, "sessions", &next).unwrap()["candidate_snapshot"],
            candidate
        );
        assert_eq!(
            entity(&state, "sessions", &next).unwrap()["report_repair"]["original_raw_verification"],
            state["reviews"][0]["raw_verification"]
        );
        assert!(queue_contract_repair(&mut state, "reviewer").unwrap());
        assert_eq!(state["reviews"].as_array().unwrap().len(), 2);
        state["reviews"][1]["state"] = json!("completed");
        state["reviews"][1]["report_validated"] = json!(false);
        assert!(!queue_contract_repair(&mut state, &next).unwrap());
        assert_eq!(state["reviews"].as_array().unwrap().len(), 2);
    }
    #[test]
    fn contract_repair_respects_budget_pause_unknown_version_and_route_ban() {
        for condition in [
            "budget",
            "paused",
            "ended",
            "unknown",
            "version",
            "route",
            "valid_report",
        ] {
            let mut state = report_repair_fixture();
            match condition {
                "budget" => state["runs"][0]["limits"]["max_invocations"] = json!(1),
                "paused" | "ended" => state["runs"][0]["state"] = json!(condition),
                "unknown" => state["runs"][0]["outstanding_cancellation"] = json!(true),
                "version" => state["problem_version"] = json!(2),
                "route" => {
                    state["candidates"][0]["route_id"] = json!("child");
                    state["routes"] = json!([{"id":"parent","status":"prohibited"},{"id":"child","status":"active","parent_route_ids":["parent"]}]);
                }
                "valid_report" => state["reviews"][0]["report_validated"] = json!(true),
                _ => unreachable!(),
            }
            assert!(
                !queue_contract_repair(&mut state, "reviewer").unwrap(),
                "{condition}"
            );
            assert_eq!(state["reviews"].as_array().unwrap().len(), 1, "{condition}");
        }
    }
    #[test]
    fn contract_repair_never_discards_negative_or_missing_mathematical_findings() {
        for field in [
            "critical_errors",
            "gaps",
            "undeclared_premises",
            "applicability_gaps",
            "unresolved_materials",
            "missing_findings",
            "negative_verdict",
            "unresolved_repair",
        ] {
            let mut state = report_repair_fixture();
            let raw = &mut state["reviews"][0]["raw_verification"];
            match field {
                "missing_findings" => {
                    raw["verification_report"]
                        .as_object_mut()
                        .unwrap()
                        .remove("gaps");
                }
                "negative_verdict" => raw["verdict"] = json!("wrong"),
                "unresolved_repair" => {
                    raw["verification_report"]["repair_checks"] =
                        json!([{"id":"old-gap","resolved":false,"explanation":"not fixed"}]);
                }
                _ => {
                    raw["verification_report"][field] =
                        json!([{"location":"L2","issue":"requires mathematical work"}]);
                }
            }
            let original = raw.clone();
            assert!(
                !queue_contract_repair(&mut state, "reviewer").unwrap(),
                "{field}"
            );
            assert_eq!(state["reviews"][0]["raw_verification"], original);
            assert_eq!(array(&state, "reviews").len(), 1);
            assert!(array(&state, "facts").is_empty());
        }
    }
    pub(super) fn good() -> Value {
        json!({"verification":{"snapshot_hash":"h","verdict":"correct","repair_hints":"","claim_coverage":true,"goal_coverage":true,"verification_report":{"summary":"All deductions checked","critical_errors":[],"gaps":[],"checked_items":["paragraph 1"],"unresolved_materials":[],"checked_dependency_ids":[],"repair_checks":[],"premise_audit_status":"complete","checked_premise_ids":[],"undeclared_premises":[],"applicability_gaps":[]}}})
    }
    #[test]
    fn strict_gate_rejects_missing_conflicting_and_incomplete_reports() {
        let packet = json!({"snapshot_hash":"h","dependencies":[],"repair_obligations":[],"unresolved_materials":[]});
        assert_eq!(normalize(&good(), &packet)["review"]["verdict"], "accepted");
        for bad in [
            json!({"review":{"verdict":"accepted","issues":[],"goal_coverage":true}}),
            json!({"verification":{"verdict":"correct"}}),
        ] {
            assert_eq!(
                normalize(&bad, &packet)["review"]["verdict"],
                "inconclusive"
            );
        }
        for field in ["summary", "checked_items", "critical_errors"] {
            let mut bad = good();
            bad["verification"]["verification_report"][field] = match field {
                "summary" => json!(""),
                "checked_items" => json!([]),
                _ => json!([{"location":"L1","issue":"gap"}]),
            };
            assert_eq!(
                normalize(&bad, &packet)["review"]["verdict"],
                "inconclusive"
            );
        }
        for extra in [
            json!({"dependencies":[{"id":"f1"}]}),
            json!({"repair_obligations":[{"id":"r:0"}]}),
            json!({"unresolved_materials":["missing source"]}),
            json!({"snapshot_hash":"other"}),
        ] {
            let mut p = packet.clone();
            for (k, v) in extra.as_object().unwrap() {
                p[k] = v.clone();
            }
            assert_eq!(normalize(&good(), &p)["review"]["verdict"], "inconclusive");
        }
    }
    #[test]
    fn located_material_warning_does_not_hide_a_proven_error_or_allow_acceptance() {
        let p = json!({"snapshot_hash":"h"});
        let mut v = good();
        v["verification"]["verification_report"]["unresolved_materials"] = json!([{"location":"Heine-Cantor citation","issue":"Original source not supplied; network disallowed"}]);
        assert_eq!(normalize(&v, &p)["review"]["verdict"], "inconclusive");
        v["verification"]["verdict"] = json!("wrong");
        v["verification"]["goal_coverage"] = json!(false);
        v["verification"]["repair_hints"] = json!(
            "The claim is false; restrict to a compact domain if changing the goal is authorized."
        );
        v["verification"]["verification_report"]["critical_errors"] = json!([{"location":"L2","issue":"R is closed but not compact; the asserted implication is false."}]);
        let result = normalize(&v, &p);
        assert_eq!(result["review"]["verdict"], "rejected");
        assert_eq!(result["review"]["report_validated"], true);
        assert_eq!(
            result["review"]["issues"][1]["location"],
            "Heine-Cantor citation"
        );
        v["verification"]["verification_report"]["unresolved_materials"][0]["invented"] =
            json!(true);
        assert_eq!(normalize(&v, &p)["review"]["report_validated"], false);
    }
    #[test]
    fn gaps_and_errors_remain_negative_and_sources_can_be_inconclusive() {
        let p = json!({"snapshot_hash":"h"});
        let mut v = good();
        v["verification"]["verdict"] = json!("wrong");
        v["verification"]["goal_coverage"] = json!(false);
        v["verification"]["repair_hints"] = json!("Prove compactness");
        v["verification"]["verification_report"]["gaps"] =
            json!([{"location":"L2","issue":"compactness missing"}]);
        assert_eq!(normalize(&v, &p)["review"]["verdict"], "changes_requested");
        v["verification"]["verdict"] = json!("inconclusive");
        v["verification"]["verification_report"]["gaps"] = json!([]);
        v["verification"]["verification_report"]["unresolved_materials"] =
            json!(["source unavailable"]);
        assert_eq!(normalize(&v, &p)["review"]["verdict"], "inconclusive");
    }

    #[test]
    fn missing_premise_audit_or_unchecked_premises_never_silently_pass() {
        let packet = json!({"snapshot_hash":"h","declared_premises":[{"id":"f1","kind":"fact"}]});
        assert_eq!(
            normalize(&good(), &packet)["review"]["verdict"],
            "inconclusive"
        );
        let mut checked = good();
        checked["verification"]["verification_report"]["checked_premise_ids"] = json!(["f1"]);
        assert_eq!(
            normalize(&checked, &packet)["review"]["verdict"],
            "accepted"
        );
        for field in [
            "premise_audit_status",
            "checked_premise_ids",
            "undeclared_premises",
            "applicability_gaps",
        ] {
            let mut omitted = checked.clone();
            omitted["verification"]["verification_report"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert_eq!(
                normalize(&omitted, &packet)["review"]["report_validated"],
                false
            );
        }
        checked["verification"]["verification_report"]["undeclared_premises"] =
            json!([{"location":"step 2","issue":"Uses compactness without declaring it"}]);
        assert_eq!(
            normalize(&checked, &packet)["review"]["verdict"],
            "inconclusive"
        );
        checked["verification"]["verdict"] = json!("wrong");
        checked["verification"]["goal_coverage"] = json!(false);
        checked["verification"]["repair_hints"] =
            json!("Establish compactness or remove that inference");
        assert_eq!(
            normalize(&checked, &packet)["review"]["verdict"],
            "changes_requested"
        );
    }
}
