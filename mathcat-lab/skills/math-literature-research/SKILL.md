---
name: math-literature-research
description: Research a specific mathematical problem by reconstructing its origin, motivation, historical milestones, related papers, current status, and potentially useful lemmas and theorems. Search and verify sources, archive legally accessible papers, and deliver a cited LaTeX/PDF survey plus a theorem toolbox. Use for 数学文献调研, 问题历史, 研究现状, literature surveys, or finding prior results for a concrete problem. If the request is only a broad field, clarify and narrow the scope before searching.
---

# 数学文献调研（Math Literature Research）

围绕一个具体数学问题，重建“问题如何提出—为何重要—怎样发展—目前到了哪里”，并交付可追溯的文献综述、论文归档和候选引理定理工具箱。

来源去重、来源台账、BibTeX 和跨任务复用遵循同仓库 `$math-literature-core` 的统一证据包契约；本 Skill 负责检索、历史重建、定理抽取和综述交付，不另建平行来源数据库。

## 先做范围门禁

按 [scoping-and-clarification.md](references/scoping-and-clarification.md) 判断问题是否具体。只有宽泛领域、缺少研究对象或目标时，先反问用户并给出 2–5 个可选聚焦方向；不要立即做大规模检索。范围明确后写入 `research-scope.json`。

## 工作流

1. **范围与策略**：明确问题的标准表述、变体、时间/语言边界和用户目的，生成 `search-strategy.md`。
2. **检索与归档**：按 [search-screen-download.md](references/search-screen-download.md) 检索原始论文、经典结果、突破、变体、反例、综述和最新进展；去重并记录完整检索日志。只下载合法可访问的 PDF。
3. **历史与动机**：按 [history-and-motivation.md](references/history-and-motivation.md) 核实先导结果、正式提出来源、数学动机、关键里程碑和当前状态。
4. **证据台账**：按 [provenance-ledgers.md](references/provenance-ledgers.md) 建立来源、声明和定理台账。历史归属、定理和当前状态必须可追溯到原文位置或明确标记证据等级。
5. **候选工具箱**：按 [theorem-toolbox.md](references/theorem-toolbox.md) 提取可能有用的定义、引理、定理、判据、估计、反例和方法性命题；保留全部假设，不把“可能适用”写成“已经适用”。
6. **撰写与构建**：使用 `assets/literature-review-template.tex` 和 `assets/theorem-toolbox-template.tex`，按 [latex-delivery.md](references/latex-delivery.md) 生成并编译两套 `.tex/.pdf`。
7. **验收与报告**：运行确定性校验脚本。成功、部分完成、失败、停止或超时都生成 `literature-research-task-report.md`。

## 必需交付

```text
文献/专题/<problem-slug>/
├─ 原始论文/ 经典结果/ 关键进展/ 最新论文/ 综述/ 待获取/
├─ metadata/
├─ selected-bibliography.bib
└─ README.md

调研/文献调研/<task-id>/
├─ research-scope.json
├─ search-strategy.md
├─ search-log.jsonl
├─ screening-log.json
├─ source-ledger.json
├─ claim-source-ledger.json
├─ theorem-ledger.json
├─ theorem-dependency-map.json
├─ historical-timeline.json
├─ candidate-gaps.md
└─ evidence-gaps.md

成果/文献调研/<task-id>/
├─ literature-review.tex
├─ literature-review.pdf
├─ theorem-toolbox.tex
├─ theorem-toolbox.pdf
├─ selected-bibliography.bib
└─ literature-research-task-report.md
```

## 硬规则

- 优先原始论文和权威元数据；二手综述只作为导航，不能无标记地替代原始来源。
- 不虚构论文、作者、年份、DOI、arXiv ID、BibTeX、定理编号、页码、历史归属或当前解决状态。
- “没有检索到证明”不等于“问题仍然开放”；当前状态必须说明检索范围和截止时间。
- 付费墙论文不绕过访问限制；保留 DOI/链接和 `待获取` 记录，并标明摘要级或二手证据。
- 下载文件与 `source-ledger.json` 一一对应；记录来源 URL、获取时间、版本和文件哈希。
- 定理陈述保留原始假设、量词和适用对象。智能体新推导的命题只能进入单独的 `unverifiedCandidates`，不能冒充文献结果。
- 文献综述与定理工具箱必须引用同一份经过校验的 BibTeX 和台账。
- 对话内先给精炼结论和成果路径；完整证据保存在本地交付物中。

## 确定性校验

```bash
node scripts/deduplicate-sources.mjs source-ledger.json source-ledger.deduped.json
node scripts/validate-source-ledger.mjs source-ledger.json
node ../math-literature-core/scripts/validate-evidence-package.mjs source-ledger.json selected-bibliography.bib
node scripts/validate-theorem-ledger.mjs theorem-ledger.json source-ledger.json
node scripts/validate-delivery.mjs <project-root> <task-id>
```
