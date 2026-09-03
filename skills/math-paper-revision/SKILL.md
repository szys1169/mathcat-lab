---
name: math-paper-revision
description: Audit an existing mathematical manuscript for incorrect claims, missing assumptions, proof gaps, and unsupported novelty, then create a versioned LaTeX revision that fixes writing issues without silently changing mathematics. Use for 论文修改, mathematical manuscript review, referee-response revisions, proof-gap audits, or revision after reviewer comments. Not for writing a new paper from results or searching for a new proof.
---

# 数学论文修改（Math Paper Revision）

先审数学，再改文字。输入必须包含现有论文；原稿始终只读，每次运行创建独立候选版本。

## 模式

- `full`（默认）：数学审计 → 门禁 → 写作修改。
- `audit_only`：只输出主题、正确性和 gap 审计。
- `writing_only`：仅在用户明确要求且已有可信数学审计时使用。

## 必需工作流

1. 定位论文主文件、审稿意见、参考文献和补充材料。找不到论文主文件时停止，生成失败报告，不凭空新写论文。
2. 为原稿建立只读快照。按 [mathematical-audit.md](references/mathematical-audit.md) 准备审计包并调用平台提供的 Rethlas 审计适配器；独立使用时，读取已有 Rethlas 审计产物，不伪造一次调用。
3. 按 [gap-ledger.md](references/gap-ledger.md) 记录每个问题及证据，并应用 [gates.md](references/gates.md) 的数学门禁。
4. 仅当门禁允许时，按 [writing-revision.md](references/writing-revision.md) 修改候选版本。复用 `$math-paper-writing` 的来源包、声明台账、proof-obligation audit、revision review 和构建检查，但不开展证明搜索。
5. 编译候选 LaTeX，生成源码差异、逐条意见处理记录和最终报告。按 [delivery-contract.md](references/delivery-contract.md) 验收。

## 不可违反的边界

- Rethlas 的自然语言验证不是 Lean 形式化证明，也不是人类同行评审；准确记录验证等级。
- 没有系统文献检索证据时，只能标记 `novelty_overlap_risk`，不得宣称绝对创新。
- 自动修改只限 `meaning_safe`：表达、结构、交叉引用、格式及不改变含义的符号统一。
- 不自动修改定理结论、关键假设、证明实质、数据、证明状态或未经核实的引用。
- 数学阻断项产生 `research_required` 或 `human_required`；不得用润色隐藏 gap。
- 不覆盖原稿，不把失败候选设为 current，不在成果中泄露 API Key、Token 或私有运行日志。
- `completed`、`partial`、`failed`、`stopped`、`timed_out` 都必须生成 `paper-revision-task-report.md`。

## 目录

```text
调研/论文审计/<revision-id>/
论文/versions/<revision-id>/original/
论文/versions/<revision-id>/source/
成果/论文修改/<revision-id>/
```

需要独立校验时运行：

```bash
node scripts/validate-gap-ledger.mjs <gap-ledger.json>
node scripts/create-revision-diff.mjs <original.tex> <revised.tex> <revision.diff>
node scripts/validate-deliverables.mjs <revision-root>
```
