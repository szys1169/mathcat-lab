# 平台接入契约

## 一套 Skill，两种执行器

内容规则、脚本和输出契约不依赖模型提供方。两种执行器都应读取同一份 `SKILL.md`，并调用同一组本地脚本。

- **Codex**：通过 `$latex-beamer-ppt` 调用；工作目录设为用户项目根目录。
- **DeepSeek Harness**：由 Harness 插件或小智能体把 `SKILL.md` 作为任务指令加载，再在允许写入的工作区调用脚本。不要让 DeepSeek API 直接访问用户文件系统；文件读取与脚本执行由本地 Harness/CLI 负责。

平台适配器只负责传入任务、材料路径、输出目录和时限，不复制生成逻辑。

## MathCat Lab 输入

至少传入：

- `objective`：用户目标；
- `projectRoot`：项目工作区；
- `outputDir`：固定为项目的 `PPT/` 子目录；
- `executor`：`codex` 或 `deepseek_harness`；
- 可用材料路径，优先读取 `论文/`、`文献/`、`调研/`、`成果/`；
- 运行模式：交互或批处理。

不得把 `.platform/` 日志、历史模型输出或临时文件当作数学来源。

## 完成判据

平台只有在以下文件存在并通过 `validate_delivery.py` 时才能返回 `completed`：

1. `slides.tex`
2. `slides.pdf`
3. `render_report.json`，状态通过、页数与当前 PDF 一致且 PDF 哈希匹配
4. `speaker_notes.md`，`## Slide N` 连续覆盖所有页面
5. `final.pptx`，页数与 PDF 一致
6. `slide_source_ledger.md`
7. `ppt-task-report.md`

缺少任一项时返回 `partial`；编译或导出失败时返回 `failed`。无论成功与否，都要写 `ppt-task-report.md`，记录执行器、输入来源、实际产物、检查结果、失败原因和建议的下一步。

## 推荐命令

```powershell
$python = "<platform-configured-python>"
& $python <skill>/scripts/check_environment.py
& $python <skill>/scripts/build_slides.py --root <outputDir> --strict-overfull
& $python <skill>/scripts/render_check.py <outputDir>/slides.pdf --out <outputDir>/page_images
& $python <skill>/scripts/export_pptx.py <outputDir>/slides.pdf --notes <outputDir>/speaker_notes.md --out <outputDir>/final.pptx --strict-notes
& $python <skill>/scripts/validate_delivery.py <outputDir> --cleanup-page-images
```

`page_images/page_*.png` 只在视觉检查期间存在。成功验收后清理，避免平台把每一页都展示为独立成果；检查失败时保留图片用于排错。

适配器应使用参数数组启动进程，不把用户输入拼接成 shell 命令。
