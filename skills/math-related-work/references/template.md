# Related Work 节 LaTeX 模板与台账

## LaTeX 模板（独立节）

```tex
\section{Background and Related Work}
% 1. 领域现状与动机（模式 D 缺口定位）
The problem of \ldots has a long history. The first result in this direction is
due to \citet{keyA}, who proved that \ldots
% 2. 时间线推进（模式 A）
This was later extended by \citet{keyB} to the case \ldots, and in a recent
breakthrough \citet{keyC} resolved the case \ldots
% 3. 方法对比（模式 B/C）
In contrast to the approach of \citet{keyB}, which relies on \ldots, the present
work is based on \ldots
% 4. 缺口与本文位置
Despite these advances, the following question remains open: \ldots
% 5. 相关方向（可选）
For related questions in \ldots, we refer the reader to \citet{keyD,keyE} and
the references therein.
```

内联版（不设独立节时）：把上述内容压缩为引言中的 2–4 段，按时间线归因，主定理前结束。

## 台账格式（related_work_ledger.md）

```markdown
| # | 综述论断 | 引用 key | 归因类型 | 来源文件 |
|---|----------|----------|----------|----------|
| 1 | 该问题最早由 X 提出 | keyA | cite-as-existing | findings.md F1 |
| 2 | Y 证明了特例 e=3 | keyB | borrowed (discharged) | routes/source_theorem_01.md |
| 3 | 一般情形仍开放 | — | open problem | findings.md F1 |
```

## 自检清单

- [ ] 无新定理、无强化论断、无"解决了开放问题"的表述
- [ ] 每个 `\cite` key 在 `refs.bib` 中可解析
- [ ] 每条载荷论断在台账中有来源
- [ ] 被引结果的假设/适用范围准确
- [ ] 已有结果全部以归因形式出现（`[{\citet{key}}]` 或 "due to X [n]"）
- [ ] 正文无 agent 历史、无内部路径
