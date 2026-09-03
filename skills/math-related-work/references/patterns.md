# 数学综述组织模式（语料：13 篇四大期刊论文）

四大期刊论文几乎不设独立 "Related Work" 节。综述以三种形态出现：

1. **引言内联历史综述**（最常见）：按时间线归因。
2. **独立背景节**：如 Large prime gaps 的 §2 "Background and Further Remarks"。
3. **Remarks / Applications 末尾**："We also note that ..."、"For related questions see [n]"。

按用户要求输出独立节或内联段落均可；独立节推荐四种组织方式：

## 模式 A：历史时间线（默认）

按时间顺序推进，每个阶段归因到具体文献。

> "The optimal density was previously known only in two [T], three [H], [FPK], and eight [V] dimensions, the latter being a recent breakthrough due to Viazovska. Building on her work, we solve ..."（Sphere packing, Annals）

## 模式 B：方法流派

把文献按技术路线分组，说明各组思路与本文的关系。

> "In contrast to the prime model C of Cramér [n] and the refinement G of Granville [n], in which random sets are formed by including positive integers with specific probabilities, the model R proposed here ..."（Large prime gaps, Invent. Math.）

## 模式 C：结果对比

并列列出已知上/下界、适用范围，突出本文位置。

> "At present, the strongest unconditional lower bound on G_P(x) is due to Ford, Green, Konyagin, Maynard, and Tao [n], who showed ..."（Large prime gaps）

## 模式 D：缺口定位

先陈述领域现状，明确指出空缺，再引出本文。

> "Although many interesting constructions are known, provable optimality is very rare."（Sphere packing）

## 归因动词库（语料高频）

```
was first proved by X [n]        is due to [n]
was later extended by X [m]      building on the work of [n]
in a recent breakthrough [n]     see [n] and the references therein
the strongest known bound is [n] the conjecture was resolved by [n]
```

## 使用建议

- 一个综述节内可混用多种模式（时间线为主干，穿插对比与缺口）。
- 引用密集但每处只承担一个职能：历史归因、对比定位、或支撑句。
- 开放问题用 "remains open" / "is still open" 明确标注。
