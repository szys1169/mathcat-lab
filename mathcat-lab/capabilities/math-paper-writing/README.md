# math-paper-writing capability

This package turns an approved Source Packet into a versioned LaTeX paper. It
keeps model-specific execution outside the shared core.

Each execution is isolated in one newly created version directory. This is the
recommended Harness behavior: all inputs, user-facing deliverables, diagnostic
logs, indexes, and reports for one task stay below one `run_dir`, while separate
tasks never share or overwrite that directory.

## Boundary

- The model writes or revises paper files.
- The shared core validates requests, creates immutable versions, compiles
  LaTeX, discovers artifacts, applies delivery gates, and writes reports.
- Codex, DeepSeek Harness, the CLI, MathCat Lab, and the legacy MathForge adapter use the same JSON contracts.
- A failed run never overwrites an earlier version.

## Quick start

Validate inputs without creating a version, then prepare one run:

```powershell
python -m math_paper_writing.cli preflight --request request.json
python -m math_paper_writing.cli prepare --request request.json
```

After an executor writes files into the returned `writer_dir`:

```powershell
python -m math_paper_writing.cli finalize --run-dir <run-directory>
```

Run the Harness adapter:

```powershell
python adapters/deepseek_harness/adapter.py prepare --request request.json
python adapters/deepseek_harness/adapter.py finalize --run-dir <run-directory>
```

The Harness Host integration contract is declared in
`adapters/deepseek_harness/adapter.json`. Call `health` before enabling the
capability. Every invocation prints exactly one JSON response envelope and
uses exit code 0 for a valid operation or 2 for a rejected operation.

`prepare` prints a machine-readable execution envelope. A Harness Host may
pass its `instructions_file`, `source_packet`, and `writer_dir` to DeepSeek or
to a Codex subagent. It must then call `finalize` and return `result.json`.

## Generated project layout

```text
论文/
├─ source_packet/
├─ versions/<task-id>-<timestamp>/
│  ├─ task.json
│  ├─ execution_request.json
│  ├─ events.jsonl
│  ├─ writer/
│  │  ├─ article_candidate.tex
│  │  ├─ article_candidate.pdf
│  │  ├─ build/
│  ├─ input_manifest.json
│  ├─ logs/
│  ├─ artifacts.json
│  ├─ result.json
│  └─ writing_report.md
└─ current/
   ├─ current.json
   └─ README.md
```

`writer/article_candidate.pdf` is the single stable user-facing PDF. The
compiler's duplicate build PDF and successful page-check images are removed
after validation; counts and findings remain in `logs/render_report.json`.
`current/current.json` records the accepted version and direct paths to its PDF,
TeX source, and writing report; it does not merge multiple tasks into one
mutable output directory.

The adapter does not silently change `executor`. Missing model integration is
reported as an adapter error, not disguised as a successful paper run.
