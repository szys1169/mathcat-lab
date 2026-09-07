# MathCat paper writing 0.4.0

This package updates the writing workflow to the workspace's current
`skills/math-paper-writing`, which has no semantic version. The exact bundled
SKILL, all references, agent metadata and audit script are listed with SHA-256
in `workflow-source.json`. `prompts/writing.md` is a byte-for-byte copy of that
SKILL. Plugin wrapper 0.3.4 is a separate version and is not the workflow source.

The shared CLI and Python adapters retain contract 1.0. New optional request
fields are `workflow_mode` (`draft`, `revision`, `bilingual-sync`,
`submission-audit`), `idempotency_key`, and `publish_current`. Legacy callers
retain timestamped version preparation and successful-run current publication.
Both `refs.bib` and the latest workflow's `references.bib` are accepted.

## Managed Node interface

Import `createPaperWriter` from `math-lab-platfrom/src/paper-writer.mjs`.

```js
const writer = createPaperWriter({ config, capabilityRoot, runModel });
const zh = await writer.prepare({ workspace, sourcePacket, selectedFiles,
  title, language: "zh", taskId: `${jobId}-zh`, signal });
await writer.write({ prepared: zh, signal, onProgress });
const zhResult = await writer.finalize({ prepared: zh, signal, publishCurrent: false });
// Continue only after completed + compile_status=passed + render_status=passed.
const en = await writer.prepare({ workspace, sourcePacket, selectedFiles,
  title, language: "en", taskId: `${jobId}-en`, signal });
await writer.write({ prepared: en, primary: zh, signal, onProgress });
const enResult = await writer.finalize({ prepared: en, signal, publishCurrent: false });
```

`prepare` returns `runDir`, `writerDir`, `version`, `language`, `workspace`,
`sourcePacket`, `instructionsFile` and `executionRequest`. Each language has
one independent version directory. Use distinct stable task IDs within the
same delivery job. Reusing a task ID with the same inputs restores the same
version, including across restart; changing its parameters is an error. An
interrupted preparation completes its snapshots within the same reserved
directory. Existing prepared snapshots and manuscripts are not replaced.

`runModel` is supplied by the project delivery manager and receives
`{workspace: runDir, taskDir: runDir, prompt, signal, onProgress}`. This adapter
does not choose a model, create a CLI model process, or issue a remote call.
The prompt explicitly leaves lifecycle operations to the driver. `write`
returns `stage: "written"`, not a completed paper. It records a failed or
stopped writing attempt without allocating another version. The caller may
retry `write` with the same prepared object.

`finalize` performs mechanical manuscript audit, TeX source checks, real local
compilation and PDF rendering. It never calls `runModel`. Repeating it recompiles
the existing manuscript and preserves prior reports under `attempts/` in that
version. The return value includes `status`, `compile_status`, `render_status`,
`warnings`, `errors` and `artifacts`. Each artifact has the legacy
`{type, path, size, sha256, preview}` (path relative to runDir), plus Node aliases
`kind` and `absolutePath`. `source_files` and `source_fingerprint` bind the whole
deliverable source tree to the checks; the publisher must verify these before
building a downloadable archive. By default Node finalization never publishes
`论文/current/current.json`; the manager publishes a pair after both pass.

The module also exports `verifyPaperSources(prepared, result)`. It requires a
completed receipt, verifies the entire supported source file set and hashes,
then returns `{path, sha256, bytes: Buffer}[]`. Archive these bytes directly so a
file edit after validation cannot alter the published ZIP. The source set covers
TeX/BibTeX/style files (`tex/bib/sty/cls/bst/bbx/cbx/lbx/def/cfg/clo/fd`), assets
(`png/jpg/jpeg/svg/eps/pdf`), and supporting text/data (`md/json/txt/csv/dat`).
It excludes build/render/cache directories and `article_candidate.pdf`; that
primary PDF is independently bound by the output artifact hash. The source set
is confined to `writerDir`, so it never includes `runDir/primary_snapshot`.

## Chinese primary and English synchronization

English writing requires a Chinese version finalized through this driver.
The current Chinese source tree must still match its completion receipt and
must not have pending/applied rows in the human revision table. Its exact source
tree is copied to `primary_snapshot`, and `primary_source.json` records every
file's hash and the source version. Repeated English operations check both the
original Chinese tree and the fixed snapshot against that receipt. A changed
primary requires a new English synchronization version.

English finalization requires a `sync_checklist.md` and compares recursively
included TeX labels, references, citation keys and ordered math fragments.
Math fragments must keep their TeX tokens; in-formula `\text{...}` and
`\mbox{...}` prose may translate. This deliberately conservative mechanical
comparison can require correction of harmless TeX reformulation. It cannot
prove semantic equivalence of surrounding prose: `sync_report.json` explicitly
records `semantic_review: not_independently_verified`. A model-authored checklist
is evidence of a writing check, not mathematical assurance.

## Delivery limits and retained evidence

`completed` means source/artifact gates, local compilation and mechanical
rendering passed. It does not mean mathematical verification, independent
semantic bilingual review, human manuscript approval, or submission readiness.
The render report labels `inspection_level: mechanical_render` and
`human_visual_review: not_performed`. Rendered PNG pages remain in
`logs/render-check` for actual visual review. Optional page-count/blank-page
checks report whether their supporting libraries were available.

The latest `audit_manuscript.py` is executed locally during finalization;
its review candidates are reported as warnings, not certified defects or
mathematical proofs. No original result is strengthened, and no missing proof
or citation may be invented. Pausing aborts only the local process tree owned
by this adapter and preserves sources. It makes no claim about remote model
billing cancellation. Temporary checking files remain under the project
workspace; this integration does not create build caches on C:.

The package tests use fake model callbacks and real local minimal TeX fixtures.
They test delivery behavior only; they are not a mathematical research result
or a test of the writing quality of any model.
