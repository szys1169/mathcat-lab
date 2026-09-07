use chrono::Utc;
use research_domain::{SourceDraft, Task, WorkerOutput};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, Transaction};

use crate::{StorageResult, json_text, new_id, source_ingestion::SourceAdmission};

#[derive(Debug, Clone, Copy)]
pub(crate) enum WorkerOutputOrigin {
    Legacy,
    Leased,
}

impl WorkerOutputOrigin {
    const fn provenance(self, upgraded: bool) -> &'static str {
        match (self, upgraded) {
            (Self::Legacy, false) => "worker_output",
            (Self::Legacy, true) => "worker_output_upgrade",
            (Self::Leased, false) => "leased_worker_result",
            (Self::Leased, true) => "leased_worker_result_upgrade",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct WorkerOutputIngestion {
    pub recorded_sources: Vec<(String, String, SourceAdmission)>,
    pub rejected_source_drafts: Vec<(String, Vec<&'static str>)>,
    pub capsule_events: Vec<(String, String)>,
    /// Aliases observed in this immutable result envelope, including aliases on
    /// source drafts deduplicated onto an older source row.
    pub source_reference_aliases: Vec<(String, String)>,
    pub inserted_source_count: usize,
    pub upgraded_source_count: usize,
}

impl WorkerOutputIngestion {
    fn remember_source_alias(&mut self, source: &SourceDraft, source_id: &str) {
        if let Some(citation_key) = source
            .citation_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.source_reference_aliases
                .push((citation_key.to_owned(), source_id.to_owned()));
        }
    }
}

/// Persist the untrusted factual payload shared by both the legacy local-task
/// adapter and the fenced V2 result-envelope path. Lifecycle transitions and
/// their events remain with each caller because their fencing contracts differ.
pub(crate) async fn ingest_worker_output_tx(
    tx: &mut Transaction<'_, Sqlite>,
    task: &Task,
    output: &WorkerOutput,
    origin: WorkerOutputOrigin,
) -> StorageResult<WorkerOutputIngestion> {
    let project_id = &task.project_id;
    let task_id = &task.task_id;
    let now = Utc::now().to_rfc3339();
    let mut outcome = WorkerOutputIngestion::default();

    for discovery in &output.discoveries {
        sqlx::query("INSERT INTO hypotheses(hypothesis_id,project_id,kind,statement,status,route_id,created_in_round,attributes_json) VALUES(?,?,?,?,?,?,?,?)")
            .bind(new_id("hyp"))
            .bind(project_id)
            .bind(&discovery.kind)
            .bind(&discovery.statement)
            .bind("proposed")
            .bind(&task.route_id)
            .bind(task.round)
            .bind(json_text(&discovery.attributes)?)
            .execute(&mut **tx)
            .await?;
    }
    for failure in &output.failures {
        sqlx::query("INSERT INTO failures(failure_id,project_id,route_id,task_id,failure_type,summary,repairable,raw_json,created_at) VALUES(?,?,?,?,?,?,?,?,?)")
            .bind(new_id("failure"))
            .bind(project_id)
            .bind(&task.route_id)
            .bind(task_id)
            .bind(&failure.failure_type)
            .bind(&failure.summary)
            .bind(failure.repairable)
            .bind(json_text(failure)?)
            .bind(&now)
            .execute(&mut **tx)
            .await?;
    }
    for description in &output.uncertainties {
        sqlx::query("INSERT INTO uncertainties(uncertainty_id,project_id,description,type,severity,affects_goal_ids_json,affects_route_ids_json,introduced_by,resolution_methods_json,status,created_at) VALUES(?,?,?,?,?,?,?,?,?,'open',?)")
            .bind(new_id("unc"))
            .bind(project_id)
            .bind(description)
            .bind("worker_uncertainty")
            .bind("medium")
            .bind(json_text(&task.goal_ids)?)
            .bind(json_text(&vec![task.route_id.clone()])?)
            .bind(task_id)
            .bind("[]")
            .bind(&now)
            .execute(&mut **tx)
            .await?;
    }

    for source in &output.sources {
        let (normalized_url, identifier_kind, identifier_value) = crate::normalized_source_identity(
            source.url.as_deref(),
            source.citation_key.as_deref(),
        );
        let fulltext_artifact_valid = match &source.fulltext_artifact_id {
            Some(artifact_id) => sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM artifacts WHERE artifact_id=? AND project_id=? AND kind='source_fulltext' AND ? IS NOT NULL AND lower(sha256)=lower(?)")
                .bind(artifact_id)
                .bind(project_id)
                .bind(&source.fulltext_sha256)
                .bind(&source.fulltext_sha256)
                .fetch_one(&mut **tx)
                .await? == 1,
            None => false,
        };
        let has_stable_identifier = normalized_url.is_some()
            || source
                .citation_key
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
        let assessment = crate::source_ingestion::assess_source_draft(
            source,
            task.worker_role == "literature_researcher",
            has_stable_identifier,
            fulltext_artifact_valid,
        );
        if !assessment.rejection_reasons.is_empty() {
            let reasons = assessment.rejection_reasons;
            let failure_id = new_id("failure");
            sqlx::query("INSERT INTO failures(failure_id,project_id,route_id,task_id,failure_type,summary,repairable,raw_json,created_at) VALUES(?,?,?,?,?,?,?,?,?)")
                .bind(&failure_id)
                .bind(project_id)
                .bind(&task.route_id)
                .bind(task_id)
                .bind("invalid_source_draft")
                .bind(format!("Rejected literature source draft: {}", reasons.join(", ")))
                .bind(true)
                .bind(json_text(&json!({"source":source,"reasons":reasons}))?)
                .bind(&now)
                .execute(&mut **tx)
                .await?;
            sqlx::query("INSERT INTO source_ingestion_records(ingestion_id,project_id,task_id,route_id,source_id,disposition,reasons_json,normalized_url,identifier_kind,identifier_value,raw_json,created_at) VALUES(?,?,?,?,NULL,'rejected',?,?,?,?,?,?)")
                .bind(new_id("sourceingest"))
                .bind(project_id)
                .bind(task_id)
                .bind(&task.route_id)
                .bind(json_text(&reasons)?)
                .bind(&normalized_url)
                .bind(&identifier_kind)
                .bind(&identifier_value)
                .bind(json_text(source)?)
                .bind(&now)
                .execute(&mut **tx)
                .await?;
            outcome.rejected_source_drafts.push((failure_id, reasons));
            continue;
        }

        let stored_status = assessment.admission.stored_status();
        let duplicate_sources = sqlx::query("SELECT DISTINCT source_id,status FROM sources WHERE project_id=? AND ((? IS NOT NULL AND normalized_url=?) OR (? IS NOT NULL AND identifier_kind=? AND identifier_value=?)) ORDER BY source_id")
            .bind(project_id)
            .bind(&normalized_url)
            .bind(&normalized_url)
            .bind(&identifier_value)
            .bind(&identifier_kind)
            .bind(&identifier_value)
            .fetch_all(&mut **tx)
            .await?;
        if duplicate_sources.len() > 1 {
            // A legacy database may contain more than one row for the same canonical
            // identity because those columns are indexed but not unique. Preserve every
            // possible target in the envelope-local alias table so candidate resolution
            // fails closed as ambiguous instead of selecting an arbitrary source.
            for existing in &duplicate_sources {
                let source_id: String = existing.try_get("source_id")?;
                outcome.remember_source_alias(source, &source_id);
                sqlx::query("INSERT INTO source_ingestion_records(ingestion_id,project_id,task_id,route_id,source_id,disposition,reasons_json,normalized_url,identifier_kind,identifier_value,raw_json,created_at) VALUES(?,?,?,?,?,'duplicate','[\"ambiguous_normalized_identifier\"]',?,?,?,?,?)")
                    .bind(new_id("sourceingest"))
                    .bind(project_id)
                    .bind(task_id)
                    .bind(&task.route_id)
                    .bind(&source_id)
                    .bind(&normalized_url)
                    .bind(&identifier_kind)
                    .bind(&identifier_value)
                    .bind(json_text(source)?)
                    .bind(&now)
                    .execute(&mut **tx)
                    .await?;
            }
            continue;
        }
        if let Some(existing) = duplicate_sources.first() {
            let source_id: String = existing.try_get("source_id")?;
            let existing_status: String = existing.try_get("status")?;
            if existing_status == crate::source_ingestion::LEAD_UNVERIFIED
                && assessment.admission == SourceAdmission::Reported
            {
                sqlx::query("UPDATE sources SET title=?,authors_json=?,url=?,normalized_url=?,identifier_kind=?,identifier_value=?,citation_key=COALESCE(citation_key,?),theorem_reference=?,statement_excerpt=?,assumptions_json=?,applicability=?,status=?,origin_task_id=?,origin_route_id=?,fulltext_artifact_id=?,provenance_json=?,retrieved_at=? WHERE source_id=? AND project_id=? AND status='lead_unverified'")
                    .bind(source.title.trim())
                    .bind(json_text(&source.authors)?)
                    .bind(&source.url)
                    .bind(&normalized_url)
                    .bind(&identifier_kind)
                    .bind(&identifier_value)
                    .bind(&source.citation_key)
                    .bind(&source.theorem_reference)
                    .bind(&source.statement_excerpt)
                    .bind(json_text(&source.assumptions)?)
                    .bind(&source.applicability)
                    .bind(stored_status)
                    .bind(task_id)
                    .bind(&task.route_id)
                    .bind(&source.fulltext_artifact_id)
                    .bind(json_text(&json!({"ingestion":origin.provenance(true),"trust":stored_status,"retrieval_query":source.retrieval_query,"document_version":source.document_version,"fulltext_sha256":source.fulltext_sha256}))?)
                    .bind(&now)
                    .bind(&source_id)
                    .bind(project_id)
                    .execute(&mut **tx)
                    .await?;
                sqlx::query("INSERT INTO source_ingestion_records(ingestion_id,project_id,task_id,route_id,source_id,disposition,reasons_json,normalized_url,identifier_kind,identifier_value,raw_json,created_at) VALUES(?,?,?,?,?,'upgraded','[\"lead_promoted_after_fulltext_verification\"]',?,?,?,?,?)")
                    .bind(new_id("sourceingest"))
                    .bind(project_id)
                    .bind(task_id)
                    .bind(&task.route_id)
                    .bind(&source_id)
                    .bind(&normalized_url)
                    .bind(&identifier_kind)
                    .bind(&identifier_value)
                    .bind(json_text(source)?)
                    .bind(&now)
                    .execute(&mut **tx)
                    .await?;
                outcome.remember_source_alias(source, &source_id);
                outcome.recorded_sources.push((
                    source_id,
                    source.title.trim().to_owned(),
                    assessment.admission,
                ));
                outcome.upgraded_source_count += 1;
                continue;
            }
            sqlx::query("INSERT INTO source_ingestion_records(ingestion_id,project_id,task_id,route_id,source_id,disposition,reasons_json,normalized_url,identifier_kind,identifier_value,raw_json,created_at) VALUES(?,?,?,?,?,'duplicate','[\"same_normalized_identifier\"]',?,?,?,?,?)")
                .bind(new_id("sourceingest"))
                .bind(project_id)
                .bind(task_id)
                .bind(&task.route_id)
                .bind(&source_id)
                .bind(&normalized_url)
                .bind(&identifier_kind)
                .bind(&identifier_value)
                .bind(json_text(source)?)
                .bind(&now)
                .execute(&mut **tx)
                .await?;
            outcome.remember_source_alias(source, &source_id);
            continue;
        }

        let source_id = new_id("source");
        sqlx::query("INSERT INTO sources(source_id,project_id,title,authors_json,url,normalized_url,identifier_kind,identifier_value,citation_key,theorem_reference,statement_excerpt,assumptions_json,applicability,status,origin_task_id,origin_route_id,fulltext_artifact_id,provenance_json,retrieved_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&source_id)
            .bind(project_id)
            .bind(source.title.trim())
            .bind(json_text(&source.authors)?)
            .bind(&source.url)
            .bind(&normalized_url)
            .bind(&identifier_kind)
            .bind(&identifier_value)
            .bind(&source.citation_key)
            .bind(&source.theorem_reference)
            .bind(&source.statement_excerpt)
            .bind(json_text(&source.assumptions)?)
            .bind(&source.applicability)
            .bind(stored_status)
            .bind(task_id)
            .bind(&task.route_id)
            .bind(&source.fulltext_artifact_id)
            .bind(json_text(&json!({"ingestion":origin.provenance(false),"trust":stored_status,"retrieval_query":source.retrieval_query,"document_version":source.document_version,"fulltext_sha256":source.fulltext_sha256}))?)
            .bind(&now)
            .execute(&mut **tx)
            .await?;
        sqlx::query("INSERT INTO source_ingestion_records(ingestion_id,project_id,task_id,route_id,source_id,disposition,reasons_json,normalized_url,identifier_kind,identifier_value,raw_json,created_at) VALUES(?,?,?,?,?,'inserted','[]',?,?,?,?,?)")
            .bind(new_id("sourceingest"))
            .bind(project_id)
            .bind(task_id)
            .bind(&task.route_id)
            .bind(&source_id)
            .bind(&normalized_url)
            .bind(&identifier_kind)
            .bind(&identifier_value)
            .bind(json_text(source)?)
            .bind(&now)
            .execute(&mut **tx)
            .await?;
        outcome.remember_source_alias(source, &source_id);
        outcome.recorded_sources.push((
            source_id,
            source.title.trim().to_owned(),
            assessment.admission,
        ));
        outcome.inserted_source_count += 1;
    }

    for experiment in &output.experiments {
        let canonical = serde_json::to_value(experiment)?;
        let content_hash = hex::encode(Sha256::digest(serde_json::to_vec(&canonical)?));
        let capsule_id = new_id("capsule");
        sqlx::query("INSERT INTO experiment_capsules(capsule_id,project_id,task_id,route_id,language,program_text,input_json,environment_json,stdout,stderr,exit_code,artifacts_json,conclusion_mapping_json,replay_command_json,status,content_hash,created_at,replayed_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,'reported_unverified',?,?,NULL)")
            .bind(&capsule_id)
            .bind(project_id)
            .bind(task_id)
            .bind(&task.route_id)
            .bind(&experiment.language)
            .bind(&experiment.program_text)
            .bind(json_text(&experiment.input)?)
            .bind(json_text(&experiment.environment)?)
            .bind(&experiment.stdout)
            .bind(&experiment.stderr)
            .bind(experiment.exit_code)
            .bind(json_text(&experiment.artifacts)?)
            .bind(json_text(&experiment.conclusion_mapping)?)
            .bind(json_text(&experiment.replay_command)?)
            .bind(&content_hash)
            .bind(&now)
            .execute(&mut **tx)
            .await?;
        outcome.capsule_events.push((capsule_id, content_hash));
    }

    Ok(outcome)
}
