# 数学门禁

- `PASS`：没有未解决的 `blocking_math` 或 `high` 数学问题；可进入完整写作修改。
- `PASS_WITH_WARNINGS`：只有中低风险、低置信度或非阻断问题；仅执行 `meaning_safe` 修改。
- `RESEARCH_REQUIRED`：存在证明缺口、反例风险、循环依赖或待确认的必要假设；停止写作实质修改，输出研究交接。
- `HUMAN_REQUIRED`：需要改变定理、假设、贡献边界或作者意图；等待人类决定。
- `FAIL`：论文无法解析、审计器失败或输入不足。

解析不到明确门禁时使用 `HUMAN_REQUIRED`。不得从“Rethlas 已生成 verified blueprint”推导论文整体为 `PASS`；门禁必须来自论文审计结论。
