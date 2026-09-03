---
name: math-paper-writing
description: "Turn completed mathematical results (theorems, lemmas, proofs, findings, source packets) into a complete, submission-style LaTeX research paper with Introduction, Notation/Definitions, Main Results, Proofs, Related Work, and Conclusion. Writing only: no formal verification, no proof search. Use when the user asks to write a math paper from finished results, organize completed results into a paper, fill in definitions/introduction/related work/conclusion, or extend an existing draft into a full article."
---

# 数学论文写作（Math Paper Writing）

把已经完成的结果整理成一篇完整的数学论文（LaTeX）。本 skill 只做写作层：不做形式化验证、不做证明搜索；所有数学内容必须来自输入材料，写作过程不得改变数学。

## 输入：来源包（Source Packet）

写作前先清点来源包（模板见 [source-packet.md](references/source-packet.md)）。以下素材至少具备核心部分才开工：

- 主结果：定理/引理/命题陈述，含量词、假设与结论；证明笔记或 proof sketch
- 定义清单：对象、记号约定、术语归因
- 文献素材：已收集论文、书目条目（BibTeX）、已知结果的归属记录
- 目标信息：目标期刊/会议、篇幅与风格约束、审稿意见（如有）
- 状态记录：哪些结果已确认、哪些仍是开放义务（若有）

素材不足时先产出来源包清单与缺口表（`Evidence Gaps`），不硬写。

## 输出

- `writer/article_plan.md` — 论文规划（§1 骨架选型 + 各节素材映射）
- `writer/claim_evidence_ledger.md` — 声明台账（每条实质论断 → 来源/证明/引用/不确定性）
- `writer/article_candidate.tex` — 独立可编译的完整论文（模板约定见 [latex-conventions.md](references/latex-conventions.md)）
- `writer/revision_notes.md` — 自检结果与交接说明

## 工作流

1. **清点来源包**：按 [source-packet.md](references/source-packet.md) 列出核心结果、素材与缺口。
2. **搭骨架**：按 [paper-skeleton.md](references/paper-skeleton.md) 生成 contribution spine、章节地图与结果依赖图；缺口显式标注，不用流畅 prose 掩盖。
3. **建声明台账**：按 [claim-evidence-ledger.md](references/claim-evidence-ledger.md) 把每条实质论断映射到证明、引用、实验或明确不确定性。
4. **义务审计（写作侧）**：按 [proof-obligation-audit.md](references/proof-obligation-audit.md) 核对假设、量词、依赖与边界；只识别并标注义务，不补证明。
5. **起草各节**：定义节 ← 定义清单；引言节 ← [intro-checklist.md](references/intro-checklist.md)；主结果与证明 ← 来源包（一字不改）；综述节 ← 委托 `$math-related-work`（同一来源包）；结尾节 ← 语料惯例（见 [structure.md](references/structure.md)）。
6. **修订**：按 [revision-and-review.md](references/revision-and-review.md) 以评论模式诊断逻辑流/记号/编辑效果，再按简化规则收紧 prose；不做无来源依据的"润色"。
7. **编译与提交前检查**：跑 [build-and-submission-audit.md](references/build-and-submission-audit.md)；编译通过、引用可解析后交付。

## 硬规则

- 不验证、不证明新东西；若写作要求数学内容变更（改假设、补证明步骤、判定理是否成立），停下写交接，不擅自改数学。
- 定理/引理陈述与输入完全一致；不强化、不弱化。
- 不虚构引用、期刊、作者位置、数值结果或证明状态；不确定的论断显式标注（`citation needed` / `needs proof` / `uncertain`），不替读者填坑。
- 载荷引用只来自来源包书目（BibTeX）；已有结果写成 `[{\citet{key}}]` 归因形式，绝不伪装原创。
- 不写 agent 运行历史、内部路径、失败路线、对话痕迹。
- 默认英文研究论文风格（用户指定中文除外）；LaTeX 模板优先用目标期刊/用户指定样式，无指定时用标准 `article` 类 + `amsmath`/`amsthm`。

## 参考

- 输入契约与来源包模板：[references/source-packet.md](references/source-packet.md)
- 论文骨架与依赖图：[references/paper-skeleton.md](references/paper-skeleton.md)
- 声明台账机制：[references/claim-evidence-ledger.md](references/claim-evidence-ledger.md)
- 写作侧义务审计：[references/proof-obligation-audit.md](references/proof-obligation-audit.md)
- 记号与公式可读性审计：[references/notation-formula-audit.md](references/notation-formula-audit.md)
- 修订与评论式审读：[references/revision-and-review.md](references/revision-and-review.md)
- 编译与提交前检查：[references/build-and-submission-audit.md](references/build-and-submission-audit.md)
- 论文骨架与各节写法（四大期刊语料统计）：[references/structure.md](references/structure.md)
- 引言写作模式与 checklist：[references/intro-checklist.md](references/intro-checklist.md)
- LaTeX 环境命名、引用纪律、编译流程：[references/latex-conventions.md](references/latex-conventions.md)
- 引用前人结果与背景定义的规范：[references/citation-and-definition.md](references/citation-and-definition.md)
- 许可证与来源声明：[references/CREDITS.md](references/CREDITS.md)
