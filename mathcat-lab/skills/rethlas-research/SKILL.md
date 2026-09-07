---
name: rethlas-research
description: Run the local Rethlas mathematical research agent for a concrete problem, preserve the selected Codex or DeepSeek provider, wait until solved, timed out, stopped, or failed, then archive and report its proof blueprint, verified blueprint, memory, and logs. Use for MathCat Lab 研究 tasks; not for ordinary chat, literature surveys, or paper writing.
---

# Rethlas 数学研究

这是本地 Rethlas 的薄包装，不把 Rethlas 源码嵌入 Codex、DeepSeek Harness 或 MathCat UI。任务执行器负责把问题写入 Rethlas、启动外部 agent、等待终态、停止进程树、读取研究报告并整理成果。

## 执行器纯度

- `executor=codex`：Rethlas generation 与 verification 均使用 Codex 模型/provider。
- `executor=deepseek_harness`：仍以 Codex CLI 作为 agent harness，但 generation 与 verification 均显式注入 DeepSeek Responses provider，并从 `DEEPSEEK_API_KEY` 读取密钥。
- 不允许选择一个执行器后静默回退到另一个；provider 与模型必须写进任务报告，但不得写入密钥。

## 生命周期

1. 运行 `scripts/preflight.mjs` 检查 Rethlas 根目录、两个 `AGENTS.md`、模型目录、Codex CLI、DeepSeek Key（若需要）和 8091 端口。
2. 把明确数学问题与任务元数据暂存到 Rethlas `agents/generation/data/platform/<project>/<task-id>.md`；附件只复制用户明确授权的引用材料。
3. 调用平台提供的安全 Rethlas adapter。独立使用时必须使用同等安全的 adapter；若不存在，不要临时拼装无沙箱命令，报告 `preflight_failed`。
4. generation 与 verification 必须使用相同 provider 配置；验证服务仅绑定 `127.0.0.1`，默认端口 8091。
5. 默认上限 120 分钟。问题解决、达到时间上限、用户主动停止、依赖失败都属于合法终态，必须保存已有产物。
6. 把 Rethlas 结果复制到用户工作区，不把 Rethlas 内部目录当作最终交付位置。
7. 运行 `scripts/validate-delivery.mjs`，无论成功与否都生成 `research-task-report.md`。

## 交付

```text
调研/Rethlas/<task-id>/
├─ problem.md
├─ provider-preflight.json
└─ run-manifest.json

成果/rethlas/<task-id>/
├─ blueprint.md                    # 可能存在
├─ blueprint_verified.md           # 仅验证成功时存在
├─ memory/                          # 已有记忆/检查点
├─ logs/                            # 脱敏后的迭代日志
└─ research-task-report.md          # 所有终态必需
```

`blueprint_verified.md` 表示 Rethlas 自然语言验证通过，不等同于 Lean 形式化证明或人类同行评审。没有 verified blueprint 时只能报告 `partial/unsolved/failed/stopped/time_limit`，不得声称已经解决问题。

## 禁止事项

- 不修改或内嵌 Rethlas 源码；不绕过其 generation/verification 分工。
- 不把 API Key、Authorization header、完整私有会话或未脱敏环境变量写进成果。
- 不把“生成了 blueprint”升级成“证明已验证”。
- 不在 8091 已占用时杀死未知进程；报告占用者并等待用户处理。
- 不覆盖旧任务；每次调用使用独立 task/attempt 目录。
