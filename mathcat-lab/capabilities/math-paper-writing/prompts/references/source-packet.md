# 来源包契约（Source Packet）

写作或修改论文前，先把可用素材整理成来源包。来源包是“输入白名单”：论文正文只允许来自其中的数学内容、事实与文献。主动文献补全所得来源只有在核验并准入后才能加入来源包；宁可列出缺口，也不要用合理推测填充。

## 输入清单

| 类别 | 内容 | 论文中的用途 | 缺失时的处理 |
|---|---|---|---|
| 主结果 | 定理/引理/命题陈述（量词、假设、结论） | 主结果节、引言主定理 | 缺 → 不写该结果 |
| 证明材料 | 证明笔记、proof sketch、已完成的证明 | 证明节 | 只有 sketch → 标注 `needs proof`，不写成已证明 |
| 定义清单 | 对象、记号、术语及其归因 | 定义节、Notation/Preliminaries | 缺 → 列入 gap |
| 文献素材 | BibTeX、论文、已知结果归属记录 | 综述、引言、归因定理 | 缺 → 默认列入 Literature Gaps；用户授权时进入主动文献补全 |
| 目标信息 | 期刊/会议、篇幅、风格、匿名要求 | 全文风格与模板 | 未知 → 用通用学术风格 |
| 作者决策（可选） | 术语、记号、主修订语言、结构与排版偏好 | style decisions / bilingual sync | 有冲突 → 以最新明确决定为准并记录 |
| 审稿意见（可选） | reviewer comments | 修订轮 | 有则逐条回应 |
| 状态记录（可选） | 哪些结果已确认、哪些是开放义务 | 结论节边界 | 无 → 默认只写来源包内已确认内容 |
| 计算材料（可选） | 代码、证书、日志、复现协议及其角色 | 证明/核验/实验/例子 | 无协议 → 不当作证明 |

## Source Packet 表

### Writing Goal

- Target deliverable:
- Target venue or audience:
- Page, style, or anonymity constraints:
- Primary revision language (if multilingual):
- Existing terminology/notation decisions:

### Core Results

| Result | Source | Proof status | Depends on | Notes |
|---|---|---|---|---|
|  |  | proved / sketch / conjecture / empirical |  |  |

### Source Materials

| Material | Path or citation | Role | Reliability |
|---|---|---|---|
|  |  | proof / related work / definition / figure / draft | verified / partial / unknown |

### Literature Policy And Coverage

- Literature mode: `closed-sources` / `active-literature-completion`
- User authorization for external search (if any):
- Search scope or exclusions:
- Allowed access methods/accounts (if any):

| Literature function | Needed source | Verified source | Claim supported | Status |
|---|---|---|---|---|
| Original problem or conjecture |  |  |  | covered / gap / not-needed |
| Foundational definition or criterion |  |  |  | covered / gap / not-needed |
| Closest prior result |  |  |  | covered / gap / not-needed |
| Known scope, limitation, or counterexample |  |  |  | covered / gap / not-needed |
| Method lineage |  |  |  | covered / gap / not-needed |
| Application or related direction |  |  |  | covered / gap / not-needed |
| Recent directly relevant work |  |  |  | covered / gap / not-needed |

文献覆盖按功能判断，不设固定引用数量。关键格为空时不得用泛泛的“此前尚未研究”“最强结果”填补。启用主动补全时，详细候选、检索式和核验状态写入 `writer/literature_search_log.md`。

### Author Decisions And Feedback

| Decision/request | Scope | Source/date | Status |
|---|---|---|---|
|  | terminology / notation / structure / language / revision |  | active / superseded / pending |

### Computation Evidence

| Artifact | Claimed role | Protocol/certificate | Reproducibility | Allowed paper wording |
|---|---|---|---|---|
|  | proof / computer-assisted-proof / independent-check / experiment / example-discovery |  | verified / partial / unknown |  |

### Evidence Gaps

| Gap | Blocking section | Needed evidence | Owner |
|---|---|---|---|
|  |  |  |  |

## 启动路由

- 章节顺序或依赖关系不清楚 → 先做 [paper-skeleton.md](paper-skeleton.md)。
- 要宣称某个数学结果"已证明" → 先跑 [proof-obligation-audit.md](proof-obligation-audit.md) 的写作侧检查。
- 要写 Abstract / Introduction / Conclusion 这类压缩性文字 → 先建 [claim-evidence-ledger.md](claim-evidence-ledger.md) 台账。
- 文献覆盖矩阵存在关键缺口且用户明确要求补全 → 调用 `$math-related-work` 的 `literature-completion` 模式；未授权则保留 `Literature Gaps`。
- 有作者实质修改或多个语言版本 → 读 [human-revision-and-bilingual-sync.md](human-revision-and-bilingual-sync.md)。
