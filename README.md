# MathCat Lab

MathCat Lab 是一个在本地运行的数学科研工作台，由三部分组成：

- `math-lab-platfrom/`：网页对话、研究白板、证明树、依赖图、猫猫提问卡和本地文件交互。
- `math-research-mvp/`：Rust 实现的 MathCat 研究智能体、可信 Fact Gate、独立验证、恢复与审计 API。
- `capabilities/`：文献调研、研究、相关工作、论文写作、论文修改和 Beamer 等能力入口。

默认研究后端是 Codex CLI。Planner、Worker 和来源记录只能产生候选或证据；只有通过独立验证与事务化 Fact Gate 的内容才能成为可信 Fact。

## 环境要求

- Windows 10/11 与 PowerShell 7
- Node.js 22+
- Rust 1.85+
- 已登录的 Codex CLI
- 可选：Lean 4.32.2、Mathlib 4.32.1、Pantograph 0.3.18

## 首次构建

```powershell
cd math-research-mvp
cargo build --release -p math-research-agent

cd ..\math-lab-platfrom
npm install
Copy-Item .env.example .env.local
```

可以让 `MATHCAT_API_TOKEN` 保持为空；首次运行启动脚本时会自动生成随机本地令牌并写入 `.env.local`。该文件不会被 Git 跟踪。

## 启动

在仓库根目录运行：

```powershell
.\start-mathcat-lab.ps1
```

平台地址为 <http://127.0.0.1:4321/>，MathCat API 默认为 <http://127.0.0.1:8787/>。

## 测试

```powershell
cd math-research-mvp
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

cd ..\math-lab-platfrom
npm test
```

## 发布包边界

本仓库不包含本地令牌、用户对话、研究数据库、生成工件、模型缓存、`node_modules`、Rust `target` 或 Mathlib/Pantograph 的 `.lake` 构建缓存。首次使用时由本机重新生成这些内容。

更多信息见 [MathCat 后端说明](math-research-mvp/README.md) 和 [平台说明](math-lab-platfrom/README.md)。
