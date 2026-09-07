// Capability flags intentionally mirror the architecture's backend capability matrix,
// and the process supervisor keeps setup, cancellation, and output collection together.
#![allow(clippy::struct_excessive_bools, clippy::too_many_lines)]

use std::{
    collections::{HashMap, VecDeque},
    ffi::OsString,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
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
    process::{Child, Command},
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;
use ulid::Ulid;

pub mod research_v2;
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
    ProblemGenerator,
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

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("agent process failed: {0}")]
    Process(String),
    #[error("agent session is unavailable: {0}")]
    SessionUnavailable(String),
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
    #[error("{error}")]
    WithUsage {
        error: Box<AgentError>,
        input_tokens: i64,
        output_tokens: i64,
    },
}

impl AgentError {
    #[must_use]
    pub const fn token_usage(&self) -> (i64, i64) {
        match self {
            Self::WithUsage {
                input_tokens,
                output_tokens,
                ..
            } => (*input_tokens, *output_tokens),
            _ => (0, 0),
        }
    }

    fn with_usage(self, input_tokens: i64, output_tokens: i64) -> Self {
        Self::WithUsage {
            error: Box::new(self),
            input_tokens: input_tokens.max(0),
            output_tokens: output_tokens.max(0),
        }
    }
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
    async fn resume(
        &self,
        handle: &AgentHandle,
        task: AgentTask,
        cancellation: CancellationToken,
    ) -> Result<AgentRunResult, AgentError>;
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
    sessions: Arc<Mutex<HashMap<String, SessionBinding>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionBinding {
    handle_id: String,
    project_id: String,
    role: String,
    model: Option<String>,
    working_directory: PathBuf,
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
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn execute(
        &self,
        handle: &AgentHandle,
        task: &AgentTask,
        cancellation: CancellationToken,
        resume_session_id: Option<&str>,
    ) -> Result<AgentRunResult, AgentError> {
        tokio::fs::create_dir_all(&handle.working_directory).await?;
        // Runtime roots may be configured as relative paths.  Once the child process changes
        // into that directory, forwarding the same relative path to Codex (`--cd`) and to the
        // output file flags would resolve it a second time (for example
        // `runtime/.../planner/runtime/.../planner`) and Codex fails with OS error 3 on Windows.
        // Resolve it once before building every child-process path.
        let working_directory = tokio::fs::canonicalize(&handle.working_directory).await?;
        let binding = SessionBinding {
            handle_id: handle.handle_id.clone(),
            project_id: handle.project_id.clone(),
            role: handle.role.clone(),
            model: handle
                .model
                .clone()
                .or_else(|| self.config.default_model.clone()),
            working_directory: working_directory.clone(),
        };
        if let Some(session_id) = resume_session_id {
            validate_session_id(session_id)?;
            let recorded = self.sessions.lock().await.get(session_id).cloned();
            match recorded {
                None => {
                    return Err(AgentError::SessionUnavailable(format!(
                        "Codex session {session_id} is not registered in this backend process"
                    )));
                }
                Some(recorded) if recorded != binding => {
                    return Err(AgentError::InvalidOutput(format!(
                        "Codex session {session_id} is bound to a different handle, project, role, model, or workspace"
                    )));
                }
                Some(_) => {}
            }
        }
        let nonce = Ulid::new();
        let schema_path = working_directory.join(format!("output-schema-{nonce}.json"));
        let output_path = working_directory.join(format!("last-message-{nonce}.json"));
        tokio::fs::write(
            &schema_path,
            serde_json::to_vec_pretty(&task.output_schema)?,
        )
        .await?;

        let isolated_problem_generation = task.kind == AgentTaskKind::ProblemGenerator;
        let sandbox = if isolated_problem_generation {
            "read-only"
        } else {
            &self.config.sandbox
        };
        let mut command = codex_process_command(&self.config.command);
        command
            .arg("exec")
            .arg("--sandbox")
            .arg(sandbox)
            .arg("--cd")
            .arg(&working_directory);
        if isolated_problem_generation {
            // Problem-definition material is explicitly untrusted.  Keep this one-shot intake
            // call isolated from repository instructions and user configuration, and never
            // persist a resumable Codex session for it.
            command
                .arg("--ephemeral")
                .arg("--ignore-rules")
                .arg("--ignore-user-config");
        }
        if resume_session_id.is_some() {
            command.arg("resume");
        }
        command
            .arg("--json")
            .arg("--skip-git-repo-check")
            .arg("--output-schema")
            .arg(&schema_path)
            .arg("--output-last-message")
            .arg(&output_path);
        if let Some(config) = live_web_search_config(&handle.role) {
            command.arg("--config").arg(config);
        }
        if let Some(model) = handle.model.as_ref().or(self.config.default_model.as_ref()) {
            command.arg("--model").arg(model);
        }
        if let Some(session_id) = resume_session_id {
            command.arg("--").arg(session_id);
        }
        command
            .arg("-")
            .current_dir(&working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let started_at = Utc::now();
        let timeout_seconds = if task.timeout_seconds == 0 {
            self.config.default_timeout.as_secs()
        } else {
            task.timeout_seconds
        };
        let capture = match run_codex_process(
            command,
            task.prompt.as_bytes(),
            timeout_seconds,
            cancellation,
            &self.config.command,
        )
        .await
        {
            Ok(capture) => capture,
            Err(error) => {
                cleanup_codex_temp_files(&schema_path, &output_path).await;
                return Err(error);
            }
        };
        let stdout_bytes = capture.stdout;
        let stderr_bytes = capture.stderr;
        let status_result = capture.status;
        let mut stderr_text = String::from_utf8_lossy(&stderr_bytes).into_owned();
        let stdout_text = String::from_utf8_lossy(&stdout_bytes).into_owned();
        let (raw_events, invalid_jsonl_lines) = parse_jsonl_events(&stdout_text);
        if !invalid_jsonl_lines.is_empty() {
            use std::fmt::Write as _;
            let _ = write!(
                stderr_text,
                "\nCodex JSONL protocol warning: {} non-empty stdout line(s) were not JSON (line numbers: {:?})",
                invalid_jsonl_lines.len(),
                invalid_jsonl_lines
            );
        }
        let (input_tokens, output_tokens) = token_usage_from_events(&raw_events);
        let status = match status_result {
            Ok(status) => status,
            Err(error) => {
                cleanup_codex_temp_files(&schema_path, &output_path).await;
                return Err(error.with_usage(input_tokens, output_tokens));
            }
        };
        if !status.success() {
            let stdout_tail: String = stdout_text
                .chars()
                .rev()
                .take(4_000)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            cleanup_codex_temp_files(&schema_path, &output_path).await;
            let message = format!(
                "Codex exited with {status}; stdout tail: {stdout_tail}; stderr: {stderr_text}"
            );
            let error = if resume_session_id.is_some() && reports_unavailable_session(&stderr_text)
            {
                AgentError::SessionUnavailable(message)
            } else {
                AgentError::Process(message)
            };
            return Err(error.with_usage(input_tokens, output_tokens));
        }
        let session_id = match resolve_session_id(&raw_events, resume_session_id) {
            Ok(session_id) => session_id,
            Err(error) => {
                cleanup_codex_temp_files(&schema_path, &output_path).await;
                return Err(error.with_usage(input_tokens, output_tokens));
            }
        };
        let output_bytes = match tokio::fs::read(&output_path).await {
            Ok(bytes) => bytes,
            Err(error) => {
                cleanup_codex_temp_files(&schema_path, &output_path).await;
                return Err(AgentError::InvalidOutput(format!(
                    "missing {}: {error}",
                    output_path.display()
                ))
                .with_usage(input_tokens, output_tokens));
            }
        };
        let structured_output = match parse_structured_output(&output_bytes) {
            Ok(output) => output,
            Err(error) => {
                cleanup_codex_temp_files(&schema_path, &output_path).await;
                return Err(error.with_usage(input_tokens, output_tokens));
            }
        };
        cleanup_codex_temp_files(&schema_path, &output_path).await;
        if let Some(session_id) = &session_id
            && !isolated_problem_generation
        {
            self.sessions
                .lock()
                .await
                .insert(session_id.clone(), binding);
        }
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

#[derive(Debug)]
struct CodexProcessCapture {
    status: Result<ExitStatus, AgentError>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

enum PromptInputOutcome {
    Finished(std::io::Result<()>),
    Cancelled,
    TimedOut,
}

async fn run_codex_process(
    mut command: Command,
    prompt: &[u8],
    timeout_seconds: u64,
    cancellation: CancellationToken,
    configured_command: &Path,
) -> Result<CodexProcessCapture, AgentError> {
    if cancellation.is_cancelled() {
        return Err(AgentError::Cancelled);
    }
    let mut child = command.spawn().map_err(|error| {
        AgentError::Process(format!(
            "failed to start {}: {error}",
            configured_command.display()
        ))
    })?;
    let Some(mut stdin) = child.stdin.take() else {
        terminate_process_tree(&mut child).await;
        return Err(AgentError::Process("stdin pipe unavailable".into()));
    };
    let Some(mut stdout) = child.stdout.take() else {
        terminate_process_tree(&mut child).await;
        return Err(AgentError::Process("stdout pipe unavailable".into()));
    };
    let Some(mut stderr) = child.stderr.take() else {
        terminate_process_tree(&mut child).await;
        return Err(AgentError::Process("stderr pipe unavailable".into()));
    };
    let mut stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        let failure = stdout
            .read_to_end(&mut bytes)
            .await
            .err()
            .map(|error| format!("Codex stdout reader failed: {error}"));
        (bytes, failure)
    });
    let mut stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        let failure = stderr
            .read_to_end(&mut bytes)
            .await
            .err()
            .map(|error| format!("Codex stderr reader failed: {error}"));
        (bytes, failure)
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_seconds);
    let input_outcome = tokio::select! {
        biased;
        () = cancellation.cancelled() => PromptInputOutcome::Cancelled,
        () = tokio::time::sleep_until(deadline) => PromptInputOutcome::TimedOut,
        result = async {
            stdin.write_all(prompt).await?;
            stdin.shutdown().await
        } => PromptInputOutcome::Finished(result),
    };
    let (mut status, stdin_failure) = match input_outcome {
        PromptInputOutcome::Cancelled => {
            drop(stdin);
            terminate_process_tree(&mut child).await;
            (Err(AgentError::Cancelled), None)
        }
        PromptInputOutcome::TimedOut => {
            drop(stdin);
            terminate_process_tree(&mut child).await;
            (Err(AgentError::Timeout(timeout_seconds)), None)
        }
        PromptInputOutcome::Finished(write_result) => {
            drop(stdin);
            let stdin_failure = write_result
                .err()
                .map(|error| format!("failed to deliver the Codex prompt: {error}"));
            let status = tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    terminate_process_tree(&mut child).await;
                    Err(AgentError::Cancelled)
                },
                () = tokio::time::sleep_until(deadline) => {
                    terminate_process_tree(&mut child).await;
                    Err(AgentError::Timeout(timeout_seconds))
                },
                status = child.wait() => status.map_err(AgentError::Io),
            };
            (status, stdin_failure)
        }
    };
    let drained = tokio::time::timeout(Duration::from_secs(5), async {
        let stdout = match (&mut stdout_task).await {
            Ok(capture) => capture,
            Err(error) => (
                Vec::new(),
                Some(format!("Codex stdout reader task failed: {error}")),
            ),
        };
        let stderr = match (&mut stderr_task).await {
            Ok(capture) => capture,
            Err(error) => (
                Vec::new(),
                Some(format!("Codex stderr reader task failed: {error}")),
            ),
        };
        (stdout, stderr)
    })
    .await;
    let (stdout, mut stderr, drain_failures) = if let Ok((stdout, stderr)) = drained {
        let mut failures = Vec::new();
        if let Some(error) = stdout.1 {
            failures.push(error);
        }
        if let Some(error) = stderr.1 {
            failures.push(error);
        }
        (stdout.0, stderr.0, failures)
    } else {
        stdout_task.abort();
        stderr_task.abort();
        (
            Vec::new(),
            Vec::new(),
            vec!["Codex pipe drain timed out after process termination".into()],
        )
    };
    if let Some(stdin_failure) = stdin_failure {
        record_supervisor_failure(&mut status, &mut stderr, stdin_failure);
    }
    if !drain_failures.is_empty() {
        let failure = drain_failures.join("; ");
        record_supervisor_failure(&mut status, &mut stderr, failure);
    }
    Ok(CodexProcessCapture {
        status,
        stdout,
        stderr,
    })
}

fn record_supervisor_failure(
    status: &mut Result<ExitStatus, AgentError>,
    stderr: &mut Vec<u8>,
    failure: String,
) {
    append_process_diagnostic(stderr, &failure);
    // A known cancellation, timeout, wait error, or non-zero exit remains the
    // primary outcome. Supervisor I/O only becomes primary after an otherwise
    // successful exit.
    if matches!(status, Ok(exit_status) if exit_status.success()) {
        *status = Err(AgentError::Process(failure));
    }
}

fn append_process_diagnostic(stderr: &mut Vec<u8>, diagnostic: &str) {
    if !stderr.is_empty() && !stderr.ends_with(b"\n") {
        stderr.push(b'\n');
    }
    stderr.extend_from_slice(diagnostic.as_bytes());
    if !stderr.ends_with(b"\n") {
        stderr.push(b'\n');
    }
}

async fn terminate_process_tree(child: &mut Child) {
    #[cfg(windows)]
    if let Some(process_id) = child.id() {
        let mut taskkill = Command::new("taskkill.exe");
        taskkill
            .arg("/PID")
            .arg(process_id.to_string())
            .arg("/T")
            .arg("/F")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000);
        let _ = tokio::time::timeout(Duration::from_secs(10), taskkill.status()).await;
    }
    let _ = child.kill().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
}

async fn cleanup_codex_temp_files(schema_path: &Path, output_path: &Path) {
    let _ = tokio::fs::remove_file(schema_path).await;
    let _ = tokio::fs::remove_file(output_path).await;
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
        self.execute(handle, &task, cancellation, None).await
    }

    async fn resume(
        &self,
        handle: &AgentHandle,
        task: AgentTask,
        cancellation: CancellationToken,
    ) -> Result<AgentRunResult, AgentError> {
        let session_id = handle.session_id.as_deref().ok_or_else(|| {
            AgentError::SessionUnavailable("resume without a recorded session".into())
        })?;
        self.execute(handle, &task, cancellation, Some(session_id))
            .await
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
        // Only the built-in bare `codex.cmd` may be replaced by the native npm
        // binary/entrypoint. An explicit wrapper path is part of the caller's
        // security and configuration boundary and must execute verbatim.
        if is_default_codex_wrapper(configured) {
            let wrapper = resolve_command_path(configured).unwrap_or_else(|| configured.into());
            if let Some(invocation) = default_codex_batch_invocation(&wrapper) {
                return invocation;
            }
        }
    }
    (configured.into(), Vec::new())
}

#[cfg(windows)]
fn is_default_codex_wrapper(configured: &Path) -> bool {
    configured
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("codex.cmd"))
        && configured
            .parent()
            .is_none_or(|parent| parent.as_os_str().is_empty())
}

#[cfg(windows)]
fn default_codex_batch_invocation(wrapper: &Path) -> Option<(PathBuf, Vec<OsString>)> {
    if let Some(native) = windows_native_codex(wrapper) {
        return Some((native, Vec::new()));
    }
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
    let entrypoint = entrypoints.into_iter().find(|path| path.is_file())?;
    let local_node = wrapper_parent
        .as_deref()
        .map(|parent| parent.join("node.exe"))
        .filter(|path| path.is_file());
    let node = local_node.unwrap_or_else(|| PathBuf::from("node.exe"));
    Some((node, vec![entrypoint.into_os_string()]))
}

#[cfg(windows)]
fn windows_native_codex(wrapper: &Path) -> Option<PathBuf> {
    let wrapper_parent = wrapper.parent()?;
    let (platform_package, target_triple) = if cfg!(target_arch = "aarch64") {
        ("codex-win32-arm64", "aarch64-pc-windows-msvc")
    } else {
        ("codex-win32-x64", "x86_64-pc-windows-msvc")
    };
    let package_root = wrapper_parent
        .join("node_modules")
        .join("@openai")
        .join("codex");
    [
        package_root
            .join("node_modules")
            .join("@openai")
            .join(platform_package)
            .join("vendor")
            .join(target_triple)
            .join("bin")
            .join("codex.exe"),
        package_root
            .join("vendor")
            .join(target_triple)
            .join("bin")
            .join("codex.exe"),
    ]
    .into_iter()
    .find(|path| path.is_file())
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

fn resolve_session_id(
    events: &[Value],
    resumed_session_id: Option<&str>,
) -> Result<Option<String>, AgentError> {
    if let Some(expected) = resumed_session_id {
        validate_session_id(expected)?;
    }
    let mut observed = None;
    for event in events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("thread.started"))
    {
        let session_id = event
            .get("thread_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AgentError::InvalidOutput(
                    "Codex thread.started event omitted a string thread_id".into(),
                )
            })?;
        validate_session_id(session_id)?;
        if let Some(first) = observed
            && first != session_id
        {
            return Err(AgentError::InvalidOutput(format!(
                "Codex emitted conflicting session identities {first} and {session_id}"
            )));
        }
        observed = Some(session_id);
    }
    let observed = observed.ok_or_else(|| {
        AgentError::InvalidOutput(if resumed_session_id.is_some() {
            "resumed Codex run emitted no thread.started identity".into()
        } else {
            "Codex run emitted no thread.started identity".into()
        })
    })?;
    if let Some(expected) = resumed_session_id
        && observed != expected
    {
        return Err(AgentError::InvalidOutput(format!(
            "resumed Codex session changed identity from {expected} to {observed}"
        )));
    }
    Ok(Some(observed.to_owned()))
}

fn validate_session_id(session_id: &str) -> Result<(), AgentError> {
    let bytes = session_id.as_bytes();
    let canonical_uuid = bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        });
    if canonical_uuid {
        return Ok(());
    }
    Err(AgentError::InvalidOutput(format!(
        "Codex session identity is not a canonical UUID: {session_id:?}"
    )))
}

fn reports_unavailable_session(stderr: &str) -> bool {
    stderr.lines().any(|line| {
        let line = line.to_ascii_lowercase();
        let names_session = line.contains("session") || line.contains("thread");
        let reports_missing = line.contains("not found")
            || line.contains("does not exist")
            || line.contains("no saved session")
            || line.contains("no saved thread")
            || line.contains("unknown session")
            || line.contains("unknown thread");
        names_session && reports_missing
    })
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

fn parse_jsonl_events(text: &str) -> (Vec<Value>, Vec<usize>) {
    let mut events = Vec::new();
    let mut invalid_lines = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(line) {
            Ok(event) => events.push(event),
            Err(_) => invalid_lines.push(index + 1),
        }
    }
    (events, invalid_lines)
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
    async fn resume(
        &self,
        _handle: &AgentHandle,
        _task: AgentTask,
        _cancellation: CancellationToken,
    ) -> Result<AgentRunResult, AgentError> {
        Err(AgentError::Unsupported("resumable session"))
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use std::path::Path;
    use std::{path::PathBuf, process::Stdio, time::Duration};

    use serde_json::json;
    use tokio::process::Command;
    use tokio_util::sync::CancellationToken;

    use super::{
        AgentBackend, AgentError, AgentHandle, AgentTask, AgentTaskKind, CodexCliBackend,
        CodexCliConfig, SessionBinding, live_web_search_config, parse_jsonl_events,
        record_supervisor_failure, reports_unavailable_session, resolve_session_id,
        run_codex_process, token_usage_from_events,
    };

    const SESSION_A: &str = "0198b86e-65ad-7c31-ae24-d3f6bb01b912";
    const SESSION_B: &str = "0198b86e-65ad-7c31-ae24-d3f6bb01b913";

    #[cfg(windows)]
    fn native_codex_fixture_path(root: &Path) -> PathBuf {
        let (platform_package, target_triple) = if cfg!(target_arch = "aarch64") {
            ("codex-win32-arm64", "aarch64-pc-windows-msvc")
        } else {
            ("codex-win32-x64", "x86_64-pc-windows-msvc")
        };
        root.join("node_modules")
            .join("@openai")
            .join("codex")
            .join("node_modules")
            .join("@openai")
            .join(platform_package)
            .join("vendor")
            .join(target_triple)
            .join("bin")
            .join("codex.exe")
    }

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
    fn jsonl_protocol_errors_are_reported_instead_of_silently_dropped() {
        let (events, invalid_lines) = parse_jsonl_events(
            "{\"type\":\"thread.started\"}\nnot-json\n\n{\"type\":\"turn.completed\"}\n",
        );
        assert_eq!(events.len(), 2);
        assert_eq!(invalid_lines, vec![2]);
    }

    #[test]
    fn session_identity_comes_only_from_the_thread_started_protocol_event() {
        let events = vec![
            json!({"type":"item.completed","payload":{"session_id":"injected"}}),
            json!({"type":"thread.started","thread_id":SESSION_A}),
        ];
        assert_eq!(
            resolve_session_id(&events, None).expect("session id"),
            Some(SESSION_A.into())
        );
        assert!(matches!(
            resolve_session_id(&events, Some(SESSION_B)),
            Err(AgentError::InvalidOutput(_))
        ));
        assert!(matches!(
            resolve_session_id(
                &[json!({"type":"turn.completed","thread_id":"noise"})],
                None
            ),
            Err(AgentError::InvalidOutput(_))
        ));
    }

    #[test]
    fn resumed_session_requires_an_explicit_consistent_protocol_identity() {
        assert!(matches!(
            resolve_session_id(&[], Some(SESSION_A)),
            Err(AgentError::InvalidOutput(_))
        ));
        assert!(matches!(
            resolve_session_id(
                &[
                    json!({"type":"thread.started"}),
                    json!({"type":"thread.started","thread_id":SESSION_A}),
                ],
                Some(SESSION_A),
            ),
            Err(AgentError::InvalidOutput(_))
        ));
        assert!(matches!(
            resolve_session_id(
                &[
                    json!({"type":"thread.started","thread_id":SESSION_A}),
                    json!({"type":"thread.started","thread_id":SESSION_B}),
                ],
                Some(SESSION_A),
            ),
            Err(AgentError::InvalidOutput(_))
        ));
        for invalid in ["", "--last", "trusted-session", "00000000-0000-0000-0000"] {
            assert!(matches!(
                resolve_session_id(
                    &[json!({"type":"thread.started","thread_id":invalid})],
                    None,
                ),
                Err(AgentError::InvalidOutput(_))
            ));
        }
    }

    #[test]
    fn only_explicit_missing_session_diagnostics_are_retryable() {
        assert!(reports_unavailable_session(
            "Error: No saved session found with ID 0198..."
        ));
        assert!(reports_unavailable_session(
            "requested thread does not exist"
        ));
        assert!(!reports_unavailable_session(
            "HTTP 404: requested model route not found"
        ));
        assert!(!reports_unavailable_session(
            "session transport failed while contacting the server"
        ));
    }

    #[tokio::test]
    async fn resume_requires_a_matching_in_process_session_binding() {
        let temp = tempfile::tempdir().expect("tempdir");
        let backend = CodexCliBackend::new(CodexCliConfig {
            command: temp.path().join("must-not-run"),
            ..CodexCliConfig::default()
        });
        let handle = AgentHandle {
            handle_id: "agent-a".into(),
            project_id: "project-a".into(),
            role: "prover".into(),
            model: Some("model-a".into()),
            working_directory: temp.path().into(),
            session_id: Some(SESSION_A.into()),
        };
        let task = AgentTask {
            kind: AgentTaskKind::Worker,
            prompt: "continue".into(),
            output_schema: json!({"type":"object"}),
            timeout_seconds: 1,
        };
        let missing = backend
            .resume(&handle, task.clone(), CancellationToken::new())
            .await
            .expect_err("unregistered session must fail before spawn");
        assert!(matches!(missing, AgentError::SessionUnavailable(_)));

        let canonical = tokio::fs::canonicalize(temp.path())
            .await
            .expect("canonical tempdir");
        backend.sessions.lock().await.insert(
            SESSION_A.into(),
            SessionBinding {
                handle_id: "different-handle".into(),
                project_id: handle.project_id.clone(),
                role: handle.role.clone(),
                model: handle.model.clone(),
                working_directory: canonical,
            },
        );
        let mismatch = backend
            .resume(&handle, task, CancellationToken::new())
            .await
            .expect_err("a different handle must not reuse the session");
        assert!(matches!(mismatch, AgentError::InvalidOutput(_)));
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
        assert_eq!(live_web_search_config("problem_generator"), None);
    }

    #[cfg(windows)]
    #[test]
    fn batch_codex_wrapper_resolves_the_native_binary_for_tree_safe_termination() {
        let temp = tempfile::tempdir().expect("tempdir");
        let wrapper = temp.path().join("codex.cmd");
        let native = native_codex_fixture_path(temp.path());
        std::fs::create_dir_all(native.parent().expect("native parent")).expect("native directory");
        std::fs::write(&wrapper, "@node codex.js %*").expect("wrapper");
        std::fs::write(&native, "fixture").expect("native binary");
        let (program, prefix) =
            super::default_codex_batch_invocation(&wrapper).expect("standard npm shim invocation");
        assert_eq!(program, native);
        assert!(prefix.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn explicit_batch_wrapper_is_never_replaced_by_an_adjacent_codex_installation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let wrapper = temp.path().join("restricted-codex.cmd");
        std::fs::write(&wrapper, "@set CODEX_HOME=C:\\restricted\r\n@codex %*").expect("wrapper");
        let native = native_codex_fixture_path(temp.path());
        std::fs::create_dir_all(native.parent().expect("native parent")).expect("native directory");
        std::fs::write(&native, "fixture").expect("native binary");

        let (program, prefix) = super::codex_invocation(&wrapper);
        assert_eq!(program, wrapper);
        assert!(prefix.is_empty());
    }

    #[test]
    #[ignore = "subprocess fixture; launched by supervisor tests"]
    fn fake_process_exits_before_reading_prompt() {
        if std::env::var_os("RESEARCH_RUNTIME_FAKE_EARLY_EXIT").is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
        eprintln!("fake-codex-early-exit");
        std::process::exit(23);
    }

    #[tokio::test]
    async fn early_child_exit_preserves_exit_status_and_stderr_after_broken_pipe() {
        let executable = std::env::current_exe().expect("current test executable");
        let mut command = Command::new(&executable);
        command
            .arg("--exact")
            .arg("tests::fake_process_exits_before_reading_prompt")
            .arg("--ignored")
            .arg("--nocapture")
            .env("RESEARCH_RUNTIME_FAKE_EARLY_EXIT", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let prompt = vec![b'x'; 8 * 1024 * 1024];
        let capture = tokio::time::timeout(
            Duration::from_secs(10),
            run_codex_process(command, &prompt, 5, CancellationToken::new(), &executable),
        )
        .await
        .expect("supervisor did not hang")
        .expect("capture");
        let status = capture.status.expect("real child exit status");
        assert_eq!(status.code(), Some(23));
        let stderr = String::from_utf8_lossy(&capture.stderr);
        assert!(stderr.contains("fake-codex-early-exit"));
        assert!(stderr.contains("failed to deliver the Codex prompt"));
    }

    #[tokio::test]
    async fn pre_cancelled_run_does_not_spawn_the_configured_program() {
        let token = CancellationToken::new();
        token.cancel();
        let command = Command::new(PathBuf::from("definitely-missing-codex-command"));
        let error = run_codex_process(
            command,
            b"prompt",
            1,
            token,
            PathBuf::from("missing").as_path(),
        )
        .await
        .expect_err("pre-cancelled run");
        assert!(matches!(error, AgentError::Cancelled));
    }

    #[test]
    fn pipe_reader_failure_does_not_override_primary_cancellation_or_timeout() {
        for primary in [AgentError::Cancelled, AgentError::Timeout(7)] {
            let mut status = Err(primary);
            let mut stderr = b"original stderr".to_vec();
            record_supervisor_failure(&mut status, &mut stderr, "stdout reader failed".into());
            assert!(matches!(
                status,
                Err(AgentError::Cancelled | AgentError::Timeout(7))
            ));
            assert!(String::from_utf8_lossy(&stderr).contains("stdout reader failed"));
        }
    }
}
