# 来源包契约（Source Packet）

写作或修改论文前，先把可用素材整理成来源包。来源包是"输入白名单"：论文正文只允许来自其中的数学内容、事实与文献。宁可列出缺口，也不要用合理推测填充。

## 输入清单

| 类别 | 内容 | 论文中的用途 | 缺失时的处理 |
|---|---|---|---|
| 主结果 | 定理/引理/命题陈述（量词、假设、结论） | 主结果节、引言主定理 | 缺 → 不写该结果 |
| 证明材料 | 证明笔记、proof sketch、已完成的证明 | 证明节 | 只有 sketch → 标注 `needs proof`，不写成已证明 |
| 定义清单 | 对象、记号、术语及其归因 | 定义节、Notation/Preliminaries | 缺 → 列入 gap |
| 文献素材 | BibTeX、论文、已知结果归属记录 | 综述、引言、归因定理 | 缺 → 综述只写缺口定位，不编造引用 |
| 目标信息 | 期刊/会议、篇幅、风格、匿名要求 | 全文风格与模板 | 未知 → 用通用学术风格 |
| 审稿意见（可选） | reviewer comments | 修订轮 | 有则逐条回应 |
| 状态记录（可选） | 哪些结果已确认、哪些是开放义务 | 结论节边界 | 无 → 默认只写来源包内已确认内容 |

## Source Packet 表

### Writing Goal

- Target deliverable:
- Target venue or audience:
- Page, style, or anonymity constraints:

### Core Results

| Result | Source | Proof status | Depends on | Notes |
|---|---|---|---|---|
|  |  | proved / sketch / conjecture / empirical |  |  |

### Source Materials

| Material | Path or citation | Role | Reliability |
|---|---|---|---|
|  |  | proof / related work / definition / figure / draft | verified / partial / unknown |

### Evidence Gaps

| Gap | Blocking section | Needed evidence | Owner |
|---|---|---|---|
|  |  |  |  |

## 启动路由

- 章节顺序或依赖关系不清楚 → 先做 [paper-skeleton.md](paper-skeleton.md)。
- 要宣称某个数学结果"已证明" → 先跑 [proof-obligation-audit.md](proof-obligation-audit.md) 的写作侧检查。
- 要写 Abstract / Introduction / Conclusion 这类压缩性文字 → 先建 [claim-evidence-ledger.md](claim-evidence-ledger.md) 台账。
