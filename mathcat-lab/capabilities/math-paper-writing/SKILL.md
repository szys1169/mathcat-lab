---
name: math-paper-writing
description: Write a versioned, compiled mathematics paper from an approved Source Packet when MathCat Lab invokes $math-paper-writing. Use for paper drafting and assembly, not proof search or mathematical verification.
---

# Math paper writing for MathCat Lab

The working directory is the user project root. MathCat Lab supplies a natural
language objective and requires every deliverable to remain below the project's
`论文/` directory. Never write into the capability source tree.


## MathCat 2.5 managed execution

The bundled workflow is the workspace latest `prompts/writing.md`, with complete
`prompts/references/` and `prompts/scripts/`; provenance hashes are recorded in
`workflow-source.json`. Package 0.4.0 is distinct from plugin wrapper versions.
When the platform supplies a prepared execution envelope or writer prompt,
prepare/finalize/terminate are owned by the outer driver. Read the latest workflow
and write within the supplied writer directory; do not run lifecycle commands
or create another version. The manual CLI instructions below apply only when no
prepared run is supplied. Chinese is the authoritative language for a paired
MathCat delivery; English uses bilingual-sync from its fixed source snapshot.

## Platform-compatible start

If the prompt gives an existing request JSON, use it. Otherwise:

1. Find the Source Packet explicitly named by the user. If none is named, look
   below `论文/source/`, `论文/source_packet/`, `成果/`, and `调研/` for a unique
   current Source Packet. Do not treat `.platform/`, logs, model transcripts, or
   old build files as mathematical sources.
2. Treat a workspace containing `dataset/cases`, multiple dataset bundles, or
   multiple unrelated paper topics as ambiguous. If the user did not name a
   case, bundle, or Source Packet, do not choose one by convenience or file
   order. Write the platform-supplied task-specific report path under `成果/任务报告/<task-id>/paper-task-report.md`, explaining which concrete path is
   needed, then stop before `prepare`.
3. Resolve every intended input before creating a version. Read the Source
   Packet first and follow its explicit local references. Ignore `.platform/`,
   `论文/versions/`, `论文/current/`, `build/`, and `rendered_pages/` during
   discovery. A pre-existing manuscript below a test bundle's `writer/` is an
   input only when the user, case definition, or Source Packet explicitly
   selects it.
4. Create one internal request at
   `.platform/tasks/math-paper-writing-<timestamp>/request.json` with executor
   equal to the platform-selected executor (`codex` or `deepseek_harness`), the current project as `workspace`, the selected Source Packet,
   language, user objective, and any explicitly named input files.
5. Run the non-mutating preflight before prepare:

```powershell
python <capability-root>/adapters/cli/main.py preflight --request <request.json>
```

Read its `writing_mode` and warnings. `new_draft` means writing from results;
`assemble_existing` means an existing TeX manuscript is being organized or
improved and must never be described as a from-scratch paper. Only when
`ready=true`, run `prepare` exactly once:

```powershell
python <capability-root>/adapters/cli/main.py prepare --request <request.json>
```

Read the returned `instructions_file`, copied `source_packet`,
`input_manifest`, `selected_files`, `task_output_dir`, and `writer_dir`.
If a prepared run later proves unusable, terminate it and report why; do not
silently call `prepare` again to create a second version for the same request.

## Produce and finalize

Write only the declared paper files into `writer_dir`. At minimum create:

- `article_candidate.tex`
- `article_plan.md`
- `claim_evidence_ledger.md`
- `revision_notes.md`

Add `refs.bib` and `related_work.tex` only when verified input supports them.
If `$math-related-work` is unavailable, record that fact and do not pretend a
separate related-work capability ran.

Do not invent submission metadata. Preserve author, affiliation, email,
keywords, MSC classification, and target-venue fields when supplied; otherwise
leave them absent and list the gaps in the report. Missing submission metadata
does not block a compiled writing draft unless the request names a venue that
requires it.

Then run:

```powershell
python <capability-root>/adapters/cli/main.py finalize --run-dir <run_dir>
```

After finalize, read this run's `result.json` again. Report completion only when
it has `status=completed`, `compile_status=passed`, and `render_status=passed`.
Copy every warning and error faithfully; if either list is non-empty, never say
there were no warnings or errors. The final response must state the writing
mode, version directory, primary PDF, primary TeX file, inputs,
compilation/rendering status, warnings, and remaining work.
Also report the `submission_metadata` audit from `result.json` so the user can
distinguish a readable paper draft from a submission-ready manuscript.
MathCat Lab writes this final response to the task-specific path `成果/任务报告/<task-id>/paper-task-report.md` so concurrent runs do not overwrite one another.

Never add claims or citations outside the Source Packet. Writing audit is not
mathematical, human, or formal verification.
