# 引用前人结果与背景定义的规范（语料：36 篇四大期刊论文）

## 引用已有定理/命题/引理

- 正文归因句式（语料高频）：
  - "due to X \cite[Theorem 3.1]{key}"（due to 164 次）；
  - "was proved by X \cite{key}"（proved by 32 次）、"established by"；
  - "was introduced by X \cite{key}"（introduced by 17 次）；
  - "see \cite[Theorem 3.1]{key} and the references therein"。
- 已有结果作为工具引用：`\cite[Theorem 3.1]{key}` 放在首次使用处；正文只说"by \cite[Theorem 3.1]{key}"，不重复陈述完整定理。该定理是论证核心时可以完整重述，但在环境前正文归因，定理环境本身不带引用标题。
- 引用公式：`\cite[Equation (3.2)]{key}` 或 `\eqref` 指向本文公式。

## 背景定义的处理

- **标准定义**：写作 "We collect here the standard definitions" / "standard notation"（语料：''general notation and standard definitions''）；不展开证明。
- **引用他人定义**（三种均可，全文一致）：
  1. 定义环境带来源：`\begin{definition}[Definition 2.3 of \cite{key}]`，正文给出定义内容；
  2. 行内引用："as in \cite[Definition 2.3]{key}"（as in 494 次）或 "in the sense of \cite[Definition 2.3]{key}"（in the sense of 74 次）；
  3. 沿用记号："we follow the convention of \cite[Section 4]{key}"（we follow 19 次）。
- **记号约定**：沿用某文献记号时写明 "Throughout, we use the notation of \cite{key}"；自行定义时 "We denote by X ..."（见 structure.md）。
- 被引用定义的术语在本文首次出现时仍按定义术语处理（`\textbf`），并注明出处。

## 归因纪律

- 已有结果绝不写成原创；本文原创结果不引用归因。
- 引用必须精确到编号（定理/命题/引理/公式/节），不写模糊的 "see [n]"（综述性提及可例外）。
- 引用他人定义时注明是"标准/惯例"还是"某文引入"，不混淆。
- `closed-sources` 下，载荷引用只来自来源包书目。用户明确启用主动文献补全时，新增来源须经 `$math-related-work` 的元数据核验、正文支撑核验和引用准入后进入书目；不虚构编号或出处。
