# 快速骨架与逻辑架构

在写流畅 prose 之前，先把论文的数学架构摆出来：定义、结果、依赖、证明占位、示例与章节职能都要可见，缺口早暴露。

## Contribution Spine

- Problem（问题）:
- Existing obstacle（已有障碍）:
- Main insight（核心洞察）:
- Main result（主结果）:
- Consequence（推论/应用）:
- Limitation（局限）:

## Contribution–Significance Map

| Result | Direct contribution | Mathematical consequence | Structural/methodological significance | Scope extension | Boundary/limitation | Evidence |
|---|---|---|---|---|---|---|
|  |  |  |  |  |  | theorem / citation / source note |

成果意义必须落到具体的问题推进、结构认识、可复用机制、范围扩展或最优性边界，且有定理、推论、应用或文献比较支撑。没有证据时写 `uncertain`，不用 “important/fundamental/broadly applicable” 等空泛修饰代替。

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

## Section Interface Map

| From | Established input | Remaining gap | Next section's job | Bridge location |
|---|---|---|---|---|
|  |  |  |  | opening / before result / after result / closing |

章节接口描述逻辑信息：前文得到什么、为什么仍不够、下一节补上什么。仅写 “we next prove” 或复述目录不算桥接。

## Opening–Closing Contract

| Opening problem or commitment | Fulfilled at | Consequence/significance | Boundary/limitation | Final callback |
|---|---|---|---|---|
|  | theorem/section |  |  | synthesis / application / optimality / limitation / open question |

结尾不必独立成节，但最后承担论证功能的段落应回答开头的问题，说明取得的成果及其范围。若以反例收尾，明确连接正面定理与不可进一步强化的边界。

## Intro Later List（延后决定项）

以下文字等技术主干稳定后再写，避免引言/摘要承诺超出实际结果：

- Abstract wording:
- Broad positioning:
- Claimed novelty:
- Final conclusion language:

## 骨架纪律

- 依赖不清时，稀疏骨架优于流畅 prose。
- Introduction / Abstract / Conclusion 的声明强度 ≤ 已验证的技术结果和已核验文献比较的强度。
- 不为了故事顺滑添加新数学论断。
- 示例、实验只有在其来源陈述存在后才作为结构证据。
