# 平台接入契约

## 一套 Skill，一个执行器

内容规则、脚本和输出契约由同一份 `SKILL.md` 定义，并通过本地 Codex CLI 调用同一组脚本。

- **Codex**：通过 `$latex-beamer-ppt` 调用；工作目录设为用户项目根目录。

平台适配器只负责传入任务、材料路径、输出目录和时限，不复制生成逻辑。

## MathCat Lab 输入

至少传入：

- `objective`：用户目标；
- `projectRoot`：项目工作区；
- `outputDir`：固定为项目的 `PPT/` 子目录；
- `executor`：固定为 `codex`；
- 可用材料路径，优先读取 `论文/`、`文献/`、`调研/`、`成果/`；
- 运行模式：交互或批处理。

不得把 `.platform/` 日志、历史模型输出或临时文件当作数学来源。

## 完成判据

平台只有在以下文件存在并通过 `validate_delivery.py` 时才能返回 `completed`：

1. `slides.tex`
2. `slides.pdf`
3. `page_images/page_*.png`，数量与 PDF 页数一致
4. `speaker_notes.md`，`## Slide N` 连续覆盖所有页面
5. `final.pptx`，页数与 PDF 一致
6. `slide_source_ledger.md`
7. `ppt-task-report.md`

缺少任一项时返回 `partial`；编译或导出失败时返回 `failed`。无论成功与否，都要写 `ppt-task-report.md`，记录执行器、输入来源、实际产物、检查结果、失败原因和建议的下一步。

## 推荐命令

```powershell
python <skill>/scripts/check_environment.py
python <skill>/scripts/build_slides.py --root <outputDir> --strict-overfull
python <skill>/scripts/render_check.py <outputDir>/slides.pdf --out <outputDir>/page_images
python <skill>/scripts/export_pptx.py <outputDir>/slides.pdf --notes <outputDir>/speaker_notes.md --out <outputDir>/final.pptx --strict-notes
python <skill>/scripts/validate_delivery.py <outputDir>
```

适配器应使用参数数组启动进程，不把用户输入拼接成 shell 命令。
