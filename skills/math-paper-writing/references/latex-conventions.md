# LaTeX 环境、引用纪律与编译

## 定理环境命名（语料惯例：短别名 + 共享计数器）

```tex
\newtheorem{theorem}{Theorem}
\newtheorem{lemma}[theorem]{Lemma}
\newtheorem{proposition}[theorem]{Proposition}
\newtheorem{corollary}[theorem]{Corollary}
\newtheorem{definition}[theorem]{Definition}
\newtheorem{remark}[theorem]{Remark}
\newtheorem{conjecture}[theorem]{Conjecture}
\newtheorem{example}[theorem]{Example}
```

## 定理/定义环境不带括号标题

- **硬规则**：`\begin{theorem}`、`\begin{lemma}`、`\begin{proposition}`、
  `\begin{corollary}`、`\begin{definition}`、`\begin{conjecture}`、
  `\begin{remark}` 等环境**不跟方括号标题**（如 `[Main result]`、
  `[Injectivity classification]`、`[Clifford uniqueness]` 一律去掉）。
- 定理在正文中的指称靠 `\label` + `\ref`（"Theorem \ref{thm:x}"），不需要
  括号里的名字；确需点明用途时，在正文引导句中说明（"The following is our
  main result."、"The next theorem classifies ..."）。
- 归因引用不放括号标题：引用已有结果时，在**正文**写 "The following is
  Theorem 3 of \cite{key}" 或 "The next lemma is due to \cite[Corollary 2]{key}"，
  环境本身不带 `[\cite{key}]`。
- 例外：仅当用户/期刊模板明确要求命名定理时才允许括号标题。

## 编号体系（语料惯例：按节编号 + 共享计数器）

- 定理类环境统一**按节编号且共享计数器**：`\newtheorem{theorem}{Theorem}[section]`，其余环境 `[theorem]` 共享（语料 36 篇中共享计数器 207 处 vs 独立 65 处，为主流）。
- 公式**按节编号**：导言加 `\numberwithin{equation}{section}`（约半数四大期刊论文使用；否则公式连续编号 (1),(2),… 会与定理 x.y 编号风格不一致，造成"x.x 混乱"）。
- 交叉引用统一用 `\ref`/`\eqref`：公式引用必须 `\eqref{eq:...}`（语料 2651 次，主流）；`cleveref` 的 `\cref` 可选。
- `\label` 命名前缀约定（语料高频）：`thm:`、`lem:`、`prop:`、`cor:`、`def:`、`rem:`、`eq:`、`sec:`、`fig:`、`tab:`。
- 文本中引用编号用 `Theorem~\ref{thm:x}`、`\S\ref{sec:y}`、`\eqref{eq:z}`，不手写编号。

## 加黑与强调（语料惯例）

- **新定义的术语用 `\textbf`**（粗体）："We call $h$ a \textbf{directed geodesic} if ..."、"a \textbf{landscape} if ..."、"{\textbf{geometrically finite}}"（语料 \textbf 375 处，绝大多数是定义术语）。
- **一般强调用 `\emph`**（斜体）：强调词、情形标签、文献引用标题等（语料 \emph 1145 处）。
- **数学粗体**：向量/矩阵等用 `\mathbf`（267 处）或 `\bm`（264 处，需 `bm` 宏包），不用 `\textbf` 表示数学粗体。
- 定理名（Theorem/Lemma 等）由环境自动渲染，不手工加粗。
- 情形容器（如 "Translational case:"）用 `\emph` 或 `\textbf` 均可，但全文一致；推荐 `\emph{...:}` 于行首，`\textbf` 留给被定义的术语。

## 引用纪律（语料验证）

- 载荷引用只来自来源包书目（`refs.bib` 或等价 BibTeX 文件）。
- 首次载荷使用处引用一次，之后自然叙述。
- 已有结果：`\begin{theorem}[{\citet{key}}]` 或 "by a theorem of X \citep{key}"，绝不写成原创。
- 综述性引用可自由补充，但必须真实、可解析。
- 不写 TODO 占位引用；缺元数据就交接。
- 引用具体编号用 `\cite[Theorem 3.1]{key}`、`\cite[Definition 2.3]{key}`、`\cite[Section 4]{key}`（可选参数首字母大写；语料中 Theorem 265 次、Proposition 187、Lemma 147、Definition 80、Corollary 70、Equation 48）。
- 具体归因与定义引用惯例见 [citation-and-definition.md](citation-and-definition.md)。

## 通用编译流程

1. 生成独立可编译的 `.tex`（不 `\input` 仓库模板）。
2. 文档类优先用目标期刊样式；无指定时 `\documentclass{article}` + `\usepackage{amsmath,amsthm,amssymb}`，有文献时 `\bibliographystyle{amsplain}`（或期刊指定样式）+ `\bibliography{refs}`。
3. 编译：`pdflatex` → `bibtex` → `pdflatex` ×2；有 latexmk 时优先 `latexmk -pdf -interaction=nonstopmode -halt-on-error`。
4. 编译与提交前细节见 [build-and-submission-audit.md](build-and-submission-audit.md)。

## 输出前自检

- [ ] 每个定理/引理有来源（原创或归因）
- [ ] 假设/记号在首次使用前定义
- [ ] 无悬空 `\ref`、每个 `\cite` 可解析
- [ ] 声明台账覆盖 Abstract / Introduction / Conclusion 的压缩性论断
- [ ] 义务缺口已在 revision_notes 中标注，未被 prose 隐藏
- [ ] 编号风格统一：定理按节共享编号、公式 `\numberwithin` 按节、所有 `\eqref`/`\ref` 可解析
- [ ] 定义术语 `\textbf`、一般强调 `\emph`、数学粗体 `\mathbf`/`\bm` 使用一致
- [ ] 致谢与资助信息齐全
- [ ] 无 agent 历史、内部路径、失败路线
