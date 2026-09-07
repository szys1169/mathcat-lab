# MathCat Lab

MathCat Lab 是一个在本地运行的数学科研工作台：让研究智能体探索问题，让用户通过白板查看进展、审查分工、提出建议，并将已有材料整理成论文或汇报。

当前源码版本为 **2.5.0 demo**，适合在已配置的 Windows 环境中开展小规模、有协助的用户试用。当前采用本地单人实例，不提供多人账号与租户隔离。

## 可以做什么

- **数学研究**：领研猫研究并安排伙伴猫；审核猫检查候选成果，顾问猫审视路线，展示猫与记忆猫整理进展和经验。
- **人机协作白板**：分别呈现精确数学问题与其他要求；查看研究角色和公开执行记录，通过证明树查看成果与依赖，记录开放疑点、失败路线和时间线。
- **参与研究**：启动时选择“自动安排分工”或“每轮分工由我批准”，设置本轮时长和伙伴猫上限；研究中提出建议、处理待办，暂停或恢复当前项目的全部研究。
- **文献与写作**：文献调研、相关工作、基于已有成果的中英文论文、论文审查与修改、Beamer / PPT 汇报。
- **成果管理**：统一成果入口、文件预览与下载、版本目录浏览、交付步骤局部重试，以及对话和工作区的保存重开。
- **模型与额度**：显示当前模型、每周剩余额度和重置时间；模型选择在后续调用生效。

候选成果、经过模型审查的成果与形式化证明需要区分。独立的自然语言审查不等于形式化验证；论文生成也不等于证明正确。本 demo 不承诺在给定时间内解决任意猜想。

## 环境准备

目前维护的启动入口面向 **Windows 10/11 + PowerShell**。本次检查使用 Node.js 24 和 Rust 1.98；前端声明的最低 Node 版本为 22，建议首次部署采用已验证的工具链。

基础功能需要：

- Node.js 与 npm。
- Rust / Cargo；Windows MSVC 工具链还需要 Visual Studio C++ Build Tools。
- 已安装、完成登录且可以正常调用的 Codex CLI。模型与额度读取依赖本机 CLI 的 app-server 接口。

论文编译、PDF 检查和 PPT 导出另外需要：

- Python 3.10+，以及 `PyMuPDF`、`python-pptx`。
- MiKTeX 或 TeX Live；确保 `xelatex`、`pdflatex` 和 `latexmk` 可用，安装中文 `ctex`、Beamer 及文献处理所需宏包。

```powershell
python -m pip install PyMuPDF python-pptx
```

这些工具不会随仓库一起提交。首次 LaTeX 编译可能需要下载宏包。需要指定可执行文件时，可在启动前设置当前终端的 `CODEX_BIN`、`MATH_LAB_PYTHON` 环境变量。

## 获取和启动

```powershell
git clone https://github.com/szys1169/mathcat-lab.git
cd mathcat-lab
.\scripts\start-version.ps1
```

也可双击根目录的 **打开MathCat-Lab-2.5.0.cmd**。启动器会安装缺失的前端依赖、在缺少程序时构建 Rust 后端，并生成本地连接凭据。首次构建可能需要数分钟，不需要自己填写令牌。

- 平台：<http://127.0.0.1:4334/>
- 研究服务：`http://127.0.0.1:8899`
- 不自动打开浏览器：`.\scripts\start-version.ps1 -NoBrowser`
- 关闭当前实例：`.\scripts\stop-version.ps1`，或双击 **停止MathCat-Lab-2.5.0.cmd**。

更新源码后，先使用停止入口关闭本实例，再执行 `.\scripts\start-version.ps1 -Rebuild`。同一台电脑请勿同时启动两个使用上述相同端口的副本。

## 第一次使用

1. 新建或选择一个空工作区，然后新建对话。
2. 普通讨论可以直接输入问题；研究时选择研究能力，在启动面板选择分工审核方式、时间上限和伙伴猫上限。
3. **第一次研究建议直接粘贴完整题面**，包括假设、量词与目标。已有论文或材料放入所选工作区，并在消息中明确文件名；选择工作区本身不意味着全部文件都已导入研究证据。
4. 打开白板，检查顶部整理出的数学问题是否准确，再查看角色进展、证明树与待处理事项。需要参与时提交建议或批准分工。
5. 研究结束后，从统一成果入口查看阶段报告与审查状态。有已有成果时，可以另行请求“根据现有材料帮我生成论文”或“根据现有材料帮我生成汇报 PPT”。

暂停期间研究时限继续计时；重新打开页面不会自动恢复暂停的研究。详情见 [使用说明](docs/USER-GUIDE.md)。

## 源码结构与数据位置

| 目录 | 内容 |
|---|---|
| `math-lab-platfrom/` | Node 平台、前端白板与能力执行适配；目录名保留历史拼写 |
| `math-research-mvp/` | Rust 研究引擎、会话与审查流程、存储及 API；当前入口为 `mathcat-v2` |
| `capabilities/` | 平台能力清单、研究工具与验证适配材料 |
| `mathcat-lab/` | 随仓库提供的 skill、模板与论文写作 0.4.0 工作流 |
| `scripts/`、`tests/` | 启动/停止脚本与可复用的离线检查 |
| `docs/` | 使用、架构与已验证范围 |

本机运行后，`workspaces/` 保存用户工作区与成果，`runtime/` 保存研究数据库、连接凭据和服务日志，`math-lab-platfrom/runtime-data/` 保存平台对话等状态。备份时一起保存这些目录，并在复制数据库前停止当前实例。

本仓库只提交源码、必要静态资源、模板、依赖锁文件和可复用测试。**不包含凭据、用户对话、研究成果、真实测试记录、数据库、构建程序、`target` 或 `node_modules`。** KaTeX 静态资源用于本地公式显示，因此保留其字体与许可。私有机构模板不随仓库分发，PPT 默认使用通用模板。

## 检查与已知边界

当前开发版已经做过猜想 3.6 的真实功能流程测试，以及自动安排、人工批准两种研究模式的短时测试；随后修复了题面审查材料传递、无效等待计划、状态残留、成果链接和白板轮询交互等问题。最新修复通过了定向回归、构建和实际页面检查。

这不代表完整验收或数学能力基准已经通过。复杂伙伴协作、长周期顾问、崩溃恢复等流程仍需更多实测；历史完整测试中的旧界面/旧默认值断言尚待适配。详情见 [验证范围](docs/VALIDATION.md) 与 [架构说明](docs/ARCHITECTURE.md)。

不调用模型的基础检查：

```powershell
node scripts/run-validation.mjs --only=rust-format,research-tools,launcher-tests

cd math-lab-platfrom
node --test test/research-conversation-status.test.mjs test/research-material-role.test.mjs test/research-delivery.test.mjs test/workspace-files.test.mjs test/workspace-links.test.mjs public/test/whiteboard24.test.mjs
```

仓库保留更完整的测试与编译入口，按需要运行。真实模型调用会使用本机账号额度；不要将离线夹具通过当作真实研究成功。
