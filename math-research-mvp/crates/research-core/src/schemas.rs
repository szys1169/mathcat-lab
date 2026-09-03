use serde_json::{Value, json};

fn string_array() -> Value {
    json!({"type":"array","items":{"type":"string"}})
}
fn attributes() -> Value {
    json!({
        "type":"object",
        "additionalProperties":false,
        "required":["entries"],
        "properties":{
            "entries":{"type":"array","items":{
                "type":"object","additionalProperties":false,"required":["name","value"],
                "properties":{"name":{"type":"string"},"value":{"type":"string"}}
            }}
        }
    })
}

#[must_use]
pub fn planner_schema() -> Value {
    json!({
        "type":"object","additionalProperties":false,
        "required":["rationale_summary","routes","assignments","targeted_uncertainty_ids","suggestion_decisions"],
        "properties":{
            "rationale_summary":{"type":"string"},
            "routes":{"type":"array","minItems":2,"items":{"type":"object","additionalProperties":false,
                "required":["title","method_summary","target_goal_ids","required_fact_ids","expected_subgoals","expected_goal_progress","uncertainty_reduction","human_suggestion_alignment","evidence_support","route_diversity","verifiability","novelty","failure_similarity_penalty","cost_penalty","risks"],
                "properties":{
                    "title":{"type":"string"},"method_summary":{"type":"string"},"target_goal_ids":string_array(),"required_fact_ids":string_array(),"expected_subgoals":string_array(),
                    "expected_goal_progress":{"type":"number","minimum":0,"maximum":1},"uncertainty_reduction":{"type":"number","minimum":0,"maximum":1},
                    "human_suggestion_alignment":{"type":"number","minimum":0,"maximum":1},"evidence_support":{"type":"number","minimum":0,"maximum":1},
                    "route_diversity":{"type":"number","minimum":0,"maximum":1},"verifiability":{"type":"number","minimum":0,"maximum":1},"novelty":{"type":"number","minimum":0,"maximum":1},
                    "failure_similarity_penalty":{"type":"number","minimum":0,"maximum":1},"cost_penalty":{"type":"number","minimum":0,"maximum":1},"risks":string_array()
                }}},
            "assignments":{"type":"array","minItems":1,"items":{"type":"object","additionalProperties":false,
                "required":["route_index","worker_role","strategic_role","addresses_interface_debt","goal_ids","objective","completion_contract","priority"],
                "properties":{"route_index":{"type":"integer","minimum":0},"worker_role":{"type":"string","enum":["prover","explorer","counterexample_hunter","literature_researcher"]},"strategic_role":{"type":"string","enum":["whole_architecture","central_bridge","adversarial","literature","computation","local_milestone"]},"addresses_interface_debt":{"type":"boolean"},"goal_ids":string_array(),"objective":{"type":"string"},"completion_contract":{"type":"string"},"priority":{"type":"number","minimum":0,"maximum":1}}}},
            "targeted_uncertainty_ids":string_array(),"suggestion_decisions":string_array()
        }
    })
}

#[must_use]
pub fn worker_schema() -> Value {
    json!({
        "type":"object","additionalProperties":false,"required":["summary","discoveries","candidates","failures","uncertainties","sources","experiments"],
        "properties":{
            "summary":{"type":"string"},
            "discoveries":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["kind","statement","attributes"],"properties":{"kind":{"type":"string"},"statement":{"type":"string"},"attributes":attributes()}}},
            "candidates":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["statement","assumptions","proof_markdown","dependency_fact_ids","definitions_introduced","external_source_ids","candidate_type","target_goal_ids"],"properties":{
                "statement":{"type":"string"},"assumptions":string_array(),"proof_markdown":{"type":"string"},"dependency_fact_ids":string_array(),"definitions_introduced":attributes(),"external_source_ids":string_array(),"candidate_type":{"type":"string","enum":["theorem","lemma","proposition","counterexample","observation"]},"target_goal_ids":string_array()
            }}},
            "failures":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["failure_type","summary","repairable"],"properties":{"failure_type":{"type":"string"},"summary":{"type":"string"},"repairable":{"type":"boolean"}}}},
            "uncertainties":string_array(),
            "sources":{"type":"array","items":{"type":"object","additionalProperties":false,
                "required":["title","authors","url","citation_key","theorem_reference","statement_excerpt","assumptions","applicability","status","retrieval_query","document_version","fulltext_path","fulltext_sha256","fulltext_artifact_id"],
                "properties":{
                    "title":{"type":"string"},"authors":string_array(),"url":{"type":["string","null"]},"citation_key":{"type":["string","null"]},
                    "theorem_reference":{"type":["string","null"]},"statement_excerpt":{"type":["string","null"]},"assumptions":string_array(),
                    "applicability":{"type":"string"},"status":{"type":"string","enum":["reported","possibly_applicable","not_applicable"]},
                    "retrieval_query":{"type":["string","null"]},"document_version":{"type":["string","null"]},
                    "fulltext_path":{"type":["string","null"]},"fulltext_sha256":{"type":["string","null"]},
                    "fulltext_artifact_id":{"type":["string","null"]}
                }}},
            "experiments":{"type":"array","items":{"type":"object","additionalProperties":false,
                "required":["language","program_text","input","environment","stdout","stderr","exit_code","artifacts","conclusion_mapping","replay_command"],
                "properties":{
                    "language":{"type":"string"},"program_text":{"type":"string"},"input":attributes(),"environment":attributes(),
                    "stdout":{"type":"string"},"stderr":{"type":"string"},"exit_code":{"type":["integer","null"]},
                    "artifacts":{"type":"array","items":{"type":"object","additionalProperties":false,
                        "required":["path","sha256","description"],
                        "properties":{"path":{"type":"string"},"sha256":{"type":"string"},"description":{"type":"string"}}
                    }},"conclusion_mapping":attributes(),"replay_command":string_array()
                }}}
        }
    })
}

#[must_use]
pub fn paper_writer_schema() -> Value {
    json!({
        "type":"object","additionalProperties":false,
        "required":["status","article_plan_md","claim_evidence_ledger_md","related_work_tex","article_candidate_tex","revision_notes_md","evidence_gaps_md"],
        "properties":{
            "status":{"type":"string","enum":["ready","evidence_gaps"]},
            "article_plan_md":{"type":"string"},
            "claim_evidence_ledger_md":{"type":"string"},
            "related_work_tex":{"type":"string"},
            "article_candidate_tex":{"type":"string"},
            "revision_notes_md":{"type":"string"},
            "evidence_gaps_md":{"type":"string"}
        }
    })
}

#[must_use]
pub fn verifier_schema() -> Value {
    json!({
        "type":"object","additionalProperties":false,
        "required":["verdict","summary","critical_errors","gaps","uncertainties","repair_actions","checked_fact_ids","checked_source_ids","evidence_level"],
        "properties":{
            "verdict":{"type":"string","enum":["accepted","rejected","unknown"]},"summary":{"type":"string"},"critical_errors":string_array(),
            "gaps":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["location","type","issue"],"properties":{"location":{"type":"string"},"type":{"type":"string"},"issue":{"type":"string"}}}},
            "uncertainties":string_array(),"repair_actions":string_array(),"checked_fact_ids":string_array(),"checked_source_ids":string_array(),"evidence_level":{"type":"string","enum":["independent_llm_check","independent_llm_plus_citation_check","formal_kernel","finite_computation","unknown"]}
        }
    })
}

#[must_use]
pub fn route_generator_schema() -> Value {
    let planner = planner_schema();
    json!({
        "type":"object","additionalProperties":false,
        "required":["rationale_summary","routes"],
        "properties":{
            "rationale_summary":{"type":"string"},
            "routes":planner["properties"]["routes"].clone()
        }
    })
}

#[must_use]
pub fn reflection_schema() -> Value {
    json!({
        "type":"object","additionalProperties":false,
        "required":["summary","reviews"],
        "properties":{
            "summary":{"type":"string"},
            "reviews":{"type":"array","minItems":2,"items":{
                "type":"object","additionalProperties":false,
                "required":["route_index","changes_problem","uses_unverified_claims","conflicts_with_facts","repeats_failure_pattern","has_verifiable_milestone","risk_score","goal_closure_leverage","generality_gain","assumption_debt","bridge_centrality","architecture_fit","unjustified_narrowing","remaining_goal_gaps_if_successful","blockers","suggestions"],
                "properties":{
                    "route_index":{"type":"integer","minimum":0},
                    "changes_problem":{"type":"boolean"},"uses_unverified_claims":{"type":"boolean"},
                    "conflicts_with_facts":{"type":"boolean"},"repeats_failure_pattern":{"type":"boolean"},
                    "has_verifiable_milestone":{"type":"boolean"},
                    "risk_score":{"type":"number","minimum":0,"maximum":1},
                    "goal_closure_leverage":{"type":"number","minimum":0,"maximum":1},
                    "generality_gain":{"type":"number","minimum":0,"maximum":1},
                    "assumption_debt":{"type":"number","minimum":0,"maximum":1},
                    "bridge_centrality":{"type":"number","minimum":0,"maximum":1},
                    "architecture_fit":{"type":"number","minimum":0,"maximum":1},
                    "unjustified_narrowing":{"type":"boolean"},
                    "remaining_goal_gaps_if_successful":string_array(),
                    "blockers":string_array(),"suggestions":string_array()
                }
            }}
        }
    })
}

#[must_use]
pub fn strategy_director_schema() -> Value {
    json!({
        "type":"object","additionalProperties":false,
        "required":["verdict_summary","fixed_goal","proof_skeleton","route_portfolio","interface_debts","central_missing_bridge","method_vs_proposition_failure","dangerous_shortcuts","strategy_directives","literature_priorities","macro_replan_required"],
        "properties":{
            "verdict_summary":{"type":"string"},
            "fixed_goal":{"type":"string"},
            "proof_skeleton":{"type":"array","minItems":1,"items":{"type":"string"}},
            "route_portfolio":{"type":"array","items":{
                "type":"object","additionalProperties":false,
                "required":["title","mechanism","mathematical_frontier","decisive_obstacle","evidence_for","evidence_against","status","revisit_condition"],
                "properties":{
                    "title":{"type":"string"},"mechanism":{"type":"string"},
                    "mathematical_frontier":{"type":"string"},"decisive_obstacle":{"type":"string"},
                    "evidence_for":string_array(),"evidence_against":string_array(),
                    "status":{"type":"string","enum":["active","parked","blocked","obsolete","refuted"]},
                    "revisit_condition":{"type":"string"}
                }
            }},
            "interface_debts":{"type":"array","items":{
                "type":"object","additionalProperties":false,
                "required":["interface_name","input_required","output_available","missing_matches","failure_if_ignored","affected_goal_ids"],
                "properties":{
                    "interface_name":{"type":"string"},"input_required":{"type":"string"},
                    "output_available":{"type":"string"},"missing_matches":string_array(),
                    "failure_if_ignored":{"type":"string"},"affected_goal_ids":string_array()
                }
            }},
            "central_missing_bridge":{"type":"string"},
            "method_vs_proposition_failure":{"type":"string","enum":["method_failure","proposition_failure","undetermined"]},
            "dangerous_shortcuts":string_array(),
            "strategy_directives":string_array(),
            "literature_priorities":string_array(),
            "macro_replan_required":{"type":"boolean"}
        }
    })
}

#[must_use]
pub fn supervisor_schema() -> Value {
    let planner = planner_schema();
    json!({
        "type":"object","additionalProperties":false,
        "required":["rationale_summary","assignments","targeted_uncertainty_ids","suggestion_decisions","deferred_route_indices"],
        "properties":{
            "rationale_summary":{"type":"string"},
            "assignments":planner["properties"]["assignments"].clone(),
            "targeted_uncertainty_ids":string_array(),
            "suggestion_decisions":string_array(),
            "deferred_route_indices":{"type":"array","items":{"type":"integer","minimum":0}}
        }
    })
}

#[must_use]
pub fn formalizer_schema() -> Value {
    json!({
        "type":"object","additionalProperties":false,
        "required":["semantic_contract","theorem_name","lean_statement","lean_source","mapping"],
        "properties":{
            "semantic_contract":{"type":"object","additionalProperties":false,
                "required":["variables","assumptions","conclusion","definitions","boundary_conditions","ambiguity_notes"],
                "properties":{
                    "variables":{"type":"array","items":{"type":"object","additionalProperties":false,
                        "required":["name","type_description","lean_type","binder_kind"],
                        "properties":{"name":{"type":"string"},"type_description":{"type":"string"},"lean_type":{"type":["string","null"]},"binder_kind":{"type":"string","enum":["explicit","implicit","instance"]}}}},
                    "assumptions":string_array(),"conclusion":{"type":"string"},
                    "definitions":{"type":"array","items":{"type":"object","additionalProperties":false,
                        "required":["name","description"],"properties":{"name":{"type":"string"},"description":{"type":"string"}}}},
                    "boundary_conditions":string_array(),"ambiguity_notes":string_array()
                }},
            "theorem_name":{"type":"string","pattern":"^[A-Za-z_][A-Za-z0-9_']*$"},
            "lean_statement":{"type":"string"},"lean_source":{"type":"string"},
            "mapping":{"type":"array","items":{"type":"object","additionalProperties":false,
                "required":["natural_component","lean_component","relation"],
                "properties":{"natural_component":{"type":"string"},"lean_component":{"type":"string"},"relation":{"type":"string"}}}}
        }
    })
}

#[must_use]
pub fn alignment_schema() -> Value {
    json!({
        "type":"object","additionalProperties":false,
        "required":["relation","rationale","missing_assumptions","extra_assumptions","confidence"],
        "properties":{
            "relation":{"type":"string","enum":["equivalent","formal_stronger","formal_weaker","incomparable","ambiguous","misaligned"]},
            "rationale":{"type":"string"},"missing_assumptions":string_array(),"extra_assumptions":string_array(),
            "confidence":{"type":"number","minimum":0,"maximum":1}
        }
    })
}

#[must_use]
pub fn tactic_proposal_schema() -> Value {
    json!({
        "type":"object","additionalProperties":false,
        "required":["candidates","retrieval_summary"],
        "properties":{
            "candidates":{"type":"array","minItems":1,"maxItems":8,"items":{
                "type":"object","additionalProperties":false,
                "required":["tactic","rationale_summary","expected_goal_reduction","premise_names"],
                "properties":{
                    "tactic":{"type":"string"},
                    "rationale_summary":{"type":"string"},
                    "expected_goal_reduction":{"type":"number","minimum":0,"maximum":1},
                    "premise_names":string_array()
                }
            }},
            "retrieval_summary":{"type":"string"}
        }
    })
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{
        alignment_schema, formalizer_schema, paper_writer_schema, planner_schema,
        reflection_schema, route_generator_schema, strategy_director_schema, supervisor_schema,
        tactic_proposal_schema, verifier_schema, worker_schema,
    };

    fn assert_strict_objects(value: &Value) {
        if value.get("type").and_then(Value::as_str) == Some("object") {
            assert_eq!(
                value.get("additionalProperties").and_then(Value::as_bool),
                Some(false),
                "structured-output object must set additionalProperties=false: {value}"
            );
        }
        match value {
            Value::Array(values) => {
                for child in values {
                    assert_strict_objects(child);
                }
            }
            Value::Object(values) => {
                for child in values.values() {
                    assert_strict_objects(child);
                }
            }
            _ => {}
        }
    }

    fn assert_array_items_are_typed(value: &Value) {
        if value.get("type").and_then(Value::as_str) == Some("array") {
            let items = value.get("items").expect("array schema must define items");
            assert!(
                items.get("type").is_some()
                    || items.get("anyOf").is_some()
                    || items.get("$ref").is_some(),
                "array items must have an explicit JSON Schema type: {value}"
            );
        }
        match value {
            Value::Array(values) => {
                for child in values {
                    assert_array_items_are_typed(child);
                }
            }
            Value::Object(values) => {
                for child in values.values() {
                    assert_array_items_are_typed(child);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn production_agent_schemas_use_strict_objects() {
        for schema in [
            planner_schema(),
            route_generator_schema(),
            strategy_director_schema(),
            reflection_schema(),
            supervisor_schema(),
            worker_schema(),
            verifier_schema(),
            formalizer_schema(),
            alignment_schema(),
            tactic_proposal_schema(),
            paper_writer_schema(),
        ] {
            assert_strict_objects(&schema);
            assert_array_items_are_typed(&schema);
        }
    }
}
