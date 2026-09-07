# 比较、Gap 与论断台账契约

## `literature-comparison-matrix.json`

```json
{
  "schemaVersion": "0.1",
  "taskId": "task-id",
  "comparisons": [{
    "comparisonId": "CMP-001",
    "priorWork": "文献中被比较的精确结果",
    "currentWork": "本文对应结果",
    "dimension": "assumptions",
    "relation": "weaker_assumptions",
    "sourceIds": ["SRC-001"],
    "presentWorkResultIds": ["PW-001"],
    "evidenceLevel": "full_text_checked",
    "caveat": "仅比较相同系数域下的结论"
  }]
}
```

`dimension` 使用 `object | assumptions | conclusion | method | scope | limitation`。`relation` 使用 `same | extends | specializes | improves | weaker_assumptions | stronger_assumptions | complementary | contrasts | incomparable | unknown`。`unknown` 不得被正文改写为确定关系。

`sourceIds` 只指向统一外部文献台账，`presentWorkResultIds` 只指向 `contribution-profile.json` 的 `mainResults[].resultId`。不得为了比较方便把本地论文伪装成来源记录。

## `gap-positioning-ledger.json`

每项包含 `gapId`、`statement`、`status`、`evidenceSourceIds`、`presentWorkResultIds`、`searchCoverage`、`cautiousWording` 和 `riskNote`。外部证据与本文结果必须分别指向来源台账和贡献画像。

状态：

- `verified_gap`：原文或充分检索直接支持，仍须写检索边界；
- `supported_but_incomplete`：证据支持但覆盖不完整；
- `author_claim_only`：来自作者声明；
- `unknown`：无法判断；
- `contradicted`：已有来源可能覆盖该声称。

只有 `verified_gap` 可以使用明确 gap 表述。其他状态必须采用 `cautiousWording`；`contradicted` 不得进入正文的创新性主张。

## `related-work-claim-ledger.json`

```json
{
  "schemaVersion": "0.1",
  "taskId": "task-id",
  "claims": [{
    "claimId": "RWC-001",
    "text": "X 在假设 H 下证明了结论 C。",
    "attributionType": "cite_as_existing",
    "citationKeys": ["Author2024"],
    "sourceIds": ["SRC-001"],
    "presentWorkResultIds": [],
    "comparisonIds": ["CMP-001"],
    "confidence": "verified"
  }]
}
```

`attributionType` 使用 `cite_as_existing | attributed | open_problem | present_work | transition`。前三类必须有引用；`present_work` 只描述贡献画像中已有的本文结果；`transition` 不承载数学或历史事实。

`present_work` 论断必须列出至少一个 `presentWorkResultIds`，且不得以本地稿件构造虚假的外部 `sourceId`。

## 证据原则

- 来源 ID 必须存在于复用或补充后的 `source-ledger.json`。
- 引用 key 必须存在于最终 `related-work.bib`。
- 历史优先权、最强结果和开放状态属于高风险论断，优先核对原始论文和权威元数据。
- 二手来源可导航，不能无标记地替代原始定理或优先权证据。
