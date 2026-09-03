# Math Lab Platform

独立本地数学智能体平台。它从同级 `../capabilities/capability-index.json` 发现能力，通过 Codex CLI 执行任务，并在 `runtime-data/state.json` 持久化工作区、对话和消息。

## 启动

双击 `start-platform.cmd`，访问 `http://127.0.0.1:4321/`。脚本会先启动同级 `../math-research-mvp` 的新版 MathCat API（`127.0.0.1:8787`），再启动网页平台，并检查两端连接健康。Node.js 需在 PATH 中；MathCat 首次部署前需要已有 release 构建。

## 对话记录

左侧按照“工作区 → 对话记录”两级组织。点击工作区可以展开或折叠其对话，也可以点击工作区右侧的“＋”直接在该工作区新建对话。工作区和历史消息保存在 `runtime-data/state.json`。

## 数学能力

功能菜单采用 MathCat 猫猫图标，提供文献调研、数学研究、相关工作、论文写作、论文修改、PPT 生成和实验性的全自动编排。能力入口由同级 `capabilities/capability-index.json` 注册；研究喵可选择本地 MathCat、Rethlas 或 Danus，三者统一由 Codex CLI 调度，其余能力引用本机最新的平台集成 Skill。

## 封装边界

- 平台代码和运行数据全部位于本目录。
- 用户成果只写入其选择的工作区。
- 能力代码只来自同级 `capabilities`，平台不复制 Skill。
- `mathcat-lab-main` 不是运行依赖。
