---
name: autonomous-math-research
description: Orchestrate an end-to-end local mathematics workflow from a concrete research question through Rethlas research, evidence-backed literature positioning, and a versioned paper draft. Use only when the user explicitly selects the experimental full-auto capability; not for ordinary chat or a single-stage deliverable.
---

# 全自动数学研究（实验性）

在用户选择的工作区中编排已有本地能力。研究阶段必须使用 Rethlas；后续阶段只消费已经形成且可追溯的结果，不得把猜测升级为定理。

## 工作流

1. 清点工作区材料，明确问题、假设、已有结果和期望交付。缺少决定性输入时，使用平台候选选择协议暂停并让用户选择。
2. 读取并遵循仓库根目录下的 `skills/rethlas-research/SKILL.md`，运行研究阶段，保存证明蓝图、验证状态和日志。
3. 读取并遵循仓库根目录下的 `skills/math-literature-research/SKILL.md`，为研究结论建立来源台账、定理台账和文献定位。外部检索不可用时明确标记证据限制。
4. 只有已证明或有可靠来源归属的内容可以进入论文。读取并遵循仓库根目录下的 `capabilities/math-paper-writing/SKILL.md`，生成不覆盖旧成果的版本化论文。
5. 若任务需要单独的 Related Work，读取仓库根目录下的 `skills/math-related-work/SKILL.md`；若用户明确要求演示文稿，再读取仓库根目录下的 `skills/latex-beamer-ppt/SKILL.md`。

## 停止条件

- Rethlas 未完成证明时，可以交付研究报告和开放缺口，但不得继续宣称论文中的主要结论已成立。
- 来源不足时保留 `evidence_limited` 或等价标记，不虚构引用。
- 任一阶段需要覆盖现有成果、扩大工作区权限或作出关键内容选择时暂停并请求用户决定。
- 最终报告列出各阶段状态、采用的输入、生成物路径、未解决问题和被跳过的阶段。
