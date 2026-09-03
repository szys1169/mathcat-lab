# PDF → PPTX 导出与讲稿格式

## 原理

Beamer 公式是 LaTeX 排版,PPT 无法直接承载,所以导出策略是:**PDF 每页渲染成高清 PNG,按原宽高比铺满 PPTX 幻灯片,再把逐页讲稿写进备注区**。需要在 PPT 里改公式时,改回 `slides.tex` 重新编译导出,而不是直接编辑 PPT 里的图。

## 依赖

`python-pptx` + `PyMuPDF`,由 `scripts/check_environment.py` 检查。

## 讲稿格式(二选一)

Markdown(推荐,agent 直接写):

```markdown
## Slide 1
这一页讲背景:……

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
python scripts/render_check.py slides.pdf --out page_images --dpi 150
```

`export_pptx.py` 会自动读取 PDF 页宽高比设定 PPTX 尺寸(模板 4:3 或 16:9 都能保持原比例,不拉伸)。

## 交付检查

- 打开 `final.pptx` 确认每页图清晰、无黑边、无拉伸。
- 备注区能看到讲稿;页数与 PDF 一致。
- `speaker_notes.md` 保留在输出目录,作为独立讲稿。
