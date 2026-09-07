# 声明台账（Claim–Evidence Ledger）

在打磨 prose 之前，先让每条实质论断可追溯到证明、引用、实验或明确不确定性。摘要是高风险区：压缩性文字最容易夸大结果。

## 台账表

| Field | Meaning |
|---|---|
| `claim` | 论文中的确切或转述论断 |
| `location` | 节、段、定理、公式或图 |
| `type` | `theorem` / `proof` / `citation` / `definition` / `positioning` / `priority` / `significance` / `computation` / `limitation` / `future-work` / `closing-callback` |
| `support` | 来源文件、定理、日志、引用 key 或审稿意见 |
| `status` | `supported` / `citation-needed` / `needs-proof` / `overclaimed` / `unsupported` / `uncertain` |
| `action` | keep / cite / soften / remove / verify / ask human |

对 `computation` 类型再记录：

| Field | Allowed values |
|---|---|
| `role` | `proof` / `computer-assisted-proof` / `independent-check` / `experiment` / `example-discovery` |
| `protocol` | 来源包中的证明协议、证书或复现说明；没有则写 `none` |

输出为 `writer/claim_evidence_ledger.md`。

## 高风险位置

- Abstract
- Introduction
- 主定理摘要段
- Related work 对比句
- Conclusion
- 含 `new`、`first`、`stronger`、`optimal`、`sharp`、`breakthrough` 等定位或优先权词的句子
- 声称结果“重要、基本、统一、可广泛应用”或声称某方法可复用的句子
- 结尾对全文贡献、适用范围和开放方向的压缩陈述

## 规则

- 不发明引用 key、定理编号、实验结果或作者论断。
- 定理/证明状态与 conjecture、heuristic、empirical evidence 严格分开。
- 未经文献核验，不写优先权、首创性或过强比较声明；能陈述集合包含、条件强弱或数值比较时，优先写可核验关系。
- `significance` 必须指向具体后果、结构解释、方法复用、范围扩展或边界结果；纯评价性形容词不构成证据。
- 外部检索所得文献只有在状态为 `admitted` 后才能作为 `citation` / `positioning` / `priority` 的 support。
- 除非来源包明确提供并认可计算机辅助证明协议，软件输出不能标记为 `proof`，只能按实际角色记录。
- 有 "a citation or proof sketch" ≠ "assumptions and proof obligations 全部覆盖"；后者要交给 [proof-obligation-audit.md](proof-obligation-audit.md)。
- 无证据的论断标注状态，而不是把它改写得像真的一样。
- 修改建议只保留最强且有来源支持的版本。
