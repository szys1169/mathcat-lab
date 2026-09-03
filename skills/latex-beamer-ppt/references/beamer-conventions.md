# Beamer 排版约定、模板用法与编译审计

## 模板用法(学堂班 Tsinghua 模板)

把 `assets/tsinghua-template/` 中的 `Tsinghua.sty` 与 `pic/` 拷到输出目录,与 `slides.tex` 同级,然后:

```latex
\documentclass[aspectratio=169]{beamer}   % 现代投影默认 16:9;旧设备可去掉
\usepackage{ctex}
\usepackage[T1]{fontenc}
\usepackage{amsmath,amssymb}
\usepackage{tikz-cd}
\usepackage{booktabs,graphicx,multicol}
\usepackage{Tsinghua}

\author{作者}
\title{标题}
\institute{单位/书院}
\date{\today}
```

参考示例 deck:`assets/tsinghua-template/slide.tex`(层和层上同调)。它的结构约定:

- `\kaishu` 开启楷体正文;标题页放 `\titlepage` + logo。
- `Tsinghua.sty` 已定义清华紫配色、smoothbars 导航、circles 内层主题、页脚页码,并自动在每个 section 前插入目录页。
- 数学内容用命名 block:`\begin{block}{Definition 1.1.} ... \end{block}`、`Theorem`、`Proposition`、`Lemma`、`Corollary`、`Example`、`Construction`。
- 校徽等品牌图片只在本地使用,不随输出公开传播。

## 公式纪律

- 行内公式 `$...$`;独立公式用 `equation`/`align`/`align*`;推导用 `split`、`multline`;分段函数用 `cases`;不用裸 `$$...$$`。
- 长公式:先 `\small` 或拆行,再考虑拆页;保证公式字号不小于正文的约 85%。
- 符号先定义后使用;同一符号在全文保持同一含义;量词、假设、边界条件一个都不能丢。
- 交换图用 `tikz-cd`;结构图用 `tikz`/`pgfplots`;图片统一放 `pic/`,用相对路径引用。

## 中文与字体

- 引擎固定 xelatex(ctex 需要);不要用 pdflatex 编译中文 deck。
- 中文标点、公式与中文之间的间距由 ctex 自动处理;`\kaishu`/`\songti` 按需切换字体。
- 若目标机器缺中文字体,先 `check_environment.py` 定位问题,不擅自改字体配置。

## 编译与审计

编译统一走 `scripts/build_slides.py`(封装 `latexmk -xelatex -interaction=nonstopmode -halt-on-error`)。MiKTeX 首次编译可能自动安装宏包,耗时较长,属正常现象。

审计清单(每项都要过):

- 编译零错误;`!` 开头的错误行全部消除。
- Overfull/Underfull 数量:Overfull 必须处理(拆行、缩字号、拆页);Underfull 可容忍但需知道原因。
- PDF 页数与大纲一致;目录、导航、页脚不重叠。
- 渲染检查:公式不乱码、图片不糊、无元素越界;用 `scripts/render_check.py` 出的 PNG 逐页看。
- 引用与交叉引用可解析;`ref.bib` 条目真实存在。
