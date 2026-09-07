# 来源与许可证声明

本 skill 在用户提供的学堂班(清华大学)Beamer 模板基础上,融合了以下开源仓库的机制与纪律,并保持 MIT 许可证兼容。

## 学堂班模板(用户资产)

- 来源:用户提供的 `学堂班.zip`(清华致理书院组会模板)
- 内容:`Tsinghua.sty`(THU Beamer 主题,参考 http://far.tooold.cn/post/latex/beamertsinghua)、示例 `slide.tex`、`pic/`(校徽等)
- 使用约定:仅本地教学/汇报和私有仓库内使用。当前本地完整包包含这些资产，不得直接作为公开 Skill 发布；制作公开包时必须排除 `assets/tsinghua-template/`，并改用权利清晰的通用模板。

## MathCat Lab 通用模板

- 路径：`assets/generic-template/slides.tex`
- 来源：MathCat Lab 项目内原创的无校徽、无机构品牌通用 Beamer 骨架。
- 用途：DeepSeek Harness 插件和公开分发包的默认模板；用户未提供模板时优先使用。

## VeryMath/AI4Math-Writing(math-beamer)

- 仓库:https://github.com/VeryMath/AI4Math-Writing
- 许可证:MIT License(Copyright (c) 2026 VeryMath)
- 吸收机制:
  - 数学 deck 的结构与领域覆盖规则(定理/算法/实验/教学各自的纪律)
  - 来源忠实纪律:不虚构公式与定理、不强化/弱化假设与量词
  - 编译命令(`latexmk -xelatex`)与 build/layout 审计清单
  - 大纲/样例帧关卡与 slide-source-ledger 台账思想

## texra-ai/texra-scientific-skills(scientific-presenter)

- 仓库:https://github.com/texra-ai/texra-scientific-skills
- 许可证:MIT © texra-ai
- 吸收机制:
  - 先读用户模板再动手、保留既有主题/排版/宏包
  - 故事驱动规划(一页一问)与视觉化优先
  - 尽早频繁编译 + 强制视觉 QA(编译通过但溢出/重叠不算完成)

## moyoo0/paper-to-latex-ppt

- 仓库:https://github.com/moyoo0/paper-to-latex-ppt
- 许可证:仓库 README 声明开源(具体许可证以其仓库为准)
- 吸收机制:
  - 论文 → 组会 PPT 的端到端流水线(结构化阅读 → 15 页大纲 → Beamer → PDF 渲染检查 → PPTX 导出)
  - 逐页讲稿写入 PPTX 备注区 + 独立 `speaker_notes.md`
  - 环境检查脚本(先查 latexmk/xelatex/Python 依赖再开工)

## Noi1r/powerpoint-skill(设计思想参考)

- 仓库:https://github.com/Noi1r/powerpoint-skill
- 许可证:MIT
- 吸收机制(仅思想,不搬代码):
  - 内容密度守卫(7 条要点 / 2 个公式 / 5 个符号)
  - 数学 slide 模式(Definition、Construction、Theorem-Proof、Comparison)
  - 生成 → PDF → 视觉检查 → 修正的 QA 循环

## 使用注意

- 依据 MIT 许可证可自由使用、修改、合并与商用,但须保留上述版权与许可声明。
- 未吸收仓库的其余内容(如 OMML/PPTX 原生公式管线)不属于本 Beamer 路线,故未并入。
