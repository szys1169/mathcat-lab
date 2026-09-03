use std::sync::Arc;

use research_core::{ResearchConfig, ResearchService};
use research_domain::{Budget, ProblemContract};
use research_storage::SqliteStore;
use research_worker_runtime::MockBackend;

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
