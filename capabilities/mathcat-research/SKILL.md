---
name: mathcat-research
description: Research a precise mathematical problem through autonomous proof exploration, counterexamples, source reading, and evidence-backed repair. Use for sustained mathematical research, not manuscript formatting. No mathematical computation tools are supplied.
---

# MathCat continuous mathematical research

The user-facing role names are 领研猫 for the main researcher (`main`) and 伙伴猫 for independent research partners (`partner`). These names preserve the same mathematical freedom and existing responsibilities; protocol role identifiers remain unchanged.

Work directly on the whole problem. A plan is optional; do not require a decomposition before trying a proof. Preserve exact assumptions, quantifiers and the original goal. Conditional exploration is welcome if its unproved premises remain explicit.

Read only the method relevant to the current obstacle:

- [Direct proof and transfer](references/direct-proof.md): a plausible argument or a similar theorem exists.
- [Examples and counterexamples](references/counterexamples.md): test an assertion, a boundary or an apparently necessary assumption by mathematical reasoning.
- [Sources and applicability](references/sources.md): a specific missing theorem, definition or proof mechanism.
- [Repair and experience](references/repair.md): a review defect, repeated obstruction or resumed research.

These are methods, not sequential stages. Do not call every method, log every deduction, or rewrite a plan every turn. Continue your native session when possible.

When running inside MathCat, the runtime supplies an optional control-file contract, evidence index and `node .mathcat-tools.mjs`. Use `search-memory`, `read-evidence` and `search-arxiv-theorems` only when useful; `--help` describes the local input contract. No mathematical calculation tool is provided. Without the platform, keep a local research note, a proof draft and an explicit list of unresolved assumptions; do not pretend platform tools exist.

Record a finding at a meaningful checkpoint: exact statement, assumptions, scope, evidence, and whether it is provisional. A failed search or unfinished proof is not a refutation. A partner is an independent mathematical researcher, not a mandatory planner. Ask for a concrete mathematical contribution and retain context across replies.

Submit a complete frozen proof when it is worth independent review. Never certify your own result. Reviewer feedback is evidence to examine, not an order to change true mathematics. Keep unresolved issues and repaired versions traceable. Respect the user's stop and run deadline; do not publish or upload results.

In MathCat 2.3, use `context-summary` and targeted search before reading selected exact statements or frozen artifacts. Do not dump the entire `.mathcat-context.json`. At meaningful checkpoints, `record_finding` may include `draft_path` or `draft_refs` with workspace-relative paths: the host freezes their exact contents. Revisions use `node_id` and `expected_revision`; keep assumptions, quantifiers, symbols, scope and unfinished steps precise. A recorded node remains unreviewed. `proposed_dependency_refs` or `record_relation` describe tentative relationships without certifying them. The main researcher may save a `record_goal` with `goal_kind: stage|extension`. Formal candidates can bind `math_node_ref` to the exact same statement version and `goal_refs` to a specific goal version, with relation `supports|covers`. A useful supporting lemma alone does not complete the original problem. Different proofs of the same statement may bind the same mathematical node, each with its own full premise list. The host, independent review and FactGate determine checked dependencies, admission and coverage.

In MathCat 2.2 the main researcher alternates research with lab-wide arrangement at meaningful boundaries. Partners have the same mathematical freedom and explicitly choose continue/wait/complete after each turn. Ordinary reassignment takes effect on the partner's next dispatch after preserving the current result; an explicit stop or route ban takes priority immediately. The main researcher normally keeps a promising current route and delegates new directions for exploration, or requests support on the current route when that is more useful.

At a meaningful handoff, record the current proof goal, local assumptions, symbols with scope, unfinished steps and next step. Immutable drafts may be referenced instead of rewritten. Check unrecoverable conditions again. Before using remembered mathematics as a premise, read its full exact source; a free summary never inherits its mathematical assurance. Declare nontrivial premises, use locations and applicability in `declared_premises` at submission (explicitly `[]` when none). Register unreviewed premises with `set_conditional_premises` before extending dependent research; related partners and revisions share the same bounded allowance. Send ordinary findings to the main researcher with `send_feedback`; use urgent priority only with a concrete urgency reason and identifiable evidence.
