# 来源、声明与证据台账

## Source Ledger

每条文献记录：稳定 ID、标题、作者、年份、出版物、DOI、arXiv ID、URL、BibTeX key、来源数据库、文献类型、版本关系、筛选状态、相关性、证据等级、全文状态、本地路径、SHA-256、获取时间和备注。

证据等级：`full_text_checked`、`primary_metadata_only`、`abstract_only`、`secondary_source_only` 或 `unverified`。

## Claim–Source Ledger

每个历史或数学主张记录：`claimId`、文本、类型、支持来源 ID、反对/冲突来源 ID、页码/章节/定理号、证据等级、核验状态和不确定性。

## 冲突处理

不同来源对提出时间、定理范围或解决状态不一致时，不替用户私自消解；并列证据，说明版本差异和需要人工核对之处。

来源台账是文献综述、BibTeX、定理工具箱和后续 `math-related-work` 的唯一引用白名单。字段、证据等级、去重和 BibTeX 解析以同仓库 `math-literature-core/references/evidence-package-contract.md` 为准；这里仅补充文献调研专用的声明与定理定位要求。
