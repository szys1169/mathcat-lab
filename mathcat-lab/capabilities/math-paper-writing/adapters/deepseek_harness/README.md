# DeepSeek Harness Host adapter

This is a process boundary, not a web-page implementation. A Host starts the
adapter, reads exactly one JSON document from stdout, and uses the exit code as
the transport outcome. The machine-readable operation manifest is
`adapter.json`.

Install `math-paper-writing/SKILL.md` in the Harness skill directory. It owns
the prepare → write/delegate → finalize protocol, so users only enter their
natural-language writing requirement.

## Health check

```powershell
python adapter.py health
```

Require `ok=true` and `data.ready=true`. Missing LaTeX tools do not prevent
preparation, but they prevent a run from passing the PDF delivery gate.

Host contract:

1. Validate the request with `contracts/input.schema.json` and call
   `adapter.py prepare --request request.json`.
2. Parse the JSON execution envelope from stdout.
3. Keep `executor=deepseek_harness`; never silently route it as a Codex task.
4. Let DeepSeek write the declared files, or let DeepSeek explicitly delegate
   file execution to Codex while retaining the Harness executor identity.
5. Call `adapter.py finalize --run-dir ...`.
6. Return the generated `result.json`, artifacts, and events to MathCat Lab.

The Host may stream its own model events, but stable user-visible state comes
from `events.jsonl` and `result.json`.

## Response envelope

Success returns exit code 0 and one JSON object with `ok=true`, `operation`,
and `data`. A rejected call returns exit code 2 and one JSON object with
`ok=false` and `error: { code, message }`. JSON is ASCII-escaped so Windows
console code pages cannot corrupt Chinese workspace paths.
