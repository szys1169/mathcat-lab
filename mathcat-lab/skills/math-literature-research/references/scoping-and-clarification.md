# 范围门禁与反问

## 可直接开始

问题应至少明确下列三项中的两项：数学对象；目标性质、猜想或分类问题；条件、参数范围或具体研究语境。还应能写出一句不会覆盖整个学科的标准问题表述。专名猜想、明确论文中的问题或带条件的分类问题通常可直接开始。

## 必须先反问

“调研代数几何”“总结数论未解决问题”“查人工智能与数学论文”等属于宽泛范围。此时暂停检索，提出至多三个短问题，优先询问：

1. 研究对象或具体猜想；
2. 关注目标（历史、方法、最新进展、开放问题）；
3. 时间、语言或子领域边界。

同时根据用户输入或工作区材料提供 2–5 个互斥、可执行的聚焦选项。用户确认前只生成范围建议，不下载论文。

## `research-scope.json`

必需字段：`problemStatement`、`mathematicalObjects`、`targetQuestion`、`conditions`、`includedVariants`、`excludedTopics`、`timeRange`、`languages`、`researchGoal`、`scopeStatus`。`scopeStatus` 只能是 `ready` 或 `needs_clarification`。
