# MathCat Lab 研究白板接口设计

> 状态：P0 已实现（P1/P2 按本文保留）
> 契约版本：`mathcat-board/v1`
> 目标后端：MathCat（Rust）
> 目标客户端：MathCat Lab 研究白板
> 日期：2026-09-03

## 1. 目的

本文档定义 MathCat 为 MathCat Lab 研究白板提供的 HTTP 与实时事件接口。后续更新 MathCat 时，应以本文档作为实现、OpenAPI、兼容性和验收测试的共同契约。

白板不是一套独立的研究状态。MathCat 的数据库、Fact Gate、项目 revision 和事件 cursor 始终是唯一可信状态源；白板只是这些状态的面向研究者的聚合视图，以及向 MathCat 提交人类命令的入口。

本文只规定 MathCat 智能体的接口。Rethlas、Danus 应由 MathCat Lab 的 adapter 转换成相同的前端白板模型，不要求它们原生实现本文接口。

## 2. 设计原则

1. **单一状态源**：白板不自行创建“已证明事实”、任务或路线状态。
2. **候选结论与事实分离**：Worker/Planner 输出只能成为 candidate；只有通过独立验证并由 Fact Gate 提交后才可显示为 Fact。
3. **命令而非直接写表**：所有白板编辑均转换为可审计的 HumanCommand 或专用命令。
4. **乐观并发**：所有可重试写请求携带 `Idempotency-Key` 和 `expected_revision`。
5. **可恢复同步**：初始聚合快照使用 revision，增量同步使用 event cursor；断线后允许重放。
6. **向后兼容**：新增字段必须可忽略；破坏性变更必须发布新的契约版本。
7. **最小暴露**：不向浏览器返回数据库路径、内部提示词、访问令牌或本机绝对存储路径。

## 3. 总体交互

```text
MathCat Lab 对话
    │ conversation_id → project_id（由 Lab 本地保存绑定）
    ▼
GET /api/v1/projects/{project_id}/board
    │ 完整白板投影 + revision + cursor
    ├── GET /events?after={cursor}       (SSE)
    └── GET /ws?after={cursor}           (WebSocket，可选)
             │ DomainEvent
             ▼
      局部更新；事件缺口时重新获取 /board

用户白板操作
    │ Idempotency-Key + expected_revision
    ▼
现有 command/suggestion/question/fact API
或本文新增的 problem-revision / route-proposal / route-approve API
```

MathCat Lab 可以在自己的数据中保存 `conversation_id → project_id`，MathCat 不需要知道工作区路径。若未来需要跨设备恢复绑定，可按第 11.2 节增加不含原始路径的 `client_ref`。

## 4. 通用协议

### 4.1 基础路径和内容类型

- 基础路径：`/api/v1`
- JSON：`Content-Type: application/json; charset=utf-8`
- 时间：UTC RFC 3339 字符串
- ID：不透明字符串；客户端不得解析 ID 的前缀或长度

### 4.2 鉴权

沿用当前 MathCat 鉴权：

```http
X-Actor-Id: mathcat-lab-user
Authorization: Bearer <token>
```

只读接口要求 viewer 权限；影响研究运行的命令要求现有控制面规定的角色。令牌不得进入 URL、事件内容或日志正文。

### 4.3 统一响应包络

沿用现有 `ApiEnvelope<T>`：

```json
{
  "data": {},
  "meta": {
    "request_id": "req_01...",
    "project_revision": 42,
    "event_cursor": 318
  },
  "error": null
}
```

失败时 `data` 为 `null`：

```json
{
  "data": null,
  "meta": {
    "request_id": "req_01...",
    "project_revision": 43,
    "event_cursor": 321
  },
  "error": {
    "code": "stale_project_revision",
    "message": "项目已被其他命令更新",
    "details": { "expected": 42, "actual": 43 },
    "retryable": true
  }
}
```

### 4.4 写请求约定

所有可能由 UI、网络层或用户重复提交的写请求必须包含：

```http
Idempotency-Key: <uuid>
```

请求体包含当前白板 revision：

```json
{
  "expected_revision": 42,
  "reason": "用户在研究白板中执行操作",
  "payload": {}
}
```

同一个 key 与同一请求重试必须返回同一结果；同一个 key 对应不同请求应返回 `409 idempotency_conflict`。revision、cursor、状态变更和 outbox event 必须在同一个 SQLite 事务中提交。

## 5. 白板需要复用的现有接口

除非下文明确要求新增，否则优先复用 MathCat 当前接口。

| 白板能力 | 现有接口 |
|---|---|
| 获取原始完整状态 | `GET /projects/{project_id}/snapshot` |
| 项目状态/最新结果 | `GET /projects/{project_id}/status`、`GET /projects/{project_id}/latest` |
| 任务、路线、目标、事实、不确定性 | 对应的 project collection GET 接口 |
| 证明/依赖关系 | `GET /projects/{project_id}/graphs/{graph_type}` |
| 增量图 | `GET /projects/{project_id}/graphs/combined/delta` |
| 验证队列与明细 | `/projects/{project_id}/verifications` 及 verification-case 接口 |
| 启动/暂停/恢复/停止/重规划 | `POST /projects/{project_id}/commands/{start|pause|resume|stop|replan}` |
| 暂停/恢复/停止/淘汰/恢复路线 | route command 接口 |
| 合并路线 | `POST /projects/{project_id}/routes/commands/merge` |
| 人类建议 | `POST /projects/{project_id}/suggestions` |
| 调整正在执行的任务 | task 的 steer、pause、resume、reassign、cancel、priority、retry 接口 |
| 回答智能体问题 | human-questions 的 list/answer 接口 |
| 质疑或增强 Fact | fact 的 challenge、reverify、request-formalization、request-independent-proof、suspend、revoke 接口 |
| 研究产物 | project artifacts 及 artifact content 接口 |
| 实时状态 | `GET /projects/{project_id}/events?after=` 或 `/ws?after=` |

`POST /projects/{project_id}/suggestions` 的现有请求可直接用于白板“给 MathCat 留言/建议”：

```json
{
  "expected_revision": 42,
  "content": "优先检查有限域上的退化情形，并记录反例边界。",
  "target_route_id": "route_17",
  "reason": "研究者白板建议"
}
```

## 6. P0：新增白板聚合读接口

### 6.1 `GET /projects/{project_id}/board`

用途：一次请求得到首屏所需、语义稳定的研究白板投影。它应由同一份一致性快照生成，避免客户端并发请求多个 collection 后拼出相互矛盾的状态。

查询参数：

| 参数 | 默认值 | 说明 |
|---|---:|---|
| `timeline_limit` | `100` | 最近事件数，范围 0–500 |
| `include` | `summary` | 逗号分隔：`summary,tasks,workers,verification,artifacts,graph` |
| `schema_version` | `1` | 客户端期望的投影版本 |

建议响应头：

```http
ETag: W/"mathcat-board-v1-project_123-r42"
Cache-Control: no-cache
```

客户端可发送 `If-None-Match`；项目 revision 未变化时返回 `304`。

### 6.2 `ResearchBoardView` 示例

```json
{
  "data": {
    "schema_version": 1,
    "project_id": "project_123",
    "agent": "mathcat",
    "mode": "human_collaboration",
    "status": "running",
    "revision": 42,
    "event_cursor": 318,
    "problem": {
      "original_problem": "证明……",
      "target_statement": "对所有 n……",
      "assumptions": ["n ≥ 2"],
      "success_criteria": "目标得到 accepted 裁决……",
      "version": 1
    },
    "summary": {
      "current_round": 3,
      "active_routes": 2,
      "active_tasks": 3,
      "open_goals": 4,
      "accepted_facts": 5,
      "pending_candidates": 2,
      "blocking_uncertainties": 1,
      "pending_human_questions": 1
    },
    "routes": [
      {
        "route_id": "route_17",
        "title": "归纳与局部化",
        "method_summary": "先验证边界情形，再对维数归纳。",
        "status": "running",
        "human_review": "approved",
        "progress": 0.45,
        "target_goal_ids": ["goal_main"],
        "active_task_ids": ["task_31"],
        "blocking_uncertainty_ids": ["uncertainty_8"],
        "updated_at": "2026-09-03T08:00:00Z"
      }
    ],
    "goals": [],
    "claims": [
      {
        "claim_id": "candidate_9",
        "kind": "candidate",
        "statement": "……",
        "status": "verifying",
        "origin_route_id": "route_17",
        "verification_id": "verification_9",
        "fact_id": null
      },
      {
        "claim_id": "fact_12",
        "kind": "fact",
        "statement": "……",
        "status": "accepted",
        "origin_route_id": "route_17",
        "verification_id": "verification_7",
        "fact_id": "fact_12"
      }
    ],
    "failed_routes": [],
    "human_questions": [],
    "uncertainties": [],
    "tasks": [],
    "workers": [],
    "verification_queue": [],
    "artifacts": [],
    "timeline": [],
    "capabilities": {
      "can_edit_problem": true,
      "can_propose_route": true,
      "can_approve_route": true,
      "can_control_project": true,
      "can_control_tasks": true,
      "can_govern_facts": true
    }
  },
  "meta": {
    "request_id": "req_01...",
    "project_revision": 42,
    "event_cursor": 318
  },
  "error": null
}
```

### 6.3 投影语义要求

- `revision` 必须等于 `meta.project_revision`；`event_cursor` 必须等于 `meta.event_cursor`。
- `claims.kind=fact` 只允许来自 Fact 表，且必须附带 `fact_id`。
- 未通过 Fact Gate 的输出只能是 `candidate`、`hypothesis` 或 `unverified_source`。
- `progress` 是展示性估计，范围 `[0,1]`，不得解释为证明正确率。
- `human_review` 与运行状态分开，建议值：`not_required | pending | approved | rejected`。
- `capabilities` 由服务端根据当前 actor、项目状态和功能版本计算；前端不可只靠隐藏按钮实施授权。
- 缺失的可选模块返回空数组或 `null`，不得用伪造演示数据填充。

## 7. P0：新增问题契约修订接口

### 7.1 `POST /projects/{project_id}/problem-revisions`

白板允许研究者修改目标、假设和成功标准，但不覆盖原问题。服务端应创建新 `ProblemContract.version`，记录差异，评估受影响实体，并触发暂停或重规划。

请求：

```json
{
  "expected_revision": 42,
  "target_statement": "修订后的精确目标",
  "assumptions": ["A", "B"],
  "success_criteria": "目标得到 accepted 裁决，依赖闭包均 active",
  "change_reason": "原目标遗漏了有限性条件",
  "replan": true
}
```

响应：`202 Accepted`

```json
{
  "data": {
    "command_id": "command_88",
    "contract_version": 2,
    "status": "applied",
    "affected_entities": [
      { "kind": "route", "id": "route_17" },
      { "kind": "task", "id": "task_31" }
    ]
  },
  "meta": {
    "request_id": "req_01...",
    "project_revision": 43,
    "event_cursor": 321
  },
  "error": null
}
```

实现约束：

- `original_problem` 永远保留，不允许此接口静默覆盖。
- 旧 contract、修改者、理由和时间必须可审计。
- 旧 Fact 不因编辑自动删除；需要计算依赖影响，并按规则 suspend/reverify。
- 正在运行的受影响 task 必须通过 cancellation epoch / task revision 机制失效，防止旧结果晚到后污染新目标。

## 8. P0：新增人类研究路线接口

### 8.1 `POST /projects/{project_id}/route-proposals`

用户在白板中“添加路线”表示提出研究建议，不表示直接写入可运行 Route。

```json
{
  "expected_revision": 43,
  "title": "极小反例法",
  "method_summary": "假设存在极小反例，分析删边后的结构。",
  "target_goal_ids": ["goal_main"],
  "required_fact_ids": ["fact_12"],
  "known_risks": ["极小性可能不保持约束 B"],
  "reason": "研究者提出的新方向"
}
```

服务端先保存为可审计 proposal，由 Planner 校验目标、依赖、预算和重复性。通过后才创建 Route；响应允许是 `202 Accepted`：

```json
{
  "data": {
    "proposal_id": "route_proposal_5",
    "command_id": "command_89",
    "status": "queued",
    "route_id": null
  },
  "meta": {
    "request_id": "req_01...",
    "project_revision": 44,
    "event_cursor": 323
  },
  "error": null
}
```

若仅希望给现有路线补充思路，应继续调用现有 `/suggestions`，不要创建重复路线。

### 8.2 `POST /projects/{project_id}/routes/{route_id}/commands/approve`

仅当项目启用 `human_route_approval` 时需要。批准表示允许 Planner 调度该路线，不表示路线正确。

请求沿用 `CommandRequest`：

```json
{
  "expected_revision": 44,
  "reason": "研究者确认该路线值得分配预算",
  "payload": {}
}
```

拒绝路线不新增接口，复用 `commands/prune` 并提供原因。恢复误拒绝路线复用 `commands/revive`。

状态约束：

```text
proposal queued → accepted → route pending_approval → approved → runnable/running
                └→ rejected                   └→ prune → pruned
```

若未启用人工批准，`human_review=not_required`，Planner 可按原行为直接调度。

## 9. 实时事件契约

### 9.1 复用当前传输

- SSE：`GET /projects/{project_id}/events?after={cursor}`
- WebSocket：`GET /projects/{project_id}/ws?after={cursor}`
- WebSocket 每 15 秒 heartbeat，并支持 `ping`、`resume`。
- WebSocket 返回 `resync_required` 时，客户端必须重新获取 `/board`。

事件沿用当前 `DomainEvent`：

```json
{
  "event_id": "event_321",
  "cursor": 321,
  "project_id": "project_123",
  "project_revision": 43,
  "type": "problem_contract_revised",
  "entity": { "kind": "project", "id": "project_123" },
  "data": { "contract_version": 2 },
  "caused_by": { "kind": "command", "id": "command_88" },
  "occurred_at": "2026-09-03T08:01:00Z"
}
```

### 9.2 白板关心的稳定事件族

内部事件名可以继续演进，但 API 必须在 OpenAPI/事件目录中稳定公开以下语义：

| 事件族 | 最低语义 |
|---|---|
| project | 创建、启动、暂停、恢复、停止、完成、失败 |
| problem contract | 修订及受影响实体 |
| route/proposal | 提出、接受、拒绝、批准、状态与进度变化、合并 |
| goal | 创建、阻塞、解决、重新打开 |
| task/worker | 创建、领取、进度、checkpoint、完成、失败、取消 |
| candidate/verification | 提交、开始验证、裁决、失效 |
| fact | admitted、suspended、revoked、reverified |
| uncertainty | 创建、更新、解决、重新打开 |
| human question/suggestion | 提问、回答、建议已接收/处理 |
| artifact | 创建、替换、完整性失败 |
| budget | 用量更新、接近上限、耗尽 |
| command | accepted、applied、rejected、failed |

事件 `data` 可以增加字段；删除或改变既有字段语义属于破坏性变更。

### 9.3 客户端同步算法

1. 获取 `/board`，原子记录 `revision` 与 `event_cursor`。
2. 使用 `after=event_cursor` 连接 SSE 或 WebSocket。
3. 只处理 `cursor` 更大的事件；重复事件按 `event_id`/cursor 丢弃。
4. 若 cursor 不连续、收到 `resync_required`、遇到未知破坏性 schema，重新获取 `/board`。
5. 写命令成功后，以响应 `meta` 更新 revision/cursor；事件稍后到达时仍按 cursor 去重。
6. `409 stale_project_revision` 时刷新 `/board`。不得自动重放需要人类判断的命令；安全且幂等的开关命令可在确认状态仍适用后重试。

## 10. 白板控件与接口映射

| 白板操作 | 接口 | 说明 |
|---|---|---|
| 建立研究项目 | `POST /projects` | 后续应要求 Idempotency-Key |
| 修改研究问题 | `POST /projects/{id}/problem-revisions` | 新增，不覆盖历史 |
| 开始/暂停/继续/停止 | project commands | 现有 |
| 请求重新规划 | project `commands/replan` | 现有 |
| 添加研究路线 | `POST /projects/{id}/route-proposals` | 新增 |
| 批准研究路线 | route `commands/approve` | 条件新增 |
| 否决研究路线 | route `commands/prune` | 现有 |
| 恢复路线 | route `commands/revive` | 现有 |
| 合并路线 | route merge command | 现有 |
| 给路线补充建议 | project `suggestions` + `target_route_id` | 现有 |
| 干预当前任务 | task `commands/steer` | 现有，需 task revision/route epoch |
| 回答待决问题 | human-question answer | 现有 |
| 质疑结论 | fact `commands/challenge` | 仅 Fact；candidate 走验证流程 |
| 请求复验/形式化/独立证明 | 对应 fact commands | 现有 |
| 打开研究产物 | artifact content | 返回受控内容，不返回 storage_path |

UI 中“添加决策”不得伪造一条由智能体提出的 HumanQuestion。研究者主动输入应使用 `suggestions`；只有 MathCat 创建的问题才进入 human-question answer 流程。

## 11. 项目创建与对话绑定

### 11.1 MVP 方案

MathCat Lab 本地持久化：

```json
{
  "conversation_id": "conversation_abc",
  "agent": "mathcat",
  "endpoint": "http://127.0.0.1:8080",
  "project_id": "project_123",
  "board_schema_version": 1
}
```

不要把 bearer token 写入这条普通对话记录，应使用系统凭据存储。工作区绝对路径也不应发送给 MathCat，除非某项明确能力需要且用户已选择该目录。

### 11.2 可选的跨设备增强（P2）

可在 `CreateProjectRequest` 增加：

```json
{
  "client_ref": {
    "namespace": "mathcat-lab",
    "id": "opaque-conversation-id"
  }
}
```

`namespace + id` 在 actor/tenant 范围内唯一，并配合 `Idempotency-Key` 防止新建项目超时后产生两个 MathCat 项目。不得在 `id` 中直接存放工作区路径。

## 12. 错误语义

| HTTP | 建议 code | 客户端处理 |
|---:|---|---|
| 400 | `invalid_request` | 标注字段错误，不重试 |
| 401 | `authentication_required` | 请求重新配置 MathCat 连接 |
| 403 | `permission_denied` | 刷新 capabilities 并提示权限不足 |
| 404 | `project_not_found` / `entity_not_found` | 检查绑定；允许重新绑定 |
| 409 | `stale_project_revision` | 刷新白板，保留用户草稿 |
| 409 | `idempotency_conflict` | 生成新 key 前先核对原命令结果 |
| 409 | `invalid_state_transition` | 刷新状态，不盲目重试 |
| 422 | `invalid_problem_revision` | 展示领域校验原因 |
| 429 | `budget_or_rate_limit` | 展示恢复时间/预算状态 |
| 503 | `state_writer_unavailable` | 按 `retryable` 退避重试 |

任何错误都必须返回统一包络和 `request_id`。前端不得通过匹配中文 message 判断错误类型。

## 13. 安全与数学可信性

- Planner、Worker、用户建议和外部来源均是不可信输入。
- `/board` 不得把 candidate 混入 `facts`，也不得用“已证明”描述自然语言验证结果。
- Source 在显式验证前显示为 unverified。
- artifact 下载只通过受鉴权的 content 接口；响应使用安全文件名并阻止目录穿越。
- LaTeX 仅作为文本/数学标记渲染，不执行 HTML、脚本、宏文件读取或 shell escape。
- 任务完成提交继续校验 `expected_task_revision` 和 `expected_route_epoch`。
- 修改问题后，旧任务的迟到结果仍必须被 revision/epoch 拒绝。
- 所有人工命令记录 actor、reason、before/after revision 与 affected entities。

## 14. Rust 实现建议

遵循仓库职责边界：

- `crates/domain`：新增 `ResearchBoardView`、`BoardCapabilities`、`ProblemRevisionRequest/Result`、`HumanRouteProposal` 等稳定 DTO/领域类型。
- `crates/storage`：一致性聚合查询、问题版本历史、proposal 持久化，以及 forward-only migration；不绕过 Fact Gate。
- `crates/research-core`：构建白板投影、问题修订影响分析、proposal 校验和重规划。
- `crates/api`：路由、鉴权、OpenAPI、ETag、错误映射和契约测试。
- `crates/worker-runtime`：无需感知 UI；只消费经控制面确认的任务和 route 状态。

`/board` 的数据库读取应在同一 read transaction/snapshot 内完成。不要在 API handler 中依次调用十几个彼此独立的查询再拼装。

## 15. 交付顺序

### P0：白板可用

1. `GET /projects/{id}/board` 与 OpenAPI schema。
2. `POST /projects/{id}/problem-revisions`。
3. `POST /projects/{id}/route-proposals`。
4. 可选人工路线批准 command。
5. 复用既有 events、suggestions、questions、task/fact commands。

### P1：体验与可观测性

1. 公共事件目录及事件 payload schema。
2. 更细的路线进度、失败原因、验证队列摘要。
3. ETag/304 与 board 查询性能指标。
4. 对话绑定恢复和连接诊断信息。

### P2：大项目优化

1. `GET /projects/{id}/board/delta?after={cursor}` 聚合增量接口。
2. 大型 timeline、artifact、proof graph 的 cursor 分页。
3. `client_ref` 跨设备绑定。

P0 不应新增 `/board/delta`；现有 DomainEvent 已能完成增量同步，先避免维护第二套事件协议。

## 16. 验收标准与测试清单

实现完成前至少满足以下自动化测试：

### 16.1 契约测试

- OpenAPI 包含新增路径、鉴权、Idempotency-Key 和所有请求/响应 schema。
- `ResearchBoardView.schema_version == 1`，revision/cursor 与 response meta 一致。
- 未知新增字段不影响旧版 MathCat Lab 解析。
- 所有错误均返回统一 `ApiEnvelope`。

### 16.2 一致性与并发测试

- 同一幂等 key 重试不产生重复问题版本、proposal、route 或 event。
- 两个客户端用同一 expected revision 写入时，仅一个成功，另一个得到 409。
- `/board` 中不存在“新 project revision + 旧 collection”的撕裂快照。
- 命令、revision、cursor 和 outbox event 在故障注入下要么全部提交，要么全部回滚。

### 16.3 实时恢复测试

- 初始 board 后连接 events 不丢失夹在两者之间的事件。
- SSE/WS 断线后从 cursor 重放，重复事件不造成重复卡片。
- WebSocket lag 返回 `resync_required`，重新获取 board 后状态一致。
- 服务重启后 cursor 仍单调，历史事件可重放。

### 16.4 可信性测试

- candidate 在 Fact Gate 前永远不会以 fact/accepted 呈现。
- 验证失败不会创建 Fact。
- problem revision 使受影响的旧任务提交因 revision/epoch 过期而失败。
- source 未经验证时保持 unverified。
- 请求自然语言复验不得被标记为形式化证明。

### 16.5 UI 场景测试

- 新建研究 → 自动绑定 project → 白板首屏可见。
- 暂停/恢复项目和路线后，按钮、事件流和刷新后的状态一致。
- 并发冲突时保留用户输入，刷新后允许用户重新确认。
- 回答 HumanQuestion 后卡片进入已回答状态。
- 路线 proposal 被接受或拒绝后有明确原因和 timeline 事件。
- artifact 可打开，但响应中看不到服务器本机 `storage_path`。

### 16.6 仓库必跑检查

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

发布接口版本时还应执行：

```powershell
cargo build --release
# 再执行本地 HTTP smoke：create → board → start → event → command → board
```

只有明确需要真实模型调用时才运行 `codex-smoke`。

## 17. 完成定义

后端实现只有同时满足以下条件才视为完成：

- 新接口进入 `/api/openapi.json`，不是仅存在于 Rust 内部类型中。
- MathCat Lab 不读取 MathCat 数据库或本地存储文件。
- 白板刷新、实时事件和命令响应最终得到同一状态。
- 所有外部可重试写入具备幂等保证和并发冲突语义。
- Fact Gate、revision、cursor、task revision 和 route epoch 不变量未被削弱。
- 新旧 MathCat Lab 在至少一个兼容窗口内都能工作。
- 本节测试与仓库规定的检查全部通过。

## 18. 当前实现映射

P0 已落入 Rust 后端：领域 DTO 位于 `crates/domain`，一致性白板读事务和两类专用写事务位于 `crates/storage`，事件发布与 Planner proposal 摄取位于 `crates/research-core`，HTTP、ETag、鉴权、统一包络及 OpenAPI 位于 `crates/api`。数据库变更使用 forward-only migration `0016_mathcat_board_v1.sql`。

人工路线批准接口已实现为条件能力。默认项目未启用 `human_route_approval`，因此 `/board.capabilities.can_approve_route=false`；启用后只允许把 `human_review=pending` 改为 `approved`，事件明确记录 `mathematical_endorsement=false`。

问题修订会立即取消受影响的内存 Worker 调用，并依靠 task revision / route epoch 拒绝任何竞态迟到结果。`replan=true` 时服务层主动唤醒研究循环；`replan=false` 且项目正在运行时，项目与白板原子进入 `paused`，等待研究者确认后再恢复。

Planner 提交有效计划时会在同一事务中关闭当轮可见的 queued proposal：路线指纹匹配成功时记录 `route.proposal.accepted` 并关联 Route；未被计划采用时记录带原因的 `route.proposal.rejected`。匹配基于目标、方法规范化文本和所需 Fact，而不依赖标题逐字相等。

---

实现时若本文示例字段与当前领域模型命名存在差异，应优先保持既有领域语义，并通过 API DTO 做稳定映射；不要为了 UI 字段名复制一套业务状态。
