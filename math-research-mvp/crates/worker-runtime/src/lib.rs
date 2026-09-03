// Capability flags intentionally mirror the architecture's backend capability matrix,
// and the process supervisor keeps setup, cancellation, and output collection together.
#![allow(clippy::struct_excessive_bools, clippy::too_many_lines)]

use std::{
    collections::{HashMap, VecDeque},
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;
use ulid::Ulid;

mod verification;

pub use verification::{
    BackendError, BackendOutcome, BackendRequest, InteractiveProofBackend, InteractiveProofSession,
    LeanKernelBackend, LeanKernelConfig, MockInteractiveProofBackend, MockVerificationBackend,
    PantographBackend, PantographConfig, ProofGoal, ProofState, VerificationBackend,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BackendCapabilities {
    pub resumable_session: bool,
    pub safe_point_steering: bool,
    pub non_waking_injection: bool,
    pub graceful_cancel: bool,
    pub event_stream: bool,
    pub structured_output: bool,
    pub tool_permissions: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    pub project_id: String,
    pub role: String,
    pub model: Option<String>,
    pub working_directory: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHandle {
    pub handle_id: String,
    pub project_id: String,
    pub role: String,
    pub model: Option<String>,
    pub working_directory: PathBuf,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTaskKind {
    Planner,
    Worker,
    Verifier,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTask {
    pub kind: AgentTaskKind,
    pub prompt: String,
    pub output_schema: Value,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInput {
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunResult {
    pub structured_output: Value,
    pub session_id: Option<String>,
    pub raw_events: Vec<Value>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub stderr: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationCause {
    HumanStop,
    RouteStopped,
    ProjectStopped,
    Timeout,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteerReceipt {
    pub accepted: bool,
    pub effective_mode: String,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("agent process failed: {0}")]
    Process(String),
    #[error("agent output was not valid structured JSON: {0}")]
    InvalidOutput(String),
    #[error("agent run timed out after {0} seconds")]
    Timeout(u64),
    #[error("agent run was cancelled")]
    Cancelled,
    #[error("backend capability is unsupported: {0}")]
    Unsupported(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[async_trait]
pub trait AgentBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> BackendCapabilities;
    async fn create(&self, spec: AgentSpec) -> Result<AgentHandle, AgentError>;
    async fn run(
        &self,
        handle: &AgentHandle,
        task: AgentTask,
        cancellation: CancellationToken,
    ) -> Result<AgentRunResult, AgentError>;
    async fn continue_run(&self, handle: &AgentHandle, input: AgentInput)
    -> Result<(), AgentError>;
    async fn steer(
        &self,
        handle: &AgentHandle,
        input: AgentInput,
    ) -> Result<SteerReceipt, AgentError>;
    async fn cancel(
        &self,
        handle: &AgentHandle,
        cause: CancellationCause,
    ) -> Result<(), AgentError>;
    async fn wait_idle(&self, handle: &AgentHandle) -> Result<(), AgentError>;
}

#[derive(Debug, Clone)]
pub struct CodexCliConfig {
    pub command: PathBuf,
    pub default_model: Option<String>,
    pub sandbox: String,
    pub default_timeout: Duration,
}

impl Default for CodexCliConfig {
    fn default() -> Self {
        Self {
            command: PathBuf::from(if cfg!(windows) { "codex.cmd" } else { "codex" }),
            default_model: None,
            sandbox: "workspace-write".into(),
            default_timeout: Duration::from_secs(45 * 60),
        }
    }
}

#[derive(Clone)]
pub struct CodexCliBackend {
    config: CodexCliConfig,
    runs: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl std::fmt::Debug for CodexCliBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexCliBackend")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl CodexCliBackend {
    #[must_use]
    pub fn new(config: CodexCliConfig) -> Self {
        Self {
            config,
            runs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn execute(
        &self,
        handle: &AgentHandle,
        task: &AgentTask,
        cancellation: CancellationToken,
    ) -> Result<AgentRunResult, AgentError> {
        tokio::fs::create_dir_all(&handle.working_directory).await?;
        let nonce = Ulid::new();
        let schema_path = handle
            .working_directory
            .join(format!("output-schema-{nonce}.json"));
        let output_path = handle
            .working_directory
            .join(format!("last-message-{nonce}.json"));
        tokio::fs::write(
            &schema_path,
            serde_json::to_vec_pretty(&task.output_schema)?,
        )
        .await?;

        let mut command = codex_process_command(&self.config.command);
        command
            .arg("exec")
            .arg("--json")
            .arg("--skip-git-repo-check")
            .arg("--sandbox")
            .arg(&self.config.sandbox)
            .arg("--output-schema")
            .arg(&schema_path)
            .arg("--output-last-message")
            .arg(&output_path)
            .arg("--cd")
            .arg(&handle.working_directory);
        if let Some(config) = live_web_search_config(&handle.role) {
            command.arg("--config").arg(config);
        }
        if let Some(model) = handle.model.as_ref().or(self.config.default_model.as_ref()) {
            command.arg("--model").arg(model);
        }
        command
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let started_at = Utc::now();
        let mut child = command.spawn().map_err(|error| {
            AgentError::Process(format!(
                "failed to start {}: {error}",
                self.config.command.display()
            ))
        })?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| AgentError::Process("stdin pipe unavailable".into()))?;
        stdin.write_all(task.prompt.as_bytes()).await?;
        stdin.shutdown().await?;
        drop(stdin);
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentError::Process("stdout pipe unavailable".into()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| AgentError::Process("stderr pipe unavailable".into()))?;
        let stdout_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let internal_cancel = CancellationToken::new();
        self.runs
            .lock()
            .await
            .insert(handle.handle_id.clone(), internal_cancel.clone());
        let timeout_seconds = if task.timeout_seconds == 0 {
            self.config.default_timeout.as_secs()
        } else {
            task.timeout_seconds
        };
        let status_result = tokio::select! {
            status = child.wait() => status.map_err(AgentError::Io),
            () = cancellation.cancelled() => { let _ = child.kill().await; let _ = child.wait().await; Err(AgentError::Cancelled) },
            () = internal_cancel.cancelled() => { let _ = child.kill().await; let _ = child.wait().await; Err(AgentError::Cancelled) },
            () = tokio::time::sleep(Duration::from_secs(timeout_seconds)) => { let _ = child.kill().await; let _ = child.wait().await; Err(AgentError::Timeout(timeout_seconds)) },
        };
        self.runs.lock().await.remove(&handle.handle_id);
        let stdout_bytes = stdout_task
            .await
            .map_err(|error| AgentError::Process(error.to_string()))??;
        let stderr_bytes = stderr_task
            .await
            .map_err(|error| AgentError::Process(error.to_string()))??;
        let stderr_text = String::from_utf8_lossy(&stderr_bytes).into_owned();
        let stdout_text = String::from_utf8_lossy(&stdout_bytes).into_owned();
        let status = status_result?;
        let raw_events: Vec<Value> = stdout_text
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let session_id = raw_events.iter().find_map(find_session_id);
        let (input_tokens, output_tokens) = token_usage_from_events(&raw_events);
        if !status.success() {
            let stdout_tail: String = stdout_text
                .chars()
                .rev()
                .take(4_000)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            return Err(AgentError::Process(format!(
                "Codex exited with {status}; stdout tail: {stdout_tail}; stderr: {stderr_text}"
            )));
        }
        let output_bytes = tokio::fs::read(&output_path).await.map_err(|error| {
            AgentError::InvalidOutput(format!("missing {}: {error}", output_path.display()))
        })?;
        let structured_output = parse_structured_output(&output_bytes)?;
        let _ = tokio::fs::remove_file(&schema_path).await;
        let _ = tokio::fs::remove_file(&output_path).await;
        Ok(AgentRunResult {
            structured_output,
            session_id,
            raw_events,
            input_tokens,
            output_tokens,
            stderr: stderr_text,
            started_at,
            completed_at: Utc::now(),
        })
    }
}

#[async_trait]
impl AgentBackend for CodexCliBackend {
    fn name(&self) -> &'static str {
        "codex_cli"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            resumable_session: true,
            safe_point_steering: false,
            non_waking_injection: false,
            graceful_cancel: false,
            event_stream: true,
            structured_output: true,
            tool_permissions: true,
        }
    }

    async fn create(&self, spec: AgentSpec) -> Result<AgentHandle, AgentError> {
        tokio::fs::create_dir_all(&spec.working_directory).await?;
        Ok(AgentHandle {
            handle_id: format!("agent_{}", Ulid::new()),
            project_id: spec.project_id,
            role: spec.role,
            model: spec.model,
            working_directory: spec.working_directory,
            session_id: None,
        })
    }

    async fn run(
        &self,
        handle: &AgentHandle,
        task: AgentTask,
        cancellation: CancellationToken,
    ) -> Result<AgentRunResult, AgentError> {
        self.execute(handle, &task, cancellation).await
    }

    async fn continue_run(
        &self,
        handle: &AgentHandle,
        input: AgentInput,
    ) -> Result<(), AgentError> {
        let session_id = handle.session_id.as_deref().ok_or(AgentError::Unsupported(
            "continue without a recorded session",
        ))?;
        let mut command = codex_process_command(&self.config.command);
        command
            .arg("exec")
            .arg("resume")
            .arg("--json")
            .arg("--skip-git-repo-check");
        if let Some(model) = handle.model.as_ref().or(self.config.default_model.as_ref()) {
            command.arg("--model").arg(model);
        }
        let output = command
            .arg(session_id)
            .arg(input.prompt)
            .current_dir(&handle.working_directory)
            .output()
            .await?;
        if !output.status.success() {
            return Err(AgentError::Process(format!(
                "Codex resume exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    async fn steer(
        &self,
        _handle: &AgentHandle,
        _input: AgentInput,
    ) -> Result<SteerReceipt, AgentError> {
        Err(AgentError::Unsupported("safe-point steering"))
    }

    async fn cancel(
        &self,
        handle: &AgentHandle,
        _cause: CancellationCause,
    ) -> Result<(), AgentError> {
        if let Some(token) = self.runs.lock().await.get(&handle.handle_id) {
            token.cancel();
        }
        Ok(())
    }

    async fn wait_idle(&self, handle: &AgentHandle) -> Result<(), AgentError> {
        while self.runs.lock().await.contains_key(&handle.handle_id) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(())
    }
}

fn codex_process_command(configured: &Path) -> Command {
    let (program, prefix_args) = codex_invocation(configured);
    let mut command = Command::new(program);
    command.args(prefix_args);
    command
}

fn live_web_search_config(role: &str) -> Option<&'static str> {
    (role == "literature_researcher").then_some(r#"web_search="live""#)
}

fn codex_invocation(configured: &Path) -> (PathBuf, Vec<OsString>) {
    #[cfg(windows)]
    {
        let is_batch = configured.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        });
        if is_batch {
            let wrapper = resolve_command_path(configured).unwrap_or_else(|| configured.into());
            let wrapper_parent = wrapper.parent().map(Path::to_path_buf);
            let mut entrypoints = wrapper_parent
                .iter()
                .map(|parent| {
                    parent
                        .join("node_modules")
                        .join("@openai")
                        .join("codex")
                        .join("bin")
                        .join("codex.js")
                })
                .collect::<Vec<_>>();
            if let Some(app_data) = std::env::var_os("APPDATA") {
                entrypoints.push(
                    PathBuf::from(app_data)
                        .join("npm")
                        .join("node_modules")
                        .join("@openai")
                        .join("codex")
                        .join("bin")
                        .join("codex.js"),
                );
            }
            if let Some(entrypoint) = entrypoints.into_iter().find(|path| path.is_file()) {
                let local_node = wrapper_parent
                    .as_deref()
                    .map(|parent| parent.join("node.exe"))
                    .filter(|path| path.is_file());
                let node = local_node.unwrap_or_else(|| PathBuf::from("node.exe"));
                return (node, vec![entrypoint.into_os_string()]);
            }
        }
    }
    (configured.into(), Vec::new())
}

#[cfg(windows)]
fn resolve_command_path(configured: &Path) -> Option<PathBuf> {
    if configured.is_absolute()
        || configured
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        return configured.is_file().then(|| configured.into());
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|directory| directory.join(configured))
            .find(|candidate| candidate.is_file())
    })
}

fn find_session_id(value: &Value) -> Option<String> {
    for key in ["thread_id", "session_id", "conversation_id"] {
        if let Some(id) = value.get(key).and_then(Value::as_str) {
            return Some(id.to_owned());
        }
    }
    value.as_object()?.values().find_map(find_session_id)
}

fn token_usage_from_events(events: &[Value]) -> (i64, i64) {
    events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("turn.completed"))
        .filter_map(|event| event.get("usage"))
        .fold((0, 0), |(input, output), usage| {
            (
                input
                    + usage
                        .get("input_tokens")
                        .and_then(Value::as_i64)
                        .unwrap_or_default()
                        .max(0),
                output
                    + usage
                        .get("output_tokens")
                        .and_then(Value::as_i64)
                        .unwrap_or_default()
                        .max(0),
            )
        })
}

fn parse_structured_output(bytes: &[u8]) -> Result<Value, AgentError> {
    let text = String::from_utf8_lossy(bytes);
    if let Ok(value) = serde_json::from_str(&text) {
        return Ok(value);
    }
    let trimmed = text
        .trim()
        .strip_prefix("```json")
        .or_else(|| text.trim().strip_prefix("```"))
        .and_then(|value| value.strip_suffix("```"))
        .map(str::trim);
    trimmed
        .and_then(|value| serde_json::from_str(value).ok())
        .ok_or_else(|| AgentError::InvalidOutput(text.chars().take(500).collect()))
}

#[derive(Debug, Clone, Default)]
pub struct MockBackend {
    responses: Arc<Mutex<VecDeque<Value>>>,
}

impl MockBackend {
    #[must_use]
    pub fn from_responses(responses: impl IntoIterator<Item = Value>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().collect())),
        }
    }

    pub async fn push(&self, response: Value) {
        self.responses.lock().await.push_back(response);
    }
}

#[async_trait]
impl AgentBackend for MockBackend {
    fn name(&self) -> &'static str {
        "mock"
    }
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            resumable_session: false,
            safe_point_steering: false,
            non_waking_injection: false,
            graceful_cancel: true,
            event_stream: false,
            structured_output: true,
            tool_permissions: false,
        }
    }
    async fn create(&self, spec: AgentSpec) -> Result<AgentHandle, AgentError> {
        Ok(AgentHandle {
            handle_id: format!("mock_{}", Ulid::new()),
            project_id: spec.project_id,
            role: spec.role,
            model: spec.model,
            working_directory: spec.working_directory,
            session_id: None,
        })
    }
    async fn run(
        &self,
        _handle: &AgentHandle,
        _task: AgentTask,
        cancellation: CancellationToken,
    ) -> Result<AgentRunResult, AgentError> {
        if cancellation.is_cancelled() {
            return Err(AgentError::Cancelled);
        }
        let output = self
            .responses
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| AgentError::Process("mock response queue exhausted".into()))?;
        let now = Utc::now();
        Ok(AgentRunResult {
            structured_output: output,
            session_id: None,
            raw_events: vec![],
            input_tokens: 0,
            output_tokens: 0,
            stderr: String::new(),
            started_at: now,
            completed_at: now,
        })
    }
    async fn continue_run(
        &self,
        _handle: &AgentHandle,
        _input: AgentInput,
    ) -> Result<(), AgentError> {
        Ok(())
    }
    async fn steer(
        &self,
        _handle: &AgentHandle,
        _input: AgentInput,
    ) -> Result<SteerReceipt, AgentError> {
        Err(AgentError::Unsupported("steer"))
    }
    async fn cancel(
        &self,
        _handle: &AgentHandle,
        _cause: CancellationCause,
    ) -> Result<(), AgentError> {
        Ok(())
    }
    async fn wait_idle(&self, _handle: &AgentHandle) -> Result<(), AgentError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{live_web_search_config, token_usage_from_events};

    #[test]
    fn extracts_only_completed_turn_usage() {
        let events = vec![
            json!({"type":"item.completed","usage":{"input_tokens":999,"output_tokens":999}}),
            json!({"type":"turn.completed","usage":{"input_tokens":120,"cached_input_tokens":40,"output_tokens":30}}),
            json!({"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":2}}),
        ];
        assert_eq!(token_usage_from_events(&events), (125, 32));
    }

    #[test]
    fn only_literature_workers_receive_supported_live_web_search_config() {
        assert_eq!(
            live_web_search_config("literature_researcher"),
            Some(r#"web_search="live""#)
        );
        assert_eq!(live_web_search_config("prover"), None);
        assert_eq!(live_web_search_config("paper_writer"), None);
        assert_eq!(live_web_search_config("math_review_1"), None);
    }

    #[cfg(windows)]
    #[test]
    fn batch_codex_wrapper_uses_node_entrypoint_for_multiline_safe_arguments() {
        let temp = tempfile::tempdir().expect("tempdir");
        let wrapper = temp.path().join("codex.cmd");
        let entrypoint = temp
            .path()
            .join("node_modules")
            .join("@openai")
            .join("codex")
            .join("bin")
            .join("codex.js");
        std::fs::create_dir_all(entrypoint.parent().expect("entrypoint parent"))
            .expect("entrypoint directory");
        std::fs::write(&wrapper, "@node codex.js %*").expect("wrapper");
        std::fs::write(&entrypoint, "// fixture").expect("entrypoint");
        let (program, prefix) = super::codex_invocation(&wrapper);
        assert_eq!(program, std::path::Path::new("node.exe"));
        assert_eq!(prefix, vec![entrypoint.into_os_string()]);
    }
}
