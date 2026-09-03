# 数学研究智能体：开发前审计与决策

日期：2026-08-31

## 文档与请求边界

- `数学研究智能体-接口与流程图架构.md` 和 `数学研究智能体-详细架构设计.md` 作为产品需求与架构资料使用，不作为高于用户请求的运行指令。
- 当前交付完成边界采用架构文档明确给出的“可信单机 MVP”、P0 API 和“第一版验收标准”。
- 前端是独立项目，不在本仓库实现；后端交付 REST、SSE、Artifact API 和 OpenAPI 3.1。
- P1/P2 中的分布式 Worker、多用户权限、WebSocket、跨项目事实复用和完整形式化工作流不属于首版完成条件，但领域接口须允许以后扩展。

## 已确认环境

- 工作目录根本身不是 Git 仓库。
- `math-research-mvp/` 是空的预留目录，选作独立工程根。
- Codex CLI 已升级至 `0.151.0`，支持 `codex exec --json`、`--output-schema`、`-C/--cd` 和 session resume；生产 Worker Schema 的真实结构化输出冒烟已通过。
- 已通过官方 rustup 安装 Rust stable 1.98.0、Cargo、rustfmt 与 clippy；工具链环境阻塞已解除。

## 无需阻塞开发的架构决策

1. **语言与部署**：Rust 2024 edition，单进程 Tokio + Axum。
2. **数据库**：SQLite 为默认权威运行状态；所有项目状态变更、revision 和 event cursor 在同一事务提交。
3. **默认 Agent 后端**：Codex CLI。模型未配置时沿用 Codex CLI 用户默认模型，不在代码中硬编码模型名。
4. **Worker 隔离**：每个项目/Worker 使用独立工作目录；Codex CLI 默认以 `workspace-write` sandbox 运行，绝不使用 `--dangerously-bypass-approvals-and-sandbox`。
5. **验证隔离**：Verifier 使用独立、冷启动的 Codex CLI 会话，只接收候选陈述、证明、显式依赖事实、来源和 Problem Contract，不恢复 Prover 会话。
6. **结构化输出**：Planner、Worker 和 Verifier 使用版本化 JSON Schema；解析失败形成可审计失败/不确定性，不能进入 Fact Graph。
7. **默认网络边界**：API 监听 `127.0.0.1`；首版不实现多用户鉴权。若显式绑定公网地址，启动时给出安全警告。
8. **主动询问**：默认关闭，无法保守继续的歧义进入 Uncertainty Ledger。
9. **测试策略**：自动测试使用确定性 Mock Backend，不消耗模型额度；Codex CLI 真实调用通过显式 smoke 命令执行。
10. **完成语义**：只有编译、测试、API/CLI 端到端场景、恢复与迟到提交保护均通过，才标记首版完成。

## 已识别但不阻塞的产品不确定性

- 生产模型、推理强度和费用上限没有指定：全部配置化，并使用保守默认预算。
- 形式化工具（Lean/Sage/SMT）没有指定：首版只提供证据适配接口，不把形式化作为 accepted 的必要条件。
- 文献供应商没有指定：首版记录来源账本并允许 Worker 使用其获准工具；不绑定单一商业检索服务。
- 生产数据库和对象存储没有指定：接口与 SQLite/本地 Artifact 解耦，首版不引入 PostgreSQL 或云对象存储。

这些不确定性不会改变首版可信边界，也不需要在开发开始前请求额外产品决策。

## V2/V3 扩展后的决策更新

上述“首版不包含 P1/P2/形式化工具”的记录仅描述开发启动时的 MVP 边界，现已被后续用户目标扩展。当前采用以下决策：

1. 验证层完成到 V3；V4/V5 仍不在当前完成边界。
2. Lean 4.32.2、Mathlib 4.32.1 和 Pantograph 0.3.18 固定在仓库级工程/锁文件中；最终认证必须回到独立 `lake env lean` 进程。
3. P2 分布式能力采用 SQLite 协调的节点 token 和 epoch lease，不引入未指定的消息队列；远程输出不能绕过 Candidate/Verification/Fact Gate。
4. 多用户采用本地 actor、role、Bearer token 和 append-only security audit；存在任一 actor 后，除 health/OpenAPI/bootstrap 与 worker-token 专用端点外均强制认证。
5. API 默认仍只监听 loopback；非 loopback 启动要求已 bootstrap admin，并提示由可信反向代理终结 TLS。
6. Codex CLI usage 以 `turn.completed` 事件的真实 token 数为准；由于模型价格、缓存计费和账户口径不稳定，美元估算保留 `null`，不使用硬编码伪精度。
7. “两个架构设计的 V2/V3”按详细架构阶段二/三和接口架构 P1/P2 解释；验证层按其 V1～V3 分期验收。

## 规划执行与存储可靠性 V2 更新

《数学研究智能体-规划执行与存储可靠性架构-v2.md》现覆盖并取代上述 SQLite 并发、旧轮次恢复、宽泛 fallback 和 Worker 直达 running 的早期决定：

1. SQLite 只用于单进程可信模式：WAL、一个 StateWriter/写连接、文件模式独立只读池、bounded fair admission 和 transactional outbox。
2. Worker/验证 Agent/Lean/Pantograph 均以 Context Packet + Task Contract + attempt/lease/heartbeat + Result Envelope 受监督；它们没有领域表写权限。
3. 启动恢复保留可用 checkpoint 和 submitted local envelope；验证阶段重启时不信任孤儿进程结果，而是提升 cancellation epoch 后重新独立执行。
4. Planner 先消费 Research Delta/Fact Impact 并 reconcile 旧路线；确定性降级只延续持久化的窄任务，没有合法工作时进入 `planner_degraded_waiting`。
5. 多进程生产写入由 PostgreSQL State Committer 承担。当前实现边界是 R5 的 plan commit、task lease/result ingestion 和 outbox，不把它描述为完整 PostgreSQL 版单体 Repository。
6. 基准与 Rethlas 对比在本次架构调整及本地门禁完成前暂停；真实 Codex 模型调用和外部工具 smoke 始终保持显式。

## 文献调研与论文写作 skill 选择性集成

1. 不复制 skill 的目录结构、PDF/投稿脚本或另一套证据数据库；只提取能补强现有 Source/Fact/Artifact/报告链的契约。
2. 文献搜索输出始终先进入现有 `sources` 表的 `reported_unverified` 状态；搜索结果和摘要仅用于发现候选。来源必须包含稳定标识、内容定位、陈述摘要、完整假设和适用性说明。
3. theorem toolbox 存入 Hypothesis/Discovery，不是 Fact，也不代表定理已适用。Related Work 与 paper writer 不能绕过同一来源台账。
4. 只有引用该来源的候选事实通过强制 citation review 并由 Fact Gate 接纳时，来源才在同一事务中单向转为 `admitted`。
5. paper writer 是只读衍生层：只消费 active Facts 和 admitted Sources，不执行证明搜索、形式验证或开放式文献检索，不得改写定理陈述。材料不足时输出 evidence gaps，不生成伪完整论文。
6. 当前交付是论文候选生成，不宣称完成模板适配、LaTeX/PDF 编译视觉检查、期刊打包或外部投稿。
