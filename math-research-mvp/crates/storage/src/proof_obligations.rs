use std::{collections::BTreeMap, str::FromStr};

use chrono::{DateTime, Utc};
use research_domain::{
    CandidateObligationCoverage, ObligationCoverageDisposition, ObligationEdge, ObligationEdgeKind,
    ProofObligation, ProofObligationGraph, ProofObligationNecessity, ProofObligationStatus,
    VerificationGap, VerificationReport, VerificationVerdict,
};
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction, sqlite::SqliteRow};

use crate::{SqliteStore, StorageError, StorageResult, json_text, new_id};

#[derive(Debug, Clone)]
struct AdvisorySeed {
    source_kind: &'static str,
    statement: String,
    completion_criteria: String,
    provenance_item: Value,
    normalized_identity: String,
    occurrences: usize,
}

pub(crate) struct AdvisoryObligationContext<'a> {
    pub project_id: &'a str,
    pub candidate_id: &'a str,
    pub verification_id: &'a str,
    pub target_goal_ids: &'a [String],
    pub bottleneck_id: Option<&'a str>,
    pub origin_kind: &'a str,
    pub report: &'a VerificationReport,
    pub project_revision: i64,
    pub now: &'a DateTime<Utc>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_root_obligation_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    goal_id: &str,
    bottleneck_id: &str,
    statement: &str,
    priority: f64,
    revision: i64,
    provenance: &Value,
    now: &DateTime<Utc>,
) -> StorageResult<String> {
    let obligation_id = new_id("obligation");
    sqlx::query(
        "INSERT INTO proof_obligations(obligation_id,project_id,goal_id,parent_obligation_id,source_kind,statement,completion_criteria,necessity,status,priority,source_bottleneck_id,source_fingerprint,provenance_json,created_revision,updated_revision,created_at,updated_at) VALUES(?,?,?,NULL,'root_goal',?,'A FullyCertified Fact must pass the Goal coverage gate; a proof satisfies this obligation and a counterexample makes it obsolete by refuting the Goal.','required','open',?,?,?,?,?,?,?,?)",
    )
    .bind(&obligation_id)
    .bind(project_id)
    .bind(goal_id)
    .bind(statement)
    .bind(priority)
    .bind(bottleneck_id)
    .bind(format!("root:{goal_id}"))
    .bind(json_text(provenance)?)
    .bind(revision)
    .bind(revision)
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(&mut **tx)
    .await?;
    Ok(obligation_id)
}

pub(crate) async fn root_obligation_id_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    goal_id: &str,
) -> StorageResult<String> {
    sqlx::query_scalar(
        "SELECT obligation_id FROM proof_obligations WHERE project_id=? AND goal_id=? AND source_kind='root_goal' ORDER BY CASE WHEN status='obsolete' THEN 1 ELSE 0 END,created_revision DESC,created_at DESC LIMIT 1",
    )
    .bind(project_id)
    .bind(goal_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        StorageError::InvalidTransition(format!(
            "goal {goal_id} has no required root proof obligation"
        ))
    })
}

pub(crate) async fn obsolete_project_obligations_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    revision: i64,
    now: &DateTime<Utc>,
) -> StorageResult<Vec<String>> {
    let obligation_ids = sqlx::query_scalar::<_, String>(
        "SELECT obligation_id FROM proof_obligations WHERE project_id=? AND created_revision<? AND status<>'obsolete' ORDER BY created_at,obligation_id",
    )
    .bind(project_id)
    .bind(revision)
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE proof_obligations SET status='obsolete',updated_revision=?,updated_at=? WHERE project_id=? AND created_revision<? AND status<>'obsolete'",
    )
    .bind(revision)
    .bind(now.to_rfc3339())
    .bind(project_id)
    .bind(revision)
    .execute(&mut **tx)
    .await?;
    Ok(obligation_ids)
}

pub(crate) async fn reopen_required_obligations_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    goal_ids: &[String],
    unavailable_fact_ids: &[String],
    revision: i64,
    now: &DateTime<Utc>,
) -> StorageResult<Vec<String>> {
    let mut reopened = Vec::new();
    for goal_id in goal_ids {
        let root = sqlx::query(
            "SELECT obligation_id,status,satisfied_by_fact_id FROM proof_obligations WHERE project_id=? AND goal_id=? AND source_kind='root_goal' ORDER BY CASE WHEN status='obsolete' THEN 1 ELSE 0 END,created_revision DESC,created_at DESC LIMIT 1",
        )
        .bind(project_id)
        .bind(goal_id)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(root) = root {
            let root_id: String = root.try_get("obligation_id")?;
            let root_status: String = root.try_get("status")?;
            let root_fact_id: Option<String> = root.try_get("satisfied_by_fact_id")?;
            if root_status != "open" || root_fact_id.is_some() {
                reopened.push(root_id.clone());
            }
            sqlx::query("UPDATE proof_obligations SET status='open',satisfied_by_fact_id=NULL,updated_revision=?,updated_at=? WHERE obligation_id=?")
                .bind(revision)
                .bind(now.to_rfc3339())
                .bind(&root_id)
                .execute(&mut **tx)
                .await?;
        }
        for fact_id in unavailable_fact_ids {
            let child_ids = sqlx::query_scalar::<_, String>(
                "SELECT obligation_id FROM proof_obligations WHERE project_id=? AND goal_id=? AND source_kind<>'root_goal' AND necessity='required' AND status='satisfied' AND satisfied_by_fact_id=?",
            )
            .bind(project_id)
            .bind(goal_id)
            .bind(fact_id)
            .fetch_all(&mut **tx)
            .await?;
            sqlx::query("UPDATE proof_obligations SET status='open',satisfied_by_fact_id=NULL,updated_revision=?,updated_at=? WHERE project_id=? AND goal_id=? AND source_kind<>'root_goal' AND necessity='required' AND status='satisfied' AND satisfied_by_fact_id=?")
                .bind(revision)
                .bind(now.to_rfc3339())
                .bind(project_id)
                .bind(goal_id)
                .bind(fact_id)
                .execute(&mut **tx)
                .await?;
            reopened.extend(child_ids);
        }
    }
    reopened.sort();
    reopened.dedup();
    Ok(reopened)
}

pub(crate) async fn has_unresolved_required_children_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    goal_id: &str,
    root_obligation_id: &str,
) -> StorageResult<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM proof_obligations WHERE project_id=? AND goal_id=? AND obligation_id<>? AND necessity='required' AND status NOT IN ('satisfied','obsolete')",
    )
    .bind(project_id)
    .bind(goal_id)
    .bind(root_obligation_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(count != 0)
}

pub(crate) struct RootCoverage<'a> {
    pub project_id: &'a str,
    pub candidate_id: &'a str,
    pub verification_id: &'a str,
    pub obligation_id: &'a str,
    pub disposition: ObligationCoverageDisposition,
    pub fact_id: Option<&'a str>,
    pub rationale: &'a str,
    pub now: &'a DateTime<Utc>,
}

pub(crate) async fn record_root_coverage_tx(
    tx: &mut Transaction<'_, Sqlite>,
    coverage: RootCoverage<'_>,
) -> StorageResult<()> {
    sqlx::query("INSERT INTO candidate_obligation_coverage(coverage_id,project_id,candidate_id,obligation_id,verification_id,disposition,fact_id,rationale,created_at) VALUES(?,?,?,?,?,?,?,?,?)")
        .bind(new_id("coverage"))
        .bind(coverage.project_id)
        .bind(coverage.candidate_id)
        .bind(coverage.obligation_id)
        .bind(coverage.verification_id)
        .bind(coverage.disposition.to_string())
        .bind(coverage.fact_id)
        .bind(coverage.rationale)
        .bind(coverage.now.to_rfc3339())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn mark_root_satisfied_tx(
    tx: &mut Transaction<'_, Sqlite>,
    obligation_id: &str,
    fact_id: &str,
    revision: i64,
    now: &DateTime<Utc>,
) -> StorageResult<()> {
    let result = sqlx::query(
        "UPDATE proof_obligations SET status='satisfied',satisfied_by_fact_id=?,updated_revision=?,updated_at=? WHERE obligation_id=? AND necessity='required' AND source_kind='root_goal' AND status IN ('open','blocked')",
    )
    .bind(fact_id)
    .bind(revision)
    .bind(now.to_rfc3339())
    .bind(obligation_id)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(StorageError::InvalidTransition(format!(
            "root proof obligation {obligation_id} was not open for satisfaction"
        )));
    }
    Ok(())
}

pub(crate) async fn mark_root_obsolete_tx(
    tx: &mut Transaction<'_, Sqlite>,
    obligation_id: &str,
    revision: i64,
    now: &DateTime<Utc>,
) -> StorageResult<()> {
    sqlx::query(
        "UPDATE proof_obligations SET status='obsolete',updated_revision=?,updated_at=? WHERE obligation_id=? AND source_kind='root_goal' AND status IN ('open','blocked')",
    )
    .bind(revision)
    .bind(now.to_rfc3339())
    .bind(obligation_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn mark_root_conflicted_tx(
    tx: &mut Transaction<'_, Sqlite>,
    obligation_id: &str,
    revision: i64,
    now: &DateTime<Utc>,
) -> StorageResult<()> {
    let result = sqlx::query(
        "UPDATE proof_obligations SET status='blocked',satisfied_by_fact_id=NULL,updated_revision=?,updated_at=? WHERE obligation_id=? AND necessity='required' AND source_kind='root_goal' AND status IN ('satisfied','obsolete')",
    )
    .bind(revision)
    .bind(now.to_rfc3339())
    .bind(obligation_id)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(StorageError::InvalidTransition(format!(
            "root proof obligation {obligation_id} was not closed before the conflicting closure"
        )));
    }
    Ok(())
}

pub(crate) async fn obsolete_resolved_bottleneck_advisories_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    revision: i64,
    now: &DateTime<Utc>,
) -> StorageResult<Vec<String>> {
    let obligation_ids = sqlx::query_scalar::<_, String>(
        "SELECT o.obligation_id FROM proof_obligations o JOIN bottlenecks b ON b.bottleneck_id=o.source_bottleneck_id AND b.project_id=o.project_id WHERE o.project_id=? AND o.necessity='advisory' AND o.status IN ('open','blocked') AND b.status='resolved' ORDER BY o.obligation_id",
    )
    .bind(project_id)
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE proof_obligations SET status='obsolete',updated_revision=?,updated_at=? WHERE project_id=? AND necessity='advisory' AND status IN ('open','blocked') AND source_bottleneck_id IN (SELECT bottleneck_id FROM bottlenecks WHERE project_id=? AND status='resolved')",
    )
    .bind(revision)
    .bind(now.to_rfc3339())
    .bind(project_id)
    .bind(project_id)
    .execute(&mut **tx)
    .await?;
    Ok(obligation_ids)
}

pub(crate) async fn insert_verifier_advisory_obligations_tx(
    tx: &mut Transaction<'_, Sqlite>,
    context: AdvisoryObligationContext<'_>,
) -> StorageResult<Vec<String>> {
    let seeds = advisory_seeds(context.report, context.origin_kind);
    if seeds.is_empty() {
        return Ok(Vec::new());
    }
    let goal_ids = if context.target_goal_ids.is_empty() {
        vec![None]
    } else {
        context.target_goal_ids.iter().map(Some).collect()
    };
    let mut inserted = Vec::new();
    for goal_id in goal_ids {
        let parent_id = match goal_id {
            Some(goal_id) => {
                Some(root_obligation_id_tx(tx, context.project_id, goal_id.as_str()).await?)
            }
            None => None,
        };
        for seed in &seeds {
            let goal_identity = goal_id.map_or("project", String::as_str);
            let source_fingerprint = crate::reliability_v2::sha256_json(&json!({
                "goal":goal_identity,
                "kind":seed.source_kind,
                "item":seed.normalized_identity,
            }))?;
            let obligation_id = new_id("obligation");
            let priority = match context.report.verdict {
                VerificationVerdict::Rejected => 1.1,
                VerificationVerdict::Unknown => 1.05,
                VerificationVerdict::Accepted => 0.8,
            };
            let provenance = json!({
                "source":context.origin_kind,
                "verification_id":context.verification_id,
                "candidate_id":context.candidate_id,
                "verdict":context.report.verdict,
                "report_summary":context.report.summary,
                "item":seed.provenance_item,
                "duplicates_collapsed":seed.occurrences.saturating_sub(1),
            });
            let result = sqlx::query("INSERT OR IGNORE INTO proof_obligations(obligation_id,project_id,goal_id,parent_obligation_id,source_kind,statement,completion_criteria,necessity,status,priority,source_verification_id,source_bottleneck_id,source_fingerprint,provenance_json,created_revision,updated_revision,created_at,updated_at) VALUES(?,?,?,?,?,?,?,'advisory','open',?,?,?,?,?,?,?,?,?)")
                .bind(&obligation_id)
                .bind(context.project_id)
                .bind(goal_id.map(String::as_str))
                .bind(&parent_id)
                .bind(seed.source_kind)
                .bind(&seed.statement)
                .bind(&seed.completion_criteria)
                .bind(priority)
                .bind(context.verification_id)
                .bind(context.bottleneck_id)
                .bind(&source_fingerprint)
                .bind(json_text(&provenance)?)
                .bind(context.project_revision)
                .bind(context.project_revision)
                .bind(context.now.to_rfc3339())
                .bind(context.now.to_rfc3339())
                .execute(&mut **tx)
                .await?;
            let persisted_id = if result.rows_affected() == 1 {
                obligation_id
            } else {
                sqlx::query_scalar("SELECT obligation_id FROM proof_obligations WHERE project_id=? AND source_verification_id=? AND source_kind=? AND source_fingerprint=?")
                    .bind(context.project_id)
                    .bind(context.verification_id)
                    .bind(seed.source_kind)
                    .bind(&source_fingerprint)
                    .fetch_one(&mut **tx)
                    .await?
            };
            if let Some(parent_id) = &parent_id {
                sqlx::query("INSERT OR IGNORE INTO obligation_edges(edge_id,project_id,source_obligation_id,target_obligation_id,kind,created_at) VALUES(?,?,?,?, 'refines',?)")
                    .bind(new_id("obligationedge"))
                    .bind(context.project_id)
                    .bind(parent_id)
                    .bind(&persisted_id)
                    .bind(context.now.to_rfc3339())
                    .execute(&mut **tx)
                    .await?;
            }
            inserted.push(persisted_id);
        }
    }
    Ok(inserted)
}

/// Recover the verifier's structured Goal-coverage report without allowing malformed optional
/// evidence to erase the debt signal or reject an otherwise valid intermediate Fact.
pub(crate) fn nonpassing_goal_coverage_report(
    check_status: &str,
    check_summary: &str,
    details_json: &str,
) -> StorageResult<Option<VerificationReport>> {
    if check_status == "passed" {
        return Ok(None);
    }
    let details: Value = serde_json::from_str(details_json)?;
    let verdict = if check_status == "failed" {
        VerificationVerdict::Rejected
    } else {
        VerificationVerdict::Unknown
    };
    if let Some(report) = details.get("coverage_report") {
        if let Ok(mut report) = serde_json::from_value::<VerificationReport>(report.clone()) {
            // The persisted check status is the authoritative outcome. Do not let an
            // inconsistent nested payload silently turn a failed check into passing coverage.
            report.verdict = verdict;
            if report.summary.trim().is_empty() {
                check_summary.clone_into(&mut report.summary);
            }
            if report.gaps.is_empty() && report.repair_actions.is_empty() {
                report.gaps.push(VerificationGap {
                    location: "goal_coverage_review".into(),
                    gap_type: "coverage_evidence".into(),
                    issue: format!("The Goal coverage check did not pass: {}", report.summary),
                });
            }
            return Ok(Some(report));
        }
    }
    Ok(Some(VerificationReport {
        verdict,
        summary: check_summary.to_owned(),
        critical_errors: Vec::new(),
        gaps: vec![VerificationGap {
            location: "goal_coverage_review".into(),
            gap_type: "coverage_evidence".into(),
            issue: "The non-passing Goal coverage check did not contain a decodable structured coverage_report.".into(),
        }],
        uncertainties: Vec::new(),
        repair_actions: Vec::new(),
        checked_fact_ids: Vec::new(),
        checked_source_ids: Vec::new(),
        evidence_level: "goal_coverage_unresolved".into(),
    }))
}

fn advisory_seeds(report: &VerificationReport, origin_kind: &str) -> Vec<AdvisorySeed> {
    let mut seeds = BTreeMap::<String, AdvisorySeed>::new();
    for gap in &report.gaps {
        let identity = normalized(&format!("{} {} {}", gap.location, gap.gap_type, gap.issue));
        let source_kind = if origin_kind == "goal_coverage_review" {
            "goal_coverage_gap"
        } else {
            "verification_gap"
        };
        let key = format!("{source_kind}:{identity}");
        let seed = seeds
            .entry(key)
            .or_insert_with(|| gap_seed(gap, identity, source_kind));
        seed.occurrences += 1;
    }
    for action in &report.repair_actions {
        let identity = normalized(action);
        if identity.is_empty() {
            continue;
        }
        let source_kind = if origin_kind == "goal_coverage_review" {
            "goal_coverage_repair_action"
        } else {
            "verification_repair_action"
        };
        let key = format!("{source_kind}:{identity}");
        let seed = seeds.entry(key).or_insert_with(|| AdvisorySeed {
            source_kind,
            statement: action.trim().to_owned(),
            completion_criteria: format!(
                "Submit independently checkable new evidence completing this action: {}",
                action.trim()
            ),
            provenance_item: json!({"repair_action":action}),
            normalized_identity: identity,
            occurrences: 0,
        });
        seed.occurrences += 1;
    }
    seeds.into_values().collect()
}

fn gap_seed(
    gap: &VerificationGap,
    normalized_identity: String,
    source_kind: &'static str,
) -> AdvisorySeed {
    AdvisorySeed {
        source_kind,
        statement: format!(
            "Close the {} gap at {}: {}",
            gap.gap_type.trim(),
            gap.location.trim(),
            gap.issue.trim()
        ),
        completion_criteria: format!(
            "Independently verify that the reported {} gap at {} is closed.",
            gap.gap_type.trim(),
            gap.location.trim()
        ),
        provenance_item: json!({"gap":gap}),
        normalized_identity,
        occurrences: 0,
    }
}

fn normalized(value: &str) -> String {
    value
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn proof_obligation_from_row(row: &SqliteRow) -> StorageResult<ProofObligation> {
    Ok(ProofObligation {
        obligation_id: row.try_get("obligation_id")?,
        project_id: row.try_get("project_id")?,
        goal_id: row.try_get("goal_id")?,
        parent_obligation_id: row.try_get("parent_obligation_id")?,
        source_kind: row.try_get("source_kind")?,
        statement: row.try_get("statement")?,
        completion_criteria: row.try_get("completion_criteria")?,
        necessity: ProofObligationNecessity::from_str(row.try_get("necessity")?)
            .map_err(StorageError::CorruptData)?,
        status: ProofObligationStatus::from_str(row.try_get("status")?)
            .map_err(StorageError::CorruptData)?,
        priority: row.try_get("priority")?,
        source_verification_id: row.try_get("source_verification_id")?,
        source_bottleneck_id: row.try_get("source_bottleneck_id")?,
        source_fingerprint: row.try_get("source_fingerprint")?,
        provenance: serde_json::from_str(row.try_get("provenance_json")?)?,
        satisfied_by_fact_id: row.try_get("satisfied_by_fact_id")?,
        created_revision: row.try_get("created_revision")?,
        updated_revision: row.try_get("updated_revision")?,
        created_at: crate::rows::timestamp(row.try_get("created_at")?)?,
        updated_at: crate::rows::timestamp(row.try_get("updated_at")?)?,
    })
}

pub(crate) fn obligation_edge_from_row(row: &SqliteRow) -> StorageResult<ObligationEdge> {
    Ok(ObligationEdge {
        edge_id: row.try_get("edge_id")?,
        project_id: row.try_get("project_id")?,
        source_obligation_id: row.try_get("source_obligation_id")?,
        target_obligation_id: row.try_get("target_obligation_id")?,
        kind: ObligationEdgeKind::from_str(row.try_get("kind")?)
            .map_err(StorageError::CorruptData)?,
        created_at: crate::rows::timestamp(row.try_get("created_at")?)?,
    })
}

pub(crate) fn candidate_coverage_from_row(
    row: &SqliteRow,
) -> StorageResult<CandidateObligationCoverage> {
    Ok(CandidateObligationCoverage {
        coverage_id: row.try_get("coverage_id")?,
        project_id: row.try_get("project_id")?,
        candidate_id: row.try_get("candidate_id")?,
        obligation_id: row.try_get("obligation_id")?,
        verification_id: row.try_get("verification_id")?,
        disposition: ObligationCoverageDisposition::from_str(row.try_get("disposition")?)
            .map_err(StorageError::CorruptData)?,
        fact_id: row.try_get("fact_id")?,
        rationale: row.try_get("rationale")?,
        created_at: crate::rows::timestamp(row.try_get("created_at")?)?,
    })
}

impl SqliteStore {
    pub async fn list_proof_obligations(
        &self,
        project_id: &str,
    ) -> StorageResult<Vec<ProofObligation>> {
        self.get_project(project_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM proof_obligations WHERE project_id=? ORDER BY CASE status WHEN 'open' THEN 0 WHEN 'blocked' THEN 1 ELSE 2 END,priority DESC,created_at,obligation_id",
        )
        .bind(project_id)
        .fetch_all(self.read_pool())
        .await?;
        rows.iter().map(proof_obligation_from_row).collect()
    }

    pub async fn list_obligation_edges(
        &self,
        project_id: &str,
    ) -> StorageResult<Vec<ObligationEdge>> {
        self.get_project(project_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM obligation_edges WHERE project_id=? ORDER BY created_at,edge_id",
        )
        .bind(project_id)
        .fetch_all(self.read_pool())
        .await?;
        rows.iter().map(obligation_edge_from_row).collect()
    }

    pub async fn list_candidate_obligation_coverage(
        &self,
        project_id: &str,
    ) -> StorageResult<Vec<CandidateObligationCoverage>> {
        self.get_project(project_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM candidate_obligation_coverage WHERE project_id=? ORDER BY created_at,coverage_id",
        )
        .bind(project_id)
        .fetch_all(self.read_pool())
        .await?;
        rows.iter().map(candidate_coverage_from_row).collect()
    }

    pub async fn proof_obligation_graph(
        &self,
        project_id: &str,
    ) -> StorageResult<ProofObligationGraph> {
        self.proof_obligation_graph_snapshot(project_id)
            .await
            .map(|(graph, _, _)| graph)
    }

    /// Read the graph and its API consistency tokens from one SQLite snapshot.
    pub async fn proof_obligation_graph_snapshot(
        &self,
        project_id: &str,
    ) -> StorageResult<(ProofObligationGraph, i64, i64)> {
        let mut tx = self.read_pool().begin().await?;
        let revision: i64 = sqlx::query_scalar("SELECT revision FROM projects WHERE project_id=?")
            .bind(project_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "project",
                id: project_id.into(),
            })?;
        let obligation_rows = sqlx::query(
            "SELECT * FROM proof_obligations WHERE project_id=? ORDER BY CASE status WHEN 'open' THEN 0 WHEN 'blocked' THEN 1 ELSE 2 END,priority DESC,created_at,obligation_id",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        let edge_rows = sqlx::query(
            "SELECT * FROM obligation_edges WHERE project_id=? ORDER BY created_at,edge_id",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        let coverage_rows = sqlx::query(
            "SELECT * FROM candidate_obligation_coverage WHERE project_id=? ORDER BY created_at,coverage_id",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        let cursor: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(cursor),0) FROM events WHERE project_id=?")
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?;
        let graph = ProofObligationGraph {
            obligations: obligation_rows
                .iter()
                .map(proof_obligation_from_row)
                .collect::<StorageResult<_>>()?,
            edges: edge_rows
                .iter()
                .map(obligation_edge_from_row)
                .collect::<StorageResult<_>>()?,
            coverage: coverage_rows
                .iter()
                .map(candidate_coverage_from_row)
                .collect::<StorageResult<_>>()?,
        };
        tx.commit().await?;
        Ok((graph, revision, cursor))
    }
}

#[cfg(test)]
mod tests {
    use super::advisory_seeds;
    use research_domain::{
        AcceptanceClass, Budget, CandidateSubmission, CandidateType, CheckStatus,
        ObligationCoverageDisposition, ProblemContract, ProblemRevisionRequest,
        ProofObligationNecessity, ProofObligationStatus, VerificationGap, VerificationProfile,
        VerificationReport, VerificationStage, VerificationVerdict,
    };
    use serde_json::json;
    use sqlx::Row;
    use tempfile::TempDir;

    use crate::{
        CheckDraft, ModelCallPurpose, ModelCallRequest, SqliteStore, VerificationCaseDraft,
        VerificationSnapshotDraft,
    };

    #[allow(clippy::struct_field_names)]
    struct VerificationFixture {
        project_id: String,
        goal_id: String,
        verification_id: String,
    }

    async fn store() -> (SqliteStore, TempDir) {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::connect("sqlite::memory:", temp.path())
            .await
            .expect("store");
        (store, temp)
    }

    fn contract() -> ProblemContract {
        ProblemContract {
            version: 1,
            original_problem: "Prove P".into(),
            target_statement: "P".into(),
            assumptions: Vec::new(),
            success_criteria: "A verified proof of P".into(),
        }
    }

    fn report(verdict: VerificationVerdict) -> VerificationReport {
        VerificationReport {
            verdict,
            summary: format!("{verdict} report"),
            critical_errors: Vec::new(),
            gaps: Vec::new(),
            uncertainties: Vec::new(),
            repair_actions: Vec::new(),
            checked_fact_ids: Vec::new(),
            checked_source_ids: Vec::new(),
            evidence_level: "test".into(),
        }
    }

    async fn verification_fixture(
        store: &SqliteStore,
        acceptance: AcceptanceClass,
        candidate_type: CandidateType,
        coverage: Option<(CheckStatus, VerificationReport)>,
    ) -> VerificationFixture {
        let (project, _) = store
            .create_project("obligation-test".into(), contract(), Budget::default())
            .await
            .expect("project");
        sqlx::query("UPDATE projects SET status='running' WHERE project_id=?")
            .bind(&project.project_id)
            .execute(store.pool())
            .await
            .expect("running project");
        let goal_id = store.list_goals(&project.project_id).await.expect("goals")[0]
            .goal_id
            .clone();
        sqlx::query("INSERT INTO routes(route_id,project_id,title,method_summary,target_goal_ids_json,required_fact_ids_json,status,score,priority,cancellation_epoch,created_in_round,attributes_json) VALUES('obligation-route',?,'route','prove P',?,'[]','active',1.0,1.0,0,1,'{}')")
            .bind(&project.project_id)
            .bind(json!([goal_id.clone()]).to_string())
            .execute(store.pool())
            .await
            .expect("route");
        sqlx::query("INSERT INTO tasks(task_id,project_id,route_id,worker_role,goal_ids_json,objective,completion_contract,status,priority,revision,route_cancellation_epoch,round) VALUES('obligation-task',?,'obligation-route','prover',?,'prove P','return a candidate','completed',1.0,1,0,1)")
            .bind(&project.project_id)
            .bind(json!([goal_id.clone()]).to_string())
            .execute(store.pool())
            .await
            .expect("task");
        let submission = CandidateSubmission {
            task_id: "obligation-task".into(),
            route_id: "obligation-route".into(),
            target_goal_ids: vec![goal_id.clone()],
            statement: if candidate_type == CandidateType::Counterexample {
                "not P".into()
            } else {
                "P".into()
            },
            assumptions: Vec::new(),
            proof_markdown: "checked proof".into(),
            dependency_fact_ids: Vec::new(),
            definitions_introduced: Default::default(),
            external_source_ids: Vec::new(),
            candidate_type,
            task_revision: 1,
            route_cancellation_epoch: 0,
        };
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO candidates(candidate_id,project_id,submission_json,status,created_at) VALUES('obligation-candidate',?,?,'verifying',?)")
            .bind(&project.project_id)
            .bind(serde_json::to_string(&submission).expect("submission json"))
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("candidate");
        sqlx::query("INSERT INTO verifications(verification_id,candidate_id,project_id,status,started_at) VALUES('obligation-verification','obligation-candidate',?,'verifying',?)")
            .bind(&project.project_id)
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("verification");
        let mut required_checks = vec!["deterministic_precheck".into(), "math_review_1".into()];
        let formal = acceptance == AcceptanceClass::FullyCertified;
        if formal {
            required_checks.extend(
                [
                    "semantic_contract",
                    "alignment_review",
                    "lean_kernel",
                    "package_integrity",
                    "fresh_replay",
                ]
                .into_iter()
                .map(str::to_owned),
            );
        }
        let (case, _) = store
            .ensure_verification_case(
                "obligation-verification",
                VerificationCaseDraft {
                    name: "obligation-gate".into(),
                    profile: if formal {
                        VerificationProfile::CriticalCertification
                    } else {
                        VerificationProfile::StandardReview
                    },
                    required_acceptance: acceptance,
                    required_checks: required_checks.clone(),
                    independent_reviewer_count: 1,
                    require_citation_review: false,
                    require_adversarial_review: false,
                    require_alignment_review: formal,
                    require_fresh_replay: formal,
                    max_attempts: 1,
                    risk_score: 0.5,
                    risk_reasons: Vec::new(),
                },
            )
            .await
            .expect("verification case");
        store
            .create_verification_snapshot(
                &case.case_id,
                VerificationSnapshotDraft {
                    toolchain_hash: None,
                    extra_payload: json!({}),
                },
            )
            .await
            .expect("snapshot");
        for kind in required_checks {
            store
                .record_verification_check(
                    &case.case_id,
                    CheckDraft {
                        attempt_id: None,
                        kind,
                        status: CheckStatus::Passed,
                        mandatory: true,
                        summary: "passed".into(),
                        details: json!({}),
                    },
                )
                .await
                .expect("required check");
        }
        if let Some((status, coverage_report)) = coverage {
            store
                .record_verification_check(
                    &case.case_id,
                    CheckDraft {
                        attempt_id: None,
                        kind: "goal_coverage_review".into(),
                        status,
                        mandatory: false,
                        summary: coverage_report.summary.clone(),
                        details: json!({"coverage_report":coverage_report}),
                    },
                )
                .await
                .expect("coverage check");
        }
        store
            .transition_verification_case(&case.case_id, 0, VerificationStage::Review, None)
            .await
            .expect("review");
        store
            .transition_verification_case(&case.case_id, 0, VerificationStage::Adjudication, None)
            .await
            .expect("adjudication");
        store
            .transition_verification_case(
                &case.case_id,
                0,
                VerificationStage::CommitReady,
                Some(acceptance),
            )
            .await
            .expect("commit ready");
        VerificationFixture {
            project_id: project.project_id,
            goal_id,
            verification_id: "obligation-verification".into(),
        }
    }

    async fn seed_existing_root_closure(store: &SqliteStore, fixture: &VerificationFixture) {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO facts(fact_id,project_id,statement,assumptions_json,proof_markdown,dependency_fact_ids_json,definitions_introduced_json,external_source_ids_json,verification_ids_json,evidence_level,created_by,status,content_hash,created_at) VALUES('existing-certified-fact',?,'P','[]','existing certified proof','[]','{}','[]','[]','fully_certified','fixture','active','existing-certified-content',?)")
            .bind(&fixture.project_id)
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("existing certified Fact");
        sqlx::query("UPDATE goals SET status='solved',solved_by_fact_id='existing-certified-fact' WHERE project_id=? AND goal_id=?")
            .bind(&fixture.project_id)
            .bind(&fixture.goal_id)
            .execute(store.pool())
            .await
            .expect("existing Goal closure");
        sqlx::query("UPDATE proof_obligations SET status='satisfied',satisfied_by_fact_id='existing-certified-fact' WHERE project_id=? AND goal_id=? AND source_kind='root_goal'")
            .bind(&fixture.project_id)
            .bind(&fixture.goal_id)
            .execute(store.pool())
            .await
            .expect("existing root closure");
    }

    async fn prepare_fully_certified_case(store: &SqliteStore, verification_id: &str) {
        let required_checks = vec![
            "deterministic_precheck",
            "math_review_1",
            "semantic_contract",
            "alignment_review",
            "lean_kernel",
            "package_integrity",
            "fresh_replay",
        ];
        let (case, _) = store
            .ensure_verification_case(
                verification_id,
                VerificationCaseDraft {
                    name: "governance-recertification".into(),
                    profile: VerificationProfile::CriticalCertification,
                    required_acceptance: AcceptanceClass::FullyCertified,
                    required_checks: required_checks.iter().map(ToString::to_string).collect(),
                    independent_reviewer_count: 1,
                    require_citation_review: false,
                    require_adversarial_review: false,
                    require_alignment_review: true,
                    require_fresh_replay: true,
                    max_attempts: 1,
                    risk_score: 1.0,
                    risk_reasons: vec!["fact_governance".into()],
                },
            )
            .await
            .expect("governance verification case");
        store
            .create_verification_snapshot(
                &case.case_id,
                VerificationSnapshotDraft {
                    toolchain_hash: None,
                    extra_payload: json!({"governance":true}),
                },
            )
            .await
            .expect("governance snapshot");
        for kind in required_checks {
            store
                .record_verification_check(
                    &case.case_id,
                    CheckDraft {
                        attempt_id: None,
                        kind: kind.into(),
                        status: CheckStatus::Passed,
                        mandatory: true,
                        summary: "passed".into(),
                        details: json!({}),
                    },
                )
                .await
                .expect("governance gate check");
        }
        store
            .record_verification_check(
                &case.case_id,
                CheckDraft {
                    attempt_id: None,
                    kind: "goal_coverage_review".into(),
                    status: CheckStatus::Passed,
                    mandatory: false,
                    summary: "coverage passed".into(),
                    details: json!({"coverage_report":report(VerificationVerdict::Accepted)}),
                },
            )
            .await
            .expect("governance coverage");
        store
            .transition_verification_case(&case.case_id, 0, VerificationStage::Review, None)
            .await
            .expect("review");
        store
            .transition_verification_case(&case.case_id, 0, VerificationStage::Adjudication, None)
            .await
            .expect("adjudication");
        store
            .transition_verification_case(
                &case.case_id,
                0,
                VerificationStage::CommitReady,
                Some(AcceptanceClass::FullyCertified),
            )
            .await
            .expect("commit ready");
    }

    #[test]
    fn verifier_items_are_normalized_and_deduplicated_without_changing_necessity() {
        let report = VerificationReport {
            verdict: VerificationVerdict::Rejected,
            summary: "gap".into(),
            critical_errors: Vec::new(),
            gaps: vec![
                VerificationGap {
                    location: "Step 2".into(),
                    gap_type: "logical".into(),
                    issue: "Missing bridge".into(),
                },
                VerificationGap {
                    location: " step 2 ".into(),
                    gap_type: "LOGICAL".into(),
                    issue: "missing   bridge".into(),
                },
            ],
            uncertainties: Vec::new(),
            repair_actions: vec!["Prove bridge lemma".into(), " prove  bridge lemma ".into()],
            checked_fact_ids: Vec::new(),
            checked_source_ids: Vec::new(),
            evidence_level: "reviewed".into(),
        };
        let seeds = advisory_seeds(&report, "verification_report");
        assert_eq!(seeds.len(), 2);
        assert!(seeds.iter().all(|seed| seed.occurrences == 2));
    }

    #[tokio::test]
    async fn project_creation_atomically_establishes_required_root_visible_to_planning() {
        let (store, _temp) = store().await;
        let (project, _) = store
            .create_project("root-obligation".into(), contract(), Budget::default())
            .await
            .expect("project");
        let graph = store
            .proof_obligation_graph(&project.project_id)
            .await
            .expect("obligation graph");
        assert_eq!(graph.obligations.len(), 1);
        let root = &graph.obligations[0];
        assert_eq!(root.source_kind, "root_goal");
        assert_eq!(root.necessity, ProofObligationNecessity::Required);
        assert_eq!(root.status, ProofObligationStatus::Open);
        assert!(root.source_bottleneck_id.is_some());
        let digest = store
            .context_digest(&project.project_id)
            .await
            .expect("planning digest");
        assert_eq!(
            digest["open_proof_obligations"]
                .as_array()
                .expect("open obligations")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn rejected_report_creates_deduplicated_advisory_debt_with_provenance() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::Reviewed,
            CandidateType::Theorem,
            None,
        )
        .await;
        let mut rejected = report(VerificationVerdict::Rejected);
        rejected.gaps = vec![
            VerificationGap {
                location: "step 2".into(),
                gap_type: "logical".into(),
                issue: "missing bridge".into(),
            },
            VerificationGap {
                location: "STEP 2".into(),
                gap_type: "logical".into(),
                issue: "missing   bridge".into(),
            },
        ];
        rejected.repair_actions = vec!["Prove the bridge lemma".into()];
        store
            .commit_verification(&fixture.verification_id, rejected)
            .await
            .expect("rejected commit");
        let graph = store
            .proof_obligation_graph(&fixture.project_id)
            .await
            .expect("graph");
        assert_eq!(graph.obligations.len(), 3, "root plus two unique debts");
        let children = graph
            .obligations
            .iter()
            .filter(|item| item.source_kind != "root_goal")
            .collect::<Vec<_>>();
        assert!(children.iter().all(|item| {
            item.necessity == ProofObligationNecessity::Advisory
                && item.status == ProofObligationStatus::Open
                && item.source_verification_id.as_deref() == Some(fixture.verification_id.as_str())
                && item.source_bottleneck_id.is_some()
                && item.provenance["candidate_id"] == "obligation-candidate"
        }));
        assert_eq!(graph.edges.len(), 2);
        assert_eq!(graph.coverage.len(), 1);
        assert_eq!(
            graph.coverage[0].disposition,
            ObligationCoverageDisposition::Insufficient
        );
    }

    #[tokio::test]
    async fn reviewed_fact_supports_but_does_not_satisfy_root() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::Reviewed,
            CandidateType::Theorem,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("reviewed Fact");
        let graph = store
            .proof_obligation_graph(&fixture.project_id)
            .await
            .expect("graph");
        assert_eq!(graph.obligations[0].status, ProofObligationStatus::Open);
        assert_eq!(
            graph.coverage[0].disposition,
            ObligationCoverageDisposition::Supports
        );
        assert_eq!(
            store.list_goals(&fixture.project_id).await.expect("goals")[0]
                .status
                .to_string(),
            "open"
        );
    }

    #[tokio::test]
    async fn reviewed_counterexample_cannot_refute_goal_or_project() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::Reviewed,
            CandidateType::Counterexample,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("reviewed counterexample Fact");
        assert_eq!(
            store.list_goals(&fixture.project_id).await.expect("goals")[0]
                .status
                .to_string(),
            "open"
        );
        assert_eq!(
            store
                .get_project(&fixture.project_id)
                .await
                .expect("project")
                .status
                .to_string(),
            "running"
        );
    }

    #[tokio::test]
    async fn certified_counterexample_can_refute_project_and_finish_its_round() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Counterexample,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        let (round, _) = store
            .begin_round(&fixture.project_id)
            .await
            .expect("active verification round");
        sqlx::query("UPDATE rounds SET status='verifying' WHERE round_id=?")
            .bind(&round.round_id)
            .execute(store.pool())
            .await
            .expect("round entered verification");

        store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("certified counterexample");
        assert_eq!(
            store
                .get_project(&fixture.project_id)
                .await
                .expect("refuted project")
                .status,
            research_domain::ProjectStatus::Refuted
        );

        store
            .complete_round(
                &fixture.project_id,
                &round.round_id,
                "certified counterexample refuted the target",
            )
            .await
            .expect("refuted project may finish its active round");
        assert_eq!(
            store
                .current_round(&fixture.project_id)
                .await
                .expect("round query")
                .expect("round")
                .status,
            research_domain::RoundStatus::Completed
        );
        assert_eq!(
            store
                .get_project(&fixture.project_id)
                .await
                .expect("project remains refuted")
                .status,
            research_domain::ProjectStatus::Refuted
        );
    }

    #[tokio::test]
    async fn certified_closure_conflict_can_finish_round_for_human_review() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Counterexample,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        let (round, _) = store
            .begin_round(&fixture.project_id)
            .await
            .expect("active verification round");
        sqlx::query("UPDATE rounds SET status='verifying' WHERE round_id=?")
            .bind(&round.round_id)
            .execute(store.pool())
            .await
            .expect("round entered verification");
        seed_existing_root_closure(&store, &fixture).await;

        store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("conflicting certified counterexample");
        assert_eq!(
            store
                .get_project(&fixture.project_id)
                .await
                .expect("review project")
                .status,
            research_domain::ProjectStatus::NeedsHumanReview
        );

        store
            .complete_round(
                &fixture.project_id,
                &round.round_id,
                "certified closure conflict requires human review",
            )
            .await
            .expect("human-review project may finish its active round");
        assert_eq!(
            store
                .current_round(&fixture.project_id)
                .await
                .expect("round query")
                .expect("round")
                .status,
            research_domain::RoundStatus::Completed
        );
        assert_eq!(
            store
                .get_project(&fixture.project_id)
                .await
                .expect("project remains in review")
                .status,
            research_domain::ProjectStatus::NeedsHumanReview
        );
    }

    #[tokio::test]
    async fn fully_certified_fact_satisfies_root_unless_required_child_is_open() {
        for blocked in [false, true] {
            let (store, _temp) = store().await;
            let fixture = verification_fixture(
                &store,
                AcceptanceClass::FullyCertified,
                CandidateType::Theorem,
                Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
            )
            .await;
            if blocked {
                let root = store
                    .list_proof_obligations(&fixture.project_id)
                    .await
                    .expect("root")
                    .remove(0);
                let now = chrono::Utc::now().to_rfc3339();
                sqlx::query("INSERT INTO proof_obligations(obligation_id,project_id,goal_id,parent_obligation_id,source_kind,statement,completion_criteria,necessity,status,priority,source_fingerprint,provenance_json,created_revision,updated_revision,created_at,updated_at) VALUES('required-child',?,?,?,'human_required_decomposition','Required bridge','Prove the bridge','required','open',2.0,'required-child','{}',1,1,?,?)")
                    .bind(&fixture.project_id)
                    .bind(&fixture.goal_id)
                    .bind(&root.obligation_id)
                    .bind(&now)
                    .bind(&now)
                    .execute(store.pool())
                    .await
                    .expect("required child");
            }
            store
                .commit_verification(
                    &fixture.verification_id,
                    report(VerificationVerdict::Accepted),
                )
                .await
                .expect("certified commit");
            let graph = store
                .proof_obligation_graph(&fixture.project_id)
                .await
                .expect("graph");
            let root = graph
                .obligations
                .iter()
                .find(|item| item.source_kind == "root_goal")
                .expect("root");
            if blocked {
                assert_eq!(root.status, ProofObligationStatus::Open);
                assert_eq!(
                    graph.coverage[0].disposition,
                    ObligationCoverageDisposition::Supports
                );
            } else {
                assert_eq!(root.status, ProofObligationStatus::Satisfied);
                assert!(root.satisfied_by_fact_id.is_some());
                assert_eq!(
                    graph.coverage[0].disposition,
                    ObligationCoverageDisposition::Satisfies
                );
            }
        }
    }

    #[tokio::test]
    async fn repeated_fully_certified_proof_records_satisfaction_without_reclosing_root() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Theorem,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        seed_existing_root_closure(&store, &fixture).await;

        store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("independent certified closure");

        let graph = store
            .proof_obligation_graph(&fixture.project_id)
            .await
            .expect("graph");
        let root = graph
            .obligations
            .iter()
            .find(|item| item.source_kind == "root_goal")
            .expect("root");
        assert_eq!(root.status, ProofObligationStatus::Satisfied);
        assert_eq!(
            root.satisfied_by_fact_id.as_deref(),
            Some("existing-certified-fact"),
            "the first audited closing Fact remains the root witness"
        );
        assert_eq!(
            graph.coverage[0].disposition,
            ObligationCoverageDisposition::Satisfies
        );
    }

    #[tokio::test]
    async fn certified_counterexample_conflicting_with_satisfied_root_fails_closed() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Counterexample,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        seed_existing_root_closure(&store, &fixture).await;

        store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("conflicting certified counterexample");

        let graph = store
            .proof_obligation_graph(&fixture.project_id)
            .await
            .expect("graph");
        let root = graph
            .obligations
            .iter()
            .find(|item| item.source_kind == "root_goal")
            .expect("root");
        assert_eq!(root.status, ProofObligationStatus::Blocked);
        assert!(root.satisfied_by_fact_id.is_none());
        assert_eq!(
            graph.coverage[0].disposition,
            ObligationCoverageDisposition::Unknown
        );
        assert_eq!(
            store.list_goals(&fixture.project_id).await.expect("goals")[0]
                .status
                .to_string(),
            "open"
        );
        assert_eq!(
            store
                .get_project(&fixture.project_id)
                .await
                .expect("project")
                .status
                .to_string(),
            "needs_human_review"
        );
        let conflicts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM uncertainties WHERE project_id=? AND type='certified_closure_conflict' AND severity='critical' AND status='open'")
            .bind(&fixture.project_id)
            .fetch_one(store.pool())
            .await
            .expect("conflict uncertainty");
        assert_eq!(conflicts, 1);
    }

    #[tokio::test]
    async fn repeated_certified_counterexample_keeps_refutation_idempotent() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Counterexample,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        seed_existing_root_closure(&store, &fixture).await;
        sqlx::query("UPDATE goals SET status='refuted' WHERE project_id=? AND goal_id=?")
            .bind(&fixture.project_id)
            .bind(&fixture.goal_id)
            .execute(store.pool())
            .await
            .expect("existing refutation");
        sqlx::query("UPDATE proof_obligations SET status='obsolete',satisfied_by_fact_id=NULL WHERE project_id=? AND goal_id=? AND source_kind='root_goal'")
            .bind(&fixture.project_id)
            .bind(&fixture.goal_id)
            .execute(store.pool())
            .await
            .expect("obsolete root");

        store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("independent certified refutation");
        let graph = store
            .proof_obligation_graph(&fixture.project_id)
            .await
            .expect("graph");
        assert_eq!(graph.obligations[0].status, ProofObligationStatus::Obsolete);
        assert_eq!(
            graph.coverage[0].disposition,
            ObligationCoverageDisposition::Supports
        );
        assert_eq!(
            store.list_goals(&fixture.project_id).await.expect("goal")[0]
                .status
                .to_string(),
            "refuted"
        );
        assert_eq!(
            store
                .get_project(&fixture.project_id)
                .await
                .expect("project")
                .status
                .to_string(),
            "refuted"
        );
    }

    #[tokio::test]
    async fn certified_proof_after_certified_refutation_fails_closed() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Theorem,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        seed_existing_root_closure(&store, &fixture).await;
        sqlx::query("UPDATE goals SET status='refuted' WHERE project_id=? AND goal_id=?")
            .bind(&fixture.project_id)
            .bind(&fixture.goal_id)
            .execute(store.pool())
            .await
            .expect("existing refutation");
        sqlx::query("UPDATE proof_obligations SET status='obsolete',satisfied_by_fact_id=NULL WHERE project_id=? AND goal_id=? AND source_kind='root_goal'")
            .bind(&fixture.project_id)
            .bind(&fixture.goal_id)
            .execute(store.pool())
            .await
            .expect("obsolete root");

        store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("conflicting certified proof");
        let graph = store
            .proof_obligation_graph(&fixture.project_id)
            .await
            .expect("graph");
        assert_eq!(graph.obligations[0].status, ProofObligationStatus::Blocked);
        assert_eq!(
            graph.coverage[0].disposition,
            ObligationCoverageDisposition::Unknown
        );
        assert_eq!(
            store.list_goals(&fixture.project_id).await.expect("goal")[0]
                .status
                .to_string(),
            "open"
        );
        assert_eq!(
            store
                .get_project(&fixture.project_id)
                .await
                .expect("project")
                .status
                .to_string(),
            "needs_human_review"
        );
    }

    #[tokio::test]
    async fn certified_counterexample_for_non_main_goal_does_not_refute_project() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Counterexample,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        sqlx::query("UPDATE goals SET priority=0.5 WHERE project_id=? AND goal_id=?")
            .bind(&fixture.project_id)
            .bind(&fixture.goal_id)
            .execute(store.pool())
            .await
            .expect("non-main goal");
        store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("subgoal refutation");
        assert_eq!(
            store.list_goals(&fixture.project_id).await.expect("goal")[0]
                .status
                .to_string(),
            "refuted"
        );
        assert_eq!(
            store
                .get_project(&fixture.project_id)
                .await
                .expect("project")
                .status
                .to_string(),
            "running"
        );
    }

    #[tokio::test]
    async fn nonpassing_goal_coverage_becomes_advisory_debt_without_rejecting_fact() {
        let (store, _temp) = store().await;
        let mut coverage_report = report(VerificationVerdict::Rejected);
        coverage_report.gaps.push(VerificationGap {
            location: "target implication".into(),
            gap_type: "goal_coverage".into(),
            issue: "the lemma does not imply P".into(),
        });
        coverage_report
            .repair_actions
            .push("Prove an implication from the lemma to P".into());
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::Reviewed,
            CandidateType::Lemma,
            Some((CheckStatus::Failed, coverage_report)),
        )
        .await;
        let commit = store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("valid intermediate Fact");
        assert!(commit.fact.is_some());
        let graph = store
            .proof_obligation_graph(&fixture.project_id)
            .await
            .expect("graph");
        assert_eq!(
            graph.coverage[0].disposition,
            ObligationCoverageDisposition::Insufficient
        );
        assert!(graph.obligations.iter().any(|item| {
            item.source_kind == "goal_coverage_gap"
                && item.necessity == ProofObligationNecessity::Advisory
                && item.source_bottleneck_id.is_some()
        }));
        let debt_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM bottlenecks WHERE project_id=? AND kind='goal_coverage_debt'",
        )
        .bind(&fixture.project_id)
        .fetch_one(store.pool())
        .await
        .expect("coverage debt");
        assert_eq!(debt_count, 1);
    }

    #[tokio::test]
    async fn problem_revision_obsoletes_old_graph_and_creates_new_required_root() {
        let (store, _temp) = store().await;
        let (project, _) = store
            .create_project("revision-obligation".into(), contract(), Budget::default())
            .await
            .expect("project");
        let old_root = store
            .list_proof_obligations(&project.project_id)
            .await
            .expect("old root")
            .remove(0);
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO proof_obligations(obligation_id,project_id,goal_id,parent_obligation_id,source_kind,statement,completion_criteria,necessity,status,priority,source_fingerprint,provenance_json,created_revision,updated_revision,created_at,updated_at) VALUES('project-level-v1-debt',?,NULL,NULL,'verification_gap','old project-level verifier gap','recheck the old contract','advisory','open',9.0,'project-level-v1-debt','{}',?,?,?,?)")
            .bind(&project.project_id)
            .bind(project.revision)
            .bind(project.revision)
            .bind(&now)
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("project-level v1 advisory");
        let mutation = store
            .revise_problem_contract(
                &project.project_id,
                &ProblemRevisionRequest {
                    expected_revision: project.revision,
                    target_statement: "Q".into(),
                    assumptions: Vec::new(),
                    success_criteria: "A verified proof of Q".into(),
                    change_reason: "replace target".into(),
                    replan: true,
                },
                "operator",
                "revision-obligation-1",
            )
            .await
            .expect("problem revision");
        let graph = store
            .proof_obligation_graph(&project.project_id)
            .await
            .expect("graph");
        assert_eq!(graph.obligations.len(), 3);
        let obsolete = graph
            .obligations
            .iter()
            .find(|item| item.obligation_id == old_root.obligation_id)
            .expect("historical root");
        assert_eq!(obsolete.status, ProofObligationStatus::Obsolete);
        let obsolete_project_debt = graph
            .obligations
            .iter()
            .find(|item| item.obligation_id == "project-level-v1-debt")
            .expect("historical project-level advisory");
        assert_eq!(
            obsolete_project_debt.status,
            ProofObligationStatus::Obsolete
        );
        assert!(obsolete_project_debt.goal_id.is_none());
        let current = graph
            .obligations
            .iter()
            .find(|item| item.status == ProofObligationStatus::Open)
            .expect("current root");
        assert_eq!(current.statement, "Q");
        assert_eq!(current.necessity, ProofObligationNecessity::Required);
        assert_ne!(current.obligation_id, old_root.obligation_id);
        assert!(mutation.events.iter().any(|event| {
            event.event_type == "proof_obligation.created"
                && event.entity.id == current.obligation_id
        }));
        assert!(mutation.events.iter().any(|event| {
            event.event_type == "proof_obligation.updated"
                && event.entity.id == old_root.obligation_id
        }));
        assert!(mutation.events.iter().any(|event| {
            event.event_type == "proof_obligation.updated"
                && event.entity.id == "project-level-v1-debt"
        }));
    }

    #[tokio::test]
    async fn problem_revision_fences_claimed_verification_from_the_new_root() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Theorem,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO uncertainties(uncertainty_id,project_id,description,type,severity,affects_goal_ids_json,affects_route_ids_json,introduced_by,resolution_methods_json,status,created_at) VALUES('old-contract-verifier-uncertainty',?,'old contract is inconclusive','verification_unknown','high','[]','[]',?,'[]','open',?)")
            .bind(&fixture.project_id)
            .bind(&fixture.verification_id)
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("old verification uncertainty");
        sqlx::query("INSERT INTO uncertainties(uncertainty_id,project_id,description,type,severity,affects_goal_ids_json,affects_route_ids_json,introduced_by,resolution_methods_json,status,created_at) VALUES('human-global-uncertainty',?,'human global risk remains relevant','human_review','high','[]','[]','operator','[]','open',?)")
            .bind(&fixture.project_id)
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("human uncertainty");
        sqlx::query("INSERT INTO worker_nodes(node_id,display_name,capabilities_json,token_hash,status,node_epoch,registered_at,last_heartbeat) VALUES('revision-result-node','revision result node','{}','revision-result-node-token','active',1,?,?)")
            .bind(&now)
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("worker node");
        sqlx::query("INSERT INTO tasks(task_id,project_id,route_id,worker_role,goal_ids_json,objective,completion_contract,status,priority,revision,route_cancellation_epoch,round) VALUES('revision-result-task',?,'obligation-route','prover',?,'old contract work','return old result','result_submitted',1.0,1,0,1)")
            .bind(&fixture.project_id)
            .bind(json!([fixture.goal_id.clone()]).to_string())
            .execute(store.pool())
            .await
            .expect("old task");
        sqlx::query("INSERT INTO task_attempts(attempt_id,project_id,task_id,attempt_no,status,lease_epoch,route_cancellation_epoch,created_at) VALUES('revision-result-attempt',?,'revision-result-task',1,'result_submitted',1,0,?)")
            .bind(&fixture.project_id)
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("old task attempt");
        sqlx::query("INSERT INTO task_leases(lease_id,project_id,task_id,node_id,task_revision,route_epoch,lease_epoch,status,leased_at,expires_at,attempt_id) VALUES('revision-result-lease',?,'revision-result-task','revision-result-node',1,0,1,'active',?,?, 'revision-result-attempt')")
            .bind(&fixture.project_id)
            .bind(&now)
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("old task lease");
        sqlx::query("INSERT INTO result_envelopes(result_envelope_id,project_id,task_id,attempt_id,lease_id,lease_epoch,route_cancellation_epoch,outcome,envelope_json,content_hash,idempotency_key,status,submitted_at) VALUES('revision-result-envelope',?,'revision-result-task','revision-result-attempt','revision-result-lease',1,0,'completed','{}','revision-result-hash','revision-result-key','submitted',?)")
            .bind(&fixture.project_id)
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("submitted old result");
        let before = store
            .get_project(&fixture.project_id)
            .await
            .expect("project");
        let mutation = store
            .revise_problem_contract(
                &fixture.project_id,
                &ProblemRevisionRequest {
                    expected_revision: before.revision,
                    target_statement: "Q".into(),
                    assumptions: Vec::new(),
                    success_criteria: "A verified proof of Q".into(),
                    change_reason: "replace P after verifier claim".into(),
                    replan: true,
                },
                "operator",
                "revision-fences-verifier",
            )
            .await
            .expect("problem revision");
        let error = store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect_err("old report must not close the revised contract");
        assert!(
            error
                .to_string()
                .contains("late or stale worker submission")
        );

        let goals = store.list_goals(&fixture.project_id).await.expect("goals");
        assert_eq!(goals[0].statement, "Q");
        assert_eq!(goals[0].status.to_string(), "open");
        let roots = store
            .list_proof_obligations(&fixture.project_id)
            .await
            .expect("roots");
        let current = roots
            .iter()
            .find(|item| item.status == ProofObligationStatus::Open)
            .expect("new root");
        assert_eq!(current.statement, "Q");
        assert!(current.satisfied_by_fact_id.is_none());
        assert!(
            store
                .list_facts(&fixture.project_id)
                .await
                .expect("facts")
                .is_empty()
        );
        let verification_status: String =
            sqlx::query_scalar("SELECT status FROM verifications WHERE verification_id=?")
                .bind(&fixture.verification_id)
                .fetch_one(store.pool())
                .await
                .expect("verification status");
        assert_eq!(verification_status, "superseded");
        let case_stage: String =
            sqlx::query_scalar("SELECT stage FROM verification_cases WHERE verification_id=?")
                .bind(&fixture.verification_id)
                .fetch_one(store.pool())
                .await
                .expect("case stage");
        assert_eq!(case_stage, "cancelled");
        // Defense in depth: even if an external writer resurrected the verification status,
        // the authoritative route epoch still fences the old contract snapshot at commit.
        sqlx::query("UPDATE verifications SET status='verifying' WHERE verification_id=?")
            .bind(&fixture.verification_id)
            .execute(store.pool())
            .await
            .expect("simulate stale external writer");
        let epoch_error = store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect_err("route epoch fence must reject resurrected old work");
        assert!(epoch_error.to_string().contains("belongs to route epoch"));
        let current_route_epoch: i64 = sqlx::query_scalar(
            "SELECT cancellation_epoch FROM routes WHERE route_id='obligation-route'",
        )
        .fetch_one(store.pool())
        .await
        .expect("current route epoch");
        let mut forged_submission: CandidateSubmission = serde_json::from_str(
            &sqlx::query_scalar::<_, String>(
                "SELECT submission_json FROM candidates WHERE candidate_id='obligation-candidate'",
            )
            .fetch_one(store.pool())
            .await
            .expect("candidate submission"),
        )
        .expect("submission json");
        forged_submission.route_cancellation_epoch = current_route_epoch;
        sqlx::query(
            "UPDATE candidates SET submission_json=? WHERE candidate_id='obligation-candidate'",
        )
        .bind(serde_json::to_string(&forged_submission).expect("forged submission json"))
        .execute(store.pool())
        .await
        .expect("forge current route epoch");
        let forged_epoch_error = store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect_err("current route epoch cannot override the task's frozen epoch");
        assert!(forged_epoch_error.to_string().contains("task epoch is 0"));
        let result_envelope_status: String = sqlx::query_scalar(
            "SELECT status FROM result_envelopes WHERE result_envelope_id='revision-result-envelope'",
        )
        .fetch_one(store.pool())
        .await
        .expect("result envelope status");
        assert_eq!(result_envelope_status, "stale");
        let result_attempt_status: String = sqlx::query_scalar(
            "SELECT status FROM task_attempts WHERE attempt_id='revision-result-attempt'",
        )
        .fetch_one(store.pool())
        .await
        .expect("result attempt status");
        assert_eq!(result_attempt_status, "cancelled");
        let old_uncertainty_status: String = sqlx::query_scalar(
            "SELECT status FROM uncertainties WHERE uncertainty_id='old-contract-verifier-uncertainty'",
        )
        .fetch_one(store.pool())
        .await
        .expect("old uncertainty status");
        assert_eq!(old_uncertainty_status, "obsolete");
        let human_uncertainty_status: String = sqlx::query_scalar(
            "SELECT status FROM uncertainties WHERE uncertainty_id='human-global-uncertainty'",
        )
        .fetch_one(store.pool())
        .await
        .expect("human uncertainty status");
        assert_eq!(human_uncertainty_status, "open");
        assert!(mutation.events.iter().any(|event| {
            event.event_type == "verification.superseded"
                && event.entity.id == fixture.verification_id
        }));
        assert!(mutation.events.iter().any(|event| {
            event.event_type == "result_envelope.stale"
                && event.entity.id == "revision-result-envelope"
        }));
        assert!(mutation.events.iter().any(|event| {
            event.event_type == "uncertainty.obsolete"
                && event.entity.id == "old-contract-verifier-uncertainty"
        }));
        assert_eq!(
            store
                .get_project(&fixture.project_id)
                .await
                .expect("project")
                .status
                .to_string(),
            "running"
        );
    }

    #[tokio::test]
    async fn problem_revision_cascades_removed_assumption_through_fact_dependencies() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Theorem,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        let root_fact = store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("certified root")
            .fact
            .expect("root Fact");
        let mut previous_contract = contract();
        previous_contract.assumptions = vec!["H".into()];
        sqlx::query(
            "UPDATE projects SET problem_contract_json=?,status='success' WHERE project_id=?",
        )
        .bind(serde_json::to_string(&previous_contract).expect("contract json"))
        .bind(&fixture.project_id)
        .execute(store.pool())
        .await
        .expect("legacy assumption");
        sqlx::query("UPDATE facts SET assumptions_json='[\"H\"]' WHERE fact_id=?")
            .bind(&root_fact.fact_id)
            .execute(store.pool())
            .await
            .expect("Fact assumption");
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO facts(fact_id,project_id,statement,assumptions_json,proof_markdown,dependency_fact_ids_json,definitions_introduced_json,external_source_ids_json,verification_ids_json,evidence_level,created_by,status,content_hash,created_at) VALUES('revision-downstream',?,'Q','[]','derived proof',?,'{}','[]','[]','fully_certified','fixture','active','revision-downstream-content',?)")
            .bind(&fixture.project_id)
            .bind(json!([root_fact.fact_id.clone()]).to_string())
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("downstream Fact");
        sqlx::query("INSERT INTO fact_edges(edge_id,project_id,source_id,target_id,kind) VALUES('revision-downstream-edge',?,?,'revision-downstream','depends_on')")
            .bind(&fixture.project_id)
            .bind(&root_fact.fact_id)
            .execute(store.pool())
            .await
            .expect("dependency edge");
        let case_id: String =
            sqlx::query_scalar("SELECT case_id FROM verification_cases WHERE verification_id=?")
                .bind(&fixture.verification_id)
                .fetch_one(store.pool())
                .await
                .expect("case");
        sqlx::query("INSERT INTO fact_assurances(assurance_id,fact_id,case_id,acceptance_class,snapshot_hash,status,created_at) VALUES('revision-downstream-assurance','revision-downstream',?,'fully_certified','revision-downstream-snapshot','active',?)")
            .bind(&case_id)
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("downstream assurance");
        sqlx::query("INSERT INTO routes(route_id,project_id,title,method_summary,target_goal_ids_json,required_fact_ids_json,status,score,priority,cancellation_epoch,created_in_round,attributes_json) VALUES('revision-dependent-route',?,'dependent','uses H Fact',?,?,'active',1.0,0.5,0,1,'{}')")
            .bind(&fixture.project_id)
            .bind(json!([fixture.goal_id.clone()]).to_string())
            .bind(json!([root_fact.fact_id.clone()]).to_string())
            .execute(store.pool())
            .await
            .expect("dependent route");
        let before = store
            .get_project(&fixture.project_id)
            .await
            .expect("project");
        let mutation = store
            .revise_problem_contract(
                &fixture.project_id,
                &ProblemRevisionRequest {
                    expected_revision: before.revision,
                    target_statement: "Q".into(),
                    assumptions: Vec::new(),
                    success_criteria: "A verified proof of Q".into(),
                    change_reason: "remove H".into(),
                    replan: true,
                },
                "operator",
                "revision-removes-assumption",
            )
            .await
            .expect("revision");

        for fact_id in [&root_fact.fact_id, "revision-downstream"] {
            assert_eq!(
                store
                    .get_fact(fact_id)
                    .await
                    .expect("Fact")
                    .status
                    .to_string(),
                "suspended"
            );
        }
        let active_assurances: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM fact_assurances WHERE fact_id IN (?, 'revision-downstream') AND status='active'",
        )
        .bind(&root_fact.fact_id)
        .fetch_one(store.pool())
        .await
        .expect("assurances");
        assert_eq!(active_assurances, 0);
        let route_status: String = sqlx::query_scalar(
            "SELECT status FROM routes WHERE route_id='revision-dependent-route'",
        )
        .fetch_one(store.pool())
        .await
        .expect("route");
        assert_eq!(route_status, "paused");
        let roots = store
            .list_proof_obligations(&fixture.project_id)
            .await
            .expect("roots");
        assert!(roots.iter().any(|item| {
            item.statement == "P" && item.status == ProofObligationStatus::Obsolete
        }));
        assert!(
            roots.iter().any(|item| {
                item.statement == "Q" && item.status == ProofObligationStatus::Open
            })
        );
        assert_eq!(
            store
                .get_project(&fixture.project_id)
                .await
                .expect("project")
                .status
                .to_string(),
            "running"
        );
        assert!(mutation.events.iter().any(|event| {
            event.event_type == "project.reopened" && event.data["previous_status"] == "success"
        }));
        let suspended_events = mutation
            .events
            .iter()
            .filter(|event| event.event_type == "fact.suspended")
            .count();
        assert_eq!(suspended_events, 2);
    }

    #[tokio::test]
    async fn stopped_project_cannot_be_reopened_by_problem_revision() {
        let (store, _temp) = store().await;
        let (project, _) = store
            .create_project("stopped-revision".into(), contract(), Budget::default())
            .await
            .expect("project");
        sqlx::query("UPDATE projects SET status='stopped_by_human' WHERE project_id=?")
            .bind(&project.project_id)
            .execute(store.pool())
            .await
            .expect("stopped project");
        let error = store
            .revise_problem_contract(
                &project.project_id,
                &ProblemRevisionRequest {
                    expected_revision: project.revision,
                    target_statement: "Q".into(),
                    assumptions: Vec::new(),
                    success_criteria: "prove Q".into(),
                    change_reason: "must not bypass stop".into(),
                    replan: true,
                },
                "operator",
                "stopped-revision-attempt",
            )
            .await
            .expect_err("human stop is absorbing");
        assert!(error.to_string().contains("stopped_by_human"));
    }

    #[tokio::test]
    async fn problem_revision_marks_old_publication_stale_and_late_completion_is_noop() {
        let (store, _temp) = store().await;
        let (project, _) = store
            .create_project("publication-revision".into(), contract(), Budget::default())
            .await
            .expect("project");
        let (publication, _) = store
            .begin_publication(
                &project.project_id,
                "publication-before-revision",
                true,
                project.revision,
                "old-source-packet",
            )
            .await
            .expect("publication");
        let before = store
            .get_project(&project.project_id)
            .await
            .expect("project");
        let mutation = store
            .revise_problem_contract(
                &project.project_id,
                &ProblemRevisionRequest {
                    expected_revision: before.revision,
                    target_statement: "Q".into(),
                    assumptions: Vec::new(),
                    success_criteria: "prove Q".into(),
                    change_reason: "publication source became stale".into(),
                    replan: true,
                },
                "operator",
                "publication-revision",
            )
            .await
            .expect("revision");
        let stale = store
            .get_publication(&publication.publication_id)
            .await
            .expect("stale publication");
        assert_eq!(stale.status, "stale");
        let (late, event) = store
            .complete_publication(
                &publication.publication_id,
                "ready",
                Some(&json!({"paper":"old"})),
                None,
            )
            .await
            .expect("late completion is idempotent no-op");
        assert_eq!(late.status, "stale");
        assert!(event.is_none());
        assert!(mutation.events.iter().any(|event| {
            event.event_type == "publication.stale" && event.entity.id == publication.publication_id
        }));
        assert!(mutation.data.affected_entities.iter().any(|entity| {
            entity.kind == "publication" && entity.id == publication.publication_id
        }));
    }

    #[tokio::test]
    async fn revision_cancels_old_challenge_and_allows_fresh_contract_reverification() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Theorem,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        let fact = store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("v1 Fact")
            .fact
            .expect("Fact");
        let (old_review, _) = store
            .govern_fact(
                &fact.fact_id,
                "reverify",
                "old-contract review",
                "operator",
                "old-contract-review",
            )
            .await
            .expect("old review");
        let old_verification_id = old_review.verification_id.expect("old verification");
        let before = store
            .get_project(&fixture.project_id)
            .await
            .expect("project");
        store
            .revise_problem_contract(
                &fixture.project_id,
                &ProblemRevisionRequest {
                    expected_revision: before.revision,
                    target_statement: "Q".into(),
                    assumptions: Vec::new(),
                    success_criteria: "A verified proof of Q".into(),
                    change_reason: "new contract".into(),
                    replan: true,
                },
                "operator",
                "revision-cancels-review",
            )
            .await
            .expect("revision");
        let old_challenge_status: String =
            sqlx::query_scalar("SELECT status FROM fact_challenges WHERE verification_id=?")
                .bind(&old_verification_id)
                .fetch_one(store.pool())
                .await
                .expect("old challenge");
        assert_eq!(old_challenge_status, "cancelled");
        let old_verification_status: String =
            sqlx::query_scalar("SELECT status FROM verifications WHERE verification_id=?")
                .bind(&old_verification_id)
                .fetch_one(store.pool())
                .await
                .expect("old verification");
        assert_eq!(old_verification_status, "superseded");
        assert_eq!(
            store
                .get_fact(&fact.fact_id)
                .await
                .expect("Fact")
                .status
                .to_string(),
            "suspended"
        );

        let (fresh_review, _) = store
            .govern_fact(
                &fact.fact_id,
                "reverify",
                "review against v2 contract",
                "operator",
                "fresh-contract-review",
            )
            .await
            .expect("fresh review");
        let fresh_verification_id = fresh_review.verification_id.expect("fresh verification");
        store
            .mark_verification_started(&fresh_verification_id)
            .await
            .expect("fresh claim");
        prepare_fully_certified_case(&store, &fresh_verification_id).await;
        let snapshot_contract_version: i64 = sqlx::query_scalar(
            "SELECT vs.contract_version FROM verification_snapshots vs JOIN verification_cases vc ON vc.snapshot_id=vs.snapshot_id WHERE vc.verification_id=?",
        )
        .bind(&fresh_verification_id)
        .fetch_one(store.pool())
        .await
        .expect("snapshot contract version");
        assert_eq!(snapshot_contract_version, 2);
        store
            .commit_verification(
                &fresh_verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("v2 governance verification bypasses only the stale production epoch");
        assert_eq!(
            store
                .get_fact(&fact.fact_id)
                .await
                .expect("Fact")
                .status
                .to_string(),
            "active"
        );
        let current_root = store
            .list_proof_obligations(&fixture.project_id)
            .await
            .expect("roots")
            .into_iter()
            .find(|item| item.statement == "Q")
            .expect("v2 root");
        assert_eq!(current_root.status, ProofObligationStatus::Satisfied);
        let open_challenges: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM fact_challenges WHERE fact_id=? AND status='open'",
        )
        .bind(&fact.fact_id)
        .fetch_one(store.pool())
        .await
        .expect("open challenges");
        assert_eq!(open_challenges, 0);
    }

    #[tokio::test]
    async fn revoking_closing_fact_reopens_goal_and_required_root_atomically() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Theorem,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        let fact = store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("certified commit")
            .fact
            .expect("Fact");
        let (governance, events) = store
            .govern_fact(
                &fact.fact_id,
                "revoke",
                "test invalidation",
                "operator",
                "revoke-obligation-1",
            )
            .await
            .expect("revoke");
        let root = store
            .list_proof_obligations(&fixture.project_id)
            .await
            .expect("obligations")
            .into_iter()
            .find(|item| item.source_kind == "root_goal")
            .expect("root");
        assert_eq!(root.status, ProofObligationStatus::Open);
        assert!(root.satisfied_by_fact_id.is_none());
        assert_eq!(
            store.list_goals(&fixture.project_id).await.expect("goals")[0]
                .status
                .to_string(),
            "open"
        );
        assert!(
            governance
                .impact
                .reopened_obligation_ids
                .contains(&root.obligation_id)
        );
        assert!(events.iter().any(|event| {
            event.event_type == "proof_obligation.updated"
                && event.entity.id == root.obligation_id
                && event.project_revision == root.updated_revision
        }));
    }

    #[tokio::test]
    async fn success_project_fact_governance_reopens_closure_and_releases_only_its_review() {
        for action in ["challenge", "reverify"] {
            let (store, _temp) = store().await;
            let fixture = verification_fixture(
                &store,
                AcceptanceClass::FullyCertified,
                CandidateType::Theorem,
                Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
            )
            .await;
            let fact = store
                .commit_verification(
                    &fixture.verification_id,
                    report(VerificationVerdict::Accepted),
                )
                .await
                .expect("certified commit")
                .fact
                .expect("Fact");
            sqlx::query("UPDATE projects SET status='success' WHERE project_id=?")
                .bind(&fixture.project_id)
                .execute(store.pool())
                .await
                .expect("successful project");
            sqlx::query("UPDATE routes SET status='paused' WHERE project_id=? AND route_id='obligation-route'")
                .bind(&fixture.project_id)
                .execute(store.pool())
                .await
                .expect("closed route");
            sqlx::query("INSERT INTO routes(route_id,project_id,title,method_summary,target_goal_ids_json,required_fact_ids_json,status,score,priority,cancellation_epoch,created_in_round,attributes_json) VALUES('dependent-route',?,'dependent','uses closing Fact',?,?, 'active',1.0,0.5,0,1,'{}')")
                .bind(&fixture.project_id)
                .bind(json!([fixture.goal_id.clone()]).to_string())
                .bind(json!([fact.fact_id.clone()]).to_string())
                .execute(store.pool())
                .await
                .expect("dependent route");

            let (governance, events) = store
                .govern_fact(
                    &fact.fact_id,
                    action,
                    "independent audit requested",
                    "operator",
                    &format!("{action}-success-project"),
                )
                .await
                .expect("governance request");
            let governance_verification_id = governance
                .verification_id
                .as_deref()
                .expect("governance verification");
            let (idempotent, duplicate_events) = store
                .govern_fact(
                    &fact.fact_id,
                    action,
                    "independent audit requested",
                    "operator",
                    &format!("{action}-success-project"),
                )
                .await
                .expect("idempotent governance retry");
            assert_eq!(idempotent.verification_id, governance.verification_id);
            assert!(duplicate_events.is_empty());
            let duplicate = store
                .govern_fact(
                    &fact.fact_id,
                    "reverify",
                    "competing audit",
                    "second-operator",
                    &format!("{action}-competing-review"),
                )
                .await
                .expect_err("a Fact may have only one open challenge");
            assert!(
                duplicate
                    .to_string()
                    .contains("already has active challenge")
            );
            let root = store
                .list_proof_obligations(&fixture.project_id)
                .await
                .expect("obligations")
                .into_iter()
                .find(|item| item.source_kind == "root_goal")
                .expect("root");
            assert_eq!(governance.fact.status.to_string(), "challenged");
            assert_eq!(root.status, ProofObligationStatus::Open);
            assert!(root.satisfied_by_fact_id.is_none());
            assert!(
                governance
                    .impact
                    .reopened_obligation_ids
                    .contains(&root.obligation_id)
            );
            assert_eq!(
                store.list_goals(&fixture.project_id).await.expect("goals")[0]
                    .status
                    .to_string(),
                "open"
            );
            let project = store
                .get_project(&fixture.project_id)
                .await
                .expect("project");
            assert_eq!(project.status.to_string(), "needs_human_review");
            let dependent_status: String = sqlx::query_scalar(
                "SELECT status FROM routes WHERE project_id=? AND route_id='dependent-route'",
            )
            .bind(&fixture.project_id)
            .fetch_one(store.pool())
            .await
            .expect("dependent route status");
            assert_eq!(dependent_status, "paused");
            assert!(events.iter().any(|event| {
                event.event_type == "proof_obligation.updated"
                    && event.entity.id == root.obligation_id
            }));

            store
                .mark_verification_started(governance_verification_id)
                .await
                .expect("open Fact challenge has a dedicated execution release");
            store
                .reserve_model_call(ModelCallRequest {
                    project_id: &fixture.project_id,
                    round: project.current_round,
                    worker_id: None,
                    task_id: None,
                    model: None,
                    purpose: ModelCallPurpose::Verification(governance_verification_id),
                    max_total_calls: project.budget.max_total_model_calls,
                    max_task_calls: project.budget.max_model_calls_per_task,
                })
                .await
                .expect("governance verifier model call is released");
            prepare_fully_certified_case(&store, governance_verification_id).await;
            store
                .commit_verification(
                    governance_verification_id,
                    report(VerificationVerdict::Accepted),
                )
                .await
                .expect("governance recertification commit");
            let restored_project = store
                .get_project(&fixture.project_id)
                .await
                .expect("restored project");
            assert_eq!(restored_project.status.to_string(), "success");
            let restored_root = store
                .list_proof_obligations(&fixture.project_id)
                .await
                .expect("restored root")
                .into_iter()
                .find(|item| item.source_kind == "root_goal")
                .expect("root");
            assert_eq!(restored_root.status, ProofObligationStatus::Satisfied);
            let open_challenges: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM fact_challenges WHERE fact_id=? AND status='open'",
            )
            .bind(&fact.fact_id)
            .fetch_one(store.pool())
            .await
            .expect("open challenges");
            assert_eq!(open_challenges, 0);
        }
    }

    #[tokio::test]
    async fn accepted_root_reverification_does_not_silently_revive_downstream_facts() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Theorem,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        let root_fact = store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("certified root")
            .fact
            .expect("root Fact");
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO facts(fact_id,project_id,statement,assumptions_json,proof_markdown,dependency_fact_ids_json,definitions_introduced_json,external_source_ids_json,verification_ids_json,evidence_level,created_by,status,content_hash,created_at) VALUES('downstream-fact',?,'Q','[]','derived proof',?,'{}','[]','[]','fully_certified','fixture','active','downstream-content',?)")
            .bind(&fixture.project_id)
            .bind(json!([root_fact.fact_id.clone()]).to_string())
            .bind(&now)
            .execute(store.pool())
            .await
            .expect("downstream Fact");
        sqlx::query("INSERT INTO fact_edges(edge_id,project_id,source_id,target_id,kind) VALUES('downstream-edge',?,?,'downstream-fact','depends_on')")
            .bind(&fixture.project_id)
            .bind(&root_fact.fact_id)
            .execute(store.pool())
            .await
            .expect("downstream edge");
        sqlx::query("UPDATE projects SET status='success' WHERE project_id=?")
            .bind(&fixture.project_id)
            .execute(store.pool())
            .await
            .expect("success project");
        let (governance, _) = store
            .govern_fact(
                &root_fact.fact_id,
                "reverify",
                "audit dependency root",
                "operator",
                "downstream-root-review",
            )
            .await
            .expect("root reverify");
        let verification_id = governance.verification_id.expect("verification");
        store
            .mark_verification_started(&verification_id)
            .await
            .expect("governance claim");
        prepare_fully_certified_case(&store, &verification_id).await;
        store
            .commit_verification(&verification_id, report(VerificationVerdict::Accepted))
            .await
            .expect("root recertified");

        let downstream_status: String =
            sqlx::query_scalar("SELECT status FROM facts WHERE fact_id='downstream-fact'")
                .fetch_one(store.pool())
                .await
                .expect("downstream status");
        assert_eq!(downstream_status, "suspended");
        assert_eq!(
            store
                .get_project(&fixture.project_id)
                .await
                .expect("project")
                .status
                .to_string(),
            "needs_human_review"
        );
    }

    #[tokio::test]
    async fn later_suspend_fences_an_open_reverification() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Theorem,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        let fact = store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("certified Fact")
            .fact
            .expect("Fact");
        let (review, _) = store
            .govern_fact(
                &fact.fact_id,
                "reverify",
                "start audit",
                "operator",
                "fenced-review",
            )
            .await
            .expect("reverify");
        let verification_id = review.verification_id.expect("verification");
        store
            .mark_verification_started(&verification_id)
            .await
            .expect("claimed review");
        prepare_fully_certified_case(&store, &verification_id).await;
        let case_id: String =
            sqlx::query_scalar("SELECT case_id FROM verification_cases WHERE verification_id=?")
                .bind(&verification_id)
                .fetch_one(store.pool())
                .await
                .expect("case id");
        let worker_offer = store
            .offer_verification_worker(
                &case_id,
                "independent_fence_test",
                "mock",
                None,
                "runtime/fact-governance-fence",
                &json!({"candidate":"P"}),
                &[],
                &json!({"success":"one auditable verdict"}),
            )
            .await
            .expect("verification worker offer");
        let worker_lease = store
            .accept_verification_worker_handshake(
                &worker_offer,
                Some("mock-1"),
                &json!({"context_hash":worker_offer.context_packet.content_hash}),
                3_600,
            )
            .await
            .expect("verification worker lease");
        store
            .govern_fact(
                &fact.fact_id,
                "suspend",
                "newer human safety decision",
                "operator",
                "fence-with-suspend",
            )
            .await
            .expect("suspend");
        let status: String =
            sqlx::query_scalar("SELECT status FROM verifications WHERE verification_id=?")
                .bind(&verification_id)
                .fetch_one(store.pool())
                .await
                .expect("verification status");
        assert_eq!(status, "superseded");
        let challenge_status: String =
            sqlx::query_scalar("SELECT status FROM fact_challenges WHERE verification_id=?")
                .bind(&verification_id)
                .fetch_one(store.pool())
                .await
                .expect("challenge status");
        assert_eq!(challenge_status, "cancelled");
        let case_stage: String =
            sqlx::query_scalar("SELECT stage FROM verification_cases WHERE verification_id=?")
                .bind(&verification_id)
                .fetch_one(store.pool())
                .await
                .expect("case stage");
        assert_eq!(case_stage, "cancelled");
        let worker_status: String =
            sqlx::query_scalar("SELECT status FROM worker_instances WHERE worker_instance_id=?")
                .bind(&worker_offer.worker_instance_id)
                .fetch_one(store.pool())
                .await
                .expect("worker status");
        assert_eq!(worker_status, "exited");
        let context_status: String =
            sqlx::query_scalar("SELECT status FROM context_packets WHERE context_packet_id=?")
                .bind(&worker_offer.context_packet.context_packet_id)
                .fetch_one(store.pool())
                .await
                .expect("verification context status");
        assert_eq!(context_status, "invalidated");
        assert!(
            store
                .heartbeat_verification_worker(&worker_lease, 60)
                .await
                .is_err()
        );
        assert!(
            store
                .try_mark_verification_started(&verification_id)
                .await
                .expect("claim lookup")
                .is_none()
        );
        assert_eq!(
            store
                .get_fact(&fact.fact_id)
                .await
                .expect("Fact")
                .status
                .to_string(),
            "suspended"
        );
    }

    #[tokio::test]
    async fn stopped_or_error_project_cannot_open_a_fact_governance_review() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Theorem,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        let fact = store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("certified Fact")
            .fact
            .expect("Fact");
        for project_status in ["stopped_by_human", "error"] {
            sqlx::query("UPDATE projects SET status=? WHERE project_id=?")
                .bind(project_status)
                .bind(&fixture.project_id)
                .execute(store.pool())
                .await
                .expect("terminal project status");
            let error = store
                .govern_fact(
                    &fact.fact_id,
                    "reverify",
                    "must use an explicit recovery flow",
                    "operator",
                    &format!("terminal-project-review-{project_status}"),
                )
                .await
                .expect_err("terminal project must reject a new review");
            assert!(error.to_string().contains(project_status));
        }
        let open_challenges: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM fact_challenges WHERE fact_id=? AND status='open'",
        )
        .bind(&fact.fact_id)
        .fetch_one(store.pool())
        .await
        .expect("open challenges");
        assert_eq!(open_challenges, 0);
        assert_eq!(
            store
                .get_fact(&fact.fact_id)
                .await
                .expect("Fact")
                .status
                .to_string(),
            "active"
        );
    }

    #[tokio::test]
    async fn adverse_governance_verdict_requires_snapshot_and_auditable_progress() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Theorem,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        let fact = store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("certified Fact")
            .fact
            .expect("Fact");
        let (review, _) = store
            .govern_fact(
                &fact.fact_id,
                "reverify",
                "adverse result must be auditable",
                "operator",
                "adverse-governance-fence",
            )
            .await
            .expect("reverify");
        let verification_id = review.verification_id.expect("verification");
        store
            .mark_verification_started(&verification_id)
            .await
            .expect("claimed review");
        let (case, _) = store
            .ensure_verification_case(
                &verification_id,
                VerificationCaseDraft {
                    name: "adverse-governance-fence".into(),
                    profile: VerificationProfile::StandardReview,
                    required_acceptance: AcceptanceClass::Reviewed,
                    required_checks: vec!["deterministic_precheck".into(), "math_review_1".into()],
                    independent_reviewer_count: 1,
                    require_citation_review: false,
                    require_adversarial_review: false,
                    require_alignment_review: false,
                    require_fresh_replay: false,
                    max_attempts: 1,
                    risk_score: 1.0,
                    risk_reasons: vec!["fact_governance".into()],
                },
            )
            .await
            .expect("case without snapshot");
        for verdict in [VerificationVerdict::Rejected, VerificationVerdict::Unknown] {
            let error = store
                .commit_verification(&verification_id, report(verdict))
                .await
                .expect_err("adverse verdict without immutable snapshot must fail closed");
            assert!(error.to_string().contains("immutable snapshot"));
        }
        assert_eq!(
            store
                .get_fact(&fact.fact_id)
                .await
                .expect("Fact remains challenged")
                .status
                .to_string(),
            "challenged"
        );
        let challenge_status: String =
            sqlx::query_scalar("SELECT status FROM fact_challenges WHERE verification_id=?")
                .bind(&verification_id)
                .fetch_one(store.pool())
                .await
                .expect("challenge status");
        assert_eq!(challenge_status, "open");

        store
            .create_verification_snapshot(
                &case.case_id,
                VerificationSnapshotDraft {
                    toolchain_hash: None,
                    extra_payload: json!({"governance":true}),
                },
            )
            .await
            .expect("snapshot");
        store
            .transition_verification_case(&case.case_id, 0, VerificationStage::Review, None)
            .await
            .expect("review");
        store
            .transition_verification_case(&case.case_id, 0, VerificationStage::Adjudication, None)
            .await
            .expect("adjudication");
        store
            .commit_verification(&verification_id, report(VerificationVerdict::Unknown))
            .await
            .expect("auditable unknown verdict");
        let inconclusive =
            sqlx::query("SELECT status,resolved_at FROM fact_challenges WHERE verification_id=?")
                .bind(&verification_id)
                .fetch_one(store.pool())
                .await
                .expect("terminal challenge");
        assert_eq!(
            inconclusive.try_get::<String, _>("status").expect("status"),
            "inconclusive"
        );
        assert!(
            inconclusive
                .try_get::<Option<String>, _>("resolved_at")
                .expect("resolved at")
                .is_some()
        );
    }

    #[tokio::test]
    async fn revoked_fact_is_terminal_for_suspend_and_reverification() {
        let (store, _temp) = store().await;
        let fixture = verification_fixture(
            &store,
            AcceptanceClass::FullyCertified,
            CandidateType::Theorem,
            Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
        )
        .await;
        let fact = store
            .commit_verification(
                &fixture.verification_id,
                report(VerificationVerdict::Accepted),
            )
            .await
            .expect("certified Fact")
            .fact
            .expect("Fact");
        store
            .govern_fact(
                &fact.fact_id,
                "revoke",
                "terminal audit failure",
                "operator",
                "terminal-revoke",
            )
            .await
            .expect("revoke");
        for (action, key) in [
            ("suspend", "suspend-after-revoke"),
            ("challenge", "challenge-after-revoke"),
            ("reverify", "reverify-after-revoke"),
        ] {
            let error = store
                .govern_fact(
                    &fact.fact_id,
                    action,
                    "must not revive terminal Fact",
                    "operator",
                    key,
                )
                .await
                .expect_err("revoked Fact is absorbing");
            assert!(error.to_string().contains("is terminal"));
        }
        assert_eq!(
            store
                .get_fact(&fact.fact_id)
                .await
                .expect("Fact")
                .status
                .to_string(),
            "revoked"
        );
    }

    #[tokio::test]
    async fn ordinary_duplicate_cannot_reactivate_non_active_fact() {
        for non_active_status in ["challenged", "suspended", "revoked"] {
            let (store, _temp) = store().await;
            let fixture = verification_fixture(
                &store,
                AcceptanceClass::FullyCertified,
                CandidateType::Theorem,
                Some((CheckStatus::Passed, report(VerificationVerdict::Accepted))),
            )
            .await;
            let fact = store
                .commit_verification(
                    &fixture.verification_id,
                    report(VerificationVerdict::Accepted),
                )
                .await
                .expect("initial Fact")
                .fact
                .expect("Fact");
            sqlx::query("UPDATE facts SET status=? WHERE fact_id=?")
                .bind(non_active_status)
                .bind(&fact.fact_id)
                .execute(store.pool())
                .await
                .expect("non-active Fact");
            sqlx::query("UPDATE goals SET status='open',solved_by_fact_id=NULL WHERE project_id=? AND goal_id=?")
                .bind(&fixture.project_id)
                .bind(&fixture.goal_id)
                .execute(store.pool())
                .await
                .expect("reopen Goal");
            sqlx::query("UPDATE proof_obligations SET status='open',satisfied_by_fact_id=NULL WHERE project_id=? AND goal_id=? AND source_kind='root_goal'")
                .bind(&fixture.project_id)
                .bind(&fixture.goal_id)
                .execute(store.pool())
                .await
                .expect("reopen root");
            let duplicate_verification_id = format!("ordinary-duplicate-{non_active_status}");
            let now = chrono::Utc::now().to_rfc3339();
            sqlx::query("UPDATE candidates SET status='verifying' WHERE candidate_id='obligation-candidate'")
                .execute(store.pool())
                .await
                .expect("candidate retry");
            sqlx::query("INSERT INTO verifications(verification_id,candidate_id,project_id,status,started_at) VALUES(?,'obligation-candidate',?,'verifying',?)")
                .bind(&duplicate_verification_id)
                .bind(&fixture.project_id)
                .bind(&now)
                .execute(store.pool())
                .await
                .expect("ordinary duplicate verification");
            prepare_fully_certified_case(&store, &duplicate_verification_id).await;
            let error = store
                .commit_verification(
                    &duplicate_verification_id,
                    report(VerificationVerdict::Accepted),
                )
                .await
                .expect_err("ordinary duplicate must not reactivate non-active Fact");
            assert!(
                error
                    .to_string()
                    .contains("ordinary verification cannot reuse")
                    || error.to_string().contains("is terminal")
            );
            assert_eq!(
                store
                    .get_fact(&fact.fact_id)
                    .await
                    .expect("Fact")
                    .status
                    .to_string(),
                non_active_status
            );
            let root = store
                .list_proof_obligations(&fixture.project_id)
                .await
                .expect("root")
                .into_iter()
                .find(|item| item.source_kind == "root_goal")
                .expect("root");
            assert_eq!(root.status, ProofObligationStatus::Open);
            assert!(root.satisfied_by_fact_id.is_none());
        }
    }
}
