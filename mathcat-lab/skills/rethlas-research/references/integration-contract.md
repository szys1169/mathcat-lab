# Rethlas 外部智能体接入契约

MathCat Lab、Codex Skill 和 Harness 插件只拥有调用边界，不拥有 Rethlas 源码。接入层必须完成：问题暂存、后端预检、安全进程启动、验证端口隔离、超时/停止、结果复制、日志脱敏和最终报告。

DeepSeek 路径通过 Codex CLI 的重复 `--config` 参数注入 `model_provider=deepseek`、`preferred_auth_method=apikey`、`forced_login_method=api`、`model_catalog_json` 与 `[model_providers.deepseek]`；密钥只通过 `DEEPSEEK_API_KEY` 子进程环境传递。generation 和 verification 的参数必须一致。

平台可以持续轮询进程状态，但不得通过读取半写入报告推断成功。成功只由经过校验的 `blueprint_verified.md` 判定；普通 blueprint 属于部分成果。
