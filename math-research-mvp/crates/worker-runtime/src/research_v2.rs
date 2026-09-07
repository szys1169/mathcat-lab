//! Streaming, cancelable Codex turns for the independent v2 research controller.
#![allow(
    clippy::missing_errors_doc,
    clippy::too_many_lines,
    clippy::needless_pass_by_value
)]

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionBinding {
    pub project_id: String,
    pub session_id: String,
    pub role: String,
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    pub workspace: PathBuf,
    pub problem_version: u64,
    pub native_session_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TurnRequest {
    pub binding: SessionBinding,
    pub prompt: String,
    pub timeout_seconds: u64,
    pub binding_path: PathBuf,
    pub control_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct TurnOutput {
    pub native_session_id: Option<String>,
    pub text: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub reconstructed: bool,
}

#[derive(Debug, Clone)]
pub struct TurnError {
    pub code: String,
    pub message: String,
}
impl TurnError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl std::fmt::Display for TurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for TurnError {}

#[async_trait]
pub trait V2Backend: Send + Sync {
    async fn preflight(&self) -> Result<Value, TurnError>;
    async fn run_turn(
        &self,
        request: TurnRequest,
        events: mpsc::Sender<Value>,
        cancel: CancellationToken,
    ) -> Result<TurnOutput, TurnError>;
}

#[derive(Clone, Debug)]
pub struct CodexV2Backend {
    pub command: PathBuf,
}

/// Validated process-local overrides. This never writes the user's Codex configuration.
pub fn execution_overrides(
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<Vec<String>, TurnError> {
    let mut args = Vec::new();
    if let Some(model) = model {
        if model.is_empty()
            || model.len() > 200
            || !model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._:/".contains(&b))
        {
            return Err(TurnError::new(
                "INVALID_MODEL_CONFIG",
                "模型 ID 必须为非空的有效标识符",
            ));
        }
        args.extend(["--model".into(), model.into()]);
    }
    if let Some(effort) = effort {
        if !matches!(
            effort,
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
        ) {
            return Err(TurnError::new(
                "INVALID_MODEL_CONFIG",
                "reasoning_effort 必须为 none/minimal/low/medium/high/xhigh/max/ultra",
            ));
        }
        args.extend([
            "--config".into(),
            format!("model_reasoning_effort=\"{effort}\""),
        ]);
    }
    Ok(args)
}

/// Only public, allowlisted fields cross the executor boundary. Never forward raw JSONL.
#[must_use]
pub fn public_event(raw: &Value) -> Option<Value> {
    let kind = raw["type"].as_str()?;
    if kind == "thread.started" {
        return Some(json!({"type":"session.bound","native_session_id":raw["thread_id"]}));
    }
    if kind == "turn.completed" {
        return Some(json!({"type":"usage.updated","usage":raw["usage"]}));
    }
    if matches!(kind, "error" | "turn.failed") {
        return Some(json!({"type":"activity.error","message":"执行器返回错误；已保留受限诊断。"}));
    }
    let item = &raw["item"];
    let item_kind = item["type"].as_str()?;
    match item_kind {
        "agent_message" => Some(
            json!({"type":if kind == "item.completed" {"message.completed"} else {"message.delta"},"message_id":item["id"],"text":redact(item["text"].as_str().unwrap_or_default())}),
        ),
        "reasoning" => Some(
            json!({"type":"activity.delta","message_id":item["id"],"text":redact(item["text"].as_str().unwrap_or_default()),"source":"provider_public_summary"}),
        ),
        "command_execution" | "mcp_tool_call" | "web_search" | "file_change" => Some(
            json!({"type":if kind == "item.completed" {"tool.completed"} else {"tool.started"},"item_id":item["id"],"tool":item_kind,"status":item["status"],"exit_code":item["exit_code"]}),
        ),
        _ => None,
    }
}

#[must_use]
pub fn redact(text: &str) -> String {
    // Secret-like payloads are suppressed conservatively, not copied to a browser log.
    text.lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if [
                "api_key",
                "api-key",
                "authorization:",
                "bearer ",
                "sk-",
                "access_token",
                "refresh_token",
                "password=",
            ]
            .iter()
            .any(|word| lower.contains(word))
            {
                "[敏感字段已隐藏]"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn save_binding(path: &std::path::Path, binding: &SessionBinding) -> Result<(), TurnError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(io_error)?;
    }
    let bytes =
        serde_json::to_vec(binding).map_err(|e| TurnError::new("PROTOCOL", e.to_string()))?;
    // The controller serializes turns for a session. A temporary file ensures readers never see half JSON.
    let temp = path.with_extension("tmp");
    tokio::fs::write(&temp, bytes).await.map_err(io_error)?;
    if path.exists() {
        tokio::fs::remove_file(path).await.map_err(io_error)?;
    }
    tokio::fs::rename(temp, path).await.map_err(io_error)
}
fn io_error(e: std::io::Error) -> TurnError {
    TurnError::new("RESOURCE", e.to_string())
}

#[derive(Default)]
struct StreamEvidence {
    started: bool,
    completed: bool,
    failed: Option<String>,
    diagnostics: Vec<String>,
}

fn bounded_diagnostic(text: &str) -> String {
    redact(text).chars().take(8192).collect()
}

// Tool outputs may be repeated in native JSONL (stdout and aggregated_output).
// Keep their lifecycle metadata without allocating or forwarding the raw payload.
#[derive(Default, Serialize)]
struct EventItem {
    id: Value,
    #[serde(rename = "type")]
    kind: Value,
    text: Value,
    status: Value,
    exit_code: Value,
}

// Codex CLI 0.153.3 emits web_search with both a lifecycle id (item_N) and an
// executor id (exec-...) under the same key. Preserve the first for event joins.
// Deserialize the allowlist directly so large, unknown tool payloads stay skipped.
impl<'de> Deserialize<'de> for EventItem {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::{Error as _, IgnoredAny, MapAccess, Visitor};
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Id,
            #[serde(rename = "type")]
            Kind,
            Text,
            Status,
            ExitCode,
            #[serde(other)]
            Other,
        }
        struct ItemVisitor;
        impl<'de> Visitor<'de> for ItemVisitor {
            type Value = EventItem;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a Codex event item object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<EventItem, A::Error> {
                let mut ids = Vec::<Value>::new();
                let (mut kind, mut text, mut status, mut exit_code) = (None, None, None, None);
                while let Some(field) = map.next_key::<Field>()? {
                    let (slot, name) = match field {
                        Field::Id => {
                            if ids.len() == 2 {
                                return Err(A::Error::duplicate_field("id"));
                            }
                            ids.push(map.next_value()?);
                            continue;
                        }
                        Field::Kind => (&mut kind, "type"),
                        Field::Text => (&mut text, "text"),
                        Field::Status => (&mut status, "status"),
                        Field::ExitCode => (&mut exit_code, "exit_code"),
                        Field::Other => {
                            let _: IgnoredAny = map.next_value()?;
                            continue;
                        }
                    };
                    if slot.is_some() {
                        return Err(A::Error::duplicate_field(name));
                    }
                    *slot = Some(map.next_value::<Value>()?);
                }
                let kind = kind.unwrap_or_default();
                if ids.len() == 2 {
                    let lifecycle = ids[0].as_str().and_then(|v| v.strip_prefix("item_"));
                    let execution = ids[1].as_str().and_then(|v| v.strip_prefix("exec-"));
                    if kind != "web_search"
                        || lifecycle
                            .is_none_or(|v| v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()))
                        || execution.is_none_or(str::is_empty)
                    {
                        return Err(A::Error::custom(
                            "duplicate item id is not the supported web_search lifecycle/executor pair",
                        ));
                    }
                }
                Ok(EventItem {
                    id: ids.into_iter().next().unwrap_or_default(),
                    kind,
                    text: text.unwrap_or_default(),
                    status: status.unwrap_or_default(),
                    exit_code: exit_code.unwrap_or_default(),
                })
            }
        }
        deserializer.deserialize_map(ItemVisitor)
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct ExecutorEvent {
    #[serde(rename = "type")]
    kind: Value,
    thread_id: Value,
    usage: Value,
    error: Value,
    message: Value,
    item: Option<EventItem>,
}

const EVENT_MEMORY_LIMIT: usize = 1024 * 1024;
const EVENT_STORAGE_LIMIT: usize = 64 * 1024 * 1024;

async fn read_executor_event<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    spool_directory: &std::path::Path,
) -> Result<Option<Value>, TurnError> {
    let mut line = Vec::new();
    let mut bytes = 0_usize;
    let mut spool: Option<(tokio::fs::File, tempfile::NamedTempFile)> = None;
    loop {
        let available = reader.fill_buf().await.map_err(io_error)?;
        if available.is_empty() {
            break;
        }
        let size = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |i| i + 1);
        bytes = bytes.saturating_add(size);
        if bytes > EVENT_STORAGE_LIMIT {
            return Err(TurnError::new(
                "RESOURCE",
                "单个执行器事件超过 64 MiB 临时存储上限，交付情况须以终态证据判断",
            ));
        }
        if bytes > EVENT_MEMORY_LIMIT && spool.is_none() {
            tokio::fs::create_dir_all(spool_directory)
                .await
                .map_err(io_error)?;
            let file = tempfile::Builder::new()
                .prefix("mathcat-event-")
                .tempfile_in(spool_directory)
                .map_err(io_error)?;
            let mut writer = tokio::fs::File::from_std(file.reopen().map_err(io_error)?);
            writer.write_all(&line).await.map_err(io_error)?;
            line.clear();
            line.shrink_to_fit();
            spool = Some((writer, file));
        }
        let finished = available[size - 1] == b'\n';
        if let Some((writer, _)) = &mut spool {
            writer
                .write_all(&available[..size])
                .await
                .map_err(io_error)?;
        } else {
            line.extend_from_slice(&available[..size]);
        }
        reader.consume(size);
        if finished {
            break;
        }
    }
    if bytes == 0 {
        return Ok(None);
    }
    let parsed = if let Some((mut writer, file)) = spool {
        writer.flush().await.map_err(io_error)?;
        drop(writer);
        tokio::task::spawn_blocking(move || {
            let reader = std::io::BufReader::new(file.reopen().map_err(io_error)?);
            // serde skips all unknown fields, including multi-megabyte tool strings.
            let event: ExecutorEvent = serde_json::from_reader(reader)
                .map_err(|e| TurnError::new("PROTOCOL", format!("执行器 JSONL 结构无效：{e}")))?;
            serde_json::to_value(event).map_err(|e| TurnError::new("PROTOCOL", e.to_string()))
        })
        .await
        .map_err(|e| TurnError::new("RESOURCE", e.to_string()))??
    } else {
        serde_json::from_slice::<ExecutorEvent>(&line)
            .and_then(serde_json::to_value)
            .map_err(|error| {
                TurnError::new(
                    "PROTOCOL",
                    format!(
                        "执行器 JSONL 结构无效：{error}；事件：{}",
                        bounded_diagnostic(&String::from_utf8_lossy(&line))
                    ),
                )
            })?
    };
    Ok(Some(parsed))
}

fn finalize_turn(
    result: Result<TurnOutput, TurnError>,
    success: bool,
    status_code: Option<i32>,
    natural_exit: bool,
    evidence: &StreamEvidence,
    stderr: &str,
) -> Result<TurnOutput, TurnError> {
    let diagnostics = bounded_diagnostic(&format!(
        "exit_code={status_code:?}; stderr={} ; public_errors={}",
        bounded_diagnostic(stderr),
        evidence.diagnostics.join(" | ")
    ));
    if let Err(error) = &result {
        if matches!(error.code.as_str(), "CANCELLED" | "DEADLINE_REACHED") {
            return Err(TurnError::new(
                &error.code,
                format!("{}；{diagnostics}", error.message),
            ));
        }
    }
    if let Some(failure) = &evidence.failed {
        return Err(TurnError::new(
            "TURN_FAILED",
            format!("执行器明确报告 turn.failed：{failure}；{diagnostics}"),
        ));
    }
    let lower = stderr.to_ascii_lowercase();
    let definite_startup_error = [
        "unexpected argument",
        "unrecognized subcommand",
        "usage: codex",
        "error parsing config",
        "failed to load config",
        "not logged in",
        "authentication required",
        "is not recognized as an internal or external command",
        "the system cannot find the path specified",
        "系统找不到指定的路径",
        "不是内部或外部命令",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    if !evidence.started && !success && natural_exit && definite_startup_error {
        return Err(TurnError::new(
            "EXECUTOR_START_FAILED",
            format!("执行器明确未进入研究调用；{diagnostics}"),
        ));
    }
    if !evidence.completed {
        let detail = result
            .as_ref()
            .err()
            .map_or("未收到 turn.completed 或 turn.failed".into(), |error| {
                format!("{}：{}", error.code, error.message)
            });
        return Err(TurnError::new(
            "DELIVERY_UNCERTAIN",
            format!("{detail}；不能确认交付，禁止盲目重发；{diagnostics}"),
        ));
    }
    let output = result.map_err(|error| {
        TurnError::new(&error.code, format!("{}；{diagnostics}", error.message))
    })?;
    if !success {
        return Err(TurnError::new(
            "PROVIDER_UNAVAILABLE",
            format!("研究终态已收到，但执行器异常退出；{diagnostics}"),
        ));
    }
    Ok(output)
}

/// Session identity stays fixed on resume; model and effort are process-local invocation options.
pub async fn validate_binding(request: &TurnRequest) -> Result<(), TurnError> {
    if request.binding.native_session_id.is_none() {
        return Ok(());
    }
    let bytes = tokio::fs::read(&request.binding_path)
        .await
        .map_err(|_| TurnError::new("SESSION_UNAVAILABLE", "持久会话凭据不可读；需要显式重建"))?;
    let recorded: SessionBinding = serde_json::from_slice(&bytes)
        .map_err(|_| TurnError::new("SESSION_UNAVAILABLE", "会话凭据损坏"))?;
    let mut identity = request.binding.clone();
    identity.model.clone_from(&recorded.model);
    identity
        .reasoning_effort
        .clone_from(&recorded.reasoning_effort);
    if recorded != identity {
        return Err(TurnError::new(
            "SESSION_BINDING_MISMATCH",
            "会话归属、角色、题目版本、原生会话或工作区不匹配",
        ));
    }
    if let Some(id) = &recorded.native_session_id {
        super::validate_session_id(id).map_err(|e| TurnError::new("PROTOCOL", e.to_string()))?;
    }
    Ok(())
}

#[async_trait]
impl V2Backend for CodexV2Backend {
    async fn preflight(&self) -> Result<Value, TurnError> {
        let mut command = super::codex_process_command(&self.command);
        command.arg("--version").kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(15), command.output())
            .await
            .map_err(|_| TurnError::new("PROVIDER_UNAVAILABLE", "执行器版本探测超时"))?
            .map_err(|e| TurnError::new("PROVIDER_UNAVAILABLE", e.to_string()))?;
        if !output.status.success() {
            return Err(TurnError::new(
                "PROVIDER_UNAVAILABLE",
                "Codex CLI 版本探测失败",
            ));
        }
        Ok(
            json!({"adapter":"codex_cli","version":redact(&String::from_utf8_lossy(&output.stdout)),"streaming_public_events":"supported","native_resume":"supported","interrupt":"supported","usage_reporting":"degraded","workspace_isolation":"degraded","enforceable_cost_cap":"unsupported","enforceable_token_cap":"unsupported","authentication":"not_probed"}),
        )
    }

    async fn run_turn(
        &self,
        mut request: TurnRequest,
        events: mpsc::Sender<Value>,
        cancel: CancellationToken,
    ) -> Result<TurnOutput, TurnError> {
        if cancel.is_cancelled() {
            return Err(TurnError::new("CANCELLED", "调用已在派发前取消"));
        }
        let model_args = execution_overrides(
            request.binding.model.as_deref(),
            request.binding.reasoning_effort.as_deref(),
        )?;
        tokio::fs::create_dir_all(&request.binding.workspace)
            .await
            .map_err(io_error)?;
        request.binding.workspace = tokio::fs::canonicalize(&request.binding.workspace)
            .await
            .map_err(io_error)?;
        validate_binding(&request).await?;
        let mut command = super::codex_process_command(&self.command);
        command
            .arg("exec")
            .arg("--sandbox")
            .arg(if request.binding.role == "discussion" {
                "read-only"
            } else {
                "workspace-write"
            })
            .arg("--cd")
            .arg(&request.binding.workspace);
        if request.binding.native_session_id.is_some() {
            command.arg("resume");
        }
        command.arg("--json").arg("--skip-git-repo-check");
        // Existing user authentication is retained. No danger/approval-bypass flags are introduced.
        command.args(&model_args);
        command.arg("--config").arg("web_search=\"live\"");
        if let Some(id) = &request.binding.native_session_id {
            command.arg("--").arg(id);
        }
        command
            .arg("-")
            .current_dir(&request.binding.workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if cancel.is_cancelled() {
            return Err(TurnError::new("CANCELLED", "调用已在启动前取消"));
        }
        let mut child = command.spawn().map_err(io_error)?;
        let _ = events.send(json!({"type":"execution.configured","configuration_source":"process_arguments","requested_model":request.binding.model,"requested_reasoning_effort":request.binding.reasoning_effort,"model_arguments":model_args,"provider_confirmed":false})).await;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| TurnError::new("PROTOCOL", "stdin unavailable"))?;
        let prompt = request.prompt.into_bytes();
        let input_writer = tokio::spawn(async move {
            stdin.write_all(&prompt).await?;
            stdin.shutdown().await
        });
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TurnError::new("PROTOCOL", "stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| TurnError::new("PROTOCOL", "stderr unavailable"))?;
        let stderr_reader = tokio::spawn(async move {
            let mut stderr = stderr;
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4096];
            while let Ok(size) = stderr.read(&mut buffer).await {
                if size == 0 {
                    break;
                }
                let retain = size.min((128 * 1024_usize).saturating_sub(bytes.len()));
                bytes.extend_from_slice(&buffer[..retain]);
            }
            redact(&String::from_utf8_lossy(&bytes))
        });
        let mut evidence = StreamEvidence::default();
        let reading = async {
            let mut reader = BufReader::new(stdout);
            let mut output = TurnOutput::default();
            let spool_directory = request
                .binding_path
                .parent()
                .unwrap_or(&request.binding.workspace);
            while let Some(raw) = read_executor_event(&mut reader, spool_directory).await? {
                if matches!(
                    raw["type"].as_str(),
                    Some(
                        "thread.started"
                            | "turn.started"
                            | "item.started"
                            | "item.updated"
                            | "item.completed"
                    )
                ) {
                    evidence.started = true;
                }
                if matches!(raw["type"].as_str(), Some("error" | "turn.failed")) {
                    let detail=bounded_diagnostic(&json!({"type":raw["type"],"code":raw["error"]["code"],"message":raw["error"].get("message").or_else(||raw.get("message"))}).to_string());
                    if raw["type"] == "turn.failed" {
                        evidence.failed = Some(detail.clone());
                    }
                    if evidence.diagnostics.len() < 8 {
                        evidence.diagnostics.push(detail);
                    }
                }
                if raw["type"] == "thread.started" {
                    let id = raw["thread_id"]
                        .as_str()
                        .ok_or_else(|| TurnError::new("PROTOCOL", "missing session identity"))?;
                    super::validate_session_id(id)
                        .map_err(|e| TurnError::new("PROTOCOL", e.to_string()))?;
                    if request
                        .binding
                        .native_session_id
                        .as_deref()
                        .is_some_and(|expected| expected != id)
                    {
                        return Err(TurnError::new(
                            "SESSION_BINDING_MISMATCH",
                            "恢复返回了其他会话",
                        ));
                    }
                    request.binding.native_session_id = Some(id.into());
                    output.native_session_id = Some(id.into());
                    save_binding(&request.binding_path, &request.binding).await?;
                }
                if raw["type"] == "turn.completed" {
                    evidence.completed = true;
                    output.input_tokens = raw["usage"]["input_tokens"].as_u64();
                    output.output_tokens = raw["usage"]["output_tokens"].as_u64();
                    output.cached_input_tokens = raw["usage"]["cached_input_tokens"].as_u64();
                }
                if raw["type"] == "item.completed" && raw["item"]["type"] == "agent_message" {
                    if let Some(text) = raw["item"]["text"].as_str() {
                        if output.text.len() + text.len() > 8 * 1024 * 1024 {
                            return Err(TurnError::new("RESOURCE", "公开回答超过 8 MiB 限制"));
                        }
                        if !output.text.is_empty() {
                            output.text.push_str("\n\n");
                        }
                        output.text.push_str(&redact(text));
                    }
                }
                if let Some(event) = public_event(&raw) {
                    let _ = events.send(event).await;
                }
            }
            Ok(output)
        };
        let result = tokio::select! {
            biased;
            () = cancel.cancelled() => Err(TurnError::new("CANCELLED", "调用已取消")),
            () = tokio::time::sleep(Duration::from_secs(request.timeout_seconds.max(1))) => Err(TurnError::new("DEADLINE_REACHED", "本次调用达到时限")),
            result = reading => result,
        };
        let mut natural_exit = result.is_ok();
        if result.is_err() {
            super::terminate_process_tree(&mut child).await;
        }
        let status =
            if let Ok(status) = tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
                status.map_err(io_error)?
            } else {
                natural_exit = false;
                super::terminate_process_tree(&mut child).await;
                child.wait().await.map_err(io_error)?
            };
        let _ = input_writer.await;
        let stderr = stderr_reader.await.unwrap_or_default();
        finalize_turn(
            result,
            status.success(),
            status.code(),
            natural_exit,
            &evidence,
            &stderr,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn web_search_duplicate_executor_id_preserves_lifecycle_id_without_claiming_completion() {
        let directory = tempfile::tempdir().unwrap();
        let raw = include_str!("../tests/fixtures/codex-web-search-duplicate-id.jsonl");
        let mut reader = BufReader::new(raw.as_bytes());
        let event = read_executor_event(&mut reader, directory.path())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event["item"]["id"], "item_5");
        assert_eq!(event["item"]["type"], "web_search");
        let public = public_event(&event).unwrap();
        assert_eq!(public["type"], "tool.started");
        assert_eq!(public["item_id"], "item_5");
        assert!(!public.to_string().contains("exec-47f28ff2"));
        assert_eq!(
            finalize_turn(
                Ok(TurnOutput::default()),
                true,
                Some(0),
                true,
                &StreamEvidence {
                    started: true,
                    ..StreamEvidence::default()
                },
                ""
            )
            .unwrap_err()
            .code,
            "DELIVERY_UNCERTAIN"
        );
    }

    #[tokio::test]
    async fn web_search_duplicate_id_spills_and_skips_private_payload() {
        let directory = tempfile::tempdir().unwrap();
        let raw = format!(
            "{{\"type\":\"item.completed\",\"item\":{{\"id\":\"item_5\",\"type\":\"web_search\",\"id\":\"exec-repro\",\"query\":\"{}\"}}}}\n",
            "private payload ".repeat(EVENT_MEMORY_LIMIT / 8)
        );
        let mut reader = BufReader::new(raw.as_bytes());
        let event = read_executor_event(&mut reader, directory.path())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(public_event(&event).unwrap()["item_id"], "item_5");
        assert_eq!(public_event(&event).unwrap()["type"], "tool.completed");
        assert!(event.to_string().len() < 300);
        assert!(!event.to_string().contains("private payload"));
        assert!(
            std::fs::read_dir(directory.path())
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[tokio::test]
    async fn web_search_compatibility_does_not_accept_other_duplicate_fields_or_malformed_json() {
        let directory = tempfile::tempdir().unwrap();
        for raw in [
            r#"{"type":"item.started","item":{"id":"item_5","type":"agent_message","id":"exec-repro"}}"#,
            r#"{"type":"item.started","item":{"id":"item_5","type":"web_search","id":"exec-repro","id":"exec-third"}}"#,
            r#"{"type":"item.started","item":{"id":"item_5","type":"web_search","type":"agent_message"}}"#,
            r#"{"type":"item.started","item":{"id":"item_5","type":"web_search","id":42}}"#,
            r#"{"type":"item.started","item":{"id":"item_5","type":"web_search","id":"exec-repro"}} garbage"#,
        ] {
            let mut reader = BufReader::new(raw.as_bytes());
            let error = read_executor_event(&mut reader, directory.path())
                .await
                .unwrap_err();
            assert_eq!(error.code, "PROTOCOL");
        }
    }
    #[tokio::test]
    async fn large_tool_event_spills_without_losing_following_proof_or_terminal_usage() {
        let directory = tempfile::tempdir().unwrap();
        let payload = "private tool output ".repeat(80_000);
        let tool = json!({"type":"item.completed","item":{"id":"tool1","type":"command_execution","status":"completed","exit_code":0,"stdout":payload,"aggregated_output":payload}});
        let stream = format!(
            "{tool}\n{}\n{}\n",
            json!({"type":"item.completed","item":{"id":"proof","type":"agent_message","text":"精确陈述：对于每个 n ≥ 1。"}}),
            json!({"type":"turn.completed","usage":{"input_tokens":120,"output_tokens":50}})
        );
        let mut reader = BufReader::with_capacity(1031, stream.as_bytes());
        let event = read_executor_event(&mut reader, directory.path())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event["item"]["exit_code"], 0);
        assert!(event.to_string().len() < 500);
        assert!(!event.to_string().contains("private tool output"));
        assert_eq!(public_event(&event).unwrap()["type"], "tool.completed");
        let proof = read_executor_event(&mut reader, directory.path())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(proof["item"]["text"], "精确陈述：对于每个 n ≥ 1。");
        let terminal = read_executor_event(&mut reader, directory.path())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(terminal["type"], "turn.completed");
        assert_eq!(terminal["usage"]["output_tokens"], 50);
        assert!(
            read_executor_event(&mut reader, directory.path())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn invalid_spilled_event_cleans_up_and_cannot_fake_delivery() {
        let directory = tempfile::tempdir().unwrap();
        let stream = format!(
            "{{\"type\":\"item.completed\",\"item\":{{\"stdout\":\"{}\"\n",
            "x".repeat(EVENT_MEMORY_LIMIT + 1)
        );
        let mut reader = BufReader::new(stream.as_bytes());
        let error = read_executor_event(&mut reader, directory.path())
            .await
            .unwrap_err();
        assert_eq!(error.code, "PROTOCOL");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        let final_error = finalize_turn(
            Err(error),
            false,
            Some(1),
            false,
            &StreamEvidence {
                started: true,
                ..StreamEvidence::default()
            },
            "",
        )
        .unwrap_err();
        assert_eq!(final_error.code, "DELIVERY_UNCERTAIN");
    }

    #[test]
    fn public_stream_drops_tool_payload_and_secret_lines() {
        let event = public_event(&json!({"type":"item.completed","item":{"type":"command_execution","command":"echo sk-secret","aggregated_output":"private","status":"completed","exit_code":0}})).unwrap();
        assert!(!event.to_string().contains("private"));
        assert!(!event.to_string().contains("sk-secret"));
        assert_eq!(
            redact("ok\nAuthorization: Bearer secret\nproof"),
            "ok\n[敏感字段已隐藏]\nproof"
        );
    }
    #[tokio::test]
    async fn persistent_binding_survives_backend_reconstruction_and_rejects_wrong_project() {
        let temp = tempfile::tempdir().unwrap();
        let mut request = TurnRequest {
            binding: SessionBinding {
                project_id: "p".into(),
                session_id: "s".into(),
                role: "main".into(),
                model: None,
                reasoning_effort: None,
                workspace: temp.path().into(),
                problem_version: 1,
                native_session_id: Some("12345678-1234-1234-1234-123456789abc".into()),
            },
            prompt: String::new(),
            timeout_seconds: 10,
            binding_path: temp.path().join("binding.json"),
            control_path: None,
        };
        save_binding(&request.binding_path, &request.binding)
            .await
            .unwrap();
        validate_binding(&request).await.unwrap();
        request.binding.reasoning_effort = Some("high".into());
        request.binding.model = Some("gpt-6-astra".into());
        validate_binding(&request).await.unwrap();
        request.binding.reasoning_effort = None;
        request.binding.project_id = "other".into();
        assert_eq!(
            validate_binding(&request).await.unwrap_err().code,
            "SESSION_BINDING_MISMATCH"
        );
    }
    #[test]
    fn process_model_overrides_are_explicit_and_old_bindings_remain_readable() {
        assert_eq!(
            execution_overrides(Some("gpt-6-astra"), Some("high")).unwrap(),
            vec![
                "--model",
                "gpt-6-astra",
                "--config",
                "model_reasoning_effort=\"high\""
            ]
        );
        assert!(execution_overrides(None, None).unwrap().is_empty());
        for effort in [
            "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
        ] {
            assert!(execution_overrides(Some("future-model"), Some(effort)).is_ok());
        }
        assert!(execution_overrides(Some("gpt-6-astra"), Some("high\";bad")).is_err());
        let old: SessionBinding = serde_json::from_value(json!({"project_id":"p","session_id":"s","role":"main","model":null,"workspace":"work","problem_version":1,"native_session_id":null})).unwrap();
        assert_eq!(old.reasoning_effort, None);
    }
    #[test]
    fn explicit_cli_start_failure_keeps_diagnostic_without_unknown_lock() {
        let error=finalize_turn(Ok(TurnOutput::default()),false,Some(2),true,&StreamEvidence::default(),"error: unexpected argument '--bad' found\nUsage: codex exec [OPTIONS]\nAuthorization: Bearer private").unwrap_err();
        assert_eq!(error.code, "EXECUTOR_START_FAILED");
        assert!(error.message.contains("unexpected argument"));
        assert!(!error.message.contains("private"));
    }
    #[test]
    fn explicit_turn_failure_is_terminal_even_without_completed_event() {
        let evidence = StreamEvidence {
            started: true,
            failed: Some("provider rejected request".into()),
            ..StreamEvidence::default()
        };
        let error = finalize_turn(
            Ok(TurnOutput::default()),
            false,
            Some(1),
            true,
            &evidence,
            "diagnostic fixture",
        )
        .unwrap_err();
        assert_eq!(error.code, "TURN_FAILED");
        assert!(error.message.contains("provider rejected request"));
    }
    #[test]
    fn absent_terminal_or_transport_error_remains_unknown_and_bounded() {
        let evidence = StreamEvidence {
            started: true,
            ..StreamEvidence::default()
        };
        let error = finalize_turn(
            Ok(TurnOutput::default()),
            false,
            Some(1),
            true,
            &evidence,
            &"transport disconnected ".repeat(2000),
        )
        .unwrap_err();
        assert_eq!(error.code, "DELIVERY_UNCERTAIN");
        assert!(error.message.contains("transport disconnected"));
        assert!(error.message.chars().count() < 9000);
        let no_stream = finalize_turn(
            Ok(TurnOutput::default()),
            false,
            Some(1),
            true,
            &StreamEvidence::default(),
            "network connection reset",
        )
        .unwrap_err();
        assert_eq!(no_stream.code, "DELIVERY_UNCERTAIN");
    }
}
