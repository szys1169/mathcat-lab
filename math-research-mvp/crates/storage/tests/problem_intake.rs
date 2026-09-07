use research_domain::{
    Budget, CommandStatus, ProblemAssumption, ProblemAssumptionProvenance, ProblemDocument,
    ProblemDraftStatus, ProblemMaterial, ProjectStatus,
};
use research_storage::{
    ProblemDraftBeginRequest, ProblemDraftCompletionRequest, ProblemDraftConfirmationRequest,
    ProblemDraftControlRequest, SqliteStore, StorageError,
};

async fn store() -> (tempfile::TempDir, SqliteStore) {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect("sqlite::memory:", temp.path())
        .await
        .expect("store");
    (temp, store)
}

fn material() -> ProblemMaterial {
    ProblemMaterial {
        relative_path: "notes/problem.md".into(),
        media_type: "text/markdown".into(),
        byte_size: 128,
        included_bytes: 128,
        sha256: "a".repeat(64),
        status: "included".into(),
        warning: None,
    }
}

fn document() -> ProblemDocument {
    ProblemDocument {
        name: "Generated problem".into(),
        problem: "Determine whether every object satisfying A also satisfies B.".into(),
        target_statement: "For every x, A(x) implies B(x).".into(),
        assumptions: vec![ProblemAssumption {
            statement: "x belongs to the stated ambient category".into(),
            provenance: ProblemAssumptionProvenance::Material,
        }],
        success_criteria: "An accepted proof or a verified counterexample.".into(),
        budget: Budget::default(),
        human_route_approval: false,
        budget_rationale: "Use the standard bounded research profile.".into(),
        generation_notes: vec!["Expanded the quantifiers explicitly.".into()],
        unresolved_questions: vec![],
        material_references: vec!["notes/problem.md".into()],
    }
}

#[tokio::test]
async fn problem_draft_creation_is_idempotent_and_does_not_create_a_project() {
    let (_temp, store) = store().await;
    let materials = vec![material()];
    let request = ProblemDraftBeginRequest {
        requested_by: "researcher-1",
        idempotency_key: "draft-create-1",
        prompt: "Study the A to B question",
        material_directory: ".",
        materials: &materials,
        model: Some("test-model"),
    };
    let (first, replayed) = store.begin_problem_draft(request).await.expect("begin");
    assert!(!replayed);
    assert_eq!(first.status, ProblemDraftStatus::Generating);
    assert_eq!(first.material_directory, ".");
    let (second, replayed) = store.begin_problem_draft(request).await.expect("replay");
    assert!(replayed);
    assert_eq!(second.draft_id, first.draft_id);

    let conflict = store
        .begin_problem_draft(ProblemDraftBeginRequest {
            material_directory: "a-different-scope",
            ..request
        })
        .await;
    assert!(matches!(
        conflict,
        Err(StorageError::IdempotencyConflict(_))
    ));
    let projects: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects")
        .fetch_one(store.pool())
        .await
        .expect("project count");
    let attempts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM problem_draft_attempts")
        .fetch_one(store.pool())
        .await
        .expect("attempt count");
    assert_eq!(projects, 0);
    assert_eq!(attempts, 1);
}

#[tokio::test]
async fn completion_allows_no_assumptions_but_rejects_a_skipped_material_reference() {
    let (_temp, store) = store().await;
    let mut skipped = material();
    skipped.status = "skipped".into();
    skipped.included_bytes = 0;
    let materials = vec![skipped];
    let (generating, _) = store
        .begin_problem_draft(ProblemDraftBeginRequest {
            requested_by: "researcher-1",
            idempotency_key: "draft-create-skipped-reference",
            prompt: "Study the A to B question",
            material_directory: ".",
            materials: &materials,
            model: None,
        })
        .await
        .expect("begin");
    let mut generated_document = document();
    generated_document.assumptions.clear();
    let rejected = store
        .complete_problem_draft(ProblemDraftCompletionRequest {
            draft_id: &generating.draft_id,
            expected_revision: generating.revision,
            document: &generated_document,
            input_tokens: 0,
            output_tokens: 0,
            elapsed_ms: 1,
        })
        .await;
    assert!(matches!(rejected, Err(StorageError::InvalidTransition(_))));

    generated_document.material_references.clear();
    let awaiting = store
        .complete_problem_draft(ProblemDraftCompletionRequest {
            draft_id: &generating.draft_id,
            expected_revision: generating.revision,
            document: &generated_document,
            input_tokens: 0,
            output_tokens: 0,
            elapsed_ms: 1,
        })
        .await
        .expect("empty assumptions are legal");
    assert_eq!(awaiting.status, ProblemDraftStatus::AwaitingConfirmation);
    assert!(awaiting.document.expect("document").assumptions.is_empty());
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn confirmation_atomically_creates_one_project_and_one_start_command_and_replays() {
    let (_temp, store) = store().await;
    let materials = vec![material()];
    let (generating, _) = store
        .begin_problem_draft(ProblemDraftBeginRequest {
            requested_by: "researcher-1",
            idempotency_key: "draft-create-confirm",
            prompt: "Study the A to B question",
            material_directory: ".",
            materials: &materials,
            model: Some("test-model"),
        })
        .await
        .expect("begin");
    let awaiting = store
        .complete_problem_draft(ProblemDraftCompletionRequest {
            draft_id: &generating.draft_id,
            expected_revision: generating.revision,
            document: &document(),
            input_tokens: 100,
            output_tokens: 50,
            elapsed_ms: 25,
        })
        .await
        .expect("complete");
    assert_eq!(awaiting.status, ProblemDraftStatus::AwaitingConfirmation);
    let document_hash = awaiting.document_hash.clone().expect("document hash");
    let confirmation_request = ProblemDraftConfirmationRequest {
        draft_id: &awaiting.draft_id,
        expected_revision: awaiting.revision,
        expected_document_hash: &document_hash,
        idempotency_key: "draft-confirm-1",
        requested_by: "researcher-1",
        start: true,
        edited_document: None,
        acknowledge_material_warnings: false,
    };
    let (first, events) = store
        .confirm_problem_draft(confirmation_request)
        .await
        .expect("confirm");
    assert!(!first.replayed);
    assert_eq!(first.draft.status, ProblemDraftStatus::Confirmed);
    assert_eq!(first.project.status, ProjectStatus::Created);
    assert_eq!(first.project.revision, 2);
    assert_eq!(
        first.start_command.as_ref().map(|command| command.status),
        Some(CommandStatus::Queued)
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "project.created")
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "human_command.queued")
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "problem_draft.confirmed")
    );

    let (replay, replay_events) = store
        .confirm_problem_draft(confirmation_request)
        .await
        .expect("confirm replay");
    assert!(replay.replayed);
    assert_eq!(replay.project.project_id, first.project.project_id);
    assert_eq!(
        replay
            .start_command
            .as_ref()
            .map(|command| &command.command_id),
        first
            .start_command
            .as_ref()
            .map(|command| &command.command_id)
    );
    assert!(replay_events.is_empty());
    let conflicting_replay = store
        .confirm_problem_draft(ProblemDraftConfirmationRequest {
            start: false,
            ..confirmation_request
        })
        .await;
    assert!(matches!(
        conflicting_replay,
        Err(StorageError::IdempotencyConflict(_))
    ));

    for table in [
        "projects",
        "goals",
        "bottlenecks",
        "planner_health",
        "human_commands",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(store.pool())
            .await
            .expect("row count");
        assert_eq!(count, 1, "unexpected row count in {table}");
    }
}

#[tokio::test]
async fn confirmation_rejects_stale_revision_and_hash_without_partial_project() {
    let (_temp, store) = store().await;
    let materials = vec![material()];
    let (generating, _) = store
        .begin_problem_draft(ProblemDraftBeginRequest {
            requested_by: "researcher-1",
            idempotency_key: "draft-create-stale",
            prompt: "Study the A to B question",
            material_directory: ".",
            materials: &materials,
            model: None,
        })
        .await
        .expect("begin");
    let generated_document = document();
    let awaiting = store
        .complete_problem_draft(ProblemDraftCompletionRequest {
            draft_id: &generating.draft_id,
            expected_revision: generating.revision,
            document: &generated_document,
            input_tokens: 0,
            output_tokens: 0,
            elapsed_ms: 1,
        })
        .await
        .expect("complete");
    let document_hash = awaiting.document_hash.clone().expect("document hash");
    let stale_revision = store
        .confirm_problem_draft(ProblemDraftConfirmationRequest {
            draft_id: &awaiting.draft_id,
            expected_revision: awaiting.revision - 1,
            expected_document_hash: &document_hash,
            idempotency_key: "stale-revision",
            requested_by: "researcher-1",
            start: true,
            edited_document: None,
            acknowledge_material_warnings: false,
        })
        .await;
    assert!(matches!(
        stale_revision,
        Err(StorageError::RevisionConflict { .. })
    ));
    let stale_hash = store
        .confirm_problem_draft(ProblemDraftConfirmationRequest {
            draft_id: &awaiting.draft_id,
            expected_revision: awaiting.revision,
            expected_document_hash: "not-the-displayed-hash",
            idempotency_key: "stale-hash",
            requested_by: "researcher-1",
            start: true,
            edited_document: None,
            acknowledge_material_warnings: false,
        })
        .await;
    assert!(matches!(
        stale_hash,
        Err(StorageError::InvalidTransition(_))
    ));
    let projects: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects")
        .fetch_one(store.pool())
        .await
        .expect("project count");
    assert_eq!(projects, 0);
    assert_eq!(
        store
            .get_problem_draft(&awaiting.draft_id)
            .await
            .expect("draft")
            .status,
        ProblemDraftStatus::AwaitingConfirmation
    );
}

#[tokio::test]
async fn cancel_and_retry_are_idempotent_and_preserve_attempt_history() {
    let (_temp, store) = store().await;
    let materials = vec![material()];
    let (generating, _) = store
        .begin_problem_draft(ProblemDraftBeginRequest {
            requested_by: "researcher-1",
            idempotency_key: "draft-create-control",
            prompt: "Study the A to B question",
            material_directory: ".",
            materials: &materials,
            model: Some("test-model"),
        })
        .await
        .expect("begin");
    let cancel = ProblemDraftControlRequest {
        draft_id: &generating.draft_id,
        expected_revision: generating.revision,
        idempotency_key: "draft-cancel-1",
        requested_by: "researcher-1",
    };
    let (cancelled, replayed) = store.cancel_problem_draft(cancel).await.expect("cancel");
    assert!(!replayed);
    assert_eq!(cancelled.status, ProblemDraftStatus::Failed);
    assert_eq!(cancelled.error_kind.as_deref(), Some("cancelled"));
    let (cancel_replay, replayed) = store
        .cancel_problem_draft(cancel)
        .await
        .expect("cancel replay");
    assert!(replayed);
    assert_eq!(cancel_replay.revision, cancelled.revision);

    let retry = ProblemDraftControlRequest {
        draft_id: &generating.draft_id,
        expected_revision: cancelled.revision,
        idempotency_key: "draft-retry-1",
        requested_by: "researcher-1",
    };
    let (retried, replayed) = store.retry_problem_draft(retry).await.expect("retry");
    assert!(!replayed);
    assert_eq!(retried.draft_id, generating.draft_id);
    assert_eq!(retried.status, ProblemDraftStatus::Generating);
    assert_eq!(retried.revision, cancelled.revision + 1);
    let (retry_replay, replayed) = store
        .retry_problem_draft(retry)
        .await
        .expect("retry replay");
    assert!(replayed);
    assert_eq!(retry_replay.revision, retried.revision);

    let attempts: Vec<(i64, String)> = sqlx::query_as(
        "SELECT attempt_number,status FROM problem_draft_attempts WHERE draft_id=? ORDER BY attempt_number",
    )
    .bind(&generating.draft_id)
    .fetch_all(store.pool())
    .await
    .expect("attempt history");
    assert_eq!(attempts, vec![(1, "failed".into()), (2, "running".into())]);
    let commands: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM problem_draft_commands WHERE draft_id=?")
            .bind(&generating.draft_id)
            .fetch_one(store.pool())
            .await
            .expect("control commands");
    assert_eq!(commands, 2);

    let conflicting = store
        .retry_problem_draft(ProblemDraftControlRequest {
            expected_revision: retried.revision,
            ..retry
        })
        .await;
    assert!(matches!(
        conflicting,
        Err(StorageError::IdempotencyConflict(_))
    ));
}

#[tokio::test]
async fn problem_draft_control_is_restricted_to_the_original_requester() {
    let (_temp, store) = store().await;
    let materials = vec![material()];
    let (generating, _) = store
        .begin_problem_draft(ProblemDraftBeginRequest {
            requested_by: "researcher-1",
            idempotency_key: "draft-create-owner",
            prompt: "Study the A to B question",
            material_directory: ".",
            materials: &materials,
            model: None,
        })
        .await
        .expect("begin");
    let denied = store
        .cancel_problem_draft(ProblemDraftControlRequest {
            draft_id: &generating.draft_id,
            expected_revision: generating.revision,
            idempotency_key: "draft-cancel-other-user",
            requested_by: "researcher-2",
        })
        .await;
    assert!(matches!(denied, Err(StorageError::InvalidTransition(_))));
    assert_eq!(
        store
            .get_problem_draft(&generating.draft_id)
            .await
            .expect("unchanged draft")
            .status,
        ProblemDraftStatus::Generating
    );
}

#[tokio::test]
async fn recovery_fails_interrupted_generating_drafts_and_their_attempts() {
    let (_temp, store) = store().await;
    let materials = vec![material()];
    let (generating, _) = store
        .begin_problem_draft(ProblemDraftBeginRequest {
            requested_by: "researcher-1",
            idempotency_key: "draft-create-recovery",
            prompt: "Study the A to B question",
            material_directory: ".",
            materials: &materials,
            model: None,
        })
        .await
        .expect("begin");
    assert_eq!(store.recover_interrupted_problem_drafts().await.unwrap(), 1);
    let recovered = store
        .get_problem_draft(&generating.draft_id)
        .await
        .expect("recovered draft");
    assert_eq!(recovered.status, ProblemDraftStatus::Failed);
    assert_eq!(recovered.revision, generating.revision + 1);
    assert_eq!(recovered.error_kind.as_deref(), Some("interrupted"));
    let attempt_status: String =
        sqlx::query_scalar("SELECT status FROM problem_draft_attempts WHERE draft_id=?")
            .bind(&generating.draft_id)
            .fetch_one(store.pool())
            .await
            .expect("attempt status");
    assert_eq!(attempt_status, "failed");
    let projects: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects")
        .fetch_one(store.pool())
        .await
        .expect("project count");
    assert_eq!(projects, 0);
}

#[tokio::test]
async fn confirmation_requires_warning_acknowledgement_and_commits_an_edited_document() {
    let (_temp, store) = store().await;
    let mut warned_material = material();
    warned_material.warning = Some("content was truncated to the configured byte limit".into());
    let materials = vec![warned_material];
    let (generating, _) = store
        .begin_problem_draft(ProblemDraftBeginRequest {
            requested_by: "researcher-1",
            idempotency_key: "draft-create-warning",
            prompt: "Study the A to B question",
            material_directory: "notes",
            materials: &materials,
            model: None,
        })
        .await
        .expect("begin");
    let generated_document = document();
    let awaiting = store
        .complete_problem_draft(ProblemDraftCompletionRequest {
            draft_id: &generating.draft_id,
            expected_revision: generating.revision,
            document: &generated_document,
            input_tokens: 0,
            output_tokens: 0,
            elapsed_ms: 1,
        })
        .await
        .expect("complete");
    let displayed_hash = awaiting.document_hash.clone().expect("document hash");
    let rejected = store
        .confirm_problem_draft(ProblemDraftConfirmationRequest {
            draft_id: &awaiting.draft_id,
            expected_revision: awaiting.revision,
            expected_document_hash: &displayed_hash,
            idempotency_key: "confirm-warning",
            requested_by: "researcher-1",
            start: false,
            edited_document: None,
            acknowledge_material_warnings: false,
        })
        .await;
    assert!(matches!(rejected, Err(StorageError::InvalidTransition(_))));

    let mut edited = generated_document;
    edited.name = "User-edited problem".into();
    let (confirmed, _) = store
        .confirm_problem_draft(ProblemDraftConfirmationRequest {
            draft_id: &awaiting.draft_id,
            expected_revision: awaiting.revision,
            expected_document_hash: &displayed_hash,
            idempotency_key: "confirm-warning",
            requested_by: "researcher-1",
            start: false,
            edited_document: Some(&edited),
            acknowledge_material_warnings: true,
        })
        .await
        .expect("acknowledged confirmation");
    assert_eq!(confirmed.project.name, "User-edited problem");
    assert_eq!(
        confirmed
            .draft
            .document
            .as_ref()
            .map(|document| document.name.as_str()),
        Some("User-edited problem")
    );
    assert_ne!(
        confirmed.draft.document_hash.as_deref(),
        Some(displayed_hash.as_str())
    );
    assert!(confirmed.start_command.is_none());
}
