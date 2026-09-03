# 快速骨架与逻辑架构

在写流畅 prose 之前，先把论文的数学架构摆出来：定义、结果、依赖、证明占位、示例与章节职能都要可见，缺口早暴露。

## Contribution Spine

- Problem（问题）:
- Existing obstacle（已有障碍）:
- Main insight（核心洞察）:
- Main result（主结果）:
- Consequence（推论/应用）:
- Limitation（局限）:

## Reader And Venue Brief

- Expected reader background（读者背景）:
- Concepts to define（需要定义的概念）:
- Concepts safe to cite（可直接引用的概念）:
- Venue constraints（期刊/会议约束）:

## Section Map

| Section | Job（章节职能） | Required inputs | Open gaps |
|---|---|---|---|
| Introduction | 动机 + 陈述贡献，不夸大 | claim ledger、主定理 |  |
| Background | 记号与引用工具 | 定义、文献 |  |
| Main results | 陈述假设与结果 | 定理陈述 |  |
| Proofs | 兑现义务 | 证明笔记、依赖 |  |
| Examples / Applications | 落地论断或说明适用范围 | 例子、应用材料 |  |
| Discussion | 局限与下一步 | 支持审计 |  |

## Result Dependency Map

| Node | Type | Depends on | Used by | Status |
|---|---|---|---|---|
|  | definition / lemma / theorem / example |  |  | ready / partial / missing |

## Intro Later List（延后决定项）

以下文字等技术主干稳定后再写，避免引言/摘要承诺超出实际结果：

- Abstract wording:
- Broad positioning:
- Claimed novelty:
- Final conclusion language:

## 骨架纪律

- 依赖不清时，稀疏骨架优于流畅 prose。
- Introduction / Abstract / Conclusion 的声明强度 ≤ 已验证的技术结果强度。
- 不为了故事顺滑添加新数学论断。
- 示例、实验只有在其来源陈述存在后才作为结构证据。
