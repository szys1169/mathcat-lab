# Rethlas 外部智能体接入契约

MathCat Lab、Codex Skill 和 Harness 插件只拥有调用边界，不拥有 Rethlas 源码。接入层必须完成：问题暂存、后端预检、安全进程启动、验证端口隔离、超时/停止、结果复制、日志脱敏和最终报告。

Rethlas 的 generation 和 verification 均通过本地 Codex CLI 执行，并沿用当前 Codex 配置。接入层不得静默切换 provider，也不得把认证信息写进任务目录或报告。

平台可以持续轮询进程状态，但不得通过读取半写入报告推断成功。成功只由经过校验的 `blueprint_verified.md` 判定；普通 blueprint 属于部分成果。
