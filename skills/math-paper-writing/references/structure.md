# 论文骨架与各节写法（依据四大期刊 13 篇语料）

## 通用骨架

```
标题 + 作者 + 日期
Abstract（1 段：问题、方法、主结果、意义）
Introduction
Notation / Preliminaries（或并入引言）
主体（定义/引理/命题/定理/证明）
Applications / Remarks（可选）
Open questions（可选）
Acknowledgments
References
```

## 语料量化基准（供篇幅参考）

- 中等论文 25–48 页：定理 1–10，引理 5–22，证明 5–24。
- 长论文 70–167 页：定理 17–48，引理 21–51，证明 28–80。
- 引用 28–194 条，主流 40–100。

## 各节写法

### Abstract
一段式：问题 → 方法 → 主结果 → 意义/应用。**硬规则**：
- 不出现人名（作者名、前人姓名一概不提）；
- 不出现文献引用（无 `\cite`）；
- 不出现论文标题或任何论文的具体名称；
- 不提"本文引用了某文的定义/结果"这类元叙述。
需要指称前人工作时用中性表述（"a two-parameter family of minimal Lagrangian
immersions"、"the family considered in this paper"），具体归因留给引言。

### Introduction
四要素（顺序固定）：**背景**（研究对象与动机）→ **问题**（本文要回答的问题，完整陈述）→ **主要成果**（主定理/主结果完整陈述）→ **方法思路**（一段证明思路）。在背景或问题段**顺带给出关键定义**（研究对象、核心概念首次出现即定义）。
引用纪律：
- 不出现论文的具体名字/标题；提到前人工作时一律用 `\cite{key}` 或 `\cite[Section x]{key}`；
- 人名 + 引用可以出现（"due to X \cite{key}"），但摘要中例外（见上）；
- 问题编号引用如 "Problem 6.3 of \cite{key}"，不写论文标题。
**必选组织段**："The paper is organized as follows"（中文"本文结构如下"）
置于引言末尾。逐节一句话概要，说明**该节是什么内容**（输入与产出），而非只写
"第几节做什么"。示例：
> "Section 2 fixes the notation and collects the preliminary facts,
> including the explicit formulas for the family and the parametrization
> of the roots. Section 3 establishes the exact collision criteria in the
> three regimes $0<\alpha<\pi/2$, $\alpha=0$, and $\alpha=\pi/2$. Section 4
> proves the main classification theorem. Section 5 reports numerical
> evidence, and Section 6 lists open problems."
每一节都要给出足够信息量，使读者仅凭组织段即可知道各节内容；附录也需提及。

### Notation / Preliminaries
- 数论/组合：首次使用处就地定义记号。
- 代数几何/表示论：独立 Notation 节（按类别列出）。
- PDE：独立 Preliminaries，含参数、范数、bootstrap 假设。

### Definitions
从来源包的定义清单提取（对象、记号、术语归因）；句式 "We say that X is ... if ..."、"Let X be ..."、"We denote by X ..."。定义有标准出处时注明 "standard" 或归因引用。

### Main Results / Proofs
- 主定理陈述保持与输入完全一致（假设、量词、结论）。
- 长证明按 Step 分段，或拆成独立证明节。
- 已有结果用 `[{\citet{key}}]` 归因。

### 正文以证明为主线（硬规则）

- 正文主体是**证明过程**：定义 → 引理 → 命题 → 定理 → 证明，按逻辑依赖推进；每节围绕证明主线组织，删去与主线无关的旁支。
- **材料筛选**：来源包资料只写入与证明主线直接相关的部分。研究过程中的错误尝试、失败路线、中间猜测、以及"纯数值观察、不依赖任何定理"这类内容**不写入正文**。可复现性说明（"checked symbolically"、"verified by computer algebra"）可保留。
- **适当加案例**：在概念定义后、关键引理/主定理后插入 1–2 个具体实例（`\begin{example}`，显式参数、可手算的数值），帮助理解；案例不打断证明主线，不引入未验证的数学。
- **数值计算**：问题可数值化的，在正文相应位置或末尾给出数值表（代表值、极值点、精度说明），或附录提供计算程序；数值表明确标注为"数值证据"而非证明。

### 列表与枚举编号
- 枚举（enumerate）编号统一用 `(1)(2)(3)` 形式：导言 `\usepackage{enumitem}` +
  `\setlist[enumerate]{label=(\arabic*)}`，或手动 `\item[(1)]`。
- 不用默认的 `1.` 编号；不同层级不混用 `(i)/(a)/1.` 风格。
- itemize（无序）用于无编号罗列，enumerate 仅用于需要编号引用的情形。

### Concluding Remarks
Remarks（例子、特殊情况、对比）、Applications（主定理的应用）、Open questions（枚举式列 1–3 个）。

### Acknowledgments
"The authors thank [referee/colleagues] for ... [Name] was supported by [grant]."
