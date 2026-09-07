use chrono::Utc;
use std::collections::BTreeSet;

use research_domain::{
    Candidate, CandidateStatus, CandidateSubmission, Verification, VerificationReport,
    VerificationVerdict,
};
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction};

use crate::{StorageError, StorageResult, json_text, new_id, rows};

pub(crate) struct CandidateInsert {
    pub candidate: Candidate,
    pub verification: Verification,
    pub inserted: bool,
    /// Present only for candidates originating in a Worker result. The result
    /// envelope remains the immutable copy of the model-provided references.
    pub source_reference_audit: Option<Value>,
    pub source_reference_failure_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct SourceReferenceNormalization {
    pub source_ids: Vec<String>,
    pub changed: bool,
    pub requires_alias_rewrite: bool,
    pub audit: Value,
}

#[derive(Debug, Clone)]
pub(crate) struct SourceReferenceRejection {
    pub summary: String,
    pub audit: Value,
}

#[derive(Debug)]
struct StoredSourceReference {
    source_id: String,
    status: String,
    citation_key: Option<String>,
    normalized_url: Option<String>,
    identifier_kind: Option<String>,
    identifier_value: Option<String>,
}

/// Resolve untrusted Worker-facing source references to the project's runtime
/// source IDs. Exact IDs take precedence. Aliases are accepted only when the
/// citation key or canonical source identity identifies exactly one row.
pub(crate) async fn normalize_source_references_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    references: &[String],
    temporary_aliases: &[(String, String)],
) -> StorageResult<Result<SourceReferenceNormalization, SourceReferenceRejection>> {
    if references.is_empty() {
        return Ok(Ok(SourceReferenceNormalization {
            source_ids: Vec::new(),
            changed: false,
            requires_alias_rewrite: false,
            audit: json!({"disposition":"unchanged","references":[]}),
        }));
    }
    let rows = sqlx::query("SELECT source_id,status,citation_key,normalized_url,identifier_kind,identifier_value FROM sources WHERE project_id=? ORDER BY source_id")
        .bind(project_id)
        .fetch_all(&mut **tx)
        .await?;
    let sources = rows
        .iter()
        .map(|row| {
            Ok(StoredSourceReference {
                source_id: row.try_get("source_id")?,
                status: row.try_get("status")?,
                citation_key: row.try_get("citation_key")?,
                normalized_url: row.try_get("normalized_url")?,
                identifier_kind: row.try_get("identifier_kind")?,
                identifier_value: row.try_get("identifier_value")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let mut normalized = Vec::with_capacity(references.len());
    let mut seen = BTreeSet::new();
    let mut resolutions = Vec::with_capacity(references.len());
    let mut issues = Vec::new();
    let mut requires_alias_rewrite = false;

    for provided in references {
        let reference = provided.trim();
        if reference.is_empty() {
            issues.push(json!({
                "provided":provided,
                "reason":"empty_source_reference",
                "matching_source_ids":[],
            }));
            continue;
        }
        let exact = sources.iter().find(|source| source.source_id == reference);
        let (matches, matched_by) = if let Some(source) = exact {
            (vec![source], "source_id")
        } else {
            let mut matching_source_ids = sources
                .iter()
                .filter(|source| source_alias_matches(source, reference))
                .map(|source| source.source_id.clone())
                .collect::<BTreeSet<_>>();
            let mut matched_by_envelope_alias = false;
            for (alias, source_id) in temporary_aliases {
                if alias.trim().eq_ignore_ascii_case(reference) {
                    matching_source_ids.insert(source_id.clone());
                    matched_by_envelope_alias = true;
                }
            }
            let matches = sources
                .iter()
                .filter(|source| matching_source_ids.contains(&source.source_id))
                .collect::<Vec<_>>();
            (
                matches,
                if matched_by_envelope_alias {
                    "envelope_alias"
                } else {
                    "project_unique_alias"
                },
            )
        };
        if matches.is_empty() {
            issues.push(json!({
                "provided":provided,
                "reason":"source_reference_not_found",
                "matching_source_ids":[],
            }));
            continue;
        }
        if matches.len() > 1 {
            issues.push(json!({
                "provided":provided,
                "reason":"ambiguous_source_reference",
                "matching_source_ids":matches.iter().map(|source| &source.source_id).collect::<Vec<_>>(),
            }));
            continue;
        }
        let source = matches[0];
        if !crate::source_ingestion::status_can_support_candidate(&source.status) {
            issues.push(json!({
                "provided":provided,
                "reason":"source_reference_is_not_evidence",
                "matching_source_ids":[source.source_id.clone()],
                "source_status":source.status,
            }));
            continue;
        }
        if seen.insert(source.source_id.clone()) {
            normalized.push(source.source_id.clone());
        }
        requires_alias_rewrite |= exact.is_none();
        resolutions.push(json!({
            "provided":provided,
            "source_id":source.source_id,
            "matched_by":matched_by,
        }));
    }

    if !issues.is_empty() {
        let count = issues.len();
        return Ok(Err(SourceReferenceRejection {
            summary: format!(
                "candidate has {count} missing, ambiguous, or non-evidence source reference(s)"
            ),
            audit: json!({
                "disposition":"rejected_untrusted",
                "resolved_references":resolutions,
                "issues":issues,
            }),
        }));
    }
    let trimmed_original = references
        .iter()
        .map(|value| value.trim().to_owned())
        .collect::<Vec<_>>();
    Ok(Ok(SourceReferenceNormalization {
        changed: normalized != trimmed_original,
        requires_alias_rewrite,
        source_ids: normalized,
        audit: json!({
            "disposition":"normalized",
            "references":resolutions,
        }),
    }))
}

fn source_alias_matches(source: &StoredSourceReference, reference: &str) -> bool {
    if source
        .citation_key
        .as_deref()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(reference))
    {
        return true;
    }
    let normalized_reference_url = reference
        .to_ascii_lowercase()
        .starts_with("http://")
        .then_some(reference)
        .or_else(|| {
            reference
                .to_ascii_lowercase()
                .starts_with("https://")
                .then_some(reference)
        })
        .map(|value| value.trim_end_matches('/'));
    if normalized_reference_url.is_some_and(|value| {
        source
            .normalized_url
            .as_deref()
            .is_some_and(|stored| stored == value)
    }) {
        return true;
    }
    let canonical = canonical_reference_identity(reference);
    if let Some((kind, value)) = canonical
        && source
            .identifier_kind
            .as_deref()
            .is_some_and(|stored| stored.eq_ignore_ascii_case(kind))
        && source
            .identifier_value
            .as_deref()
            .is_some_and(|stored| stored.eq_ignore_ascii_case(&value))
    {
        return true;
    }
    source
        .identifier_value
        .as_deref()
        .is_some_and(|stored| stored.trim().eq_ignore_ascii_case(reference))
}

fn canonical_reference_identity(reference: &str) -> Option<(&str, String)> {
    let lower = reference.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let (_, kind, value) = crate::normalized_source_identity(Some(reference), None);
        return kind.zip(value).map(|(kind, value)| {
            let stable_kind = match kind.as_str() {
                "doi" => "doi",
                "arxiv" => "arxiv",
                _ => "url",
            };
            (stable_kind, value)
        });
    }
    let (kind, value) = reference.split_once(':')?;
    let kind = kind.trim();
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if kind.eq_ignore_ascii_case("doi") {
        Some(("doi", value.to_ascii_lowercase()))
    } else if kind.eq_ignore_ascii_case("arxiv") {
        let value = value
            .strip_prefix("abs/")
            .unwrap_or(value)
            .strip_prefix("pdf/")
            .unwrap_or(value)
            .strip_suffix(".pdf")
            .unwrap_or(value);
        Some(("arxiv", value.to_ascii_lowercase()))
    } else if kind.eq_ignore_ascii_case("citation_key") {
        Some(("citation_key", value.to_ascii_lowercase()))
    } else {
        None
    }
}

pub(crate) fn untrusted_source_reference_report(
    rejection: &SourceReferenceRejection,
) -> VerificationReport {
    VerificationReport {
        verdict: VerificationVerdict::Unknown,
        summary: rejection.summary.clone(),
        critical_errors: vec![rejection.summary.clone()],
        gaps: Vec::new(),
        uncertainties: vec!["The cited source identity could not be trusted.".into()],
        repair_actions: vec![
            "Resubmit the candidate with an unambiguous source_id from this project.".into(),
        ],
        checked_fact_ids: Vec::new(),
        checked_source_ids: Vec::new(),
        evidence_level: "untrusted_source_reference".into(),
    }
}

/// Worker result ingestion is fail-closed per candidate rather than per result
/// envelope: an invalid source alias becomes an auditable terminal unknown
/// candidate and cannot make the same durable envelope retry forever.
pub(crate) async fn insert_worker_candidate_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    mut submission: CandidateSubmission,
    idempotency_key: &str,
    temporary_aliases: &[(String, String)],
) -> StorageResult<CandidateInsert> {
    if submission.statement.trim().is_empty() || submission.proof_markdown.trim().is_empty() {
        return Err(StorageError::InvalidTransition(
            "candidate statement and proof must not be empty".into(),
        ));
    }
    if idempotency_key.trim().is_empty() {
        return Err(StorageError::InvalidTransition(
            "candidate Idempotency-Key must not be empty".into(),
        ));
    }
    match normalize_source_references_tx(
        tx,
        project_id,
        &submission.external_source_ids,
        temporary_aliases,
    )
    .await?
    {
        Ok(normalization) => {
            submission.external_source_ids = normalization.source_ids;
            let audit = normalization.changed.then_some(normalization.audit);
            let mut inserted =
                insert_candidate_tx(tx, project_id, submission, idempotency_key).await?;
            inserted.source_reference_audit = audit;
            Ok(inserted)
        }
        Err(rejection) => {
            insert_untrusted_worker_candidate_tx(
                tx,
                project_id,
                submission,
                idempotency_key,
                rejection,
            )
            .await
        }
    }
}

async fn insert_untrusted_worker_candidate_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    submission: CandidateSubmission,
    idempotency_key: &str,
    rejection: SourceReferenceRejection,
) -> StorageResult<CandidateInsert> {
    let submission_json = json_text(&submission)?;
    if let Some(existing) =
        sqlx::query("SELECT * FROM candidates WHERE project_id=? AND idempotency_key=?")
            .bind(project_id)
            .bind(idempotency_key)
            .fetch_optional(&mut **tx)
            .await?
    {
        let stored_submission: String = existing.try_get("submission_json")?;
        if stored_submission != submission_json {
            return Err(StorageError::IdempotencyConflict(format!(
                "candidate key {idempotency_key} was already used for a different submission"
            )));
        }
        let candidate = rows::candidate(&existing)?;
        let verification_row = sqlx::query("SELECT * FROM verifications WHERE candidate_id=?")
            .bind(&candidate.candidate_id)
            .fetch_one(&mut **tx)
            .await?;
        return Ok(CandidateInsert {
            candidate,
            verification: rows::verification(&verification_row)?,
            inserted: false,
            source_reference_audit: Some(rejection.audit),
            source_reference_failure_id: None,
        });
    }
    let now = Utc::now();
    let candidate_id = new_id("candidate");
    let verification_id = new_id("verification");
    let failure_id = new_id("failure");
    let report = untrusted_source_reference_report(&rejection);
    sqlx::query("INSERT INTO candidates(candidate_id,project_id,submission_json,status,created_at,idempotency_key) VALUES(?,?,?,?,?,?)")
        .bind(&candidate_id).bind(project_id).bind(&submission_json)
        .bind(CandidateStatus::Unknown.to_string()).bind(now.to_rfc3339()).bind(idempotency_key)
        .execute(&mut **tx).await?;
    sqlx::query("INSERT INTO verifications(verification_id,candidate_id,project_id,status,report_json,completed_at) VALUES(?,?,?,?,?,?)")
        .bind(&verification_id).bind(&candidate_id).bind(project_id)
        .bind(CandidateStatus::Unknown.to_string()).bind(json_text(&report)?).bind(now.to_rfc3339())
        .execute(&mut **tx).await?;
    sqlx::query("INSERT INTO failures(failure_id,project_id,route_id,task_id,failure_type,summary,repairable,raw_json,created_at) VALUES(?,?,?,?,?,?,?,?,?)")
        .bind(&failure_id).bind(project_id).bind(&submission.route_id).bind(&submission.task_id)
        .bind("invalid_candidate_source_reference").bind(&rejection.summary).bind(true)
        .bind(json_text(&json!({"candidate_id":candidate_id,"verification_id":verification_id,"submission":submission,"source_reference_audit":rejection.audit}))?)
        .bind(now.to_rfc3339()).execute(&mut **tx).await?;
    Ok(CandidateInsert {
        candidate: Candidate {
            candidate_id: candidate_id.clone(),
            project_id: project_id.into(),
            submission,
            status: CandidateStatus::Unknown,
            created_at: now,
        },
        verification: Verification {
            verification_id,
            candidate_id,
            project_id: project_id.into(),
            status: CandidateStatus::Unknown,
            report: Some(report),
            started_at: None,
            completed_at: Some(now),
        },
        inserted: true,
        source_reference_audit: Some(rejection.audit),
        source_reference_failure_id: Some(failure_id),
    })
}

pub(crate) async fn insert_candidate_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    submission: CandidateSubmission,
    idempotency_key: &str,
) -> StorageResult<CandidateInsert> {
    if submission.statement.trim().is_empty() || submission.proof_markdown.trim().is_empty() {
        return Err(StorageError::InvalidTransition(
            "candidate statement and proof must not be empty".into(),
        ));
    }
    if idempotency_key.trim().is_empty() {
        return Err(StorageError::InvalidTransition(
            "candidate Idempotency-Key must not be empty".into(),
        ));
    }
    let submission_json = json_text(&submission)?;
    if let Some(existing) =
        sqlx::query("SELECT * FROM candidates WHERE project_id=? AND idempotency_key=?")
            .bind(project_id)
            .bind(idempotency_key)
            .fetch_optional(&mut **tx)
            .await?
    {
        let stored_submission: String = existing.try_get("submission_json")?;
        if stored_submission != submission_json {
            return Err(StorageError::IdempotencyConflict(format!(
                "candidate key {idempotency_key} was already used for a different submission"
            )));
        }
        let candidate = rows::candidate(&existing)?;
        let verification_row = sqlx::query("SELECT * FROM verifications WHERE candidate_id=?")
            .bind(&candidate.candidate_id)
            .fetch_one(&mut **tx)
            .await?;
        return Ok(CandidateInsert {
            candidate,
            verification: rows::verification(&verification_row)?,
            inserted: false,
            source_reference_audit: None,
            source_reference_failure_id: None,
        });
    }

    let row = sqlx::query("SELECT t.revision,t.status,t.route_cancellation_epoch AS task_route_epoch,r.status AS route_status,r.cancellation_epoch AS route_epoch,r.human_review,p.status AS project_status FROM tasks t JOIN routes r ON r.project_id=t.project_id AND r.route_id=t.route_id JOIN projects p ON p.project_id=t.project_id WHERE t.project_id=? AND t.task_id=? AND t.route_id=?")
        .bind(project_id)
        .bind(&submission.task_id)
        .bind(&submission.route_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "task",
            id: submission.task_id.clone(),
        })?;
    let revision: i64 = row.try_get("revision")?;
    let task_route_epoch: i64 = row.try_get("task_route_epoch")?;
    let route_epoch: i64 = row.try_get("route_epoch")?;
    let route_status: String = row.try_get("route_status")?;
    let human_review: String = row.try_get("human_review")?;
    let project_status: String = row.try_get("project_status")?;
    let task_status: String = row.try_get("status")?;
    let route_is_active = matches!(
        route_status.as_str(),
        "incubating" | "active" | "probation" | "revived"
    );
    if revision != submission.task_revision
        || task_route_epoch != submission.route_cancellation_epoch
        || route_epoch != submission.route_cancellation_epoch
        || !route_is_active
        || !crate::route_execution_is_released(&project_status, &human_review)
    {
        return Err(StorageError::LateSubmission(format!(
            "expected task rev/epoch {}/{}, actual revision={revision}, task_epoch={task_route_epoch}, route_epoch={route_epoch}, project={project_status}, route={route_status}, human_review={human_review}",
            submission.task_revision, submission.route_cancellation_epoch,
        )));
    }
    if !matches!(task_status.as_str(), "running" | "completed") {
        return Err(StorageError::InvalidTransition(format!(
            "task {} cannot submit candidates while {task_status}",
            submission.task_id
        )));
    }
    for fact_id in &submission.dependency_fact_ids {
        let active: Option<String> = sqlx::query_scalar("SELECT f.status FROM facts f WHERE f.fact_id=? AND (f.project_id=? OR EXISTS (SELECT 1 FROM project_fact_imports i WHERE i.target_project_id=? AND i.source_fact_id=f.fact_id AND i.status='active'))")
            .bind(fact_id)
            .bind(project_id)
            .bind(project_id)
            .fetch_optional(&mut **tx)
            .await?;
        if active.as_deref() != Some("active") {
            return Err(StorageError::InvalidDependency(fact_id.clone()));
        }
    }
    for source_id in &submission.external_source_ids {
        let status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM sources WHERE project_id=? AND source_id=?",
        )
        .bind(project_id)
        .bind(source_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "source",
            id: source_id.clone(),
        })?;
        if !crate::source_ingestion::status_can_support_candidate(&status) {
            return Err(StorageError::InvalidDependency(format!(
                "source {source_id} has non-evidence status {status}"
            )));
        }
    }

    let candidate_id = new_id("candidate");
    let verification_id = new_id("verification");
    let now = Utc::now();
    sqlx::query("INSERT INTO candidates(candidate_id,project_id,submission_json,status,created_at,idempotency_key) VALUES(?,?,?,?,?,?)")
        .bind(&candidate_id)
        .bind(project_id)
        .bind(&submission_json)
        .bind(CandidateStatus::Submitted.to_string())
        .bind(now.to_rfc3339())
        .bind(idempotency_key)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO verifications(verification_id,candidate_id,project_id,status) VALUES(?,?,?,?)",
    )
    .bind(&verification_id)
    .bind(&candidate_id)
    .bind(project_id)
    .bind(CandidateStatus::Submitted.to_string())
    .execute(&mut **tx)
    .await?;
    Ok(CandidateInsert {
        candidate: Candidate {
            candidate_id: candidate_id.clone(),
            project_id: project_id.into(),
            submission,
            status: CandidateStatus::Submitted,
            created_at: now,
        },
        verification: Verification {
            verification_id,
            candidate_id,
            project_id: project_id.into(),
            status: CandidateStatus::Submitted,
            report: None,
            started_at: None,
            completed_at: None,
        },
        inserted: true,
        source_reference_audit: None,
        source_reference_failure_id: None,
    })
}
