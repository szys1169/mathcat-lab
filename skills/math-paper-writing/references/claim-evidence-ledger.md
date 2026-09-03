# 声明台账（Claim–Evidence Ledger）

在打磨 prose 之前，先让每条实质论断可追溯到证明、引用、实验或明确不确定性。摘要是高风险区：压缩性文字最容易夸大结果。

## 台账表

| Field | Meaning |
|---|---|
| `claim` | 论文中的确切或转述论断 |
| `location` | 节、段、定理、公式或图 |
| `type` | `theorem` / `proof` / `citation` / `positioning` / `limitation` / `future-work` |
| `support` | 来源文件、定理、日志、引用 key 或审稿意见 |
| `status` | `supported` / `citation-needed` / `needs-proof` / `overclaimed` / `unsupported` / `uncertain` |
| `action` | keep / cite / soften / remove / verify / ask human |

输出为 `writer/claim_evidence_ledger.md`。

## 高风险位置

- Abstract
- Introduction
- 主定理摘要段
- Related work 对比句
- Conclusion

## 规则

- 不发明引用 key、定理编号、实验结果或作者论断。
- 定理/证明状态与 conjecture、heuristic、empirical evidence 严格分开。
- 有 "a citation or proof sketch" ≠ "assumptions and proof obligations 全部覆盖"；后者要交给 [proof-obligation-audit.md](proof-obligation-audit.md)。
- 无证据的论断标注状态，而不是把它改写得像真的一样。
- 修改建议只保留最强且有来源支持的版本。
