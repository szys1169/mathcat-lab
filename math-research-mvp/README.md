# 数学研究智能体

Rust 主体、默认 Codex CLI 的可恢复数学研究后端。当前实现覆盖验证层 V1～V3、总体架构阶段二/三和接口 P0～P2：多阶段研究规划、独立验证、Lean/Mathlib 终审、Pantograph 交互证明搜索、人类控制、权限、跨项目事实复用与分布式 Worker。

核心可信边界始终不变：Planner、Worker、Formalizer、Proof Search、远程节点和 API 只能产生 Candidate、Evidence 或控制命令；Fact 只能由 SQLite 事务中的 Fact Gate 在验证策略、不可变快照、依赖闭包和强制证据全部满足后写入。

## 主要能力

- Route Generator → Reflection → Proximity → Ranking → Supervisor 的结构化规划流水线，阶段输入/输出/失败/fallback 全部可审计。
- prover、explorer、counterexample hunter、literature researcher 并发 Worker，以及来源、不确定性、失败知识和实验胶囊台账。
- Verification Case/Attempt/Check/Finding/Evidence、不可变传递闭包快照、确定性预检、并行双数学审查、引用/对抗审查和 Evidence Adjudicator。
- Semantic Contract、Formalizer、Alignment Gate、Lean 安全扫描、公理审计、Verification Package 和新进程 replay。
- Pantograph JSONL 会话、proof tree、有界 best-first/beam、premise retrieval、hint/prune/cancel/epoch；搜索结果必须再次通过独立 Lean 检查。
- Fact challenge/reverify/formalize/independent proof/suspend/revoke；assurance 历史保留，下游 Fact/Goal/Route/项目/跨项目 import 级联失效。
- REST、SSE、WebSocket、Artifact、组合图/delta 和 OpenAPI 3.1；actor/role/Bearer 权限与安全审计。
- 项目/轮次/任务/验证/证明搜索动态预算；真实 Codex token、调用次数和耗时汇总。
- 分布式节点注册、能力、heartbeat、lease/renew/complete 和多重 epoch；远程 candidate 使用同一本地 Fact Gate。
- V2 增量规划：Research Delta、Fact Impact、Bottleneck Register、Plan Revision、路线容量/合并/剪枝/Tombstone/证据复活和确定性 Continuity Planner。
- V2 可靠执行：不可变 Context Packet/Task Contract、Worker handshake、attempt/lease/heartbeat/checkpoint、Result Envelope、有限重试、late-result 拒绝和启动恢复。
- SQLite 文件模式采用 WAL、单 StateWriter/单写连接和独立只读池；服务启动及每 30 秒 Watchdog 修复过期租约、重复活动 attempt、缺失终态事件并审计 Artifact 完整性。
- PostgreSQL 生产 State Committer 提供 `SKIP LOCKED` 领取、项目事务锁、租约、幂等 Result Envelope、transactional outbox 及死锁/serialization failure 有界重试。
- 选择性吸收本地文献调研/论文写作 skill：literature researcher 通过当前 Codex CLI 的 `web_search="live"` 配置使用实时检索，生成带稳定定位、完整假设和适用性说明的来源候选；已打开全文优先落入隔离 Worker 目录并由运行时复核路径、大小与 SHA-256。若 Codex 沙箱只能返回公开 URL，可信父进程会在 HTTPS/443、公共 DNS 地址、逐跳重定向复核、30 秒超时和 50 MiB 上限下获取 PDF/TeX/HTML/TXT，再计算哈希并归档；Codex 托管环境特有的 `198.18.0.0/15` 合成 egress 只在 `CODEX_CI` 标志存在时允许，普通运行仍按保留地址拒绝。失败只隔离该来源。引用候选若没有全文 Artifact 会在确定性预检被拒绝；citation reviewer 得到的是从内容寻址 Artifact 重新验 hash 后复制出的本地冻结文件，并被要求实际打开全文核对定理定位。来源摄取会归一化 DOI/arXiv/URL、去重并保留 inserted/duplicate/rejected provenance；有效来源也只有经引用审查和 Fact 接纳后才单向变为 `admitted`。
- Strategy Director 每轮维护不可变的固定目标、完整证明骨架、路线组合、接口债务和中央桥梁；新 Fact、目标变化、失败模式、证明债务、重复失败、路线组合变化、人类介入或无 Fact 的多任务停滞会触发带原因的宏观审计，此外每三次控制审计或四小时强制周期宏观审计。Supervisor 输出在持久化前还会把固定目标、中央桥梁、证明骨架和接口债务确定性绑定到 whole-architecture/central-bridge 任务合同，防止任务退化成泛泛的“直接解决整题”。
- Route Generator 仍提出至少两种分解；若 Reflection 后只剩一条合法中央路线，Supervisor 可保留它并在同一路线上附加独立反压力任务。Math/Adversarial reviewer 只裁决候选按明示假设是否成立，`target_goal_ids` 仅表示下游关联；中间引理不会因尚未闭合主目标而被拒绝，独立 Goal Coverage check 继续阻止局部 Fact 冒充整题完成。`Prove or disprove that P`/`证明或否证 P` 与候选 `P` 会按同一主目标识别，不能利用任务包装文本绕过主目标的高风险形式化认证。缺失来源依赖在确定性预检中形成终态拒绝。
- 三路数学 reviewer 使用不同的认知契约：正向逐步证明审计、反向最小反例/边界攻击、遮蔽原证明后的独立重构；规范化结果指纹完全重复时，`reviewer_independence` 强制检查失败并把裁决降为 `unknown`，不把复制式输出计作两份独立证据。
- 每个 Worker 候选必须独立自包含，不得用 “Candidate 1/2” 引用同次输出中的其他候选，并须显式处理不同见证指标、非零关系、空支撑与不可用极值等退化边界。目标经 coverage gate 关闭后，只影响已关闭目标的历史 uncertainty 会转为可审计的 `obsolete`，不会继续阻塞项目成功；项目级和仍关联开放目标的风险保持原状态。
- 受控 paper writer 只消费 active Facts/admitted Sources；发布具有 Idempotency-Key、attempt、失败重放和版本目录。ready 稿必须通过危险 TeX 拦截、无 shell-escape LaTeX 编译、引用闭合、PDF 文本/页数检查和逐页渲染。

验证层 V4/V5（cvc5、Z3、ATP、严格数值 portfolio、定时独立认证服务）、外部期刊投稿、独立前端和 P3 生态集成不在当前范围。

## 环境

- Rust 1.85+；当前验收工具链为 Rust 1.98.0。
- 已登录的 Codex CLI；当前真实 smoke 使用 0.151.0。
- Lean 4.32.2 + Mathlib 4.32.1，位于 `lean-verifier/` 并由 toolchain/manifest/source lock 固定。
- Pantograph 0.3.18，位于 `pantograph/`。

默认本地单进程无需外部数据库或消息队列；SQLite 是权威状态，文件系统保存内容寻址产物和人类可读投影。多进程/多主机生产提交路径使用 PostgreSQL State Committer，不能把 PostgreSQL URL 传给 SQLite 单体模式冒充生产支持。

## 构建与验收

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release
cargo run -p math-research-agent -- doctor
```

若配置了隔离的 PostgreSQL 测试库，可额外运行：

```powershell
$env:MRA_TEST_POSTGRES_URL = "postgres://..."
cargo test -p research-storage --test postgres_committer
cargo run -p math-research-agent -- postgres-doctor --postgres-url "postgres://..."
```

真实后端 smoke（会调用一次模型/工具）：

```powershell
cargo run -p math-research-agent -- codex-smoke
cargo run -p math-research-agent -- pantograph-smoke
```

## 同材料对照评测

`benchmarks/rethlas/` 提供隐藏历史答案的同初始材料评测协议与可复现 runner。历史有效正式运行 `20260831-v2-rethlas-valid1` 保存候选数据库、快照、模型用量、worker 制品、独立重放、逐题评分与总报告。缺口修复后以相同公开输入完成了 Case 03 的 `20260903-gap-closure-v2` 和 Case 10 的 `20260903-gap-closure-v3`；两题均正确保持 `partial_success`，恢复机制和剩余数学差距见 [复测报告](benchmarks/rethlas/runs/20260903-gap-closure-v3/POST_REMEDIATION_COMPARISON.md)。`20260903-gap-closure-v1` 仅用于诊断并修复 Windows 长命令行限制，不计入效果比较。

- [总对比报告](benchmarks/rethlas/runs/20260831-v2-rethlas-valid1/COMPARISON_REPORT.md)
- [评测规则](benchmarks/rethlas/runs/20260831-v2-rethlas-valid1/EVALUATION_RUBRIC.md)
- [独立重放证据](benchmarks/rethlas/runs/20260831-v2-rethlas-valid1/REPLAY_EVIDENCE.md)
- [机器可读溯源清单](benchmarks/rethlas/runs/20260831-v2-rethlas-valid1/PROVENANCE.json)

早期基础设施故障运行保留在相邻 run 目录并明确标为无效，不参与评分。

若 `lake` 未进入 PATH，可在子命令前设置：

```powershell
cargo run -p math-research-agent -- `
  --lake-command C:\path\to\lake.exe `
  doctor
```

## CLI 快速开始

全局参数放在子命令之前：

```powershell
cargo run -p math-research-agent -- create `
  --name "示例研究" `
  --problem "证明任意奇数的平方仍为奇数" `
  --max-rounds 4

cargo run -p math-research-agent -- run-once <project_id>
cargo run -p math-research-agent -- run <project_id>
cargo run -p math-research-agent -- status <project_id>
cargo run -p math-research-agent -- snapshot <project_id>
cargo run -p math-research-agent -- publish <project_id>
```

`run` 会从 created/paused 自动执行 start/resume，直到 success、partial_success、refuted、needs_human_review、stopped_by_human 或 error。

`publish` 不参与证明搜索，也不能修改 Fact。默认要求全部高优先级主目标闭合、无 high/critical 开放不确定性、存在 active Fact 与 admitted Source，并要求每个使用的来源均已准入；否则在版本目录写 evidence gaps 和 manifest，且不会调用模型。`--allow-partial` 可生成明确标注覆盖范围的阶段稿，但仍不能引用未准入来源。输出目录为 `output/{project_id}/writer/{publication_id}/`。

## API 与认证

```powershell
cargo run -p math-research-agent -- serve --bind 127.0.0.1:8787
```

在尚无 actor 时可本地 bootstrap 唯一首个管理员：

```text
POST /api/v1/actors/bootstrap
{"actor_id":"admin","display_name":"Admin","token":"至少16字符的秘密"}
```

一旦存在 actor，普通请求必须同时携带：

```text
X-Actor-Id: admin
Authorization: Bearer <token>
```

所有可重试写命令还必须携带唯一 `Idempotency-Key`，状态转换携带最新 snapshot revision 或任务/证明/租约 epoch。Worker heartbeat 和 lease 接口使用独立 WorkerToken 信任域。

常用端点：

```text
GET  /api/v1/projects/{project_id}/snapshot
GET  /api/v1/projects/{project_id}/events?after={cursor}
GET  /api/v1/projects/{project_id}/ws?after={cursor}
GET  /api/v1/projects/{project_id}/graphs/combined/delta?after={cursor}
GET  /api/v1/projects/{project_id}/usage
GET  /api/v1/projects/{project_id}/sources/ingestions
GET  /api/v1/projects/{project_id}/board
POST /api/v1/projects/{project_id}/problem-revisions
GET  /api/v1/projects/{project_id}/route-proposals
POST /api/v1/projects/{project_id}/route-proposals
POST /api/v1/projects/{project_id}/routes/{route_id}/commands/approve
POST /api/v1/projects/{project_id}/publications
GET  /api/v1/projects/{project_id}/publications
GET  /api/v1/publications/{publication_id}
GET  /api/v1/projects/{project_id}/goals/{goal_id}/closures
GET  /api/v1/projects/{project_id}/planning/revisions
GET  /api/v1/projects/{project_id}/planning/delta
GET  /api/v1/system/storage/health
GET  /api/v1/projects/{project_id}/tasks/{task_id}/attempts
POST /api/v1/projects/{project_id}/routes/{route_id}/commands/prune
POST /api/v1/projects/{project_id}/routes/commands/merge
POST /api/v1/projects/{project_id}/tasks/{task_id}/commands/retry
POST /api/v1/projects/{project_id}/tasks/{task_id}/commands/rebuild-context
POST /api/v1/system/reconciliation/commands/run
GET  /api/v1/verification-cases/{case_id}/checks
GET  /api/v1/formalizations/{formalization_id}/proof-tree
GET  /api/v1/facts/{fact_id}/verification-history
GET  /api/v1/facts/catalog
POST /api/v1/projects/{project_id}/fact-imports
GET  /api/openapi.json
```

白板问题修订不会覆盖 `original_problem`。修订会原子推进 task revision / route cancellation epoch 并取消受影响的运行调用；`replan=true` 会唤醒研究循环，`replan=false` 会把正在运行的项目置为 `paused`，由研究者确认后再恢复。

服务默认监听 loopback。绑定非 loopback 地址前必须先 bootstrap admin；生产环境仍应由可信反向代理终结 TLS。完整契约见 [openapi.json](openapi/openapi.json)。

## 配置

| 环境变量 | 默认值 | 说明 |
|---|---|---|
| `MRA_DATABASE_URL` | `sqlite://runtime/research.db` | SQLite URL |
| `MRA_POSTGRES_URL` | 未设置 | `postgres-doctor` 使用的生产 State Committer URL |
| `MRA_TEST_POSTGRES_URL` | 未设置 | 仅 PostgreSQL 集成测试使用的隔离测试库 URL |
| `MRA_ARTIFACT_ROOT` | `runtime/artifacts` | 内容寻址产物根目录 |
| `MRA_RUNTIME_ROOT` | `runtime/projects` | Agent/验证工作目录 |
| `MRA_OUTPUT_ROOT` | `output` | 报告与图投影 |
| `MRA_CODEX_COMMAND` | Windows: `codex.cmd` | Codex CLI |
| `MRA_MODEL` | 未设置 | 沿用 Codex CLI 默认模型 |
| `MRA_LAKE_COMMAND` | `lake` | Lake 可执行文件 |
| `MRA_LEAN_PROJECT_ROOT` | `lean-verifier` | 固定 Lean 工程 |
| `MRA_PANTOGRAPH_COMMAND` | `lake` | Pantograph 启动命令 |
| `MRA_PANTOGRAPH_ROOT` | `pantograph` | Pantograph 工程 |
| `MRA_BIND` | `127.0.0.1:8787` | API 地址 |

## 数据与代码结构

```text
crates/domain          领域对象、状态和传输契约
crates/storage         migration、事务、事件、Fact Gate、权限和租约
crates/worker-runtime  Codex CLI、Lean、Pantograph 进程适配器
crates/research-core   规划、调度、验证、证明搜索、恢复、受控论文候选和报告
crates/api             REST/SSE/WebSocket/Artifact/OpenAPI
crates/cli             serve/doctor/create/run/smoke 入口
migrations             仅向前迁移
migrations-postgres    PostgreSQL State Committer 仅向前迁移
lean-verifier          固定 Lean/Mathlib 验证工程
pantograph             固定 Pantograph 工程
```

`runtime/research.db` 保存权威状态；`runtime/artifacts/` 保存不可变产物；`output/{project_id}/` 保存 `LATEST.md`、逐轮报告、图、文献投影与 `writer/` 候选稿。不要手工修改运行数据库。

详细边界与证据见 [DECISIONS.md](DECISIONS.md)、[IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md) 和 [ARCHITECTURE_V2_V3_AUDIT.md](ARCHITECTURE_V2_V3_AUDIT.md)。
