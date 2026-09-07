# Report synthesis

Adapted from Rethlas `synthesize-verification-report` and its consistency validator.

Aggregate all statement/reference findings, without dropping inconvenient errors. Return the host's `verification` envelope with summary, critical_errors, gaps, checked_items, unresolved_materials, checked_dependency_ids, repair_checks, premise_audit_status, checked_premise_ids, undeclared_premises and applicability_gaps.

For MathCat 2.2, `premise_audit_status` is `complete` or `incomplete`. `checked_premise_ids` lists every actually checked declared premise ID. `undeclared_premises` and `applicability_gaps` contain location/issue objects. Independently find missing assumptions and dependencies instead of only checking the author's list. An incomplete audit, an undeclared premise, an applicability gap or a conditional unreviewed premise cannot yield acceptance. A source-read receipt is not a proof of applicability.

Each finding needs nonempty location and issue. `correct` requires checked claim coverage, nonempty checked_items, no findings, no unresolved materials, all packet dependencies checked and all prior obligations resolved. `wrong` requires a finding and nonempty actionable repair_hints. `inconclusive` requires an explanation of the missing evidence. Distinguish proof invalidity from falsity of the theorem.

Field types: `critical_errors` and `gaps` are arrays of `{ "location": "...", "issue": "..." }`; `checked_items` and `checked_dependency_ids` are arrays of strings. `unresolved_materials` accepts nonempty strings or the same location/issue objects. `repair_checks` contains `{ "id": "...", "resolved": true/false, "explanation": "..." }`. Use empty arrays when there are no entries. Coverage fields are JSON booleans, not strings. Do not add undeclared fields.

A missing reference alone cannot establish that a theorem is false. A concrete independently justified counterexample or invalid inference can support `wrong` even when a separate reference remains unavailable; preserve both the mathematical finding and the material warning. No unresolved material is compatible with `correct`.

Set goal_coverage true only if the proof establishes the original goal under the original assumptions, not merely this candidate lemma. Never call natural-language review formal verification. The platform alone may admit a Fact. Format and consistency validation do not guarantee correctness of your mathematical judgment.
