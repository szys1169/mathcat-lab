# 统一文献证据包契约

## 唯一来源白名单

`source-ledger.json` 是文献综述、定理工具箱、相关工作和后续论文写作共同使用的来源白名单。上层任务可以冻结一个只含实际使用来源的副本，但不得修改原始来源的书目信息、证据等级或核验记录。

核心字段包括：

- 包级：`schemaVersion`、`taskId`、`searchCutoff`、`sources[]`；
- 来源级：`sourceId`、`title`、`authors`、`year`、`bibtexKey`、`sourceType`、`screeningStatus`、`evidenceLevel`；
- 可定位标识：至少一个 DOI、arXiv ID 或 URL；
- 已下载全文：还需 `localPath`、SHA-256 和 `retrievedAt`。

证据等级统一为 `full_text_checked`、`primary_metadata_only`、`abstract_only`、`secondary_source_only` 或 `unverified`。上层流程不得擅自提升等级。

## BibTeX 契约

- 正文中的每个引用 key 必须存在于交付 BibTeX；
- 引用既有工作的论断必须同时解析到来源台账；
- DOI 相同的版本可以去重并保留版本关系；
- 同 key 不同条目属于冲突，必须人工选择或更名，不能静默覆盖；
- `screeningStatus=included` 的来源应在实际交付书目中可解析，除非上层任务明确冻结了更小的使用子集。

## 复用路径

1. **冻结**：记录输入文献包路径、任务 ID、检索截止日期和校验结果。
2. **评估覆盖**：分别检查研究对象、最接近结果、方法和当前状态。
3. **快速复用**：覆盖充分时不重新搜索，只生成任务专用的来源台账快照。
4. **增量补充**：覆盖不足时仅检索明确缺口，新来源通过核心校验后再并入。
5. **冷启动**：没有可用来源包时，交给 `$math-literature-research` 建立最小包。

公开状态始终附检索截止日期。“未检索到”不能改写为“不存在”。本地论文中的候选成果不能自动升级为外部已验证文献。
