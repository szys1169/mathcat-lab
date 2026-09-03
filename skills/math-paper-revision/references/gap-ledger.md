# Gap Ledger 契约

根对象字段：

- `schemaVersion`：固定 `0.1`；
- `revisionId`；
- `gate`：`PASS`、`PASS_WITH_WARNINGS`、`RESEARCH_REQUIRED`、`HUMAN_REQUIRED` 或 `FAIL`；
- `verificationLevel`：`rethlas_natural_language`、`formal_lean`、`human_reviewed` 或 `unverified`；
- `issues`：问题数组。

每条问题必须包含：

- `id`；
- `type`：`false_or_counterexample_risk`、`missing_assumption`、`proof_gap`、`circular_dependency`、`citation_mismatch`、`novelty_overlap_risk`、`definition_or_notation`、`unsupported_computation` 或 `unresolved`；
- `severity`：`blocking_math`、`high`、`medium`、`low` 或 `info`；
- `confidence`：0 到 1；
- `paperLocation`、`claim`、`evidence`、`reasoningSummary`、`recommendedAction`；
- `status`：`open`、`resolved`、`research_required`、`human_required` 或 `not_applicable`。

严重度与置信度分开表达。高严重度、低置信度的问题仍应提示，但不能表述成已证实错误。
