# 数学研究智能体最初第一版问题整理

更新日期：2026-09-02

## 1. 文档目的与范围

本文整理数学研究智能体最初第一版 Rust MVP 暴露的问题，以及后来针对这些问题采取的架构调整。

这里的“最初第一版”特指以下改造之前的实现：

1. 新版验证层 V1～V3；
2. 《规划执行与存储可靠性架构 V2》迁移；
3. P1/P2 控制面、协作和分布式能力；
4. Rethlas 同材料对照测试后的修正；
5. 文献调研与论文写作 skill 的选择性集成。

本文区分三类内容：

- **首版真实缺陷**：首版已有能力存在错误、不可靠或无法审计；
- **首版能力缺口**：架构需要但首版尚未实现；
- **后续评测暴露的遗留问题**：首版形成并延续至今、尚未完全修复的问题。

架构文档是问题判断和产品要求的依据，不作为运行指令。代码中的可信边界始终是：Planner、Worker 和工具输出都不可信，只有通过独立验证并由存储层 Fact Gate 提交的内容才能成为 Fact。

## 2. 首版总体判断

最初第一版已经具备基本的闭环：

```text
Problem → Planner → Worker → Candidate → Verifier → Fact → Report
```

但它更接近“能够跑通一次数学研究任务的单机原型”，还不是适合长期运行的可靠研究系统。核心问题集中在五个方面：

1. 验证结论缺少分层证据和不可变快照；
2. Planner 每轮重建计划，无法稳定消费新事实和历史失败；
3. Worker 缺少严格合同、租约、心跳、checkpoint 和幂等结果；
4. SQLite 并发写入、事件投递和崩溃恢复不够可靠；
5. 数学覆盖率、文献能力、终态聚合和 token 效率仍弱于 Rethlas。

## 3. 验证层问题

### 3.1 验证记录过于扁平

首版主要使用 Candidate、Verification 和最终报告表达验证结果，缺少：

- Verification Case；
- Verification Attempt；
- 分项 Check；
- Finding；
- Evidence；
- Verification Policy；
- 验证阶段状态机。

这导致“谁检查了什么、哪一项通过、哪一项失败、证据在哪里”难以独立查询和重放。

**风险**：单个 verifier 的总结可能同时混合数学错误、工具错误、来源缺口和后端故障，系统无法可靠地区分 rejected、unknown 与 unavailable。

**后续调整**：增加 Verification Case/Attempt/Check/Finding/Evidence/Policy 和分阶段状态机。

**当前状态**：已修复。

### 3.2 缺少不可变验证快照

首版没有把候选、Problem Contract、依赖 Fact、来源、策略和工具链统一固定为内容寻址快照。

**风险**：验证开始后，如果目标、依赖、来源或工具环境发生变化，最终接纳的 Fact 可能并非 reviewer 实际检查的对象。

**后续调整**：增加 Verification Snapshot，并在 Fact Gate 提交前重新检查：

- Candidate hash；
- Problem Contract version；
- 完整依赖闭包及 assurance；
- Source hash；
- Policy hash；
- Toolchain hash。

**当前状态**：已修复。

### 3.3 自然语言命题与形式化命题没有独立对齐门禁

首版即使 Lean 代码编译成功，也缺少结构化机制证明 Lean statement 与原始自然语言候选等价。

**风险**：Formalizer 可以通过遗漏假设、弱化结论、改变量词或改变定义，生成一个容易证明但不是原问题的 Lean 命题。

**后续调整**：增加：

- Semantic Contract；
- Formalizer Output；
- Alignment Reviewer；
- Alignment Gate；
- `equivalent`、`formal_stronger`、`formal_weaker`、`misaligned` 等显式关系。

形式化较弱、含糊或不一致时禁止升级认证。

**当前状态**：已修复。

### 3.4 形式工具成功可能被误当作最终认证

首版缺少完整的工具安全策略、锁文件证据、package 和 fresh replay。

**风险**：一次 Lean 进程成功可能依赖：

- `sorry`、`admit`、新公理或不安全逃生口；
- 未固定版本的 Mathlib；
- 当前工作目录中的临时文件；
- 无法在新进程复现的状态。

**后续调整**：固定 Lean/Mathlib，增加源码扫描、公理审计、Verification Package、SHA-256 manifest 和新进程 replay。

**当前状态**：验证信任链已修复；工具预热和离线准备仍只部分完成。

### 3.5 交互式证明搜索缺少可信隔离

首版没有完整区分“Pantograph 找到一条 tactic path”和“该路径已经通过独立 Lean 最终认证”。

**风险**：搜索器自身状态、缓存或协议错误可能直接污染正式事实。

**后续调整**：Pantograph 只产生候选路径；结果必须重新生成 Lean 文件，经过独立 kernel 检查和 fresh replay 后才能进入 Fact Gate。

**当前状态**：已修复。

### 3.6 缺少 Fact 的后续治理

首版 Fact 一旦接纳，缺少完整的 challenge、reverify、suspend、revoke 和下游影响传播。

**风险**：上游事实被发现错误后，下游 Fact、Goal、Route 和跨项目复用仍可能把它当作可信前提。

**后续调整**：增加 Fact Assurance 历史、Fact Challenge、独立重新验证，以及 Fact/Goal/Route/Project/Import 的级联失效。

**当前状态**：已修复。

## 4. 规划层问题

### 4.1 Planner 每轮读取宽泛快照并重新规划

首版以整份 Project Snapshot 作为每轮规划输入，Planner 缺少精确的增量变化范围。

**风险**：

- 上轮已经处理的内容反复进入上下文；
- 新 Fact 是否被实际采用不可审计；
- 路线容易重复生成；
- token 随项目增长不断增加。

**后续调整**：增加 Research Delta、Fact Impact、Plan Revision 和 Fact disposition。

**当前状态**：数据与编排结构已修复；token 效率仍未完全解决。

### 4.2 生成新路线前不先协调旧路线

首版每轮可以生成新的路线池，但缺少路线族、语义指纹、容量和持续进展判定。

**风险**：

- 同一方法换名后重复出现；
- 失败路线不断复活；
- 路线数量无界增长；
- Planner 无法判断应该继续、合并、暂停还是淘汰。

**后续调整**：增加 Route Family、semantic fingerprint、progress ledger、hard capacity、merge、prune、tombstone 和 evidence-backed revive。

**当前状态**：已修复。

### 4.3 缺少持久化 Bottleneck Register

首版证明缺口主要散落在 verifier 报告、不确定性和失败摘要中，没有可以直接生成窄任务的统一瓶颈对象。

**风险**：下一轮 Planner 容易遗忘具体 proof debt，重新生成宽泛任务。

**后续调整**：增加带目标、路线、证据、完成合同和 repair action 的 Bottleneck Register。

**当前状态**：已修复。

### 4.4 Planner 失败后的 fallback 过于宽泛

首版 Planner 失败时会生成通用证明路线和反例路线。

**风险**：模型失败后系统看似继续运行，实际上脱离当前证据、checkpoint 和真实瓶颈，产生不可控的重复工作。

**后续调整**：确定性 Continuity Planner 只能从以下持久状态恢复：

- checkpoint；
- verification repair action；
- Bottleneck；
- 已有 Candidate；
- 尚未完成的窄任务。

没有合法工作时进入 `planner_degraded_waiting`，不虚构新路线。

**当前状态**：已修复。

### 4.5 Planner 单次超时过长

默认 Planner 硬超时为 20 分钟。Rethlas Case 10 对照中，这一设置占用了大量墙钟时间。

**缺少的机制**：

- 较短软超时；
- 阶段性 checkpoint；
- Route Generator、Reflection、Supervisor 差异化预算；
- 中间结果恢复；
- 超时前降级提示。

**当前状态**：尚未修复。

## 5. Worker 执行问题

### 5.1 Task 只有自由文本完成条件

首版任务缺少不可变、可签名的结构化合同。

**风险**：Worker 可能静默扩大任务、读取未授权输入、改变目标或把不满足完成条件的输出当作成功。

**后续调整**：增加 Task Contract，明确：

- 目标和允许输入；
- 允许工具；
- 禁止行为；
- 输出类型；
- 成功、部分成功和拒绝条件；
- 预算、重试和 fallback；
- 内容哈希。

**当前状态**：已修复。

### 5.2 本地 Worker 直接进入 running

首版缺少 Worker Instance、offer、handshake、attempt、lease 和 heartbeat。

**风险**：

- 无法确认 Worker 是否真实启动；
- 进程消失后任务可能永久处于 running；
- 重启后难以判断哪个结果仍有效；
- 同一任务可能出现多个并行活动执行。

**后续调整**：建立 offer → handshake → leased → running → checkpointed/completed 生命周期，并使用 attempt/lease/epoch 防止重复和迟到结果。

**当前状态**：普通 Worker 和验证 Worker 已修复。

### 5.3 Worker 输出直接修改多个领域表

首版 Worker Output 解析后直接写入发现、失败、不确定性、来源和 Candidate。

**风险**：进程重试、网络重试或服务崩溃可能造成部分写入和重复写入。

**后续调整**：增加幂等 Result Envelope，在摄取前检查：

- lease；
- attempt；
- task revision；
- route cancellation epoch；
- plan revision；
- artifact hash；
- completion contract。

**当前状态**：研究 Worker 已修复；后来新增的 Paper Writer 尚未纳入同一可靠生命周期。

### 5.4 缺少 checkpoint 和精细恢复

首版恢复策略倾向于取消整个未完成轮次。

**风险**：长期任务在进程重启后丢失已完成的中间工作，造成昂贵重复调用。

**后续调整**：保留可用 checkpoint 和 submitted envelope；Reconciler 修复过期 lease、孤儿 attempt、重复活动 attempt 和缺失终态事件。

**当前状态**：研究和验证任务已修复；论文写作任务尚未覆盖。

### 5.5 上下文包过宽

首版 Worker prompt 包含较宽的项目快照，没有严格标注来源、遗漏项和 token 预算。

**风险**：

- token 成本过高；
- 非 active 内容被误当作前提；
- 上下文变化后无法确定 Worker 实际看到了什么；
- 不同任务接收到大量无关材料。

**后续调整**：增加 Context Compiler、不可变 Context Packet、依赖闭包、来源说明、token estimate、omission 和 rebuild-context。

**当前状态**：基本机制已修复；reviewer 的证明切片和差异化上下文尚未完成。

## 6. 存储与恢复问题

### 6.1 SQLite 多连接并发写入

首版连接池中的多个连接可以同时写入 SQLite。

**风险**：高并发时出现 `database is locked`、写入饥饿、事务顺序不稳定或重试风暴。

**后续调整**：SQLite 本地模式改为：

- WAL；
- 单 StateWriter；
- 单写连接；
- 独立只读池；
- bounded weighted-fair admission；
- busy timeout 和同步策略。

**当前状态**：已修复，并有 64 路并发写测试。

### 6.2 缺少 transactional outbox

首版虽然事件与领域状态基本同事务写入，但没有独立的投递 outbox 状态。

**风险**：事务已提交但实时广播失败时，外部消费者难以判断事件是否已可靠投递。

**后续调整**：增加 transactional outbox，领域状态、revision 和事件在同一事务提交，投递状态独立管理。

**当前状态**：已修复。

### 6.3 启动恢复过于粗糙

首版启动时缺少对 lease、attempt、result envelope、artifact 和 terminal event 的逐项协调。

**风险**：任务被重复执行、有效结果被丢弃、孤儿结果被错误信任或状态永久卡住。

**后续调整**：增加启动 Reconciler 和服务 Watchdog，处理：

- 过期 lease；
- 可重放 Result Envelope；
- 中断的 verification；
- 重复活动 attempt；
- completed task 缺少 terminal event；
- inactive route 上的任务；
- Artifact 丢失或哈希不一致。

**当前状态**：已修复。

### 6.4 缺少明确的生产多进程提交路径

首版只有 SQLite，无法安全承担多进程或多主机写协调。

**后续调整**：增加 PostgreSQL State Committer，覆盖 plan commit、task lease/renew、幂等结果摄取和 outbox，并使用 `SKIP LOCKED`、项目 advisory lock 和事务重试。

**当前状态**：State Committer 范围已实现；完整 PostgreSQL 业务 Repository 仍未实现，也未被宣称为已实现。

## 7. 控制面与接口能力缺口

首版主要满足 P0 查询和基本控制，尚缺少：

- safe-point steer；
- 人工创建和重新分配任务；
- 动态预算；
- 主动问题与回答；
- Fact challenge/reverify/formalize/independent proof/revoke；
- WebSocket 双向通道；
- 多用户权限；
- 跨项目 Fact 复用；
- 分布式 Worker；
- 规划 revision、attempt、context 和 storage health API。

这些部分在首版中主要属于能力缺口，而不是已实现功能的错误。

**后续调整**：按接口 P1/P2 逐步实现，并增加 actor/role/Bearer、WorkerToken、幂等键、revision/epoch 和安全审计。

**当前状态**：原 P1/P2 范围基本完成；后来新增的论文发布目前只有 CLI，没有对应发布 API。

## 8. 文献与外部来源问题

### 8.1 来源只是研究线索，没有完整准入生命周期

首版可以保存 Source，但没有清晰区分“Worker 报告的来源”和“已经过引用审查、可以进入论文的来源”。

**风险**：搜索摘要、记忆中的引用或假设不匹配的定理可能被当作可信依据。

**后续调整**：

- 来源默认 `reported_unverified`；
- source-backed Candidate 必须通过 mandatory citation review；
- Candidate 经 Fact Gate 接纳后，引用来源在同一事务升级为 `admitted`；
- Paper Writer 只能引用 admitted Source。

**当前状态**：来源信任门禁已修复。

### 8.2 没有真正的文献调研代理

首版及当前实现仍缺少完整的：

- 研究范围定义；
- 检索策略；
- 搜索供应商适配；
- 筛选日志；
- 来源去重和版本合并；
- 全文获取；
- theorem ledger；
- theorem dependency map；
- claim-source ledger；
- 历史时间线；
- 文献覆盖率和缺口驱动的继续检索。

后来只选择性吸收了 literature researcher 的来源稳定标识、完整假设、适用性和 theorem-toolbox 信任规则。

**当前状态**：部分修复，真实文献代理仍未完成。

## 9. 结果发布与论文写作问题

首版只输出研究报告、图和验证结果，没有完整论文生产阶段。

后来选择性加入了：

- Source Packet；
- article plan；
- claim-evidence ledger；
- Related Work；
- LaTeX candidate；
- revision notes；
- evidence gaps；
- 引用白名单和内部 ID 泄漏检查。

仍未完成：

- 独立 Related Work 流水线；
- proof-obligation audit；
- 假设、符号、变量和定义一致性审计；
- LaTeX 编译；
- PDF 渲染和版面检查；
- 论文版本和 current 指针；
- 原子 publication commit；
- Paper Writer 的 Task Contract/attempt/lease/checkpoint/envelope；
- 发布 API；
- 成功论文生成端到端测试。

**当前状态**：最小可信候选写作已实现，完整论文生产尚未完成。

## 10. Rethlas 对照暴露的首版遗留问题

### 10.1 目标完成聚合错误

Case 10 的完整全称 Fact 已通过三名 reviewer 和 Fact Gate，但项目仍为 `partial_success`。

当前 Goal 关闭仍主要依赖 Fact statement 与 Goal statement 的规范化字符串相等，无法识别语义等价、蕴含或多个 Fact 联合闭合目标。

**当前状态**：尚未修复。

### 10.2 全局数学覆盖率不足

Case 03 中，Rust 智能体只证明了 `e=3` 的局部结论；Rethlas 找到必要性下界和递归 gluing 构造，完成全部参数的结果。

这说明首版虽然能够可靠验证找到的结论，但在以下方面不足：

- 从小参数结果猜测全局结构；
- 自动寻找归纳参数；
- 递归构造和 gluing 路线；
- 必要性与充分性任务分解；
- 从 proof debt 生成新的构造型任务。

**当前状态**：尚未实质修复，是与 Rethlas 的主要能力差距。

### 10.3 文献和论文表达弱

Rethlas 在两个案例中都提供了更成熟的来源和论文式叙述。Rust 系统的优势主要是验证、重放和事件审计，而不是文献覆盖和成稿质量。

**当前状态**：来源准入和最小写作门禁已加入；完整文献调研和论文质量控制仍未完成。

### 10.4 token 和墙钟效率低

正式对照运行共使用约 460 万输入 token。三个 reviewer、大证明上下文和 Planner 长超时是主要原因。

尚缺少：

- 内容寻址证明切片；
- reviewer 差异化最小上下文；
- 已验证片段缓存；
- 来源按 claim 定位加载；
- Planner 软超时和中间结果恢复。

**当前状态**：尚未修复。

## 11. 问题状态汇总

| 问题 | 首版状态 | 当前状态 |
|---|---|---|
| 验证记录扁平、缺少分项证据 | 缺陷 | 已修复 |
| 缺少不可变验证快照 | 缺陷 | 已修复 |
| 自然语言与 Lean 对齐不足 | 缺陷 | 已修复 |
| 工具成功可能被误当作最终认证 | 缺陷 | 信任链已修复，工具预热部分未完成 |
| 缺少 Fact challenge/revoke 传播 | 能力缺口 | 已修复 |
| Planner 每轮宽泛重建 | 缺陷 | 增量结构已修复 |
| 路线重复、换名复活、无容量 | 缺陷 | 已修复 |
| 缺少 Bottleneck Register | 能力缺口 | 已修复 |
| Planner fallback 生成通用任务 | 缺陷 | 已修复 |
| Planner 20 分钟硬超时 | 效率问题 | 未修复 |
| Task Contract 不严格 | 缺陷 | 已修复 |
| Worker 无 handshake/lease/heartbeat | 缺陷 | 已修复 |
| Worker 输出无幂等 Result Envelope | 缺陷 | 研究/验证 Worker 已修复，Paper Writer 未覆盖 |
| 上下文过宽 | 效率与信任风险 | 基本修复，reviewer 切片未完成 |
| SQLite 多连接并发写 | 可靠性风险 | 已修复 |
| 缺少 outbox 和精细恢复 | 可靠性风险 | 已修复 |
| 缺少 PostgreSQL 生产提交路径 | 能力缺口 | State Committer 已实现 |
| 来源缺少准入状态 | 信任风险 | 已修复 |
| 完整文献调研代理 | 能力缺口 | 未完成 |
| 完整论文生产流水线 | 能力缺口 | 部分完成 |
| Goal 语义完成判定 | 正确性问题 | 未修复 |
| 全局构造与探索能力 | 能力问题 | 未修复 |
| token/墙钟效率 | 效率问题 | 未修复 |
| 发布 API 和可靠写作生命周期 | 后续集成缺口 | 未完成 |

## 12. 当前建议的修复优先级

### P0：结果正确性

1. 建立 Goal Completion Gate，支持等价、蕴含、反例和 Fact 组合闭合；
2. 将 Goal completion 判定与验证证据一起持久化，替代纯字符串相等；
3. 为 Case 10 增加回归测试，确保完整 Fact 不再低报为 `partial_success`。

### P1：研究能力和效率

1. Planner 软超时、checkpoint 和分阶段预算；
2. proof slice、reviewer 差异化上下文和缓存；
3. 针对递归构造、归纳参数、必要性/充分性分解的专门路线生成；
4. 使用 Case 03 作为全局覆盖能力回归基准。

### P2：文献和论文能力

1. 完整文献检索、筛选、去重和 theorem/claim ledger；
2. 独立 Related Work 阶段及逐句来源审计；
3. proof-obligation、符号、定义、引用和 LaTeX build audit；
4. 将 Paper Writer 纳入 Task Contract/lease/checkpoint/result-envelope；
5. 增加发布 API、版本化、原子提交和成功端到端测试。

## 13. 依据文件

- `DECISIONS.md`：开发前不确定性和架构决定；
- `ARCHITECTURE_V2_V3_AUDIT.md`：验证层、总体阶段二/三和接口 P1/P2 验收；
- `ARCHITECTURE_RELIABILITY_V2_AUDIT.md`：可靠性 V2 改造前缺口和迁移结果；
- `IMPLEMENTATION_STATUS.md`：当前实现边界；
- `benchmarks/rethlas/runs/20260831-v2-rethlas-valid1/COMPARISON_REPORT.md`：同材料对照暴露的数学覆盖、终态、文献和效率问题；
- 四份上层架构设计文档：产品和信任边界依据。

## 14. 最终结论

最初第一版的主要价值，是建立了一个可以运行的 Rust 数学研究闭环；其主要问题，是长期运行、验证可信性、状态恢复和研究覆盖能力不足。

后续工作已经把验证、事实治理、Worker 生命周期、增量规划、SQLite 可靠写入和控制面提升到可审计状态。但以下问题仍继承自首版并未闭合：

1. Goal 语义完成判定；
2. Planner 长超时；
3. 全局构造与探索覆盖；
4. 完整文献调研代理；
5. token 与墙钟效率；
6. 完整、可靠、版本化的论文生产流程。

因此，当前系统已经较好解决“错误结果不能轻易成为 Fact”，但还没有完全解决“如何更快、更全面地找到正确结果并形成高质量研究论文”。
