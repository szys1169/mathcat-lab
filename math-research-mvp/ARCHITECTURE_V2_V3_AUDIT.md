# V2/V3 架构审计与验收矩阵

更新日期：2026-08-31

## 1. 范围与结论

本轮范围按以下口径验收：

1. 《数学研究智能体：验证层详细架构》的 V1、V2、V3；
2. 《数学研究智能体详细架构设计》的阶段二、阶段三；
3. 《数学研究智能体：接口与流程图架构》的 P1、P2。

V4/V5、总体阶段四、接口 P3 和独立前端不在本轮完成边界。V4 所列 cvc5、Z3、ATP、严格数值后端没有被伪装成已实现能力。

审计结论：上述 V1～V3、阶段二/三、P1/P2 的明确交付项均已实现，并通过确定性测试与真实 Codex CLI、Lean/Mathlib、Pantograph 验收。模型、远程 Worker、Pantograph 和前端调用者均不能直接写 Fact；唯一写入路径是存储层事务 Fact Gate。

## 2. 验证层 V1～V3

| 要求 | 状态 | 实现与证据 |
|---|---|---|
| Verification Case/Attempt/Profile 状态机 | 完成 | `0004_verification_v2_v3.sql`；Case、Attempt、Check、Finding、Evidence、Policy 类型和非法转换保护 |
| 不可变验证快照 | 完成 | 候选、Problem Contract、完整传递依赖闭包、各依赖 active assurance、来源、策略、工具链哈希进入内容寻址快照；重复快照被拒绝 |
| 确定性预检 | 完成 | 必填/大小、占位符、目标范围、反例目标、重复/失效依赖、来源稳定标识；失败不调用模型 |
| 并行独立审查 | 完成 | 两个数学 reviewer 与 adversarial reviewer 并发冷启动；引用存在时增加 citation reviewer；独立第二证明再增加第三 reviewer |
| Evidence Adjudicator 与 Fact Gate | 完成 | 强制检查、对齐、依赖/来源新鲜度、package/replay、assurance 等级在同一 SQLite 事务核验后提交 Fact |
| Semantic Contract / Formalizer | 完成 | 自然语言结构、Lean statement/source、逐项 mapping 分开保存；Formalizer 输出无认证权限 |
| Alignment Gate | 完成 | 仅 `equivalent` / `formal_stronger` 可继续；weaker、ambiguous、misaligned 等硬阻断并生成可回答问题 |
| Lean 最终检查与安全策略 | 完成 | 独立进程 `lake env lean`；只批准单一 `import Mathlib`；禁止 sorry/admit/axiom/unsafe/FFI/native_decide/run_tac；记录 axioms |
| Verification Package / Replay | 完成 | manifest、源码、契约、对齐、策略、锁文件和 SHA-256；新进程重放同时检查输入哈希、结果和版本 |
| Pantograph 适配与证明树 | 完成 | JSONL ready/请求响应、会话回收、goal/tactic、ProofNode/Edge、backend state id、诊断持久化 |
| 有界交互搜索 | 完成 | whole-proof 后回退 best-first/beam；前提检索、状态去重、失败压缩、节点/深度/时间/模型预算 |
| 人类提示、剪枝、取消 | 完成 | hint 在下一扩展消费；prune/cancel 增加 epoch；迟到分支结果被拒绝 |
| 搜索结果回归 Lean | 完成 | Pantograph 闭合只产生候选 tactic path，随后必须通过独立 Lean 终审和 fresh replay |
| Query/Event API | 完成 | Case、snapshot、checks、findings、evidence、formalization、alignment、backend runs、package、replay、goals、proof tree、attempts、hints |

真实工具证据：

- Lean `v4.32.2` 与 Mathlib `v4.32.1` 由 `lean-toolchain`、`lake-manifest.json`、`SOURCE_LOCK.json` 固定；`lake -Kjobs=1 build MathResearchVerifier` 完成 8656 个 job。
- `lake env lean MathResearchVerifier.lean` 通过，kernel smoke 的公理列表仅含允许的 `propext`。
- Pantograph `0.3.18` 真实会话对 `(1 : Nat) + 1 = 2` 执行 `norm_num` 后闭合，随后独立 Lean 检查通过。

## 3. 总体架构阶段二/三

| 要求 | 状态 | 实现与证据 |
|---|---|---|
| Route Generator | 完成 | 独立结构化模型调用，输出至少两条路线、依赖、风险、里程碑和评分维度 |
| Reflection / Proximity / Ranking | 完成 | 独立反思；事实/目标邻近度；确定性加权、风险惩罚和多样性保留；各阶段输入哈希和输出持久化 |
| Supervisor | 完成 | 基于快照、反思、排序、预算、不确定性和建议形成任务 bundle；deferred route 与建议处置可审计 |
| 规划降级可审计 | 完成 | 模型/Schema 失败时保守 fallback 也保存 `fallback` stage、原因、输入和输出，不静默替代 |
| 专职 Worker | 完成 | prover、explorer、counterexample_hunter、literature_researcher 角色契约；只输出候选和台账 |
| 知识台账 | 完成 | uncertainty、source provenance、raw failure 与 failure pattern 均可查并进入后续规划 prompt |
| 完整依赖闭包 | 完成 | reviewer、premise retrieval、snapshot、Fact Gate 和跨项目 import 均递归展开闭包并检查 assurance |
| 双验证/冲突裁决 | 完成 | 至少双数学审查加对抗审查；任一强制检查冲突不会以多数票静默通过 |
| Fact 治理和传播 | 完成 | challenge/reverify/formalize/independent proof/suspend/revoke；历史 assurance 不覆盖；下游 Fact/Goal/Route/project/import 同步失效 |
| Experiment Capsule | 完成 | 程序、输入、环境、stdout/stderr、退出码、artifact hash、结论映射、重放命令和状态 |
| 组合图与增量 | 完成 | Goal/Hypothesis/Fact/Source/Verification/Proof 组合投影，cursor delta 与 snapshot-assisted resync |

## 4. 接口 P1/P2

| 要求 | 状态 | 实现与证据 |
|---|---|---|
| Safe-point steer | 完成 | 绑定 task revision/route epoch；下一安全点消费；Codex 后端不伪报原生即时 steer 能力 |
| 人工任务控制 | 完成 | create/reassign/pause/resume/cancel/priority，幂等键与 task/project revision 校验 |
| 动态预算 | 完成 | project/round/task/verification/search 作用域；不允许降低到已消耗量以下；搜索循环读取实时 budget |
| usage/cost | 完成 | 调用数、Codex `turn.completed` 真实 input/output token、耗时按 scope 汇总；无稳定计价口径时 cost 明确为 `null` |
| 主动询问 | 完成 | question/options/blocking entities/timeout/answer/resume 及事件 |
| Fact challenge/reverify | 完成 | 新 Verification Case 和 verification history；原事实正文不可就地编辑 |
| WebSocket | 完成 | cursor 回放、双向 ping/resume、heartbeat usage、慢消费者 `resync_required`；真实 socket smoke 通过 |
| 多用户权限 | 完成 | admin/operator/reviewer/researcher/viewer 最小权限；Bearer + actor id；全局认证 middleware 与 append-only security audit |
| Fact revoke/formalize/独立证明 | 完成 | 独立权限、理由和幂等 API；结果走同一 Fact Gate |
| 跨项目 Fact 复用 | 完成 | 全局内容哈希目录；递归依赖导入；逐 Fact assurance snapshot；根或下游撤销实时传播 |
| 分布式 Worker | 完成 | 节点 token、能力、心跳、lease/renew/complete、node/lease/task/route epoch；远程 candidate 同本地门禁 |
| OpenAPI 3.1 | 完成 | ActorBearer、ActorId、WorkerToken 与 P0/P1/P2 路径、参数和主要请求 schema |

## 5. 不可妥协规则核验

1. Planner、Worker、Formalizer、Proof Search、远程节点和 API 都没有直接 Fact 写接口。
2. Fact assurance 可追溯到 snapshot、checks、evidence、tool versions、package 和 replay；治理只追加历史。
3. Lean 编译成功但对齐失败时不能升级认证。
4. backend unavailable、timeout、protocol error、unknown、rejected 分开记录。
5. project revision、event cursor 与状态变更在同一事务提交；跨项目失效事件也随事务返回并实时广播。
6. 外部重试写要求 idempotency key；任务、租约、证明分支校验 revision/epoch。
7. 默认 Codex CLI 使用 `workspace-write`，未启用 danger bypass。

## 6. 验收命令

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release
cargo run -p math-research-agent -- doctor
cargo run -p math-research-agent -- codex-smoke
cargo run -p math-research-agent -- pantograph-smoke
```

`codex-smoke` 已以生产 Worker Schema 完成真实结构化输出；该次验收还发现并修复了实验 artifact item 缺少 JSON Schema `type` 的生产兼容问题。HTTP router smoke、真实 TCP WebSocket smoke、Mathlib 全构建、Pantograph 会话和 Lean 新进程重放均有自动或命令级证据。

## 7. 可靠性 V2 叠加约束

后续《数学研究智能体-规划执行与存储可靠性架构-v2.md》不改变上述数学信任边界，但替换了旧的规划、路线、Worker、上下文和存储并发实现：

- reviewer/formalizer/alignment Agent 以及 Lean/Pantograph 工具运行都已纳入 packet/contract/verification-attempt/lease/heartbeat/result-envelope；
- BackendRun 关联实际 verification attempt，验证工具输出仍需 Evidence Adjudicator 和 Fact Gate，不能凭进程成功写 Fact；
- 规划使用 Delta/FactImpact/Bottleneck/PlanRevision，路线使用容量、指纹、进展、merge/prune/tombstone/revive；
- SQLite 使用单写者和独立只读池，PostgreSQL State Committer 承担多进程 plan/task/result 事务；
- 启动 Reconciler 和周期 Watchdog 处理 checkpoint、过期租约、重复 attempt、缺失终态事件及 Artifact 完整性。

详细实现和未配置 PostgreSQL 测试库时的证据边界见 [ARCHITECTURE_RELIABILITY_V2_AUDIT.md](ARCHITECTURE_RELIABILITY_V2_AUDIT.md)。
