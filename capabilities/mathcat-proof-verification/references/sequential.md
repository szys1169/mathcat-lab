# Sequential statement verification

Adapted from Rethlas `verify-sequential-statements` (Apache-2.0; see upstream provenance).

Read the complete proof in mathematical order. Identify locations by theorem, lemma or paragraph. For each meaningful deduction, check actual inference validity, hypotheses, quantifiers, theorem application and omitted steps. Compare exact definitions and defining formulas, not just similar names. Verify asserted objects actually exist or were constructed. Distinguish genuinely redundant hypotheses from missing uses that signal a gap. Examine boundary and degenerate cases relevant to the claim.

Record checked locations, critical errors (false implication, invalid application, contradiction) and gaps (missing justification, unsupported existence, hand-wavy specialization). Do not certify a full theorem from a plausible outline or a verified special case. Record findings with location, issue and any concrete evidence. Store local statement checks with `memory-append` when useful for a long proof.
