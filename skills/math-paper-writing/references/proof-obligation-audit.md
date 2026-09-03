# 写作侧义务审计（Proof Obligation Audit）

本检查是**写作评审**，不是自动定理证明：它识别一篇论文"必须兑现哪些证明义务"，并保证 prose 不隐藏义务缺口。证明本身由作者/研究层提供；写作层只标注。

## 义务表

| Field | Meaning |
|---|---|
| `result` | 被审的定理/引理/命题/claim |
| `assumptions` | 量词、定义域、正则性、约束、边界条件 |
| `dependencies` | 定义、前序结果、外部定理、实验 |
| `obligation` | 证明必须建立或验证的内容（存在性/唯一性/界/收敛/情形拆分/边界） |
| `coverage` | `covered` / `partial` / `missing` / `conflicting` / `unclear` |
| `risk` | 保义问题、漏情形、overclaim、引用适配风险、记号风险 |
| `action` | keep / soften / split / add assumption / cite precisely / prove / verify / ask human |

## 工作流

1. 重述结果，使量词、定义域、假设全部可见。
2. 列出证明义务：存在性、唯一性、不变性、收敛性、最优性、界、极限情形、情形拆分、依赖条件。
3. 有源可查时，核对每个被引外部定理的原始假设与结论强度。
4. 将证明覆盖与义务比对；缺失/部分覆盖的标注出来，**不补数学**。
5. 只在保持已验证数学含义的前提下建议措辞修改。

## 外部结果适配表

| Cited result | Needed condition | Paper establishes condition? | Conclusion strength match? | Action |
|---|---|---|---|---|
|  |  | yes / no / unclear | yes / stronger / weaker / unclear |  |

## 边界情形清单

- 退化定义域
- 边界值
- 空集/奇异情形
- 维数与指标约定
- 算法停止条件或失败模式

## 规则

- 不为了匹配贡献而强化定理。
- 不检查证明依赖就弱化或删除假设。
- 经验证据、启发式论证、猜想与形式证明状态分开。
- 语义判断依赖符号/维数时，先跑 [notation-formula-audit.md](notation-formula-audit.md)。
