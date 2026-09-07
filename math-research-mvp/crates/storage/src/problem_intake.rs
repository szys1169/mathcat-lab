use std::{path::Component, str::FromStr};

use chrono::Utc;
use research_domain::{
    CommandMode, HumanCommand, ProblemAssumption, ProblemContract, ProblemDocument, ProblemDraft,
    ProblemDraftConfirmation, ProblemDraftStatus, ProblemMaterial, Project,
};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{Row, sqlite::SqliteRow};

use crate::{
    SqliteStore, StorageError, StorageResult, append_event, current_revision, entity, json_text,
    new_id, rows,
    state_writer::WritePriority,
    write::{CommandDraft, create_project_tx, enqueue_command_tx, validate_project_input},
};

#[derive(Debug, Clone, Copy)]
pub struct ProblemDraftBeginRequest<'a> {
    pub requested_by: &'a str,
    pub idempotency_key: &'a str,
    pub prompt: &'a str,
    pub material_directory: &'a str,
    pub materials: &'a [ProblemMaterial],
    pub model: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub struct ProblemDraftCompletionRequest<'a> {
    pub draft_id: &'a str,
    pub expected_revision: i64,
    pub document: &'a ProblemDocument,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub elapsed_ms: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct ProblemDraftFailureRequest<'a> {
    pub draft_id: &'a str,
    pub expected_revision: i64,
    pub error_kind: &'a str,
    pub error_message: &'a str,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub elapsed_ms: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct ProblemDraftControlRequest<'a> {
    pub draft_id: &'a str,
    pub expected_revision: i64,
    pub idempotency_key: &'a str,
    pub requested_by: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct ProblemDraftConfirmationRequest<'a> {
    pub draft_id: &'a str,
    pub expected_revision: i64,
    pub expected_document_hash: &'a str,
    pub idempotency_key: &'a str,
    pub requested_by: &'a str,
    pub start: bool,
    pub edited_document: Option<&'a ProblemDocument>,
    pub acknowledge_material_warnings: bool,
}

impl SqliteStore {
    /// Creates exactly one generating draft and model-attempt record for an idempotent request.
    ///
    /// The returned boolean is `replayed`: it is `true` only when the same requester and
    /// idempotency key already stored an identical creation request. A replay never creates a
    /// second draft or model attempt.
    pub async fn begin_problem_draft(
        &self,
        request: ProblemDraftBeginRequest<'_>,
    ) -> StorageResult<(ProblemDraft, bool)> {
        validate_begin_request(&request)?;
        let material_manifest_hash = problem_material_manifest_hash(request.materials)?;
        let creation_request_hash = sha256_json(&json!({
            "prompt": request.prompt.trim(),
            "material_directory": request.material_directory.trim(),
            "materials": request.materials,
            "material_manifest_hash": material_manifest_hash,
            "model": request.model.map(str::trim).filter(|value| !value.is_empty()),
        }))?;
        let _admission = self
            .admit_write(WritePriority::PlanningOrCheckpoint, "begin_problem_draft")
            .await?;
        let mut tx = self.pool().begin().await?;
        if let Some(row) = sqlx::query(
            "SELECT * FROM problem_drafts WHERE requested_by=? AND creation_idempotency_key=?",
        )
        .bind(request.requested_by.trim())
        .bind(request.idempotency_key.trim())
        .fetch_optional(&mut *tx)
        .await?
        {
            let existing = problem_draft_from_row(&row)?;
            if existing.creation_request_hash != creation_request_hash {
                return Err(StorageError::IdempotencyConflict(format!(
                    "problem draft creation key {} was reused with a different request",
                    request.idempotency_key
                )));
            }
            tx.commit().await?;
            return Ok((existing, true));
        }

        let draft_id = new_id("problem_draft");
        let attempt_id = new_id("problem_draft_attempt");
        let now = Utc::now();
        let model = request
            .model
            .map(str::trim)
            .filter(|value| !value.is_empty());
        sqlx::query("INSERT INTO problem_drafts(draft_id,requested_by,creation_idempotency_key,creation_request_hash,prompt,material_directory,materials_json,material_manifest_hash,status,revision,model,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,'generating',1,?,?,?)")
            .bind(&draft_id)
            .bind(request.requested_by.trim())
            .bind(request.idempotency_key.trim())
            .bind(&creation_request_hash)
            .bind(request.prompt.trim())
            .bind(request.material_directory.trim())
            .bind(json_text(request.materials)?)
            .bind(&material_manifest_hash)
            .bind(model)
            .bind(now.to_rfc3339())
            .bind(now.to_rfc3339())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO problem_draft_attempts(attempt_id,draft_id,attempt_number,status,model,started_at) VALUES(?,?,1,'running',?,?)")
            .bind(attempt_id)
            .bind(&draft_id)
            .bind(model)
            .bind(now.to_rfc3339())
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query("SELECT * FROM problem_drafts WHERE draft_id=?")
            .bind(&draft_id)
            .fetch_one(&mut *tx)
            .await?;
        let draft = problem_draft_from_row(&row)?;
        tx.commit().await?;
        Ok((draft, false))
    }

    pub async fn get_problem_draft(&self, draft_id: &str) -> StorageResult<ProblemDraft> {
        let row = sqlx::query("SELECT * FROM problem_drafts WHERE draft_id=?")
            .bind(draft_id)
            .fetch_optional(self.read_pool())
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "problem_draft",
                id: draft_id.into(),
            })?;
        problem_draft_from_row(&row)
    }

    /// Cancels exactly one active generation attempt and records an idempotent control command.
    pub async fn cancel_problem_draft(
        &self,
        request: ProblemDraftControlRequest<'_>,
    ) -> StorageResult<(ProblemDraft, bool)> {
        validate_control_request(&request)?;
        let request_hash = control_request_hash("cancel", &request)?;
        let _admission = self
            .admit_write(WritePriority::HumanSafety, "cancel_problem_draft")
            .await?;
        let mut tx = self.pool().begin().await?;
        if let Some(draft) =
            replayed_control_command(&mut tx, "cancel", &request, &request_hash).await?
        {
            tx.commit().await?;
            return Ok((draft, true));
        }

        let row = get_problem_draft_row(&mut tx, request.draft_id).await?;
        let draft = problem_draft_from_row(&row)?;
        require_draft_requester(&draft, request.requested_by)?;
        require_generating_revision(&draft, request.expected_revision)?;
        let now = Utc::now().to_rfc3339();
        let error_kind = "cancelled";
        let error_message = "problem draft generation was cancelled by its requester";
        let updated = sqlx::query("UPDATE problem_drafts SET status='failed',revision=revision+1,error_kind=?,error_message=?,updated_at=?,generation_completed_at=? WHERE draft_id=? AND status='generating' AND revision=?")
            .bind(error_kind)
            .bind(error_message)
            .bind(&now)
            .bind(&now)
            .bind(request.draft_id)
            .bind(request.expected_revision)
            .execute(&mut *tx)
            .await?;
        require_single_transition(updated.rows_affected(), "cancel problem draft")?;
        finish_running_attempt(
            &mut tx,
            request.draft_id,
            "failed",
            0,
            0,
            0,
            Some(error_kind),
            Some(error_message),
            &now,
        )
        .await?;
        let result_revision = request.expected_revision.saturating_add(1);
        record_control_command(
            &mut tx,
            "cancel",
            &request,
            &request_hash,
            result_revision,
            &now,
        )
        .await?;
        let row = get_problem_draft_row(&mut tx, request.draft_id).await?;
        let draft = problem_draft_from_row(&row)?;
        tx.commit().await?;
        Ok((draft, false))
    }

    /// Starts a new durable attempt on the same failed draft, preserving all prior attempts.
    pub async fn retry_problem_draft(
        &self,
        request: ProblemDraftControlRequest<'_>,
    ) -> StorageResult<(ProblemDraft, bool)> {
        validate_control_request(&request)?;
        let request_hash = control_request_hash("retry", &request)?;
        let _admission = self
            .admit_write(WritePriority::PlanningOrCheckpoint, "retry_problem_draft")
            .await?;
        let mut tx = self.pool().begin().await?;
        if let Some(draft) =
            replayed_control_command(&mut tx, "retry", &request, &request_hash).await?
        {
            tx.commit().await?;
            return Ok((draft, true));
        }

        let row = get_problem_draft_row(&mut tx, request.draft_id).await?;
        let draft = problem_draft_from_row(&row)?;
        require_draft_requester(&draft, request.requested_by)?;
        if draft.revision != request.expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: request.expected_revision,
                actual: draft.revision,
            });
        }
        if draft.status != ProblemDraftStatus::Failed {
            return Err(StorageError::InvalidTransition(format!(
                "problem draft {} can only retry from failed state",
                draft.draft_id
            )));
        }
        let running_attempts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM problem_draft_attempts WHERE draft_id=? AND status='running'",
        )
        .bind(request.draft_id)
        .fetch_one(&mut *tx)
        .await?;
        if running_attempts != 0 {
            return Err(StorageError::CorruptData(format!(
                "failed problem draft {} still has a running attempt",
                draft.draft_id
            )));
        }
        let attempt_number: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(attempt_number),0)+1 FROM problem_draft_attempts WHERE draft_id=?",
        )
        .bind(request.draft_id)
        .fetch_one(&mut *tx)
        .await?;
        let now = Utc::now().to_rfc3339();
        let updated = sqlx::query("UPDATE problem_drafts SET status='generating',revision=revision+1,document_json=NULL,document_hash=NULL,input_tokens=0,output_tokens=0,elapsed_ms=0,error_kind=NULL,error_message=NULL,updated_at=?,generation_completed_at=NULL WHERE draft_id=? AND status='failed' AND revision=?")
            .bind(&now)
            .bind(request.draft_id)
            .bind(request.expected_revision)
            .execute(&mut *tx)
            .await?;
        require_single_transition(updated.rows_affected(), "retry problem draft")?;
        sqlx::query("INSERT INTO problem_draft_attempts(attempt_id,draft_id,attempt_number,status,model,started_at) VALUES(?,?,?,'running',?,?)")
            .bind(new_id("problem_draft_attempt"))
            .bind(request.draft_id)
            .bind(attempt_number)
            .bind(draft.model.as_deref())
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        let result_revision = request.expected_revision.saturating_add(1);
        record_control_command(
            &mut tx,
            "retry",
            &request,
            &request_hash,
            result_revision,
            &now,
        )
        .await?;
        let row = get_problem_draft_row(&mut tx, request.draft_id).await?;
        let draft = problem_draft_from_row(&row)?;
        tx.commit().await?;
        Ok((draft, false))
    }

    /// Commits structured model output only while the exact generating revision is current.
    pub async fn complete_problem_draft(
        &self,
        request: ProblemDraftCompletionRequest<'_>,
    ) -> StorageResult<ProblemDraft> {
        let _admission = self
            .admit_write(
                WritePriority::PlanningOrCheckpoint,
                "complete_problem_draft",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = get_problem_draft_row(&mut tx, request.draft_id).await?;
        let draft = problem_draft_from_row(&row)?;
        require_generating_revision(&draft, request.expected_revision)?;
        validate_problem_document(request.document, &draft.materials)?;
        let document_hash = problem_document_hash(request.document)?;
        let now = Utc::now();
        let input_tokens = request.input_tokens.max(0);
        let output_tokens = request.output_tokens.max(0);
        let elapsed_ms = request.elapsed_ms.max(0);
        let updated = sqlx::query("UPDATE problem_drafts SET status='awaiting_confirmation',revision=revision+1,document_json=?,document_hash=?,input_tokens=?,output_tokens=?,elapsed_ms=?,error_kind=NULL,error_message=NULL,updated_at=?,generation_completed_at=? WHERE draft_id=? AND status='generating' AND revision=?")
            .bind(json_text(request.document)?)
            .bind(document_hash)
            .bind(input_tokens)
            .bind(output_tokens)
            .bind(elapsed_ms)
            .bind(now.to_rfc3339())
            .bind(now.to_rfc3339())
            .bind(request.draft_id)
            .bind(request.expected_revision)
            .execute(&mut *tx)
            .await?;
        require_single_transition(updated.rows_affected(), "complete problem draft")?;
        finish_running_attempt(
            &mut tx,
            request.draft_id,
            "succeeded",
            input_tokens,
            output_tokens,
            elapsed_ms,
            None,
            None,
            &now.to_rfc3339(),
        )
        .await?;
        let row = get_problem_draft_row(&mut tx, request.draft_id).await?;
        let draft = problem_draft_from_row(&row)?;
        tx.commit().await?;
        Ok(draft)
    }

    /// Records a bounded, public-safe failure without leaving a running intake attempt.
    pub async fn fail_problem_draft(
        &self,
        request: ProblemDraftFailureRequest<'_>,
    ) -> StorageResult<ProblemDraft> {
        if request.error_kind.trim().is_empty() {
            return Err(StorageError::InvalidTransition(
                "problem draft error kind must not be empty".into(),
            ));
        }
        let _admission = self
            .admit_write(WritePriority::PlanningOrCheckpoint, "fail_problem_draft")
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = get_problem_draft_row(&mut tx, request.draft_id).await?;
        let draft = problem_draft_from_row(&row)?;
        require_generating_revision(&draft, request.expected_revision)?;
        let input_tokens = request.input_tokens.max(0);
        let output_tokens = request.output_tokens.max(0);
        let elapsed_ms = request.elapsed_ms.max(0);
        let error_kind = request.error_kind.trim();
        let error_message = public_safe_error(error_kind, request.error_message);
        let now = Utc::now();
        let updated = sqlx::query("UPDATE problem_drafts SET status='failed',revision=revision+1,input_tokens=?,output_tokens=?,elapsed_ms=?,error_kind=?,error_message=?,updated_at=?,generation_completed_at=? WHERE draft_id=? AND status='generating' AND revision=?")
            .bind(input_tokens)
            .bind(output_tokens)
            .bind(elapsed_ms)
            .bind(error_kind)
            .bind(&error_message)
            .bind(now.to_rfc3339())
            .bind(now.to_rfc3339())
            .bind(request.draft_id)
            .bind(request.expected_revision)
            .execute(&mut *tx)
            .await?;
        require_single_transition(updated.rows_affected(), "fail problem draft")?;
        finish_running_attempt(
            &mut tx,
            request.draft_id,
            "failed",
            input_tokens,
            output_tokens,
            elapsed_ms,
            Some(error_kind),
            Some(&error_message),
            &now.to_rfc3339(),
        )
        .await?;
        let row = get_problem_draft_row(&mut tx, request.draft_id).await?;
        let draft = problem_draft_from_row(&row)?;
        tx.commit().await?;
        Ok(draft)
    }

    /// Fails attempts whose process disappeared before committing a terminal result.
    pub async fn recover_interrupted_problem_drafts(&self) -> StorageResult<u64> {
        let _admission = self
            .admit_write(
                WritePriority::PlanningOrCheckpoint,
                "recover_interrupted_problem_drafts",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE problem_draft_attempts SET status='failed',error_kind='interrupted',error_message='problem draft generation was interrupted before completion',completed_at=? WHERE status='running' AND draft_id IN (SELECT draft_id FROM problem_drafts WHERE status='generating')")
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        let updated = sqlx::query("UPDATE problem_drafts SET status='failed',revision=revision+1,error_kind='interrupted',error_message='problem draft generation was interrupted before completion',updated_at=?,generation_completed_at=? WHERE status='generating'")
            .bind(&now)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(updated.rows_affected())
    }

    /// Confirms exactly the displayed draft revision and atomically creates its research project.
    pub async fn confirm_problem_draft(
        &self,
        request: ProblemDraftConfirmationRequest<'_>,
    ) -> StorageResult<(ProblemDraftConfirmation, Vec<research_domain::DomainEvent>)> {
        validate_confirmation_request(&request)?;
        let confirmation_request_hash = sha256_json(&json!({
            "draft_id": request.draft_id,
            "expected_revision": request.expected_revision,
            "expected_document_hash": request.expected_document_hash,
            "requested_by": request.requested_by.trim(),
            "start": request.start,
            "edited_document": request.edited_document,
            "acknowledge_material_warnings": request.acknowledge_material_warnings,
        }))?;
        let _admission = self
            .admit_write(WritePriority::HumanSafety, "confirm_problem_draft")
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = get_problem_draft_row(&mut tx, request.draft_id).await?;
        let draft = problem_draft_from_row(&row)?;
        if draft.status == ProblemDraftStatus::Confirmed {
            if draft.confirmation_idempotency_key.as_deref() != Some(request.idempotency_key.trim())
                || draft.confirmation_request_hash.as_deref()
                    != Some(confirmation_request_hash.as_str())
            {
                return Err(StorageError::IdempotencyConflict(format!(
                    "problem draft {} was already confirmed with a different request",
                    request.draft_id
                )));
            }
            let project = confirmed_project(&mut tx, &draft).await?;
            let start_command = confirmed_start_command(&mut tx, &draft).await?;
            tx.commit().await?;
            return Ok((
                ProblemDraftConfirmation {
                    draft,
                    project,
                    start_command,
                    replayed: true,
                },
                vec![],
            ));
        }
        if draft.status != ProblemDraftStatus::AwaitingConfirmation {
            return Err(StorageError::InvalidTransition(format!(
                "problem draft {} cannot be confirmed while {}",
                draft.draft_id, draft.status
            )));
        }
        if draft.revision != request.expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: request.expected_revision,
                actual: draft.revision,
            });
        }
        if draft.document_hash.as_deref() != Some(request.expected_document_hash) {
            return Err(StorageError::InvalidTransition(
                "problem draft document hash is stale".into(),
            ));
        }
        if !request.acknowledge_material_warnings && has_material_warnings(&draft.materials) {
            return Err(StorageError::InvalidTransition(
                "material warnings must be acknowledged before confirmation".into(),
            ));
        }
        let document = request
            .edited_document
            .cloned()
            .or(draft.document.clone())
            .ok_or_else(|| {
                StorageError::CorruptData(format!(
                    "awaiting-confirmation draft {} has no document",
                    draft.draft_id
                ))
            })?;
        validate_problem_document(&document, &draft.materials)?;
        let document_hash = problem_document_hash(&document)?;
        let contract = ProblemContract {
            original_problem: document.problem.clone(),
            target_statement: document.target_statement.clone(),
            assumptions: document
                .assumptions
                .iter()
                .map(|assumption| assumption.statement.clone())
                .collect(),
            success_criteria: document.success_criteria.clone(),
            version: 1,
        };
        validate_project_input(&document.name, &contract, &document.budget)?;
        let (created_project, project_created) = create_project_tx(
            &mut tx,
            &document.name,
            contract,
            document.budget.clone(),
            if document.human_route_approval {
                research_domain::ReviewMode::Strict
            } else {
                research_domain::ReviewMode::Balanced
            },
        )
        .await?;
        let mut events = vec![project_created];
        let start_command = if request.start {
            let (command, event) = enqueue_command_tx(
                &mut tx,
                &created_project.project_id,
                CommandDraft {
                    command_type: "start_project".into(),
                    target_kind: "project".into(),
                    target_id: created_project.project_id.clone(),
                    mode: CommandMode::Immediate,
                    payload: json!({"source_problem_draft_id": request.draft_id}),
                    expected_project_revision: created_project.revision,
                    idempotency_key: format!(
                        "problem-draft-confirm:{}:{}",
                        request.draft_id,
                        request.idempotency_key.trim()
                    ),
                    reason: format!("confirmed problem draft {}", request.draft_id),
                    requested_by: request.requested_by.trim().into(),
                },
            )
            .await?;
            events.push(event);
            Some(command)
        } else {
            None
        };
        let project_revision = current_revision(&mut tx, &created_project.project_id).await?;
        let now = Utc::now();
        let updated = sqlx::query("UPDATE problem_drafts SET status='confirmed',revision=revision+1,document_json=?,document_hash=?,confirmation_idempotency_key=?,confirmation_request_hash=?,confirmed_project_id=?,start_command_id=?,updated_at=?,confirmed_at=? WHERE draft_id=? AND status='awaiting_confirmation' AND revision=? AND document_hash=?")
            .bind(json_text(&document)?)
            .bind(&document_hash)
            .bind(request.idempotency_key.trim())
            .bind(&confirmation_request_hash)
            .bind(&created_project.project_id)
            .bind(start_command.as_ref().map(|command| command.command_id.as_str()))
            .bind(now.to_rfc3339())
            .bind(now.to_rfc3339())
            .bind(request.draft_id)
            .bind(request.expected_revision)
            .bind(request.expected_document_hash)
            .execute(&mut *tx)
            .await?;
        require_single_transition(updated.rows_affected(), "confirm problem draft")?;
        events.push(
            append_event(
                &mut tx,
                &created_project.project_id,
                project_revision,
                "problem_draft.confirmed",
                entity("problem_draft", request.draft_id),
                json!({
                    "document_hash": document_hash,
                    "material_manifest_hash": draft.material_manifest_hash,
                    "start_command_id": start_command.as_ref().map(|command| &command.command_id),
                }),
                None,
            )
            .await?,
        );
        let row = get_problem_draft_row(&mut tx, request.draft_id).await?;
        let confirmed_draft = problem_draft_from_row(&row)?;
        let project_row = sqlx::query("SELECT * FROM projects WHERE project_id=?")
            .bind(&created_project.project_id)
            .fetch_one(&mut *tx)
            .await?;
        let project = rows::project(&project_row)?;
        tx.commit().await?;
        Ok((
            ProblemDraftConfirmation {
                draft: confirmed_draft,
                project,
                start_command,
                replayed: false,
            },
            events,
        ))
    }
}

pub fn problem_material_manifest_hash(materials: &[ProblemMaterial]) -> StorageResult<String> {
    sha256_json(materials)
}

pub fn problem_document_hash(document: &ProblemDocument) -> StorageResult<String> {
    sha256_json(document)
}

fn sha256_json(value: &(impl Serialize + ?Sized)) -> StorageResult<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}

fn validate_begin_request(request: &ProblemDraftBeginRequest<'_>) -> StorageResult<()> {
    if request.requested_by.trim().is_empty()
        || request.idempotency_key.trim().is_empty()
        || request.prompt.trim().is_empty()
    {
        return Err(StorageError::InvalidTransition(
            "problem draft requester, Idempotency-Key, and prompt must not be empty".into(),
        ));
    }
    validate_material_directory(request.material_directory)?;
    for material in request.materials {
        validate_material(material)?;
    }
    Ok(())
}

fn validate_material_directory(directory: &str) -> StorageResult<()> {
    let trimmed = directory.trim();
    let path = std::path::Path::new(trimmed);
    if trimmed.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(StorageError::InvalidTransition(
            "problem material directory must be '.' or a safe relative path".into(),
        ));
    }
    Ok(())
}

fn validate_material(material: &ProblemMaterial) -> StorageResult<()> {
    let path = std::path::Path::new(&material.relative_path);
    if material.relative_path.trim().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(StorageError::InvalidTransition(
            "problem material path must be a non-empty relative path without parent traversal"
                .into(),
        ));
    }
    if material.media_type.trim().is_empty()
        || material.status.trim().is_empty()
        || material.included_bytes > material.byte_size
        || (material.included_bytes > 0 && material.sha256.trim().is_empty())
    {
        return Err(StorageError::InvalidTransition(format!(
            "problem material {} has invalid metadata",
            material.relative_path
        )));
    }
    if material
        .warning
        .as_deref()
        .is_some_and(contains_absolute_path)
    {
        return Err(StorageError::InvalidTransition(
            "problem material warning must not contain an absolute path".into(),
        ));
    }
    Ok(())
}

fn validate_problem_document(
    document: &ProblemDocument,
    materials: &[ProblemMaterial],
) -> StorageResult<()> {
    let contract = ProblemContract {
        original_problem: document.problem.clone(),
        target_statement: document.target_statement.clone(),
        assumptions: document
            .assumptions
            .iter()
            .map(|assumption| assumption.statement.clone())
            .collect(),
        success_criteria: document.success_criteria.clone(),
        version: 1,
    };
    validate_project_input(&document.name, &contract, &document.budget)?;
    if document.problem.trim().is_empty()
        || document.success_criteria.trim().is_empty()
        || document.budget_rationale.trim().is_empty()
        || document
            .assumptions
            .iter()
            .any(|assumption: &ProblemAssumption| assumption.statement.trim().is_empty())
    {
        return Err(StorageError::InvalidTransition(
            "problem, success criteria, every present assumption, and budget rationale must not be empty"
                .into(),
        ));
    }
    for reference in &document.material_references {
        if !materials.iter().any(|material| {
            material.relative_path == *reference
                && material.status == "included"
                && material.included_bytes > 0
        }) {
            return Err(StorageError::InvalidTransition(
                "problem document references material that was not included".into(),
            ));
        }
    }
    Ok(())
}

fn validate_confirmation_request(
    request: &ProblemDraftConfirmationRequest<'_>,
) -> StorageResult<()> {
    if request.draft_id.trim().is_empty()
        || request.expected_document_hash.trim().is_empty()
        || request.idempotency_key.trim().is_empty()
        || request.requested_by.trim().is_empty()
    {
        return Err(StorageError::InvalidTransition(
            "draft id, document hash, Idempotency-Key, and requester must not be empty".into(),
        ));
    }
    Ok(())
}

fn validate_control_request(request: &ProblemDraftControlRequest<'_>) -> StorageResult<()> {
    if request.draft_id.trim().is_empty()
        || request.idempotency_key.trim().is_empty()
        || request.requested_by.trim().is_empty()
        || request.expected_revision <= 0
    {
        return Err(StorageError::InvalidTransition(
            "draft id, positive expected revision, Idempotency-Key, and requester must not be empty"
                .into(),
        ));
    }
    Ok(())
}

fn control_request_hash(
    command_type: &str,
    request: &ProblemDraftControlRequest<'_>,
) -> StorageResult<String> {
    sha256_json(&json!({
        "command_type": command_type,
        "draft_id": request.draft_id.trim(),
        "expected_revision": request.expected_revision,
    }))
}

fn require_draft_requester(draft: &ProblemDraft, requested_by: &str) -> StorageResult<()> {
    if draft.requested_by == requested_by.trim() {
        Ok(())
    } else {
        Err(StorageError::InvalidTransition(
            "only the problem-draft requester may control it".into(),
        ))
    }
}

fn require_generating_revision(draft: &ProblemDraft, expected_revision: i64) -> StorageResult<()> {
    if draft.revision != expected_revision {
        return Err(StorageError::RevisionConflict {
            expected: expected_revision,
            actual: draft.revision,
        });
    }
    if draft.status != ProblemDraftStatus::Generating {
        return Err(StorageError::InvalidTransition(format!(
            "problem draft {} cannot finish while {}",
            draft.draft_id, draft.status
        )));
    }
    Ok(())
}

fn require_single_transition(rows_affected: u64, operation: &str) -> StorageResult<()> {
    if rows_affected == 1 {
        Ok(())
    } else {
        Err(StorageError::InvalidTransition(format!(
            "{operation} lost its state-transition race"
        )))
    }
}

async fn replayed_control_command(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    command_type: &str,
    request: &ProblemDraftControlRequest<'_>,
    request_hash: &str,
) -> StorageResult<Option<ProblemDraft>> {
    let existing = sqlx::query("SELECT draft_id,command_type,request_hash FROM problem_draft_commands WHERE requested_by=? AND idempotency_key=?")
        .bind(request.requested_by.trim())
        .bind(request.idempotency_key.trim())
        .fetch_optional(&mut **tx)
        .await?;
    let Some(existing) = existing else {
        return Ok(None);
    };
    let existing_draft_id: String = existing.try_get("draft_id")?;
    let existing_type: String = existing.try_get("command_type")?;
    let existing_hash: String = existing.try_get("request_hash")?;
    if existing_draft_id != request.draft_id
        || existing_type != command_type
        || existing_hash != request_hash
    {
        return Err(StorageError::IdempotencyConflict(format!(
            "problem draft control key {} was reused with a different request",
            request.idempotency_key
        )));
    }
    let row = get_problem_draft_row(tx, request.draft_id).await?;
    problem_draft_from_row(&row).map(Some)
}

async fn record_control_command(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    command_type: &str,
    request: &ProblemDraftControlRequest<'_>,
    request_hash: &str,
    result_revision: i64,
    created_at: &str,
) -> StorageResult<()> {
    sqlx::query("INSERT INTO problem_draft_commands(command_id,draft_id,command_type,requested_by,idempotency_key,request_hash,result_revision,created_at) VALUES(?,?,?,?,?,?,?,?)")
        .bind(new_id("problem_draft_command"))
        .bind(request.draft_id)
        .bind(command_type)
        .bind(request.requested_by.trim())
        .bind(request.idempotency_key.trim())
        .bind(request_hash)
        .bind(result_revision)
        .bind(created_at)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn has_material_warnings(materials: &[ProblemMaterial]) -> bool {
    materials.iter().any(|material| {
        material
            .warning
            .as_deref()
            .is_some_and(|warning| !warning.trim().is_empty())
    })
}

fn public_safe_error(error_kind: &str, message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() || contains_absolute_path(trimmed) {
        format!("problem draft generation failed ({error_kind})")
    } else {
        trimmed.chars().take(2_000).collect()
    }
}

fn contains_absolute_path(value: &str) -> bool {
    value.contains(":\\")
        || value.contains(":/")
        || value.starts_with("\\\\")
        || value
            .split_whitespace()
            .any(|token| std::path::Path::new(token).is_absolute())
}

async fn get_problem_draft_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    draft_id: &str,
) -> StorageResult<SqliteRow> {
    sqlx::query("SELECT * FROM problem_drafts WHERE draft_id=?")
        .bind(draft_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "problem_draft",
            id: draft_id.into(),
        })
}

#[allow(clippy::too_many_arguments)]
async fn finish_running_attempt(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    draft_id: &str,
    status: &str,
    input_tokens: i64,
    output_tokens: i64,
    elapsed_ms: i64,
    error_kind: Option<&str>,
    error_message: Option<&str>,
    completed_at: &str,
) -> StorageResult<()> {
    let updated = sqlx::query("UPDATE problem_draft_attempts SET status=?,input_tokens=?,output_tokens=?,elapsed_ms=?,error_kind=?,error_message=?,completed_at=? WHERE attempt_id=(SELECT attempt_id FROM problem_draft_attempts WHERE draft_id=? AND status='running' ORDER BY attempt_number DESC LIMIT 1) AND status='running'")
        .bind(status)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(elapsed_ms)
        .bind(error_kind)
        .bind(error_message)
        .bind(completed_at)
        .bind(draft_id)
        .execute(&mut **tx)
        .await?;
    require_single_transition(updated.rows_affected(), "finish problem draft attempt")
}

async fn confirmed_project(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    draft: &ProblemDraft,
) -> StorageResult<Project> {
    let project_id = draft.confirmed_project_id.as_deref().ok_or_else(|| {
        StorageError::CorruptData(format!("confirmed draft {} has no project", draft.draft_id))
    })?;
    let row = sqlx::query("SELECT * FROM projects WHERE project_id=?")
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            StorageError::CorruptData(format!("confirmed project {project_id} is missing"))
        })?;
    rows::project(&row)
}

async fn confirmed_start_command(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    draft: &ProblemDraft,
) -> StorageResult<Option<HumanCommand>> {
    let Some(command_id) = draft.start_command_id.as_deref() else {
        return Ok(None);
    };
    let row = sqlx::query("SELECT * FROM human_commands WHERE command_id=?")
        .bind(command_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            StorageError::CorruptData(format!(
                "problem draft start command {command_id} is missing"
            ))
        })?;
    rows::command(&row).map(Some)
}

fn problem_draft_from_row(row: &SqliteRow) -> StorageResult<ProblemDraft> {
    let document = row
        .try_get::<Option<String>, _>("document_json")?
        .map(|value| serde_json::from_str(&value))
        .transpose()?;
    Ok(ProblemDraft {
        draft_id: row.try_get("draft_id")?,
        requested_by: row.try_get("requested_by")?,
        creation_idempotency_key: row.try_get("creation_idempotency_key")?,
        creation_request_hash: row.try_get("creation_request_hash")?,
        prompt: row.try_get("prompt")?,
        material_directory: row.try_get("material_directory")?,
        materials: serde_json::from_str(row.try_get("materials_json")?)?,
        status: ProblemDraftStatus::from_str(row.try_get("status")?)
            .map_err(StorageError::CorruptData)?,
        revision: row.try_get("revision")?,
        document,
        document_hash: row.try_get("document_hash")?,
        material_manifest_hash: row.try_get("material_manifest_hash")?,
        model: row.try_get("model")?,
        input_tokens: row.try_get("input_tokens")?,
        output_tokens: row.try_get("output_tokens")?,
        elapsed_ms: row.try_get("elapsed_ms")?,
        error_kind: row.try_get("error_kind")?,
        error_message: row.try_get("error_message")?,
        confirmation_idempotency_key: row.try_get("confirmation_idempotency_key")?,
        confirmation_request_hash: row.try_get("confirmation_request_hash")?,
        confirmed_project_id: row.try_get("confirmed_project_id")?,
        start_command_id: row.try_get("start_command_id")?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
        updated_at: rows::timestamp(row.try_get("updated_at")?)?,
        generation_completed_at: rows::optional_timestamp(row.try_get("generation_completed_at")?)?,
        confirmed_at: rows::optional_timestamp(row.try_get("confirmed_at")?)?,
    })
}
