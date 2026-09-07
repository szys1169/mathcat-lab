# Development rules for math-paper-writing

- Keep `math_paper_writing/` independent of Codex, DeepSeek Harness, and
  MathForge UI objects. Runtime-specific code belongs in `adapters/`.
- All adapters consume the same input contract and return the same result and
  artifact contracts. Never silently change the requested executor.
- Treat model output as untrusted until deterministic delivery gates pass.
- Never report a paper as completed unless its TeX source and PDF exist and the
  configured compilation completed successfully without unresolved citations.
- Preserve every prior version. A failed, stopped, or partial run must not
  update `论文/current/current.json`.
- Keep mathematical verification distinct from writing audit. This capability
  does not prove new claims or certify mathematical correctness.
- Do not place credentials, model transcripts, or unrestricted environment
  dumps in task files, logs, reports, or fixtures.
- Contract changes require a version decision, compatible migration behavior,
  documentation updates, and tests for both accepted and rejected input.
- Every behavior change needs at least one success or invariant test and one
  relevant failure-path test.

