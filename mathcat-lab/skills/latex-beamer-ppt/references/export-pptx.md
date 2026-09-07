# PDF → PPTX 导出与讲稿格式

## 原理

Beamer 公式是 LaTeX 排版,PPT 无法直接承载,所以导出策略是:**PDF 每页渲染成高清 PNG,按原宽高比铺满 PPTX 幻灯片,再把逐页讲稿写进备注区**。因此 `final.pptx` 的页面主体不是可逐元素编辑的原生 PowerPoint 对象；这一限制必须在 `ppt-task-report.md` 中明示。需要改公式或版式时，应修改 `slides.tex` 后重新编译导出。

## 依赖

`python-pptx` + `PyMuPDF`,由 `scripts/check_environment.py` 检查。

## 讲稿格式(二选一)

Markdown(推荐,agent 直接写):

```markdown
## Slide 1
预计用时：40 秒
必须讲：这一页建立问题背景与报告主线。
讲稿：……
过渡句：接下来先统一符号。
[Sources] source.pdf, p. 1

## Slide 2
这一页讲定义:……
```

JSON:

```json
{"slides": [{"title": "背景", "notes": "这一页讲背景:……"}]}
```

编号从 1 开始,与 PDF 页序一一对应;缺页或多余页导出时会警告,交付前必须对齐。

## 用法

```bash
python scripts/export_pptx.py slides.pdf --notes speaker_notes.md --out final.pptx
python scripts/render_check.py slides.pdf --out page_images --dpi 150 --strict-layout
```

`export_pptx.py` 会自动读取 PDF 页宽高比设定 PPTX 尺寸(模板 4:3 或 16:9 都能保持原比例,不拉伸)。

## 交付检查

- 打开 `final.pptx` 确认每页图清晰、无黑边、无拉伸。
- 备注区能看到讲稿;页数与 PDF 一致。
- `speaker_notes.md` 保留在输出目录,作为独立讲稿。
- 每页备注必须有 `预计用时`、`必须讲`、`过渡句`、`[Sources]`；无外部来源时写 `[Sources] none (title/transition slide)`，不得留空或杜撰。
- `ppt-task-report.md` 必须写明 PPTX 为逐页高清图导出，内容修改应回到 `slides.tex`。
