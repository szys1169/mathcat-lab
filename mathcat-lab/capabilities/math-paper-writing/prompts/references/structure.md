# 论文骨架与各节写法（依据四大期刊 13 篇语料）

## 通用骨架

```
标题 + 作者 + 日期
Abstract（1 段：问题、方法、主结果、意义）
Introduction
Notation / Preliminaries（或并入引言）
主体（定义/引理/命题/定理/证明）
Related Work / Background（可独立，也可并入引言）
Applications / Remarks（可选）
Open questions（可选）
Acknowledgments
References
```

这是常见组件清单，不是固定章节模板。章节选择由论证依赖、目标期刊、领域惯例和现有材料决定；不要为了凑齐骨架生成空洞的 Related Work、Applications 或 Conclusion。

## 语料量化基准（供篇幅参考）

- 中等论文 25–48 页：定理 1–10，引理 5–22，证明 5–24。
- 长论文 70–167 页：定理 17–48，引理 21–51，证明 28–80。
- 引用 28–194 条，主流 40–100。

## 各节写法

### Abstract
默认一段式：问题 → 主结果 → 最少量方法 → 意义/应用。除非目标模板或用户另有要求：

- 使用行内公式，不放 display math；
- 不放完整 proof sketch 或逐步技术路线；
- 避免人名、文献引用和论文标题；
- 不提"本文引用了某文的定义/结果"或研究过程、失败路线等元叙述。

需要指称前人工作时用中性表述（"a two-parameter family of minimal Lagrangian
immersions"、"the family considered in this paper"），具体归因留给引言。

### Introduction
通常覆盖六种功能：**对象与问题**、**研究意义**、**已有工作与缺口**、**本文回答**、**成果意义**、**方法桥接与必要路线**。具体顺序由领域和材料决定；核心概念在首次实质使用前定义或精确引用。成果意义必须说明可核验的数学后果、结构认识、方法价值、范围推进或边界，不写无支撑的宏大评价。
引用纪律：
- 不出现论文的具体名字/标题；提到前人工作时一律用 `\cite{key}` 或 `\cite[Section x]{key}`；
- 人名 + 引用可以出现（"due to X \cite{key}"），但摘要中例外（见上）；
- 问题编号引用如 "Problem 6.3 of \cite{key}"，不写论文标题。
论文较长、章节依赖不直观或目标期刊惯用时，在引言末尾加入 "The paper is organized as follows"（中文"本文结构如下"）组织段。逐节说明该节的逻辑作用，而非只写"第几节做什么"。短文或结构显然时可以省略。示例：
> "Section 2 fixes the notation and collects the preliminary facts,
> including the explicit formulas for the family and the parametrization
> of the roots. Section 3 establishes the exact collision criteria in the
> three regimes $0<\alpha<\pi/2$, $\alpha=0$, and $\alpha=\pi/2$. Section 4
> proves the main classification theorem. Section 5 reports numerical
> evidence, and Section 6 lists open problems."
若保留组织段，覆盖主要章节和有实质作用的附录，但不要在此复述完整证明路线。

### Notation / Preliminaries
- 数论/组合：首次使用处就地定义记号。
- 代数几何/表示论：独立 Notation 节（按类别列出）。
- PDE：独立 Preliminaries，含参数、范数、bootstrap 假设。

### Definitions
从来源包的定义清单提取（对象、记号、术语归因）。核心对象、后文反复使用的概念和具有实质条件的术语进入正式 `definition` 环境；简单记号可在首次使用处就地定义。标准概念根据目标读者给出定义或精确引用，不以缩写或默认常识替代。

### Main Results / Proofs
- 主定理陈述保持与输入完全一致（假设、量词、结论）。
- 长证明按 Step 分段，或拆成独立证明节。
- 已有结果在环境前用作者归因与精确引用标明来源，不伪装成本文原创。
- 证明允许重组和展开，但只能展开来源包已有数学；良定义、局部化、非零性、有限性等接口缺失时标注 gap，不用 prose 填补。

### 章节与结果之间的桥接

- 每个非平凡章节开头用少量 prose 说明前文已建立的输入、仍缺的环节和本节任务；章节顺序显然时不必机械套用。
- 关键引理或定理前，若角色并非一望可知，说明它消除哪个障碍或服务于哪个后续结果。
- 关键结果后，若用途不会立即显现，说明直接后果或后文调用位置。
- 桥接必须携带数学逻辑，不能只是 “We now prove the following lemma” 或逐项预告目录；也不能用流畅文字掩盖尚缺的证明接口。

### 正文以证明为主线（硬规则）

- 正文主体是**证明过程**：定义 → 引理 → 命题 → 定理 → 证明，按逻辑依赖推进；每节围绕证明主线组织，删去与主线无关的旁支。
- **材料筛选**：来源包资料只写入与证明主线直接相关的部分。研究过程中的错误尝试、失败路线、中间猜测、以及"纯数值观察、不依赖任何定理"这类内容**不写入正文**。可复现性说明（"checked symbolically"、"verified by computer algebra"）可保留。
- **案例按需加入**：只有来源包提供可靠例子且它能解释定义、边界或主结果时才加入；不为满足模板自动发明例子。
- **计算角色显式化**：数值表、符号计算和软件证书必须标记为证明、计算机辅助证明、独立核验、实验或例子发现；没有认可协议时不得替代正文证明。

### 列表与枚举编号
- 枚举（enumerate）编号统一用 `(1)(2)(3)` 形式：导言 `\usepackage{enumitem}` +
  `\setlist[enumerate]{label=(\arabic*)}`，或手动 `\item[(1)]`。
- 不用默认的 `1.` 编号；不同层级不混用 `(i)/(a)/1.` 风格。
- itemize（无序）用于无编号罗列，enumerate 仅用于需要编号引用的情形。

### Concluding Remarks
结尾章节是条件组件，但全文必须有终止功能。它可以由最后的应用、反例、remark 或一至两段总结承担：回扣开头的问题，说明本文实际建立了什么及其意义，并交代适用边界。若以反例收尾，应连接正面结果与自然强化失败的原因。若独立 Conclusion 只重复摘要、引言和主定理，则删除；不凭空发明开放问题。

### Acknowledgments
只使用用户或来源包提供的致谢、资助与利益冲突信息；缺失时保留交接项，不虚构姓名、机构或基金。
