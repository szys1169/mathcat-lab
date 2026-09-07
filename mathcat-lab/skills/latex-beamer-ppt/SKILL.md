---
name: latex-beamer-ppt
description: "把论文、讲义、研究报告或笔记整理成中文/英文数学科研 Beamer 幻灯片，编译并逐页检查 PDF，导出带讲稿备注的 PPTX。用于组会、学术报告、读书班、课程与答辩；不用于普通商务演示或从零开展数学研究。"
---

# LaTeX Beamer 数学科研 PPT

把已有数学材料整理成 LaTeX Beamer 幻灯片，编译 PDF，完成逐页视觉检查，并导出“每页高清图 + 备注区讲稿”的 PPTX。默认 16:9；公式使用原生 LaTeX，不用图片重绘。该 Skill 负责内容组织与交付，不搜索新定理、不补做证明、不宣称研究结果已经验证。

在 MathCat Lab 平台调用时，使用 Runner 明确传入的 Python 可执行文件；
不得硬编码另一台机器的虚拟环境路径。平台调用时，当前工作目录是用户项目根目录。

## 输入

- 必需:汇报主题或来源材料(论文 PDF、讲义、笔记、证明草稿)。
- 可推断:受众、语言(默认中文)、时长或页数预算(默认 15 页左右)。
- 可选:现有 `.tex`/`.sty` 模板、参考文献 BibTeX、图片、机构样式约束、是否需要 PPTX 导出。

素材不足时记录缺口，不硬编内容；没有来源的公式与定理不放进 deck。交互任务可以询问用户，平台批处理任务应采用保守默认值继续，并在任务报告中列出假设和缺口。若工作区包含多个测试 bundle、多篇无关论文或多个主题，而用户没有指定具体材料，平台任务不得擅自挑选；写失败报告并要求给出论文、bundle 或文件路径。

## 输出

独立运行默认落在 `output/YYYYMMDD_主题/`；由平台调用时，在平台传入的
`PPT/` 下新建 `versions/<task-id>-YYYYMMDD-HHMMSS-ffffff-主题/`，本次全部产物写入该版本
目录，不覆盖旧版本：

- `slides.tex` + 必要的自定义样式与 `pic/` — 可继续精修的 Beamer 源文件(默认使用通用模板，见 [beamer-conventions.md](references/beamer-conventions.md))
- `slides.pdf` — xelatex 编译产物,视觉基准
- `render_report.json` — 逐页渲染检查的页数与 PDF 哈希证据
- `speaker_notes.md` — 逐页讲稿
- `final.pptx` — 每页高清图 + 备注区讲稿(优先拿它去讲)
- `slide_source_ledger.md` — 每页论断 → 来源/公式/引用的台账
- `ppt-task-report.md` — 完成状态、输入、假设、检查结果、缺失项与文件清单；平台任务必需

## 工作流

1. **确定运行模式**：交互模式允许大纲与样例帧确认；平台/全自动模式不暂停等待确认，采用保守默认值并记录决策。平台适配见 [platform-integration.md](references/platform-integration.md)。
2. **清点输入**：识别数学类型(定理课 / 论文报告 / 算法 / 实验 / 读书班)，按 [deck-structure.md](references/deck-structure.md) 列出素材、来源与缺口。
3. **规划大纲**：给出论点主线、section 地图与页数预算(默认 12–18 页)，为每页规划要点、公式、图和讲稿。交互模式先征求确认；批处理模式直接执行。
4. **验证样例帧**：先编译 1–2 页代表帧(公式/定理密集页优先)。交互模式可等待风格确认；批处理模式自行检查后继续。
5. **写全 deck**：按 [beamer-conventions.md](references/beamer-conventions.md) 写作，并同步维护 `slide_source_ledger.md` 与 `speaker_notes.md`。
6. **编译**：运行 `scripts/build_slides.py --strict-overfull`，修到零错误、零 Overfull。Windows 下不得设置或重定向 `MIKTEX_USERCONFIG`、`MIKTEX_USERDATA`、`MIKTEX_USERINSTALL`；脚本会在 `latexmk` 的 MSYS Perl 被沙箱阻止时自动回退到原生 XeLaTeX。
7. **渲染检查**：运行 `scripts/render_check.py ... --strict-layout` 临时输出 PNG 和 `render_report.json`；逐页检查越界、重叠、乱码、对比度、语义配色、版面利用率和图片清晰度，发现问题后回改重编。机器报告不能替代逐页目视检查。
8. **导出 PPTX**：运行 `scripts/export_pptx.py ... --strict-notes`，确保讲稿与 PDF 页数一一对应。
9. **验收与清理**：先写 `ppt-task-report.md`（明确说明 PPTX 为图片式、不可逐元素编辑），再运行 `scripts/validate_delivery.py <输出目录> --strict-notes-metadata --cleanup-page-images`。验收通过后删除正常逐页 PNG，保留 `render_report.json`；失败时保留 PNG 排错。未通过时不得报告“已完成”。
10. **交付**：重新读取验收输出和 `render_report.json`，列出 PDF、PPTX、源文件、讲稿、台账、页数、警告与剩余风险，不得凭记忆声称“无错误”。

平台会把执行器的最终回复写到 `成果/任务报告/<task-id>/ppt-task-report.md`，避免并发任务互相覆盖。版本
目录内也必须保留完整报告，并在最终回复中给出版本目录、PDF、PPTX 和讲稿
的准确路径，供平台发现和展示。

## 硬规则

- 公式用原生 LaTeX 排版(`$...$`、`align`、`equation`、`cases`、`tikz-cd` 等),绝不截图、绝不用图像模型渲染公式。
- 不虚构公式、定理、数值、引用;不确定的论断显式标注;不改变输入材料的数学内容(不强化/弱化假设与量词)。
- 保持来源忠实:定理陈述、符号约定、假设、量词、证明状态与输入一致;overlay 不得改变公式编号或隐藏假设。
- 中文用 `ctex` + xelatex。若用户提供模板，优先保留其主题、宏包和版式；否则使用可公开分发的 `assets/generic-template/slides.tex`。私有仓库中的学堂班模板只能在用户明确选择且本地资产可用时使用，不得进入公开包。
- 密度纪律:默认每页 ≤ 7 条要点、≤ 2 个显示公式;定义/定理/证明用 block 环境;一页塞不下的推导拆页。
- 默认采用“问题—符号—主定理—证明地图—关键台阶—应用—反例—结论—参考文献”的论文汇报叙事；避免密集微型导航、大面积无意义留白和无语义的红色强调。
- 每页讲稿含预计用时、必须讲、过渡句与 `[Sources]`，并与 PDF 页码严格对应。
- `slides.tex`、`slides.pdf`、`render_report.json`、`speaker_notes.md`、`final.pptx`、`slide_source_ledger.md` 和平台任务报告齐全且验收通过，才算平台任务完成；逐页 PNG 是检查过程文件，成功后不作为平台成果保留。只生成 `.tex` 或只生成 `.pptx` 均为 partial。
- 学堂班模板与校徽有品牌归属,可本地使用,不得随 skill 公开再分发(见 [CREDITS.md](references/CREDITS.md))。

## 参考

- 结构与数学 slide 模式:[references/deck-structure.md](references/deck-structure.md)
- Beamer 排版约定、模板用法、编译与审计:[references/beamer-conventions.md](references/beamer-conventions.md)
- PDF → PPTX 导出与讲稿格式:[references/export-pptx.md](references/export-pptx.md)
- 来源与许可证声明:[references/CREDITS.md](references/CREDITS.md)
- Codex / DeepSeek Harness / MathCat Lab 接入:[references/platform-integration.md](references/platform-integration.md)
- 默认模板资产：`assets/generic-template/slides.tex`；私有可选模板：`assets/tsinghua-template/`（不得进入公开分发包）
