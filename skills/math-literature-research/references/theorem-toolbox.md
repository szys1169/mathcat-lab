# 候选引理与定理工具箱

## 收集范围

收集可能服务当前问题的定义、直接相关结果、归约引理、特殊情形、分类/结构定理、存在唯一性结果、估计和界、判据、反例/障碍以及相邻领域的方法性命题。

## 每条记录

- `theoremId` 和显示名称；
- `kind`：`definition/lemma/theorem/proposition/corollary/criterion/bound/counterexample/method`；
- 规范化陈述、完整假设、量词、适用对象和结论；
- 来源 ID、BibTeX key、页码/章节/原定理编号；
- `relevance`：`direct/conditional/analogical/background/uncertain`；
- `possibleUse`、前置结果和依赖；
- 证据等级、核验状态、适用风险和建议下一步。

## 边界

- 规范化表述不得强化或弱化原定理；无法保真时保存摘要并标 `statement_needs_verification`。
- 二手引用标 `secondary_source_only`，不得伪装成已核对原始定理。
- “可能用到”不表示当前问题已满足假设。
- 智能体推导的新命题放在单独的 `unverifiedCandidates`，不得进入 `theorems`。
- 建立依赖图时只记录来源明确或逻辑上显式的边；推断边标 `inferred`。

`theorem-toolbox.tex/pdf` 面向人类阅读，`theorem-ledger.json` 和 `theorem-dependency-map.json` 面向 Rethlas、论文写作和论文修改流程。
