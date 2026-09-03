// Service methods share the CoreError contract; prompt and orchestration APIs are documented
// at the crate/README boundary instead of repeating identical error sections per method.
#![allow(clippy::missing_errors_doc)]

mod prompts;
mod publication;
mod reporting;
mod schemas;

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use futures::{StreamExt, stream};
use research_domain::{
    AcceptanceClass, AlignmentRelation, AlignmentReviewerOutput, Budget, CandidateDraft,
    CandidateSubmission, CheckStatus, CommandStatus, ContextPacket, DomainEvent, FailureDraft,
    Formalization, FormalizerOutput, Goal, HumanCommand, HumanRouteProposalRequest,
    HumanRouteProposalResult, PlannerOutput, ProblemContract, ProblemRevisionRequest,
    ProblemRevisionResult, Project, ProjectSnapshot, ProjectStatus, ProofHint, ProofNode,
    ProofNodeStatus, ProofSearchBudget, RankingWeights, ReflectionOutput, ResearchDelta,
    RouteGeneratorOutput, RouteProposal, SourceDraft, SourceRecord, StrategyDirectorOutput,
    SupervisorOutput, TacticCandidate, TacticProposalOutput, Task, TaskContract, TaskStatus,
    VerificationCase, VerificationProfile, VerificationReport, VerificationStage,
    VerificationVerdict, WorkerOutput,
};
use research_storage::{
    BackendRunDraft, CheckDraft, CommandDraft, EvidenceDraft, FindingDraft, ModelCallRequest,
    ProofNodeDraft, SqliteStore, StorageError, SubmissionReceipt, VerificationCaseDraft,
    VerificationSnapshotDraft,
};
use research_worker_runtime::{
    AgentBackend, AgentError, AgentHandle, AgentRunResult, AgentSpec, AgentTask, AgentTaskKind,
    BackendError, BackendOutcome, BackendRequest, InteractiveProofBackend, VerificationBackend,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::process::Command;
use tokio::sync::{Mutex, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

pub use prompts::{
    alignment_prompt, formalizer_prompt, planner_prompt, reflection_delta_prompt,
    reflection_prompt, route_generator_delta_prompt, route_generator_prompt,
    strategy_director_prompt, supervisor_delta_prompt, supervisor_prompt, tactic_proposal_prompt,
    verifier_prompt, worker_packet_prompt, worker_prompt,
};
pub use publication::{PaperWriterOutput, PublicationResult};
pub use schemas::{
    alignment_schema, formalizer_schema, paper_writer_schema, planner_schema, reflection_schema,
    route_generator_schema, strategy_director_schema, supervisor_schema, tactic_proposal_schema,
    verifier_schema, worker_schema,
};

#[derive(Debug, Clone)]
pub struct ResearchConfig {
    pub runtime_root: PathBuf,
    pub output_root: PathBuf,
    pub model: Option<String>,
    pub lean_project_root: Option<PathBuf>,
    pub proof_search_budget: ProofSearchBudget,
    pub ranking_weights: RankingWeights,
    pub planner_timeout_seconds: u64,
    pub worker_timeout_seconds: u64,
    pub verifier_timeout_seconds: u64,
}

struct ReviewerContext<'a> {
    project: &'a Project,
    submission: &'a CandidateSubmission,
    dependencies: &'a [research_domain::Fact],
    sources: &'a [SourceRecord],
    target_goals: &'a [Goal],
}

struct PlanningStageRequest<'a> {
    round: &'a research_domain::ResearchRound,
    snapshot: &'a ProjectSnapshot,
    stage: &'a str,
    role: &'a str,
    input: &'a Value,
    prompt: String,
    output_schema: Value,
}

impl Default for ResearchConfig {
    fn default() -> Self {
        Self {
            runtime_root: PathBuf::from("runtime/projects"),
            output_root: PathBuf::from("output"),
            model: None,
            lean_project_root: Some(PathBuf::from("lean-verifier")),
            proof_search_budget: ProofSearchBudget::default(),
            ranking_weights: RankingWeights::default(),
            planner_timeout_seconds: 20 * 60,
            worker_timeout_seconds: 45 * 60,
            verifier_timeout_seconds: 30 * 60,
        }
    }
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error(transparent)]
    VerificationBackend(#[from] BackendError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid agent output: {0}")]
    InvalidAgentOutput(String),
}

pub type CoreResult<T> = Result<T, CoreError>;

struct CountedAgentRun<'a> {
    handle: &'a AgentHandle,
    task: AgentTask,
    project: &'a Project,
    round: i64,
    worker_id: Option<&'a str>,
    task_id: Option<&'a str>,
    cancellation: CancellationToken,
}

#[derive(Debug, Clone)]
struct SearchFrontier {
    node: ProofNode,
    tactic_path: Vec<String>,
}

#[derive(Clone)]
pub struct ResearchService {
    store: SqliteStore,
    backend: Arc<dyn AgentBackend>,
    verification_backend: Option<Arc<dyn VerificationBackend>>,
    interactive_proof_backend: Option<Arc<dyn InteractiveProofBackend>>,
    config: ResearchConfig,
    event_bus: broadcast::Sender<DomainEvent>,
    task_cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    project_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl std::fmt::Debug for ResearchService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResearchService")
            .field("store", &self.store)
            .field("backend", &self.backend.name())
            .field(
                "verification_backend",
                &self
                    .verification_backend
                    .as_ref()
                    .map(|backend| backend.name()),
            )
            .field(
                "interactive_proof_backend",
                &self
                    .interactive_proof_backend
                    .as_ref()
                    .map(|backend| backend.name()),
            )
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl ResearchService {
    #[must_use]
    pub fn new(store: SqliteStore, backend: Arc<dyn AgentBackend>, config: ResearchConfig) -> Self {
        Self::new_with_verification_backends(store, backend, None, None, config)
    }

    #[must_use]
    pub fn new_with_verification_backend(
        store: SqliteStore,
        backend: Arc<dyn AgentBackend>,
        verification_backend: Option<Arc<dyn VerificationBackend>>,
        config: ResearchConfig,
    ) -> Self {
        Self::new_with_verification_backends(store, backend, verification_backend, None, config)
    }

    #[must_use]
    pub fn new_with_verification_backends(
        store: SqliteStore,
        backend: Arc<dyn AgentBackend>,
        verification_backend: Option<Arc<dyn VerificationBackend>>,
        interactive_proof_backend: Option<Arc<dyn InteractiveProofBackend>>,
        config: ResearchConfig,
    ) -> Self {
        let (event_bus, _) = broadcast::channel(4096);
        Self {
            store,
            backend,
            verification_backend,
            interactive_proof_backend,
            config,
            event_bus,
            task_cancellations: Arc::new(Mutex::new(HashMap::new())),
            project_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    #[must_use]
    pub fn config(&self) -> &ResearchConfig {
        &self.config
    }

    #[must_use]
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    /// Atomically revise the immutable problem-contract history and invalidate stale work.
    pub async fn revise_problem_contract(
        &self,
        project_id: &str,
        request: &ProblemRevisionRequest,
        requested_by: &str,
        idempotency_key: &str,
    ) -> CoreResult<research_storage::BoardMutation<ProblemRevisionResult>> {
        let mut result = self
            .store
            .revise_problem_contract(project_id, request, requested_by, idempotency_key)
            .await?;
        self.publish_all(std::mem::take(&mut result.events));
        {
            let tokens = self.task_cancellations.lock().await;
            for affected in &result.data.affected_entities {
                if affected.kind == "task"
                    && let Some(token) = tokens.get(&affected.id)
                {
                    token.cancel();
                }
            }
        }
        if request.replan {
            let service = self.clone();
            let project_id = project_id.to_owned();
            tokio::spawn(async move {
                if let Err(error) = Box::pin(service.run_project_until_terminal(&project_id)).await
                {
                    error!(%project_id, %error, "research loop stopped after problem revision");
                }
            });
        }
        Ok(result)
    }

    /// Persist a human route as an auditable proposal, never as a directly runnable route.
    pub async fn propose_human_route(
        &self,
        project_id: &str,
        request: &HumanRouteProposalRequest,
        requested_by: &str,
        idempotency_key: &str,
    ) -> CoreResult<research_storage::BoardMutation<HumanRouteProposalResult>> {
        let mut result = self
            .store
            .propose_human_route(project_id, request, requested_by, idempotency_key)
            .await?;
        self.publish_all(std::mem::take(&mut result.events));
        Ok(result)
    }

    /// Build a paper candidate from verified facts and admitted sources without changing either.
    pub async fn publish_paper(
        &self,
        project_id: &str,
        allow_partial: bool,
    ) -> CoreResult<PublicationResult> {
        let revision = self.store.get_project(project_id).await?.revision;
        self.publish_paper_idempotent(
            project_id,
            allow_partial,
            &format!("cli-publication-{revision}-{allow_partial}"),
        )
        .await
    }

    /// Build or replay an idempotent, revision-pinned publication run.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn publish_paper_idempotent(
        &self,
        project_id: &str,
        allow_partial: bool,
        idempotency_key: &str,
    ) -> CoreResult<PublicationResult> {
        let project_lock = {
            let mut locks = self.project_locks.lock().await;
            locks
                .entry(project_id.to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = project_lock.lock().await;
        let snapshot = self.store.snapshot(project_id).await?;
        let packet = publication::source_packet(&snapshot);
        let packet_bytes = serde_json::to_vec_pretty(&packet)?;
        let packet_hash = hex::encode(Sha256::digest(&packet_bytes));
        let (publication_run, publication_event) = self
            .store
            .begin_publication(
                project_id,
                idempotency_key,
                allow_partial,
                snapshot.project_revision,
                &packet_hash,
            )
            .await?;
        if let Some(event) = publication_event {
            self.publish(event);
        } else if publication_run.status != "running" {
            if publication_run.status == "failed" {
                return Err(CoreError::InvalidAgentOutput(
                    publication_run
                        .error
                        .unwrap_or_else(|| "publication failed without a stored reason".into()),
                ));
            }
            return publication_run
                .result
                .ok_or_else(|| {
                    CoreError::InvalidAgentOutput(
                        "terminal publication run has no stored result".into(),
                    )
                })
                .and_then(|value| serde_json::from_value(value).map_err(CoreError::from));
        }
        let writer_root = self
            .config
            .output_root
            .join(project_id)
            .join("writer")
            .join(&publication_run.publication_id);
        tokio::fs::create_dir_all(&writer_root).await?;
        let outcome = self
            .generate_publication(
                &publication_run.publication_id,
                project_id,
                allow_partial,
                &snapshot,
                &packet,
                &packet_bytes,
                &writer_root,
            )
            .await;
        match outcome {
            Ok(result) => {
                let terminal_status = if result.status == "ready" {
                    "ready"
                } else {
                    "blocked_by_evidence"
                };
                let (_, event) = self
                    .store
                    .complete_publication(
                        &publication_run.publication_id,
                        terminal_status,
                        Some(&serde_json::to_value(&result)?),
                        None,
                    )
                    .await?;
                if let Some(event) = event {
                    self.publish(event);
                }
                Ok(result)
            }
            Err(error) => {
                let error_text = error.to_string();
                match self
                    .store
                    .complete_publication(
                        &publication_run.publication_id,
                        "failed",
                        None,
                        Some(&error_text),
                    )
                    .await
                {
                    Ok((_, Some(event))) => self.publish(event),
                    Ok((_, None)) => {}
                    Err(storage_error) => error!(
                        publication_id = %publication_run.publication_id,
                        %storage_error,
                        "failed to persist publication failure"
                    ),
                }
                Err(error)
            }
        }
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn generate_publication(
        &self,
        publication_id: &str,
        project_id: &str,
        allow_partial: bool,
        snapshot: &ProjectSnapshot,
        packet: &Value,
        packet_bytes: &[u8],
        writer_root: &std::path::Path,
    ) -> CoreResult<PublicationResult> {
        let mut artifact_ids = vec![
            self.persist_publication_file(
                project_id,
                snapshot.project.current_round,
                writer_root,
                "paper_source_packet",
                "source_packet.json",
                packet_bytes,
            )
            .await?,
        ];
        let gaps = publication::publication_gaps(snapshot, allow_partial);
        if !gaps.is_empty() {
            let gap_text = format!("# Evidence gaps\n\n- {}\n", gaps.join("\n- "));
            artifact_ids.push(
                self.persist_publication_file(
                    project_id,
                    snapshot.project.current_round,
                    writer_root,
                    "paper_evidence_gaps",
                    "evidence_gaps.md",
                    gap_text.as_bytes(),
                )
                .await?,
            );
            let manifest = serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1,
                "project_id": project_id,
                "status": "blocked_by_evidence",
                "allow_partial": allow_partial,
                "evidence_gaps": gaps,
                "source_packet_sha256": hex::encode(Sha256::digest(packet_bytes)),
            }))?;
            artifact_ids.push(
                self.persist_publication_file(
                    project_id,
                    snapshot.project.current_round,
                    writer_root,
                    "paper_manifest",
                    "publication_manifest.json",
                    &manifest,
                )
                .await?,
            );
            let result = PublicationResult {
                publication_id: publication_id.into(),
                project_id: project_id.into(),
                status: "blocked_by_evidence".into(),
                output_directory: writer_root.to_string_lossy().into_owned(),
                evidence_gaps: gaps,
                artifact_ids,
                cited_keys: Vec::new(),
            };
            return Ok(result);
        }

        let handle = self
            .backend
            .create(AgentSpec {
                project_id: project_id.into(),
                role: "paper_writer".into(),
                model: self.config.model.clone(),
                working_directory: self
                    .config
                    .runtime_root
                    .join(project_id)
                    .join("paper_writer"),
            })
            .await?;
        let result = self
            .run_agent_counted(CountedAgentRun {
                handle: &handle,
                task: AgentTask {
                    kind: AgentTaskKind::Worker,
                    prompt: publication::writer_prompt(packet, allow_partial)?,
                    output_schema: paper_writer_schema(),
                    timeout_seconds: self.config.worker_timeout_seconds,
                },
                project: &snapshot.project,
                round: snapshot.project.current_round,
                worker_id: None,
                task_id: None,
                cancellation: CancellationToken::new(),
            })
            .await?;
        let output: PaperWriterOutput = serde_json::from_value(result.structured_output)
            .map_err(|error| CoreError::InvalidAgentOutput(format!("paper writer: {error}")))?;
        let cited_keys = publication::validate_writer_output(&output, packet)
            .map_err(CoreError::InvalidAgentOutput)?;
        for (kind, filename, content) in [
            (
                "paper_plan",
                "article_plan.md",
                output.article_plan_md.as_bytes(),
            ),
            (
                "paper_claim_ledger",
                "claim_evidence_ledger.md",
                output.claim_evidence_ledger_md.as_bytes(),
            ),
            (
                "paper_related_work",
                "related_work.tex",
                output.related_work_tex.as_bytes(),
            ),
            (
                "paper_candidate",
                "article_candidate.tex",
                output.article_candidate_tex.as_bytes(),
            ),
            (
                "paper_revision_notes",
                "revision_notes.md",
                output.revision_notes_md.as_bytes(),
            ),
            (
                "paper_evidence_gaps",
                "evidence_gaps.md",
                output.evidence_gaps_md.as_bytes(),
            ),
        ] {
            artifact_ids.push(
                self.persist_publication_file(
                    project_id,
                    snapshot.project.current_round,
                    writer_root,
                    kind,
                    filename,
                    content,
                )
                .await?,
            );
        }
        if output.status == "ready" {
            artifact_ids.extend(
                self.compile_and_validate_publication_pdf(
                    project_id,
                    snapshot.project.current_round,
                    writer_root,
                )
                .await?,
            );
        }
        let status = output.status;
        let manifest = serde_json::to_vec_pretty(&serde_json::json!({
            "version": 1,
            "project_id": project_id,
            "status": status,
            "allow_partial": allow_partial,
            "active_fact_ids": snapshot.facts.iter().filter(|fact| fact.status.to_string()=="active").map(|fact| &fact.fact_id).collect::<Vec<_>>(),
            "admitted_source_ids": snapshot.sources.iter().filter(|source| source.status=="admitted").map(|source| &source.source_id).collect::<Vec<_>>(),
            "cited_keys": cited_keys,
            "source_packet_sha256": hex::encode(Sha256::digest(packet_bytes)),
        }))?;
        artifact_ids.push(
            self.persist_publication_file(
                project_id,
                snapshot.project.current_round,
                writer_root,
                "paper_manifest",
                "publication_manifest.json",
                &manifest,
            )
            .await?,
        );
        let result = PublicationResult {
            publication_id: publication_id.into(),
            project_id: project_id.into(),
            status,
            output_directory: writer_root.to_string_lossy().into_owned(),
            evidence_gaps: if output.evidence_gaps_md.trim().is_empty() {
                Vec::new()
            } else {
                vec![output.evidence_gaps_md]
            },
            artifact_ids,
            cited_keys,
        };
        Ok(result)
    }

    #[allow(clippy::too_many_lines)]
    async fn compile_and_validate_publication_pdf(
        &self,
        project_id: &str,
        round: i64,
        writer_root: &std::path::Path,
    ) -> CoreResult<Vec<String>> {
        let compile = Command::new("latexmk")
            .args([
                "-pdf",
                "-interaction=nonstopmode",
                "-halt-on-error",
                "-file-line-error",
                "-no-shell-escape",
                "article_candidate.tex",
            ])
            .current_dir(writer_root)
            .output()
            .await?;
        let compile_log = format!(
            "exit_status: {}\n\nstdout:\n{}\n\nstderr:\n{}",
            compile.status,
            String::from_utf8_lossy(&compile.stdout),
            String::from_utf8_lossy(&compile.stderr)
        );
        let mut artifact_ids = vec![
            self.persist_publication_file(
                project_id,
                round,
                writer_root,
                "paper_compile_log",
                "latexmk.log.txt",
                compile_log.as_bytes(),
            )
            .await?,
        ];
        if !compile.status.success() {
            return Err(CoreError::InvalidAgentOutput(
                "LaTeX quality gate failed; see latexmk.log.txt".into(),
            ));
        }
        let engine_log = tokio::fs::read_to_string(writer_root.join("article_candidate.log"))
            .await
            .unwrap_or_default();
        let lower_log = engine_log.to_ascii_lowercase();
        if lower_log.contains("undefined references")
            || (lower_log.contains("citation") && lower_log.contains("undefined"))
        {
            return Err(CoreError::InvalidAgentOutput(
                "LaTeX quality gate found unresolved references or citations".into(),
            ));
        }
        let pdf_path = writer_root.join("article_candidate.pdf");
        let pdf_bytes = tokio::fs::read(&pdf_path).await?;
        if pdf_bytes.len() < 5 || &pdf_bytes[..5] != b"%PDF-" {
            return Err(CoreError::InvalidAgentOutput(
                "LaTeX quality gate did not produce a valid PDF header".into(),
            ));
        }
        artifact_ids.push(
            self.persist_publication_file(
                project_id,
                round,
                writer_root,
                "paper_pdf",
                "article_candidate.pdf",
                &pdf_bytes,
            )
            .await?,
        );

        let info = Command::new("pdfinfo")
            .arg("article_candidate.pdf")
            .current_dir(writer_root)
            .output()
            .await?;
        if !info.status.success() {
            return Err(CoreError::InvalidAgentOutput(
                "pdfinfo quality check failed".into(),
            ));
        }
        let info_text = String::from_utf8_lossy(&info.stdout).into_owned();
        let pages = info_text
            .lines()
            .find_map(|line| {
                line.strip_prefix("Pages:")
                    .and_then(|value| value.trim().parse::<usize>().ok())
            })
            .filter(|pages| *pages > 0)
            .ok_or_else(|| {
                CoreError::InvalidAgentOutput("PDF quality gate found no pages".into())
            })?;

        let text_output = Command::new("pdftotext")
            .args(["article_candidate.pdf", "-"])
            .current_dir(writer_root)
            .output()
            .await?;
        if !text_output.status.success()
            || String::from_utf8_lossy(&text_output.stdout)
                .trim()
                .is_empty()
        {
            return Err(CoreError::InvalidAgentOutput(
                "PDF quality gate found no extractable text".into(),
            ));
        }
        artifact_ids.push(
            self.persist_publication_file(
                project_id,
                round,
                writer_root,
                "paper_extracted_text",
                "article_candidate.txt",
                &text_output.stdout,
            )
            .await?,
        );

        let render = Command::new("pdftoppm")
            .args([
                "-png",
                "-r",
                "120",
                "article_candidate.pdf",
                "article_candidate_preview",
            ])
            .current_dir(writer_root)
            .output()
            .await?;
        if !render.status.success() {
            return Err(CoreError::InvalidAgentOutput(
                "PDF rendering quality check failed".into(),
            ));
        }
        let mut preview_paths = Vec::new();
        let mut entries = tokio::fs::read_dir(writer_root).await?;
        while let Some(entry) = entries.next_entry().await? {
            let filename = entry.file_name().to_string_lossy().into_owned();
            if filename.starts_with("article_candidate_preview-")
                && std::path::Path::new(&filename)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
            {
                preview_paths.push((filename, entry.path()));
            }
        }
        preview_paths.sort_by(|left, right| left.0.cmp(&right.0));
        if preview_paths.len() != pages {
            return Err(CoreError::InvalidAgentOutput(format!(
                "PDF rendering produced {} previews for {pages} pages",
                preview_paths.len()
            )));
        }
        for (filename, path) in &preview_paths {
            let bytes = tokio::fs::read(path).await?;
            artifact_ids.push(
                self.persist_publication_file(
                    project_id,
                    round,
                    writer_root,
                    "paper_page_preview",
                    filename,
                    &bytes,
                )
                .await?,
            );
        }
        let qa = serde_json::to_vec_pretty(&serde_json::json!({
            "version": 1,
            "latex_shell_escape": false,
            "compile_exit_status": compile.status.code(),
            "pdf_header_valid": true,
            "pages": pages,
            "extractable_text": true,
            "rendered_page_count": preview_paths.len(),
            "unresolved_references_or_citations": false,
        }))?;
        artifact_ids.push(
            self.persist_publication_file(
                project_id,
                round,
                writer_root,
                "paper_pdf_quality_report",
                "pdf_quality_report.json",
                &qa,
            )
            .await?,
        );
        Ok(artifact_ids)
    }

    async fn persist_publication_file(
        &self,
        project_id: &str,
        round: i64,
        writer_root: &std::path::Path,
        kind: &str,
        filename: &str,
        content: &[u8],
    ) -> CoreResult<String> {
        tokio::fs::write(writer_root.join(filename), content).await?;
        let (artifact, event) = self
            .store
            .store_artifact(project_id, kind, filename, content, round, Vec::new())
            .await?;
        self.publish(event);
        Ok(artifact.artifact_id)
    }

    #[must_use]
    pub fn verification_backend_name(&self) -> Option<&'static str> {
        self.verification_backend
            .as_ref()
            .map(|backend| backend.name())
    }

    pub async fn verification_backend_version(&self) -> CoreResult<Option<String>> {
        match &self.verification_backend {
            Some(backend) => Ok(Some(backend.version().await?)),
            None => Ok(None),
        }
    }

    #[must_use]
    pub fn interactive_proof_backend_name(&self) -> Option<&'static str> {
        self.interactive_proof_backend
            .as_ref()
            .map(|backend| backend.name())
    }

    pub async fn interactive_proof_backend_version(&self) -> CoreResult<Option<String>> {
        match &self.interactive_proof_backend {
            Some(backend) => Ok(Some(backend.version().await?)),
            None => Ok(None),
        }
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<DomainEvent> {
        self.event_bus.subscribe()
    }

    pub async fn create_project(
        &self,
        name: String,
        contract: ProblemContract,
        budget: Budget,
    ) -> CoreResult<Project> {
        self.create_project_with_route_approval(name, contract, budget, false)
            .await
    }

    pub async fn create_project_with_route_approval(
        &self,
        name: String,
        contract: ProblemContract,
        budget: Budget,
        human_route_approval: bool,
    ) -> CoreResult<Project> {
        let (project, event) = self
            .store
            .create_project_with_route_approval(name, contract, budget, human_route_approval)
            .await?;
        self.publish(event);
        let initial = reporting::initial_report(&project);
        self.write_latest_files(&project.project_id, 0, &initial)
            .await?;
        let (_, artifact_event) = self
            .store
            .store_artifact(
                &project.project_id,
                "latest_report",
                "LATEST.md",
                initial.as_bytes(),
                0,
                vec![project.project_id.clone()],
            )
            .await?;
        self.publish(artifact_event);
        Ok(self.store.get_project(&project.project_id).await?)
    }

    pub async fn submit_command(
        &self,
        project_id: &str,
        draft: CommandDraft,
    ) -> CoreResult<HumanCommand> {
        let (command, event) = self.store.enqueue_command(project_id, draft).await?;
        if let Some(event) = event {
            self.publish(event);
            let service = self.clone();
            let project_id = project_id.to_owned();
            let command_id = command.command_id.clone();
            tokio::spawn(async move {
                if let Err(error) = service.dispatch_command(&project_id, &command_id).await {
                    error!(%project_id, %command_id, %error, "command dispatch failed");
                    if let Ok((_, event)) = service
                        .store
                        .fail_command(&project_id, &command_id, &error.to_string())
                        .await
                    {
                        service.publish(event);
                    }
                }
            });
        }
        Ok(command)
    }

    pub async fn dispatch_command(
        &self,
        project_id: &str,
        command_id: &str,
    ) -> CoreResult<HumanCommand> {
        let before = self.store.get_command(project_id, command_id).await?;
        let (command, events) = self.store.apply_command(project_id, command_id).await?;
        self.publish_all(events);
        self.cancel_affected(&command).await;
        if command.status == CommandStatus::Applied
            && matches!(
                before.command_type.as_str(),
                "start_project"
                    | "resume_project"
                    | "trigger_replan"
                    | "stop_route"
                    | "create_task"
                    | "resume_task"
                    | "reassign_task"
            )
        {
            let service = self.clone();
            let project_id = project_id.to_owned();
            tokio::spawn(async move {
                if let Err(error) = Box::pin(service.run_project_until_terminal(&project_id)).await
                {
                    error!(%project_id, %error, "research loop stopped with error");
                }
            });
        }
        Ok(command)
    }

    pub async fn submit_candidate(
        &self,
        project_id: &str,
        submission: CandidateSubmission,
        idempotency_key: &str,
    ) -> CoreResult<SubmissionReceipt> {
        let receipt = self
            .store
            .submit_candidate(project_id, submission, idempotency_key)
            .await?;
        let should_verify = receipt.event.is_some()
            || matches!(
                receipt.verification.status,
                research_domain::CandidateStatus::Submitted
                    | research_domain::CandidateStatus::Verifying
            );
        if let Some(event) = receipt.event.clone() {
            self.publish(event);
        }
        if should_verify {
            let service = self.clone();
            let verification_id = receipt.verification.verification_id.clone();
            tokio::spawn(async move {
                if let Err(error) = Box::pin(service.verify_candidate(&verification_id)).await {
                    error!(%verification_id, %error, "candidate verification failed");
                }
            });
        }
        Ok(receipt)
    }

    pub async fn govern_fact(
        &self,
        fact_id: &str,
        action: &str,
        reason: &str,
        requested_by: &str,
        idempotency_key: &str,
    ) -> CoreResult<research_domain::FactGovernanceResult> {
        let (result, events) = self
            .store
            .govern_fact(fact_id, action, reason, requested_by, idempotency_key)
            .await?;
        let should_verify = !events.is_empty();
        self.publish_all(events);
        if should_verify && let Some(verification_id) = result.verification_id.clone() {
            let service = self.clone();
            tokio::spawn(async move {
                if let Err(error) = Box::pin(service.verify_candidate(&verification_id)).await {
                    error!(%verification_id, %error, "fact reverification failed");
                }
            });
        }
        Ok(result)
    }

    pub async fn import_catalog_fact(
        &self,
        project_id: &str,
        content_hash: &str,
        imported_by: &str,
    ) -> CoreResult<research_domain::ProjectFactImport> {
        let (import, event) = self
            .store
            .import_catalog_fact(project_id, content_hash, imported_by)
            .await?;
        if let Some(event) = event {
            self.publish(event);
        }
        Ok(import)
    }

    pub async fn lease_next_distributed_task(
        &self,
        project_id: &str,
        node_id: &str,
        token: &str,
        node_epoch: i64,
        ttl_seconds: u64,
    ) -> CoreResult<Option<(research_domain::TaskLease, Task)>> {
        let (leased, events) = self
            .store
            .lease_next_task(project_id, node_id, token, node_epoch, ttl_seconds)
            .await?;
        self.publish_all(events);
        Ok(leased)
    }

    pub async fn complete_distributed_task(
        &self,
        lease_id: &str,
        node_id: &str,
        token: &str,
        node_epoch: i64,
        lease_epoch: i64,
        output: &WorkerOutput,
    ) -> CoreResult<()> {
        let lease = self.store.get_task_lease(lease_id).await?;
        let events = self
            .store
            .complete_task_lease(lease_id, node_id, token, node_epoch, lease_epoch, output)
            .await?;
        self.publish_all(events);
        let completed_task = self
            .store
            .get_task(&lease.project_id, &lease.task_id)
            .await?;
        Box::pin(self.submit_worker_candidates(&completed_task, output.candidates.clone())).await?;
        Ok(())
    }

    pub async fn run_project_until_terminal(&self, project_id: &str) -> CoreResult<()> {
        let project_lock = {
            let mut locks = self.project_locks.lock().await;
            locks
                .entry(project_id.to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = project_lock.lock().await;
        let _ = Box::pin(self.recover_project_state(project_id)).await?;
        let project = self.store.get_project(project_id).await?;
        if project.status != ProjectStatus::Running {
            return Ok(());
        }
        if self.store.total_model_calls(project_id).await?
            >= i64::from(project.budget.max_total_model_calls)
        {
            self.publish(self.store.mark_budget_exhausted(project_id).await?);
            return Ok(());
        }
        for verification in self.store.list_verifications(project_id).await? {
            if verification.status == research_domain::CandidateStatus::Submitted {
                Box::pin(self.verify_candidate(&verification.verification_id)).await?;
            }
        }
        loop {
            let project = self.store.get_project(project_id).await?;
            if project.status != ProjectStatus::Running {
                return Ok(());
            }
            if project.current_round >= i64::from(project.budget.max_rounds) {
                self.publish(self.store.mark_budget_exhausted(project_id).await?);
                return Ok(());
            }
            if self.store.total_model_calls(project_id).await?
                >= i64::from(project.budget.max_total_model_calls)
            {
                self.publish(self.store.mark_budget_exhausted(project_id).await?);
                return Ok(());
            }
            self.run_round_unlocked(project_id).await?;
        }
    }

    pub async fn run_one_round(&self, project_id: &str) -> CoreResult<()> {
        let project_lock = {
            let mut locks = self.project_locks.lock().await;
            locks
                .entry(project_id.to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = project_lock.lock().await;
        let resumed_interrupted_round = Box::pin(self.recover_project_state(project_id)).await?;
        let project = self.store.get_project(project_id).await?;
        if self.store.total_model_calls(project_id).await?
            >= i64::from(project.budget.max_total_model_calls)
        {
            self.publish(self.store.mark_budget_exhausted(project_id).await?);
            return Ok(());
        }
        if resumed_interrupted_round {
            return Ok(());
        }
        self.run_round_unlocked(project_id).await?;
        let project = self.store.get_project(project_id).await?;
        if project.status == ProjectStatus::Running
            && self.store.total_model_calls(project_id).await?
                >= i64::from(project.budget.max_total_model_calls)
        {
            self.publish(self.store.mark_budget_exhausted(project_id).await?);
        }
        Ok(())
    }

    async fn recover_project_state(&self, project_id: &str) -> CoreResult<bool> {
        for envelope_id in self.store.pending_result_envelopes(project_id).await? {
            match self.store.ingest_local_result_envelope(&envelope_id).await {
                Ok((output, events)) => {
                    let envelope_task_id = events.iter().find_map(|event| {
                        event
                            .data
                            .get("task_id")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned)
                    });
                    self.publish_all(events);
                    if let Some(task_id) = envelope_task_id {
                        let task = self.store.get_task(project_id, &task_id).await?;
                        Box::pin(self.submit_worker_candidates(&task, output.candidates)).await?;
                    }
                }
                Err(StorageError::LateSubmission(reason)) => {
                    warn!(%project_id, %envelope_id, %reason, "startup reconciler rejected a stale result envelope");
                }
                Err(error) => return Err(error.into()),
            }
        }
        let _ = self
            .store
            .run_reconciliation(Some(project_id), "service_startup")
            .await?;
        self.publish_all(self.store.recover_project(project_id).await?);
        for verification in self.store.list_verifications(project_id).await? {
            if verification.status == research_domain::CandidateStatus::Submitted {
                Box::pin(self.verify_candidate(&verification.verification_id)).await?;
            }
        }
        self.resume_interrupted_v2_round(project_id).await
    }

    async fn resume_interrupted_v2_round(&self, project_id: &str) -> CoreResult<bool> {
        let Some(round) = self.store.current_round(project_id).await? else {
            return Ok(false);
        };
        if matches!(
            round.status,
            research_domain::RoundStatus::Completed
                | research_domain::RoundStatus::Failed
                | research_domain::RoundStatus::Interrupted
        ) {
            return Ok(false);
        }
        let Some(plan_revision) = self
            .store
            .list_plan_revisions(project_id)
            .await?
            .into_iter()
            .find(|revision| {
                revision.round_id.as_deref() == Some(round.round_id.as_str())
                    && revision.status == "committed"
            })
        else {
            return Ok(false);
        };
        let project = self.store.get_project(project_id).await?;
        let executable_tasks = self
            .store
            .list_tasks(project_id)
            .await?
            .into_iter()
            .filter(|task| task.round == round.number && task.status == TaskStatus::Queued)
            .collect::<Vec<_>>();
        let max_parallel = usize::try_from(project.budget.max_parallel_workers.max(1)).unwrap_or(1);
        let results: Vec<CoreResult<()>> = stream::iter(executable_tasks.into_iter().map(|task| {
            let service = self.clone();
            async move { Box::pin(service.execute_task(task)).await }
        }))
        .buffer_unordered(max_parallel)
        .collect()
        .await;
        for result in results {
            if let Err(error) = result {
                warn!(%project_id, %error, "recovered V2 task ended without a trusted result");
            }
        }
        self.publish_all(self.store.compress_failures(project_id, 3).await?);
        let plan_view = PlannerOutput {
            rationale_summary: format!(
                "Recovered committed plan revision {}: {}",
                plan_revision.plan_revision_id, plan_revision.rationale
            ),
            routes: Vec::new(),
            assignments: Vec::new(),
            targeted_uncertainty_ids: Vec::new(),
            suggestion_decisions: Vec::new(),
        };
        let latest_snapshot = self.store.snapshot(project_id).await?;
        let report = reporting::round_report(&latest_snapshot, &plan_view);
        self.write_latest_files(project_id, round.number, &report)
            .await?;
        for (kind, filename) in [
            (
                "round_report",
                format!("round_{:03}_recovered_summary.md", round.number),
            ),
            ("latest_report", "LATEST.md".into()),
        ] {
            let (_, event) = self
                .store
                .store_artifact(
                    project_id,
                    kind,
                    &filename,
                    report.as_bytes(),
                    round.number,
                    vec![
                        round.round_id.clone(),
                        plan_revision.plan_revision_id.clone(),
                    ],
                )
                .await?;
            self.publish(event);
        }
        self.publish_all(
            self.store
                .complete_round(
                    project_id,
                    &round.round_id,
                    &format!(
                        "Recovered and completed committed plan revision {}",
                        plan_revision.plan_revision_id
                    ),
                )
                .await?,
        );
        Ok(true)
    }

    #[allow(clippy::too_many_lines)]
    async fn run_round_unlocked(&self, project_id: &str) -> CoreResult<()> {
        let (round, event) = self.store.begin_round(project_id).await?;
        self.publish(event);
        let snapshot = self.store.snapshot(project_id).await?;
        let delta = self.store.collect_research_delta(project_id).await?;
        let bottlenecks = self.store.list_bottlenecks(project_id).await?;
        let failure_patterns = self.store.list_failure_patterns(project_id).await?;
        let suggestions = self.store.pending_suggestions(project_id).await?;
        let route_proposals = self.store.list_human_route_proposals(project_id).await?;
        let planning_context = planning_context(
            &snapshot,
            &delta,
            &bottlenecks,
            &failure_patterns,
            &suggestions,
            &route_proposals,
        );
        let (planner_packet, packet_event) = self
            .store
            .create_planner_context_packet(
                project_id,
                &round.round_id,
                snapshot.project_revision,
                &planning_context,
            )
            .await?;
        self.publish(packet_event);
        let planner_permitted = self.store.planner_call_permitted(project_id).await?;
        let planner_result = if planner_permitted {
            self.call_planner(&round, &snapshot, &planning_context)
                .await
        } else {
            Err(CoreError::InvalidAgentOutput(
                "planner circuit breaker is open".into(),
            ))
        };
        let (plan, planner_mode, fallback_reason) = match planner_result {
            Ok(plan) if primary_plan_is_executable(&plan) => (plan, "primary", None),
            Ok(_) => {
                let reason =
                    "planner returned no policy-eligible route or no assignment".to_owned();
                (
                    continuity_plan(&snapshot, &bottlenecks, &reason)
                        .unwrap_or_else(|| degraded_waiting_plan(&reason)),
                    "deterministic_continuity",
                    Some(reason),
                )
            }
            Err(error) => {
                warn!(%project_id, %error, "planner failed; entering bounded continuity mode");
                let reason = error.to_string();
                (
                    continuity_plan(&snapshot, &bottlenecks, &reason)
                        .unwrap_or_else(|| degraded_waiting_plan(&reason)),
                    "deterministic_continuity",
                    Some(reason),
                )
            }
        };
        if let Some(reason) = &fallback_reason {
            self.record_planning_stage(
                &round,
                "deterministic_continuity",
                &serde_json::json!({"planning_context":planning_context,"failure":reason}),
                &serde_json::to_value(&plan)?,
            )
            .await?;
        }
        let saved = self
            .store
            .save_plan_v2(
                project_id,
                &round,
                &plan,
                &delta,
                planner_mode,
                Some(&planner_packet.context_packet_id),
            )
            .await?;
        self.publish_all(saved.events);
        if planner_mode == "primary" {
            self.publish(self.store.record_planner_success(project_id).await?);
        } else if let Some(reason) = &fallback_reason {
            self.publish(
                self.store
                    .record_planner_failure(project_id, reason)
                    .await?,
            );
        }
        let mut executable_tasks = saved.tasks;
        let mut known_task_ids = executable_tasks
            .iter()
            .map(|task| task.task_id.clone())
            .collect::<HashSet<_>>();
        for task in self.store.list_tasks(project_id).await? {
            if task.status == TaskStatus::Queued && known_task_ids.insert(task.task_id.clone()) {
                executable_tasks.push(task);
            }
        }
        if executable_tasks.is_empty() && planner_mode != "primary" {
            self.publish(
                self.store
                    .record_planner_degraded_waiting(
                        project_id,
                        fallback_reason
                            .as_deref()
                            .unwrap_or("no legal narrow continuation task"),
                    )
                    .await?,
            );
        }
        let max_parallel =
            usize::try_from(snapshot.project.budget.max_parallel_workers.max(1)).unwrap_or(1);
        let results: Vec<CoreResult<()>> = stream::iter(executable_tasks.into_iter().map(|task| {
            let service = self.clone();
            async move { Box::pin(service.execute_task(task)).await }
        }))
        .buffer_unordered(max_parallel)
        .collect()
        .await;
        for result in results {
            if let Err(error) = result {
                warn!(%project_id, %error, "worker task ended without a trusted result");
            }
        }
        self.publish_all(self.store.compress_failures(project_id, 3).await?);
        let latest_snapshot = self.store.snapshot(project_id).await?;
        let report = reporting::round_report(&latest_snapshot, &plan);
        self.write_latest_files(project_id, round.number, &report)
            .await?;
        for (kind, filename) in [
            (
                "round_report",
                format!("round_{:03}_summary.md", round.number),
            ),
            ("latest_report", "LATEST.md".into()),
        ] {
            let (_, event) = self
                .store
                .store_artifact(
                    project_id,
                    kind,
                    &filename,
                    report.as_bytes(),
                    round.number,
                    vec![round.round_id.clone()],
                )
                .await?;
            self.publish(event);
        }
        self.publish_all(
            self.store
                .complete_round(project_id, &round.round_id, &plan.rationale_summary)
                .await?,
        );
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn call_planner(
        &self,
        round: &research_domain::ResearchRound,
        snapshot: &ProjectSnapshot,
        planning_context: &Value,
    ) -> CoreResult<PlannerOutput> {
        let (strategy, strategy_context) = self
            .establish_strategy_state(round, snapshot, planning_context)
            .await?;
        let generator_output = self
            .run_or_resume_planning_stage(PlanningStageRequest {
                round,
                snapshot,
                stage: "route_generator",
                role: "route_generator",
                input: &strategy_context,
                prompt: route_generator_delta_prompt(&strategy_context)?,
                output_schema: route_generator_schema(),
            })
            .await?;
        let generator: RouteGeneratorOutput = serde_json::from_value(generator_output)
            .map_err(|error| CoreError::InvalidAgentOutput(format!("route generator: {error}")))?;
        if generator.routes.len() < 2 {
            return Err(CoreError::InvalidAgentOutput(
                "route generator returned fewer than two routes".into(),
            ));
        }
        let reflection_input = serde_json::json!({
            "strategy_state":strategy_context["strategy_state"],
            "generator":generator,
        });
        let reflection_output = self
            .run_or_resume_planning_stage(PlanningStageRequest {
                round,
                snapshot,
                stage: "reflection",
                role: "route_reflection",
                input: &reflection_input,
                prompt: reflection_delta_prompt(&strategy_context, &generator.routes)?,
                output_schema: reflection_schema(),
            })
            .await?;
        let mut reflection: ReflectionOutput = serde_json::from_value(reflection_output)
            .map_err(|error| CoreError::InvalidAgentOutput(format!("reflection: {error}")))?;
        validate_reflection(&reflection, generator.routes.len())?;

        let active_fact_ids = snapshot
            .facts
            .iter()
            .filter(|fact| fact.status.to_string() == "active")
            .map(|fact| fact.fact_id.as_str())
            .collect::<HashSet<_>>();
        let mut routes = Vec::new();
        let mut remapped_reviews = Vec::new();
        for (index, mut route) in generator.routes.into_iter().enumerate() {
            let review = reflection
                .reviews
                .iter()
                .find(|review| review.route_index == index)
                .expect("reflection indices validated");
            let dependencies_valid = route
                .required_fact_ids
                .iter()
                .all(|fact_id| active_fact_ids.contains(fact_id.as_str()));
            if !reflection_route_is_policy_eligible(review, dependencies_valid) {
                continue;
            }
            route.failure_similarity_penalty += if review.repeats_failure_pattern {
                0.25
            } else {
                0.0
            };
            route.cost_penalty += review.risk_score * 0.10;
            let mut review = review.clone();
            review.route_index = routes.len();
            remapped_reviews.push(review);
            routes.push(route);
        }
        if routes.is_empty() {
            return Err(CoreError::InvalidAgentOutput(
                "no policy-eligible route remains after reflection".into(),
            ));
        }
        reflection.reviews = remapped_reviews;
        let proximity = proximity_projection(&mut routes);
        self.record_planning_stage(
            round,
            "proximity",
            &serde_json::to_value(&routes)?,
            &proximity,
        )
        .await?;
        let ranking = rank_routes(&routes, &reflection, self.config.ranking_weights);
        self.record_planning_stage(
            round,
            "ranking",
            &serde_json::json!({"routes":routes,"reflection":reflection,"proximity":proximity,"weights":self.config.ranking_weights}),
            &ranking,
        )
        .await?;

        let supervisor_input = serde_json::json!({
            "strategy_state":strategy_context["strategy_state"],
            "ranking":ranking,
            "reflection":reflection,
        });
        let supervisor_output = self
            .run_or_resume_planning_stage(PlanningStageRequest {
                round,
                snapshot,
                stage: "supervisor",
                role: "planning_supervisor",
                input: &supervisor_input,
                prompt: supervisor_delta_prompt(&strategy_context, &ranking, &reflection)?,
                output_schema: supervisor_schema(),
            })
            .await?;
        let mut supervisor: SupervisorOutput = serde_json::from_value(supervisor_output)
            .map_err(|error| CoreError::InvalidAgentOutput(format!("supervisor: {error}")))?;
        supervisor
            .assignments
            .retain(|assignment| assignment.route_index < routes.len());
        supervisor.assignments.retain(|assignment| {
            !reflection.reviews[assignment.route_index].unjustified_narrowing
                || assignment.addresses_interface_debt
        });
        bind_strategy_to_assignments(&strategy, &mut supervisor);
        if !supervisor.assignments.iter().any(|assignment| {
            matches!(
                assignment.strategic_role.as_str(),
                "whole_architecture" | "central_bridge"
            )
        }) {
            return Err(CoreError::InvalidAgentOutput(
                "supervisor omitted a whole-architecture or central-bridge assignment".into(),
            ));
        }
        ensure_adversarial_assignment(&routes, &mut supervisor);
        if supervisor.assignments.is_empty() {
            return Err(CoreError::InvalidAgentOutput(
                "supervisor produced no policy-eligible assignments".into(),
            ));
        }
        Ok(PlannerOutput {
            rationale_summary: format!(
                "Strategy: {} | Generator: {} | Reflection: {} | Supervisor: {}",
                strategy.verdict_summary,
                generator.rationale_summary,
                reflection.summary,
                supervisor.rationale_summary
            ),
            routes,
            assignments: supervisor.assignments,
            targeted_uncertainty_ids: supervisor.targeted_uncertainty_ids,
            suggestion_decisions: supervisor.suggestion_decisions,
        })
    }

    #[allow(clippy::too_many_lines)]
    async fn establish_strategy_state(
        &self,
        round: &research_domain::ResearchRound,
        snapshot: &ProjectSnapshot,
        planning_context: &Value,
    ) -> CoreResult<(StrategyDirectorOutput, Value)> {
        let previous = self.store.latest_strategy_state(&round.project_id).await?;
        let states = self.store.list_strategy_states(&round.project_id).await?;
        let audit = strategy_audit_decision(previous.as_ref(), &states, planning_context);
        let audit_kind = audit.kind;
        let strategy_input = serde_json::json!({
            "audit_kind":audit_kind,
            "trigger_reasons":audit.reasons,
            "planning_context":planning_context,
            "previous_strategy_state":previous,
        });
        let generated = self
            .run_or_resume_planning_stage(PlanningStageRequest {
                round,
                snapshot,
                stage: "strategy_director",
                role: "strategy_director",
                input: &strategy_input,
                prompt: strategy_director_prompt(
                    planning_context,
                    strategy_input.get("previous_strategy_state"),
                    audit_kind,
                    &audit.reasons,
                )?,
                output_schema: strategy_director_schema(),
            })
            .await;
        let validated = generated.and_then(|output| {
            let strategy: StrategyDirectorOutput =
                serde_json::from_value(output).map_err(|error| {
                    CoreError::InvalidAgentOutput(format!("strategy director: {error}"))
                })?;
            if strategy.fixed_goal.trim() != snapshot.project.contract.target_statement.trim() {
                return Err(CoreError::InvalidAgentOutput(
                    "strategy director changed the fixed target statement".into(),
                ));
            }
            if strategy.proof_skeleton.is_empty()
                || strategy.central_missing_bridge.trim().is_empty()
            {
                return Err(CoreError::InvalidAgentOutput(
                    "strategy director omitted the proof skeleton or central bridge".into(),
                ));
            }
            Ok(strategy)
        });
        let strategy = match validated {
            Ok(strategy) => strategy,
            Err(error) => {
                warn!(project_id=%round.project_id, %error, "strategy director failed; preserving a conservative strategy state");
                let fallback = conservative_strategy_state(
                    snapshot,
                    planning_context,
                    strategy_input.get("previous_strategy_state"),
                    &error.to_string(),
                );
                self.record_planning_stage(
                    round,
                    "strategy_director_fallback",
                    &strategy_input,
                    &serde_json::to_value(&fallback)?,
                )
                .await?;
                fallback
            }
        };
        let input_hash = sha256_json(&strategy_input)?;
        let (persisted, event) = self
            .store
            .record_strategy_state(
                &round.project_id,
                &round.round_id,
                &input_hash,
                audit_kind,
                &audit.reasons,
                &strategy,
            )
            .await?;
        if let Some(event) = event {
            self.publish(event);
        }
        Ok((
            strategy,
            serde_json::json!({
                "research_context":planning_context,
                "strategy_state":persisted,
            }),
        ))
    }

    async fn run_or_resume_planning_stage(
        &self,
        request: PlanningStageRequest<'_>,
    ) -> CoreResult<Value> {
        let input_hash = sha256_json(request.input)?;
        if let Some(output) = self
            .store
            .list_planning_stages(&request.round.project_id, &request.round.round_id)
            .await?
            .into_iter()
            .find(|run| {
                run.get("stage").and_then(Value::as_str) == Some(request.stage)
                    && run.get("input_hash").and_then(Value::as_str) == Some(input_hash.as_str())
                    && run.get("status").and_then(Value::as_str) == Some("completed")
            })
            .and_then(|run| run.get("output").cloned())
        {
            return Ok(output);
        }
        let mut last_error = None;
        let mut completed = None;
        for attempt_number in 1..=2 {
            let (soft_timeout_seconds, hard_timeout_seconds) =
                planning_attempt_timeouts(self.config.planner_timeout_seconds, attempt_number);
            let (attempt_id, event) = self
                .store
                .begin_planning_stage_attempt(
                    &request.round.project_id,
                    &request.round.round_id,
                    request.stage,
                    &input_hash,
                    attempt_number,
                    (
                        i64::try_from(soft_timeout_seconds).unwrap_or(i64::MAX),
                        i64::try_from(hard_timeout_seconds).unwrap_or(i64::MAX),
                    ),
                )
                .await?;
            if let Some(event) = event {
                self.publish(event);
            }
            let run = self.run_planning_agent(
                request.snapshot,
                request.role,
                request.prompt.clone(),
                request.output_schema.clone(),
                hard_timeout_seconds,
            );
            let attempt_result = self
                .await_planning_attempt(&attempt_id, soft_timeout_seconds, run)
                .await;
            match attempt_result {
                Ok(result) => {
                    self.store
                        .complete_planning_stage_attempt(&attempt_id, true, None)
                        .await?;
                    completed = Some(result);
                    break;
                }
                Err(error) => {
                    self.store
                        .complete_planning_stage_attempt(
                            &attempt_id,
                            false,
                            Some(&error.to_string()),
                        )
                        .await?;
                    last_error = Some(error);
                }
            }
        }
        let result = completed.ok_or_else(|| {
            last_error.unwrap_or_else(|| {
                CoreError::InvalidAgentOutput("planning stage produced no attempt".into())
            })
        })?;
        self.record_planning_stage(
            request.round,
            request.stage,
            request.input,
            &result.structured_output,
        )
        .await?;
        Ok(result.structured_output)
    }

    async fn await_planning_attempt<F>(
        &self,
        attempt_id: &str,
        soft_timeout_seconds: u64,
        run: F,
    ) -> CoreResult<AgentRunResult>
    where
        F: std::future::Future<Output = CoreResult<AgentRunResult>>,
    {
        tokio::pin!(run);
        let soft_deadline =
            tokio::time::sleep(std::time::Duration::from_secs(soft_timeout_seconds));
        tokio::pin!(soft_deadline);
        let heartbeat_period = std::time::Duration::from_secs(30);
        let mut heartbeat = tokio::time::interval_at(
            tokio::time::Instant::now() + heartbeat_period,
            heartbeat_period,
        );
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut soft_budget_marked = false;
        loop {
            tokio::select! {
                result = &mut run => return result,
                () = &mut soft_deadline, if !soft_budget_marked => {
                    soft_budget_marked = true;
                    if let Some(event) = self.store.mark_planning_stage_soft_budget_exceeded(attempt_id).await? {
                        self.publish(event);
                    }
                }
                _ = heartbeat.tick() => {
                    self.store.heartbeat_planning_stage_attempt(attempt_id).await?;
                }
            }
        }
    }

    async fn run_planning_agent(
        &self,
        snapshot: &ProjectSnapshot,
        role: &str,
        prompt: String,
        output_schema: Value,
        timeout_seconds: u64,
    ) -> CoreResult<AgentRunResult> {
        let handle = self
            .backend
            .create(AgentSpec {
                project_id: snapshot.project.project_id.clone(),
                role: role.into(),
                model: self.config.model.clone(),
                working_directory: self
                    .config
                    .runtime_root
                    .join(&snapshot.project.project_id)
                    .join("planner")
                    .join(format!("round_{:03}", snapshot.project.current_round))
                    .join(role),
            })
            .await?;
        self.run_agent_counted(CountedAgentRun {
            handle: &handle,
            task: AgentTask {
                kind: AgentTaskKind::Planner,
                prompt,
                output_schema,
                timeout_seconds,
            },
            project: &snapshot.project,
            round: snapshot.project.current_round,
            worker_id: None,
            task_id: None,
            cancellation: CancellationToken::new(),
        })
        .await
    }

    async fn record_planning_stage(
        &self,
        round: &research_domain::ResearchRound,
        stage: &str,
        input: &Value,
        output: &Value,
    ) -> CoreResult<()> {
        let input_hash = sha256_json(input)?;
        if let Some(event) = self
            .store
            .record_planning_stage(
                &round.project_id,
                &round.round_id,
                stage,
                &input_hash,
                output,
            )
            .await?
        {
            self.publish(event);
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn execute_task(&self, task: Task) -> CoreResult<()> {
        let working_directory = self
            .config
            .runtime_root
            .join(&task.project_id)
            .join("workers")
            .join(&task.task_id);
        let offer = self
            .store
            .offer_local_task(
                &task,
                self.backend.name(),
                self.config.model.as_deref(),
                &working_directory.to_string_lossy(),
                serde_json::to_value(self.backend.capabilities())?,
            )
            .await?;
        self.publish_all(offer.events.clone());
        let handle = match self
            .backend
            .create(AgentSpec {
                project_id: task.project_id.clone(),
                role: task.worker_role.clone(),
                model: self.config.model.clone(),
                working_directory,
            })
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                self.publish_all(
                    self.store
                        .fail_local_attempt(None, &offer, &error.to_string())
                        .await?,
                );
                return Err(error.into());
            }
        };
        let lease = self
            .store
            .accept_local_handshake(
                &offer,
                None,
                serde_json::json!({
                    "handle_id":&handle.handle_id,
                    "session_id":&handle.session_id,
                    "role":&handle.role,
                    "context_hash":&offer.context_packet.content_hash,
                    "task_contract_hash":&offer.contract.content_hash,
                    "capabilities":self.backend.capabilities(),
                }),
                90,
            )
            .await?;
        self.publish_all(lease.events.clone());
        let checkpoint_payload = serde_json::json!({
            "checkpoint_kind":"worker_start_safe_point",
            "task_id":task.task_id,
            "attempt_id":lease.attempt_id,
            "lease_epoch":lease.lease_epoch,
            "task_contract_id":offer.contract.task_contract_id,
            "task_contract_hash":offer.contract.content_hash,
            "context_packet_id":offer.context_packet.context_packet_id,
            "context_packet_hash":offer.context_packet.content_hash,
            "resume_from":offer.resume_checkpoint,
            "note":"This checkpoint records resumable execution inputs and is not a mathematical premise."
        });
        let checkpoint_bytes = serde_json::to_vec_pretty(&checkpoint_payload)?;
        let (checkpoint_artifact, checkpoint_artifact_event) = self
            .store
            .store_artifact(
                &task.project_id,
                "task_checkpoint",
                &format!(
                    "{}-{}-start-checkpoint.json",
                    task.task_id, lease.attempt_id
                ),
                &checkpoint_bytes,
                task.round,
                vec![task.task_id.clone(), lease.attempt_id.clone()],
            )
            .await?;
        self.publish(checkpoint_artifact_event);
        self.publish(
            self.store
                .save_local_checkpoint(
                    &lease,
                    &checkpoint_artifact.artifact_id,
                    "worker handshake completed; immutable contract and context are resumable",
                )
                .await?,
        );
        let running_task = lease.task.clone();
        let cancellation = CancellationToken::new();
        self.task_cancellations
            .lock()
            .await
            .insert(task.task_id.clone(), cancellation.clone());
        let heartbeat_stop = CancellationToken::new();
        let heartbeat_handle = {
            let store = self.store.clone();
            let lease = lease.clone();
            let stop = heartbeat_stop.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        () = stop.cancelled() => break,
                        () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                            if store.heartbeat_local_lease(&lease, 90).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            })
        };
        let mut steering = Vec::new();
        let result = loop {
            let (pending, events) = self
                .store
                .consume_pending_task_steers(
                    &running_task.project_id,
                    &running_task.task_id,
                    running_task.revision,
                    running_task.route_cancellation_epoch,
                )
                .await?;
            self.publish_all(events);
            steering.extend(pending.into_iter().map(|steer| steer.content));
            let output = self
                .call_worker(
                    &handle,
                    &running_task,
                    &offer.contract,
                    &offer.context_packet,
                    offer.resume_checkpoint.as_ref(),
                    cancellation.clone(),
                    &steering,
                )
                .await;
            let output = match output {
                Ok(output) => output,
                Err(error) => break Err(error),
            };
            let (arrived_during_run, events) = self
                .store
                .consume_pending_task_steers(
                    &running_task.project_id,
                    &running_task.task_id,
                    running_task.revision,
                    running_task.route_cancellation_epoch,
                )
                .await?;
            self.publish_all(events);
            if arrived_during_run.is_empty() {
                break Ok(output);
            }
            let bytes = serde_json::to_vec_pretty(&output)?;
            let (_, event) = self
                .store
                .store_artifact(
                    &running_task.project_id,
                    "superseded_worker_output",
                    &format!("{}-pre-steer.json", running_task.task_id),
                    &bytes,
                    running_task.round,
                    vec![running_task.task_id.clone()],
                )
                .await?;
            self.publish(event);
            steering.extend(arrived_during_run.into_iter().map(|steer| steer.content));
        };
        heartbeat_stop.cancel();
        let _ = heartbeat_handle.await;
        self.task_cancellations.lock().await.remove(&task.task_id);
        let mut output = match result {
            Ok(output) => output,
            Err(error) => {
                self.publish_all(
                    self.store
                        .fail_local_attempt(Some(&lease), &offer, &error.to_string())
                        .await?,
                );
                return Err(error);
            }
        };
        if let Err(error) = self
            .archive_source_fulltexts(&handle, &running_task, &mut output)
            .await
        {
            self.publish_all(
                self.store
                    .fail_local_attempt(Some(&lease), &offer, &error.to_string())
                    .await?,
            );
            return Err(error);
        }
        let (result_envelope_id, events) = self
            .store
            .submit_local_result_envelope(&lease, &output)
            .await?;
        self.publish_all(events);
        let (output, events) = self
            .store
            .ingest_local_result_envelope(&result_envelope_id)
            .await?;
        self.publish_all(events);
        let completed_task = self
            .store
            .get_task(&running_task.project_id, &running_task.task_id)
            .await?;
        Box::pin(self.submit_worker_candidates(&completed_task, output.candidates)).await?;
        Ok(())
    }

    async fn archive_source_fulltexts(
        &self,
        handle: &AgentHandle,
        task: &Task,
        output: &mut WorkerOutput,
    ) -> CoreResult<()> {
        let root = if output
            .sources
            .iter()
            .any(|source| source.fulltext_path.is_some())
        {
            tokio::fs::canonicalize(&handle.working_directory)
                .await
                .ok()
        } else {
            None
        };
        let mut archival_failures = Vec::new();
        for source in &mut output.sources {
            if source.fulltext_artifact_id.take().is_some() {
                archival_failures.push(source_archival_failure(
                    source,
                    "worker_assigned_trusted_fulltext_artifact_id",
                ));
                continue;
            }
            if source.fulltext_path.is_none() {
                if source.fulltext_sha256.is_some() {
                    archival_failures.push(source_archival_failure(
                        source,
                        "fulltext_sha256_without_fulltext_path",
                    ));
                } else if task.worker_role == "literature_researcher" {
                    if let Some(url) = source.url.as_deref() {
                        match fetch_public_source_fulltext(url).await {
                            Ok((filename, bytes, actual_hash)) => {
                                let (artifact, event) = self
                                    .store
                                    .store_artifact(
                                        &task.project_id,
                                        "source_fulltext",
                                        &filename,
                                        &bytes,
                                        task.round,
                                        vec![task.task_id.clone()],
                                    )
                                    .await?;
                                self.publish(event);
                                source.fulltext_sha256 = Some(actual_hash);
                                source.fulltext_artifact_id = Some(artifact.artifact_id);
                                source.status = "reported_unverified".into();
                                source.applicability = format!(
                                    "{} Trusted runtime fetched the public HTTPS document; mathematical applicability remains unverified.",
                                    source.applicability.trim()
                                );
                                continue;
                            }
                            Err(reason) => archival_failures.push(source_archival_failure(
                                source,
                                format!("trusted_fulltext_fetch_failed:{reason}"),
                            )),
                        }
                    } else if source.status != "not_applicable" {
                        archival_failures.push(source_archival_failure(
                            source,
                            "reported_literature_source_missing_fulltext_and_url",
                        ));
                    }
                }
                continue;
            }
            let Some(root) = root.as_deref() else {
                archival_failures.push(source_archival_failure(
                    source,
                    "worker_directory_missing_or_unreadable",
                ));
                continue;
            };
            let (filename, bytes, actual_hash) = match load_source_fulltext(root, source).await {
                Ok(loaded) => loaded,
                Err(reason) => {
                    archival_failures.push(source_archival_failure(source, reason));
                    continue;
                }
            };
            let (artifact, event) = self
                .store
                .store_artifact(
                    &task.project_id,
                    "source_fulltext",
                    &filename,
                    &bytes,
                    task.round,
                    vec![task.task_id.clone()],
                )
                .await?;
            self.publish(event);
            source.fulltext_sha256 = Some(actual_hash);
            source.fulltext_artifact_id = Some(artifact.artifact_id);
        }
        output.failures.extend(archival_failures);
        Ok(())
    }

    async fn submit_worker_candidates(
        &self,
        completed_task: &Task,
        candidates: Vec<CandidateDraft>,
    ) -> CoreResult<()> {
        for (candidate_index, draft) in candidates.into_iter().enumerate() {
            let submission = CandidateSubmission {
                task_id: completed_task.task_id.clone(),
                route_id: completed_task.route_id.clone(),
                target_goal_ids: if draft.target_goal_ids.is_empty() {
                    completed_task.goal_ids.clone()
                } else {
                    draft.target_goal_ids
                },
                statement: draft.statement,
                assumptions: draft.assumptions,
                proof_markdown: draft.proof_markdown,
                dependency_fact_ids: draft.dependency_fact_ids,
                definitions_introduced: draft.definitions_introduced,
                external_source_ids: draft.external_source_ids,
                candidate_type: draft.candidate_type,
                task_revision: completed_task.revision,
                route_cancellation_epoch: completed_task.route_cancellation_epoch,
            };
            let idempotency_key = format!(
                "worker-{}-{}-{candidate_index}",
                completed_task.task_id, completed_task.revision
            );
            let receipt = self
                .store
                .submit_candidate(&completed_task.project_id, submission, &idempotency_key)
                .await?;
            let should_verify = receipt.event.is_some()
                || matches!(
                    receipt.verification.status,
                    research_domain::CandidateStatus::Submitted
                        | research_domain::CandidateStatus::Verifying
                );
            if let Some(event) = receipt.event {
                self.publish(event);
            }
            if should_verify {
                Box::pin(self.verify_candidate(&receipt.verification.verification_id)).await?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn call_worker(
        &self,
        handle: &AgentHandle,
        task: &Task,
        task_contract: &TaskContract,
        context_packet: &ContextPacket,
        resume_checkpoint: Option<&Value>,
        cancellation: CancellationToken,
        steering: &[String],
    ) -> CoreResult<WorkerOutput> {
        let project = self.store.get_project(&task.project_id).await?;
        let mut prompt = worker_packet_prompt(task, task_contract, context_packet)?;
        if let Some(checkpoint) = resume_checkpoint {
            prompt.push_str("\n\nA prior attempt left the following resumable checkpoint. Verify its artifact hash before using it. It is execution state, not a mathematical premise:\n");
            prompt.push_str(&serde_json::to_string_pretty(checkpoint)?);
        }
        if !steering.is_empty() {
            prompt.push_str("\n\nHuman steering applied at a safe point. Treat it as research guidance, not as a fact or proof:\n");
            for (index, item) in steering.iter().enumerate() {
                let _ = writeln!(prompt, "{}. {}", index + 1, item);
            }
        }
        let agent_task = AgentTask {
            kind: AgentTaskKind::Worker,
            prompt,
            output_schema: worker_schema(),
            timeout_seconds: self
                .config
                .worker_timeout_seconds
                .min(u64::from(project.budget.max_minutes_per_task).saturating_mul(60)),
        };
        let result = self
            .run_agent_counted(CountedAgentRun {
                handle,
                task: agent_task,
                project: &project,
                round: task.round,
                worker_id: task.worker_id.as_deref(),
                task_id: Some(&task.task_id),
                cancellation,
            })
            .await?;
        serde_json::from_value(result.structured_output).map_err(|error| {
            CoreError::InvalidAgentOutput(format!("worker {}: {error}", task.task_id))
        })
    }

    #[allow(clippy::too_many_lines)]
    pub async fn verify_candidate(&self, verification_id: &str) -> CoreResult<()> {
        let (verification, event) = self
            .store
            .mark_verification_started(verification_id)
            .await?;
        self.publish(event);
        let candidate = self.store.get_candidate(&verification.candidate_id).await?;
        let project = self.store.get_project(&verification.project_id).await?;
        let task = self
            .store
            .get_task(&verification.project_id, &candidate.submission.task_id)
            .await?;
        let target_goal_ids = candidate
            .submission
            .target_goal_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let target_goals = self
            .store
            .list_goals(&verification.project_id)
            .await?
            .into_iter()
            .filter(|goal| target_goal_ids.contains(goal.goal_id.as_str()))
            .collect::<Vec<_>>();
        let dependencies = self
            .load_dependency_closure(&candidate.submission.dependency_fact_ids)
            .await?;
        let available_sources = self.store.list_sources(&verification.project_id).await?;
        let sources = candidate
            .submission
            .external_source_ids
            .iter()
            .filter_map(|source_id| {
                available_sources
                    .iter()
                    .find(|source| source.source_id == *source_id)
                    .cloned()
            })
            .collect::<Vec<_>>();
        let governance_kind = self
            .store
            .verification_governance_kind(verification_id)
            .await?;
        let independent_proof_requested =
            governance_kind.as_deref() == Some("request_independent_proof");
        let formal_required = requires_formal_verification(&project, &candidate.submission)
            || matches!(
                governance_kind.as_deref(),
                Some("request_formalization" | "request_independent_proof")
            );
        let required_checks = required_checks(
            !sources.is_empty(),
            formal_required,
            independent_proof_requested,
        );
        let (profile, required_acceptance, policy_name) = if independent_proof_requested {
            (
                VerificationProfile::CriticalCertification,
                AcceptanceClass::FullyCertified,
                "independent_proof_certification_v1",
            )
        } else if formal_required {
            (
                VerificationProfile::CriticalCertification,
                AcceptanceClass::FullyCertified,
                "critical_certification_v2",
            )
        } else {
            (
                VerificationProfile::StandardReview,
                AcceptanceClass::Reviewed,
                "standard_review_v1",
            )
        };
        let (case, case_event) = self
            .store
            .ensure_verification_case(
                verification_id,
                VerificationCaseDraft {
                    name: policy_name.into(),
                    profile,
                    required_acceptance,
                    required_checks,
                    independent_reviewer_count: if independent_proof_requested { 3 } else { 2 },
                    require_citation_review: !sources.is_empty(),
                    require_adversarial_review: true,
                    require_alignment_review: formal_required,
                    require_fresh_replay: formal_required,
                    max_attempts: 3,
                    risk_score: v1_risk_score(&project, &candidate.submission),
                    risk_reasons: v1_risk_reasons(&project, &candidate.submission),
                },
            )
            .await?;
        if let Some(event) = case_event {
            self.publish(event);
        }
        let toolchain_hash = if formal_required {
            self.verification_toolchain_hash().await.ok()
        } else {
            None
        };
        if case.snapshot_id.is_none() {
            let (_, snapshot_event) = self
                .store
                .create_verification_snapshot(
                    &case.case_id,
                    VerificationSnapshotDraft {
                        toolchain_hash,
                        extra_payload: serde_json::json!({
                            "verification_layer": if formal_required { "v2" } else { "v1" },
                            "backend": self.backend.name(),
                            "governance_request": governance_kind,
                        }),
                    },
                )
                .await?;
            self.publish(snapshot_event);
        }

        if let Some(report) = deterministic_precheck(
            &candidate.submission,
            &task,
            &dependencies,
            &sources,
            &target_goals,
        ) {
            let (_, event) = self
                .store
                .record_verification_check(
                    &case.case_id,
                    CheckDraft {
                        attempt_id: None,
                        kind: "deterministic_precheck".into(),
                        status: CheckStatus::Failed,
                        mandatory: true,
                        summary: report.summary.clone(),
                        details: serde_json::to_value(&report)?,
                    },
                )
                .await?;
            self.publish(event);
            let commit = self
                .store
                .commit_verification(verification_id, report)
                .await?;
            self.publish_all(commit.events);
            return Ok(());
        }
        let (_, precheck_event) = self
            .store
            .record_verification_check(
                &case.case_id,
                CheckDraft {
                    attempt_id: None,
                    kind: "deterministic_precheck".into(),
                    status: CheckStatus::Passed,
                    mandatory: true,
                    summary: "结构、占位符与本地依赖预检通过".into(),
                    details: serde_json::json!({
                        "checked_fact_ids": candidate.submission.dependency_fact_ids,
                        "checked_source_ids": candidate.submission.external_source_ids,
                    }),
                },
            )
            .await?;
        self.publish(precheck_event);
        self.publish(
            self.store
                .transition_verification_case(
                    &case.case_id,
                    case.cancellation_epoch,
                    VerificationStage::Review,
                    None,
                )
                .await?,
        );

        let reviewer_kinds: Vec<String> = if sources.is_empty() && independent_proof_requested {
            vec![
                "math_review_1".into(),
                "math_review_2".into(),
                "math_review_3".into(),
                "adversarial_review".into(),
            ]
        } else if sources.is_empty() {
            vec![
                "math_review_1".into(),
                "math_review_2".into(),
                "adversarial_review".into(),
            ]
        } else if independent_proof_requested {
            vec![
                "math_review_1".into(),
                "math_review_2".into(),
                "math_review_3".into(),
                "citation_review".into(),
                "adversarial_review".into(),
            ]
        } else {
            vec![
                "math_review_1".into(),
                "math_review_2".into(),
                "citation_review".into(),
                "adversarial_review".into(),
            ]
        };
        let service = self;
        let project_ref = &project;
        let submission_ref = &candidate.submission;
        let dependencies_ref = dependencies.as_slice();
        let sources_ref = sources.as_slice();
        let target_goals_ref = target_goals.as_slice();
        let mut completed_reviews = stream::iter(reviewer_kinds.into_iter().enumerate().map(
            move |(index, reviewer_kind)| async move {
                let report = service
                    .run_reviewer(
                        &reviewer_kind,
                        verification_id,
                        ReviewerContext {
                            project: project_ref,
                            submission: submission_ref,
                            dependencies: dependencies_ref,
                            sources: sources_ref,
                            target_goals: target_goals_ref,
                        },
                    )
                    .await;
                (index, reviewer_kind, report)
            },
        ))
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;
        completed_reviews.sort_by_key(|(index, _, _)| *index);
        let mut reviews = Vec::with_capacity(completed_reviews.len() + 1);
        let mut reviewer_reports = Vec::with_capacity(completed_reviews.len());
        for (_, reviewer_kind, report) in completed_reviews {
            let check_status = verdict_check_status(report.verdict);
            let (check, event) = self
                .store
                .record_verification_check(
                    &case.case_id,
                    CheckDraft {
                        attempt_id: None,
                        kind: reviewer_kind.clone(),
                        status: check_status,
                        mandatory: true,
                        summary: report.summary.clone(),
                        details: serde_json::to_value(&report)?,
                    },
                )
                .await?;
            self.publish(event);
            self.record_review_details(&case.case_id, &reviewer_kind, &check.check_id, &report)
                .await?;
            reviewer_reports.push((reviewer_kind, report.clone()));
            reviews.push(report);
        }
        let independence = assess_reviewer_independence(&reviewer_reports);
        let (_, event) = self
            .store
            .record_verification_check(
                &case.case_id,
                CheckDraft {
                    attempt_id: None,
                    kind: "reviewer_independence".into(),
                    status: if independence.duplicate_groups.is_empty() {
                        CheckStatus::Passed
                    } else {
                        CheckStatus::Failed
                    },
                    mandatory: true,
                    summary: independence.summary.clone(),
                    details: serde_json::to_value(&independence)?,
                },
            )
            .await?;
        self.publish(event);
        if !independence.duplicate_groups.is_empty() {
            reviews.push(VerificationReport {
                verdict: VerificationVerdict::Unknown,
                summary: independence.summary,
                critical_errors: Vec::new(),
                gaps: Vec::new(),
                uncertainties: vec![
                    "two mathematical reviewers returned byte-equivalent normalized findings"
                        .into(),
                ],
                repair_actions: vec![
                    "rerun the duplicated reviewer roles with an independently varied audit contract"
                        .into(),
                ],
                checked_fact_ids: candidate.submission.dependency_fact_ids.clone(),
                checked_source_ids: candidate.submission.external_source_ids.clone(),
                evidence_level: "reviewer_independence_unresolved".into(),
            });
        }
        if !target_goals.is_empty() {
            let coverage_report = if candidate.submission.candidate_type
                != research_domain::CandidateType::Counterexample
                && target_goals.iter().all(|goal| {
                    statements_match_target(&goal.statement, &candidate.submission.statement)
                }) {
                VerificationReport {
                    verdict: VerificationVerdict::Accepted,
                    summary: "候选陈述与所有目标陈述规范化后相同；目标覆盖关系确定性通过。".into(),
                    critical_errors: Vec::new(),
                    gaps: Vec::new(),
                    uncertainties: Vec::new(),
                    repair_actions: Vec::new(),
                    checked_fact_ids: candidate.submission.dependency_fact_ids.clone(),
                    checked_source_ids: candidate.submission.external_source_ids.clone(),
                    evidence_level: "deterministic_goal_equivalence".into(),
                }
            } else {
                self.run_reviewer(
                    "goal_coverage_review",
                    verification_id,
                    ReviewerContext {
                        project: &project,
                        submission: &candidate.submission,
                        dependencies: &dependencies,
                        sources: &sources,
                        target_goals: &target_goals,
                    },
                )
                .await
            };
            let (_, event) = self
                .store
                .record_verification_check(
                    &case.case_id,
                    CheckDraft {
                        attempt_id: None,
                        kind: "goal_coverage_review".into(),
                        status: verdict_check_status(coverage_report.verdict),
                        // This check controls Goal closure, not whether a valid intermediate
                        // result may enter the Fact DAG.
                        mandatory: false,
                        summary: coverage_report.summary.clone(),
                        details: serde_json::json!({
                            "target_goals":target_goals,
                            "coverage_report":coverage_report,
                        }),
                    },
                )
                .await?;
            self.publish(event);
        }
        let mut report = adjudicate_reviews(&candidate.submission, &reviews);
        if formal_required && report.verdict == VerificationVerdict::Accepted {
            self.publish(
                self.store
                    .transition_verification_case(
                        &case.case_id,
                        case.cancellation_epoch,
                        VerificationStage::Formalization,
                        None,
                    )
                    .await?,
            );
            report = self
                .run_v2_certification(
                    &case,
                    verification_id,
                    &project,
                    &candidate.submission,
                    &dependencies,
                    &reviews,
                )
                .await?;
        } else {
            self.publish(
                self.store
                    .transition_verification_case(
                        &case.case_id,
                        case.cancellation_epoch,
                        VerificationStage::Adjudication,
                        None,
                    )
                    .await?,
            );
            if report.verdict == VerificationVerdict::Accepted {
                self.publish(
                    self.store
                        .transition_verification_case(
                            &case.case_id,
                            case.cancellation_epoch,
                            VerificationStage::CommitReady,
                            Some(AcceptanceClass::Reviewed),
                        )
                        .await?,
                );
            }
        }
        let commit = self
            .store
            .commit_verification(verification_id, report)
            .await?;
        self.publish_all(commit.events);
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn run_reviewer(
        &self,
        reviewer_kind: &str,
        verification_id: &str,
        context: ReviewerContext<'_>,
    ) -> VerificationReport {
        let ReviewerContext {
            project,
            submission,
            dependencies,
            sources,
            target_goals,
        } = context;
        let working_directory = self
            .config
            .runtime_root
            .join(&project.project_id)
            .join("verifiers")
            .join(verification_id)
            .join(reviewer_kind);
        let source_snapshot = if reviewer_kind == "citation_review" {
            stage_sources_for_citation_review(
                self,
                &project.project_id,
                &working_directory,
                sources,
            )
            .await
        } else {
            serde_json::to_value(sources).unwrap_or_else(|_| serde_json::json!([]))
        };
        let base_prompt =
            match verifier_prompt(&project.contract, submission, dependencies, reviewer_kind) {
                Ok(prompt) => prompt,
                Err(error) => {
                    return unavailable_verification_report(
                        submission,
                        &format!("{reviewer_kind} prompt unavailable: {error}"),
                    );
                }
            };
        let role_contract = reviewer_role_contract(reviewer_kind);
        let source_packet =
            serde_json::to_string_pretty(&source_snapshot).unwrap_or_else(|_| "[]".into());
        let verification_context = serde_json::json!({
            "problem_contract":project.contract,
            "candidate":submission,
            "active_dependency_facts":dependencies,
            "source_snapshot":source_snapshot,
            "target_goals":target_goals,
            "reviewer_kind":reviewer_kind,
            "reviewer_role_contract":role_contract,
            "reviewer_isolation":"No other reviewer result is present in this packet.",
            "trust_rule":"Only active_dependency_facts are mathematical premises; sources remain evidence requiring citation review.",
        });
        let allowed_fact_ids = dependencies
            .iter()
            .map(|fact| fact.fact_id.clone())
            .collect::<Vec<_>>();
        let offer = match self
            .store
            .offer_verification_worker(
                &self
                    .store
                    .verification_case_for_verification(verification_id)
                    .await
                    .map(|case| case.case_id)
                    .unwrap_or_default(),
                reviewer_kind,
                self.backend.name(),
                self.config.model.as_deref(),
                &working_directory.to_string_lossy(),
                &verification_context,
                &allowed_fact_ids,
                &serde_json::json!({
                    "success":"return one schema-valid accepted/rejected/unknown report",
                    "not_acceptable":["consult another reviewer result","write Fact Graph","treat backend failure as mathematical rejection"]
                }),
            )
            .await
        {
            Ok(offer) => {
                self.publish_all(offer.events.clone());
                offer
            }
            Err(error) => {
                return unavailable_verification_report(
                    submission,
                    &format!("{reviewer_kind} offer unavailable: {error}"),
                );
            }
        };
        let handle = match self
            .backend
            .create(AgentSpec {
                project_id: project.project_id.clone(),
                role: reviewer_kind.into(),
                model: self.config.model.clone(),
                working_directory,
            })
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                if let Ok(events) = self
                    .store
                    .fail_verification_worker(&offer, None, &error.to_string())
                    .await
                {
                    self.publish_all(events);
                }
                return unavailable_verification_report(
                    submission,
                    &format!("{reviewer_kind} create unavailable: {error}"),
                );
            }
        };
        let lease = match self
            .store
            .accept_verification_worker_handshake(
                &offer,
                None,
                &serde_json::json!({
                    "handle_id":&handle.handle_id,
                    "role":&handle.role,
                    "context_hash":&offer.context_packet.content_hash,
                    "contract_hash":&offer.contract_hash,
                    "capabilities":self.backend.capabilities(),
                }),
                self.config.verifier_timeout_seconds.saturating_add(60),
            )
            .await
        {
            Ok(lease) => {
                self.publish_all(lease.events.clone());
                lease
            }
            Err(error) => {
                if let Ok(events) = self
                    .store
                    .fail_verification_worker(&offer, None, &error.to_string())
                    .await
                {
                    self.publish_all(events);
                }
                return unavailable_verification_report(
                    submission,
                    &format!("{reviewer_kind} handshake unavailable: {error}"),
                );
            }
        };
        let heartbeat_stop = CancellationToken::new();
        let heartbeat_handle = {
            let store = self.store.clone();
            let lease = lease.clone();
            let stop = heartbeat_stop.clone();
            let ttl = self.config.verifier_timeout_seconds.saturating_add(60);
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        () = stop.cancelled() => break,
                        () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                            if store.heartbeat_verification_worker(&lease,ttl).await.is_err() { break; }
                        }
                    }
                }
            })
        };
        let task = AgentTask {
            kind: AgentTaskKind::Verifier,
            prompt: format!(
                "独立审查角色：{reviewer_kind}\n{role_contract}\n不得参考其他审查者的结论。后端故障与数学否定必须区分。\nTask Contract hash：{}\nContext Packet revision/hash：{}/{}\n目标快照：{}\n来源快照：{source_packet}\n\n{base_prompt}",
                offer.contract_hash,
                offer.context_packet.source_revision,
                offer.context_packet.content_hash,
                serde_json::to_string_pretty(target_goals).unwrap_or_else(|_| "[]".into())
            ),
            output_schema: verifier_schema(),
            timeout_seconds: self.config.verifier_timeout_seconds,
        };
        let run_result = self
            .run_agent_counted(CountedAgentRun {
                handle: &handle,
                task,
                project,
                round: project.current_round,
                worker_id: None,
                // Verification has its own policy budget; it must not consume the producing
                // worker task's model-call allowance.
                task_id: None,
                cancellation: CancellationToken::new(),
            })
            .await;
        heartbeat_stop.cancel();
        let _ = heartbeat_handle.await;
        match run_result {
            Ok(result) => {
                let (envelope_id, events) = match self
                    .store
                    .submit_verification_result(&lease, "completed", &result.structured_output)
                    .await
                {
                    Ok(value) => value,
                    Err(error) => {
                        if let Ok(events) = self
                            .store
                            .fail_verification_worker(&offer, Some(&lease), &error.to_string())
                            .await
                        {
                            self.publish_all(events);
                        }
                        return unavailable_verification_report(
                            submission,
                            &format!("{reviewer_kind} result submit unavailable: {error}"),
                        );
                    }
                };
                self.publish_all(events);
                let (payload, events) =
                    match self.store.ingest_verification_result(&envelope_id).await {
                        Ok(value) => value,
                        Err(error) => {
                            return unavailable_verification_report(
                                submission,
                                &format!("{reviewer_kind} result ingestion unavailable: {error}"),
                            );
                        }
                    };
                self.publish_all(events);
                serde_json::from_value(payload).unwrap_or_else(|error| {
                    unavailable_verification_report(
                        submission,
                        &format!("invalid {reviewer_kind} output: {error}"),
                    )
                })
            }
            Err(error) => {
                if let Ok(events) = self
                    .store
                    .fail_verification_worker(&offer, Some(&lease), &error.to_string())
                    .await
                {
                    self.publish_all(events);
                }
                unavailable_verification_report(
                    submission,
                    &format!("{reviewer_kind} runtime unavailable: {error}"),
                )
            }
        }
    }

    async fn load_dependency_closure(
        &self,
        root_fact_ids: &[String],
    ) -> CoreResult<Vec<research_domain::Fact>> {
        let mut pending = root_fact_ids.to_vec();
        let mut visited = HashSet::new();
        let mut facts = Vec::new();
        while let Some(fact_id) = pending.pop() {
            if !visited.insert(fact_id.clone()) {
                continue;
            }
            let fact = self.store.get_fact(&fact_id).await?;
            pending.extend(fact.dependency_fact_ids.iter().cloned());
            facts.push(fact);
        }
        facts.sort_by(|left, right| left.fact_id.cmp(&right.fact_id));
        Ok(facts)
    }

    async fn record_review_details(
        &self,
        case_id: &str,
        reviewer_kind: &str,
        check_id: &str,
        report: &VerificationReport,
    ) -> CoreResult<()> {
        for critical_error in &report.critical_errors {
            let (_, event) = self
                .store
                .record_verification_finding(
                    case_id,
                    FindingDraft {
                        check_id: Some(check_id.into()),
                        reviewer_kind: reviewer_kind.into(),
                        severity: "critical".into(),
                        category: "logical_error".into(),
                        location: None,
                        claim: critical_error.clone(),
                        rationale: report.summary.clone(),
                        status: "open".into(),
                    },
                )
                .await?;
            self.publish(event);
        }
        for gap in &report.gaps {
            let (_, event) = self
                .store
                .record_verification_finding(
                    case_id,
                    FindingDraft {
                        check_id: Some(check_id.into()),
                        reviewer_kind: reviewer_kind.into(),
                        severity: "major".into(),
                        category: gap.gap_type.clone(),
                        location: Some(gap.location.clone()),
                        claim: gap.issue.clone(),
                        rationale: report.summary.clone(),
                        status: "open".into(),
                    },
                )
                .await?;
            self.publish(event);
        }
        let dimension = if reviewer_kind == "citation_review" {
            "reproducibility"
        } else {
            "logical_check"
        };
        let (_, event) = self
            .store
            .record_verification_evidence(
                case_id,
                EvidenceDraft {
                    check_id: Some(check_id.into()),
                    dimension: dimension.into(),
                    kind: reviewer_kind.into(),
                    uri: None,
                    payload: serde_json::to_value(report)?,
                },
            )
            .await?;
        self.publish(event);
        Ok(())
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_v2_certification(
        &self,
        case: &VerificationCase,
        verification_id: &str,
        project: &Project,
        submission: &CandidateSubmission,
        dependencies: &[research_domain::Fact],
        reviews: &[VerificationReport],
    ) -> CoreResult<VerificationReport> {
        let formalizer = match self
            .run_formalizer_agent(verification_id, project, submission, dependencies)
            .await
        {
            Ok(output) => output,
            Err(reason) => {
                self.record_case_check(
                    &case.case_id,
                    "semantic_contract",
                    CheckStatus::Unknown,
                    &reason,
                    serde_json::json!({"error_kind":"formalizer_unavailable"}),
                )
                .await?;
                return Ok(certification_unavailable_report(
                    submission, reviews, &reason,
                ));
            }
        };
        let (semantic_contract, event) = self
            .store
            .store_semantic_contract(
                &case.case_id,
                &submission.statement,
                formalizer.semantic_contract.clone(),
            )
            .await?;
        self.publish(event);
        let (formalization, event) = self
            .store
            .store_formalization(&case.case_id, &semantic_contract.contract_id, &formalizer)
            .await?;
        self.publish(event);
        self.record_case_check(
            &case.case_id,
            "semantic_contract",
            CheckStatus::Passed,
            "自然语言命题已拆分为版本化语义契约，并生成可追溯 Lean 映射",
            serde_json::json!({
                "semantic_contract_id": semantic_contract.contract_id,
                "formalization_id": formalization.formalization_id,
                "ambiguity_notes": semantic_contract.ambiguity_notes,
            }),
        )
        .await?;
        if !semantic_contract.ambiguity_notes.is_empty() {
            let (_, event) = self
                .store
                .create_human_question(
                    &case.project_id,
                    &format!(
                        "Resolve semantic ambiguities before certifying '{}': {}",
                        submission.statement,
                        semantic_contract.ambiguity_notes.join("; ")
                    ),
                    Vec::new(),
                    vec![case.case_id.clone(), formalization.formalization_id.clone()],
                    "formalizer",
                    None,
                )
                .await?;
            self.publish(event);
        }

        let alignment = match self
            .run_alignment_agent(
                verification_id,
                project,
                submission,
                &semantic_contract,
                &formalization,
            )
            .await
        {
            Ok(output) => output,
            Err(reason) => {
                self.record_case_check(
                    &case.case_id,
                    "alignment_review",
                    CheckStatus::Unknown,
                    &reason,
                    serde_json::json!({"error_kind":"alignment_reviewer_unavailable"}),
                )
                .await?;
                return Ok(certification_unavailable_report(
                    submission, reviews, &reason,
                ));
            }
        };
        let (alignment_review, event) = self
            .store
            .store_alignment_review(
                &case.case_id,
                &formalization.formalization_id,
                "independent_alignment_reviewer",
                alignment,
            )
            .await?;
        self.publish(event);
        let aligned = matches!(
            alignment_review.relation,
            AlignmentRelation::Equivalent | AlignmentRelation::FormalStronger
        );
        self.record_case_check(
            &case.case_id,
            "alignment_review",
            if aligned {
                CheckStatus::Passed
            } else {
                CheckStatus::Failed
            },
            &alignment_review.rationale,
            serde_json::to_value(&alignment_review)?,
        )
        .await?;
        let (_, event) = self
            .store
            .record_verification_evidence(
                &case.case_id,
                EvidenceDraft {
                    check_id: None,
                    dimension: "statement_alignment".into(),
                    kind: "semantic_alignment_review".into(),
                    uri: None,
                    payload: serde_json::to_value(&alignment_review)?,
                },
            )
            .await?;
        self.publish(event);
        if !aligned {
            let reason = format!(
                "形式化陈述与自然语言命题不满足认证对齐要求：{}",
                alignment_review.relation
            );
            let (_, event) = self
                .store
                .create_human_question(
                    &case.project_id,
                    &format!("{reason}。请选择修订自然语言陈述、修订形式化陈述或拒绝该形式化。"),
                    vec![
                        serde_json::json!({"id":"revise_natural_statement"}),
                        serde_json::json!({"id":"revise_formal_statement"}),
                        serde_json::json!({"id":"reject_formalization"}),
                    ],
                    vec![case.case_id.clone(), formalization.formalization_id.clone()],
                    "alignment_reviewer",
                    None,
                )
                .await?;
            self.publish(event);
            return Ok(certification_unavailable_report(
                submission, reviews, &reason,
            ));
        }

        let Some(backend) = self.verification_backend.as_ref() else {
            let reason = "Lean verification backend is not configured";
            self.record_case_check(
                &case.case_id,
                "lean_kernel",
                CheckStatus::Error,
                reason,
                serde_json::json!({"error_kind":"backend_unavailable"}),
            )
            .await?;
            return Ok(certification_unavailable_report(
                submission, reviews, reason,
            ));
        };
        let mut certified_formalization = formalization.clone();
        let (mut first_outcome, first_attempt_id) = match self
            .run_reliable_verification_backend(
                case,
                project,
                backend,
                "lean_final",
                BackendRequest {
                    case_id: case.case_id.clone(),
                    attempt_id: None,
                    theorem_name: formalization.theorem_name.clone(),
                    source: formalization.lean_source.clone(),
                    working_directory: self
                        .config
                        .runtime_root
                        .join(&project.project_id)
                        .join("verifiers")
                        .join(verification_id)
                        .join("lean_final"),
                    timeout_seconds: self.config.verifier_timeout_seconds,
                },
                dependencies,
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                let reason =
                    format!("Lean backend unavailable without mathematical verdict: {error}");
                self.record_case_check(
                    &case.case_id,
                    "lean_kernel",
                    CheckStatus::Error,
                    &reason,
                    serde_json::json!({"error_kind":"backend_error"}),
                )
                .await?;
                return Ok(certification_unavailable_report(
                    submission, reviews, &reason,
                ));
            }
        };
        let (mut first_run, event) = self
            .store_backend_outcome(&case.case_id, Some(&first_attempt_id), &first_outcome)
            .await?;
        self.publish(event);
        self.record_case_check(
            &case.case_id,
            "lean_kernel",
            first_outcome.status,
            if first_outcome.status == CheckStatus::Passed {
                "Lean 内核检查通过，且未发现未批准公理或逃逸构造"
            } else {
                "Lean 编译、公理审计或安全策略未通过；这不是对自然语言命题的数学否定"
            },
            serde_json::json!({
                "backend_run_id": first_run.backend_run_id,
                "diagnostics": first_outcome.diagnostics,
                "axioms": first_outcome.axioms,
            }),
        )
        .await?;
        if first_outcome.status != CheckStatus::Passed {
            let Some(interactive_backend) = self.interactive_proof_backend.as_ref() else {
                return Ok(certification_unavailable_report(
                    submission,
                    reviews,
                    "Lean final checker did not certify the prepared formalization and no interactive proof backend is configured",
                ));
            };
            self.publish(
                self.store
                    .transition_verification_case(
                        &case.case_id,
                        case.cancellation_epoch,
                        VerificationStage::ProofSearch,
                        None,
                    )
                    .await?,
            );
            let repaired = match self
                .run_interactive_proof_search(
                    interactive_backend,
                    case,
                    verification_id,
                    project,
                    submission,
                    dependencies,
                    &formalization,
                )
                .await
            {
                Ok(Some(formalization)) => formalization,
                Ok(None) => {
                    return Ok(certification_unavailable_report(
                        submission,
                        reviews,
                        "interactive proof search exhausted its budget or was cancelled",
                    ));
                }
                Err(reason) => {
                    return Ok(certification_unavailable_report(
                        submission, reviews, &reason,
                    ));
                }
            };
            let (repaired_outcome, repaired_attempt_id) = match self
                .run_reliable_verification_backend(
                    case,
                    project,
                    backend,
                    "lean_after_search",
                    BackendRequest {
                        case_id: case.case_id.clone(),
                        attempt_id: None,
                        theorem_name: repaired.theorem_name.clone(),
                        source: repaired.lean_source.clone(),
                        working_directory: self
                            .config
                            .runtime_root
                            .join(&project.project_id)
                            .join("verifiers")
                            .join(verification_id)
                            .join("lean_after_search"),
                        timeout_seconds: self.config.verifier_timeout_seconds,
                    },
                    dependencies,
                )
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    let reason =
                        format!("Lean final checker after proof search unavailable: {error}");
                    self.record_case_check(
                        &case.case_id,
                        "lean_kernel",
                        CheckStatus::Error,
                        &reason,
                        serde_json::json!({"error_kind":"backend_error_after_search"}),
                    )
                    .await?;
                    return Ok(certification_unavailable_report(
                        submission, reviews, &reason,
                    ));
                }
            };
            let (repaired_run, event) = self
                .store_backend_outcome(&case.case_id, Some(&repaired_attempt_id), &repaired_outcome)
                .await?;
            self.publish(event);
            self.record_case_check(
                &case.case_id,
                "lean_kernel",
                repaired_outcome.status,
                if repaired_outcome.status == CheckStatus::Passed {
                    "Pantograph 找到的 tactic 路径已由独立 Lean 内核新进程验证"
                } else {
                    "交互搜索候选未通过独立 Lean 内核终审"
                },
                serde_json::json!({
                    "backend_run_id": repaired_run.backend_run_id,
                    "formalization_id": repaired.formalization_id,
                    "diagnostics": repaired_outcome.diagnostics,
                    "axioms": repaired_outcome.axioms,
                }),
            )
            .await?;
            if repaired_outcome.status != CheckStatus::Passed {
                return Ok(certification_unavailable_report(
                    submission,
                    reviews,
                    "interactive proof candidate failed independent Lean final checking",
                ));
            }
            certified_formalization = repaired;
            first_outcome = repaired_outcome;
            first_run = repaired_run;
        }

        self.publish(
            self.store
                .transition_verification_case(
                    &case.case_id,
                    case.cancellation_epoch,
                    VerificationStage::Adjudication,
                    None,
                )
                .await?,
        );
        self.publish(
            self.store
                .transition_verification_case(
                    &case.case_id,
                    case.cancellation_epoch,
                    VerificationStage::Packaging,
                    None,
                )
                .await?,
        );
        let (manifest, package_path) = match self
            .build_v2_package(
                project,
                verification_id,
                case,
                &semantic_contract,
                &certified_formalization,
                &alignment_review,
                &first_run,
            )
            .await
        {
            Ok(package) => package,
            Err(error) => {
                let reason = format!("verification package build failed: {error}");
                self.record_case_check(
                    &case.case_id,
                    "package_integrity",
                    CheckStatus::Error,
                    &reason,
                    serde_json::json!({"error_kind":"package_build_error"}),
                )
                .await?;
                return Ok(certification_unavailable_report(
                    submission, reviews, &reason,
                ));
            }
        };
        let package_path_text = package_path.to_string_lossy().into_owned();
        let (package, event) = self
            .store
            .store_verification_package(&case.case_id, manifest, &package_path_text)
            .await?;
        self.publish(event);
        let observed_manifest_hash = match self.verify_package_integrity(&package).await {
            Ok(hash) => hash,
            Err(error) => {
                let reason = format!("verification package could not be re-read: {error}");
                self.record_case_check(
                    &case.case_id,
                    "package_integrity",
                    CheckStatus::Error,
                    &reason,
                    serde_json::json!({"error_kind":"package_read_error"}),
                )
                .await?;
                return Ok(certification_unavailable_report(
                    submission, reviews, &reason,
                ));
            }
        };
        let package_ok = observed_manifest_hash == package.manifest_hash;
        self.record_case_check(
            &case.case_id,
            "package_integrity",
            if package_ok {
                CheckStatus::Passed
            } else {
                CheckStatus::Failed
            },
            if package_ok {
                "验证包 manifest 与所有内容哈希一致"
            } else {
                "验证包内容哈希不一致"
            },
            serde_json::json!({"package_id": package.package_id, "manifest_hash": package.manifest_hash, "observed_hash": observed_manifest_hash}),
        )
        .await?;
        if !package_ok {
            return Ok(certification_unavailable_report(
                submission,
                reviews,
                "verification package integrity failed",
            ));
        }
        self.publish(
            self.store
                .transition_verification_case(
                    &case.case_id,
                    case.cancellation_epoch,
                    VerificationStage::Replay,
                    None,
                )
                .await?,
        );
        let (replay_outcome, replay_attempt_id) = match self
            .run_reliable_verification_backend(
                case,
                project,
                backend,
                "lean_replay",
                BackendRequest {
                    case_id: case.case_id.clone(),
                    attempt_id: None,
                    theorem_name: certified_formalization.theorem_name.clone(),
                    source: certified_formalization.lean_source.clone(),
                    working_directory: self
                        .config
                        .runtime_root
                        .join(&project.project_id)
                        .join("verifiers")
                        .join(verification_id)
                        .join("lean_replay"),
                    timeout_seconds: self.config.verifier_timeout_seconds,
                },
                dependencies,
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                let reason = format!("fresh-process replay backend error: {error}");
                self.record_case_check(
                    &case.case_id,
                    "fresh_replay",
                    CheckStatus::Error,
                    &reason,
                    serde_json::json!({"error_kind":"backend_error"}),
                )
                .await?;
                return Ok(certification_unavailable_report(
                    submission, reviews, &reason,
                ));
            }
        };
        let (replay_run, event) = self
            .store_backend_outcome(&case.case_id, Some(&replay_attempt_id), &replay_outcome)
            .await?;
        self.publish(event);
        let replay_passed = replay_outcome.status == CheckStatus::Passed
            && replay_outcome.input_hash == first_outcome.input_hash;
        let (replay, event) = self
            .store
            .store_verification_replay(
                &case.case_id,
                &package.package_id,
                if replay_passed {
                    CheckStatus::Passed
                } else {
                    CheckStatus::Failed
                },
                &package.manifest_hash,
                &observed_manifest_hash,
                Some(&replay_run.backend_run_id),
                if replay_passed {
                    "fresh process Lean replay passed"
                } else {
                    "fresh process Lean replay diverged or failed"
                },
            )
            .await?;
        self.publish(event);
        self.record_case_check(
            &case.case_id,
            "fresh_replay",
            replay.status,
            &replay.summary,
            serde_json::to_value(&replay)?,
        )
        .await?;
        if !replay_passed {
            return Ok(certification_unavailable_report(
                submission,
                reviews,
                "fresh-process Lean replay did not reproduce the result",
            ));
        }
        self.publish(
            self.store
                .transition_verification_case(
                    &case.case_id,
                    case.cancellation_epoch,
                    VerificationStage::CommitReady,
                    Some(AcceptanceClass::FullyCertified),
                )
                .await?,
        );
        Ok(certified_report(
            submission,
            reviews,
            &package.package_id,
            &replay.replay_id,
        ))
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_interactive_proof_search(
        &self,
        backend: &Arc<dyn InteractiveProofBackend>,
        case: &VerificationCase,
        verification_id: &str,
        project: &Project,
        submission: &CandidateSubmission,
        dependencies: &[research_domain::Fact],
        formalization: &Formalization,
    ) -> Result<Option<Formalization>, String> {
        let allowed_fact_ids = dependencies
            .iter()
            .map(|fact| fact.fact_id.clone())
            .collect::<Vec<_>>();
        let source_hash = hex::encode(Sha256::digest(formalization.lean_source.as_bytes()));
        let working_directory = self
            .config
            .runtime_root
            .join(&project.project_id)
            .join("verifiers")
            .join(verification_id)
            .join("pantograph_search");
        let offer = self
            .store
            .offer_verification_worker(
                &case.case_id,
                "pantograph_proof_search",
                backend.name(),
                None,
                &working_directory.to_string_lossy(),
                &serde_json::json!({
                    "project_id":project.project_id,
                    "case_id":case.case_id,
                    "formalization_id":formalization.formalization_id,
                    "lean_statement":formalization.lean_statement,
                    "source_hash":source_hash,
                    "active_dependency_fact_ids":allowed_fact_ids,
                }),
                &allowed_fact_ids,
                &serde_json::json!({
                    "success":"return a kernel-checkable tactic path or an explicit exhausted result",
                    "budget":self.config.proof_search_budget,
                }),
            )
            .await
            .map_err(|error| error.to_string())?;
        self.publish_all(offer.events.clone());
        let backend_version = match backend.version().await {
            Ok(version) => version,
            Err(error) => {
                if let Ok(events) = self
                    .store
                    .fail_verification_worker(&offer, None, &error.to_string())
                    .await
                {
                    self.publish_all(events);
                }
                return Err(error.to_string());
            }
        };
        let ttl = self.config.verifier_timeout_seconds.saturating_add(60);
        let lease = match self
            .store
            .accept_verification_worker_handshake(
                &offer,
                Some(&backend_version),
                &serde_json::json!({
                    "backend":backend.name(),"backend_version":backend_version,
                    "context_hash":offer.context_packet.content_hash,
                    "contract_hash":offer.contract_hash,"source_hash":source_hash,
                }),
                ttl,
            )
            .await
        {
            Ok(lease) => lease,
            Err(error) => {
                if let Ok(events) = self
                    .store
                    .fail_verification_worker(&offer, None, &error.to_string())
                    .await
                {
                    self.publish_all(events);
                }
                return Err(error.to_string());
            }
        };
        self.publish_all(lease.events.clone());
        let stop = CancellationToken::new();
        let heartbeat = {
            let store = self.store.clone();
            let lease = lease.clone();
            let stop = stop.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        () = stop.cancelled() => break,
                        () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                            if store.heartbeat_verification_worker(&lease,ttl).await.is_err() { break; }
                        }
                    }
                }
            })
        };
        let result = self
            .run_interactive_proof_search_inner(
                backend,
                case,
                verification_id,
                project,
                submission,
                dependencies,
                formalization,
            )
            .await;
        stop.cancel();
        let _ = heartbeat.await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if let Ok(events) = self
                    .store
                    .fail_verification_worker(&offer, Some(&lease), &error)
                    .await
                {
                    self.publish_all(events);
                }
                return Err(error);
            }
        };
        let payload = serde_json::json!({
            "outcome":if result.is_some() {"closed"} else {"exhausted"},
            "formalization_id":result.as_ref().map(|item| &item.formalization_id),
            "source_hash":source_hash,
        });
        let (envelope_id, events) = self
            .store
            .submit_verification_result(
                &lease,
                if result.is_some() {
                    "closed"
                } else {
                    "exhausted"
                },
                &payload,
            )
            .await
            .map_err(|error| error.to_string())?;
        self.publish_all(events);
        let (_, events) = self
            .store
            .ingest_verification_result(&envelope_id)
            .await
            .map_err(|error| error.to_string())?;
        self.publish_all(events);
        Ok(result)
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_interactive_proof_search_inner(
        &self,
        backend: &Arc<dyn InteractiveProofBackend>,
        case: &VerificationCase,
        verification_id: &str,
        project: &Project,
        _submission: &CandidateSubmission,
        dependencies: &[research_domain::Fact],
        formalization: &Formalization,
    ) -> Result<Option<Formalization>, String> {
        let cancellation = CancellationToken::new();
        let session = backend
            .open_session(cancellation.clone())
            .await
            .map_err(|error| format!("Pantograph session unavailable: {error}"))?;
        let root_state = match session
            .start_goal(&formalization.lean_statement, cancellation.clone())
            .await
        {
            Ok(state) => state,
            Err(error) => {
                let _ = session.shutdown().await;
                return Err(format!(
                    "Pantograph could not elaborate the Lean statement: {error}"
                ));
            }
        };
        if root_state.has_sorry || root_state.has_unsafe {
            let _ = session.shutdown().await;
            return Err("Pantograph reported unsafe or sorry-dependent root state".into());
        }
        let root_goal = proof_goal_summary(&root_state.goals);
        let root_context = serde_json::to_value(&root_state.goals)
            .map_err(|error| format!("serialize Pantograph root goals: {error}"))?;
        let root_state_id = root_state
            .state_id
            .and_then(|value| i64::try_from(value).ok());
        let (search, root, events) = self
            .store
            .create_proof_search(
                &case.case_id,
                &formalization.formalization_id,
                "best_first_beam",
                self.config.proof_search_budget.clone(),
                ProofNodeDraft {
                    parent_node_id: None,
                    state_id: root_state_id,
                    goal: root_goal,
                    local_context: root_context,
                    tactic: None,
                    score: proof_state_score(&root_state.goals, 0, 0.5),
                    status: if root_state.is_closed() {
                        ProofNodeStatus::Closed
                    } else {
                        ProofNodeStatus::Open
                    },
                    diagnostic: None,
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        self.publish_all(events);
        self.task_cancellations
            .lock()
            .await
            .insert(search.search_id.clone(), cancellation.clone());
        if root_state.is_closed() {
            let (finished, event) = self
                .store
                .finish_proof_search(
                    &search.search_id,
                    search.cancellation_epoch,
                    CheckStatus::Passed,
                    Some(&root.node_id),
                )
                .await
                .map_err(|error| error.to_string())?;
            self.publish(event);
            self.task_cancellations
                .lock()
                .await
                .remove(&search.search_id);
            let _ = session.shutdown().await;
            let source = proof_source(formalization, &[]);
            let (solution, event) = self
                .store
                .store_proof_search_solution(
                    &case.case_id,
                    &formalization.formalization_id,
                    &source,
                )
                .await
                .map_err(|error| error.to_string())?;
            self.publish(event);
            let _ = finished;
            return Ok(Some(solution));
        }

        let started = Instant::now();
        let mut frontier = vec![SearchFrontier {
            node: root,
            tactic_path: vec![],
        }];
        let mut seen_states = HashSet::new();
        seen_states.insert(proof_state_signature(&root_state.goals));
        let mut failures = Vec::new();

        while !frontier.is_empty() {
            let live = self
                .store
                .get_proof_search(&search.search_id)
                .await
                .map_err(|error| error.to_string())?;
            if live.status != CheckStatus::Running || cancellation.is_cancelled() {
                self.task_cancellations
                    .lock()
                    .await
                    .remove(&search.search_id);
                let _ = session.shutdown().await;
                return Ok(None);
            }
            if started.elapsed().as_secs() >= live.budget.max_seconds
                || live.nodes_created >= i64::from(live.budget.max_nodes)
            {
                break;
            }
            frontier.sort_by(|left, right| {
                right
                    .node
                    .score
                    .total_cmp(&left.node.score)
                    .then_with(|| left.node.depth.cmp(&right.node.depth))
            });
            frontier.truncate(usize::try_from(live.budget.beam_width.max(1)).unwrap_or(1));
            let mut current = frontier.remove(0);
            current.node.cancellation_epoch = live.cancellation_epoch;
            let Some(state_id) = current
                .node
                .state_id
                .and_then(|value| u64::try_from(value).ok())
            else {
                failures.push(format!(
                    "node {} has no Pantograph state",
                    current.node.node_id
                ));
                continue;
            };
            let hints = self
                .store
                .consume_proof_hints(
                    &search.search_id,
                    &current.node.node_id,
                    live.cancellation_epoch,
                )
                .await
                .map_err(|error| error.to_string())?;
            let (mut candidates, avoid) = deterministic_tactic_candidates(&hints);
            if self
                .store
                .reserve_proof_model_call(&search.search_id, live.cancellation_epoch)
                .await
                .is_ok()
            {
                match self
                    .run_tactic_agent(
                        verification_id,
                        project,
                        formalization,
                        &current.node,
                        &hints,
                        dependencies,
                        &failures,
                    )
                    .await
                {
                    Ok(model_candidates) => candidates.extend(model_candidates),
                    Err(error) => failures.push(format!("tactic model unavailable: {error}")),
                }
            }
            let mut tactics_seen = HashSet::new();
            candidates.retain(|candidate| {
                safe_tactic(&candidate.tactic)
                    && !avoid.contains(candidate.tactic.trim())
                    && tactics_seen.insert(candidate.tactic.trim().to_owned())
            });
            candidates.truncate(usize::try_from(live.budget.beam_width.max(1)).unwrap_or(1));
            let mut created_child = false;
            for candidate in candidates {
                let outcome = match session
                    .apply_tactic(state_id, &candidate.tactic, 30_000, cancellation.clone())
                    .await
                {
                    Ok(outcome) => outcome,
                    Err(BackendError::Cancelled) => {
                        self.task_cancellations
                            .lock()
                            .await
                            .remove(&search.search_id);
                        let _ = session.shutdown().await;
                        return Ok(None);
                    }
                    Err(error) => {
                        failures.push(format!("{}: backend error: {error}", candidate.tactic));
                        continue;
                    }
                };
                let state_signature = proof_state_signature(&outcome.goals);
                let duplicate = outcome.succeeded
                    && !outcome.is_closed()
                    && !seen_states.insert(state_signature);
                let child_status = if outcome.has_sorry || outcome.has_unsafe {
                    ProofNodeStatus::Failed
                } else if outcome.is_closed() {
                    ProofNodeStatus::Closed
                } else if !outcome.succeeded {
                    ProofNodeStatus::Failed
                } else if duplicate {
                    ProofNodeStatus::Pruned
                } else {
                    ProofNodeStatus::Open
                };
                let diagnostic = serde_json::to_string(&outcome.messages).ok();
                let child = self
                    .store
                    .add_proof_node(
                        &search.search_id,
                        live.cancellation_epoch,
                        ProofNodeDraft {
                            parent_node_id: Some(current.node.node_id.clone()),
                            state_id: outcome.state_id.and_then(|value| i64::try_from(value).ok()),
                            goal: proof_goal_summary(&outcome.goals),
                            local_context: serde_json::to_value(&outcome.goals)
                                .unwrap_or_else(|_| serde_json::json!([])),
                            tactic: Some(candidate.tactic.clone()),
                            score: proof_state_score(
                                &outcome.goals,
                                current.node.depth + 1,
                                candidate.expected_goal_reduction,
                            ),
                            status: child_status,
                            diagnostic: diagnostic.clone(),
                        },
                    )
                    .await;
                let (child, _, events) = match child {
                    Ok(value) => value,
                    Err(StorageError::LateSubmission(_)) => {
                        if let Some(state_id) = outcome.state_id {
                            let _ = session.remove_states(&[state_id]).await;
                        }
                        break;
                    }
                    Err(error) => {
                        failures.push(format!("persist tactic {}: {error}", candidate.tactic));
                        continue;
                    }
                };
                created_child = true;
                self.publish_all(events);
                let mut path = current.tactic_path.clone();
                path.push(candidate.tactic.clone());
                if child.status == ProofNodeStatus::Closed {
                    self.publish(
                        self.store
                            .mark_proof_node_expanded(
                                &search.search_id,
                                &current.node.node_id,
                                live.cancellation_epoch,
                                ProofNodeStatus::Expanded,
                                None,
                            )
                            .await
                            .map_err(|error| error.to_string())?,
                    );
                    let (_, event) = self
                        .store
                        .finish_proof_search(
                            &search.search_id,
                            live.cancellation_epoch,
                            CheckStatus::Passed,
                            Some(&child.node_id),
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                    self.publish(event);
                    let source = proof_source(formalization, &path);
                    let (solution, event) = self
                        .store
                        .store_proof_search_solution(
                            &case.case_id,
                            &formalization.formalization_id,
                            &source,
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                    self.publish(event);
                    let (_, evidence_event) = self
                        .store
                        .record_verification_evidence(
                            &case.case_id,
                            EvidenceDraft {
                                check_id: None,
                                dimension: "formal_proof_search".into(),
                                kind: "pantograph_tactic_path".into(),
                                uri: None,
                                payload: serde_json::json!({
                                    "search_id": search.search_id,
                                    "solution_node_id": child.node_id,
                                    "tactics": path,
                                    "backend": backend.name(),
                                }),
                            },
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                    self.publish(evidence_event);
                    self.task_cancellations
                        .lock()
                        .await
                        .remove(&search.search_id);
                    let _ = session.shutdown().await;
                    return Ok(Some(solution));
                }
                if child.status == ProofNodeStatus::Open {
                    frontier.push(SearchFrontier {
                        node: child,
                        tactic_path: path,
                    });
                } else if child.status == ProofNodeStatus::Failed {
                    failures.push(format!(
                        "node {} tactic `{}` failed: {}",
                        current.node.node_id,
                        candidate.tactic,
                        diagnostic.unwrap_or_default()
                    ));
                }
            }
            let parent_status = if created_child {
                ProofNodeStatus::Expanded
            } else {
                ProofNodeStatus::Failed
            };
            match self
                .store
                .mark_proof_node_expanded(
                    &search.search_id,
                    &current.node.node_id,
                    live.cancellation_epoch,
                    parent_status,
                    if created_child {
                        None
                    } else {
                        Some("no viable tactic candidates")
                    },
                )
                .await
            {
                Ok(event) => self.publish(event),
                Err(StorageError::LateSubmission(_)) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        let live = self
            .store
            .get_proof_search(&search.search_id)
            .await
            .map_err(|error| error.to_string())?;
        if live.status == CheckStatus::Running {
            let (_, event) = self
                .store
                .finish_proof_search(
                    &search.search_id,
                    live.cancellation_epoch,
                    CheckStatus::Unknown,
                    None,
                )
                .await
                .map_err(|error| error.to_string())?;
            self.publish(event);
            let summary = compress_proof_failures(&failures);
            let (_, event) = self
                .store
                .record_verification_finding(
                    &case.case_id,
                    FindingDraft {
                        check_id: None,
                        reviewer_kind: "proof_search_coordinator".into(),
                        severity: "major".into(),
                        category: "proof_search_exhausted".into(),
                        location: Some(search.search_id.clone()),
                        claim: "交互式证明搜索未在预算内闭合目标".into(),
                        rationale: summary,
                        status: "open".into(),
                    },
                )
                .await
                .map_err(|error| error.to_string())?;
            self.publish(event);
        }
        self.task_cancellations
            .lock()
            .await
            .remove(&search.search_id);
        let _ = session.shutdown().await;
        Ok(None)
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_reliable_verification_agent(
        &self,
        verification_id: &str,
        role: &str,
        project: &Project,
        working_directory: PathBuf,
        task: AgentTask,
        context: Value,
        dependencies: &[research_domain::Fact],
        completion_contract: Value,
    ) -> Result<AgentRunResult, String> {
        let case = self
            .store
            .verification_case_for_verification(verification_id)
            .await
            .map_err(|error| error.to_string())?;
        let allowed_fact_ids = dependencies
            .iter()
            .map(|fact| fact.fact_id.clone())
            .collect::<Vec<_>>();
        let offer = self
            .store
            .offer_verification_worker(
                &case.case_id,
                role,
                self.backend.name(),
                self.config.model.as_deref(),
                &working_directory.to_string_lossy(),
                &context,
                &allowed_fact_ids,
                &completion_contract,
            )
            .await
            .map_err(|error| error.to_string())?;
        self.publish_all(offer.events.clone());
        let handle = match self
            .backend
            .create(AgentSpec {
                project_id: project.project_id.clone(),
                role: role.into(),
                model: self.config.model.clone(),
                working_directory,
            })
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                if let Ok(events) = self
                    .store
                    .fail_verification_worker(&offer, None, &error.to_string())
                    .await
                {
                    self.publish_all(events);
                }
                return Err(error.to_string());
            }
        };
        let ttl = task.timeout_seconds.saturating_add(60);
        let lease = match self
            .store
            .accept_verification_worker_handshake(
                &offer,
                None,
                &serde_json::json!({
                    "handle_id":&handle.handle_id,"role":role,
                    "context_hash":&offer.context_packet.content_hash,
                    "contract_hash":&offer.contract_hash,
                    "capabilities":self.backend.capabilities(),
                }),
                ttl,
            )
            .await
        {
            Ok(lease) => lease,
            Err(error) => {
                if let Ok(events) = self
                    .store
                    .fail_verification_worker(&offer, None, &error.to_string())
                    .await
                {
                    self.publish_all(events);
                }
                return Err(error.to_string());
            }
        };
        self.publish_all(lease.events.clone());
        let stop = CancellationToken::new();
        let heartbeat = {
            let store = self.store.clone();
            let lease = lease.clone();
            let stop = stop.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        () = stop.cancelled() => break,
                        () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                            if store.heartbeat_verification_worker(&lease,ttl).await.is_err() { break; }
                        }
                    }
                }
            })
        };
        let run = self
            .run_agent_counted(CountedAgentRun {
                handle: &handle,
                task,
                project,
                round: project.current_round,
                worker_id: None,
                task_id: None,
                cancellation: CancellationToken::new(),
            })
            .await;
        stop.cancel();
        let _ = heartbeat.await;
        let mut result = match run {
            Ok(result) => result,
            Err(error) => {
                if let Ok(events) = self
                    .store
                    .fail_verification_worker(&offer, Some(&lease), &error.to_string())
                    .await
                {
                    self.publish_all(events);
                }
                return Err(error.to_string());
            }
        };
        let (envelope_id, events) = self
            .store
            .submit_verification_result(&lease, "completed", &result.structured_output)
            .await
            .map_err(|error| error.to_string())?;
        self.publish_all(events);
        let (payload, events) = self
            .store
            .ingest_verification_result(&envelope_id)
            .await
            .map_err(|error| error.to_string())?;
        self.publish_all(events);
        result.structured_output = payload;
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_tactic_agent(
        &self,
        verification_id: &str,
        project: &Project,
        formalization: &Formalization,
        node: &ProofNode,
        hints: &[ProofHint],
        dependencies: &[research_domain::Fact],
        failures: &[String],
    ) -> Result<Vec<TacticCandidate>, String> {
        let recent_failures = failures.iter().rev().take(16).cloned().collect::<Vec<_>>();
        let prompt =
            tactic_proposal_prompt(formalization, node, hints, dependencies, &recent_failures)
                .map_err(|error| error.to_string())?;
        let result = self
            .run_reliable_verification_agent(
                verification_id,
                "proof_search_tactic_generator",
                project,
                self.config
                    .runtime_root
                    .join(&project.project_id)
                    .join("verifiers")
                    .join(verification_id)
                    .join("proof_search")
                    .join(&node.node_id),
                AgentTask {
                    kind: AgentTaskKind::Verifier,
                    prompt,
                    output_schema: tactic_proposal_schema(),
                    timeout_seconds: 120,
                },
                serde_json::json!({
                    "formalization":formalization,"proof_node":node,"hints":hints,
                    "active_dependency_facts":dependencies,"recent_failure_signatures":recent_failures,
                }),
                dependencies,
                serde_json::json!({"success":"bounded tactic candidates for this exact proof node"}),
            )
            .await
            ?;
        let output: TacticProposalOutput =
            serde_json::from_value(result.structured_output).map_err(|error| error.to_string())?;
        Ok(output.candidates)
    }

    pub async fn add_proof_hint(
        &self,
        search_id: &str,
        node_id: Option<&str>,
        hint_type: &str,
        content: &str,
        requested_by: &str,
        idempotency_key: &str,
    ) -> CoreResult<ProofHint> {
        if !matches!(
            hint_type,
            "use_lemma"
                | "unfold_definition"
                | "try_strategy"
                | "avoid_tactic"
                | "focus_goal"
                | "add_intermediate"
        ) {
            return Err(CoreError::InvalidAgentOutput(format!(
                "unsupported proof hint type: {hint_type}"
            )));
        }
        let (hint, event) = self
            .store
            .add_proof_hint(
                search_id,
                node_id,
                &format!("{hint_type}: {content}"),
                requested_by,
                idempotency_key,
            )
            .await?;
        if let Some(event) = event {
            self.publish(event);
        }
        Ok(hint)
    }

    pub async fn prune_proof_branch(
        &self,
        search_id: &str,
        node_id: &str,
        expected_epoch: i64,
        requested_by: &str,
        idempotency_key: &str,
    ) -> CoreResult<research_domain::ProofSearch> {
        let (search, event) = self
            .store
            .prune_proof_branch(
                search_id,
                node_id,
                expected_epoch,
                requested_by,
                idempotency_key,
            )
            .await?;
        if let Some(event) = event {
            self.publish(event);
        }
        Ok(search)
    }

    pub async fn cancel_proof_search(
        &self,
        search_id: &str,
        expected_epoch: i64,
        requested_by: &str,
        idempotency_key: &str,
    ) -> CoreResult<research_domain::ProofSearch> {
        let (search, event) = self
            .store
            .cancel_proof_search(search_id, expected_epoch, requested_by, idempotency_key)
            .await?;
        if let Some(token) = self.task_cancellations.lock().await.get(search_id) {
            token.cancel();
        }
        if let Some(event) = event {
            self.publish(event);
        }
        Ok(search)
    }

    async fn run_formalizer_agent(
        &self,
        verification_id: &str,
        project: &Project,
        submission: &CandidateSubmission,
        dependencies: &[research_domain::Fact],
    ) -> Result<FormalizerOutput, String> {
        let prompt = formalizer_prompt(&project.contract, submission, dependencies)
            .map_err(|error| error.to_string())?;
        let result = self
            .run_reliable_verification_agent(
                verification_id,
                "formalizer",
                project,
                self.config
                    .runtime_root
                    .join(&project.project_id)
                    .join("verifiers")
                    .join(verification_id)
                    .join("formalizer"),
                AgentTask {
                    kind: AgentTaskKind::Verifier,
                    prompt,
                    output_schema: formalizer_schema(),
                    timeout_seconds: self.config.verifier_timeout_seconds,
                },
                serde_json::json!({"problem_contract":project.contract,"candidate":submission,"active_dependency_facts":dependencies}),
                dependencies,
                serde_json::json!({"success":"semantic contract and self-contained Lean formalization candidate"}),
            )
            .await
            ?;
        serde_json::from_value(result.structured_output).map_err(|error| error.to_string())
    }

    async fn run_alignment_agent(
        &self,
        verification_id: &str,
        project: &Project,
        submission: &CandidateSubmission,
        semantic_contract: &research_domain::SemanticContract,
        formalization: &research_domain::Formalization,
    ) -> Result<AlignmentReviewerOutput, String> {
        let prompt = alignment_prompt(submission, semantic_contract, formalization)
            .map_err(|error| error.to_string())?;
        let result = self
            .run_reliable_verification_agent(
                verification_id,
                "alignment_reviewer",
                project,
                self.config
                    .runtime_root
                    .join(&project.project_id)
                    .join("verifiers")
                    .join(verification_id)
                    .join("alignment"),
                AgentTask {
                    kind: AgentTaskKind::Verifier,
                    prompt,
                    output_schema: alignment_schema(),
                    timeout_seconds: self.config.verifier_timeout_seconds,
                },
                serde_json::json!({"candidate":submission,"semantic_contract":semantic_contract,"formalization":formalization}),
                &[],
                serde_json::json!({"success":"independent semantic alignment relation with explicit assumption differences"}),
            )
            .await
            ?;
        serde_json::from_value(result.structured_output).map_err(|error| error.to_string())
    }

    async fn record_case_check(
        &self,
        case_id: &str,
        kind: &str,
        status: CheckStatus,
        summary: &str,
        details: Value,
    ) -> CoreResult<()> {
        let (_, event) = self
            .store
            .record_verification_check(
                case_id,
                CheckDraft {
                    attempt_id: None,
                    kind: kind.into(),
                    status,
                    mandatory: true,
                    summary: summary.into(),
                    details,
                },
            )
            .await?;
        self.publish(event);
        Ok(())
    }

    async fn store_backend_outcome(
        &self,
        case_id: &str,
        attempt_id: Option<&str>,
        outcome: &BackendOutcome,
    ) -> CoreResult<(research_domain::BackendRun, DomainEvent)> {
        Ok(self
            .store
            .store_backend_run(
                case_id,
                BackendRunDraft {
                    attempt_id: attempt_id.map(str::to_owned),
                    backend: outcome.backend.clone(),
                    backend_version: outcome.backend_version.clone(),
                    status: outcome.status,
                    command: outcome.command.clone(),
                    working_directory: outcome.working_directory.clone(),
                    exit_code: outcome.exit_code,
                    stdout: outcome.stdout.clone(),
                    stderr: outcome.stderr.clone(),
                    diagnostics: outcome.diagnostics.clone(),
                    axioms: outcome.axioms.clone(),
                    elapsed_ms: outcome.elapsed_ms,
                    input_hash: outcome.input_hash.clone(),
                    output_hash: outcome.output_hash.clone(),
                },
            )
            .await?)
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn run_reliable_verification_backend(
        &self,
        case: &VerificationCase,
        project: &Project,
        backend: &Arc<dyn VerificationBackend>,
        kind: &str,
        mut request: BackendRequest,
        dependencies: &[research_domain::Fact],
    ) -> Result<(BackendOutcome, String), String> {
        let allowed_fact_ids = dependencies
            .iter()
            .map(|fact| fact.fact_id.clone())
            .collect::<Vec<_>>();
        let source_hash = hex::encode(Sha256::digest(request.source.as_bytes()));
        let context = serde_json::json!({
            "project_id":project.project_id,
            "case_id":case.case_id,
            "verification_task_kind":kind,
            "theorem_name":request.theorem_name,
            "source_hash":source_hash,
            "active_dependency_fact_ids":allowed_fact_ids,
            "trust_rule":"Only active dependency Facts and the immutable formalization source are premises.",
        });
        let offer = self
            .store
            .offer_verification_worker(
                &case.case_id,
                kind,
                backend.name(),
                None,
                &request.working_directory.to_string_lossy(),
                &context,
                &allowed_fact_ids,
                &serde_json::json!({
                    "success":"return one immutable backend outcome envelope",
                    "required_checks":["process_exit","diagnostics","axiom_audit","input_hash","output_hash"]
                }),
            )
            .await
            .map_err(|error| error.to_string())?;
        self.publish_all(offer.events.clone());
        let backend_version = match backend.version().await {
            Ok(version) => version,
            Err(error) => {
                if let Ok(events) = self
                    .store
                    .fail_verification_worker(&offer, None, &error.to_string())
                    .await
                {
                    self.publish_all(events);
                }
                return Err(error.to_string());
            }
        };
        let lease = match self
            .store
            .accept_verification_worker_handshake(
                &offer,
                Some(&backend_version),
                &serde_json::json!({
                    "backend":backend.name(),
                    "backend_version":backend_version,
                    "context_hash":offer.context_packet.content_hash,
                    "contract_hash":offer.contract_hash,
                    "source_hash":source_hash,
                }),
                request.timeout_seconds.saturating_add(60),
            )
            .await
        {
            Ok(lease) => lease,
            Err(error) => {
                if let Ok(events) = self
                    .store
                    .fail_verification_worker(&offer, None, &error.to_string())
                    .await
                {
                    self.publish_all(events);
                }
                return Err(error.to_string());
            }
        };
        self.publish_all(lease.events.clone());
        request.attempt_id = Some(offer.attempt_id.clone());
        let stop = CancellationToken::new();
        let heartbeat = {
            let store = self.store.clone();
            let lease = lease.clone();
            let stop = stop.clone();
            let ttl = request.timeout_seconds.saturating_add(60);
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        () = stop.cancelled() => break,
                        () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                            if store.heartbeat_verification_worker(&lease,ttl).await.is_err() { break; }
                        }
                    }
                }
            })
        };
        let outcome = backend.verify(request, CancellationToken::new()).await;
        stop.cancel();
        let _ = heartbeat.await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                if let Ok(events) = self
                    .store
                    .fail_verification_worker(&offer, Some(&lease), &error.to_string())
                    .await
                {
                    self.publish_all(events);
                }
                return Err(error.to_string());
            }
        };
        let payload = serde_json::to_value(&outcome).map_err(|error| error.to_string())?;
        let (envelope_id, events) = self
            .store
            .submit_verification_result(&lease, &outcome.status.to_string(), &payload)
            .await
            .map_err(|error| error.to_string())?;
        self.publish_all(events);
        let (_, events) = self
            .store
            .ingest_verification_result(&envelope_id)
            .await
            .map_err(|error| error.to_string())?;
        self.publish_all(events);
        Ok((outcome, offer.attempt_id))
    }

    async fn verification_toolchain_hash(&self) -> CoreResult<String> {
        let backend = self
            .verification_backend
            .as_ref()
            .ok_or_else(|| CoreError::InvalidAgentOutput("verification backend missing".into()))?;
        let version = backend.version().await?;
        let pinned = self.read_pinned_lean_files().await?;
        Ok(sha256_json(
            &serde_json::json!({"backend_version": version, "pinned": pinned}),
        )?)
    }

    async fn read_pinned_lean_files(&self) -> CoreResult<Value> {
        let root = self.config.lean_project_root.as_ref().ok_or_else(|| {
            CoreError::InvalidAgentOutput("lean_project_root is not configured".into())
        })?;
        let mut files = serde_json::Map::new();
        for filename in [
            "lean-toolchain",
            "lakefile.toml",
            "lake-manifest.json",
            "SOURCE_LOCK.json",
        ] {
            let path = root.join(filename);
            let bytes = tokio::fs::read(&path).await.map_err(|error| {
                CoreError::InvalidAgentOutput(format!(
                    "required pinned Lean file {}: {error}",
                    path.display()
                ))
            })?;
            files.insert(
                filename.into(),
                serde_json::json!({
                    "sha256": hex::encode(Sha256::digest(&bytes)),
                    "content": String::from_utf8_lossy(&bytes),
                }),
            );
        }
        Ok(Value::Object(files))
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_v2_package(
        &self,
        project: &Project,
        verification_id: &str,
        case: &VerificationCase,
        semantic_contract: &research_domain::SemanticContract,
        formalization: &research_domain::Formalization,
        alignment: &research_domain::AlignmentReview,
        backend_run: &research_domain::BackendRun,
    ) -> Result<(Value, PathBuf), String> {
        let package_path = self
            .config
            .runtime_root
            .join(&project.project_id)
            .join("verifiers")
            .join(verification_id)
            .join("package");
        tokio::fs::create_dir_all(&package_path)
            .await
            .map_err(|error| error.to_string())?;
        let snapshot = self
            .store
            .get_verification_snapshot(&case.case_id)
            .await
            .map_err(|error| error.to_string())?;
        let pinned = self
            .read_pinned_lean_files()
            .await
            .map_err(|error| error.to_string())?;
        let files = [
            (
                "semantic_contract.json",
                serde_json::to_vec_pretty(semantic_contract).map_err(|error| error.to_string())?,
            ),
            (
                "formalization.json",
                serde_json::to_vec_pretty(formalization).map_err(|error| error.to_string())?,
            ),
            (
                "alignment_review.json",
                serde_json::to_vec_pretty(alignment).map_err(|error| error.to_string())?,
            ),
            (
                "backend_run.json",
                serde_json::to_vec_pretty(backend_run).map_err(|error| error.to_string())?,
            ),
            (
                "Verification.lean",
                formalization.lean_source.as_bytes().to_vec(),
            ),
        ];
        let mut file_entries = serde_json::Map::new();
        for (filename, bytes) in files {
            tokio::fs::write(package_path.join(filename), &bytes)
                .await
                .map_err(|error| error.to_string())?;
            file_entries.insert(
                filename.into(),
                serde_json::json!({"sha256": hex::encode(Sha256::digest(&bytes)), "size": bytes.len()}),
            );
        }
        let manifest = serde_json::json!({
            "schema_version": 1,
            "case_id": case.case_id,
            "verification_id": verification_id,
            "snapshot_hash": snapshot.content_hash,
            "semantic_contract_hash": semantic_contract.content_hash,
            "formalization_source_hash": formalization.source_hash,
            "backend": backend_run.backend,
            "backend_version": backend_run.backend_version,
            "axioms": backend_run.axioms,
            "pinned_toolchain": pinned,
            "files": file_entries,
        });
        tokio::fs::write(
            package_path.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
        )
        .await
        .map_err(|error| error.to_string())?;
        Ok((manifest, package_path))
    }

    async fn verify_package_integrity(
        &self,
        package: &research_domain::VerificationPackage,
    ) -> CoreResult<String> {
        let root = PathBuf::from(&package.storage_path);
        let manifest_bytes = tokio::fs::read(root.join("manifest.json")).await?;
        let manifest: Value = serde_json::from_slice(&manifest_bytes)?;
        let files = manifest
            .get("files")
            .and_then(Value::as_object)
            .ok_or_else(|| CoreError::InvalidAgentOutput("package manifest has no files".into()))?;
        for (filename, metadata) in files {
            let expected = metadata
                .get("sha256")
                .and_then(Value::as_str)
                .ok_or_else(|| CoreError::InvalidAgentOutput("package file has no hash".into()))?;
            let bytes = tokio::fs::read(root.join(filename)).await?;
            let observed = hex::encode(Sha256::digest(&bytes));
            if observed != expected {
                return Ok(format!("file_mismatch:{filename}:{observed}"));
            }
        }
        Ok(sha256_json(&manifest)?)
    }

    async fn run_agent_counted(&self, request: CountedAgentRun<'_>) -> CoreResult<AgentRunResult> {
        let CountedAgentRun {
            handle,
            task,
            project,
            round,
            worker_id,
            task_id,
            cancellation,
        } = request;
        self.task_cancellations
            .lock()
            .await
            .insert(handle.handle_id.clone(), cancellation.clone());
        let reservation = match self
            .store
            .reserve_model_call(ModelCallRequest {
                project_id: &project.project_id,
                round,
                worker_id,
                task_id,
                model: self.config.model.as_deref(),
                max_total_calls: project.budget.max_total_model_calls,
                max_task_calls: project.budget.max_model_calls_per_task,
            })
            .await
        {
            Ok(reservation) => reservation,
            Err(error) => {
                self.task_cancellations
                    .lock()
                    .await
                    .remove(&handle.handle_id);
                return Err(error.into());
            }
        };
        let started = Instant::now();
        let result = self.backend.run(handle, task, cancellation).await;
        self.task_cancellations
            .lock()
            .await
            .remove(&handle.handle_id);
        let elapsed_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
        match result {
            Ok(result) => {
                self.store
                    .finish_model_call_with_tokens(
                        &reservation.usage_id,
                        elapsed_ms,
                        result.input_tokens,
                        result.output_tokens,
                    )
                    .await?;
                Ok(result)
            }
            Err(error) => {
                self.store
                    .finish_model_call(&reservation.usage_id, elapsed_ms)
                    .await?;
                Err(error.into())
            }
        }
    }

    async fn write_latest_files(
        &self,
        project_id: &str,
        round: i64,
        content: &str,
    ) -> CoreResult<()> {
        let project_root = self.config.output_root.join(project_id);
        tokio::fs::create_dir_all(project_root.join("rounds")).await?;
        tokio::fs::create_dir_all(project_root.join("results")).await?;
        tokio::fs::create_dir_all(project_root.join("graphs")).await?;
        tokio::fs::create_dir_all(project_root.join("literature")).await?;
        tokio::fs::create_dir_all(project_root.join("final")).await?;
        tokio::fs::write(project_root.join("LATEST.md"), content).await?;
        for (graph_type, filename) in [
            ("goals", "goal_graph.json"),
            ("hypotheses", "hypothesis_graph.json"),
            ("facts", "fact_graph.json"),
            ("combined", "combined_graph.json"),
        ] {
            let graph = self.store.graph(project_id, graph_type).await?;
            tokio::fs::write(
                project_root.join("graphs").join(filename),
                serde_json::to_vec_pretty(&graph)?,
            )
            .await?;
        }
        let sources = self.store.list_sources(project_id).await?;
        tokio::fs::write(
            project_root.join("literature").join("source_ledger.json"),
            serde_json::to_vec_pretty(&sources)?,
        )
        .await?;
        let mut bibliography = String::from(
            "# Source ledger\n\n`reported_unverified` entries are research leads only. `admitted` means the source was cited by an accepted Fact after the mandatory citation review; only admitted entries may be used by the paper writer.\n",
        );
        for source in &sources {
            let authors = if source.authors.is_empty() {
                String::new()
            } else {
                format!(" — {}", source.authors.join(", "))
            };
            let url = source
                .url
                .as_ref()
                .map(|value| format!(" — {value}"))
                .unwrap_or_default();
            let _ = writeln!(
                bibliography,
                "\n- `{}` [{}] {}{authors}{url}",
                source.source_id, source.status, source.title,
            );
        }
        tokio::fs::write(
            project_root.join("literature").join("bibliography.md"),
            bibliography,
        )
        .await?;
        if round > 0 {
            let round_root = project_root
                .join("rounds")
                .join(format!("round_{round:03}"));
            tokio::fs::create_dir_all(&round_root).await?;
            tokio::fs::write(round_root.join("summary.md"), content).await?;
        }
        Ok(())
    }

    async fn cancel_affected(&self, command: &HumanCommand) {
        let tokens = self.task_cancellations.lock().await;
        if command.command_type == "stop_project" {
            for token in tokens.values() {
                token.cancel();
            }
            return;
        }
        for affected in &command.affected_entities {
            if affected.kind == "task" {
                if let Some(token) = tokens.get(&affected.id) {
                    token.cancel();
                }
            }
        }
    }

    fn publish(&self, event: DomainEvent) {
        let _ = self.event_bus.send(event);
    }

    fn publish_all(&self, events: Vec<DomainEvent>) {
        for event in events {
            self.publish(event);
        }
    }
}

fn source_archival_failure(source: &SourceDraft, reason: impl Into<String>) -> FailureDraft {
    let reason = reason.into();
    FailureDraft {
        failure_type: "source_fulltext_archival".into(),
        summary: format!("Source '{}' was not admitted: {reason}", source.title),
        repairable: true,
    }
}

const MAX_SOURCE_FULLTEXT_BYTES: usize = 50 * 1024 * 1024;
const MAX_SOURCE_REDIRECTS: usize = 5;

async fn fetch_public_source_fulltext(url: &str) -> Result<(String, Vec<u8>, String), String> {
    let mut current = reqwest::Url::parse(url).map_err(|_| "invalid_url".to_owned())?;
    for redirect_count in 0..=MAX_SOURCE_REDIRECTS {
        let (client, validated) = public_https_client(&current).await?;
        let response = client
            .get(validated.clone())
            .send()
            .await
            .map_err(|_| "request_failed".to_owned())?;
        if response.status().is_redirection() {
            if redirect_count == MAX_SOURCE_REDIRECTS {
                return Err("too_many_redirects".into());
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| "redirect_missing_location".to_owned())?;
            current = validated
                .join(location)
                .map_err(|_| "invalid_redirect_url".to_owned())?;
            continue;
        }
        if !response.status().is_success() {
            return Err(format!("http_status_{}", response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|length| length == 0 || length > MAX_SOURCE_FULLTEXT_BYTES as u64)
        {
            return Err("content_length_out_of_bounds".into());
        }
        let media_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !source_media_type_allowed(&media_type, &validated) {
            return Err("unsupported_content_type".into());
        }
        let filename = safe_source_filename(&validated, &media_type);
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "response_body_failed".to_owned())?;
            if bytes.len().saturating_add(chunk.len()) > MAX_SOURCE_FULLTEXT_BYTES {
                return Err("response_body_too_large".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err("empty_response_body".into());
        }
        let hash = hex::encode(Sha256::digest(&bytes));
        return Ok((filename, bytes, hash));
    }
    Err("too_many_redirects".into())
}

async fn public_https_client(
    url: &reqwest::Url,
) -> Result<(reqwest::Client, reqwest::Url), String> {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
    {
        return Err("only_public_https_port_443_without_credentials_is_allowed".into());
    }
    let host = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| "url_host_missing".to_owned())?;
    let addresses = tokio::net::lookup_host((host, 443))
        .await
        .map_err(|_| "dns_resolution_failed".to_owned())?
        .collect::<Vec<SocketAddr>>();
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| !is_allowed_egress_ip(address.ip()))
    {
        return Err("dns_resolved_to_non_public_address".into());
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent("math-research-agent/0.1 source-archiver")
        .resolve_to_addrs(host, &addresses)
        .build()
        .map_err(|_| "http_client_build_failed".to_owned())?;
    Ok((client, url.clone()))
}

fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let octets = address.octets();
            !(address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_multicast()
                || address.is_broadcast()
                || address.is_unspecified()
                || octets[0] == 0
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
                || (octets[0] == 198 && (18..=19).contains(&octets[1]))
                || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
                || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
                || octets[0] >= 240)
        }
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(mapped));
            }
            let segments = address.segments();
            !(address.is_loopback()
                || address.is_unspecified()
                || address.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
                || (segments[0] == 0x2001 && segments[1] == 0x0db8))
        }
    }
}

fn is_allowed_egress_ip(address: IpAddr) -> bool {
    is_public_ip(address)
        || (std::env::var_os("CODEX_CI").is_some() && is_codex_synthetic_egress(address))
}

fn is_codex_synthetic_egress(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let octets = address.octets();
            octets[0] == 198 && (18..=19).contains(&octets[1])
        }
        IpAddr::V6(_) => false,
    }
}

fn source_media_type_allowed(media_type: &str, url: &reqwest::Url) -> bool {
    matches!(
        media_type,
        "application/pdf"
            | "application/octet-stream"
            | "application/x-tex"
            | "application/xhtml+xml"
            | "text/html"
            | "text/plain"
            | "text/x-tex"
    ) || (media_type.is_empty()
        && url.path().rsplit('.').next().is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "pdf" | "tex" | "txt" | "html" | "htm"
            )
        }))
}

fn safe_source_filename(url: &reqwest::Url, media_type: &str) -> String {
    let candidate = url
        .path_segments()
        .and_then(Iterator::last)
        .filter(|name| !name.is_empty())
        .unwrap_or("source");
    let mut safe = candidate
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || ".-_".contains(*character))
        .take(120)
        .collect::<String>();
    if safe.is_empty() || !safe.contains('.') {
        let extension = match media_type {
            "application/pdf" => "pdf",
            "application/x-tex" | "text/x-tex" => "tex",
            "text/plain" => "txt",
            _ => "html",
        };
        safe = format!("{safe}source.{extension}");
    }
    safe
}

fn safe_local_filename(filename: &str) -> String {
    let safe = filename
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || ".-_".contains(*character))
        .take(120)
        .collect::<String>();
    if safe.is_empty() {
        "source.bin".into()
    } else {
        safe
    }
}

async fn load_source_fulltext(
    root: &Path,
    source: &SourceDraft,
) -> Result<(String, Vec<u8>, String), String> {
    let relative_path = Path::new(
        source
            .fulltext_path
            .as_deref()
            .ok_or_else(|| "missing_fulltext_path".to_owned())?,
    );
    if relative_path.is_absolute() {
        return Err("fulltext_path_must_be_relative".into());
    }
    let path = tokio::fs::canonicalize(root.join(relative_path))
        .await
        .map_err(|_| "fulltext_path_missing_or_unreadable".to_owned())?;
    if !path.starts_with(root) {
        return Err("fulltext_path_escaped_worker_directory".into());
    }
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|_| "fulltext_metadata_unreadable".to_owned())?;
    if !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_SOURCE_FULLTEXT_BYTES as u64
    {
        return Err("fulltext_must_be_nonempty_file_at_most_50_mib".into());
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| "fulltext_file_unreadable".to_owned())?;
    let actual_hash = hex::encode(Sha256::digest(&bytes));
    let expected_hash = source
        .fulltext_sha256
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "fulltext_sha256_missing".to_owned())?;
    if !actual_hash.eq_ignore_ascii_case(expected_hash.trim()) {
        return Err("fulltext_sha256_mismatch".into());
    }
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "fulltext_filename_invalid".to_owned())?;
    Ok((filename.to_owned(), bytes, actual_hash))
}

fn deterministic_tactic_candidates(hints: &[ProofHint]) -> (Vec<TacticCandidate>, HashSet<String>) {
    let mut candidates = Vec::new();
    let mut avoid = HashSet::new();
    for hint in hints {
        let Some((kind, value)) = hint.content.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match kind.trim() {
            "use_lemma" if !value.is_empty() => {
                candidates.push(tactic_candidate(
                    format!("exact {value}"),
                    0.9,
                    vec![value.into()],
                ));
                candidates.push(tactic_candidate(
                    format!("simpa using {value}"),
                    0.85,
                    vec![value.into()],
                ));
                candidates.push(tactic_candidate(
                    format!("rw [{value}]"),
                    0.7,
                    vec![value.into()],
                ));
            }
            "unfold_definition" if !value.is_empty() => {
                candidates.push(tactic_candidate(
                    format!("unfold {value}"),
                    0.65,
                    vec![value.into()],
                ));
            }
            "try_strategy" | "add_intermediate" if !value.is_empty() => {
                candidates.push(tactic_candidate(value.into(), 0.75, vec![]));
            }
            "avoid_tactic" if !value.is_empty() => {
                avoid.insert(value.into());
            }
            _ => {}
        }
    }
    for (tactic, score) in [
        ("assumption", 0.95),
        ("rfl", 0.9),
        ("simp", 0.82),
        ("norm_num", 0.8),
        ("omega", 0.78),
        ("aesop", 0.72),
    ] {
        candidates.push(tactic_candidate(tactic.into(), score, vec![]));
    }
    (candidates, avoid)
}

fn tactic_candidate(
    tactic: String,
    expected_goal_reduction: f64,
    premise_names: Vec<String>,
) -> TacticCandidate {
    TacticCandidate {
        rationale_summary: "deterministic_or_human_hint_candidate".into(),
        tactic,
        expected_goal_reduction,
        premise_names,
    }
}

fn safe_tactic(tactic: &str) -> bool {
    let trimmed = tactic.trim();
    if trimmed.is_empty() || trimmed.len() > 4096 {
        return false;
    }
    let tokens = trimmed
        .split(|character: char| {
            !(character.is_alphanumeric() || character == '_' || character == '\'')
        })
        .map(str::to_ascii_lowercase)
        .collect::<HashSet<_>>();
    ![
        "sorry",
        "admit",
        "axiom",
        "unsafe",
        "extern",
        "foreign",
        "native_decide",
        "run_tac",
    ]
    .iter()
    .any(|forbidden| tokens.contains(*forbidden))
}

fn proof_goal_summary(goals: &[research_worker_runtime::ProofGoal]) -> String {
    if goals.is_empty() {
        return "no_open_goals".into();
    }
    goals
        .iter()
        .enumerate()
        .map(|(index, goal)| {
            if goal.local_context.is_empty() {
                format!("goal_{}: {}", index + 1, goal.target)
            } else {
                format!(
                    "goal_{}: {} |- {}",
                    index + 1,
                    goal.local_context.join(", "),
                    goal.target
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn proof_state_signature(goals: &[research_worker_runtime::ProofGoal]) -> String {
    let mut canonical = goals
        .iter()
        .map(|goal| {
            format!(
                "{}|{}",
                goal.local_context.join(";"),
                goal.target.split_whitespace().collect::<String>()
            )
        })
        .collect::<Vec<_>>();
    canonical.sort();
    hex::encode(Sha256::digest(canonical.join("\n").as_bytes()))
}

fn proof_state_score(
    goals: &[research_worker_runtime::ProofGoal],
    depth: i64,
    expected_reduction: f64,
) -> f64 {
    let goal_count = u32::try_from(goals.len()).unwrap_or(u32::MAX);
    let bounded_depth = i32::try_from(depth.max(0)).unwrap_or(i32::MAX);
    let goal_bonus = 1.0 / (1.0 + f64::from(goal_count));
    let depth_penalty = f64::from(bounded_depth) * 0.01;
    expected_reduction.mul_add(0.6, goal_bonus * 0.4) - depth_penalty
}

fn proof_source(formalization: &Formalization, tactics: &[String]) -> String {
    let mut source = format!(
        "import Mathlib\n\ntheorem {} : {} := by\n",
        formalization.theorem_name, formalization.lean_statement
    );
    for tactic in tactics {
        for line in tactic.lines() {
            let _ = writeln!(source, "  {}", line.trim());
        }
    }
    source
}

fn compress_proof_failures(failures: &[String]) -> String {
    let mut unique = HashSet::new();
    let selected = failures
        .iter()
        .rev()
        .filter(|failure| unique.insert((*failure).clone()))
        .take(24)
        .cloned()
        .collect::<Vec<_>>();
    if selected.is_empty() {
        "搜索队列耗尽，未产生可用 tactic 状态转移".into()
    } else {
        format!("最近且去重后的失败知识：{}", selected.join(" | "))
    }
}

#[allow(clippy::too_many_lines)]
fn deterministic_precheck(
    submission: &CandidateSubmission,
    task: &Task,
    dependencies: &[research_domain::Fact],
    sources: &[SourceRecord],
    target_goals: &[Goal],
) -> Option<VerificationReport> {
    let proof = submission.proof_markdown.trim();
    let lower_proof = proof.to_lowercase();
    let placeholders = [
        "todo",
        "tbd",
        "待证明",
        "此处略",
        "proof omitted",
        "类似可得",
        "同理可得",
        "显然可得",
    ];
    let mut critical_errors = Vec::new();
    if submission.statement.trim().is_empty() || proof.is_empty() {
        critical_errors.push("empty_required_field".to_owned());
    }
    if submission.statement.len() > 100_000 || submission.proof_markdown.len() > 2_000_000 {
        critical_errors.push("candidate_size_limit_exceeded".to_owned());
    }
    if placeholders
        .iter()
        .any(|placeholder| lower_proof.contains(placeholder))
    {
        critical_errors.push("proof_placeholder".to_owned());
    }
    let unique_dependencies = submission
        .dependency_fact_ids
        .iter()
        .collect::<HashSet<_>>();
    if unique_dependencies.len() != submission.dependency_fact_ids.len() {
        critical_errors.push("duplicate_fact_dependency".to_owned());
    }
    let loaded_dependency_ids = dependencies
        .iter()
        .map(|fact| fact.fact_id.as_str())
        .collect::<HashSet<_>>();
    if dependencies
        .iter()
        .any(|fact| fact.status != research_domain::FactStatus::ActiveFact)
        || unique_dependencies
            .iter()
            .any(|fact_id| !loaded_dependency_ids.contains(fact_id.as_str()))
    {
        critical_errors.push("inactive_or_missing_fact_dependency".to_owned());
    }
    if !task.goal_ids.is_empty()
        && submission
            .target_goal_ids
            .iter()
            .any(|goal_id| !task.goal_ids.contains(goal_id))
    {
        critical_errors.push("candidate_target_outside_task_scope".to_owned());
    }
    let unique_target_goal_ids = submission.target_goal_ids.iter().collect::<HashSet<_>>();
    if unique_target_goal_ids.len() != submission.target_goal_ids.len() {
        critical_errors.push("duplicate_target_goal".to_owned());
    }
    let loaded_target_goal_ids = target_goals
        .iter()
        .map(|goal| goal.goal_id.as_str())
        .collect::<HashSet<_>>();
    if unique_target_goal_ids
        .iter()
        .any(|goal_id| !loaded_target_goal_ids.contains(goal_id.as_str()))
    {
        critical_errors.push("missing_target_goal".to_owned());
    }
    if submission.candidate_type == research_domain::CandidateType::Counterexample
        && submission.target_goal_ids.is_empty()
    {
        critical_errors.push("counterexample_without_target_goal".to_owned());
    }
    if sources.len() != submission.external_source_ids.len() {
        critical_errors.push("missing_source_dependency".to_owned());
    }
    if sources.iter().any(|source| {
        source.title.trim().is_empty()
            || source.applicability.trim().is_empty()
            || (source.url.as_deref().is_none_or(str::is_empty)
                && source.citation_key.as_deref().is_none_or(str::is_empty))
    }) {
        critical_errors.push("source_without_stable_identifier".to_owned());
    }
    if sources
        .iter()
        .any(|source| source.fulltext_artifact_id.is_none())
    {
        critical_errors.push("source_fulltext_artifact_missing".to_owned());
    }
    if !critical_errors.is_empty() {
        return Some(VerificationReport {
            verdict: VerificationVerdict::Rejected,
            summary: format!("确定性预检查拒绝候选：{}", critical_errors.join(", ")),
            critical_errors,
            gaps: vec![],
            uncertainties: vec![],
            repair_actions: vec![
                "修复结构、任务范围、依赖或来源标识后，以新候选版本重新提交".into(),
            ],
            checked_fact_ids: submission.dependency_fact_ids.clone(),
            checked_source_ids: submission.external_source_ids.clone(),
            evidence_level: "deterministic_precheck".into(),
        });
    }
    None
}

fn required_checks(
    has_sources: bool,
    formal_required: bool,
    independent_proof_requested: bool,
) -> Vec<String> {
    let mut checks = vec![
        "deterministic_precheck".into(),
        "math_review_1".into(),
        "math_review_2".into(),
        "reviewer_independence".into(),
        "adversarial_review".into(),
    ];
    if has_sources {
        checks.push("citation_review".into());
    }
    if independent_proof_requested {
        checks.push("math_review_3".into());
    }
    if formal_required {
        checks.extend([
            "semantic_contract".into(),
            "alignment_review".into(),
            "lean_kernel".into(),
            "package_integrity".into(),
            "fresh_replay".into(),
        ]);
    }
    checks
}

fn requires_formal_verification(project: &Project, submission: &CandidateSubmission) -> bool {
    statements_match_target(&project.contract.target_statement, &submission.statement)
        || submission.candidate_type == research_domain::CandidateType::Counterexample
}

fn v1_risk_score(project: &Project, submission: &CandidateSubmission) -> f64 {
    let mut score: f64 = 0.25;
    if statements_match_target(&project.contract.target_statement, &submission.statement) {
        score += 0.45;
    }
    if !submission.external_source_ids.is_empty() {
        score += 0.15;
    }
    if !submission.dependency_fact_ids.is_empty() {
        score += 0.10;
    }
    score.min(1.0)
}

fn v1_risk_reasons(project: &Project, submission: &CandidateSubmission) -> Vec<String> {
    let mut reasons = Vec::new();
    if statements_match_target(&project.contract.target_statement, &submission.statement) {
        reasons.push("candidate_matches_main_target".into());
    }
    if !submission.external_source_ids.is_empty() {
        reasons.push("depends_on_external_sources".into());
    }
    if !submission.dependency_fact_ids.is_empty() {
        reasons.push("has_fact_dependencies".into());
    }
    if reasons.is_empty() {
        reasons.push("intermediate_claim".into());
    }
    reasons
}

fn normalized_statement(statement: &str) -> String {
    statement
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn statements_match_target(target: &str, candidate: &str) -> bool {
    const PROOF_REQUEST_PREFIXES: [&str; 6] = [
        "proveordisprovethat",
        "proveordisprovewhether",
        "provethat",
        "showthat",
        "证明或否证",
        "证明",
    ];
    let target = normalized_statement(target);
    let candidate = normalized_statement(candidate);
    if target == candidate {
        return true;
    }
    PROOF_REQUEST_PREFIXES.iter().any(|prefix| {
        target
            .strip_prefix(prefix)
            .is_some_and(|claim| claim == candidate)
    })
}

const fn verdict_check_status(verdict: VerificationVerdict) -> CheckStatus {
    match verdict {
        VerificationVerdict::Accepted => CheckStatus::Passed,
        VerificationVerdict::Rejected => CheckStatus::Failed,
        VerificationVerdict::Unknown => CheckStatus::Unknown,
    }
}

fn reviewer_role_contract(reviewer_kind: &str) -> &'static str {
    match reviewer_kind {
        "math_review_1" => {
            "执行正向证明审计：按候选证明的原顺序建立逐步义务表，逐项核对量词、假设、定义域、推理方向和依赖 Fact；明确指出第一个不能从前提推出的步骤。只审查候选自身，目标覆盖由单独的 goal_coverage_review 裁决。"
        }
        "math_review_2" => {
            "执行反向证伪审计：从候选结论的否定、最小反例和极端边界出发，主动寻找量词交换、遗漏退化情形、隐含有限性/非空性以及不可逆推理；再回到原证明判断这些攻击是否被排除。只审查候选自身，目标覆盖由单独检查负责。"
        }
        "math_review_3" => {
            "执行独立重构审计：先遮蔽候选给出的证明正文，仅从精确陈述、明示假设和 active Facts 尝试构造一条独立证明或反例；随后再与候选证明比较关键桥梁。无法独立重构时必须返回 unknown，不得把措辞相似当作独立证明。"
        }
        "citation_review" => {
            "逐一打开 source_snapshot.citation_review_material.local_fulltext_path 指向的本地冻结全文，核对 SHA-256、定理定位、原文陈述、假设、适用范围和候选中的实际用法；缺少 citation_review_material、文件不可读、定位失败或来源无法支撑时不得 accepted。"
        }
        "adversarial_review" => {
            "主动攻击候选自身的陈述与证明，寻找反例、偷换定义、循环论证、隐含假设和从有限陈述越界到全局陈述；若候选只明示一个正确的中间或有限结论，不得仅因它未闭合主目标而 rejected，目标覆盖由单独检查负责。"
        }
        "goal_coverage_review" => {
            "只判断候选结论连同列出的 active 依赖是否在不增加、替换或削弱假设的前提下完整蕴含每个 target goal。等价改写或更强结论可以 accepted；只证明特例、中间引理、必要条件或部分方向必须 rejected；信息不足必须 unknown。此裁决只决定目标闭合，不否定候选本身可能是正确的中间事实。"
        }
        _ => "独立核查候选，不得以风格偏好替代数学判断。",
    }
}

#[derive(Debug, serde::Serialize)]
struct ReviewerIndependenceAssessment {
    summary: String,
    reviewer_fingerprints: Vec<Value>,
    duplicate_groups: Vec<Vec<String>>,
}

fn assess_reviewer_independence(
    reviewer_reports: &[(String, VerificationReport)],
) -> ReviewerIndependenceAssessment {
    let mut by_fingerprint = std::collections::BTreeMap::<String, Vec<String>>::new();
    let mut reviewer_fingerprints = Vec::new();
    for (kind, report) in reviewer_reports
        .iter()
        .filter(|(kind, _)| kind.starts_with("math_review_"))
    {
        let normalized_summary = normalized_statement(&report.summary);
        let mut critical_errors = report.critical_errors.clone();
        critical_errors.sort();
        let mut gaps = report
            .gaps
            .iter()
            .map(|gap| serde_json::to_string(gap).unwrap_or_default())
            .collect::<Vec<_>>();
        gaps.sort();
        let mut uncertainties = report.uncertainties.clone();
        uncertainties.sort();
        let fingerprint_payload = serde_json::json!({
            "verdict":report.verdict,
            "summary":normalized_summary,
            "critical_errors":critical_errors,
            "gaps":gaps,
            "uncertainties":uncertainties,
        });
        let fingerprint = hex::encode(Sha256::digest(
            serde_json::to_vec(&fingerprint_payload).unwrap_or_default(),
        ));
        by_fingerprint
            .entry(fingerprint.clone())
            .or_default()
            .push(kind.clone());
        reviewer_fingerprints.push(serde_json::json!({
            "reviewer_kind":kind,
            "fingerprint":fingerprint,
        }));
    }
    let duplicate_groups = by_fingerprint
        .into_values()
        .filter(|kinds| kinds.len() > 1)
        .collect::<Vec<_>>();
    let summary = if duplicate_groups.is_empty() {
        "数学审查角色使用不同审计契约，且规范化审查结果没有完全重复。".into()
    } else {
        format!(
            "检测到完全重复的数学审查结果，不能把这些运行计作认知独立：{}",
            duplicate_groups
                .iter()
                .map(|group| group.join("/"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    ReviewerIndependenceAssessment {
        summary,
        reviewer_fingerprints,
        duplicate_groups,
    }
}

fn adjudicate_reviews(
    submission: &CandidateSubmission,
    reviews: &[VerificationReport],
) -> VerificationReport {
    let rejected = reviews
        .iter()
        .filter(|review| review.verdict == VerificationVerdict::Rejected)
        .count();
    let unknown = reviews
        .iter()
        .filter(|review| review.verdict == VerificationVerdict::Unknown)
        .count();
    let verdict = if rejected > 0 {
        VerificationVerdict::Rejected
    } else if unknown > 0 || reviews.is_empty() {
        VerificationVerdict::Unknown
    } else {
        VerificationVerdict::Accepted
    };
    let summaries = reviews
        .iter()
        .enumerate()
        .map(|(index, review)| {
            format!(
                "review_{}({}): {}",
                index + 1,
                review.verdict,
                review.summary
            )
        })
        .collect::<Vec<_>>();
    VerificationReport {
        verdict,
        summary: format!(
            "结构化裁决：{} 个独立检查，{} 个拒绝，{} 个未知。{}",
            reviews.len(),
            rejected,
            unknown,
            summaries.join(" | ")
        ),
        critical_errors: reviews
            .iter()
            .flat_map(|review| review.critical_errors.clone())
            .collect(),
        gaps: reviews
            .iter()
            .flat_map(|review| review.gaps.clone())
            .collect(),
        uncertainties: reviews
            .iter()
            .flat_map(|review| review.uncertainties.clone())
            .collect(),
        repair_actions: reviews
            .iter()
            .flat_map(|review| review.repair_actions.clone())
            .collect(),
        checked_fact_ids: submission.dependency_fact_ids.clone(),
        checked_source_ids: submission.external_source_ids.clone(),
        evidence_level: if verdict == VerificationVerdict::Accepted {
            AcceptanceClass::Reviewed.to_string()
        } else {
            "review_inconclusive".into()
        },
    }
}

fn certification_unavailable_report(
    submission: &CandidateSubmission,
    reviews: &[VerificationReport],
    reason: &str,
) -> VerificationReport {
    let mut report = adjudicate_reviews(submission, reviews);
    report.verdict = VerificationVerdict::Unknown;
    report.summary = format!("自然语言审查未发现阻断问题，但未达到强制形式认证等级：{reason}");
    report.uncertainties.push(reason.into());
    report
        .repair_actions
        .push("repair_or_retry_formal_certification".into());
    report.evidence_level = "formal_certification_incomplete".into();
    report
}

async fn stage_sources_for_citation_review(
    service: &ResearchService,
    project_id: &str,
    working_directory: &Path,
    sources: &[SourceRecord],
) -> Value {
    let source_directory = working_directory.join("sources");
    let canonical_artifact_root = tokio::fs::canonicalize(service.store.artifact_root())
        .await
        .ok();
    let mut packet = Vec::with_capacity(sources.len());
    for source in sources {
        let mut entry = serde_json::to_value(source)
            .unwrap_or_else(|_| serde_json::json!({"source_id":source.source_id}));
        let staged = match source.fulltext_artifact_id.as_deref() {
            Some(artifact_id) => {
                async {
                    let artifact = service
                        .store
                        .get_artifact(project_id, artifact_id)
                        .await
                        .map_err(|_| "fulltext_artifact_missing")?;
                    if artifact.kind != "source_fulltext" {
                        return Err("artifact_is_not_source_fulltext");
                    }
                    let root = canonical_artifact_root
                        .as_deref()
                        .ok_or("artifact_root_unreadable")?;
                    let path = tokio::fs::canonicalize(&artifact.storage_path)
                        .await
                        .map_err(|_| "fulltext_artifact_unreadable")?;
                    if !path.starts_with(root) {
                        return Err("fulltext_artifact_escaped_root");
                    }
                    let bytes = tokio::fs::read(path)
                        .await
                        .map_err(|_| "fulltext_artifact_unreadable")?;
                    if bytes.is_empty() || bytes.len() > MAX_SOURCE_FULLTEXT_BYTES {
                        return Err("fulltext_artifact_size_invalid");
                    }
                    let hash = hex::encode(Sha256::digest(&bytes));
                    if !hash.eq_ignore_ascii_case(&artifact.sha256) {
                        return Err("fulltext_artifact_hash_mismatch");
                    }
                    tokio::fs::create_dir_all(&source_directory)
                        .await
                        .map_err(|_| "citation_source_directory_unavailable")?;
                    let filename = format!(
                        "{}-{}",
                        source.source_id,
                        safe_local_filename(&artifact.filename)
                    );
                    let staged_path = source_directory.join(filename);
                    tokio::fs::write(&staged_path, bytes)
                        .await
                        .map_err(|_| "citation_source_staging_failed")?;
                    Ok::<_, &'static str>(serde_json::json!({
                        "local_fulltext_path":staged_path.to_string_lossy(),
                        "sha256":hash,
                        "media_type":artifact.media_type,
                        "size":artifact.size,
                    }))
                }
                .await
            }
            None => Err("source_fulltext_artifact_missing"),
        };
        if let Some(object) = entry.as_object_mut() {
            match staged {
                Ok(value) => {
                    object.insert("citation_review_material".into(), value);
                }
                Err(reason) => {
                    object.insert(
                        "citation_review_material_error".into(),
                        Value::String(reason.into()),
                    );
                }
            }
        }
        packet.push(entry);
    }
    Value::Array(packet)
}

fn certified_report(
    submission: &CandidateSubmission,
    reviews: &[VerificationReport],
    package_id: &str,
    replay_id: &str,
) -> VerificationReport {
    let mut report = adjudicate_reviews(submission, reviews);
    report.summary = format!(
        "自然语言双审查、对抗审查、语义对齐、Lean 内核、公理审计、内容寻址验证包及独立新进程重放全部通过；package={package_id}, replay={replay_id}"
    );
    report.evidence_level = AcceptanceClass::FullyCertified.to_string();
    report
}

fn sha256_json(value: &Value) -> Result<String, serde_json::Error> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}

fn unavailable_verification_report(
    submission: &CandidateSubmission,
    reason: &str,
) -> VerificationReport {
    VerificationReport {
        verdict: VerificationVerdict::Unknown,
        summary: reason.into(),
        critical_errors: vec![],
        gaps: vec![],
        uncertainties: vec![reason.into()],
        repair_actions: vec!["retry_independent_verification".into()],
        checked_fact_ids: submission.dependency_fact_ids.clone(),
        checked_source_ids: submission.external_source_ids.clone(),
        evidence_level: "unknown".into(),
    }
}

fn validate_reflection(reflection: &ReflectionOutput, route_count: usize) -> CoreResult<()> {
    if reflection.reviews.len() != route_count {
        return Err(CoreError::InvalidAgentOutput(format!(
            "reflection returned {} reviews for {route_count} routes",
            reflection.reviews.len()
        )));
    }
    let indices = reflection
        .reviews
        .iter()
        .map(|review| review.route_index)
        .collect::<HashSet<_>>();
    if indices.len() != route_count || !indices.iter().all(|index| *index < route_count) {
        return Err(CoreError::InvalidAgentOutput(
            "reflection route indices are missing, duplicated, or out of range".into(),
        ));
    }
    Ok(())
}

fn primary_plan_is_executable(plan: &PlannerOutput) -> bool {
    !plan.routes.is_empty() && !plan.assignments.is_empty()
}

fn proximity_projection(routes: &mut [RouteProposal]) -> Value {
    let token_sets = routes.iter().map(route_tokens).collect::<Vec<_>>();
    let mut nearest = vec![0.0_f64; routes.len()];
    let mut pairs = Vec::new();
    for left in 0..routes.len() {
        for right in left + 1..routes.len() {
            let similarity = jaccard(&token_sets[left], &token_sets[right]);
            nearest[left] = nearest[left].max(similarity);
            nearest[right] = nearest[right].max(similarity);
            if similarity >= 0.45 {
                pairs.push(serde_json::json!({
                    "left_route_index":left,
                    "right_route_index":right,
                    "similarity":similarity,
                    "semantic_duplicate":similarity >= 0.82,
                }));
            }
        }
    }
    for (route, similarity) in routes.iter_mut().zip(nearest.iter()) {
        route.route_diversity = route.route_diversity.min(1.0 - similarity);
        if *similarity >= 0.82 {
            route.cost_penalty += 0.15;
        }
    }
    serde_json::json!({"related_pairs":pairs,"nearest_similarity":nearest})
}

fn route_tokens(route: &RouteProposal) -> HashSet<String> {
    format!(
        "{} {} {}",
        route.title,
        route.method_summary,
        route.expected_subgoals.join(" ")
    )
    .to_lowercase()
    .split(|character: char| !character.is_alphanumeric())
    .filter(|token| token.chars().count() >= 2)
    .map(str::to_owned)
    .collect()
}

fn jaccard(left: &HashSet<String>, right: &HashSet<String>) -> f64 {
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }
    let intersection =
        f64::from(u32::try_from(left.intersection(right).count()).unwrap_or(u32::MAX));
    let union = f64::from(u32::try_from(left.union(right).count()).unwrap_or(u32::MAX));
    if union == 0.0 {
        0.0
    } else {
        intersection / union
    }
}

fn reflection_route_is_policy_eligible(
    review: &research_domain::RouteReflection,
    dependencies_valid: bool,
) -> bool {
    dependencies_valid
        && !review.changes_problem
        && !review.uses_unverified_claims
        && !review.conflicts_with_facts
        && review.has_verifiable_milestone
}

fn rank_routes(
    routes: &[RouteProposal],
    reflection: &ReflectionOutput,
    weights: RankingWeights,
) -> Value {
    let mut ranked = routes
        .iter()
        .enumerate()
        .map(|(index, route)| {
            let review = reflection
                .reviews
                .iter()
                .find(|review| review.route_index == index);
            let reflection_penalty = review.map_or(0.0, |review| review.risk_score * 0.10)
                + review
                    .filter(|review| review.repeats_failure_pattern)
                    .map_or(0.0, |_| 0.15)
                + review
                    .filter(|review| !review.has_verifiable_milestone)
                    .map_or(0.0, |_| 0.15);
            let score = weights.expected_goal_progress * route.expected_goal_progress
                + weights.uncertainty_reduction * route.uncertainty_reduction
                + weights.human_suggestion_alignment * route.human_suggestion_alignment
                + weights.evidence_support * route.evidence_support
                + weights.route_diversity * route.route_diversity
                + weights.verifiability * route.verifiability
                + weights.novelty * route.novelty
                + weights.goal_closure_leverage
                    * review.map_or(0.0, |review| review.goal_closure_leverage)
                + weights.generality_gain * review.map_or(0.0, |review| review.generality_gain)
                + weights.bridge_centrality
                    * review.map_or(0.0, |review| review.bridge_centrality)
                + weights.architecture_fit * review.map_or(0.0, |review| review.architecture_fit)
                - route.failure_similarity_penalty
                - route.cost_penalty
                - weights.assumption_debt_penalty
                    * review.map_or(0.0, |review| review.assumption_debt)
                - weights.unjustified_narrowing_penalty
                    * review.map_or(0.0, |review| {
                        if review.unjustified_narrowing {
                            1.0
                        } else {
                            0.0
                        }
                    })
                - reflection_penalty;
            serde_json::json!({
                "route_index":index,
                "title":route.title,
                "score":score,
                "reflection_penalty":reflection_penalty,
                "components":{
                    "expected_goal_progress":route.expected_goal_progress,
                    "uncertainty_reduction":route.uncertainty_reduction,
                    "human_suggestion_alignment":route.human_suggestion_alignment,
                    "evidence_support":route.evidence_support,
                    "route_diversity":route.route_diversity,
                    "verifiability":route.verifiability,
                    "novelty":route.novelty,
                    "goal_closure_leverage":review.map_or(0.0, |review| review.goal_closure_leverage),
                    "generality_gain":review.map_or(0.0, |review| review.generality_gain),
                    "assumption_debt":review.map_or(0.0, |review| review.assumption_debt),
                    "bridge_centrality":review.map_or(0.0, |review| review.bridge_centrality),
                    "architecture_fit":review.map_or(0.0, |review| review.architecture_fit),
                    "unjustified_narrowing":review.is_some_and(|review| review.unjustified_narrowing),
                    "remaining_goal_gaps_if_successful":review.map_or_else(Vec::new, |review| review.remaining_goal_gaps_if_successful.clone()),
                    "failure_similarity_penalty":route.failure_similarity_penalty,
                    "cost_penalty":route.cost_penalty,
                }
            })
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right["score"]
            .as_f64()
            .unwrap_or(f64::NEG_INFINITY)
            .total_cmp(&left["score"].as_f64().unwrap_or(f64::NEG_INFINITY))
            .then_with(|| {
                left["route_index"]
                    .as_u64()
                    .cmp(&right["route_index"].as_u64())
            })
    });
    for (rank, route) in ranked.iter_mut().enumerate() {
        route["rank"] = serde_json::json!(rank + 1);
    }
    serde_json::json!({"weights":weights,"ranked_routes":ranked})
}

fn ensure_adversarial_assignment(routes: &[RouteProposal], supervisor: &mut SupervisorOutput) {
    if supervisor
        .assignments
        .iter()
        .any(|assignment| assignment.worker_role == "counterexample_hunter")
    {
        return;
    }
    let route_index = routes
        .iter()
        .enumerate()
        .find(|(_, route)| {
            let text = format!("{} {}", route.title, route.method_summary).to_lowercase();
            text.contains("counterexample")
                || text.contains("反例")
                || text.contains("边界")
                || text.contains("attack")
        })
        .map_or(0, |(index, _)| index);
    supervisor
        .assignments
        .push(research_domain::AssignmentDraft {
            route_index,
            worker_role: "counterexample_hunter".into(),
            strategic_role: "adversarial".into(),
            addresses_interface_debt: false,
            goal_ids: routes[route_index].target_goal_ids.clone(),
            objective: "独立攻击主张，系统检查反例、边界与隐藏假设".into(),
            completion_contract: "提交可验证反例候选，或记录覆盖范围和仍未排除的情形".into(),
            priority: 0.8,
        });
}

fn bind_strategy_to_assignments(
    strategy: &StrategyDirectorOutput,
    supervisor: &mut SupervisorOutput,
) {
    let skeleton = strategy
        .proof_skeleton
        .iter()
        .take(8)
        .enumerate()
        .map(|(index, step)| format!("{}. {}", index + 1, step.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    let debts = strategy
        .interface_debts
        .iter()
        .take(6)
        .map(|debt| {
            format!(
                "- {}: requires [{}], available [{}], missing [{}]; failure if ignored: {}",
                debt.interface_name.trim(),
                debt.input_required.trim(),
                debt.output_available.trim(),
                debt.missing_matches.join("; "),
                debt.failure_if_ignored.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let directives = strategy
        .strategy_directives
        .iter()
        .take(6)
        .map(|directive| format!("- {}", directive.trim()))
        .collect::<Vec<_>>()
        .join("\n");

    for assignment in &mut supervisor.assignments {
        if matches!(
            assignment.strategic_role.as_str(),
            "whole_architecture" | "central_bridge"
        ) {
            let original_objective = assignment.objective.trim().to_owned();
            assignment.objective = format!(
                "{original_objective}\n\n[Persisted Strategy Binding]\nFixed goal (do not weaken or replace): {}\nCentral missing bridge (primary work target): {}\nProof skeleton:\n{}\nOpen interface debts:\n{}\nStrategy directives:\n{}\n\
                 Mechanism probes required when applicable: (1) if minimality is available, assume the desired bridge object exists and decompose through strictly lower-complexity objects to derive a local obstruction; (2) if a local obstruction must be upgraded to an existential/global witness, test a maximal or extremal extension inside the admissible substructure and prove termination plus membership; (3) if recursion is not closed on the projected parameters, introduce the smallest auxiliary resource/coverage state and prove a one-step extension or covering lemma. These are proof-search obligations, not premises.",
                strategy.fixed_goal.trim(),
                strategy.central_missing_bridge.trim(),
                if skeleton.is_empty() {
                    "- no verified skeleton step is currently available"
                } else {
                    &skeleton
                },
                if debts.is_empty() {
                    "- no named interface debt is currently recorded"
                } else {
                    &debts
                },
                if directives.is_empty() {
                    "- preserve exact whole-goal coverage"
                } else {
                    &directives
                },
            );
            assignment.completion_contract = format!(
                "{}\nStrategy-bound completion: either (a) give a checkable derivation of the central missing bridge with every hypothesis matched to the proof skeleton, (b) give a checkable counterexample to that bridge, or (c) isolate the smallest exact residual obstruction and record which of the minimality, extremal-extension, and auxiliary-state probes fail and why. A restatement of the whole goal, a special case without a coverage reduction, or an additional unstated assumption is not completion. Every submitted candidate must be self-contained rather than referring to sibling Candidate numbers, and must explicitly discharge degenerate cases such as equal witness indices, zero/trivial objects, empty supports, and unavailable extrema whenever its construction can encounter them.",
                assignment.completion_contract.trim()
            );
            assignment.addresses_interface_debt = !strategy.interface_debts.is_empty();
        }

        if assignment.worker_role == "literature_researcher"
            && !strategy.literature_priorities.is_empty()
        {
            assignment.objective = format!(
                "{}\n\nAuthoritative literature priorities from the persisted Strategy State:\n{}",
                assignment.objective.trim(),
                strategy
                    .literature_priorities
                    .iter()
                    .take(8)
                    .map(|priority| format!("- {}", priority.trim()))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            assignment.completion_contract = format!(
                "{}\nFor every claimed theorem, return a stable source identifier, exact theorem/section locator, hypotheses, conclusion, and an explicit match or mismatch against the central bridge. Discovery alone does not make the source verified or the theorem a Fact.",
                assignment.completion_contract.trim()
            );
        }
    }
}

const fn planning_attempt_timeouts(max_hard_seconds: u64, attempt_number: i64) -> (u64, u64) {
    let max_hard_seconds = if max_hard_seconds == 0 {
        1
    } else {
        max_hard_seconds
    };
    let hard_seconds = if attempt_number <= 1 {
        let half = max_hard_seconds / 2;
        if half == 0 { 1 } else { half }
    } else {
        max_hard_seconds
    };
    let half_soft = hard_seconds / 2;
    let soft_seconds = if half_soft == 0 { 1 } else { half_soft };
    (soft_seconds, hard_seconds)
}

fn conservative_strategy_state(
    snapshot: &ProjectSnapshot,
    planning_context: &Value,
    previous: Option<&Value>,
    failure: &str,
) -> StrategyDirectorOutput {
    if let Some(mut prior) = previous
        .filter(|value| !value.is_null())
        .and_then(|value| serde_json::from_value::<StrategyDirectorOutput>(value.clone()).ok())
        .filter(|prior| {
            prior.fixed_goal.trim() == snapshot.project.contract.target_statement.trim()
                && !prior.proof_skeleton.is_empty()
                && !prior.central_missing_bridge.trim().is_empty()
        })
    {
        prior.verdict_summary = format!(
            "Conservative carry-forward after Strategy Director failure; no mathematical status was upgraded: {failure}"
        );
        prior.macro_replan_required = false;
        return prior;
    }

    let target = snapshot.project.contract.target_statement.clone();
    let central_bridge = planning_context["open_bottlenecks"]
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item.get("precise_statement"))
        .and_then(Value::as_str)
        .map_or_else(
            || format!("Construct a complete, assumption-preserving derivation of: {target}"),
            str::to_owned,
        );
    let route_portfolio = snapshot
        .routes
        .iter()
        .filter(|route| {
            matches!(
                route.status.to_string().as_str(),
                "incubating" | "active" | "blocked" | "probation" | "revived"
            )
        })
        .map(|route| research_domain::StrategyRouteState {
            title: route.title.clone(),
            mechanism: route.method_summary.clone(),
            mathematical_frontier: central_bridge.clone(),
            decisive_obstacle: "Not re-audited because the Strategy Director was unavailable"
                .into(),
            evidence_for: route.required_fact_ids.clone(),
            evidence_against: vec![],
            status: route.status.to_string(),
            revisit_condition: "Revisit at the next successful strategy audit".into(),
        })
        .collect();
    StrategyDirectorOutput {
        verdict_summary: format!(
            "Conservative strategy initialized after Strategy Director failure; no mathematical status was inferred: {failure}"
        ),
        fixed_goal: target.clone(),
        proof_skeleton: vec![format!(
            "Supply a complete derivation from the exact Problem Contract to {target}; all missing steps remain open"
        )],
        route_portfolio,
        interface_debts: vec![research_domain::StrategyInterfaceDebt {
            interface_name: "problem-contract-to-complete-proof".into(),
            input_required: "Only active Facts under the exact Problem Contract".into(),
            output_available: "No Strategy Director audit is available for this round".into(),
            missing_matches: vec![central_bridge.clone()],
            failure_if_ignored: "Local results could be mistaken for closure of the fixed goal".into(),
            affected_goal_ids: snapshot.goals.iter().map(|goal| goal.goal_id.clone()).collect(),
        }],
        central_missing_bridge: central_bridge,
        method_vs_proposition_failure: "undetermined".into(),
        dangerous_shortcuts: vec![
            "Do not infer whole-goal coverage from a special case, finite search, necessary condition, or one-way implication"
                .into(),
        ],
        strategy_directives: vec![
            "Preserve the fixed goal and prioritize a complete architecture or its central missing bridge"
                .into(),
        ],
        literature_priorities: vec![],
        macro_replan_required: false,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct StrategyAuditDecision {
    kind: &'static str,
    reasons: Vec<String>,
}

fn strategy_audit_decision(
    previous: Option<&Value>,
    states: &[Value],
    planning_context: &Value,
) -> StrategyAuditDecision {
    if previous.is_none() {
        return StrategyAuditDecision {
            kind: "initial",
            reasons: vec!["initial_strategy_state".into()],
        };
    }

    let delta = &planning_context["research_delta"];
    let mut reasons = Vec::new();
    let nonempty = |field: &str| {
        delta[field]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    };
    let count = |field: &str| delta[field].as_array().map_or(0, Vec::len);

    if nonempty("accepted_fact_ids") {
        reasons.push("accepted_fact_changed_global_premises".into());
    }
    if nonempty("solved_goal_ids") {
        reasons.push("goal_closed".into());
    }
    if nonempty("reopened_goal_ids") {
        reasons.push("goal_reopened".into());
    }
    if nonempty("new_failure_pattern_ids") {
        reasons.push("new_failure_pattern".into());
    }
    if nonempty("new_proof_debt_ids") {
        reasons.push("new_proof_debt".into());
    }
    if count("rejected_candidate_ids") >= 2 {
        reasons.push("repeated_candidate_rejection".into());
    }
    if count("failed_or_expired_attempt_ids") >= 2 {
        reasons.push("repeated_execution_failure".into());
    }
    if count("completed_task_attempt_ids") >= 2 && !nonempty("accepted_fact_ids") {
        reasons.push("multiple_completed_tasks_without_new_fact".into());
    }
    if nonempty("human_command_ids") {
        reasons.push("human_intervention".into());
    }
    if delta["route_state_changes"]
        .as_array()
        .is_some_and(|changes| {
            changes.iter().any(|change| {
                matches!(
                    change.get("event_type").and_then(Value::as_str),
                    Some(
                        "route.blocked"
                            | "route.pruned"
                            | "route.merged"
                            | "route.human_stopped"
                            | "route.revived"
                            | "route.invalidated"
                    )
                )
            })
        })
    {
        reasons.push("route_portfolio_state_changed".into());
    }

    let last_macro_index = states
        .iter()
        .rposition(|state| state.get("audit_kind").and_then(Value::as_str) == Some("macro"));
    let controls_since_macro =
        last_macro_index.map_or(states.len(), |index| states.len().saturating_sub(index + 1));
    let time_due = last_macro_index
        .and_then(|index| states.get(index))
        .and_then(|state| state.get("created_at"))
        .and_then(Value::as_str)
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .is_some_and(|created| Utc::now().signed_duration_since(created).num_hours() >= 4);
    if controls_since_macro >= 3 || time_due {
        reasons.push("periodic_macro_audit_due".into());
    }

    if reasons.is_empty() {
        StrategyAuditDecision {
            kind: "control",
            reasons: vec!["routine_round_control".into()],
        }
    } else {
        StrategyAuditDecision {
            kind: "macro",
            reasons,
        }
    }
}

fn planning_context(
    snapshot: &ProjectSnapshot,
    delta: &ResearchDelta,
    bottlenecks: &[research_domain::Bottleneck],
    failure_patterns: &[Value],
    suggestions: &[Value],
    route_proposals: &[research_domain::HumanRouteProposal],
) -> Value {
    let live_routes = snapshot
        .routes
        .iter()
        .filter(|route| {
            matches!(
                route.status.to_string().as_str(),
                "incubating" | "active" | "blocked" | "probation" | "revived"
            )
        })
        .collect::<Vec<_>>();
    let live_tasks = snapshot
        .tasks
        .iter()
        .filter(|task| {
            matches!(
                task.status,
                TaskStatus::Queued
                    | TaskStatus::Offered
                    | TaskStatus::Leased
                    | TaskStatus::Running
                    | TaskStatus::Checkpointed
                    | TaskStatus::ResultSubmitted
                    | TaskStatus::Ingesting
            )
        })
        .collect::<Vec<_>>();
    let active_facts = snapshot
        .facts
        .iter()
        .filter(|fact| fact.status.to_string() == "active")
        .collect::<Vec<_>>();
    let open_bottlenecks = bottlenecks
        .iter()
        .filter(|bottleneck| bottleneck.status == "open")
        .collect::<Vec<_>>();
    let relevant_uncertainties = snapshot
        .uncertainties
        .iter()
        .filter(|uncertainty| {
            delta
                .changed_uncertainty_ids
                .contains(&uncertainty.uncertainty_id)
                || matches!(uncertainty.severity.as_str(), "critical" | "high")
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "planning_protocol":"research_delta_v2",
        "project_revision":snapshot.project_revision,
        "problem_contract":snapshot.project.contract,
        "budget":snapshot.project.budget,
        "research_delta":delta,
        "open_bottlenecks":open_bottlenecks,
        "live_routes":live_routes,
        "live_tasks":live_tasks,
        "active_facts":active_facts,
        "relevant_uncertainties":relevant_uncertainties,
        "failure_patterns":failure_patterns,
        "human_suggestions":suggestions,
        "human_route_proposals":route_proposals.iter().filter(|proposal| proposal.status == "queued").collect::<Vec<_>>(),
        "hard_limits":{"max_live_routes":6,"max_new_routes":2,"max_routes_per_goal":3,"max_live_routes_per_family":1},
        "trust_rule":"Only active_facts are mathematical premises; summaries, routes, bottlenecks, suggestions, and deltas are planning evidence only.",
        "omitted":["raw artifacts","complete event history","terminal task logs not referenced by delta","unrelated sources"],
    })
}

fn continuity_plan(
    snapshot: &ProjectSnapshot,
    bottlenecks: &[research_domain::Bottleneck],
    reason: &str,
) -> Option<PlannerOutput> {
    let bottleneck = bottlenecks
        .iter()
        .filter(|bottleneck| bottleneck.status == "open")
        .max_by(|left, right| left.priority.total_cmp(&right.priority))?;
    let route = snapshot.routes.iter().find(|route| {
        matches!(
            route.status.to_string().as_str(),
            "incubating" | "active" | "blocked" | "probation" | "revived"
        ) && route
            .target_goal_ids
            .iter()
            .any(|goal_id| bottleneck.target_goal_ids.contains(goal_id))
    })?;
    let worker_role = if bottleneck.kind.contains("counterexample") {
        "counterexample_hunter"
    } else if bottleneck.kind.contains("source") || bottleneck.kind.contains("citation") {
        "literature_researcher"
    } else {
        "prover"
    };
    Some(PlannerOutput {
        rationale_summary: format!(
            "Deterministic continuity plan for bottleneck {} after planner failure: {reason}",
            bottleneck.bottleneck_id
        ),
        routes: vec![RouteProposal {
            title: route.title.clone(),
            method_summary: route.method_summary.clone(),
            target_goal_ids: route.target_goal_ids.clone(),
            required_fact_ids: route.required_fact_ids.clone(),
            expected_subgoals: vec![bottleneck.precise_statement.clone()],
            expected_goal_progress: 0.4,
            uncertainty_reduction: 0.4,
            human_suggestion_alignment: 0.0,
            evidence_support: 0.5,
            route_diversity: 0.0,
            verifiability: 0.9,
            novelty: 0.0,
            failure_similarity_penalty: 0.0,
            cost_penalty: 0.1,
            risks: vec![format!("planner unavailable: {reason}")],
        }],
        assignments: vec![research_domain::AssignmentDraft {
            route_index: 0,
            worker_role: worker_role.into(),
            strategic_role: "central_bridge".into(),
            addresses_interface_debt: true,
            goal_ids: bottleneck.target_goal_ids.clone(),
            objective: bottleneck.precise_statement.clone(),
            completion_contract: serde_json::to_string(&bottleneck.completion_contract)
                .unwrap_or_else(|_| "submit the named repair output or an explicit blocker".into()),
            priority: bottleneck.priority.clamp(0.0, 1.0),
        }],
        targeted_uncertainty_ids: vec![],
        suggestion_decisions: vec![format!(
            "planner unavailable; continued only persisted bottleneck {}",
            bottleneck.bottleneck_id
        )],
    })
}

fn degraded_waiting_plan(reason: &str) -> PlannerOutput {
    PlannerOutput {
        rationale_summary: format!(
            "planner_degraded_waiting: no persisted checkpoint, repair action, candidate review, or route-bound bottleneck can produce a legal narrow task; {reason}"
        ),
        routes: vec![],
        assignments: vec![],
        targeted_uncertainty_ids: vec![],
        suggestion_decisions: vec![
            "no new generic research task was manufactured while the planner was unavailable"
                .into(),
        ],
    }
}

#[cfg(test)]
mod strategy_audit_tests {
    use chrono::{Duration, Utc};
    use serde_json::json;

    use research_domain::{
        AssignmentDraft, CandidateSubmission, CandidateType, PlannerOutput, RouteProposal,
        RouteReflection, SourceRecord, StrategyDirectorOutput, StrategyInterfaceDebt,
        SupervisorOutput, Task, TaskStatus, VerificationReport, VerificationVerdict,
    };

    use super::{
        assess_reviewer_independence, bind_strategy_to_assignments, deterministic_precheck,
        planning_attempt_timeouts, primary_plan_is_executable, reflection_route_is_policy_eligible,
        reviewer_role_contract, statements_match_target, strategy_audit_decision,
    };

    fn review() -> RouteReflection {
        RouteReflection {
            route_index: 0,
            changes_problem: false,
            uses_unverified_claims: false,
            conflicts_with_facts: false,
            repeats_failure_pattern: false,
            has_verifiable_milestone: true,
            risk_score: 0.0,
            goal_closure_leverage: 1.0,
            generality_gain: 1.0,
            assumption_debt: 0.0,
            bridge_centrality: 1.0,
            architecture_fit: 1.0,
            unjustified_narrowing: false,
            remaining_goal_gaps_if_successful: vec![],
            blockers: vec![],
            suggestions: vec![],
        }
    }

    #[test]
    fn reflection_rejects_placeholder_and_unverified_routes_before_supervision() {
        let valid = review();
        assert!(reflection_route_is_policy_eligible(&valid, true));

        let mut placeholder = review();
        placeholder.has_verifiable_milestone = false;
        assert!(!reflection_route_is_policy_eligible(&placeholder, true));

        let mut unverified = review();
        unverified.uses_unverified_claims = true;
        assert!(!reflection_route_is_policy_eligible(&unverified, true));
        assert!(!reflection_route_is_policy_eligible(&valid, false));
    }

    #[test]
    fn central_assignment_cannot_drop_persisted_strategy_contract() {
        let strategy = StrategyDirectorOutput {
            verdict_summary: "one bridge remains".into(),
            fixed_goal: "prove the exact theorem".into(),
            proof_skeleton: vec!["local obstruction".into(), "global witness".into()],
            route_portfolio: vec![],
            interface_debts: vec![StrategyInterfaceDebt {
                interface_name: "local-to-global".into(),
                input_required: "local obstruction".into(),
                output_available: "component separation".into(),
                missing_matches: vec!["global witness".into()],
                failure_if_ignored: "the exact theorem remains open".into(),
                affected_goal_ids: vec!["goal-main".into()],
            }],
            central_missing_bridge: "upgrade every local obstruction to a global witness".into(),
            method_vs_proposition_failure: "undetermined".into(),
            dangerous_shortcuts: vec![],
            strategy_directives: vec!["preserve universal quantifiers".into()],
            literature_priorities: vec!["locate the exact local-to-global theorem".into()],
            macro_replan_required: false,
        };
        let mut supervisor = SupervisorOutput {
            rationale_summary: "test".into(),
            assignments: vec![
                AssignmentDraft {
                    route_index: 0,
                    worker_role: "prover".into(),
                    strategic_role: "central_bridge".into(),
                    addresses_interface_debt: false,
                    goal_ids: vec!["goal-main".into()],
                    objective: "resolve the goal".into(),
                    completion_contract: "submit a proof".into(),
                    priority: 1.0,
                },
                AssignmentDraft {
                    route_index: 1,
                    worker_role: "literature_researcher".into(),
                    strategic_role: "supporting".into(),
                    addresses_interface_debt: false,
                    goal_ids: vec!["goal-main".into()],
                    objective: "search the literature".into(),
                    completion_contract: "return sources".into(),
                    priority: 0.8,
                },
            ],
            targeted_uncertainty_ids: vec![],
            suggestion_decisions: vec![],
            deferred_route_indices: vec![],
        };

        bind_strategy_to_assignments(&strategy, &mut supervisor);

        let central = &supervisor.assignments[0];
        assert!(central.objective.contains("prove the exact theorem"));
        assert!(central.objective.contains(&strategy.central_missing_bridge));
        assert!(central.objective.contains("local-to-global"));
        assert!(central.objective.contains("maximal or extremal extension"));
        assert!(
            central
                .objective
                .contains("auxiliary resource/coverage state")
        );
        assert!(central.addresses_interface_debt);
        assert!(
            central
                .completion_contract
                .contains("smallest exact residual obstruction")
        );
        assert!(
            central
                .completion_contract
                .contains("equal witness indices")
        );
        assert!(central.completion_contract.contains("sibling Candidate"));
        let literature = &supervisor.assignments[1];
        assert!(
            literature
                .objective
                .contains("locate the exact local-to-global theorem")
        );
        assert!(
            literature
                .completion_contract
                .contains("exact theorem/section locator")
        );
    }

    #[test]
    fn first_planning_attempt_fails_fast_but_retry_keeps_full_budget() {
        assert_eq!(planning_attempt_timeouts(1_200, 1), (300, 600));
        assert_eq!(planning_attempt_timeouts(1_200, 2), (600, 1_200));
        assert_eq!(planning_attempt_timeouts(1, 1), (1, 1));
        assert_eq!(planning_attempt_timeouts(0, 2), (1, 1));
    }

    #[test]
    fn proof_request_wrapper_cannot_bypass_main_target_matching() {
        let claim = "Every minimal relation of every numerical semigroup is an RF-relation.";
        assert!(statements_match_target(
            "Prove or disprove that every minimal relation of every numerical semigroup is an RF-relation.",
            claim,
        ));
        assert!(statements_match_target(
            "证明或否证每个数值半群的每个最小关系都是 RF-relation。",
            "每个数值半群的每个最小关系都是 RF-relation。",
        ));
        assert!(!statements_match_target(
            "Prove or disprove that every minimal relation is an RF-relation.",
            "Every minimal relation in embedding dimension four is an RF-relation.",
        ));
    }

    #[test]
    fn mathematical_reviewers_use_distinct_cognitive_contracts() {
        let forward = reviewer_role_contract("math_review_1");
        let falsification = reviewer_role_contract("math_review_2");
        let reconstruction = reviewer_role_contract("math_review_3");
        assert!(forward.contains("正向证明审计"));
        assert!(falsification.contains("反向证伪审计"));
        assert!(reconstruction.contains("独立重构审计"));
        assert_ne!(forward, falsification);
        assert_ne!(forward, reconstruction);
        assert_ne!(falsification, reconstruction);
    }

    #[test]
    fn byte_equivalent_math_reviews_do_not_count_as_independent() {
        let report = VerificationReport {
            verdict: VerificationVerdict::Accepted,
            summary: "The stated proof is valid.".into(),
            critical_errors: vec![],
            gaps: vec![],
            uncertainties: vec![],
            repair_actions: vec![],
            checked_fact_ids: vec![],
            checked_source_ids: vec![],
            evidence_level: "independent_llm_check".into(),
        };
        let duplicated = assess_reviewer_independence(&[
            ("math_review_1".into(), report.clone()),
            ("math_review_2".into(), report.clone()),
            ("adversarial_review".into(), report.clone()),
        ]);
        assert_eq!(
            duplicated.duplicate_groups,
            vec![vec![
                String::from("math_review_1"),
                String::from("math_review_2"),
            ]]
        );

        let mut distinct_report = report;
        distinct_report.summary = "No counterexample survives the boundary audit.".into();
        let distinct = assess_reviewer_independence(&[
            ("math_review_1".into(), distinct_report),
            ("math_review_2".into(), duplicated_report()),
        ]);
        assert!(distinct.duplicate_groups.is_empty());
    }

    fn duplicated_report() -> VerificationReport {
        VerificationReport {
            verdict: VerificationVerdict::Accepted,
            summary: "The stated proof is valid.".into(),
            critical_errors: vec![],
            gaps: vec![],
            uncertainties: vec![],
            repair_actions: vec![],
            checked_fact_ids: vec![],
            checked_source_ids: vec![],
            evidence_level: "independent_llm_check".into(),
        }
    }

    #[test]
    fn one_surviving_policy_route_can_execute_with_adversarial_pressure() {
        let route = RouteProposal {
            title: "central bridge".into(),
            method_summary: "prove the exact missing bridge".into(),
            target_goal_ids: vec!["goal-main".into()],
            required_fact_ids: vec![],
            expected_subgoals: vec!["bridge".into()],
            expected_goal_progress: 1.0,
            uncertainty_reduction: 1.0,
            human_suggestion_alignment: 0.0,
            evidence_support: 0.0,
            route_diversity: 1.0,
            verifiability: 1.0,
            novelty: 0.0,
            failure_similarity_penalty: 0.0,
            cost_penalty: 0.0,
            risks: vec![],
        };
        let central = AssignmentDraft {
            route_index: 0,
            worker_role: "prover".into(),
            strategic_role: "central_bridge".into(),
            addresses_interface_debt: true,
            goal_ids: vec!["goal-main".into()],
            objective: "prove the bridge".into(),
            completion_contract: "checked proof or exact blocker".into(),
            priority: 1.0,
        };
        let plan = PlannerOutput {
            rationale_summary: "one route survived independent reflection".into(),
            routes: vec![route],
            assignments: vec![central],
            targeted_uncertainty_ids: vec![],
            suggestion_decisions: vec![],
        };

        assert!(primary_plan_is_executable(&plan));

        let empty = PlannerOutput {
            routes: vec![],
            ..plan
        };
        assert!(!primary_plan_is_executable(&empty));
    }

    #[test]
    fn missing_source_dependency_is_a_terminal_precheck_error() {
        let task = Task {
            task_id: "task-1".into(),
            project_id: "project-1".into(),
            route_id: "route-1".into(),
            worker_id: None,
            worker_role: "prover".into(),
            goal_ids: vec![],
            objective: "prove a lemma".into(),
            completion_contract: "submit a checked lemma".into(),
            status: TaskStatus::Completed,
            priority: 1.0,
            revision: 1,
            route_cancellation_epoch: 0,
            round: 1,
            result_summary: None,
            plan_revision_id: None,
            task_signature: None,
            context_packet_id: None,
        };
        let submission = CandidateSubmission {
            task_id: task.task_id.clone(),
            route_id: task.route_id.clone(),
            target_goal_ids: vec![],
            statement: "A checked intermediate lemma".into(),
            assumptions: vec![],
            proof_markdown: "Every inference is written explicitly.".into(),
            dependency_fact_ids: vec![],
            definitions_introduced: std::collections::BTreeMap::new(),
            external_source_ids: vec!["source-that-was-never-ingested".into()],
            candidate_type: CandidateType::Lemma,
            task_revision: task.revision,
            route_cancellation_epoch: task.route_cancellation_epoch,
        };

        let report = deterministic_precheck(&submission, &task, &[], &[], &[])
            .expect("missing source must fail deterministically");

        assert!(
            report
                .critical_errors
                .contains(&"missing_source_dependency".to_owned())
        );
    }

    #[test]
    fn cited_source_without_archived_fulltext_is_rejected_before_review() {
        let task = Task {
            task_id: "task-1".into(),
            project_id: "project-1".into(),
            route_id: "route-1".into(),
            worker_id: None,
            worker_role: "prover".into(),
            goal_ids: vec![],
            objective: "prove a source-backed lemma".into(),
            completion_contract: "submit a checked lemma".into(),
            status: TaskStatus::Completed,
            priority: 1.0,
            revision: 1,
            route_cancellation_epoch: 0,
            round: 1,
            result_summary: None,
            plan_revision_id: None,
            task_signature: None,
            context_packet_id: None,
        };
        let submission = CandidateSubmission {
            task_id: task.task_id.clone(),
            route_id: task.route_id.clone(),
            target_goal_ids: vec![],
            statement: "A source-backed lemma".into(),
            assumptions: vec![],
            proof_markdown: "The cited theorem implies the stated lemma.".into(),
            dependency_fact_ids: vec![],
            definitions_introduced: std::collections::BTreeMap::new(),
            external_source_ids: vec!["source-1".into()],
            candidate_type: CandidateType::Lemma,
            task_revision: task.revision,
            route_cancellation_epoch: task.route_cancellation_epoch,
        };
        let source = SourceRecord {
            source_id: "source-1".into(),
            project_id: task.project_id.clone(),
            title: "Primary theorem".into(),
            authors: vec![],
            url: Some("https://example.org/paper.pdf".into()),
            normalized_url: Some("https://example.org/paper.pdf".into()),
            identifier_kind: None,
            identifier_value: None,
            citation_key: Some("Primary".into()),
            theorem_reference: Some("Theorem 1".into()),
            statement_excerpt: Some("A".into()),
            assumptions: vec![],
            applicability: "claimed exact match".into(),
            status: "reported_unverified".into(),
            origin_task_id: Some(task.task_id.clone()),
            origin_route_id: Some(task.route_id.clone()),
            fulltext_artifact_id: None,
            provenance: json!({}),
            disposition_reason: None,
            retrieved_at: Utc::now(),
        };

        let report = deterministic_precheck(&submission, &task, &[], &[source], &[])
            .expect("citation without archived content must fail deterministically");
        assert!(
            report
                .critical_errors
                .contains(&"source_fulltext_artifact_missing".to_owned())
        );
    }

    fn context(delta: &serde_json::Value) -> serde_json::Value {
        json!({"research_delta": delta})
    }

    #[test]
    fn selects_initial_control_and_event_driven_macro_audits() {
        let empty = json!({
            "accepted_fact_ids":[], "solved_goal_ids":[], "reopened_goal_ids":[],
            "new_failure_pattern_ids":[], "new_proof_debt_ids":[],
            "rejected_candidate_ids":[], "failed_or_expired_attempt_ids":[],
            "completed_task_attempt_ids":[], "human_command_ids":[],
            "route_state_changes":[]
        });
        let initial = strategy_audit_decision(None, &[], &context(&empty));
        assert_eq!(initial.kind, "initial");

        let prior = json!({"audit_kind":"initial","created_at":Utc::now().to_rfc3339()});
        let control =
            strategy_audit_decision(Some(&prior), std::slice::from_ref(&prior), &context(&empty));
        assert_eq!(control.kind, "control");

        let mut event_delta = empty;
        event_delta["accepted_fact_ids"] = json!(["fact-1"]);
        event_delta["route_state_changes"] =
            json!([{"event_type":"route.blocked","route_id":"route-1"}]);
        let event = strategy_audit_decision(
            Some(&prior),
            std::slice::from_ref(&prior),
            &context(&event_delta),
        );
        assert_eq!(event.kind, "macro");
        assert!(
            event
                .reasons
                .contains(&"accepted_fact_changed_global_premises".to_owned())
        );
        assert!(
            event
                .reasons
                .contains(&"route_portfolio_state_changed".to_owned())
        );
    }

    #[test]
    fn selects_periodic_macro_after_three_controls_or_four_hours() {
        let empty = context(&json!({
            "accepted_fact_ids":[], "solved_goal_ids":[], "reopened_goal_ids":[],
            "new_failure_pattern_ids":[], "new_proof_debt_ids":[],
            "rejected_candidate_ids":[], "failed_or_expired_attempt_ids":[],
            "completed_task_attempt_ids":[], "human_command_ids":[],
            "route_state_changes":[]
        }));
        let initial = json!({"audit_kind":"initial","created_at":Utc::now().to_rfc3339()});
        let controls = vec![
            initial.clone(),
            json!({"audit_kind":"control"}),
            json!({"audit_kind":"control"}),
        ];
        let by_count = strategy_audit_decision(Some(&initial), &controls, &empty);
        assert_eq!(by_count.kind, "macro");
        assert!(
            by_count
                .reasons
                .contains(&"periodic_macro_audit_due".to_owned())
        );

        let old_macro = json!({
            "audit_kind":"macro",
            "created_at":(Utc::now() - Duration::hours(5)).to_rfc3339()
        });
        let by_time =
            strategy_audit_decision(Some(&old_macro), std::slice::from_ref(&old_macro), &empty);
        assert_eq!(by_time.kind, "macro");
    }
}

#[cfg(test)]
mod fulltext_tests {
    use std::{net::IpAddr, str::FromStr, sync::Arc};

    use chrono::Utc;
    use research_domain::{
        Budget, ProblemContract, SourceDraft, SourceRecord, Task, TaskStatus, WorkerOutput,
    };
    use research_storage::SqliteStore;
    use research_worker_runtime::{AgentHandle, MockBackend};
    use sha2::{Digest, Sha256};

    use super::{
        ResearchConfig, ResearchService, fetch_public_source_fulltext, is_codex_synthetic_egress,
        is_public_ip, safe_source_filename, source_media_type_allowed,
        stage_sources_for_citation_review,
    };

    #[test]
    fn trusted_source_fetch_rejects_private_addresses_and_unsafe_media() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.1.1",
            "192.0.2.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
        ] {
            assert!(!is_public_ip(IpAddr::from_str(address).expect("ip")));
        }
        assert!(is_public_ip(IpAddr::from_str("1.1.1.1").expect("ip")));
        assert!(is_public_ip(
            IpAddr::from_str("2606:4700:4700::1111").expect("ip")
        ));
        assert!(is_codex_synthetic_egress(
            IpAddr::from_str("198.18.0.42").expect("ip")
        ));
        assert!(!is_codex_synthetic_egress(
            IpAddr::from_str("198.20.0.42").expect("ip")
        ));

        let pdf = reqwest::Url::parse("https://example.org/papers/result.pdf").expect("url");
        assert!(source_media_type_allowed("application/pdf", &pdf));
        assert!(!source_media_type_allowed("application/zip", &pdf));
        assert_eq!(safe_source_filename(&pdf, "application/pdf"), "result.pdf");
    }

    #[tokio::test]
    #[ignore = "live HTTPS diagnostic; run explicitly when validating the egress environment"]
    async fn trusted_source_fetch_reads_a_known_public_pdf() {
        let (filename, bytes, hash) =
            fetch_public_source_fulltext("https://www.numdam.org/item/10.24033/asens.1307.pdf")
                .await
                .expect("public PDF fetch");
        assert!(
            std::path::Path::new(&filename)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        );
        assert!(bytes.starts_with(b"%PDF"));
        assert_eq!(hash, hex::encode(Sha256::digest(&bytes)));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn archives_verified_fulltext_and_quarantines_invalid_sources() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
            .await
            .expect("store");
        let service = ResearchService::new(
            store.clone(),
            Arc::new(MockBackend::default()),
            ResearchConfig::default(),
        );
        let project = service
            .create_project(
                "fulltext".into(),
                ProblemContract {
                    original_problem: "A".into(),
                    target_statement: "A".into(),
                    assumptions: vec![],
                    success_criteria: "verified".into(),
                    version: 1,
                },
                Budget::default(),
            )
            .await
            .expect("project");
        let worker_root = temp.path().join("worker");
        tokio::fs::create_dir_all(&worker_root)
            .await
            .expect("worker root");
        let document = b"primary source full text";
        tokio::fs::write(worker_root.join("paper.txt"), document)
            .await
            .expect("fulltext");
        tokio::fs::write(temp.path().join("outside.txt"), b"secret")
            .await
            .expect("outside");
        let handle = AgentHandle {
            handle_id: "agent-test".into(),
            project_id: project.project_id.clone(),
            role: "literature_researcher".into(),
            model: None,
            working_directory: worker_root,
            session_id: None,
        };
        let task = Task {
            task_id: "task-fulltext".into(),
            project_id: project.project_id.clone(),
            route_id: "route-fulltext".into(),
            worker_id: None,
            worker_role: "literature_researcher".into(),
            goal_ids: vec![],
            objective: "find source".into(),
            completion_contract: "archive source".into(),
            status: TaskStatus::Running,
            priority: 1.0,
            revision: 0,
            route_cancellation_epoch: 0,
            round: 0,
            result_summary: None,
            plan_revision_id: None,
            task_signature: None,
            context_packet_id: None,
        };
        let source = |path: &str, hash: Option<String>| SourceDraft {
            title: "Primary source".into(),
            authors: vec![],
            url: Some("https://example.test/paper".into()),
            citation_key: Some("Primary".into()),
            theorem_reference: Some("Theorem 1".into()),
            statement_excerpt: Some("A".into()),
            assumptions: vec![],
            applicability: "exact".into(),
            status: "reported".into(),
            retrieval_query: Some("A theorem".into()),
            document_version: Some("v1".into()),
            fulltext_path: Some(path.into()),
            fulltext_sha256: hash,
            fulltext_artifact_id: None,
        };
        let mut output = WorkerOutput {
            summary: "source".into(),
            discoveries: vec![],
            candidates: vec![],
            failures: vec![],
            uncertainties: vec![],
            sources: vec![source(
                "paper.txt",
                Some(hex::encode(Sha256::digest(document))),
            )],
            experiments: vec![],
        };
        service
            .archive_source_fulltexts(&handle, &task, &mut output)
            .await
            .expect("archive");
        assert!(output.sources[0].fulltext_artifact_id.is_some());
        let artifacts = store
            .list_artifacts(&project.project_id)
            .await
            .expect("artifacts");
        assert_eq!(
            artifacts
                .iter()
                .filter(|artifact| artifact.kind == "source_fulltext")
                .count(),
            1
        );
        let archived_source = SourceRecord {
            source_id: "source-staged".into(),
            project_id: project.project_id.clone(),
            title: "Primary source".into(),
            authors: vec![],
            url: Some("https://example.test/paper".into()),
            normalized_url: Some("https://example.test/paper".into()),
            identifier_kind: None,
            identifier_value: None,
            citation_key: Some("Primary".into()),
            theorem_reference: Some("Theorem 1".into()),
            statement_excerpt: Some("A".into()),
            assumptions: vec![],
            applicability: "exact".into(),
            status: "reported_unverified".into(),
            origin_task_id: Some(task.task_id.clone()),
            origin_route_id: Some(task.route_id.clone()),
            fulltext_artifact_id: output.sources[0].fulltext_artifact_id.clone(),
            provenance: serde_json::json!({}),
            disposition_reason: None,
            retrieved_at: Utc::now(),
        };
        let citation_packet = stage_sources_for_citation_review(
            &service,
            &project.project_id,
            &temp.path().join("citation-review"),
            &[archived_source],
        )
        .await;
        let staged_path = citation_packet[0]["citation_review_material"]["local_fulltext_path"]
            .as_str()
            .expect("staged source path");
        assert_eq!(
            tokio::fs::read(staged_path).await.expect("staged bytes"),
            document
        );

        let mut escaped = WorkerOutput {
            sources: vec![source(
                "../outside.txt",
                Some(hex::encode(Sha256::digest(b"secret"))),
            )],
            ..output
        };
        service
            .archive_source_fulltexts(&handle, &task, &mut escaped)
            .await
            .expect("quarantine path escape without losing task output");
        assert!(escaped.sources[0].fulltext_artifact_id.is_none());
        assert!(escaped.failures.iter().any(|failure| {
            failure.failure_type == "source_fulltext_archival"
                && failure
                    .summary
                    .contains("fulltext_path_escaped_worker_directory")
        }));

        let mut missing_source = source("unused.txt", None);
        missing_source.fulltext_path = None;
        missing_source.url = None;
        let mut missing = WorkerOutput {
            summary: "theorem toolbox discovery survives unavailable source".into(),
            discoveries: vec![],
            candidates: vec![],
            failures: vec![],
            uncertainties: vec![],
            sources: vec![missing_source],
            experiments: vec![],
        };
        service
            .archive_source_fulltexts(&handle, &task, &mut missing)
            .await
            .expect("missing source is a per-source rejection");
        assert!(missing.sources[0].fulltext_artifact_id.is_none());
        assert!(missing.failures.iter().any(|failure| {
            failure
                .summary
                .contains("reported_literature_source_missing_fulltext_and_url")
        }));
    }
}
