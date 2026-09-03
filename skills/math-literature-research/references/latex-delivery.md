# LaTeX、PDF 与最终交付

复制 `assets/literature-review-template.tex` 和 `assets/theorem-toolbox-template.tex` 到成果目录再编辑，不修改 Skill 资产。

文献综述至少包含：问题表述、历史起源、数学动机、早期结果、关键进展、当前状态、变体与方法、候选 gap、证据缺口和检索局限。工具箱按直接工具、条件工具、特殊情形、障碍/反例、跨领域方法和未核实候选组织。

两份文档必须使用同一 `selected-bibliography.bib`，正文引用键必须出现在来源台账中。默认先运行 `latexmk -pdf`；含中文且模板切换为 `ctexart` 时使用 `latexmk -xelatex`。若无 `latexmk`，按所选引擎运行足够轮次并处理 BibTeX/Biber。

完成前检查编译退出码、未解析引用、缺图、溢出、空白页、目录一致性和 PDF 存在。编译失败时保留 `.tex` 和日志，状态为 `partial`，不得伪造 PDF。

最终任务报告列出范围、执行器、检索来源和截止时间、纳入/排除数量、下载/待获取数量、证据等级分布、当前状态、交付物、已知局限和失败阶段。任何终止状态都必须生成。
