# macOS 使用说明

MathCat Lab 2.5.1 的核心服务支持 Apple Silicon 与 Intel Mac。平台只监听本机回环地址，运行数据保存在当前版本目录下。

## 准备

1. 安装 Xcode Command Line Tools：`xcode-select --install`。
2. 安装 Node.js 22 或更高版本，以及包含 Cargo 的 Rust 1.88 或更高版本。
3. 安装并登录 Codex CLI，确认 `codex --version` 可以在终端运行。
4. 论文/PPT 功能按需安装 Python 3、PyMuPDF、python-pptx 与 MacTeX；研究和白板本身不需要这些可选依赖。

## 启动与停止

```bash
bash scripts/start-version.sh
bash scripts/stop-version.sh
```

首次启动会执行 `npm ci --ignore-scripts`，并在没有本机后端程序时执行 Cargo release 构建。服务地址是 `http://127.0.0.1:4335/`。需要重新编译时使用：

```bash
bash scripts/start-version.sh --rebuild
```

若要使用 Finder 双击入口，先在仓库根目录执行：

```bash
chmod +x scripts/*.sh *.command
```

文件夹选择使用 macOS 的 `osascript` 对话框，“在 Finder 中显示”使用 `open -R`。停止脚本只处理 PID 文件指向且命令路径属于当前版本的进程；研究取消状态无法确认时会保留服务，供用户查看回执。

## 当前验证边界

2.5.1 已在 Windows 上完成 Node 语法、跨平台入口单元检查、Rust workspace 编译和原 Windows 启动器边界检查。仓库包含 `macos-14` GitHub Actions smoke workflow，用于在 macOS runner 上重复 Node、shell、Cargo 和平台定向检查。发布前仍应在一台 Apple Silicon Mac 上完成首次构建、文件夹选择、Finder 定位、启动和安全停止实机检查。
