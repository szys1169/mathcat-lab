# Repository instructions

## Scope

This repository implements the trusted single-node MVP described in the parent architecture documents. Preserve the trust boundary: agent output is untrusted until an independent verification is committed by the storage Fact Gate.

## Required checks

Before handing off a change, run:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For release-facing changes, also run `cargo build --release` and the local HTTP smoke scenario. Run `codex-smoke` only when a real model call is intended.

## Invariants

- Never write a Fact directly from Planner or Worker output.
- Keep project revision, event cursor, and state mutation in the same SQLite transaction.
- Require idempotency keys for externally retried writes.
- Reject task submissions with stale task revision or route cancellation epoch.
- Treat source records as unverified until an explicit verification checks them.
- Do not enable Codex CLI danger bypass flags.
- Do not claim natural-language verification is formal proof.
- Add forward-only migrations; do not rewrite an applied migration.

## Layout

- Domain changes belong in `crates/domain`.
- Persistence and trust gates belong in `crates/storage`.
- Agent process/backend behavior belongs in `crates/worker-runtime`.
- Orchestration and reporting belong in `crates/research-core`.
- Transport contracts belong in `crates/api` and operator workflows in `crates/cli`.
