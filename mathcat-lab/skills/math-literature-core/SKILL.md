---
name: math-literature-core
description: Validate, normalize, deduplicate, and reuse mathematical literature evidence packages shared by literature surveys and related-work writing. Use for source-ledger, BibTeX, provenance, or evidence-package operations; not as the user-facing workflow for conducting a survey or drafting a paper section.
---

# 数学文献核心（Math Literature Core）

为 `$math-literature-research` 与 `$math-related-work` 提供同一套来源、引用和证据契约。它是共享能力层，不替代两个面向用户的 Skill。

## 核心职责

- 规范化并去重 DOI、arXiv 和标题级来源记录；
- 校验 `source-ledger.json` 的来源、证据等级、下载状态与哈希；
- 解析、合并和去重 BibTeX，拒绝同 key 不同条目的静默覆盖；
- 校验被纳入来源与 BibTeX 的可解析关系；
- 规定文献包的复用、冻结和增量扩展方式。

涉及来源包格式或跨 Skill 复用时，读取 [evidence-package-contract.md](references/evidence-package-contract.md)。

## 使用边界

- 用户要调查一个数学问题时，使用 `$math-literature-research`。
- 用户要为已有论文定位贡献并撰写相关工作时，使用 `$math-related-work`。
- 不在核心层搜索网页、下载论文、撰写综述或判断论文创新性。
- 上层 Skill 可以增加专用台账，但不得建立与核心 `source-ledger.json` 平行且互不兼容的来源数据库。

## 确定性工具

```bash
node scripts/deduplicate-sources.mjs source-ledger.json source-ledger.deduped.json
node scripts/validate-source-ledger.mjs source-ledger.json
node scripts/validate-evidence-package.mjs source-ledger.json selected-bibliography.bib
node scripts/bibtex.mjs merged.bib input-a.bib input-b.bib
```
