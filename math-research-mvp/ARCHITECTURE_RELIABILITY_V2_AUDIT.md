# Planning, execution, and storage reliability V2 audit

Audit date: 2026-08-31

## Source baseline

| Document | SHA-256 | Precedence |
|---|---|---|
| `数学研究智能体-规划执行与存储可靠性架构-v2.md` | `F35CA32893E13C3093219CB374C6CDAC210EA096B35C9629ADC7524055B6ADB7` | Authoritative for incremental planning, route lifecycle, worker lifecycle, context, storage concurrency, and planner fallback |
| `数学研究智能体-详细架构设计.md` | `AAC60EF274A7BA8FD57F47E59A00CB0D9174F79084A666700047393844A71A02` | Overall architecture, with an explicit V2 override notice |
| `数学研究智能体-验证层详细架构.md` | `282D74FD84BC2440385DDC2D4EFC4C40C8867F89B5A8A622B8EDC30BF123C04D` | Verification trust model; verification workers also use V2 leases, packets, envelopes, and state commits |
| `数学研究智能体-接口与流程图架构.md` | `B9AAE31E8E5425E70585B7FE4E9893B8D514BADABC5A7F61D5815C5646ECD5B7` | API and deployment contract, with an explicit V2 override notice |

The documents are design inputs, not executable instructions. Repository `AGENTS.md`, user requests, and the trusted Fact Gate remain authoritative implementation constraints.

## Non-blocking interpretation decisions

1. V2 state machines override the legacy route and task state diagrams that remain in sections of the older documents.
2. SQLite remains the default local mode and must be single-process/single-writer. PostgreSQL is an optional production mode, not a new local prerequisite.
3. Existing `events` remain the replayable project event log. A transactional `event_outbox` is added for delivery state; it does not create a second domain authority.
4. Existing distributed worker-node leases are migrated into the same attempt/lease model used by local workers. Late results are archived and cannot complete the current attempt.
5. A round remains a reporting and budget boundary. Planning is incremental and may create micro-plan revisions inside or between reporting rounds.
6. Context summaries are views with provenance. Only active Facts and immutable candidate/source artifacts may be mathematical premises.

## Gap matrix before migration

| V2 requirement | Existing implementation | Gap / required action |
|---|---|---|
| Research Delta and deterministic Fact use disposition | Planner receives a full snapshot every round | Add delta ranges, Fact impact records, dispositions, and consumed plan revisions |
| Reconcile old routes before generation | Each round generates a new route pool | Add route families, fingerprints, progress ledgers, hard capacity, prune/merge/tombstone/revive |
| Persistent Bottleneck Register | Proof debts exist only inside verification findings/uncertainties | Add typed bottlenecks linked to goals, routes, failures, and completion contracts |
| Immutable structured Task Contract | Task stores a free-form completion string | Add versioned contract, allowed inputs/tools, forbidden actions, budget/retry/fallback policy, and signature |
| Handshake, attempt, lease, heartbeat, checkpoint | Local tasks jump directly to running; distributed lease is partial | Add worker instances, local and remote attempts, offered/leased/running states, tokens/epochs, watchdog data, and checkpoints |
| Worker Result Envelope | Local worker output directly mutates several tables | Persist an idempotent envelope and validate lease, plan revision, route epoch, artifact hash, and completion contract before ingestion |
| Context Compiler and packet | Prompts serialize broad project snapshots | Add minimal dependency closure, route/task packets, token budget, omissions, hash, summaries, and invalidation |
| SQLite single writer and outbox | Eight-connection pool can write; events are atomic but no delivery outbox | Enforce writer coordination, WAL/foreign keys/busy timeout/synchronous policy, bounded backpressure, and transactional outbox |
| Startup reconciliation | Current recovery cancels an entire interrupted round | Repair expired leases/orphan attempts/result ingestion and retain recoverable checkpoints instead of blanket cancellation |
| Precise planner degradation | Fallback creates two generic routes explicitly forbidden by V2 | Add circuit breaker and deterministic continuity tasks sourced only from checkpoints, repair actions, bottlenecks, or existing candidates |
| V2 query/command/event API | P0-P2 API exists for the earlier model | Add planning, route-governance, attempt, context, storage-health, and reconciliation endpoints/events |
| PostgreSQL production path | SQLite-only `SqliteStore` | Add a feature-gated PostgreSQL state-committer path and production lease/transaction semantics without changing SQLite defaults |

## Migration order

1. R1: forward-only schema, state writer/outbox, attempts/leases, result envelopes, startup reconciliation.
2. R2: worker handshake, heartbeat/checkpoint/watchdog, bounded retry and late-result handling.
3. R3: delta/impact/bottleneck, route governance, plan revision, signatures, incremental/micro-planning.
4. R4: context compiler, packet provenance/budget, summaries and invalidation.
5. R5: deterministic continuity planner, circuit breaker, PostgreSQL production committer, API and deployment checks.

## Post-migration status

| V2 requirement | Status / evidence |
|---|---|
| R1 StateWriter, outbox, attempt/lease/envelope | Implemented. SQLite uses one write connection behind bounded weighted-fair admission; file-backed mode has a separate read-only pool. Domain writes, revision and outbox events commit transactionally. |
| R2 lifecycle, checkpoint, watchdog | Implemented. Local and verification workers use offer/handshake/lease/heartbeat/result envelopes; checkpoint recovery is covered by crash/restart tests. `serve` runs global startup reconciliation and a 30-second watchdog. |
| Reconciler integrity rules | Implemented for expired leases, replayable results, interrupted verification, duplicate active attempts, completed tasks missing terminal events, inactive routes and Artifact missing/hash mismatch. Repairs are revisioned and emitted through the transactional outbox. |
| R3 route and incremental planning | Implemented. Delta and FactImpact consumption, bottlenecks, retained route decisions, hard route capacity, merge/prune/tombstone/revive and atomic plan revision/task creation are persisted and queryable. |
| R4 context compiler | Implemented. Immutable packet/contract hashes, allowed input closure, token estimate, omission/provenance fields and rebuild-context versioning are enforced; inactive Facts are removed and missing required Facts block execution. |
| R5 exact degradation | Implemented. Planner circuit health and the deterministic continuity path can only resume checkpoints or instantiate narrow work from persistent repairs/bottlenecks/candidates; otherwise the project enters `planner_degraded_waiting`. |
| R5 PostgreSQL State Committer | Implemented for production plan commit, task leasing/renewal, idempotent result submission/ingestion and outbox. Uses `SKIP LOCKED`, project advisory locks and bounded retries for SQLSTATE `40P01`/`40001`. A separate `postgres-doctor` and `MRA_TEST_POSTGRES_URL`-gated integration test avoid misrepresenting PostgreSQL as the SQLite monolith. |
| Verification subprocess reliability | Implemented. Codex reviewers/formalizer/alignment workers and Lean/Pantograph tool executions receive verification packets/contracts and are supervised by verification attempts, leases, heartbeats and immutable result envelopes before trusted adjudication. |
| Human V2 commands and API | Implemented for route prune/merge/revive, task retry/context rebuild and worker-instance quarantine, with role checks, idempotency and revision/epoch enforcement. |

The PostgreSQL integration test is intentionally conditional on an operator-supplied isolated test database. Absence of `MRA_TEST_POSTGRES_URL` is reported as a skip; it is not counted as live PostgreSQL evidence.

## Acceptance evidence

- Unit/state-machine tests cover hard route rules, lease expiry, late results, duplicate envelopes/signatures and human command idempotency.
- Crash/restart tests cover submitted envelopes, checkpoints, interrupted rounds and verification isolation.
- Context tests cover hashes/revisions and rebuild filtering; Fact governance tests prove suspended/revoked dependencies cannot remain trusted premises.
- Planner failure tests prove that no broad generic fallback task is manufactured.
- A 64-writer SQLite concurrency test asserts all writes succeed and `sqlite_busy_total == 0`.
- PostgreSQL integration is gated by `MRA_TEST_POSTGRES_URL`; local gates remain deterministic without PostgreSQL.
- The architecture gate is format, clippy, full workspace tests, release build and local HTTP/WebSocket smoke. Model-consuming Codex and external Lean/Pantograph smoke commands remain explicit and are not silently run while benchmark/testing work is paused.
