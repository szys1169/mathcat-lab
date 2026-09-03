---
name: latex-beamer-ppt
description: "把论文、讲义、研究报告或笔记整理成中文/英文数学科研 Beamer 幻灯片，编译并逐页检查 PDF，导出带讲稿备注的 PPTX。用于组会、学术报告、读书班、课程与答辩；不用于普通商务演示或从零开展数学研究。"
---

# LaTeX Beamer 数学科研 PPT

把已有数学材料整理成 LaTeX Beamer 幻灯片，编译 PDF，完成逐页视觉检查，并导出“每页高清图 + 备注区讲稿”的 PPTX。默认 16:9；公式使用原生 LaTeX，不用图片重绘。该 Skill 负责内容组织与交付，不搜索新定理、不补做证明、不宣称研究结果已经验证。

## 输入

- 必需:汇报主题或来源材料(论文 PDF、讲义、笔记、证明草稿)。
- 可推断:受众、语言(默认中文)、时长或页数预算(默认 15 页左右)。
- 可选:现有 `.tex`/`.sty` 模板、参考文献 BibTeX、图片、机构样式约束、是否需要 PPTX 导出。

素材不足时记录缺口，不硬编内容；没有来源的公式与定理不放进 deck。交互任务可以询问用户，平台批处理任务应采用保守默认值继续，并在任务报告中列出假设和缺口。

## 输出

独立运行默认落在 `output/YYYYMMDD_主题/`；由平台调用时，必须写入平台传入的输出目录（MathCat Lab 为项目的 `PPT/` 子目录）：

- `slides.tex` + `Tsinghua.sty` + `pic/` — 可继续精修的 Beamer 源文件(模板见 [beamer-conventions.md](references/beamer-conventions.md))
- `slides.pdf` — xelatex 编译产物,视觉基准
- `page_images/` — 逐页 PNG，供视觉检查
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
6. **编译**：运行 `scripts/build_slides.py --strict-overfull`，修到零错误、零 Overfull。
7. **渲染检查**：运行 `scripts/render_check.py` 输出 PNG；逐页检查越界、重叠、乱码、对比度和图片清晰度，发现问题后回改重编。
8. **导出 PPTX**：运行 `scripts/export_pptx.py ... --strict-notes`，确保讲稿与 PDF 页数一一对应。
9. **验收**：运行 `scripts/validate_delivery.py <输出目录>`；未通过时不得报告“已完成”。
10. **交付**：写 `ppt-task-report.md`，列出 PDF、PPTX、源文件、讲稿、台账、逐页渲染结果与剩余风险。

## 硬规则

- 公式用原生 LaTeX 排版(`$...$`、`align`、`equation`、`cases`、`tikz-cd` 等),绝不截图、绝不用图像模型渲染公式。
- 不虚构公式、定理、数值、引用;不确定的论断显式标注;不改变输入材料的数学内容(不强化/弱化假设与量词)。
- 保持来源忠实:定理陈述、符号约定、假设、量词、证明状态与输入一致;overlay 不得改变公式编号或隐藏假设。
- 中文用 `ctex` + xelatex。若用户提供模板，优先保留其主题、宏包和版式；否则可使用本 Skill 的本地模板。
- 密度纪律:默认每页 ≤ 7 条要点、≤ 2 个显示公式;定义/定理/证明用 block 环境;一页塞不下的推导拆页。
- `slides.tex`、`slides.pdf`、`page_images/`、`speaker_notes.md`、`final.pptx`、`slide_source_ledger.md` 和平台任务报告齐全且验收通过，才算平台任务完成；只生成 `.tex` 或只生成 `.pptx` 均为 partial。
- 学堂班模板与校徽有品牌归属,可本地使用,不得随 skill 公开再分发(见 [CREDITS.md](references/CREDITS.md))。

## 参考

- 结构与数学 slide 模式:[references/deck-structure.md](references/deck-structure.md)
- Beamer 排版约定、模板用法、编译与审计:[references/beamer-conventions.md](references/beamer-conventions.md)
- PDF → PPTX 导出与讲稿格式:[references/export-pptx.md](references/export-pptx.md)
- 来源与许可证声明:[references/CREDITS.md](references/CREDITS.md)
- Codex / MathCat Lab 接入:[references/platform-integration.md](references/platform-integration.md)
- 模板资产:`assets/tsinghua-template/`(`Tsinghua.sty`、示例 `slide.tex`、`pic/`)
