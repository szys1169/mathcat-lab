---
name: math-related-work
description: "Write the related-work / background section of a mathematical research paper from verified source material (source packet, provenance ledger, findings, source-theorem files), following top-journal conventions: historical timeline attribution, 'due to X [n]' phrasing, no new claims, real citations only. Use when the user asks to write, draft, or expand the related work, literature review, background, or 综述 section of a math paper, or when a paper-writing pipeline needs the Related Work section."
---

# 数学论文综述写作（Math Related Work）

撰写数学论文的 Related Work / Background / 综述章节。你是写作者，不是证明搜索者：不证明、不验证、不发明数学内容。

## 输入（至少一项存在才开工）

- 已知结果记录 — 带来源标记（proved / attributed / open）的既有结果
- 既有定理陈述 — 已审计或已归因的定理
- 引用白名单 — BibTeX 书目与来源信任记录（存在时）
- 问题背景与本文声称的结果 — 综述要定位的"本文位置"
- 已收集文献 — 论文、书目条目、DOI / arXiv 记录

## 输出

- `related_work.tex` — 可独立成节的 LaTeX（或按用户要求输出段落）
- `related_work_ledger.md` — 每条综述论断 → 引用 key → 归因类型 的台账

## 工作流

1. **读来源标记**。把每个已知结果映射到标记：`proved` 按已确立事实陈述并引用；`attributed` 按"作者声称"陈述并引用；开放问题绝不写成已解决。
2. **验证文献**（见 [provenance.md](references/provenance.md) 的文献验证清单）。核对标题、作者、年份、出处、DOI / arXiv ID；区分 preprint 与正式发表；确认引文确实支撑所挂论断。
3. **选择组织方式**（见 [patterns.md](references/patterns.md)）：历史时间线（默认）、方法流派、结果对比、缺口定位。
4. **起草**。首次载荷使用处引用一次；用归因动词："was proved by X [n]"、"is due to [n]"、"in a recent breakthrough, [n]"。
5. **归因而非占有**。已有结果写成 `\begin{theorem}[{\citet{key}}]` 或 "by a theorem of X [n]"，绝不伪装成本文原创。
6. **自检**（见 [template.md](references/template.md) 清单）：无新论断、无虚构引用、每个 `\cite` 可解析、每个被引结果的适用范围（假设/边界）陈述准确。

## 硬规则

- 不引入新定理、不强化论断、不解决开放问题。
- 载荷引用只来自 ledger；综述性提及可自由补充，但必须是 `refs.bib` 中真实可解析的文献。
- 正文不写 agent 运行历史或内部路径。
- 若任务需要数学决策（判断某个证明是否成立、某结果是否已被解决），停下并交接，不要代答。

## 参考

- 组织模式与语料示例：[references/patterns.md](references/patterns.md)
- 归因纪律、来源标记与文献验证：[references/provenance.md](references/provenance.md)
- LaTeX 模板与台账格式：[references/template.md](references/template.md)
- 许可证与来源声明：[references/CREDITS.md](references/CREDITS.md)
