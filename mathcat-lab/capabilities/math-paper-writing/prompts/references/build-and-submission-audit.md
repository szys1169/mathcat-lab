# 编译与提交前检查

本检查负责"论文能否可靠构建并提交"的机械质量，不负责证明正确性。

## 编译流程（通用）

1. 识别根 `.tex` 文件，检查项目结构（文档类、宏包、biblio、figures）。
2. 允许编译时优先：
   `latexmk -pdf -interaction=nonstopmode -halt-on-error <root>.tex`
   无 latexmk 时用 `pdflatex` → `bibtex` → `pdflatex` ×2。
3. 读日志中的错误、警告、引用/标签、盒子问题。
4. 长编译、安装 TeX 宏包或改动文档类之前先询问。

## 硬失败

- 致命编译错误
- 未定义引用/引用未解析
- 重复标签
- 缺失文件
- 损坏的参考文献
- 不兼容宏包

## 软警告

- overfull / underfull box
- 浮动对象拥塞
- 页数超限风险
- 脆弱宏
- arXiv 兼容性隐患

## 视觉边界验证（交付前必做）

- 编译日志中的每个 `Overfull \hbox` 都要定位并消除：它表示内容超出页面文本区右边界。数学公式超宽按 [notation-formula-audit.md](notation-formula-audit.md) 的长公式排版纪律重排（`\textstyle` → 等价重排 → 按运算符断行），不得用缩小字体或隐藏内容掩盖。
- **列表标签溢出（隐蔽坑）**：`\item[\emph{(长标签)}]` 这类自定义标签若宽于列表的 label 区，会**向左溢出左边界且不产生任何 Overfull 警告**（LaTeX 允许标签伸入页边距）。这是"编译零警告但页面超界"的常见来源。检查与修复：
  - 检查：渲染 PDF 后用 `pdftotext -bbox <root>.pdf -` 扫描 `xMin` 小于左边距的词，或逐页人工查看列表项；也可用 `pdftoppm` 渲染后做像素检测。
  - 修复：把长标签移入正文行首（`\item \emph{Translational case:} ...`），或设置列表的 `labelwidth`/`itemindent`（enumitem），不保留 `\item[...]` 长标签。
- 数值表格超宽属于表格而非数学公式，可用 `{\small \[ ... \]}` 包裹或调整列格式（`tabular` 列对齐 `r`/`l`）解决；表格换行不用 `\\` 语法问题（xeCJK 环境用 `\cr`，见 latex-conventions）。
- 渲染 PDF 抽查：用 `pdftoppm -png -r 100 <root>.pdf page` 渲染后，人工抽查首页、含长公式页、数值表页，确认无内容超出文本区、无异常公式换行。
- 页眉/页脚超宽（如长标题导致的每页 `Overfull \hbox`）：用文档类的短标题机制（amsart 支持 `\title[短标题]{长标题}`）缩短页眉，而不是改正文。

## 提交前清单

### Build

- Root TeX file:
- Engine and command:
- Clean build completed:
- Remaining fatal errors:
- Remaining undefined references or citations:
- Duplicate labels:

### Source Package

| Item | Status | Notes |
|---|---|---|
| TeX 输入、图形、书目、宏包引用无绝对路径 |  |  |
| 无缺失 style/class/bibliography/figure 文件 |  |  |
| 文件名避免空格与脆弱字符 |  |  |
| 需要时 `.bbl` 与当前书目工作流一致 |  |  |
| 图形格式与所选引擎匹配 |  |  |

### Policy And Venue

- arXiv 兼容性风险:
- 期刊 class/模板要求:
- 匿名评审要求:
- 代码/数据链接公开且可解析:
- 违禁水印、行号、审稿注释、页边批注已移除:

### Final Evidence Check

- Abstract 声明已对照 claim ledger:
- 主结果已对照 proof-obligation 表:
- Related work 对比引用了真实来源:
- 主动检索文献均在 `literature_search_log.md` 中达到 `admitted`，BibTeX 与核验版本一致:
- 文献功能覆盖矩阵的关键缺口已覆盖或明确报告:
- Introduction 的成果意义可追溯到定理、后果、应用或已核验比较:
- 局限已如实陈述，未隐藏证明/实验缺口:

### Semantic Interface Check

- 定理假设、量词、对象类型与来源包逐项一致:
- 非标准符号在首次实质使用前定义或精确引用:
- 新映射给出定义域、陪域、公式与所需良定义依据:
- 底环、基域、局部化、商和环境对象在需要处可见:
- “显然/容易/标准论证”等位置均有实际依据，或已标为 gap:
- 自创关系已说明是定义、相容性、推论、冗余关系或独立论断:
- 计算结果已标明 proof / computer-assisted-proof / independent-check / experiment / example-discovery:

### Revision And Bilingual Check（适用时）

- `human_revision_log.md` 无未说明的 `pending` / `applied` 条目:
- 项目术语与记号符合 `style_decisions.md`:
- 主语言版本已经作者确认或达到约定状态:
- 多语言版本的定理、假设、标签、引用和术语逐项同步:
- 摘要、引言、章节导语和结尾无大段功能重复:
- 章节接口图中的关键桥接已落实，正文不是无解释的定理/引理队列:
- 结尾已履行 Opening–Closing Contract；无需为此强制添加 Conclusion:
- 正文无“按要求/上一版/这里修改/不再尝试”等过程痕迹:

## 规则

- 不通过隐藏内容或改变数学含义来消除警告。
- 构建可复现性是论文交付物的一部分。
- 本审计不变成证明审查；数学支持问题转交 [proof-obligation-audit.md](proof-obligation-audit.md) / [claim-evidence-ledger.md](claim-evidence-ledger.md)。
- 可运行 `scripts/audit_manuscript.py` 生成机械候选报告；其短语、标签和双语差异结果只用于定位，不构成数学判断。
