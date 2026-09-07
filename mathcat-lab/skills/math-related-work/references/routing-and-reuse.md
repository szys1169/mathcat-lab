# 输入、路由与文献调研复用

来源包结构、证据等级、BibTeX 与冻结规则以同仓库 `math-literature-core/references/evidence-package-contract.md` 为统一契约；本文件只规定相关工作的贡献画像和三路径路由。

## 贡献画像

从论文草稿、成果包或用户说明中提取：

- `researchObject`：研究对象与基础设定；
- `problem`：本文处理的问题；
- `mainResults[]`：结果、假设、范围和 proof status；
- `methods[]`：实际使用的方法，不根据关键词猜测；
- `authorNoveltyClaims[]`：作者希望表达的创新点，初始状态只能是 `author_claim_only`；
- `targetLanguage`、`targetVenue`、`preferredForm`（独立节/引言内联/两者）。

主定理、摘要和引言相互冲突时，不替作者选择；记录冲突并请求确认。

## 来源发现顺序

1. 用户指定的来源台账和 BibTeX；
2. `调研/文献调研/*/` 中最近且通过校验的来源、声明和定理台账；
3. `成果/文献调研/*/` 的综述、工具箱和书目；
4. `文献/专题/` 中有来源台账对应项的论文；
5. 论文草稿自带参考文献，仅作为待核验输入。

不要把平台日志、模型输出缓存或没有来源记录的 PDF 自动提升为可信来源。

## 三条运行路径

`reused-source-manifest.json` 应包含 `coverage`，至少记录 `researchObject`、`closestResults`、`methods` 和 `currentStatus` 四项是否已被核验来源覆盖，并以 `conflicts[]` 记录版本或论断冲突。运行 `scripts/assess-route.mjs` 得到一致的路由建议。

### 快速路径

来源覆盖本文的研究对象、最接近结果、主要方法和当前状态，且关键比较都有全文级或权威元数据级证据。冻结本任务实际使用的合并 `source-ledger.json`，但不复制论文归档。

### 补充路径

先把缺口写成具体检索问题，例如“是否已有工作把定理 A 从特征零推广到正特征”，再调用文献调研的搜索、去重、下载和来源核验组件。只追加新来源，并在 `targeted-search-log.jsonl` 记录查询、时间、命中和停止原因。

### 冷启动路径

把本文问题和贡献画像转成一个具体文献调研任务，调用 `$math-literature-research`。只要求能支撑相关工作写作的最小来源包，不要求无边界扩展成领域百科。范围仍不清楚时询问用户。

## 停止条件

- 所有准备写入正文的已有工作论断均有来源；
- 最接近工作的比较维度已覆盖；
- 仍无法核验的 gap 已降级为保守措辞并写入 `evidence-gaps.md`；
- 达到用户时间/费用上限时停止并交付部分结果。
