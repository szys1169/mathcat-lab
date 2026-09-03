# 交付与验收

候选版本目录至少包含：

- `source/`：完整修订 LaTeX 工程；
- `paper.pdf`：成功编译的候选论文；
- `revision.diff`：原稿与候选稿差异；
- `revision-log.json`：逐项意见、决定、理由、风险和文件位置；
- `writing-audit.md`：结构、符号、引用和构建检查；
- `response-to-reviewers.md`：输入含审稿意见时必需。

成果目录必须包含：

- `gap-ledger.json`；
- `mathematical-correctness-report.md`；
- `paper-revision-task-report.md`。

只有候选 `.tex`、`.pdf`、diff、revision log、gap ledger 和最终报告齐全，且没有未解决的数学阻断项，任务才可标记 `completed`。编译失败、需要研究或需要人类决定时为 `partial`。原稿永远不被覆盖。
