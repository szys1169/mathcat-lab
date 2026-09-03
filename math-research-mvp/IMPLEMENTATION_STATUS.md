# 实现状态

状态日期：2026-09-03。完成边界为验证层 V1～V3、总体架构阶段二/三、接口 P0～P2、Goal Completion Gate、可审计文献摄取，以及带 LaTeX/PDF 质量门的本地版本化发布；V4/V5、外部期刊投稿、P3 与前端不在当前范围。

## 已完成

- Rust/Tokio/Axum/SQLx/SQLite 六 crate workspace，默认 Codex CLI 后端。
- Route Generator、Reflection、Proximity、Ranking、Supervisor 分阶段规划与结构化审计；保守 fallback 可追溯。
- 并发专职 Worker、来源/不确定性/失败知识台账、Experiment Capsule。
- Verification Case、不可变传递闭包快照、确定性预检、并行双数学审查、引用审查、对抗审查、Evidence Adjudicator、事务 Fact Gate。
- Semantic Contract、Formalizer、Alignment Gate、Lean/Mathlib 终审、安全扫描、公理审计、内容寻址 Verification Package、新进程 replay。
- Pantograph JSONL 适配、proof tree、有界 best-first/beam、premise retrieval、hint/prune/cancel/epoch 和独立 Lean 回归检查。
- Fact challenge/reverify/formalize/independent proof/suspend/revoke 与 assurance 历史；Fact/Goal/Route/project/cross-project import 级联失效。
- P0/P1/P2 REST、SSE、WebSocket、OpenAPI 3.1、Artifact、组合图/delta、usage/budget/question/control API。
- MathCat Lab 白板契约 `mathcat-board/v1`：同一 SQLite read transaction 生成 `/board` 投影，支持 ETag/304、模块 include 和稳定 claim 信任分类；问题契约修订保留原题与版本历史，原子取消旧 task/lease/attempt、立即中断内存 Worker 并推进 route epoch，`replan=true` 唤醒研究循环，`replan=false` 原子暂停运行项目；人工路线先进入幂等 proposal，Planner 以目标/方法/Fact 路线指纹匹配并在计划事务中产生带原因的 accepted/rejected 终态事件；条件式 route approve 只授予预算调度资格，不构成数学背书。
- actor/role/Bearer 权限、全局认证 middleware、security audit；远程节点 token/heartbeat/lease/renew/complete。
- 跨项目内容哈希目录、递归依赖导入和逐 Fact assurance snapshot；远程候选和本地候选使用同一验证门禁。
- Codex `turn.completed` 的真实 input/output token 与调用耗时汇总；没有稳定价格口径时 cost 返回 `null`，不伪造费用。
- 增量 Planning V2：Research Delta、Fact Impact、Bottleneck、Plan Revision、路线族/指纹/进展账本、硬容量、merge/prune/tombstone/revive 与精确 Continuity Planner。
- 文献 Worker 的 V2 Task Contract 明确要求把已打开全文保存到隔离工作目录并提交相对路径与 SHA-256；可信运行时逐文件校验后才创建 `source_fulltext` Artifact。若 Codex 沙箱无法写入检索到的公开全文，父进程使用受限 HTTPS 获取器补偿：只允许 443、拒绝私有/回环/链路本地/保留地址，DNS 结果固定到本次请求，每次重定向重新校验，限制媒体类型、30 秒与 50 MiB，并由父进程计算 SHA-256。引用候选缺少全文 Artifact 会在确定性预检终止；citation reviewer 的隔离目录只接收从 Artifact root 重新验 hash 后复制的冻结文件，并必须实际打开全文核对 theorem locator。缺失、越界、哈希错误、伪造 artifact 或安全获取失败的单个来源形成可审计失败，不再使整项 Worker 输出失败；有效来源保持 `reported_unverified`，产生 `source.reported` 和路线进展，但仍须独立来源核验后才能被 Fact Gate 使用。
- Execution V2：Context Packet、Task Contract、Worker Instance、handshake、attempt/lease/heartbeat/checkpoint、Result Envelope、有限重试、dead-letter、late-result/epoch 防护。
- Verification Worker 与 Lean/Pantograph 工具执行统一经过 verification packet/contract/lease/heartbeat/result-envelope；BackendRun 反向关联 attempt。
- SQLite WAL 单 StateWriter/单写连接及文件模式独立只读池；启动 Reconciler、30 秒 Watchdog、Artifact SHA-256 审计、重复 attempt 和缺失 terminal event 修复。
- PostgreSQL 生产 State Committer：`FOR UPDATE SKIP LOCKED`、advisory lock、租约、幂等结果、transactional outbox、死锁/serialization failure 有界重试和显式 doctor/集成测试入口。
- Rethlas 同材料对照评测：两个候选正式运行均达到终态，历史答案隔离、运行/来源哈希、逐题证据审计、两条 RF 计算重放、CI 辅助审计、机器可读评分和总报告均已固化；见 [COMPARISON_REPORT.md](benchmarks/rethlas/runs/20260831-v2-rethlas-valid1/COMPARISON_REPORT.md)。
- 缺口修复后再次以相同公开输入运行 Case 03 和 Case 10：Goal Completion Gate 正确阻止局部 Fact 冒充整题完成；Case 10 真实触发并恢复一次 1,200 秒 Planner 硬超时；Windows 长提示改走 Codex CLI stdin 后两题第二轮均可执行。两题数学终态仍为 `partial_success`，与 Rethlas 的剩余效果差距已固化在 [POST_REMEDIATION_COMPARISON.md](benchmarks/rethlas/runs/20260903-gap-closure-v3/POST_REMEDIATION_COMPARISON.md)。
- Strategy Director 已持久化固定目标、证明骨架、路线组合、接口债务和中央桥梁；宏观审计同时支持事件触发（Fact/Goal/失败/债务/路线/人类命令/停滞）及三次控制审计或四小时周期触发，触发原因随状态和事件保存。文献 Worker 已从被当前 Codex CLI 移除的 `--search` 参数迁移到受支持的 `web_search="live"` 配置。
- 文献/写作能力选择性集成：增强 literature researcher 的来源定位、假设、适用性和 theorem-toolbox 契约；来源只有在强制 citation review 后随 accepted Fact 原子准入。`publish` 只消费 active Fact/admitted Source，材料不足时零模型调用地产生 evidence gaps，材料满足时生成可校验的论文计划、claim-evidence ledger、Related Work 与 LaTeX 候选稿。
- Goal Completion Gate 第一阶段：候选事实正确性与目标闭合分离；精确同陈述使用确定性覆盖证据，等价改写、更强结论和依赖 Fact 联合覆盖须经独立 `goal_coverage_review`，只有覆盖通过才可 solved/refuted；项目成功要求全部高优先级主目标解决且无高危阻塞不确定性。
- 文献执行与成本第一阶段：Codex CLI 仅对 `literature_researcher` 启用 live web search；提示词要求真实打开全文、版本去重、定理定位和适用性检查；无效来源草稿保留失败记录与事件，不再静默丢弃；自然语言 reviewer 只接收依赖 Fact 的最小 claim slice，不重复传输完整证明正文。
- Planner 恢复第一阶段：模型阶段输出按输入 hash 持久化，重入时复用已完成的 Route Generator/Reflection/Supervisor checkpoint；当前阶段失败采用两次有界尝试，已完成阶段不重复调用模型。Planner 与 Worker 提示词会把 failure pattern 编译为禁止条件/修复要求，并强制检查特例推广、递归构造和不同子目标分解。
- Planner 软/硬预算：每个模型规划阶段保存 attempt、软/硬截止、heartbeat、软预算超出标志、失败原因和终态。第一次尝试使用配置硬上限的一半并在其一半处发出软预算事件，第二次重试保留完整硬上限；由此让卡死的首次生成更早进入有界重试，同时不削减最终可用推理预算。崩溃重入继续复用已完成阶段。
- Strategy-to-task 绑定：whole-architecture/central-bridge assignment 在保存前强制携带固定目标、central missing bridge、proof skeleton、接口债务和完成判据；通用机制探针包括“极小性→低复杂度分解障碍”“局部障碍→子结构内极值扩张见证”和“投影参数不闭合→最小辅助覆盖状态”，失败输出也必须收敛到最小精确剩余障碍。文献任务同步继承 Strategy State 的检索优先级和定理定位契约。
- 主目标风险匹配不再只比较包装后的字面文本：`Prove or disprove that P`、`Prove that P`、`证明或否证 P` 与候选结论 `P` 被视为同一数学目标，因而会触发 `critical_certification_v2`，避免开放问题总定理由 `standard_review` 误接纳。
- 规划与验证职责修复：Route Generator 仍须提出至少两种分解，但 Reflection 后若只剩一条合格的中央路线，系统保留它并附加独立反压力任务，不再整轮空转。数学/对抗 reviewer 只裁决候选自身真伪，Goal Coverage 单独裁决主目标闭合；缺失的 source ID 在确定性预检中终结为 rejected，不再把 verification 留在无 Case 的 `verifying/submitted` 状态。
- Reviewer 去相关门禁：`math_review_1/2/3` 分别执行正向逐步义务审计、反向最小反例/边界证伪和遮蔽原证明后的独立重构，不再共享同一角色契约；每次验证保存规范化输出指纹，完全重复的数学审查不能计作独立证据，裁决保守降为 `unknown`。这降低同模型相关性，但不把同一底层模型的多次调用宣称为真正异构同行评审。
- Worker/Strategy 完成合同要求每个候选独立自包含，禁止依赖 sibling Candidate 编号，并显式检查相同见证指标、零/平凡对象、空支撑、空集合和不可用极值。Case 10 两轮同材料复测中 9/9 reviewer-independence 检查通过，第一轮退化域错误被拒绝，第二轮定向修复形成 4 条 reviewed Fact 和一个内部关闭目标的完整证明候选；复现数据与 Rethlas/Danus 差距见 [POST_REVIEWER_DECORRELATION_COMPARISON.md](benchmarks/rethlas/runs/20260903-degenerate-repair-v1/POST_REVIEWER_DECORRELATION_COMPARISON.md)。
- 目标闭合会把仅影响已关闭目标的旧 open/investigating uncertainty 标为 `obsolete`，保留原记录与 `resolved_by` 并写入 Fact Impact；项目级或仍影响未闭合目标的不确定性不受影响，避免已解决目标被陈旧路线疑点错误降为 `partial_success`。
- 来源 provenance、全文与去重：来源 URL 去片段/尾斜杠归一化，DOI/arXiv 标识提取；每次插入、重复或拒绝都写 `source_ingestion_records`。literature worker 的适用来源必须提供工作区内全文和 SHA-256，核心验证路径未逃逸、文件非空、大小受限且 hash 相符后保存为内容寻址 Artifact；来源记录保留 origin task/route、版本、查询、全文 hash/Artifact 和信任声明，API 可查询摄取历史。
- 统一发布生命周期：POST/GET publication API 使用 Idempotency-Key；`publication_runs` 保存输入 revision/hash、attempt、running/blocked/ready/failed、结果和原始错误，重启可恢复未终结运行，重复请求重放终态。
- 版本化论文发布：每次 publication 使用独立目录；ready 稿必须是自包含 LaTeX，拒绝外部读写/执行命令，以 `-no-shell-escape` 编译，并经 PDF header、页数、可抽取文本、未解析引用和逐页 PNG 渲染检查。编译日志、PDF、文本、预览和 QA JSON 均进入 Artifact。

## 自动化证据

- 当前修复 release：`target-strategy-v8/release/math-research-agent.exe`，SHA-256 `C5CEAEAFCAB70B0D0ABC1E952428B140C692FCDE3A3C99FD885F476AAAFBB190`；workspace 75 passed、0 failed、1 ignored live HTTPS diagnostic，fmt 与 Clippy `-D warnings` 通过；隔离 HTTP 冒烟覆盖 OpenAPI 3.1、storage health、白板投影和 ETag 304。
- HTTP 创建/读取/鉴权/权限与命令幂等测试。
- 真实 TCP WebSocket：cursor 回放、双向 ping/pong、heartbeat/resync 协议。
- 完整 Mock round：中间 lemma 不误报成功，主目标必须 fully certified。
- 远程 Worker candidate 通过相同的三路 reviewer 和 Fact Gate。
- task/route/proof/lease/node epoch 与重复完成、迟到提交保护。
- checkpoint 崩溃恢复、submitted envelope 重放、verification restart 隔离、重复活动 attempt 修复和 Artifact 篡改故障注入。
- 64 路并发 SQLite 领域写入均经 StateWriter，`sqlite_busy_total == 0`；文件 SQLite 写连接/只读池可见性测试。
- PostgreSQL 集成测试由 `MRA_TEST_POSTGRES_URL` 显式启用；未配置测试库时明确跳过，不伪造在线验收结果。
- Fact 治理、跨项目依赖闭包、assurance 快照和撤销事件传播。
- Production JSON Schema 递归严格性测试；真实 Codex CLI structured output smoke。
- 论文写作 Schema 递归严格性、引用/`bibitem` 白名单、内部标识和危险 TeX 检查；“无 active Fact 时阻断且零模型调用”测试；完整 round → Fact/Goal closure → writer → LaTeX → PDF → render ready-path E2E；失败 publication 落库和幂等错误重放测试。
- Lean 4.32.2 + Mathlib 4.32.1 全构建，Pantograph 0.3.18 真实 tactic smoke，新进程 Lean replay。

## 有意未包含

- 验证层 V4 的 cvc5、Z3、ATP、FLINT/Arb 和 CAS portfolio。
- 验证层 V5 的定时后台重放服务和外部验证包签名基础设施。
- 外部期刊模板选择、投稿站点打包与提交；当前完成的是受信任材料约束下的本地版本化 LaTeX/PDF 发布和结构/渲染 QA，不把自动结构检查声称为人工同行评审。
- 独立前端、完整 PostgreSQL 版全业务 Repository/API 单体、外部消息队列、云对象存储和公网 TLS 终结。当前 PostgreSQL 范围是架构 R5 要求的生产 State Committer（规划提交、任务租约和结果摄取）。

逐项证据见 [ARCHITECTURE_V2_V3_AUDIT.md](ARCHITECTURE_V2_V3_AUDIT.md)。
