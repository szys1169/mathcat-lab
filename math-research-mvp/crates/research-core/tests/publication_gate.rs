use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::Utc;
use research_core::{ResearchConfig, ResearchService};
use research_domain::{Budget, CommandMode, ProblemContract};
use research_storage::{CommandDraft, SqliteStore};
use research_worker_runtime::{
    AgentBackend, AgentError, AgentHandle, AgentRunResult, AgentSpec, AgentTask,
    BackendCapabilities, MockBackend,
};
use serde_json::json;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct BlockingPublicationBackend {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl BlockingPublicationBackend {
    async fn result_after_release(&self) -> AgentRunResult {
        self.entered.notify_one();
        self.release.notified().await;
        let now = Utc::now();
        AgentRunResult {
            structured_output: json!({
                "status": "evidence_gaps",
                "article_plan_md": "",
                "claim_evidence_ledger_md": "",
                "related_work_tex": "",
                "article_candidate_tex": "",
                "revision_notes_md": "",
                "evidence_gaps_md": "writer stopped fixture"
            }),
            session_id: None,
            raw_events: Vec::new(),
            input_tokens: 0,
            output_tokens: 0,
            stderr: String::new(),
            started_at: now,
            completed_at: now,
        }
    }
}

#[async_trait]
impl AgentBackend for BlockingPublicationBackend {
    fn name(&self) -> &'static str {
        "blocking-publication"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            resumable_session: false,
            safe_point_steering: false,
            non_waking_injection: false,
            graceful_cancel: false,
            event_stream: false,
            structured_output: true,
            tool_permissions: false,
        }
    }

    async fn create(&self, spec: AgentSpec) -> Result<AgentHandle, AgentError> {
        Ok(AgentHandle {
            handle_id: "blocking-publication-handle".into(),
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
        _cancellation: CancellationToken,
    ) -> Result<AgentRunResult, AgentError> {
        Ok(self.result_after_release().await)
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

async fn apply_command(store: &SqliteStore, project_id: &str, command_type: &str, key: &str) {
    let revision = store
        .get_project(project_id)
        .await
        .expect("project revision")
        .revision;
    let (command, _) = store
        .enqueue_command(
            project_id,
            CommandDraft {
                command_type: command_type.into(),
                target_kind: "project".into(),
                target_id: project_id.into(),
                mode: CommandMode::Immediate,
                payload: json!({}),
                expected_project_revision: revision,
                idempotency_key: key.into(),
                reason: "publication race fixture".into(),
                requested_by: "test".into(),
            },
        )
        .await
        .expect("enqueue command");
    store
        .apply_command(project_id, &command.command_id)
        .await
        .expect("apply command");
}

#[tokio::test]
async fn publication_gate_writes_evidence_gaps_without_calling_the_model() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
        .await
        .expect("store");
    let service = ResearchService::new(
        store.clone(),
        Arc::new(MockBackend::default()),
        ResearchConfig {
            runtime_root: temp.path().join("runtime"),
            output_root: temp.path().join("output"),
            ..ResearchConfig::default()
        },
    );
    let project = service
        .create_project(
            "paper-gate".into(),
            ProblemContract {
                original_problem: "Prove a theorem".into(),
                target_statement: "A".into(),
                assumptions: Vec::new(),
                success_criteria: "verified result and auditable paper".into(),
                version: 1,
            },
            Budget::default(),
        )
        .await
        .expect("project");

    let result = service
        .publish_paper(&project.project_id, false)
        .await
        .expect("blocked publication is a normal result");

    assert_eq!(result.status, "blocked_by_evidence");
    assert!(
        result
            .evidence_gaps
            .iter()
            .any(|gap| gap.contains("active Fact"))
    );
    assert_eq!(
        store
            .total_model_calls(&project.project_id)
            .await
            .expect("usage"),
        0
    );
    let writer_root = temp
        .path()
        .join("output")
        .join(&project.project_id)
        .join("writer");
    let version_root = std::fs::read_dir(&writer_root)
        .expect("writer versions")
        .next()
        .expect("one version")
        .expect("version entry")
        .path();
    assert!(version_root.join("source_packet.json").is_file());
    assert!(version_root.join("evidence_gaps.md").is_file());
    assert!(version_root.join("publication_manifest.json").is_file());
}

#[tokio::test]
async fn project_stop_wins_against_a_late_publication_success() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
        .await
        .expect("store");
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let service = ResearchService::new(
        store.clone(),
        Arc::new(BlockingPublicationBackend {
            entered: entered.clone(),
            release: release.clone(),
        }),
        ResearchConfig {
            runtime_root: temp.path().join("runtime"),
            output_root: temp.path().join("output"),
            ..ResearchConfig::default()
        },
    );
    let project = service
        .create_project(
            "publication-stop-race".into(),
            ProblemContract {
                original_problem: "Prove a theorem".into(),
                target_statement: "A".into(),
                assumptions: Vec::new(),
                success_criteria: "verified result".into(),
                version: 1,
            },
            Budget::default(),
        )
        .await
        .expect("project");
    apply_command(&store, &project.project_id, "start_project", "start-race").await;
    sqlx::query("INSERT INTO facts(fact_id,project_id,statement,assumptions_json,proof_markdown,dependency_fact_ids_json,definitions_introduced_json,external_source_ids_json,verification_ids_json,evidence_level,created_by,status,content_hash,created_at) VALUES('fact-publication-race',?,'A','[]','verified fixture','[]','{}','[]','[]','reviewed','test','active','fact-publication-race-hash',?)")
        .bind(&project.project_id)
        .bind(Utc::now().to_rfc3339())
        .execute(store.pool())
        .await
        .expect("active fact fixture");

    let publishing = {
        let service = service.clone();
        let project_id = project.project_id.clone();
        tokio::spawn(async move {
            service
                .publish_paper_idempotent(&project_id, true, "stop-race-publication")
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("paper writer entered");
    apply_command(&store, &project.project_id, "stop_project", "stop-race").await;
    let artifacts_at_stop: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM artifacts WHERE project_id=?")
            .bind(&project.project_id)
            .fetch_one(store.pool())
            .await
            .expect("artifacts at stop");
    release.notify_one();

    let error = tokio::time::timeout(Duration::from_secs(5), publishing)
        .await
        .expect("publication returns after writer release")
        .expect("publication task joins")
        .expect_err("stopped publication must not report a late success");
    assert!(error.to_string().contains("stopped by human"));
    let run = store
        .list_publications(&project.project_id)
        .await
        .expect("publication runs")
        .into_iter()
        .next()
        .expect("publication run");
    assert_eq!(run.status, "failed");
    assert_eq!(run.error.as_deref(), Some("project stopped by human"));
    let artifacts_after_return: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM artifacts WHERE project_id=?")
            .bind(&project.project_id)
            .fetch_one(store.pool())
            .await
            .expect("artifacts after publication return");
    assert_eq!(artifacts_after_return, artifacts_at_stop);
}
