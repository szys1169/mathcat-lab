use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use research_core::{ResearchConfig, ResearchService};
use research_domain::{Budget, ProblemContract, ProjectStatus, WorkerOutput};
use research_storage::{CommandDraft, PostgresStateCommitter, SqliteStore};
use research_worker_runtime::{
    AgentBackend, AgentSpec, AgentTask, AgentTaskKind, CodexCliBackend, CodexCliConfig,
    InteractiveProofBackend, LeanKernelBackend, LeanKernelConfig, PantographBackend,
    PantographConfig,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "math-research-agent",
    version,
    about = "可恢复、可审计的数学研究智能体后端"
)]
struct Cli {
    #[arg(
        long,
        env = "MRA_DATABASE_URL",
        default_value = "sqlite://runtime/research.db"
    )]
    database_url: String,
    #[arg(long, env = "MRA_ARTIFACT_ROOT", default_value = "runtime/artifacts")]
    artifact_root: PathBuf,
    #[arg(long, env = "MRA_RUNTIME_ROOT", default_value = "runtime/projects")]
    runtime_root: PathBuf,
    #[arg(long, env = "MRA_OUTPUT_ROOT", default_value = "output")]
    output_root: PathBuf,
    #[arg(long, env = "MRA_CODEX_COMMAND", default_value = "codex.cmd")]
    codex_command: PathBuf,
    #[arg(long, env = "MRA_LAKE_COMMAND", default_value = "lake")]
    lake_command: PathBuf,
    #[arg(long, env = "MRA_LEAN_PROJECT_ROOT", default_value = "lean-verifier")]
    lean_project_root: PathBuf,
    #[arg(long, env = "MRA_PANTOGRAPH_COMMAND", default_value = "lake")]
    pantograph_command: PathBuf,
    #[arg(long, env = "MRA_PANTOGRAPH_ROOT", default_value = "pantograph")]
    pantograph_root: PathBuf,
    #[arg(
        long = "pantograph-module",
        env = "MRA_PANTOGRAPH_MODULES",
        value_delimiter = ',',
        default_value = "Mathlib"
    )]
    pantograph_modules: Vec<String>,
    #[arg(long, env = "MRA_MODEL")]
    model: Option<String>,
    #[arg(long, env = "MRA_PLANNER_TIMEOUT_SECONDS", default_value_t = 20 * 60)]
    planner_timeout_seconds: u64,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// 启动 REST/SSE 服务。
    Serve {
        #[arg(long, env = "MRA_BIND", default_value = "127.0.0.1:8787")]
        bind: SocketAddr,
    },
    /// 检查数据库、目录和 Codex CLI。
    Doctor,
    /// 连接 PostgreSQL、应用生产 State Committer 迁移并检查连接池。
    PostgresDoctor {
        #[arg(long, env = "MRA_POSTGRES_URL")]
        postgres_url: String,
        #[arg(long, default_value_t = 4)]
        max_connections: u32,
    },
    /// 创建数学研究项目。
    Create {
        #[arg(long)]
        name: String,
        #[arg(
            long,
            conflicts_with = "problem_file",
            required_unless_present = "problem_file"
        )]
        problem: Option<String>,
        /// 从 UTF-8 文件逐字读取题面，适合可重放基准测试。
        #[arg(long, conflicts_with = "problem")]
        problem_file: Option<PathBuf>,
        #[arg(long)]
        target: Option<String>,
        #[arg(long = "assumption")]
        assumptions: Vec<String>,
        #[arg(long, default_value_t = 12)]
        max_rounds: u32,
        #[arg(long, default_value_t = 3)]
        max_parallel_workers: u32,
        #[arg(long, default_value_t = 45)]
        max_minutes_per_task: u32,
        #[arg(long, default_value_t = 4)]
        max_model_calls_per_task: u32,
        #[arg(long, default_value_t = 120)]
        max_total_model_calls: u32,
    },
    /// 从持久状态持续运行项目直到终态。
    Run { project_id: String },
    /// 只执行一轮，便于诊断。
    RunOnce { project_id: String },
    /// 打印聚合状态。
    Status { project_id: String },
    /// 打印完整一致性快照。
    Snapshot { project_id: String },
    /// 打印项目用量与可比较的运行指标。
    Metrics { project_id: String },
    /// 仅用 active Facts 与 admitted Sources 生成受控论文候选。
    Publish {
        project_id: String,
        /// 允许主目标未闭合或没有文献来源时生成明确标注的阶段稿。
        #[arg(long)]
        allow_partial: bool,
    },
    /// 输出 `OpenAPI` 3.1 文档。
    Openapi {
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// 显式执行一次最小 Codex CLI 结构化输出测试（会调用模型）。
    CodexSmoke,
    /// 启动真实 Pantograph 会话并执行一个 tactic。
    PantographSmoke {
        #[arg(long, default_value = "(1 : Nat) + 1 = 2")]
        expression: String,
        #[arg(long, default_value = "norm_num")]
        tactic: String,
    },
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info")),
        )
        .init();
    let cli = Cli::parse();
    if let Commands::PostgresDoctor {
        postgres_url,
        max_connections,
    } = &cli.command
    {
        let committer = PostgresStateCommitter::connect(postgres_url, *max_connections).await?;
        let _connection = committer.pool().acquire().await?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "status":"ok",
                "storage_mode":"postgres_state_committer",
                "migrations":"applied",
                "pool_size":committer.pool().size(),
            }))?
        );
        return Ok(());
    }
    if !cli.database_url.starts_with("sqlite:") {
        bail!(
            "the monolithic CLI currently requires a sqlite: URL; use postgres-doctor for the production State Committer instead of treating PostgreSQL as SQLite"
        );
    }
    prepare_database_parent(&cli.database_url).await?;
    let store = SqliteStore::connect(&cli.database_url, &cli.artifact_root).await?;
    let backend = Arc::new(CodexCliBackend::new(CodexCliConfig {
        command: cli.codex_command.clone(),
        default_model: cli.model.clone(),
        ..CodexCliConfig::default()
    }));
    let lean_backend = Arc::new(LeanKernelBackend::new(LeanKernelConfig {
        lake_executable: cli.lake_command.clone(),
        project_root: cli.lean_project_root.clone(),
        ..LeanKernelConfig::default()
    }));
    let lean_path = discover_lean_path(&cli.lake_command, &cli.lean_project_root).await;
    if lean_path.is_none() {
        warn!(
            "could not discover LEAN_PATH; Pantograph Mathlib sessions will be unavailable until the Lean project is built"
        );
    }
    let (pantograph_executable, pantograph_launcher_args) =
        if cli.pantograph_command == Path::new("lake") {
            (cli.lake_command.clone(), vec!["exe".into(), "repl".into()])
        } else {
            (cli.pantograph_command.clone(), vec![])
        };
    let pantograph_backend = Arc::new(PantographBackend::new(PantographConfig {
        executable: pantograph_executable,
        launcher_args: pantograph_launcher_args,
        working_directory: cli.pantograph_root.clone(),
        modules: cli.pantograph_modules.clone(),
        lean_path,
        ..PantographConfig::default()
    }));
    let service = ResearchService::new_with_verification_backends(
        store,
        backend.clone(),
        Some(lean_backend),
        Some(pantograph_backend.clone()),
        ResearchConfig {
            runtime_root: cli.runtime_root.clone(),
            output_root: cli.output_root.clone(),
            model: cli.model.clone(),
            lean_project_root: Some(cli.lean_project_root.clone()),
            planner_timeout_seconds: cli.planner_timeout_seconds,
            ..ResearchConfig::default()
        },
    );

    match cli.command {
        Commands::Serve { bind } => serve(service, bind).await,
        Commands::Doctor => doctor(&cli, &service).await,
        Commands::PostgresDoctor { .. } => unreachable!("handled before SQLite initialization"),
        Commands::Create {
            name,
            problem,
            problem_file,
            target,
            assumptions,
            max_rounds,
            max_parallel_workers,
            max_minutes_per_task,
            max_model_calls_per_task,
            max_total_model_calls,
        } => {
            let problem = read_problem(problem, problem_file.as_deref()).await?;
            let project = service
                .create_project(
                    name,
                    ProblemContract {
                        original_problem: problem.clone(),
                        target_statement: target.unwrap_or(problem),
                        assumptions,
                        success_criteria:
                            "目标陈述得到 accepted 裁决，依赖闭包均为 active，且无阻塞不确定性"
                                .into(),
                        version: 1,
                    },
                    Budget {
                        max_rounds,
                        max_parallel_workers,
                        max_minutes_per_task,
                        max_model_calls_per_task,
                        max_total_model_calls,
                    },
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&project)?);
            Ok(())
        }
        Commands::Run { project_id } => {
            ensure_running(&service, &project_id).await?;
            Box::pin(service.run_project_until_terminal(&project_id)).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&service.store().get_project(&project_id).await?)?
            );
            Ok(())
        }
        Commands::RunOnce { project_id } => {
            ensure_running(&service, &project_id).await?;
            service.run_one_round(&project_id).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&service.store().snapshot(&project_id).await?)?
            );
            Ok(())
        }
        Commands::Status { project_id } => {
            let snapshot = service.store().snapshot(&project_id).await?;
            let model_calls = service.store().total_model_calls(&project_id).await?;
            let model_call_limit = snapshot.project.budget.max_total_model_calls;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "project": snapshot.project,
                    "round": snapshot.current_round,
                    "worker_count": snapshot.workers.len(),
                    "task_count": snapshot.tasks.len(),
                    "fact_count": snapshot.facts.len(),
                    "open_uncertainty_count": snapshot.uncertainties.iter().filter(|item| item.status.to_string()=="open").count(),
                    "model_calls": {"used":model_calls,"limit":model_call_limit},
                    "event_cursor": snapshot.event_cursor,
                }))?
            );
            Ok(())
        }
        Commands::Snapshot { project_id } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&service.store().snapshot(&project_id).await?)?
            );
            Ok(())
        }
        Commands::Metrics { project_id } => {
            let snapshot = service.store().snapshot(&project_id).await?;
            let usage = service.store().usage_summary(&project_id).await?;
            let candidates = service.store().list_verifications(&project_id).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "project_id": project_id,
                    "status": snapshot.project.status,
                    "rounds": snapshot.project.current_round,
                    "project_revision": snapshot.project_revision,
                    "event_cursor": snapshot.event_cursor,
                    "counts": {
                        "goals": snapshot.goals.len(),
                        "routes": snapshot.routes.len(),
                        "workers": snapshot.workers.len(),
                        "tasks": snapshot.tasks.len(),
                        "candidates": candidates.len(),
                        "facts": snapshot.facts.len(),
                        "open_uncertainties": snapshot.uncertainties.iter().filter(|item| item.status.to_string() == "open").count(),
                    },
                    "usage": usage,
                }))?
            );
            Ok(())
        }
        Commands::Publish {
            project_id,
            allow_partial,
        } => {
            let result = service.publish_paper(&project_id, allow_partial).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(())
        }
        Commands::Openapi { output } => {
            let document = serde_json::to_vec_pretty(&research_api::openapi_document())?;
            if let Some(path) = output {
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&path, document).await?;
                info!(path=%path.display(), "OpenAPI document written");
            } else {
                println!("{}", String::from_utf8(document)?);
            }
            Ok(())
        }
        Commands::CodexSmoke => codex_smoke(backend, &cli.runtime_root, cli.model).await,
        Commands::PantographSmoke { expression, tactic } => {
            pantograph_smoke(pantograph_backend, &expression, &tactic).await
        }
    }
}

async fn serve(service: ResearchService, bind: SocketAddr) -> Result<()> {
    if !bind.ip().is_loopback() && service.store().actor_count().await? == 0 {
        bail!(
            "refusing to bind {bind} before an administrator is bootstrapped; use the local bootstrap endpoint first"
        );
    }
    if !bind.ip().is_loopback() {
        warn!(%bind, "binding beyond loopback; terminate TLS and protect the service at a trusted reverse proxy");
    }
    service
        .store()
        .run_reconciliation(None, "service_startup")
        .await
        .context("startup reconciliation failed")?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    let shutdown = CancellationToken::new();
    let watchdog_shutdown = shutdown.clone();
    let watchdog_service = service.clone();
    let watchdog = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            tokio::select! {
                () = watchdog_shutdown.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(error) = watchdog_service.store().run_reconciliation(None, "watchdog_tick").await {
                        warn!(%error, "storage watchdog reconciliation failed");
                    }
                }
            }
        }
    });
    info!(%bind, "math research agent API listening");
    let signal_shutdown = shutdown.clone();
    let result = axum::serve(listener, research_api::router(service))
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            signal_shutdown.cancel();
        })
        .await;
    shutdown.cancel();
    let _ = watchdog.await;
    result?;
    Ok(())
}

async fn doctor(cli: &Cli, service: &ResearchService) -> Result<()> {
    let output = tokio::process::Command::new(&cli.codex_command)
        .arg("--version")
        .output()
        .await
        .with_context(|| format!("cannot execute {}", cli.codex_command.display()))?;
    if !output.status.success() {
        bail!(
            "Codex CLI failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let capabilities = service.backend_name();
    let lean_version = service.verification_backend_version().await?;
    let pantograph_version = service.interactive_proof_backend_version().await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status":"ok",
            "database_url":cli.database_url,
            "artifact_root":cli.artifact_root,
            "runtime_root":cli.runtime_root,
            "output_root":cli.output_root,
            "backend":capabilities,
            "codex_version":version,
            "verification_backend":service.verification_backend_name(),
            "lean_version":lean_version,
            "lean_project_root":cli.lean_project_root,
            "interactive_proof_backend":service.interactive_proof_backend_name(),
            "pantograph_version":pantograph_version,
            "pantograph_root":cli.pantograph_root,
            "pantograph_modules":cli.pantograph_modules,
        }))?
    );
    Ok(())
}

async fn pantograph_smoke(
    backend: Arc<PantographBackend>,
    expression: &str,
    tactic: &str,
) -> Result<()> {
    let cancellation = CancellationToken::new();
    let session = backend.open_session(cancellation.clone()).await?;
    let root = session.start_goal(expression, cancellation.clone()).await?;
    let state_id = root.state_id.context("Pantograph root has no state id")?;
    let result = session
        .apply_tactic(state_id, tactic, 60_000, cancellation)
        .await?;
    session.shutdown().await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "backend":backend.name(),
            "version":backend.version().await?,
            "expression":expression,
            "tactic":tactic,
            "root":root,
            "result":result,
            "closed":result.is_closed(),
        }))?
    );
    Ok(())
}

async fn discover_lean_path(lake_command: &Path, lean_project_root: &Path) -> Option<String> {
    let output = tokio::process::Command::new(lake_command)
        .arg("env")
        .arg("printenv")
        .arg("LEAN_PATH")
        .current_dir(lean_project_root)
        .output()
        .await
        .ok()?;
    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !value.is_empty() {
            return Some(value);
        }
    }
    if cfg!(windows) {
        let output = tokio::process::Command::new(lake_command)
            .arg("env")
            .arg("powershell.exe")
            .arg("-NoProfile")
            .arg("-Command")
            .arg("$env:LEAN_PATH")
            .current_dir(lean_project_root)
            .output()
            .await
            .ok()?;
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

async fn ensure_running(service: &ResearchService, project_id: &str) -> Result<()> {
    let project = service.store().get_project(project_id).await?;
    let command_type = match project.status {
        ProjectStatus::Created => Some("start_project"),
        ProjectStatus::Paused => Some("resume_project"),
        ProjectStatus::Running => None,
        status => bail!("project cannot run from terminal status {status}"),
    };
    if let Some(command_type) = command_type {
        let key = format!("cli-{command_type}-{}", ulid::Ulid::new());
        let (command, _) = service
            .store()
            .enqueue_command(
                project_id,
                CommandDraft {
                    command_type: command_type.into(),
                    target_kind: "project".into(),
                    target_id: project_id.into(),
                    mode: research_domain::CommandMode::Immediate,
                    payload: json!({}),
                    expected_project_revision: project.revision,
                    idempotency_key: key,
                    reason: "CLI run requested".into(),
                    requested_by: "cli:operator".into(),
                },
            )
            .await?;
        service
            .store()
            .apply_command(project_id, &command.command_id)
            .await?;
    }
    Ok(())
}

async fn codex_smoke(
    backend: Arc<CodexCliBackend>,
    runtime_root: &Path,
    model: Option<String>,
) -> Result<()> {
    let handle = backend
        .create(AgentSpec {
            project_id: "smoke".into(),
            role: "doctor".into(),
            model,
            working_directory: runtime_root.join("smoke"),
        })
        .await?;
    let result = backend.run(&handle, AgentTask {
        kind: AgentTaskKind::Worker,
        prompt: "Return a minimal valid WorkerOutput with summary='codex cli production schema works' and empty discoveries, candidates, failures, uncertainties, and sources arrays. Do not call tools.".into(),
        output_schema: research_core::worker_schema(),
        timeout_seconds: 120,
    }, CancellationToken::new()).await?;
    let output: WorkerOutput = serde_json::from_value(result.structured_output)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"ok":true,"summary":output.summary}))?
    );
    Ok(())
}

async fn read_problem(problem: Option<String>, problem_file: Option<&Path>) -> Result<String> {
    let content = match (problem, problem_file) {
        (Some(content), None) => content,
        (None, Some(path)) => tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("read UTF-8 problem file {}", path.display()))?,
        _ => bail!("provide exactly one of --problem or --problem-file"),
    };
    if content.trim().is_empty() {
        bail!("problem statement must not be empty");
    }
    Ok(content)
}

async fn prepare_database_parent(database_url: &str) -> Result<()> {
    if let Some(path) = database_url.strip_prefix("sqlite://") {
        let path = Path::new(path);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Cli, Commands};
    use clap::Parser;

    #[test]
    fn create_accepts_problem_file_for_replayable_inputs() {
        let cli = Cli::try_parse_from([
            "math-research-agent",
            "create",
            "--name",
            "benchmark",
            "--problem-file",
            "case.md",
        ])
        .expect("problem file should be accepted");
        let Commands::Create {
            problem,
            problem_file,
            ..
        } = cli.command
        else {
            panic!("expected create command");
        };
        assert!(problem.is_none());
        assert_eq!(problem_file.as_deref(), Some(Path::new("case.md")));
    }

    #[test]
    fn create_rejects_ambiguous_problem_sources() {
        let error = Cli::try_parse_from([
            "math-research-agent",
            "create",
            "--name",
            "benchmark",
            "--problem",
            "statement",
            "--problem-file",
            "case.md",
        ])
        .expect_err("inline and file inputs must conflict");
        assert!(error.to_string().contains("cannot be used with"));
    }
}
