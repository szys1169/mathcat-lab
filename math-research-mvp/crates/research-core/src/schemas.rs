use serde_json::{Value, json};

fn string_array() -> Value {
    json!({"type":"array","items":{"type":"string"}})
}

fn suggestion_decisions() -> Value {
    json!({
        "type":"array",
        "items":{
            "type":"object",
            "additionalProperties":false,
            "required":["suggestion_id","disposition","rationale"],
            "properties":{
                "suggestion_id":{"type":"string","minLength":1},
                "disposition":{"type":"string","enum":["applied","deferred","rejected"]},
                "rationale":{"type":"string","minLength":1}
            }
        }
    })
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

/// Strict output contract for the pre-project problem-definition generator.
///
/// `material_paths` is projected into an enum so generated provenance cannot name files that were
/// not part of the bounded material snapshot.
#[must_use]
pub fn problem_generator_schema(material_paths: &[String]) -> Value {
    let material_references = if material_paths.is_empty() {
        json!({"type":"array","maxItems":0,"items":{"type":"string"}})
    } else {
        json!({
            "type":"array",
            "items":{"type":"string","enum":material_paths}
        })
    };
    json!({
        "type":"object",
        "additionalProperties":false,
        "required":[
            "name","problem","target_statement","assumptions","success_criteria","budget",
            "human_route_approval","budget_rationale","generation_notes",
            "unresolved_questions","material_references"
        ],
        "properties":{
            "name":{"type":"string","minLength":1,"maxLength":160},
            "problem":{"type":"string","minLength":1},
            "target_statement":{"type":"string","minLength":1},
            "assumptions":{"type":"array","maxItems":64,"items":{
                "type":"object","additionalProperties":false,
                "required":["statement","provenance"],
                "properties":{
                    "statement":{"type":"string","minLength":1},
                    "provenance":{"type":"string","enum":["prompt","material","inferred"]}
                }
            }},
            "success_criteria":{"type":"string","minLength":1},
            "budget":{"type":"object","additionalProperties":false,
                "required":[
                    "max_rounds","max_parallel_workers","max_minutes_per_task",
                    "max_model_calls_per_task","max_total_model_calls"
                ],
                "properties":{
                    "max_rounds":{"type":"integer","minimum":1,"maximum":64},
                    "max_parallel_workers":{"type":"integer","minimum":1,"maximum":16},
                    "max_minutes_per_task":{"type":"integer","minimum":1,"maximum":180},
                    "max_model_calls_per_task":{"type":"integer","minimum":1,"maximum":16},
                    "max_total_model_calls":{"type":"integer","minimum":1,"maximum":1000}
                }
            },
            "human_route_approval":{"type":"boolean"},
            "budget_rationale":{"type":"string","minLength":1},
            "generation_notes":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1}},
            "unresolved_questions":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1}},
            "material_references":material_references
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
                "required":["title","method_summary","approach_kind","route_role","user_title","plain_language_summary","why_this_route","expected_output","relation_to_goal","steps","target_goal_ids","required_fact_ids","expected_subgoals","expected_goal_progress","uncertainty_reduction","human_suggestion_alignment","evidence_support","route_diversity","verifiability","novelty","failure_similarity_penalty","cost_penalty","risks"],
                "properties":{
                    "title":{"type":"string"},"method_summary":{"type":"string"},
                    "approach_kind":{"type":"string","enum":["direct_proof","counterexample","computation","reduction","literature","formalization","other"]},
                    "route_role":{"type":"string","enum":["primary","adversarial","auxiliary","prerequisite"]},
                    "user_title":{"type":"string","minLength":1,"maxLength":40},
                    "plain_language_summary":{"type":"string","minLength":1},
                    "why_this_route":{"type":"string","minLength":1},
                    "expected_output":{"type":"string","minLength":1,"description":"完成时预期得到的可检查产物及检查标准；不宣称该产物已经存在或已获认证。"},
                    "relation_to_goal":{"type":"string","minLength":1},
                    "steps":{"type":"array","minItems":2,"maxItems":6,"items":{"type":"string","minLength":1}},
                    "target_goal_ids":string_array(),
                    "required_fact_ids":{"type":"array","items":{"type":"string"},"description":"仅列当前已有的 active Fact ID；没有此类输入时可为空，不得把待证明义务或未知引理编造成事实 ID。"},
                    "expected_subgoals":{"type":"array","items":{"type":"string"},"description":"计划证明、证伪、核对或计算检查的研究义务；尚未完成的子目标不是已成立前提。"},
                    "expected_goal_progress":{"type":"number","minimum":0,"maximum":1},"uncertainty_reduction":{"type":"number","minimum":0,"maximum":1},
                    "human_suggestion_alignment":{"type":"number","minimum":0,"maximum":1},"evidence_support":{"type":"number","minimum":0,"maximum":1},
                    "route_diversity":{"type":"number","minimum":0,"maximum":1},"verifiability":{"type":"number","minimum":0,"maximum":1},"novelty":{"type":"number","minimum":0,"maximum":1},
                    "failure_similarity_penalty":{"type":"number","minimum":0,"maximum":1},"cost_penalty":{"type":"number","minimum":0,"maximum":1},"risks":string_array()
                }}},
            "assignments":{"type":"array","minItems":1,"items":{"type":"object","additionalProperties":false,
                "required":["route_index","worker_role","strategic_role","addresses_interface_debt","goal_ids","objective","completion_contract","priority"],
                "properties":{"route_index":{"type":"integer","minimum":0},"worker_role":{"type":"string","enum":["prover","explorer","counterexample_hunter","literature_researcher"]},"strategic_role":{"type":"string","enum":["whole_architecture","central_bridge","adversarial","literature","computation","local_milestone"]},"addresses_interface_debt":{"type":"boolean"},"goal_ids":string_array(),"objective":{"type":"string"},"completion_contract":{"type":"string"},"priority":{"type":"number","minimum":0,"maximum":1}}}},
            "targeted_uncertainty_ids":string_array(),"suggestion_decisions":suggestion_decisions()
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
                    "applicability":{"type":"string"},"status":{"type":"string","enum":["lead_unverified","reported","possibly_applicable","not_applicable"]},
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
                    "changes_problem":{"type":"boolean"},
                    "uses_unverified_claims":{"type":"boolean","description":"仅当路线把未经 Problem Contract 允许或 active Fact 认证的结论当作已成立前提时为 true。计划证明、证伪或计算检查未知桥梁本身应为 false；不能仅因引理尚未证明、算法尚未实现或没有已有事实而为 true。为 true 时 blockers 必须给出被当真结论及其路线字段或步骤位置。"},
                    "conflicts_with_facts":{"type":"boolean"},"repeats_failure_pattern":{"type":"boolean"},
                    "has_verifiable_milestone":{"type":"boolean","description":"是否承诺将来可检查的具体产物及检查标准，而非该产物是否已经存在。"},
                    "risk_score":{"type":"number","minimum":0,"maximum":1},
                    "goal_closure_leverage":{"type":"number","minimum":0,"maximum":1},
                    "generality_gain":{"type":"number","minimum":0,"maximum":1},
                    "assumption_debt":{"type":"number","minimum":0,"maximum":1},
                    "bridge_centrality":{"type":"number","minimum":0,"maximum":1},
                    "architecture_fit":{"type":"number","minimum":0,"maximum":1},
                    "unjustified_narrowing":{"type":"boolean"},
                    "remaining_goal_gaps_if_successful":string_array(),
                    "blockers":{"type":"array","items":{"type":"string"},"description":"具体障碍或待完成的研究义务。若 uses_unverified_claims=true，必须指明被当作已成立前提的结论、路线字段或步骤位置，以及为何是使用而非计划证明；仅说尚未证明或尚未实现不足以支持该标记。"},
                    "suggestions":string_array()
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
            "suggestion_decisions":suggestion_decisions(),
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
        problem_generator_schema, reflection_schema, route_generator_schema,
        strategy_director_schema, supervisor_schema, tactic_proposal_schema, verifier_schema,
        worker_schema,
    };

    fn assert_keyword_absent(value: &Value, keyword: &str) {
        match value {
            Value::Array(values) => {
                for child in values {
                    assert_keyword_absent(child, keyword);
                }
            }
            Value::Object(values) => {
                assert!(
                    !values.contains_key(keyword),
                    "structured-output schema contains unsupported keyword {keyword}: {value}"
                );
                for child in values.values() {
                    assert_keyword_absent(child, keyword);
                }
            }
            _ => {}
        }
    }

    fn assert_all_object_properties_required(value: &Value) {
        if value.get("type").and_then(Value::as_str) == Some("object") {
            let properties = value
                .get("properties")
                .and_then(Value::as_object)
                .expect("object schema must define properties");
            let required = value
                .get("required")
                .and_then(Value::as_array)
                .expect("strict object schema must define required fields");
            for property in properties.keys() {
                assert!(
                    required
                        .iter()
                        .any(|required| required.as_str() == Some(property)),
                    "strict object property {property} is not required: {value}"
                );
            }
        }
        match value {
            Value::Array(values) => {
                for child in values {
                    assert_all_object_properties_required(child);
                }
            }
            Value::Object(values) => {
                for child in values.values() {
                    assert_all_object_properties_required(child);
                }
            }
            _ => {}
        }
    }

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

    #[test]
    fn problem_generator_schema_is_codex_compatible_and_strict() {
        let schema = problem_generator_schema(&["notes/a.md".into()]);

        assert_keyword_absent(&schema, "uniqueItems");
        assert_strict_objects(&schema);
        assert_all_object_properties_required(&schema);
        assert_array_items_are_typed(&schema);
        assert_eq!(
            schema["properties"]["material_references"]["items"]["enum"],
            serde_json::json!(["notes/a.md"])
        );

        let empty_material_schema = problem_generator_schema(&[]);
        assert_keyword_absent(&empty_material_schema, "uniqueItems");
        assert_eq!(
            empty_material_schema["properties"]["material_references"]["maxItems"],
            0
        );
    }

    #[test]
    fn suggestion_decisions_require_exact_ids_typed_dispositions_and_rationales() {
        let schema = supervisor_schema();
        let decisions = &schema["properties"]["suggestion_decisions"];
        assert_eq!(
            decisions["items"]["required"],
            serde_json::json!(["suggestion_id", "disposition", "rationale"])
        );
        assert_eq!(decisions["items"]["additionalProperties"], false);
        assert_eq!(
            decisions["items"]["properties"]["disposition"]["enum"],
            serde_json::json!(["applied", "deferred", "rejected"])
        );
        assert_eq!(
            decisions["items"]["properties"]["suggestion_id"]["minLength"],
            1
        );
        assert_eq!(
            decisions["items"]["properties"]["rationale"]["minLength"],
            1
        );
    }

    #[test]
    fn worker_source_schema_distinguishes_search_leads_from_evidence_candidates() {
        assert_eq!(
            worker_schema()["properties"]["sources"]["items"]["properties"]["status"]["enum"],
            serde_json::json!([
                "lead_unverified",
                "reported",
                "possibly_applicable",
                "not_applicable"
            ])
        );
    }

    #[test]
    fn route_schema_requires_complete_researcher_facing_presentation() {
        let route = &route_generator_schema()["properties"]["routes"]["items"];
        let required = route["required"].as_array().expect("required fields");
        for field in [
            "approach_kind",
            "route_role",
            "user_title",
            "plain_language_summary",
            "why_this_route",
            "expected_output",
            "relation_to_goal",
            "steps",
        ] {
            assert!(required.iter().any(|item| item == field), "missing {field}");
        }
        assert_eq!(route["properties"]["steps"]["minItems"], 2);
        assert_eq!(route["properties"]["steps"]["maxItems"], 6);
    }

    #[test]
    fn route_schema_documents_dependencies_and_unfinished_obligations_separately() {
        let schema = route_generator_schema();
        let properties = &schema["properties"]["routes"]["items"]["properties"];
        let dependencies = &properties["required_fact_ids"];
        let obligations = &properties["expected_subgoals"];

        assert_eq!(dependencies["type"], "array");
        assert!(dependencies.get("minItems").is_none());
        assert!(
            dependencies["description"]
                .as_str()
                .expect("dependency semantics")
                .contains("不得把待证明义务或未知引理编造成事实 ID")
        );
        assert!(
            obligations["description"]
                .as_str()
                .expect("obligation semantics")
                .contains("尚未完成的子目标不是已成立前提")
        );
        assert!(
            properties["expected_output"]["description"]
                .as_str()
                .expect("future output semantics")
                .contains("不宣称该产物已经存在或已获认证")
        );
    }

    #[test]
    fn reflection_schema_clarifies_semantics_without_changing_wire_fields() {
        let schema = reflection_schema();
        let review = &schema["properties"]["reviews"]["items"];
        let properties = &review["properties"];

        assert_eq!(
            review["required"],
            serde_json::json!([
                "route_index",
                "changes_problem",
                "uses_unverified_claims",
                "conflicts_with_facts",
                "repeats_failure_pattern",
                "has_verifiable_milestone",
                "risk_score",
                "goal_closure_leverage",
                "generality_gain",
                "assumption_debt",
                "bridge_centrality",
                "architecture_fit",
                "unjustified_narrowing",
                "remaining_goal_gaps_if_successful",
                "blockers",
                "suggestions"
            ])
        );
        assert_eq!(properties.as_object().expect("properties").len(), 16);
        assert_eq!(properties["uses_unverified_claims"]["type"], "boolean");
        assert!(
            properties["uses_unverified_claims"]["description"]
                .as_str()
                .expect("premise semantics")
                .contains("计划证明、证伪或计算检查未知桥梁本身应为 false")
        );
        assert!(
            properties["blockers"]["description"]
                .as_str()
                .expect("misuse evidence requirement")
                .contains("结论、路线字段或步骤位置")
        );
        assert!(
            properties["has_verifiable_milestone"]["description"]
                .as_str()
                .expect("milestone semantics")
                .contains("而非该产物是否已经存在")
        );
    }
}
