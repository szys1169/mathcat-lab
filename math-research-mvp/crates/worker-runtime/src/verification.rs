use std::{
    collections::{BTreeSet, VecDeque},
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::Instant,
};

use async_trait::async_trait;
use research_domain::CheckStatus;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct BackendRequest {
    pub case_id: String,
    pub attempt_id: Option<String>,
    pub theorem_name: String,
    pub source: String,
    pub working_directory: PathBuf,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendOutcome {
    pub backend: String,
    pub backend_version: String,
    pub status: CheckStatus,
    pub command: Vec<String>,
    pub working_directory: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub diagnostics: Value,
    pub axioms: Vec<String>,
    pub elapsed_ms: i64,
    pub input_hash: String,
    pub output_hash: String,
}

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("verification backend unavailable: {0}")]
    Unavailable(String),
    #[error("verification backend timed out after {0} seconds")]
    TimedOut(u64),
    #[error("verification backend was cancelled")]
    Cancelled,
    #[error("verification backend protocol error: {0}")]
    Protocol(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[async_trait]
pub trait VerificationBackend: Send + Sync {
    fn name(&self) -> &'static str;
    async fn version(&self) -> Result<String, BackendError>;
    async fn verify(
        &self,
        request: BackendRequest,
        cancellation: CancellationToken,
    ) -> Result<BackendOutcome, BackendError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProofGoal {
    pub name: String,
    pub target: String,
    pub local_context: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofState {
    pub state_id: Option<u64>,
    pub goals: Vec<ProofGoal>,
    pub messages: Value,
    pub succeeded: bool,
    pub has_sorry: bool,
    pub has_unsafe: bool,
}

impl ProofState {
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.succeeded
            && self.state_id.is_some()
            && self.goals.is_empty()
            && !self.has_sorry
            && !self.has_unsafe
    }
}

#[async_trait]
pub trait InteractiveProofSession: Send + Sync {
    async fn start_goal(
        &self,
        expression: &str,
        cancellation: CancellationToken,
    ) -> Result<ProofState, BackendError>;

    async fn apply_tactic(
        &self,
        state_id: u64,
        tactic: &str,
        timeout_ms: u64,
        cancellation: CancellationToken,
    ) -> Result<ProofState, BackendError>;

    async fn remove_states(&self, state_ids: &[u64]) -> Result<(), BackendError>;

    async fn shutdown(self: Box<Self>) -> Result<(), BackendError>;
}

#[async_trait]
pub trait InteractiveProofBackend: Send + Sync {
    fn name(&self) -> &'static str;
    async fn version(&self) -> Result<String, BackendError>;
    async fn open_session(
        &self,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn InteractiveProofSession>, BackendError>;
}

#[derive(Debug, Clone)]
pub struct MockInteractiveProofBackend {
    root: ProofState,
    outcomes: Arc<Mutex<VecDeque<ProofState>>>,
}

impl MockInteractiveProofBackend {
    #[must_use]
    pub fn new(root: ProofState, outcomes: impl IntoIterator<Item = ProofState>) -> Self {
        Self {
            root,
            outcomes: Arc::new(Mutex::new(outcomes.into_iter().collect())),
        }
    }
}

#[derive(Debug, Clone)]
struct MockInteractiveProofSession {
    root: ProofState,
    outcomes: Arc<Mutex<VecDeque<ProofState>>>,
}

#[async_trait]
impl InteractiveProofBackend for MockInteractiveProofBackend {
    fn name(&self) -> &'static str {
        "mock_pantograph"
    }

    async fn version(&self) -> Result<String, BackendError> {
        Ok("mock-pantograph-1".into())
    }

    async fn open_session(
        &self,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn InteractiveProofSession>, BackendError> {
        if cancellation.is_cancelled() {
            return Err(BackendError::Cancelled);
        }
        Ok(Box::new(MockInteractiveProofSession {
            root: self.root.clone(),
            outcomes: self.outcomes.clone(),
        }))
    }
}

#[async_trait]
impl InteractiveProofSession for MockInteractiveProofSession {
    async fn start_goal(
        &self,
        _expression: &str,
        cancellation: CancellationToken,
    ) -> Result<ProofState, BackendError> {
        if cancellation.is_cancelled() {
            return Err(BackendError::Cancelled);
        }
        Ok(self.root.clone())
    }

    async fn apply_tactic(
        &self,
        _state_id: u64,
        _tactic: &str,
        _timeout_ms: u64,
        cancellation: CancellationToken,
    ) -> Result<ProofState, BackendError> {
        if cancellation.is_cancelled() {
            return Err(BackendError::Cancelled);
        }
        self.outcomes
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| BackendError::Unavailable("mock tactic outcome queue exhausted".into()))
    }

    async fn remove_states(&self, _state_ids: &[u64]) -> Result<(), BackendError> {
        Ok(())
    }

    async fn shutdown(self: Box<Self>) -> Result<(), BackendError> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct PantographConfig {
    pub executable: PathBuf,
    pub launcher_args: Vec<String>,
    pub working_directory: PathBuf,
    pub modules: Vec<String>,
    pub lean_path: Option<String>,
    pub startup_timeout_seconds: u64,
}

impl Default for PantographConfig {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("lake"),
            launcher_args: vec!["exe".into(), "repl".into()],
            working_directory: PathBuf::from("pantograph"),
            modules: vec!["Mathlib".into()],
            lean_path: None,
            startup_timeout_seconds: 120,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PantographBackend {
    config: PantographConfig,
}

impl PantographBackend {
    #[must_use]
    pub const fn new(config: PantographConfig) -> Self {
        Self { config }
    }

    fn resolved_executable(&self) -> Result<PathBuf, BackendError> {
        if self.config.executable.is_absolute() {
            return Ok(self.config.executable.clone());
        }
        if self.config.executable.components().count() == 1 {
            return Ok(self.config.executable.clone());
        }
        let current = std::env::current_dir()?;
        Ok(current
            .join(&self.config.working_directory)
            .join(&self.config.executable))
    }
}

#[async_trait]
impl InteractiveProofBackend for PantographBackend {
    fn name(&self) -> &'static str {
        "pantograph"
    }

    async fn version(&self) -> Result<String, BackendError> {
        if !self.config.launcher_args.is_empty() {
            let lock_path = self
                .config
                .working_directory
                .join("PANTOGRAPH_SOURCE_LOCK.json");
            let bytes = tokio::fs::read(&lock_path).await.map_err(|error| {
                BackendError::Unavailable(format!("read {}: {error}", lock_path.display()))
            })?;
            let lock: Value = serde_json::from_slice(&bytes)
                .map_err(|error| BackendError::Protocol(error.to_string()))?;
            let version = lock
                .pointer("/pantograph/version")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    BackendError::Protocol("Pantograph source lock has no version".into())
                })?;
            let commit = lock
                .pointer("/pantograph/commit")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            return Ok(format!(
                "Pantograph {version} ({})",
                commit.chars().take(12).collect::<String>()
            ));
        }
        let output = Command::new(self.resolved_executable()?)
            .args(&self.config.launcher_args)
            .arg("--version")
            .current_dir(&self.config.working_directory)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|error| BackendError::Unavailable(error.to_string()))?;
        if !output.status.success() {
            return Err(BackendError::Unavailable(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    async fn open_session(
        &self,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn InteractiveProofSession>, BackendError> {
        if cancellation.is_cancelled() {
            return Err(BackendError::Cancelled);
        }
        let mut command = Command::new(self.resolved_executable()?);
        command
            .args(&self.config.launcher_args)
            .args(&self.config.modules)
            .current_dir(&self.config.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(lean_path) = &self.config.lean_path {
            command.env("LEAN_PATH", lean_path);
        }
        let mut child = command
            .spawn()
            .map_err(|error| BackendError::Unavailable(error.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| BackendError::Protocol("Pantograph stdin missing".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| BackendError::Protocol("Pantograph stdout missing".into()))?;
        let mut process = PantographProcess {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        };
        let ready = tokio::select! {
            () = cancellation.cancelled() => return Err(BackendError::Cancelled),
            result = tokio::time::timeout(
                std::time::Duration::from_secs(self.config.startup_timeout_seconds),
                read_line(&mut process.stdout),
            ) => result.map_err(|_| BackendError::TimedOut(self.config.startup_timeout_seconds))??,
        };
        if ready.trim() != "ready." {
            return Err(BackendError::Protocol(format!(
                "Pantograph did not emit ready signal: {ready}"
            )));
        }
        Ok(Box::new(PantographSession {
            process: Mutex::new(process),
        }))
    }
}

#[derive(Debug)]
struct PantographProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

#[derive(Debug)]
struct PantographSession {
    process: Mutex<PantographProcess>,
}

impl PantographSession {
    async fn command(
        &self,
        command: &str,
        payload: Value,
        timeout_ms: u64,
        cancellation: CancellationToken,
    ) -> Result<Value, BackendError> {
        let mut process = self.process.lock().await;
        let request = serde_json::to_string(&json!({"cmd": command, "payload": payload}))
            .map_err(|error| BackendError::Protocol(error.to_string()))?;
        process.stdin.write_all(request.as_bytes()).await?;
        process.stdin.write_all(b"\n").await?;
        process.stdin.flush().await?;
        let timeout_ms = timeout_ms.max(1);
        let line = tokio::select! {
            () = cancellation.cancelled() => return Err(BackendError::Cancelled),
            result = tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms),
                read_line(&mut process.stdout),
            ) => result.map_err(|_| BackendError::TimedOut(timeout_ms.div_ceil(1000)))??,
        };
        let value: Value = serde_json::from_str(&line)
            .map_err(|error| BackendError::Protocol(format!("{error}; response={line}")))?;
        if let Some(kind) = value.get("error").and_then(Value::as_str) {
            let description = value
                .get("desc")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return Err(BackendError::Protocol(format!("{kind}: {description}")));
        }
        Ok(value)
    }
}

#[async_trait]
impl InteractiveProofSession for PantographSession {
    async fn start_goal(
        &self,
        expression: &str,
        cancellation: CancellationToken,
    ) -> Result<ProofState, BackendError> {
        let start = self
            .command(
                "goal.start",
                json!({"expr": expression}),
                120_000,
                cancellation.clone(),
            )
            .await?;
        let state_id = start
            .get("stateId")
            .and_then(Value::as_u64)
            .ok_or_else(|| BackendError::Protocol("goal.start returned no stateId".into()))?;
        let printed = self
            .command(
                "goal.print",
                json!({"stateId": state_id, "goals": true}),
                120_000,
                cancellation,
            )
            .await?;
        Ok(ProofState {
            state_id: Some(state_id),
            goals: parse_goals(printed.get("goals")),
            messages: json!([]),
            succeeded: true,
            has_sorry: printed
                .get("rootHasSorry")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            has_unsafe: printed
                .get("rootHasUnsafe")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    async fn apply_tactic(
        &self,
        state_id: u64,
        tactic: &str,
        timeout_ms: u64,
        cancellation: CancellationToken,
    ) -> Result<ProofState, BackendError> {
        let response = self
            .command(
                "goal.tactic",
                json!({"stateId": state_id, "tactic": tactic}),
                timeout_ms,
                cancellation,
            )
            .await?;
        let goals_field = response.get("goals");
        Ok(ProofState {
            state_id: response.get("nextStateId").and_then(Value::as_u64),
            goals: parse_goals(goals_field),
            messages: response
                .get("messages")
                .cloned()
                .unwrap_or_else(|| json!([])),
            succeeded: goals_field.is_some() && !goals_field.is_some_and(Value::is_null),
            has_sorry: response
                .get("hasSorry")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            has_unsafe: response
                .get("hasUnsafe")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    async fn remove_states(&self, state_ids: &[u64]) -> Result<(), BackendError> {
        self.command(
            "goal.remove",
            json!({"stateIds": state_ids}),
            30_000,
            CancellationToken::new(),
        )
        .await?;
        Ok(())
    }

    async fn shutdown(self: Box<Self>) -> Result<(), BackendError> {
        let mut process = self.process.lock().await;
        process.stdin.write_all(b"\n").await?;
        process.stdin.flush().await?;
        let _ = process.child.wait().await?;
        Ok(())
    }
}

fn parse_goals(value: Option<&Value>) -> Vec<ProofGoal> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|goal| ProofGoal {
            name: goal
                .get("userName")
                .or_else(|| goal.get("name"))
                .and_then(json_display)
                .unwrap_or_default(),
            target: goal
                .get("target")
                .and_then(expression_display)
                .unwrap_or_default(),
            local_context: goal
                .get("vars")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|variable| {
                    let name = variable
                        .get("userName")
                        .and_then(json_display)
                        .unwrap_or_default();
                    let value = variable
                        .get("type")
                        .and_then(expression_display)
                        .unwrap_or_default();
                    format!("{name} : {value}")
                })
                .collect(),
        })
        .collect()
}

fn expression_display(value: &Value) -> Option<String> {
    value
        .get("pp")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| json_display(value))
}

fn json_display(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| serde_json::to_string(value).ok())
}

async fn read_line(reader: &mut BufReader<ChildStdout>) -> Result<String, BackendError> {
    let mut line = String::new();
    let count = reader.read_line(&mut line).await?;
    if count == 0 {
        return Err(BackendError::Protocol(
            "Pantograph closed its protocol stream".into(),
        ));
    }
    Ok(line.trim().to_owned())
}

#[derive(Debug, Clone)]
pub struct MockVerificationBackend {
    outcomes: Arc<Mutex<VecDeque<Result<BackendOutcome, String>>>>,
}

impl MockVerificationBackend {
    #[must_use]
    pub fn from_outcomes(outcomes: impl IntoIterator<Item = BackendOutcome>) -> Self {
        Self {
            outcomes: Arc::new(Mutex::new(
                outcomes.into_iter().map(Ok).collect::<VecDeque<_>>(),
            )),
        }
    }
}

#[async_trait]
impl VerificationBackend for MockVerificationBackend {
    fn name(&self) -> &'static str {
        "mock_verification"
    }

    async fn version(&self) -> Result<String, BackendError> {
        Ok("mock-1".into())
    }

    async fn verify(
        &self,
        _request: BackendRequest,
        cancellation: CancellationToken,
    ) -> Result<BackendOutcome, BackendError> {
        if cancellation.is_cancelled() {
            return Err(BackendError::Cancelled);
        }
        self.outcomes
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| BackendError::Unavailable("mock outcome queue exhausted".into()))?
            .map_err(BackendError::Unavailable)
    }
}

#[derive(Debug, Clone)]
pub struct LeanKernelConfig {
    pub lake_executable: PathBuf,
    pub project_root: PathBuf,
    pub default_timeout_seconds: u64,
    pub allowed_axioms: BTreeSet<String>,
}

impl Default for LeanKernelConfig {
    fn default() -> Self {
        Self {
            lake_executable: PathBuf::from("lake"),
            project_root: PathBuf::from("lean-verifier"),
            default_timeout_seconds: 10 * 60,
            allowed_axioms: BTreeSet::from([
                "propext".into(),
                "Classical.choice".into(),
                "Quot.sound".into(),
            ]),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LeanKernelBackend {
    config: LeanKernelConfig,
}

impl LeanKernelBackend {
    #[must_use]
    pub const fn new(config: LeanKernelConfig) -> Self {
        Self { config }
    }

    #[allow(clippy::unused_self)]
    fn safety_violations(&self, source: &str) -> Vec<String> {
        let forbidden = [
            "sorry",
            "admit",
            "axiom",
            "unsafe",
            "foreign",
            "extern",
            "native_decide",
            "run_tac",
        ];
        let mut violations = forbidden
            .iter()
            .filter(|token| contains_token(source, token))
            .map(|token| format!("forbidden Lean token: {token}"))
            .collect::<Vec<_>>();
        let imports = source
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("import "))
            .collect::<Vec<_>>();
        if imports != ["import Mathlib"] {
            violations.push(
                "Lean source must contain exactly one approved import: import Mathlib".into(),
            );
        }
        violations
    }

    async fn run_version(&self) -> Result<String, BackendError> {
        let output = Command::new(&self.config.lake_executable)
            .arg("env")
            .arg("lean")
            .arg("--version")
            .current_dir(&self.config.project_root)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|error| BackendError::Unavailable(error.to_string()))?;
        if !output.status.success() {
            return Err(BackendError::Unavailable(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }
}

#[async_trait]
impl VerificationBackend for LeanKernelBackend {
    fn name(&self) -> &'static str {
        "lean_kernel"
    }

    async fn version(&self) -> Result<String, BackendError> {
        self.run_version().await
    }

    async fn verify(
        &self,
        request: BackendRequest,
        cancellation: CancellationToken,
    ) -> Result<BackendOutcome, BackendError> {
        let started = Instant::now();
        let input_hash = sha256(request.source.as_bytes());
        let version = self.run_version().await?;
        let command = vec![
            self.config.lake_executable.to_string_lossy().into_owned(),
            "env".into(),
            "lean".into(),
            "Verification.lean".into(),
        ];
        let working_directory = request.working_directory.to_string_lossy().into_owned();
        let violations = self.safety_violations(&request.source);
        if !violations.is_empty() {
            let diagnostics = json!({"kind":"safety_policy", "violations":violations});
            let output_hash = sha256(&serde_json::to_vec(&diagnostics).unwrap_or_default());
            return Ok(BackendOutcome {
                backend: self.name().into(),
                backend_version: version,
                status: CheckStatus::Failed,
                command,
                working_directory,
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                diagnostics,
                axioms: vec![],
                elapsed_ms: elapsed_ms(started),
                input_hash,
                output_hash,
            });
        }
        tokio::fs::create_dir_all(&request.working_directory).await?;
        let source_path = request.working_directory.join("Verification.lean");
        let checked_source = format!(
            "{}\n\n#print axioms {}\n",
            request.source, request.theorem_name
        );
        let mut file = tokio::fs::File::create(&source_path).await?;
        file.write_all(checked_source.as_bytes()).await?;
        file.flush().await?;

        let mut process = Command::new(&self.config.lake_executable);
        process
            .arg("env")
            .arg("lean")
            .arg(&source_path)
            .current_dir(&self.config.project_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = process
            .spawn()
            .map_err(|error| BackendError::Unavailable(error.to_string()))?;
        let timeout_seconds = if request.timeout_seconds == 0 {
            self.config.default_timeout_seconds
        } else {
            request.timeout_seconds
        };
        let wait = child.wait_with_output();
        let output = tokio::select! {
            () = cancellation.cancelled() => return Err(BackendError::Cancelled),
            result = tokio::time::timeout(std::time::Duration::from_secs(timeout_seconds), wait) => {
                match result {
                    Ok(output) => output?,
                    Err(_) => return Err(BackendError::TimedOut(timeout_seconds)),
                }
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let axioms = parse_axioms(&stdout);
        let unapproved_axioms = axioms
            .iter()
            .filter(|axiom| !self.config.allowed_axioms.contains(*axiom))
            .cloned()
            .collect::<Vec<_>>();
        let status = if output.status.success() && unapproved_axioms.is_empty() {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        };
        let diagnostics = json!({
            "compiler_success": output.status.success(),
            "unapproved_axioms": unapproved_axioms,
            "source_path": source_path,
        });
        let output_hash = sha256(
            &serde_json::to_vec(&json!({
                "exit_code": output.status.code(), "stdout": stdout, "stderr": stderr,
                "diagnostics": diagnostics, "axioms": axioms,
            }))
            .unwrap_or_default(),
        );
        Ok(BackendOutcome {
            backend: self.name().into(),
            backend_version: version,
            status,
            command,
            working_directory,
            exit_code: output.status.code(),
            stdout,
            stderr,
            diagnostics,
            axioms,
            elapsed_ms: elapsed_ms(started),
            input_hash,
            output_hash,
        })
    }
}

fn contains_token(source: &str, token: &str) -> bool {
    source.match_indices(token).any(|(index, _)| {
        let before = source[..index].chars().next_back();
        let after = source[index + token.len()..].chars().next();
        before.is_none_or(|character| !is_identifier_character(character))
            && after.is_none_or(|character| !is_identifier_character(character))
    })
}

fn is_identifier_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_' || character == '\''
}

fn parse_axioms(output: &str) -> Vec<String> {
    let Some(start) = output.rfind('[') else {
        return vec![];
    };
    let Some(relative_end) = output[start..].find(']') else {
        return vec![];
    };
    output[start + 1..start + relative_end]
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn elapsed_ms(started: Instant) -> i64 {
    i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{LeanKernelBackend, LeanKernelConfig, contains_token, parse_axioms};

    #[test]
    fn forbidden_tokens_use_identifier_boundaries() {
        assert!(contains_token("by sorry", "sorry"));
        assert!(!contains_token(
            "theorem sorryFree : True := by trivial",
            "sorry"
        ));
    }

    #[test]
    fn parses_print_axioms_output() {
        assert_eq!(
            parse_axioms("'demo' depends on axioms: [propext, Classical.choice, Quot.sound]"),
            ["propext", "Classical.choice", "Quot.sound"]
        );
    }

    #[test]
    fn safety_policy_rejects_untrusted_escape_hatches() {
        let backend = LeanKernelBackend::new(LeanKernelConfig::default());
        assert!(
            backend
                .safety_violations("import Mathlib\naxiom bad : False")
                .iter()
                .any(|item| item.contains("axiom"))
        );
        assert!(
            backend
                .safety_violations(
                    "import Mathlib\nimport Batteries\ntheorem ok : True := by trivial"
                )
                .iter()
                .any(|item| item.contains("approved import"))
        );
        assert!(
            backend
                .safety_violations("import Mathlib\ntheorem bad : True := by native_decide")
                .iter()
                .any(|item| item.contains("native_decide"))
        );
        assert_eq!(
            backend
                .safety_violations("import Mathlib\ntheorem ok : True := by trivial")
                .len(),
            0
        );
    }
}
