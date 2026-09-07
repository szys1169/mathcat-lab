---
name: math-paper-writing
description: "Write or substantially revise submission-style LaTeX mathematical papers from completed results, proofs, source packets, citations, and author feedback. Handles evidence-backed significance, literature positioning and optional user-authorized source discovery, narrative transitions, theorem-preserving exposition, revision tracking, and bilingual synchronization. Writing only: no proof search or formal verification."
---

# 数学论文写作（Math Paper Writing）

把已经完成的结果整理成或修订为可投稿的数学论文（LaTeX）。本 skill 只做写作层：不做形式化验证、不做证明搜索；所有数学内容必须来自输入材料，写作过程不得改变数学。

## 模式

先判断当前任务属于哪一模式；可组合，但必须指定一个主模式：

- `draft`：从来源包起草新论文。
- `revision`：根据现有稿件、作者意见或审稿意见做实质修订。
- `bilingual-sync`：以用户指定的主版本为准，同步另一语言版本。
- `submission-audit`：不重写论文，只做语义、构建与投稿前检查。

文献使用另有两种可组合策略：

- `closed-sources`（默认）：只使用用户提供或项目中已有的文献。
- `active-literature-completion`：仅在用户明确要求搜索、补充或核验引用时启用；按 `$math-related-work` 的检索、核验和准入流程补全来源。授权检索不等于授权购买、登录、上传未公开材料或进行其他外部变更。

## 输入：来源包（Source Packet）

写作前先清点来源包（模板见 [source-packet.md](references/source-packet.md)）。以下素材至少具备核心部分才开工：

- 主结果：定理/引理/命题陈述，含量词、假设与结论；证明笔记或 proof sketch
- 定义清单：对象、记号约定、术语归因
- 文献素材：已收集论文、书目条目（BibTeX）、已知结果的归属记录
- 目标信息：目标期刊/会议、篇幅与风格约束、审稿意见（如有）
- 状态记录：哪些结果已确认、哪些仍是开放义务（若有）

素材不足时先产出来源包清单与缺口表（`Evidence Gaps`），不硬写。

## 输出

核心输出：

- `writer/article_plan.md` — 论文规划（§1 骨架选型 + 各节素材映射）
- `writer/claim_evidence_ledger.md` — 声明台账（每条实质论断 → 来源/证明/引用/不确定性）
- `writer/article_candidate.tex` — 独立可编译的完整论文（模板约定见 [latex-conventions.md](references/latex-conventions.md)）
- `writer/revision_notes.md` — 自检结果与交接说明

条件输出（只在对应情形生成，格式见 [human-revision-and-bilingual-sync.md](references/human-revision-and-bilingual-sync.md)）：

- `writer/human_revision_log.md` — 用户提出实质性修改时，记录原始要求、处理与验证状态。
- `writer/style_decisions.md` — 项目已有术语、记号、语气或结构决策时，记录项目级约定。
- `writer/sync_checklist.md` — 存在两个及以上语言版本时，逐项核对数学与编辑同步。
- `writer/literature_search_log.md` — 启用主动文献补全时，记录检索目标、候选来源、核验状态、准入或排除理由。
- `writer/references.bib` — 项目尚无书目文件或用户要求补全时，仅写入已通过准入的文献。

## 工作流

1. **确定模式与主版本**：明确是起草、修订、双语同步还是投稿审计；多语言任务先指定修订主版本。
2. **清点来源包与文献覆盖**：按 [source-packet.md](references/source-packet.md) 列出核心结果、证明材料、定义、文献、作者决策与缺口；以文献功能覆盖矩阵判断缺失的是原始出处、基础工具、最近结果、方法谱系还是边界文献。
   - `closed-sources` 下把缺失项列为 `Literature Gaps`，不擅自联网补齐。
   - `active-literature-completion` 下调用 `$math-related-work` 的文献补全模式；只有通过正文支撑核验并准入的来源才能进入引用白名单和 BibTeX。
3. **登记作者意见**：在 `revision` 模式下，修改前先把实质意见写入 `human_revision_log.md`，区分 `generalizable` 与 `paper-specific`。
4. **搭骨架和叙事闭环**：按 [paper-skeleton.md](references/paper-skeleton.md) 生成 contribution spine、成果—意义映射、章节地图、章节接口图、结果依赖图与开头—结尾契约；章节由目标期刊、领域和材料决定，不强制独立 Related Work 或 Conclusion。
5. **建声明台账**：按 [claim-evidence-ledger.md](references/claim-evidence-ledger.md) 把每条实质论断（包括成果意义、文献定位和结尾回扣）映射到证明、引用、计算、作者决定或明确不确定性。
6. **审计陈述与接口**：按 [proof-obligation-audit.md](references/proof-obligation-audit.md) 核对假设、量词、依赖、边界与证明接口；按 [notation-formula-audit.md](references/notation-formula-audit.md) 检查首次定义和对象类型。只识别并标注缺口，不补数学。
7. **起草或修订**：引言按 [intro-checklist.md](references/intro-checklist.md) 完成问题、意义、已有工作、本文回答与方法桥接；综述内容需要时委托 `$math-related-work`，但其位置可在引言、背景或独立章节。各节按章节接口图解释其必要性，不把定理和引理机械并列。结尾按开头—结尾契约回扣本文回答、意义和边界；可由末节或末条 remark 承担，不强制独立 Conclusion。
8. **验证修订与同步**：按 [revision-and-review.md](references/revision-and-review.md) 去除过程痕迹、检查结构重复；有多语言版本时按同步表核对定理假设、标签、引用和术语。
9. **编译与提交前检查**：跑 [build-and-submission-audit.md](references/build-and-submission-audit.md)；确认核心声明、用户修改与构建状态后交付。

## 硬规则

- 不验证、不证明新东西；若写作要求数学内容变更（改假设、补证明步骤、判定理是否成立），停下写交接，不擅自改数学。
- 定理、引理和命题的数学陈述必须保持量词、假设、对象类型和结论强度不变；证明允许重组、展开和改善表达，但只能使用来源包已有的数学内容。若展开需要新增论证，标记缺口并交接。
- 不虚构引用、期刊、作者位置、数值结果或证明状态；不确定的论断显式标注（`citation needed` / `needs proof` / `uncertain`），不替读者填坑。
- `closed-sources` 下，载荷引用只来自来源包书目。`active-literature-completion` 下，候选来源须经过元数据核验、正文支撑核验和引用准入后才能成为载荷引用；搜索摘要、搜索片段和二手提及不能单独支撑数学论断。
- 已有结果用作者归因与精确引用标明来源，绝不伪装原创；有限检索不能证明首创性、完备性或“目前最强”。
- 不写 agent 运行历史、内部路径、失败路线、对话痕迹。
- 用户提出的实质修改不能只在最终回复中确认；应记录处理位置、状态和验证方式。项目个案不得直接升级为通用规则。
- 计算结果必须标明其角色是证明、计算机辅助证明、独立核验、实验或例子发现；没有来源包认可的证明协议时，软件输出不能替代正文证明。
- 默认英文研究论文风格（用户指定中文除外）；LaTeX 模板优先用目标期刊/用户指定样式，无指定时用标准 `article` 类 + `amsmath`/`amsthm`。

## 参考

- 输入契约与来源包模板：[references/source-packet.md](references/source-packet.md)
- 论文骨架与依赖图：[references/paper-skeleton.md](references/paper-skeleton.md)
- 声明台账机制：[references/claim-evidence-ledger.md](references/claim-evidence-ledger.md)
- 写作侧义务审计：[references/proof-obligation-audit.md](references/proof-obligation-audit.md)
- 记号与公式可读性审计：[references/notation-formula-audit.md](references/notation-formula-audit.md)
- 修订与评论式审读：[references/revision-and-review.md](references/revision-and-review.md)
- 人工修改台账与双语同步：[references/human-revision-and-bilingual-sync.md](references/human-revision-and-bilingual-sync.md)
- 编译与提交前检查：[references/build-and-submission-audit.md](references/build-and-submission-audit.md)
- 论文骨架与各节写法（四大期刊语料统计）：[references/structure.md](references/structure.md)
- 引言写作模式与 checklist：[references/intro-checklist.md](references/intro-checklist.md)
- LaTeX 环境命名、引用纪律、编译流程：[references/latex-conventions.md](references/latex-conventions.md)
- 引用前人结果与背景定义的规范：[references/citation-and-definition.md](references/citation-and-definition.md)
- 许可证与来源声明：[references/CREDITS.md](references/CREDITS.md)
