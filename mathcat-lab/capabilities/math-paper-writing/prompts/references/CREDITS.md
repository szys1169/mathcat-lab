# 来源与许可证声明

本 skill 在原始版本（基于四大期刊语料特征）基础上，融合了以下两个 MIT 许可证开源仓库的写作机制，并移除了对特定研究流水线（MMAT）工作区格式与 CLI 工具的依赖，泛化为通用来源包契约。

## VeryMath/AI4Math-Writing

- 仓库：https://github.com/VeryMath/AI4Math-Writing
- 许可证：MIT License（Copyright (c) 2026 VeryMath）
- 吸收机制：
  - Source Packet 输入契约与缺口表（source-packet.md）
  - Rapid Prototype Paper Skeleton：contribution spine、section map、result dependency map（paper-skeleton.md）
  - Claim–Evidence Ledger 台账（claim-evidence-ledger.md）
  - Proof Obligation and Assumption Audit 写作侧检查（proof-obligation-audit.md）
  - Notation / Formula 可读性审计（notation-formula-audit.md）
  - LaTeX Build 与 Submission Readiness 检查（build-and-submission-audit.md）

## texra-ai/texra-scientific-skills

- 仓库：https://github.com/texra-ai/texra-scientific-skills
- 许可证：MIT © texra-ai
- 吸收机制：
  - Writing Commenter：logical-flow / notation / editorial 三模式评论纪律（revision-and-review.md）
  - Scientific Simplifier：prose 简化规则与保义回退（revision-and-review.md）
  - Manuscript Review 的 findings-first 报告格式（revision-and-review.md）
  - Literature Search 的 source-evaluation 思想（另见 `../math-related-work/references/provenance.md`）

## 使用注意

- 依据 MIT 许可证，可自由使用、修改、合并与商用，但须保留上述版权与许可声明。
- 未并入本 skill 的仓库内容（如 math-beamer 演示文稿、mathematical-enhancer 数学强化）不属于写作层，未吸收。
