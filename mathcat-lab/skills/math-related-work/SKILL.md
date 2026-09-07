---
name: math-related-work
description: Write or revise the Related Work, Background, or literature-positioning section of a mathematical paper from verified sources. Reuse math-literature-research ledgers when available, compare prior results with the present contribution, audit gap and novelty wording, and deliver cited LaTeX/BibTeX plus evidence ledgers. Use for 数学论文相关工作、研究背景、文献定位或已有工作对比；not for proving theorems or conducting unrestricted topic surveys.
---

# 数学论文相关工作（Math Related Work）

围绕“本文贡献相对于已有工作的准确位置”生成可直接审阅和合并的数学论文综述。复用文献调研的证据层，不重复建立平行的文献数据库。

来源台账、BibTeX、去重和跨任务复用统一使用同仓库 `$math-literature-core`；本 Skill 只增加贡献画像、比较矩阵、gap 定位和论文写作层。

## 输入门禁

至少取得一类本文材料：论文草稿、主定理/成果包，或用户确认的贡献摘要。先提取研究对象、问题、主结果、方法、完整假设、适用范围和用户声称的创新点，写入 `contribution-profile.json`。无法确定本文贡献时先询问，不写泛化综述。

按 [routing-and-reuse.md](references/routing-and-reuse.md) 选择路径：

- **快速路径**：现有文献调研台账足以支撑比较，直接写作。
- **补充路径**：只为明确的证据缺口做定向检索。
- **冷启动路径**：调用 `$math-literature-research` 建立最小可用来源包；若不可用，停止载荷写作并报告缺口。

## 工作流

1. 建立贡献画像；把“作者声称”与已核验事实分开。
2. 发现并复用 `source-ledger.json`、`claim-source-ledger.json`、`theorem-ledger.json`、时间线、BibTeX 和合法归档论文；按 `$math-literature-core` 契约记录到 `reused-source-manifest.json`，并冻结本任务实际使用的来源台账快照为 `source-ledger.json`，不得擅自提升证据等级。
3. 按 [evidence-contracts.md](references/evidence-contracts.md) 建立比较矩阵、gap 台账和综述论断台账。比较对象、假设、结论、方法、范围和局限，不从标题推断数学关系。
4. 对缺失证据执行最小定向补充；不得把“未检索到”写成“不存在”。
5. 按 [writing-and-integration.md](references/writing-and-integration.md) 选择历史时间线、方法流派、结果对比或 gap 定位结构，生成独立节和引言内联版。
6. 合并并去重 BibTeX；每个载荷引用必须同时在来源台账、综述论断台账和 `.bib` 中可解析。
7. 编译独立预览 PDF，运行确定性校验。默认不修改原论文；只有用户明确要求合并时才创建版本化副本和 `integration.diff`。
8. 成功、部分完成、失败、停止或超时都生成 `related-work-task-report.md`。

## 必需交付

```text
调研/相关工作/<task-id>/
├─ contribution-profile.json
├─ reused-source-manifest.json
├─ source-ledger.json
├─ targeted-search-log.jsonl
├─ literature-comparison-matrix.json
├─ gap-positioning-ledger.json
├─ related-work-claim-ledger.json
└─ evidence-gaps.md

论文/相关工作/<task-id>/
├─ related-work.tex
├─ related-work-inline.tex
├─ related-work-preview.pdf
├─ related-work.bib
└─ integration.diff                 # 仅在请求合并时

成果/相关工作/<task-id>/
├─ related-work-summary.md
└─ related-work-task-report.md
```

使用 [related-work-section-template.tex](assets/related-work-section-template.tex) 和 [related-work-inline-template.tex](assets/related-work-inline-template.tex) 作为起点，并跟随用户论文的语言、宏、引用风格和目标期刊格式。

## 硬规则

- 不证明、不补证明、不验证新数学结果；数学正确性问题交给论文修改/研究流程。
- 不宣布绝对原创。`首次`、`唯一`、`最强`、`此前无人研究` 等表述只有在强证据和明确检索边界下才能保留。
- 已有结果保留假设、量词、对象和边界；不可比结果不得强行排序。
- `source-ledger.json` 只保存外部文献；本地稿件结果通过 `contribution-profile.json` 的 `resultId` 和各台账的 `presentWorkResultIds` 引用。
- `verified_gap` 必须有可核验来源与明确检索覆盖；其余使用保守措辞。
- 正文不出现智能体运行历史、内部路径、API 信息或未公开审计结论。
- 付费墙不绕过；预印本与正式版本并存时说明版本关系并优先权威版本。
- 分发时保留 [来源与许可证声明](references/CREDITS.md)。

## 确定性工具

```bash
node scripts/assess-route.mjs contribution-profile.json source-ledger.json reused-source-manifest.json
node scripts/merge-bibliographies.mjs related-work.bib input-a.bib input-b.bib
node ../math-literature-core/scripts/validate-evidence-package.mjs source-ledger.json related-work.bib
node scripts/validate-evidence.mjs <research-root> <related-work.bib>
node scripts/validate-delivery.mjs <project-root> <task-id>
```
