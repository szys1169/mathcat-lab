use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
};

use chrono::Utc;
use research_domain::{
    AcceptanceClass, AlignmentRelation, AlignmentReview, BackendRun,
    CanonicalVerificationRequirements, CheckStatus, DomainEvent, Formalization, FormalizerOutput,
    ProofEdge, ProofHint, ProofNode, ProofNodeStatus, ProofSearch, ProofSearchBudget,
    SemanticContract, SemanticContractDraft, VerificationCase, VerificationCheck,
    VerificationEvidence, VerificationFinding, VerificationPackage, VerificationPolicy,
    VerificationProfile, VerificationReplay, VerificationSnapshot, VerificationStage,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, Transaction, sqlite::SqliteRow};

use crate::{
    SqliteStore, StorageError, StorageResult, append_event, bump_revision, entity, json_text,
    new_id, rows,
};

#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct VerificationCaseDraft {
    pub name: String,
    pub profile: VerificationProfile,
    pub required_acceptance: AcceptanceClass,
    pub required_checks: Vec<String>,
    pub independent_reviewer_count: u32,
    pub require_citation_review: bool,
    pub require_adversarial_review: bool,
    pub require_alignment_review: bool,
    pub require_fresh_replay: bool,
    pub max_attempts: u32,
    pub risk_score: f64,
    pub risk_reasons: Vec<String>,
}

fn validate_verification_case_draft(draft: &VerificationCaseDraft) -> StorageResult<()> {
    let requirements =
        CanonicalVerificationRequirements::from_required_checks(&draft.required_checks)
            .map_err(StorageError::InvalidTransition)?;
    let projected_required_checks = [
        draft.require_citation_review.then_some("citation_review"),
        draft
            .require_adversarial_review
            .then_some("adversarial_review"),
        draft.require_alignment_review.then_some("alignment_review"),
        draft.require_fresh_replay.then_some("fresh_replay"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    requirements
        .validate_projection(draft.independent_reviewer_count, &projected_required_checks)
        .map_err(StorageError::InvalidTransition)?;
    if matches!(
        draft.required_acceptance,
        AcceptanceClass::FormallyVerified | AcceptanceClass::FullyCertified
    ) && !requirements.requires_formal_pipeline()
    {
        return Err(StorageError::InvalidTransition(
            "formal acceptance requires the complete formal verification check bundle".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct VerificationSnapshotDraft {
    pub toolchain_hash: Option<String>,
    pub extra_payload: Value,
}

#[derive(Debug, Clone)]
pub struct CheckDraft {
    pub attempt_id: Option<String>,
    pub kind: String,
    pub status: CheckStatus,
    pub mandatory: bool,
    pub summary: String,
    pub details: Value,
}

#[derive(Debug, Clone)]
pub struct FindingDraft {
    pub check_id: Option<String>,
    pub reviewer_kind: String,
    pub severity: String,
    pub category: String,
    pub location: Option<String>,
    pub claim: String,
    pub rationale: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct EvidenceDraft {
    pub check_id: Option<String>,
    pub dimension: String,
    pub kind: String,
    pub uri: Option<String>,
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub struct BackendRunDraft {
    pub attempt_id: Option<String>,
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

#[derive(Debug, Clone)]
pub struct ProofNodeDraft {
    pub parent_node_id: Option<String>,
    pub state_id: Option<i64>,
    pub goal: String,
    pub local_context: Value,
    pub tactic: Option<String>,
    pub score: f64,
    pub status: ProofNodeStatus,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct FactGateContext {
    pub case_id: String,
    pub acceptance: AcceptanceClass,
    pub snapshot_hash: String,
    pub package_id: Option<String>,
    pub replay_id: Option<String>,
}

impl SqliteStore {
    pub async fn ensure_verification_case(
        &self,
        verification_id: &str,
        draft: VerificationCaseDraft,
    ) -> StorageResult<(VerificationCase, Option<DomainEvent>)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "ensure_verification_case",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        if let Some(row) = sqlx::query("SELECT * FROM verification_cases WHERE verification_id=?")
            .bind(verification_id)
            .fetch_optional(&mut *tx)
            .await?
        {
            let case = case_from_row(&row)?;
            tx.rollback().await?;
            return Ok((case, None));
        }
        validate_verification_case_draft(&draft)?;
        let verification = sqlx::query(
            "SELECT v.project_id,v.candidate_id,v.status AS verification_status,\
                    p.status AS project_status \
             FROM verifications v JOIN projects p ON p.project_id=v.project_id \
             WHERE v.verification_id=?",
        )
        .bind(verification_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "verification",
            id: verification_id.into(),
        })?;
        let project_id: String = verification.try_get("project_id")?;
        let candidate_id: String = verification.try_get("candidate_id")?;
        let verification_status: String = verification.try_get("verification_status")?;
        let project_status: String = verification.try_get("project_status")?;
        let route_released =
            crate::verification_execution_is_released(&mut tx, verification_id).await?;
        if !route_released || verification_status != "verifying" {
            return Err(StorageError::LateSubmission(format!(
                "verification {verification_id} cannot create a case while verification is {verification_status} and project is {project_status}"
            )));
        }
        let policy_id = new_id("vpolicy");
        let case_id = new_id("vcase");
        let now = Utc::now();
        sqlx::query("INSERT INTO verification_policies(policy_id,project_id,name,profile,required_acceptance,required_checks_json,independent_reviewer_count,require_citation_review,require_adversarial_review,require_alignment_review,require_fresh_replay,max_attempts,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&policy_id).bind(&project_id).bind(&draft.name).bind(draft.profile.to_string())
            .bind(draft.required_acceptance.to_string()).bind(json_text(&draft.required_checks)?)
            .bind(i64::from(draft.independent_reviewer_count)).bind(draft.require_citation_review)
            .bind(draft.require_adversarial_review).bind(draft.require_alignment_review)
            .bind(draft.require_fresh_replay).bind(i64::from(draft.max_attempts))
            .bind(now.to_rfc3339()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO verification_cases(case_id,verification_id,candidate_id,project_id,policy_id,profile,required_acceptance,stage,risk_score,risk_reasons_json,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&case_id).bind(verification_id).bind(&candidate_id).bind(&project_id).bind(&policy_id)
            .bind(draft.profile.to_string()).bind(draft.required_acceptance.to_string())
            .bind(VerificationStage::Intake.to_string()).bind(draft.risk_score)
            .bind(json_text(&draft.risk_reasons)?).bind(now.to_rfc3339()).bind(now.to_rfc3339())
            .execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &project_id).await?;
        let event = append_event(&mut tx, &project_id, revision, "verification.case.created", entity("verification_case", &case_id), json!({"verification_id": verification_id, "candidate_id": candidate_id, "profile": draft.profile, "required_acceptance": draft.required_acceptance, "risk_score": draft.risk_score}), Some(entity("verification", verification_id))).await?;
        tx.commit().await?;
        Ok((
            VerificationCase {
                case_id,
                verification_id: verification_id.into(),
                candidate_id,
                project_id,
                policy_id,
                snapshot_id: None,
                profile: draft.profile,
                required_acceptance: draft.required_acceptance,
                achieved_acceptance: None,
                stage: VerificationStage::Intake,
                risk_score: draft.risk_score,
                risk_reasons: draft.risk_reasons,
                cancellation_epoch: 0,
                created_at: now,
                updated_at: now,
                completed_at: None,
            },
            Some(event),
        ))
    }

    pub async fn create_verification_snapshot(
        &self,
        case_id: &str,
        draft: VerificationSnapshotDraft,
    ) -> StorageResult<(VerificationSnapshot, DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "create_verification_snapshot",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        if sqlx::query("SELECT snapshot_id FROM verification_snapshots WHERE case_id=?")
            .bind(case_id)
            .fetch_optional(&mut *tx)
            .await?
            .is_some()
        {
            return Err(StorageError::InvalidTransition(format!(
                "verification case {case_id} already has an immutable snapshot"
            )));
        }
        let row = sqlx::query("SELECT vc.*,c.submission_json,p.problem_contract_json,p.revision FROM verification_cases vc JOIN candidates c ON c.candidate_id=vc.candidate_id JOIN projects p ON p.project_id=vc.project_id WHERE vc.case_id=?")
            .bind(case_id).fetch_optional(&mut *tx).await?
            .ok_or_else(|| StorageError::NotFound { kind: "verification_case", id: case_id.into() })?;
        let case = case_from_row(&row)?;
        ensure_case_progression_allowed(&mut tx, &case).await?;
        let stage: String = row.try_get("stage")?;
        if stage != "intake" && stage != "snapshotting" {
            return Err(StorageError::InvalidTransition(format!(
                "cannot snapshot case {case_id} while {stage}"
            )));
        }
        let project_id: String = row.try_get("project_id")?;
        let candidate_id: String = row.try_get("candidate_id")?;
        let submission_json: String = row.try_get("submission_json")?;
        let candidate_hash = sha256(submission_json.as_bytes());
        let submission: research_domain::CandidateSubmission =
            serde_json::from_str(&submission_json)?;
        let contract: research_domain::ProblemContract =
            serde_json::from_str(row.try_get("problem_contract_json")?)?;
        let project_revision: i64 = row.try_get("revision")?;

        let mut dependency_hashes = BTreeMap::new();
        let mut dependency_assurances = BTreeMap::new();
        let mut dependencies = Vec::new();
        let mut visited_dependencies = BTreeSet::new();
        let mut pending_dependencies = submission
            .dependency_fact_ids
            .iter()
            .cloned()
            .map(|fact_id| (fact_id, Vec::<String>::new()))
            .collect::<Vec<_>>();
        while let Some((fact_id, path)) = pending_dependencies.pop() {
            if path.contains(&fact_id) {
                return Err(StorageError::DependencyCycle);
            }
            if !visited_dependencies.insert(fact_id.clone()) {
                continue;
            }
            let fact_row = sqlx::query("SELECT f.* FROM facts f WHERE f.fact_id=? AND (f.project_id=? OR EXISTS (SELECT 1 FROM project_fact_imports i WHERE i.target_project_id=? AND i.source_fact_id=f.fact_id AND i.status='active'))")
                .bind(&fact_id)
                .bind(&project_id)
                .bind(&project_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StorageError::InvalidDependency(fact_id.clone()))?;
            let fact = rows::fact(&fact_row)?;
            if fact.status != research_domain::FactStatus::ActiveFact {
                return Err(StorageError::InvalidDependency(fact_id.clone()));
            }
            dependency_hashes.insert(fact_id.clone(), fact.content_hash.clone());
            let assurance = sqlx::query("SELECT assurance_id,acceptance_class,snapshot_hash,package_id,replay_id,status,created_at FROM fact_assurances WHERE fact_id=? AND status='active' ORDER BY created_at DESC LIMIT 1")
                .bind(&fact_id).fetch_optional(&mut *tx).await?
                .ok_or_else(|| StorageError::InvalidTransition(format!("dependency fact {fact_id} has no active assurance")))?;
            dependency_assurances.insert(
                fact_id.clone(),
                json!({
                    "assurance_id":assurance.try_get::<String,_>("assurance_id")?,
                    "acceptance_class":assurance.try_get::<String,_>("acceptance_class")?,
                    "snapshot_hash":assurance.try_get::<String,_>("snapshot_hash")?,
                    "package_id":assurance.try_get::<Option<String>,_>("package_id")?,
                    "replay_id":assurance.try_get::<Option<String>,_>("replay_id")?,
                    "status":assurance.try_get::<String,_>("status")?,
                    "created_at":assurance.try_get::<String,_>("created_at")?,
                }),
            );
            let mut next_path = path;
            next_path.push(fact_id.clone());
            pending_dependencies.extend(
                fact.dependency_fact_ids
                    .iter()
                    .cloned()
                    .map(|dependency_id| (dependency_id, next_path.clone())),
            );
            dependencies.push(fact);
        }
        let mut source_hashes = BTreeMap::new();
        let mut sources = Vec::new();
        for source_id in &submission.external_source_ids {
            let source_row =
                sqlx::query("SELECT * FROM sources WHERE project_id=? AND source_id=?")
                    .bind(&project_id)
                    .bind(source_id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .ok_or_else(|| StorageError::NotFound {
                        kind: "source",
                        id: source_id.clone(),
                    })?;
            let source = rows::source(&source_row)?;
            if !crate::source_ingestion::status_can_support_candidate(&source.status) {
                return Err(StorageError::InvalidDependency(format!(
                    "source {source_id} has non-evidence status {}",
                    source.status
                )));
            }
            let hash = sha256(&serde_json::to_vec(&source)?);
            source_hashes.insert(source_id.clone(), hash);
            sources.push(source);
        }
        let policy_row = sqlx::query("SELECT * FROM verification_policies WHERE policy_id=?")
            .bind(row.try_get::<String, _>("policy_id")?)
            .fetch_one(&mut *tx)
            .await?;
        let policy = policy_from_row(&policy_row)?;
        let policy_hash = sha256(&serde_json::to_vec(&policy)?);
        let payload = json!({
            "candidate_id": candidate_id,
            "submission": submission,
            "problem_contract": contract,
            "dependencies": dependencies,
            "dependency_assurances": dependency_assurances,
            "sources": sources,
            "policy": policy,
            "extra": draft.extra_payload,
        });
        let canonical = json!({
            "candidate_hash": candidate_hash,
            "contract_version": contract.version,
            "dependency_hashes": dependency_hashes,
            "source_hashes": source_hashes,
            "policy_hash": policy_hash,
            "toolchain_hash": draft.toolchain_hash,
            "payload": payload,
        });
        let content_hash = sha256(&serde_json::to_vec(&canonical)?);
        let snapshot_id = format!("vsnapshot_{}", &content_hash[..24]);
        let now = Utc::now();
        sqlx::query("INSERT INTO verification_snapshots(snapshot_id,case_id,project_id,candidate_hash,project_revision,contract_version,dependency_hashes_json,source_hashes_json,policy_hash,toolchain_hash,content_hash,payload_json,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&snapshot_id).bind(case_id).bind(&project_id).bind(&candidate_hash).bind(project_revision)
            .bind(contract.version).bind(json_text(&dependency_hashes)?).bind(json_text(&source_hashes)?)
            .bind(&policy_hash).bind(&draft.toolchain_hash).bind(&content_hash).bind(json_text(&payload)?)
            .bind(now.to_rfc3339()).execute(&mut *tx).await?;
        sqlx::query("UPDATE verification_cases SET snapshot_id=?,stage='precheck',updated_at=? WHERE case_id=?")
            .bind(&snapshot_id).bind(now.to_rfc3339()).bind(case_id).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &project_id).await?;
        let event = append_event(&mut tx, &project_id, revision, "verification.snapshot.created", entity("verification_snapshot", &snapshot_id), json!({"case_id": case_id, "content_hash": content_hash, "project_revision": project_revision}), Some(entity("verification_case", case_id))).await?;
        tx.commit().await?;
        Ok((
            VerificationSnapshot {
                snapshot_id,
                case_id: case_id.into(),
                project_id,
                candidate_hash,
                project_revision,
                contract_version: contract.version,
                dependency_hashes,
                source_hashes,
                policy_hash,
                toolchain_hash: draft.toolchain_hash,
                content_hash,
                payload,
                created_at: now,
            },
            event,
        ))
    }

    pub async fn record_verification_check(
        &self,
        case_id: &str,
        draft: CheckDraft,
    ) -> StorageResult<(VerificationCheck, DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "record_verification_check",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let case = case_in_tx(&mut tx, case_id).await?;
        ensure_case_progression_allowed(&mut tx, &case).await?;
        let check_id = new_id("vcheck");
        let now = Utc::now();
        let completed_at = if matches!(draft.status, CheckStatus::Queued | CheckStatus::Running) {
            None
        } else {
            Some(now)
        };
        sqlx::query("INSERT INTO verification_checks(check_id,case_id,attempt_id,kind,status,mandatory,summary,details_json,created_at,completed_at) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(&check_id).bind(case_id).bind(&draft.attempt_id).bind(&draft.kind).bind(draft.status.to_string())
            .bind(draft.mandatory).bind(&draft.summary).bind(json_text(&draft.details)?)
            .bind(now.to_rfc3339()).bind(completed_at.map(|value| value.to_rfc3339())).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &case.project_id).await?;
        let event = append_event(&mut tx, &case.project_id, revision, "verification.check.completed", entity("verification_check", &check_id), json!({"case_id": case_id, "kind": draft.kind, "status": draft.status, "mandatory": draft.mandatory}), Some(entity("verification_case", case_id))).await?;
        tx.commit().await?;
        Ok((
            VerificationCheck {
                check_id,
                case_id: case_id.into(),
                attempt_id: draft.attempt_id,
                kind: draft.kind,
                status: draft.status,
                mandatory: draft.mandatory,
                summary: draft.summary,
                details: draft.details,
                created_at: now,
                completed_at,
            },
            event,
        ))
    }

    pub async fn record_verification_finding(
        &self,
        case_id: &str,
        draft: FindingDraft,
    ) -> StorageResult<(VerificationFinding, DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "record_verification_finding",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let case = case_in_tx(&mut tx, case_id).await?;
        ensure_case_progression_allowed(&mut tx, &case).await?;
        let finding_id = new_id("vfinding");
        let now = Utc::now();
        sqlx::query("INSERT INTO verification_findings(finding_id,case_id,check_id,reviewer_kind,severity,category,location,claim,rationale,status,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&finding_id).bind(case_id).bind(&draft.check_id).bind(&draft.reviewer_kind).bind(&draft.severity)
            .bind(&draft.category).bind(&draft.location).bind(&draft.claim).bind(&draft.rationale).bind(&draft.status)
            .bind(now.to_rfc3339()).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &case.project_id).await?;
        let event = append_event(&mut tx, &case.project_id, revision, "verification.finding.recorded", entity("verification_finding", &finding_id), json!({"case_id": case_id, "reviewer_kind": draft.reviewer_kind, "severity": draft.severity, "category": draft.category}), Some(entity("verification_case", case_id))).await?;
        tx.commit().await?;
        Ok((
            VerificationFinding {
                finding_id,
                case_id: case_id.into(),
                check_id: draft.check_id,
                reviewer_kind: draft.reviewer_kind,
                severity: draft.severity,
                category: draft.category,
                location: draft.location,
                claim: draft.claim,
                rationale: draft.rationale,
                status: draft.status,
                created_at: now,
            },
            event,
        ))
    }

    pub async fn record_verification_evidence(
        &self,
        case_id: &str,
        draft: EvidenceDraft,
    ) -> StorageResult<(VerificationEvidence, DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "record_verification_evidence",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let case = case_in_tx(&mut tx, case_id).await?;
        ensure_case_progression_allowed(&mut tx, &case).await?;
        let evidence_id = new_id("vevidence");
        let now = Utc::now();
        let sha256 = sha256(&serde_json::to_vec(&draft.payload)?);
        sqlx::query("INSERT INTO verification_evidence(evidence_id,case_id,check_id,dimension,kind,uri,sha256,payload_json,created_at) VALUES(?,?,?,?,?,?,?,?,?)")
            .bind(&evidence_id).bind(case_id).bind(&draft.check_id).bind(&draft.dimension).bind(&draft.kind)
            .bind(&draft.uri).bind(&sha256).bind(json_text(&draft.payload)?).bind(now.to_rfc3339())
            .execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &case.project_id).await?;
        let event = append_event(&mut tx, &case.project_id, revision, "verification.evidence.recorded", entity("verification_evidence", &evidence_id), json!({"case_id": case_id, "dimension": draft.dimension, "kind": draft.kind, "sha256": sha256}), Some(entity("verification_case", case_id))).await?;
        tx.commit().await?;
        Ok((
            VerificationEvidence {
                evidence_id,
                case_id: case_id.into(),
                check_id: draft.check_id,
                dimension: draft.dimension,
                kind: draft.kind,
                uri: draft.uri,
                sha256,
                payload: draft.payload,
                created_at: now,
            },
            event,
        ))
    }

    pub async fn transition_verification_case(
        &self,
        case_id: &str,
        expected_epoch: i64,
        next: VerificationStage,
        achieved: Option<AcceptanceClass>,
    ) -> StorageResult<DomainEvent> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "transition_verification_case",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let case = case_in_tx(&mut tx, case_id).await?;
        ensure_case_progression_allowed(&mut tx, &case).await?;
        if case.cancellation_epoch != expected_epoch {
            return Err(StorageError::LateSubmission(format!(
                "verification case {case_id} epoch expected {expected_epoch}, actual {}",
                case.cancellation_epoch
            )));
        }
        if !valid_transition(case.stage, next) {
            return Err(StorageError::InvalidTransition(format!(
                "verification case {case_id}: {} -> {next}",
                case.stage
            )));
        }
        if next == VerificationStage::CommitReady {
            ensure_gate_checks(&mut tx, &case).await?;
            let achieved = achieved.ok_or_else(|| {
                StorageError::InvalidTransition(
                    "commit_ready requires an achieved acceptance class".into(),
                )
            })?;
            if acceptance_rank(achieved) < acceptance_rank(case.required_acceptance) {
                return Err(StorageError::InvalidTransition(format!(
                    "achieved {achieved} is below required {}",
                    case.required_acceptance
                )));
            }
        }
        let now = Utc::now();
        let terminal = matches!(
            next,
            VerificationStage::Committed
                | VerificationStage::Rejected
                | VerificationStage::Unknown
                | VerificationStage::Failed
                | VerificationStage::Cancelled
        );
        sqlx::query("UPDATE verification_cases SET stage=?,achieved_acceptance=COALESCE(?,achieved_acceptance),updated_at=?,completed_at=CASE WHEN ? THEN ? ELSE completed_at END WHERE case_id=?")
            .bind(next.to_string()).bind(achieved.map(|value| value.to_string())).bind(now.to_rfc3339())
            .bind(terminal).bind(now.to_rfc3339()).bind(case_id).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &case.project_id).await?;
        let event = append_event(
            &mut tx,
            &case.project_id,
            revision,
            "verification.case.transitioned",
            entity("verification_case", case_id),
            json!({"from": case.stage, "to": next, "achieved_acceptance": achieved}),
            None,
        )
        .await?;
        tx.commit().await?;
        Ok(event)
    }

    pub async fn get_verification_case(&self, case_id: &str) -> StorageResult<VerificationCase> {
        let row = sqlx::query("SELECT * FROM verification_cases WHERE case_id=?")
            .bind(case_id)
            .fetch_optional(self.pool())
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "verification_case",
                id: case_id.into(),
            })?;
        case_from_row(&row)
    }

    pub async fn verification_case_for_verification(
        &self,
        verification_id: &str,
    ) -> StorageResult<VerificationCase> {
        let row = sqlx::query("SELECT * FROM verification_cases WHERE verification_id=?")
            .bind(verification_id)
            .fetch_optional(self.pool())
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "verification_case",
                id: verification_id.into(),
            })?;
        case_from_row(&row)
    }

    pub async fn get_verification_snapshot(
        &self,
        case_id: &str,
    ) -> StorageResult<VerificationSnapshot> {
        let row = sqlx::query("SELECT * FROM verification_snapshots WHERE case_id=?")
            .bind(case_id)
            .fetch_optional(self.pool())
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "verification_snapshot",
                id: case_id.into(),
            })?;
        snapshot_from_row(&row)
    }

    pub async fn list_verification_checks(
        &self,
        case_id: &str,
    ) -> StorageResult<Vec<VerificationCheck>> {
        let rows = sqlx::query(
            "SELECT * FROM verification_checks WHERE case_id=? ORDER BY created_at,check_id",
        )
        .bind(case_id)
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(check_from_row).collect()
    }

    pub async fn list_verification_findings(
        &self,
        case_id: &str,
    ) -> StorageResult<Vec<VerificationFinding>> {
        let rows = sqlx::query(
            "SELECT * FROM verification_findings WHERE case_id=? ORDER BY created_at,finding_id",
        )
        .bind(case_id)
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(finding_from_row).collect()
    }

    pub async fn list_verification_evidence(
        &self,
        case_id: &str,
    ) -> StorageResult<Vec<VerificationEvidence>> {
        let rows = sqlx::query(
            "SELECT * FROM verification_evidence WHERE case_id=? ORDER BY created_at,evidence_id",
        )
        .bind(case_id)
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(evidence_from_row).collect()
    }

    pub async fn verification_policy(&self, case_id: &str) -> StorageResult<VerificationPolicy> {
        let row = sqlx::query("SELECT vp.* FROM verification_policies vp JOIN verification_cases vc ON vc.policy_id=vp.policy_id WHERE vc.case_id=?")
            .bind(case_id).fetch_optional(self.pool()).await?
            .ok_or_else(|| StorageError::NotFound { kind: "verification_policy", id: case_id.into() })?;
        policy_from_row(&row)
    }

    pub async fn store_semantic_contract(
        &self,
        case_id: &str,
        natural_language_statement: &str,
        draft: SemanticContractDraft,
    ) -> StorageResult<(SemanticContract, DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "store_semantic_contract",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let case = case_in_tx(&mut tx, case_id).await?;
        ensure_case_progression_allowed(&mut tx, &case).await?;
        let version: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version),0)+1 FROM semantic_contracts WHERE case_id=?",
        )
        .bind(case_id)
        .fetch_one(&mut *tx)
        .await?;
        let definitions = draft
            .definitions
            .iter()
            .map(|definition| (definition.name.clone(), definition.description.clone()))
            .collect::<BTreeMap<_, _>>();
        let canonical = json!({
            "natural_language_statement": natural_language_statement,
            "variables": draft.variables,
            "assumptions": draft.assumptions,
            "conclusion": draft.conclusion,
            "definitions": definitions,
            "boundary_conditions": draft.boundary_conditions,
            "ambiguity_notes": draft.ambiguity_notes,
        });
        let content_hash = sha256(&serde_json::to_vec(&canonical)?);
        let contract_id = new_id("semantic");
        let now = Utc::now();
        sqlx::query("INSERT INTO semantic_contracts(contract_id,case_id,version,natural_language_statement,variables_json,assumptions_json,conclusion,definitions_json,boundary_conditions_json,ambiguity_notes_json,content_hash,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&contract_id).bind(case_id).bind(version).bind(natural_language_statement)
            .bind(json_text(&draft.variables)?).bind(json_text(&draft.assumptions)?).bind(&draft.conclusion)
            .bind(json_text(&definitions)?).bind(json_text(&draft.boundary_conditions)?)
            .bind(json_text(&draft.ambiguity_notes)?).bind(&content_hash).bind(now.to_rfc3339())
            .execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &case.project_id).await?;
        let event = append_event(&mut tx, &case.project_id, revision, "verification.semantic_contract.created", entity("semantic_contract", &contract_id), json!({"case_id": case_id, "version": version, "content_hash": content_hash, "ambiguity_count": draft.ambiguity_notes.len()}), Some(entity("verification_case", case_id))).await?;
        tx.commit().await?;
        Ok((
            SemanticContract {
                contract_id,
                case_id: case_id.into(),
                version,
                natural_language_statement: natural_language_statement.into(),
                variables: draft.variables,
                assumptions: draft.assumptions,
                conclusion: draft.conclusion,
                definitions,
                boundary_conditions: draft.boundary_conditions,
                ambiguity_notes: draft.ambiguity_notes,
                content_hash,
                created_at: now,
            },
            event,
        ))
    }

    pub async fn store_formalization(
        &self,
        case_id: &str,
        semantic_contract_id: &str,
        output: &FormalizerOutput,
    ) -> StorageResult<(Formalization, DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "store_formalization",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let case = case_in_tx(&mut tx, case_id).await?;
        ensure_case_progression_allowed(&mut tx, &case).await?;
        let owns_contract: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM semantic_contracts WHERE case_id=? AND contract_id=?",
        )
        .bind(case_id)
        .bind(semantic_contract_id)
        .fetch_one(&mut *tx)
        .await?;
        if owns_contract != 1 {
            return Err(StorageError::InvalidTransition(
                "formalization semantic contract does not belong to case".into(),
            ));
        }
        let formalization_id = new_id("formalization");
        let source_hash = sha256(output.lean_source.as_bytes());
        let now = Utc::now();
        sqlx::query("INSERT INTO formalizations(formalization_id,case_id,semantic_contract_id,theorem_name,lean_statement,lean_source,mapping_json,source_hash,status,created_at) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(&formalization_id).bind(case_id).bind(semantic_contract_id).bind(&output.theorem_name)
            .bind(&output.lean_statement).bind(&output.lean_source).bind(json_text(&output.mapping)?)
            .bind(&source_hash).bind("prepared").bind(now.to_rfc3339()).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &case.project_id).await?;
        let event = append_event(&mut tx, &case.project_id, revision, "verification.formalization.created", entity("formalization", &formalization_id), json!({"case_id": case_id, "semantic_contract_id": semantic_contract_id, "theorem_name": output.theorem_name, "source_hash": source_hash}), Some(entity("verification_case", case_id))).await?;
        tx.commit().await?;
        Ok((
            Formalization {
                formalization_id,
                case_id: case_id.into(),
                semantic_contract_id: semantic_contract_id.into(),
                theorem_name: output.theorem_name.clone(),
                lean_statement: output.lean_statement.clone(),
                lean_source: output.lean_source.clone(),
                mapping: serde_json::to_value(&output.mapping)?,
                source_hash,
                status: "prepared".into(),
                created_at: now,
            },
            event,
        ))
    }

    pub async fn store_alignment_review(
        &self,
        case_id: &str,
        formalization_id: &str,
        reviewer_kind: &str,
        output: research_domain::AlignmentReviewerOutput,
    ) -> StorageResult<(AlignmentReview, DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "store_alignment_review",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let case = case_in_tx(&mut tx, case_id).await?;
        ensure_case_progression_allowed(&mut tx, &case).await?;
        let alignment_id = new_id("alignment");
        let now = Utc::now();
        sqlx::query("INSERT INTO alignment_reviews(alignment_id,case_id,formalization_id,relation,reviewer_kind,rationale,missing_assumptions_json,extra_assumptions_json,confidence,created_at) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(&alignment_id).bind(case_id).bind(formalization_id).bind(output.relation.to_string()).bind(reviewer_kind)
            .bind(&output.rationale).bind(json_text(&output.missing_assumptions)?).bind(json_text(&output.extra_assumptions)?)
            .bind(output.confidence).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &case.project_id).await?;
        let event = append_event(&mut tx, &case.project_id, revision, "verification.alignment.reviewed", entity("alignment_review", &alignment_id), json!({"case_id": case_id, "formalization_id": formalization_id, "relation": output.relation, "confidence": output.confidence}), Some(entity("verification_case", case_id))).await?;
        tx.commit().await?;
        Ok((
            AlignmentReview {
                alignment_id,
                case_id: case_id.into(),
                formalization_id: formalization_id.into(),
                relation: output.relation,
                reviewer_kind: reviewer_kind.into(),
                rationale: output.rationale,
                missing_assumptions: output.missing_assumptions,
                extra_assumptions: output.extra_assumptions,
                confidence: output.confidence,
                created_at: now,
            },
            event,
        ))
    }

    pub async fn store_backend_run(
        &self,
        case_id: &str,
        draft: BackendRunDraft,
    ) -> StorageResult<(BackendRun, DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "store_verification_backend_run",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let case = case_in_tx(&mut tx, case_id).await?;
        ensure_case_progression_allowed(&mut tx, &case).await?;
        let backend_run_id = new_id("backendrun");
        let now = Utc::now();
        sqlx::query("INSERT INTO backend_runs(backend_run_id,case_id,attempt_id,backend,backend_version,status,command_json,working_directory,exit_code,stdout,stderr,diagnostics_json,axioms_json,elapsed_ms,input_hash,output_hash,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&backend_run_id).bind(case_id).bind(&draft.attempt_id).bind(&draft.backend).bind(&draft.backend_version)
            .bind(draft.status.to_string()).bind(json_text(&draft.command)?).bind(&draft.working_directory).bind(draft.exit_code)
            .bind(&draft.stdout).bind(&draft.stderr).bind(json_text(&draft.diagnostics)?).bind(json_text(&draft.axioms)?)
            .bind(draft.elapsed_ms).bind(&draft.input_hash).bind(&draft.output_hash).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &case.project_id).await?;
        let event = append_event(&mut tx, &case.project_id, revision, "verification.backend.completed", entity("backend_run", &backend_run_id), json!({"case_id": case_id, "backend": draft.backend, "status": draft.status, "exit_code": draft.exit_code, "elapsed_ms": draft.elapsed_ms}), Some(entity("verification_case", case_id))).await?;
        tx.commit().await?;
        Ok((
            BackendRun {
                backend_run_id,
                case_id: case_id.into(),
                attempt_id: draft.attempt_id,
                backend: draft.backend,
                backend_version: draft.backend_version,
                status: draft.status,
                command: draft.command,
                working_directory: draft.working_directory,
                exit_code: draft.exit_code,
                stdout: draft.stdout,
                stderr: draft.stderr,
                diagnostics: draft.diagnostics,
                axioms: draft.axioms,
                elapsed_ms: draft.elapsed_ms,
                input_hash: draft.input_hash,
                output_hash: draft.output_hash,
                created_at: now,
            },
            event,
        ))
    }

    pub async fn store_verification_package(
        &self,
        case_id: &str,
        manifest: Value,
        storage_path: &str,
    ) -> StorageResult<(VerificationPackage, DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "store_verification_package",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let case = case_in_tx(&mut tx, case_id).await?;
        ensure_case_progression_allowed(&mut tx, &case).await?;
        let manifest_hash = sha256(&serde_json::to_vec(&manifest)?);
        let package_id = format!("vpackage_{}", &manifest_hash[..24]);
        let now = Utc::now();
        sqlx::query("INSERT INTO verification_packages(package_id,case_id,manifest_json,manifest_hash,storage_path,created_at) VALUES(?,?,?,?,?,?)")
            .bind(&package_id).bind(case_id).bind(json_text(&manifest)?).bind(&manifest_hash).bind(storage_path).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &case.project_id).await?;
        let event = append_event(
            &mut tx,
            &case.project_id,
            revision,
            "verification.package.created",
            entity("verification_package", &package_id),
            json!({"case_id": case_id, "manifest_hash": manifest_hash}),
            Some(entity("verification_case", case_id)),
        )
        .await?;
        tx.commit().await?;
        Ok((
            VerificationPackage {
                package_id,
                case_id: case_id.into(),
                manifest,
                manifest_hash,
                storage_path: storage_path.into(),
                created_at: now,
            },
            event,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn store_verification_replay(
        &self,
        case_id: &str,
        package_id: &str,
        status: CheckStatus,
        manifest_hash: &str,
        observed_hash: &str,
        backend_run_id: Option<&str>,
        summary: &str,
    ) -> StorageResult<(VerificationReplay, DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "store_verification_replay",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let case = case_in_tx(&mut tx, case_id).await?;
        ensure_case_progression_allowed(&mut tx, &case).await?;
        let replay_id = new_id("vreplay");
        let now = Utc::now();
        sqlx::query("INSERT INTO verification_replays(replay_id,case_id,package_id,status,fresh_process,manifest_hash,observed_hash,backend_run_id,summary,created_at) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(&replay_id).bind(case_id).bind(package_id).bind(status.to_string()).bind(true)
            .bind(manifest_hash).bind(observed_hash).bind(backend_run_id).bind(summary).bind(now.to_rfc3339())
            .execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &case.project_id).await?;
        let event = append_event(&mut tx, &case.project_id, revision, "verification.replay.completed", entity("verification_replay", &replay_id), json!({"case_id": case_id, "package_id": package_id, "status": status, "fresh_process": true}), Some(entity("verification_package", package_id))).await?;
        tx.commit().await?;
        Ok((
            VerificationReplay {
                replay_id,
                case_id: case_id.into(),
                package_id: package_id.into(),
                status,
                fresh_process: true,
                manifest_hash: manifest_hash.into(),
                observed_hash: observed_hash.into(),
                backend_run_id: backend_run_id.map(str::to_owned),
                summary: summary.into(),
                created_at: now,
            },
            event,
        ))
    }

    pub async fn get_semantic_contract(&self, case_id: &str) -> StorageResult<SemanticContract> {
        let row = sqlx::query(
            "SELECT * FROM semantic_contracts WHERE case_id=? ORDER BY version DESC LIMIT 1",
        )
        .bind(case_id)
        .fetch_optional(self.pool())
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "semantic_contract",
            id: case_id.into(),
        })?;
        semantic_from_row(&row)
    }

    pub async fn get_formalization(&self, case_id: &str) -> StorageResult<Formalization> {
        let row = sqlx::query(
            "SELECT * FROM formalizations WHERE case_id=? ORDER BY created_at DESC LIMIT 1",
        )
        .bind(case_id)
        .fetch_optional(self.pool())
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "formalization",
            id: case_id.into(),
        })?;
        formalization_from_row(&row)
    }

    pub async fn get_formalization_by_id(
        &self,
        formalization_id: &str,
    ) -> StorageResult<Formalization> {
        let row = sqlx::query("SELECT * FROM formalizations WHERE formalization_id=?")
            .bind(formalization_id)
            .fetch_optional(self.pool())
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "formalization",
                id: formalization_id.into(),
            })?;
        formalization_from_row(&row)
    }

    pub async fn list_alignment_reviews(
        &self,
        case_id: &str,
    ) -> StorageResult<Vec<AlignmentReview>> {
        let rows =
            sqlx::query("SELECT * FROM alignment_reviews WHERE case_id=? ORDER BY created_at")
                .bind(case_id)
                .fetch_all(self.pool())
                .await?;
        rows.iter().map(alignment_from_row).collect()
    }

    pub async fn list_backend_runs(&self, case_id: &str) -> StorageResult<Vec<BackendRun>> {
        let rows = sqlx::query("SELECT * FROM backend_runs WHERE case_id=? ORDER BY created_at")
            .bind(case_id)
            .fetch_all(self.pool())
            .await?;
        rows.iter().map(backend_run_from_row).collect()
    }

    pub async fn get_verification_package(
        &self,
        case_id: &str,
    ) -> StorageResult<VerificationPackage> {
        let row = sqlx::query(
            "SELECT * FROM verification_packages WHERE case_id=? ORDER BY created_at DESC LIMIT 1",
        )
        .bind(case_id)
        .fetch_optional(self.pool())
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "verification_package",
            id: case_id.into(),
        })?;
        package_from_row(&row)
    }

    pub async fn list_verification_replays(
        &self,
        case_id: &str,
    ) -> StorageResult<Vec<VerificationReplay>> {
        let rows =
            sqlx::query("SELECT * FROM verification_replays WHERE case_id=? ORDER BY created_at")
                .bind(case_id)
                .fetch_all(self.pool())
                .await?;
        rows.iter().map(replay_from_row).collect()
    }

    pub async fn create_proof_search(
        &self,
        case_id: &str,
        formalization_id: &str,
        strategy: &str,
        budget: ProofSearchBudget,
        root: ProofNodeDraft,
    ) -> StorageResult<(ProofSearch, ProofNode, Vec<DomainEvent>)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "create_proof_search",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let case = case_in_tx(&mut tx, case_id).await?;
        ensure_case_progression_allowed(&mut tx, &case).await?;
        let owns_formalization: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM formalizations WHERE case_id=? AND formalization_id=?",
        )
        .bind(case_id)
        .bind(formalization_id)
        .fetch_one(&mut *tx)
        .await?;
        if owns_formalization != 1 {
            return Err(StorageError::InvalidTransition(
                "proof search formalization does not belong to case".into(),
            ));
        }
        let search_id = new_id("psearch");
        let root_node_id = new_id("pnode");
        let now = Utc::now();
        sqlx::query("INSERT INTO proof_searches(search_id,case_id,formalization_id,status,strategy,budget_json,nodes_created,nodes_expanded,model_calls,cancellation_epoch,root_node_id,started_at) VALUES(?,?,?,?,?,?,1,0,0,0,?,?)")
            .bind(&search_id).bind(case_id).bind(formalization_id)
            .bind(CheckStatus::Running.to_string()).bind(strategy).bind(json_text(&budget)?)
            .bind(&root_node_id).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO proof_nodes(node_id,search_id,parent_node_id,depth,state_id,goal,local_context_json,tactic,score,status,diagnostic,cancellation_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&root_node_id).bind(&search_id).bind(Option::<String>::None).bind(0_i64)
            .bind(root.state_id).bind(&root.goal).bind(json_text(&root.local_context)?)
            .bind(Option::<String>::None).bind(root.score).bind(root.status.to_string())
            .bind(&root.diagnostic).bind(0_i64).bind(now.to_rfc3339()).bind(now.to_rfc3339())
            .execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &case.project_id).await?;
        let search_event = append_event(&mut tx, &case.project_id, revision, "proof_search.started", entity("proof_search", &search_id), json!({"case_id": case_id, "formalization_id": formalization_id, "strategy": strategy, "budget": budget}), Some(entity("verification_case", case_id))).await?;
        let revision = bump_revision(&mut tx, &case.project_id).await?;
        let node_event = append_event(
            &mut tx,
            &case.project_id,
            revision,
            "proof_node.created",
            entity("proof_node", &root_node_id),
            json!({"search_id": search_id, "depth": 0, "status": root.status, "goal": root.goal}),
            Some(entity("proof_search", &search_id)),
        )
        .await?;
        tx.commit().await?;
        Ok((
            ProofSearch {
                search_id: search_id.clone(),
                case_id: case_id.into(),
                formalization_id: formalization_id.into(),
                status: CheckStatus::Running,
                strategy: strategy.into(),
                budget,
                nodes_created: 1,
                nodes_expanded: 0,
                model_calls: 0,
                cancellation_epoch: 0,
                root_node_id: Some(root_node_id.clone()),
                solution_node_id: None,
                started_at: now,
                completed_at: None,
            },
            ProofNode {
                node_id: root_node_id,
                search_id,
                parent_node_id: None,
                depth: 0,
                state_id: root.state_id,
                goal: root.goal,
                local_context: root.local_context,
                tactic: None,
                score: root.score,
                status: root.status,
                diagnostic: root.diagnostic,
                cancellation_epoch: 0,
                created_at: now,
                updated_at: now,
            },
            vec![search_event, node_event],
        ))
    }

    pub async fn add_proof_node(
        &self,
        search_id: &str,
        expected_epoch: i64,
        draft: ProofNodeDraft,
    ) -> StorageResult<(ProofNode, Option<ProofEdge>, Vec<DomainEvent>)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "add_proof_node",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let search_row = sqlx::query("SELECT ps.*,vc.project_id FROM proof_searches ps JOIN verification_cases vc ON vc.case_id=ps.case_id WHERE ps.search_id=?")
            .bind(search_id).fetch_optional(&mut *tx).await?
            .ok_or_else(|| StorageError::NotFound { kind: "proof_search", id: search_id.into() })?;
        let status: String = search_row.try_get("status")?;
        let epoch: i64 = search_row.try_get("cancellation_epoch")?;
        let budget: ProofSearchBudget = rows::json(&search_row, "budget_json")?;
        let nodes_created: i64 = search_row.try_get("nodes_created")?;
        let project_id: String = search_row.try_get("project_id")?;
        if status != "running" || epoch != expected_epoch {
            return Err(StorageError::LateSubmission(format!(
                "proof search {search_id} is {status} at epoch {epoch}, submission epoch {expected_epoch}"
            )));
        }
        if nodes_created >= i64::from(budget.max_nodes) {
            return Err(StorageError::BudgetExhausted(format!(
                "proof search {search_id} reached max_nodes={} ",
                budget.max_nodes
            )));
        }
        let depth = if let Some(parent_id) = &draft.parent_node_id {
            let parent = sqlx::query("SELECT depth,status,cancellation_epoch FROM proof_nodes WHERE search_id=? AND node_id=?")
                .bind(search_id).bind(parent_id).fetch_optional(&mut *tx).await?
                .ok_or_else(|| StorageError::NotFound { kind: "proof_node", id: parent_id.clone() })?;
            let parent_status: String = parent.try_get("status")?;
            let parent_epoch: i64 = parent.try_get("cancellation_epoch")?;
            if !matches!(parent_status.as_str(), "open" | "running")
                || parent_epoch != expected_epoch
            {
                return Err(StorageError::LateSubmission(format!(
                    "parent node {parent_id} is {parent_status} at epoch {parent_epoch}"
                )));
            }
            let parent_depth: i64 = parent.try_get("depth")?;
            if parent_depth + 1 > i64::from(budget.max_depth) {
                return Err(StorageError::BudgetExhausted(format!(
                    "proof search {search_id} reached max_depth={} ",
                    budget.max_depth
                )));
            }
            parent_depth + 1
        } else {
            0
        };
        let node_id = new_id("pnode");
        let now = Utc::now();
        sqlx::query("INSERT INTO proof_nodes(node_id,search_id,parent_node_id,depth,state_id,goal,local_context_json,tactic,score,status,diagnostic,cancellation_epoch,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&node_id).bind(search_id).bind(&draft.parent_node_id).bind(depth).bind(draft.state_id)
            .bind(&draft.goal).bind(json_text(&draft.local_context)?).bind(&draft.tactic).bind(draft.score)
            .bind(draft.status.to_string()).bind(&draft.diagnostic).bind(expected_epoch)
            .bind(now.to_rfc3339()).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        sqlx::query("UPDATE proof_searches SET nodes_created=nodes_created+1 WHERE search_id=?")
            .bind(search_id)
            .execute(&mut *tx)
            .await?;
        let edge = if let (Some(parent_id), Some(tactic)) = (&draft.parent_node_id, &draft.tactic) {
            let edge_id = new_id("pedge");
            sqlx::query("INSERT INTO proof_edges(edge_id,search_id,source_node_id,target_node_id,tactic,created_at) VALUES(?,?,?,?,?,?)")
                .bind(&edge_id).bind(search_id).bind(parent_id).bind(&node_id).bind(tactic)
                .bind(now.to_rfc3339()).execute(&mut *tx).await?;
            Some(ProofEdge {
                edge_id,
                search_id: search_id.into(),
                source_node_id: parent_id.clone(),
                target_node_id: node_id.clone(),
                tactic: tactic.clone(),
                created_at: now,
            })
        } else {
            None
        };
        let revision = bump_revision(&mut tx, &project_id).await?;
        let event = append_event(&mut tx, &project_id, revision, "proof_node.created", entity("proof_node", &node_id), json!({"search_id": search_id, "parent_node_id": draft.parent_node_id, "depth": depth, "status": draft.status, "tactic": draft.tactic, "goal": draft.goal}), Some(entity("proof_search", search_id))).await?;
        tx.commit().await?;
        Ok((
            ProofNode {
                node_id,
                search_id: search_id.into(),
                parent_node_id: draft.parent_node_id,
                depth,
                state_id: draft.state_id,
                goal: draft.goal,
                local_context: draft.local_context,
                tactic: draft.tactic,
                score: draft.score,
                status: draft.status,
                diagnostic: draft.diagnostic,
                cancellation_epoch: expected_epoch,
                created_at: now,
                updated_at: now,
            },
            edge,
            vec![event],
        ))
    }

    pub async fn mark_proof_node_expanded(
        &self,
        search_id: &str,
        node_id: &str,
        expected_epoch: i64,
        status: ProofNodeStatus,
        diagnostic: Option<&str>,
    ) -> StorageResult<DomainEvent> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "mark_proof_node_expanded",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query("SELECT pn.status,pn.cancellation_epoch,vc.project_id FROM proof_nodes pn JOIN proof_searches ps ON ps.search_id=pn.search_id JOIN verification_cases vc ON vc.case_id=ps.case_id WHERE pn.search_id=? AND pn.node_id=? AND ps.status='running' AND ps.cancellation_epoch=?")
            .bind(search_id).bind(node_id).bind(expected_epoch).fetch_optional(&mut *tx).await?
            .ok_or_else(|| StorageError::LateSubmission(format!("stale proof-node update {node_id} at epoch {expected_epoch}")))?;
        let node_epoch: i64 = row.try_get("cancellation_epoch")?;
        let old_status: String = row.try_get("status")?;
        if node_epoch != expected_epoch || !matches!(old_status.as_str(), "open" | "running") {
            return Err(StorageError::LateSubmission(format!(
                "proof node {node_id} is {old_status} at epoch {node_epoch}"
            )));
        }
        let project_id: String = row.try_get("project_id")?;
        let now = Utc::now();
        sqlx::query("UPDATE proof_nodes SET status=?,diagnostic=?,updated_at=? WHERE node_id=?")
            .bind(status.to_string())
            .bind(diagnostic)
            .bind(now.to_rfc3339())
            .bind(node_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE proof_searches SET nodes_expanded=nodes_expanded+1 WHERE search_id=?")
            .bind(search_id)
            .execute(&mut *tx)
            .await?;
        let revision = bump_revision(&mut tx, &project_id).await?;
        let event = append_event(&mut tx, &project_id, revision, "proof_node.expanded", entity("proof_node", node_id), json!({"search_id": search_id, "status": status, "diagnostic": diagnostic, "epoch": expected_epoch}), Some(entity("proof_search", search_id))).await?;
        tx.commit().await?;
        Ok(event)
    }

    pub async fn add_proof_hint(
        &self,
        search_id: &str,
        node_id: Option<&str>,
        content: &str,
        requested_by: &str,
        idempotency_key: &str,
    ) -> StorageResult<(ProofHint, Option<DomainEvent>)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "add_proof_hint",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query("SELECT ps.nodes_expanded,ps.status,vc.project_id FROM proof_searches ps JOIN verification_cases vc ON vc.case_id=ps.case_id WHERE ps.search_id=?")
            .bind(search_id).fetch_optional(&mut *tx).await?
            .ok_or_else(|| StorageError::NotFound { kind: "proof_search", id: search_id.into() })?;
        let status: String = row.try_get("status")?;
        if status != "running" {
            return Err(StorageError::InvalidTransition(format!(
                "proof search {search_id} is {status}"
            )));
        }
        if let Some(node_id) = node_id {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM proof_nodes WHERE search_id=? AND node_id=?",
            )
            .bind(search_id)
            .bind(node_id)
            .fetch_one(&mut *tx)
            .await?;
            if count != 1 {
                return Err(StorageError::NotFound {
                    kind: "proof_node",
                    id: node_id.into(),
                });
            }
        }
        let expanded: i64 = row.try_get("nodes_expanded")?;
        let project_id: String = row.try_get("project_id")?;
        let request_hash =
            control_request_hash("add_hint", search_id, node_id, content, requested_by)?;
        if let Some(existing) = existing_control_result::<ProofHint>(
            &mut tx,
            &project_id,
            idempotency_key,
            &request_hash,
        )
        .await?
        {
            tx.rollback().await?;
            return Ok((existing, None));
        }
        reserve_control_command(
            &mut tx,
            &project_id,
            idempotency_key,
            "add_hint",
            search_id,
            &request_hash,
        )
        .await?;
        let effective_after_expansion = expanded + 1;
        let hint_id = new_id("phint");
        let now = Utc::now();
        sqlx::query("INSERT INTO proof_hints(hint_id,search_id,node_id,content,requested_by,effective_after_expansion,created_at) VALUES(?,?,?,?,?,?,?)")
            .bind(&hint_id).bind(search_id).bind(node_id).bind(content).bind(requested_by)
            .bind(effective_after_expansion).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &project_id).await?;
        let event = append_event(&mut tx, &project_id, revision, "proof_hint.added", entity("proof_hint", &hint_id), json!({"search_id": search_id, "node_id": node_id, "effective_after_expansion": effective_after_expansion}), Some(entity("proof_search", search_id))).await?;
        let hint = ProofHint {
            hint_id,
            search_id: search_id.into(),
            node_id: node_id.map(str::to_owned),
            content: content.into(),
            requested_by: requested_by.into(),
            effective_after_expansion,
            consumed_at: None,
            created_at: now,
        };
        complete_control_command(&mut tx, &project_id, idempotency_key, &hint).await?;
        tx.commit().await?;
        Ok((hint, Some(event)))
    }

    pub async fn reserve_proof_model_call(
        &self,
        search_id: &str,
        expected_epoch: i64,
    ) -> StorageResult<i64> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::Telemetry,
                "reserve_proof_model_call",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query("SELECT status,cancellation_epoch,model_calls,budget_json FROM proof_searches WHERE search_id=?")
            .bind(search_id).fetch_optional(&mut *tx).await?
            .ok_or_else(|| StorageError::NotFound { kind: "proof_search", id: search_id.into() })?;
        let status: String = row.try_get("status")?;
        let epoch: i64 = row.try_get("cancellation_epoch")?;
        let calls: i64 = row.try_get("model_calls")?;
        let budget: ProofSearchBudget = rows::json(&row, "budget_json")?;
        if status != "running" || epoch != expected_epoch {
            return Err(StorageError::LateSubmission(format!(
                "proof search {search_id} is {status} at epoch {epoch}"
            )));
        }
        if calls >= i64::from(budget.max_model_calls) {
            return Err(StorageError::BudgetExhausted(format!(
                "proof search {search_id} reached max_model_calls={}",
                budget.max_model_calls
            )));
        }
        let next = calls + 1;
        sqlx::query(
            "UPDATE proof_searches SET model_calls=? WHERE search_id=? AND cancellation_epoch=?",
        )
        .bind(next)
        .bind(search_id)
        .bind(expected_epoch)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(next)
    }

    pub async fn consume_proof_hints(
        &self,
        search_id: &str,
        node_id: &str,
        expected_epoch: i64,
    ) -> StorageResult<Vec<ProofHint>> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "consume_proof_hints",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let search = sqlx::query(
            "SELECT nodes_expanded,status,cancellation_epoch FROM proof_searches WHERE search_id=?",
        )
        .bind(search_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "proof_search",
            id: search_id.into(),
        })?;
        let status: String = search.try_get("status")?;
        let epoch: i64 = search.try_get("cancellation_epoch")?;
        if status != "running" || epoch != expected_epoch {
            return Err(StorageError::LateSubmission(format!(
                "proof search {search_id} is {status} at epoch {epoch}"
            )));
        }
        let expansion: i64 = search.try_get("nodes_expanded")?;
        let rows = sqlx::query("SELECT * FROM proof_hints WHERE search_id=? AND consumed_at IS NULL AND effective_after_expansion<=? AND (node_id IS NULL OR node_id=?) ORDER BY created_at,hint_id")
            .bind(search_id).bind(expansion + 1).bind(node_id).fetch_all(&mut *tx).await?;
        let now = Utc::now();
        for row in &rows {
            let hint_id: String = row.try_get("hint_id")?;
            sqlx::query(
                "UPDATE proof_hints SET consumed_at=? WHERE hint_id=? AND consumed_at IS NULL",
            )
            .bind(now.to_rfc3339())
            .bind(hint_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        rows.iter()
            .map(|row| {
                let mut hint = hint_from_row(row)?;
                hint.consumed_at = Some(now);
                Ok(hint)
            })
            .collect()
    }

    pub async fn prune_proof_branch(
        &self,
        search_id: &str,
        node_id: &str,
        expected_epoch: i64,
        requested_by: &str,
        idempotency_key: &str,
    ) -> StorageResult<(ProofSearch, Option<DomainEvent>)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "prune_proof_branch",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query("SELECT ps.*,vc.project_id FROM proof_searches ps JOIN verification_cases vc ON vc.case_id=ps.case_id WHERE ps.search_id=?")
            .bind(search_id).fetch_optional(&mut *tx).await?
            .ok_or_else(|| StorageError::NotFound { kind: "proof_search", id: search_id.into() })?;
        let search = proof_search_from_row(&row)?;
        if search.status != CheckStatus::Running {
            return Err(StorageError::InvalidTransition(format!(
                "proof search {search_id} is {}",
                search.status
            )));
        }
        if search.cancellation_epoch != expected_epoch {
            return Err(StorageError::LateSubmission(format!(
                "proof search {search_id} epoch is {}, expected {expected_epoch}",
                search.cancellation_epoch
            )));
        }
        let project_id: String = row.try_get("project_id")?;
        let request_hash = control_request_hash(
            "prune_branch",
            search_id,
            Some(node_id),
            &expected_epoch.to_string(),
            requested_by,
        )?;
        if let Some(existing) = existing_control_result::<ProofSearch>(
            &mut tx,
            &project_id,
            idempotency_key,
            &request_hash,
        )
        .await?
        {
            tx.rollback().await?;
            return Ok((existing, None));
        }
        let new_epoch = search.cancellation_epoch + 1;
        reserve_control_command(
            &mut tx,
            &project_id,
            idempotency_key,
            "prune_branch",
            node_id,
            &request_hash,
        )
        .await?;
        let now = Utc::now();
        let affected = sqlx::query("WITH RECURSIVE descendants(node_id) AS (SELECT node_id FROM proof_nodes WHERE search_id=? AND node_id=? UNION ALL SELECT pn.node_id FROM proof_nodes pn JOIN descendants d ON pn.parent_node_id=d.node_id WHERE pn.search_id=?) UPDATE proof_nodes SET status='pruned',cancellation_epoch=?,updated_at=? WHERE node_id IN (SELECT node_id FROM descendants)")
            .bind(search_id).bind(node_id).bind(search_id).bind(new_epoch).bind(now.to_rfc3339())
            .execute(&mut *tx).await?.rows_affected();
        if affected == 0 {
            return Err(StorageError::NotFound {
                kind: "proof_node",
                id: node_id.into(),
            });
        }
        sqlx::query("UPDATE proof_nodes SET cancellation_epoch=? WHERE search_id=? AND status IN ('open','running')")
            .bind(new_epoch).bind(search_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE proof_searches SET cancellation_epoch=? WHERE search_id=?")
            .bind(new_epoch)
            .bind(search_id)
            .execute(&mut *tx)
            .await?;
        let revision = bump_revision(&mut tx, &project_id).await?;
        let event = append_event(&mut tx, &project_id, revision, "proof_branch.pruned", entity("proof_node", node_id), json!({"search_id": search_id, "requested_by": requested_by, "affected_nodes": affected, "cancellation_epoch": new_epoch}), Some(entity("proof_search", search_id))).await?;
        let result = ProofSearch {
            cancellation_epoch: new_epoch,
            ..search
        };
        complete_control_command(&mut tx, &project_id, idempotency_key, &result).await?;
        tx.commit().await?;
        Ok((result, Some(event)))
    }

    pub async fn finish_proof_search(
        &self,
        search_id: &str,
        expected_epoch: i64,
        status: CheckStatus,
        solution_node_id: Option<&str>,
    ) -> StorageResult<(ProofSearch, DomainEvent)> {
        if !matches!(
            status,
            CheckStatus::Passed
                | CheckStatus::Unknown
                | CheckStatus::Failed
                | CheckStatus::Cancelled
                | CheckStatus::Error
        ) {
            return Err(StorageError::InvalidTransition(format!(
                "invalid terminal proof-search status {status}"
            )));
        }
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "finish_proof_search",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query("SELECT ps.*,vc.project_id FROM proof_searches ps JOIN verification_cases vc ON vc.case_id=ps.case_id WHERE ps.search_id=? AND ps.status='running' AND ps.cancellation_epoch=?")
            .bind(search_id).bind(expected_epoch).fetch_optional(&mut *tx).await?
            .ok_or_else(|| StorageError::LateSubmission(format!("stale proof-search completion {search_id} at epoch {expected_epoch}")))?;
        if let Some(solution_id) = solution_node_id {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM proof_nodes WHERE search_id=? AND node_id=? AND status='closed'")
                .bind(search_id).bind(solution_id).fetch_one(&mut *tx).await?;
            if count != 1 || status != CheckStatus::Passed {
                return Err(StorageError::InvalidTransition(
                    "proof solution must be a closed node in a passed search".into(),
                ));
            }
        }
        let project_id: String = row.try_get("project_id")?;
        let now = Utc::now();
        sqlx::query("UPDATE proof_searches SET status=?,solution_node_id=?,completed_at=? WHERE search_id=?")
            .bind(status.to_string()).bind(solution_node_id).bind(now.to_rfc3339()).bind(search_id)
            .execute(&mut *tx).await?;
        if status != CheckStatus::Passed {
            sqlx::query("UPDATE proof_nodes SET status='cancelled',updated_at=? WHERE search_id=? AND status IN ('open','running')")
                .bind(now.to_rfc3339()).bind(search_id).execute(&mut *tx).await?;
        }
        let revision = bump_revision(&mut tx, &project_id).await?;
        let event = append_event(&mut tx, &project_id, revision, "proof_search.completed", entity("proof_search", search_id), json!({"status": status, "solution_node_id": solution_node_id, "epoch": expected_epoch}), None).await?;
        tx.commit().await?;
        let mut search = proof_search_from_row(&row)?;
        search.status = status;
        search.solution_node_id = solution_node_id.map(str::to_owned);
        search.completed_at = Some(now);
        Ok((search, event))
    }

    pub async fn cancel_proof_search(
        &self,
        search_id: &str,
        expected_epoch: i64,
        requested_by: &str,
        idempotency_key: &str,
    ) -> StorageResult<(ProofSearch, Option<DomainEvent>)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "cancel_proof_search",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query("SELECT ps.*,vc.project_id FROM proof_searches ps JOIN verification_cases vc ON vc.case_id=ps.case_id WHERE ps.search_id=?")
            .bind(search_id).fetch_optional(&mut *tx).await?
            .ok_or_else(|| StorageError::NotFound { kind: "proof_search", id: search_id.into() })?;
        let mut search = proof_search_from_row(&row)?;
        let project_id: String = row.try_get("project_id")?;
        let request_hash = control_request_hash(
            "cancel_search",
            search_id,
            None,
            &expected_epoch.to_string(),
            requested_by,
        )?;
        if let Some(existing) = existing_control_result::<ProofSearch>(
            &mut tx,
            &project_id,
            idempotency_key,
            &request_hash,
        )
        .await?
        {
            tx.rollback().await?;
            return Ok((existing, None));
        }
        if search.cancellation_epoch != expected_epoch {
            return Err(StorageError::LateSubmission(format!(
                "proof search {search_id} epoch is {}, expected {expected_epoch}",
                search.cancellation_epoch
            )));
        }
        if search.status != CheckStatus::Running {
            reserve_control_command(
                &mut tx,
                &project_id,
                idempotency_key,
                "cancel_search",
                search_id,
                &request_hash,
            )
            .await?;
            let revision = bump_revision(&mut tx, &project_id).await?;
            let event = append_event(&mut tx, &project_id, revision, "proof_search.cancel.ignored", entity("proof_search", search_id), json!({"already_terminal": true, "status": search.status, "requested_by": requested_by}), None).await?;
            complete_control_command(&mut tx, &project_id, idempotency_key, &search).await?;
            tx.commit().await?;
            return Ok((search, Some(event)));
        }
        reserve_control_command(
            &mut tx,
            &project_id,
            idempotency_key,
            "cancel_search",
            search_id,
            &request_hash,
        )
        .await?;
        let new_epoch = search.cancellation_epoch + 1;
        let now = Utc::now();
        sqlx::query("UPDATE proof_searches SET status='cancelled',cancellation_epoch=?,completed_at=? WHERE search_id=?")
            .bind(new_epoch).bind(now.to_rfc3339()).bind(search_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE proof_nodes SET status='cancelled',cancellation_epoch=?,updated_at=? WHERE search_id=? AND status IN ('open','running')")
            .bind(new_epoch).bind(now.to_rfc3339()).bind(search_id).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &project_id).await?;
        let event = append_event(
            &mut tx,
            &project_id,
            revision,
            "proof_search.cancelled",
            entity("proof_search", search_id),
            json!({"requested_by": requested_by, "cancellation_epoch": new_epoch}),
            None,
        )
        .await?;
        search.status = CheckStatus::Cancelled;
        search.cancellation_epoch = new_epoch;
        search.completed_at = Some(now);
        complete_control_command(&mut tx, &project_id, idempotency_key, &search).await?;
        tx.commit().await?;
        Ok((search, Some(event)))
    }

    pub async fn store_proof_search_solution(
        &self,
        case_id: &str,
        base_formalization_id: &str,
        lean_source: &str,
    ) -> StorageResult<(Formalization, DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "store_proof_search_solution",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let case = case_in_tx(&mut tx, case_id).await?;
        ensure_case_progression_allowed(&mut tx, &case).await?;
        let base_row =
            sqlx::query("SELECT * FROM formalizations WHERE case_id=? AND formalization_id=?")
                .bind(case_id)
                .bind(base_formalization_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StorageError::NotFound {
                    kind: "formalization",
                    id: base_formalization_id.into(),
                })?;
        let base = formalization_from_row(&base_row)?;
        let formalization_id = new_id("formalization");
        let source_hash = sha256(lean_source.as_bytes());
        let now = Utc::now();
        sqlx::query("INSERT INTO formalizations(formalization_id,case_id,semantic_contract_id,theorem_name,lean_statement,lean_source,mapping_json,source_hash,status,created_at) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(&formalization_id).bind(case_id).bind(&base.semantic_contract_id).bind(&base.theorem_name)
            .bind(&base.lean_statement).bind(lean_source).bind(json_text(&base.mapping)?)
            .bind(&source_hash).bind("proof_search_solution").bind(now.to_rfc3339()).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &case.project_id).await?;
        let event = append_event(&mut tx, &case.project_id, revision, "verification.formalization.proof_found", entity("formalization", &formalization_id), json!({"case_id": case_id, "base_formalization_id": base_formalization_id, "source_hash": source_hash}), Some(entity("verification_case", case_id))).await?;
        tx.commit().await?;
        Ok((
            Formalization {
                formalization_id,
                case_id: case_id.into(),
                semantic_contract_id: base.semantic_contract_id,
                theorem_name: base.theorem_name,
                lean_statement: base.lean_statement,
                lean_source: lean_source.into(),
                mapping: base.mapping,
                source_hash,
                status: "proof_search_solution".into(),
                created_at: now,
            },
            event,
        ))
    }

    pub async fn get_proof_search(&self, search_id: &str) -> StorageResult<ProofSearch> {
        let row = sqlx::query("SELECT * FROM proof_searches WHERE search_id=?")
            .bind(search_id)
            .fetch_optional(self.pool())
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "proof_search",
                id: search_id.into(),
            })?;
        proof_search_from_row(&row)
    }

    pub async fn get_formalization_proof_search(
        &self,
        formalization_id: &str,
    ) -> StorageResult<ProofSearch> {
        let row = sqlx::query("SELECT * FROM proof_searches WHERE formalization_id=? ORDER BY started_at DESC LIMIT 1")
            .bind(formalization_id).fetch_optional(self.pool()).await?
            .ok_or_else(|| StorageError::NotFound { kind: "proof_search", id: formalization_id.into() })?;
        proof_search_from_row(&row)
    }

    pub async fn get_case_proof_search(&self, case_id: &str) -> StorageResult<ProofSearch> {
        let row = sqlx::query(
            "SELECT * FROM proof_searches WHERE case_id=? ORDER BY started_at DESC LIMIT 1",
        )
        .bind(case_id)
        .fetch_optional(self.pool())
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "proof_search",
            id: case_id.into(),
        })?;
        proof_search_from_row(&row)
    }

    pub async fn list_proof_nodes(
        &self,
        search_id: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> StorageResult<Vec<ProofNode>> {
        let limit = i64::from(limit.clamp(1, 200));
        let rows = if let Some(cursor) = cursor {
            sqlx::query("SELECT * FROM proof_nodes WHERE search_id=? AND (created_at,node_id)>(SELECT created_at,node_id FROM proof_nodes WHERE node_id=? AND search_id=?) ORDER BY created_at,node_id LIMIT ?")
                .bind(search_id).bind(cursor).bind(search_id).bind(limit).fetch_all(self.pool()).await?
        } else {
            sqlx::query(
                "SELECT * FROM proof_nodes WHERE search_id=? ORDER BY created_at,node_id LIMIT ?",
            )
            .bind(search_id)
            .bind(limit)
            .fetch_all(self.pool())
            .await?
        };
        rows.iter().map(proof_node_from_row).collect()
    }

    pub async fn list_open_proof_nodes(&self, search_id: &str) -> StorageResult<Vec<ProofNode>> {
        let rows = sqlx::query("SELECT * FROM proof_nodes WHERE search_id=? AND status IN ('open','running') ORDER BY score DESC,created_at,node_id LIMIT 200")
            .bind(search_id).fetch_all(self.pool()).await?;
        rows.iter().map(proof_node_from_row).collect()
    }

    pub async fn list_proof_edges(&self, search_id: &str) -> StorageResult<Vec<ProofEdge>> {
        let rows =
            sqlx::query("SELECT * FROM proof_edges WHERE search_id=? ORDER BY created_at,edge_id")
                .bind(search_id)
                .fetch_all(self.pool())
                .await?;
        rows.iter().map(proof_edge_from_row).collect()
    }

    pub async fn list_proof_hints(&self, search_id: &str) -> StorageResult<Vec<ProofHint>> {
        let rows =
            sqlx::query("SELECT * FROM proof_hints WHERE search_id=? ORDER BY created_at,hint_id")
                .bind(search_id)
                .fetch_all(self.pool())
                .await?;
        rows.iter().map(hint_from_row).collect()
    }
}

fn control_request_hash(
    action: &str,
    search_id: &str,
    node_id: Option<&str>,
    content: &str,
    requested_by: &str,
) -> StorageResult<String> {
    Ok(sha256(&serde_json::to_vec(&json!({
        "action": action,
        "search_id": search_id,
        "node_id": node_id,
        "content": content,
        "requested_by": requested_by,
    }))?))
}

async fn existing_control_result<T: DeserializeOwned>(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    idempotency_key: &str,
    request_hash: &str,
) -> StorageResult<Option<T>> {
    let row = sqlx::query("SELECT request_hash,result_json FROM proof_control_commands WHERE project_id=? AND idempotency_key=?")
        .bind(project_id).bind(idempotency_key).fetch_optional(&mut **tx).await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let stored_hash: String = row.try_get("request_hash")?;
    if stored_hash != request_hash {
        return Err(StorageError::InvalidTransition(
            "Idempotency-Key was reused with a different proof-control request".into(),
        ));
    }
    let result: Option<String> = row.try_get("result_json")?;
    let result = result.ok_or_else(|| {
        StorageError::InvalidTransition("matching proof-control command is still incomplete".into())
    })?;
    Ok(Some(serde_json::from_str(&result)?))
}

async fn reserve_control_command(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    idempotency_key: &str,
    action: &str,
    target_id: &str,
    request_hash: &str,
) -> StorageResult<()> {
    sqlx::query("INSERT INTO proof_control_commands(command_id,project_id,idempotency_key,action,target_id,request_hash,created_at) VALUES(?,?,?,?,?,?,?)")
        .bind(new_id("pcommand")).bind(project_id).bind(idempotency_key).bind(action)
        .bind(target_id).bind(request_hash).bind(Utc::now().to_rfc3339())
        .execute(&mut **tx).await?;
    Ok(())
}

async fn complete_control_command<T: Serialize>(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    idempotency_key: &str,
    result: &T,
) -> StorageResult<()> {
    sqlx::query("UPDATE proof_control_commands SET result_json=?,completed_at=? WHERE project_id=? AND idempotency_key=?")
        .bind(json_text(result)?).bind(Utc::now().to_rfc3339()).bind(project_id).bind(idempotency_key)
        .execute(&mut **tx).await?;
    Ok(())
}

async fn case_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    case_id: &str,
) -> StorageResult<VerificationCase> {
    let row = sqlx::query("SELECT * FROM verification_cases WHERE case_id=?")
        .bind(case_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "verification_case",
            id: case_id.into(),
        })?;
    case_from_row(&row)
}

async fn ensure_gate_checks(
    tx: &mut Transaction<'_, Sqlite>,
    case: &VerificationCase,
) -> StorageResult<()> {
    let policy_row = sqlx::query("SELECT * FROM verification_policies WHERE policy_id=?")
        .bind(&case.policy_id)
        .fetch_one(&mut **tx)
        .await?;
    let policy = policy_from_row(&policy_row)?;
    for required in &policy.required_checks {
        let status: Option<String> = sqlx::query_scalar("SELECT status FROM verification_checks WHERE case_id=? AND kind=? AND mandatory=1 ORDER BY completed_at DESC LIMIT 1")
            .bind(&case.case_id).bind(required).fetch_optional(&mut **tx).await?;
        if status.as_deref() != Some("passed") {
            return Err(StorageError::InvalidTransition(format!(
                "required verification check {required} is not passed"
            )));
        }
    }
    let snapshot: Option<String> =
        sqlx::query_scalar("SELECT content_hash FROM verification_snapshots WHERE case_id=?")
            .bind(&case.case_id)
            .fetch_optional(&mut **tx)
            .await?;
    if snapshot.is_none() {
        return Err(StorageError::InvalidTransition(
            "verification case has no immutable snapshot".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn validate_verification_snapshot_fence(
    tx: &mut Transaction<'_, Sqlite>,
    verification_id: &str,
) -> StorageResult<(VerificationCase, VerificationSnapshot)> {
    let case_row = sqlx::query("SELECT * FROM verification_cases WHERE verification_id=?")
        .bind(verification_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            StorageError::InvalidTransition(format!(
                "verification {verification_id} has no trusted verification case"
            ))
        })?;
    let case = case_from_row(&case_row)?;
    let snapshot_row = sqlx::query("SELECT * FROM verification_snapshots WHERE case_id=?")
        .bind(&case.case_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            StorageError::InvalidTransition(format!(
                "verification case {} has no immutable snapshot",
                case.case_id
            ))
        })?;
    let snapshot = snapshot_from_row(&snapshot_row)?;
    let candidate_json: String =
        sqlx::query_scalar("SELECT submission_json FROM candidates WHERE candidate_id=?")
            .bind(&case.candidate_id)
            .fetch_one(&mut **tx)
            .await?;
    if sha256(candidate_json.as_bytes()) != snapshot.candidate_hash {
        return Err(StorageError::LateSubmission(
            "candidate changed after verification snapshot".into(),
        ));
    }
    let contract_json: String =
        sqlx::query_scalar("SELECT problem_contract_json FROM projects WHERE project_id=?")
            .bind(&case.project_id)
            .fetch_one(&mut **tx)
            .await?;
    let contract: research_domain::ProblemContract = serde_json::from_str(&contract_json)?;
    if contract.version != snapshot.contract_version {
        return Err(StorageError::LateSubmission(
            "problem contract changed after verification snapshot".into(),
        ));
    }
    Ok((case, snapshot))
}

pub(crate) async fn validate_fact_gate(
    tx: &mut Transaction<'_, Sqlite>,
    verification_id: &str,
) -> StorageResult<FactGateContext> {
    let (case, snapshot) = validate_verification_snapshot_fence(tx, verification_id).await?;
    if case.stage != VerificationStage::CommitReady {
        return Err(StorageError::InvalidTransition(format!(
            "verification case {} is {}, not commit_ready",
            case.case_id, case.stage
        )));
    }
    ensure_gate_checks(tx, &case).await?;
    let acceptance = case.achieved_acceptance.ok_or_else(|| {
        StorageError::InvalidTransition("verification case has no achieved acceptance".into())
    })?;
    if acceptance_rank(acceptance) < acceptance_rank(case.required_acceptance) {
        return Err(StorageError::InvalidTransition(format!(
            "achieved {acceptance} is below required {}",
            case.required_acceptance
        )));
    }
    let mut pending_dependencies = snapshot
        .dependency_hashes
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut checked_dependencies = BTreeSet::new();
    while let Some(fact_id) = pending_dependencies.pop() {
        if !checked_dependencies.insert(fact_id.clone()) {
            continue;
        }
        let row = sqlx::query("SELECT f.status,f.content_hash,f.dependency_fact_ids_json FROM facts f WHERE f.fact_id=? AND (f.project_id=? OR EXISTS (SELECT 1 FROM project_fact_imports i WHERE i.target_project_id=? AND i.source_fact_id=f.fact_id AND i.status='active'))")
            .bind(&fact_id).bind(&case.project_id).bind(&case.project_id).fetch_optional(&mut **tx).await?
            .ok_or_else(|| StorageError::InvalidDependency(fact_id.clone()))?;
        let status: String = row.try_get("status")?;
        let content_hash: String = row.try_get("content_hash")?;
        if status != "active" {
            return Err(StorageError::InvalidDependency(fact_id.clone()));
        }
        if let Some(expected_hash) = snapshot.dependency_hashes.get(&fact_id)
            && &content_hash != expected_hash
        {
            return Err(StorageError::InvalidDependency(fact_id.clone()));
        }
        let assurance = sqlx::query("SELECT acceptance_class FROM fact_assurances WHERE fact_id=? AND status='active' ORDER BY created_at DESC LIMIT 1")
            .bind(&fact_id).fetch_optional(&mut **tx).await?
            .ok_or_else(|| StorageError::InvalidTransition(format!("dependency fact {fact_id} has no active assurance")))?;
        let dependency_acceptance =
            AcceptanceClass::from_str(assurance.try_get("acceptance_class")?)
                .map_err(StorageError::CorruptData)?;
        if acceptance_rank(dependency_acceptance) < acceptance_rank(case.required_acceptance) {
            return Err(StorageError::InvalidTransition(format!(
                "dependency fact {fact_id} assurance {dependency_acceptance} is below required {}",
                case.required_acceptance
            )));
        }
        let transitive: Vec<String> =
            serde_json::from_str(row.try_get("dependency_fact_ids_json")?)?;
        pending_dependencies.extend(transitive);
    }
    for (source_id, expected_hash) in &snapshot.source_hashes {
        let row = sqlx::query("SELECT * FROM sources WHERE project_id=? AND source_id=?")
            .bind(&case.project_id)
            .bind(source_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "source",
                id: source_id.clone(),
            })?;
        let source = rows::source(&row)?;
        if !crate::source_ingestion::status_can_support_candidate(&source.status) {
            return Err(StorageError::InvalidDependency(format!(
                "source {source_id} has non-evidence status {}",
                source.status
            )));
        }
        if sha256(&serde_json::to_vec(&source)?) != *expected_hash {
            return Err(StorageError::LateSubmission(format!(
                "source {source_id} changed after verification snapshot"
            )));
        }
    }
    let package_id = sqlx::query_scalar("SELECT package_id FROM verification_packages WHERE case_id=? ORDER BY created_at DESC LIMIT 1")
        .bind(&case.case_id).fetch_optional(&mut **tx).await?;
    let replay_id = sqlx::query_scalar("SELECT replay_id FROM verification_replays WHERE case_id=? AND status='passed' ORDER BY created_at DESC LIMIT 1")
        .bind(&case.case_id).fetch_optional(&mut **tx).await?;
    Ok(FactGateContext {
        case_id: case.case_id,
        acceptance,
        snapshot_hash: snapshot.content_hash,
        package_id,
        replay_id,
    })
}

fn ensure_case_mutable(case: &VerificationCase) -> StorageResult<()> {
    if matches!(
        case.stage,
        VerificationStage::Committed
            | VerificationStage::Rejected
            | VerificationStage::Unknown
            | VerificationStage::Failed
            | VerificationStage::Cancelled
    ) {
        return Err(StorageError::InvalidTransition(format!(
            "verification case {} is terminal ({})",
            case.case_id, case.stage
        )));
    }
    Ok(())
}

async fn ensure_case_progression_allowed(
    tx: &mut Transaction<'_, Sqlite>,
    case: &VerificationCase,
) -> StorageResult<()> {
    ensure_case_mutable(case)?;
    let lifecycle = sqlx::query(
        "SELECT v.status AS verification_status,p.status AS project_status \
         FROM verifications v JOIN projects p ON p.project_id=v.project_id \
         WHERE v.verification_id=? AND v.project_id=?",
    )
    .bind(&case.verification_id)
    .bind(&case.project_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        StorageError::CorruptData(format!(
            "verification case {} lost its owning verification or project",
            case.case_id
        ))
    })?;
    let verification_status: String = lifecycle.try_get("verification_status")?;
    let project_status: String = lifecycle.try_get("project_status")?;
    let route_released =
        crate::verification_execution_is_released(tx, &case.verification_id).await?;
    if verification_status != "verifying" || !route_released {
        return Err(StorageError::LateSubmission(format!(
            "verification case {} cannot advance while verification is {verification_status} and project is {project_status}",
            case.case_id
        )));
    }
    Ok(())
}

fn valid_transition(from: VerificationStage, to: VerificationStage) -> bool {
    use VerificationStage as S;
    matches!(
        (from, to),
        (
            S::Intake,
            S::Snapshotting | S::Precheck | S::Rejected | S::Failed | S::Cancelled
        ) | (S::Snapshotting, S::Precheck | S::Failed | S::Cancelled)
            | (
                S::Precheck,
                S::Review | S::Rejected | S::Unknown | S::Failed | S::Cancelled
            )
            | (
                S::Review,
                S::Formalization
                    | S::Adjudication
                    | S::Rejected
                    | S::Unknown
                    | S::Failed
                    | S::Cancelled
            )
            | (
                S::Formalization,
                S::ProofSearch
                    | S::Adjudication
                    | S::Rejected
                    | S::Unknown
                    | S::Failed
                    | S::Cancelled
            )
            | (
                S::ProofSearch,
                S::Adjudication | S::Rejected | S::Unknown | S::Failed | S::Cancelled
            )
            | (
                S::Adjudication,
                S::Packaging | S::CommitReady | S::Rejected | S::Unknown | S::Failed | S::Cancelled
            )
            | (
                S::Packaging,
                S::Replay | S::CommitReady | S::Failed | S::Cancelled
            )
            | (S::Replay, S::CommitReady | S::Failed | S::Cancelled)
            | (S::CommitReady, S::Committed | S::Failed | S::Cancelled)
    )
}

const fn acceptance_rank(value: AcceptanceClass) -> u8 {
    match value {
        AcceptanceClass::Reviewed => 1,
        AcceptanceClass::ComputationallyCertified => 2,
        AcceptanceClass::FormallyVerified => 3,
        AcceptanceClass::FullyCertified => 4,
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn case_from_row(row: &SqliteRow) -> StorageResult<VerificationCase> {
    Ok(VerificationCase {
        case_id: row.try_get("case_id")?,
        verification_id: row.try_get("verification_id")?,
        candidate_id: row.try_get("candidate_id")?,
        project_id: row.try_get("project_id")?,
        policy_id: row.try_get("policy_id")?,
        snapshot_id: row.try_get("snapshot_id")?,
        profile: VerificationProfile::from_str(row.try_get("profile")?)
            .map_err(StorageError::CorruptData)?,
        required_acceptance: AcceptanceClass::from_str(row.try_get("required_acceptance")?)
            .map_err(StorageError::CorruptData)?,
        achieved_acceptance: row
            .try_get::<Option<String>, _>("achieved_acceptance")?
            .map(|value| AcceptanceClass::from_str(&value).map_err(StorageError::CorruptData))
            .transpose()?,
        stage: VerificationStage::from_str(row.try_get("stage")?)
            .map_err(StorageError::CorruptData)?,
        risk_score: row.try_get("risk_score")?,
        risk_reasons: rows::json(row, "risk_reasons_json")?,
        cancellation_epoch: row.try_get("cancellation_epoch")?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
        updated_at: rows::timestamp(row.try_get("updated_at")?)?,
        completed_at: rows::optional_timestamp(row.try_get("completed_at")?)?,
    })
}

fn policy_from_row(row: &SqliteRow) -> StorageResult<VerificationPolicy> {
    let policy = VerificationPolicy {
        policy_id: row.try_get("policy_id")?,
        project_id: row.try_get("project_id")?,
        name: row.try_get("name")?,
        profile: VerificationProfile::from_str(row.try_get("profile")?)
            .map_err(StorageError::CorruptData)?,
        required_acceptance: AcceptanceClass::from_str(row.try_get("required_acceptance")?)
            .map_err(StorageError::CorruptData)?,
        required_checks: rows::json(row, "required_checks_json")?,
        independent_reviewer_count: u32::try_from(
            row.try_get::<i64, _>("independent_reviewer_count")?,
        )
        .map_err(|error| StorageError::CorruptData(error.to_string()))?,
        require_citation_review: row.try_get("require_citation_review")?,
        require_adversarial_review: row.try_get("require_adversarial_review")?,
        require_alignment_review: row.try_get("require_alignment_review")?,
        require_fresh_replay: row.try_get("require_fresh_replay")?,
        max_attempts: u32::try_from(row.try_get::<i64, _>("max_attempts")?)
            .map_err(|error| StorageError::CorruptData(error.to_string()))?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
    };
    policy
        .canonical_requirements()
        .map_err(StorageError::CorruptData)?;
    Ok(policy)
}

fn snapshot_from_row(row: &SqliteRow) -> StorageResult<VerificationSnapshot> {
    Ok(VerificationSnapshot {
        snapshot_id: row.try_get("snapshot_id")?,
        case_id: row.try_get("case_id")?,
        project_id: row.try_get("project_id")?,
        candidate_hash: row.try_get("candidate_hash")?,
        project_revision: row.try_get("project_revision")?,
        contract_version: row.try_get("contract_version")?,
        dependency_hashes: rows::json(row, "dependency_hashes_json")?,
        source_hashes: rows::json(row, "source_hashes_json")?,
        policy_hash: row.try_get("policy_hash")?,
        toolchain_hash: row.try_get("toolchain_hash")?,
        content_hash: row.try_get("content_hash")?,
        payload: rows::json(row, "payload_json")?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
    })
}

fn check_from_row(row: &SqliteRow) -> StorageResult<VerificationCheck> {
    Ok(VerificationCheck {
        check_id: row.try_get("check_id")?,
        case_id: row.try_get("case_id")?,
        attempt_id: row.try_get("attempt_id")?,
        kind: row.try_get("kind")?,
        status: CheckStatus::from_str(row.try_get("status")?).map_err(StorageError::CorruptData)?,
        mandatory: row.try_get("mandatory")?,
        summary: row.try_get("summary")?,
        details: rows::json(row, "details_json")?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
        completed_at: rows::optional_timestamp(row.try_get("completed_at")?)?,
    })
}

fn finding_from_row(row: &SqliteRow) -> StorageResult<VerificationFinding> {
    Ok(VerificationFinding {
        finding_id: row.try_get("finding_id")?,
        case_id: row.try_get("case_id")?,
        check_id: row.try_get("check_id")?,
        reviewer_kind: row.try_get("reviewer_kind")?,
        severity: row.try_get("severity")?,
        category: row.try_get("category")?,
        location: row.try_get("location")?,
        claim: row.try_get("claim")?,
        rationale: row.try_get("rationale")?,
        status: row.try_get("status")?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
    })
}

fn evidence_from_row(row: &SqliteRow) -> StorageResult<VerificationEvidence> {
    Ok(VerificationEvidence {
        evidence_id: row.try_get("evidence_id")?,
        case_id: row.try_get("case_id")?,
        check_id: row.try_get("check_id")?,
        dimension: row.try_get("dimension")?,
        kind: row.try_get("kind")?,
        uri: row.try_get("uri")?,
        sha256: row.try_get("sha256")?,
        payload: rows::json(row, "payload_json")?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
    })
}

fn semantic_from_row(row: &SqliteRow) -> StorageResult<SemanticContract> {
    Ok(SemanticContract {
        contract_id: row.try_get("contract_id")?,
        case_id: row.try_get("case_id")?,
        version: row.try_get("version")?,
        natural_language_statement: row.try_get("natural_language_statement")?,
        variables: rows::json(row, "variables_json")?,
        assumptions: rows::json(row, "assumptions_json")?,
        conclusion: row.try_get("conclusion")?,
        definitions: rows::json(row, "definitions_json")?,
        boundary_conditions: rows::json(row, "boundary_conditions_json")?,
        ambiguity_notes: rows::json(row, "ambiguity_notes_json")?,
        content_hash: row.try_get("content_hash")?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
    })
}

fn formalization_from_row(row: &SqliteRow) -> StorageResult<Formalization> {
    Ok(Formalization {
        formalization_id: row.try_get("formalization_id")?,
        case_id: row.try_get("case_id")?,
        semantic_contract_id: row.try_get("semantic_contract_id")?,
        theorem_name: row.try_get("theorem_name")?,
        lean_statement: row.try_get("lean_statement")?,
        lean_source: row.try_get("lean_source")?,
        mapping: rows::json(row, "mapping_json")?,
        source_hash: row.try_get("source_hash")?,
        status: row.try_get("status")?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
    })
}

fn alignment_from_row(row: &SqliteRow) -> StorageResult<AlignmentReview> {
    Ok(AlignmentReview {
        alignment_id: row.try_get("alignment_id")?,
        case_id: row.try_get("case_id")?,
        formalization_id: row.try_get("formalization_id")?,
        relation: AlignmentRelation::from_str(row.try_get("relation")?)
            .map_err(StorageError::CorruptData)?,
        reviewer_kind: row.try_get("reviewer_kind")?,
        rationale: row.try_get("rationale")?,
        missing_assumptions: rows::json(row, "missing_assumptions_json")?,
        extra_assumptions: rows::json(row, "extra_assumptions_json")?,
        confidence: row.try_get("confidence")?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
    })
}

fn backend_run_from_row(row: &SqliteRow) -> StorageResult<BackendRun> {
    Ok(BackendRun {
        backend_run_id: row.try_get("backend_run_id")?,
        case_id: row.try_get("case_id")?,
        attempt_id: row.try_get("attempt_id")?,
        backend: row.try_get("backend")?,
        backend_version: row.try_get("backend_version")?,
        status: CheckStatus::from_str(row.try_get("status")?).map_err(StorageError::CorruptData)?,
        command: rows::json(row, "command_json")?,
        working_directory: row.try_get("working_directory")?,
        exit_code: row.try_get("exit_code")?,
        stdout: row.try_get("stdout")?,
        stderr: row.try_get("stderr")?,
        diagnostics: rows::json(row, "diagnostics_json")?,
        axioms: rows::json(row, "axioms_json")?,
        elapsed_ms: row.try_get("elapsed_ms")?,
        input_hash: row.try_get("input_hash")?,
        output_hash: row.try_get("output_hash")?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
    })
}

fn package_from_row(row: &SqliteRow) -> StorageResult<VerificationPackage> {
    Ok(VerificationPackage {
        package_id: row.try_get("package_id")?,
        case_id: row.try_get("case_id")?,
        manifest: rows::json(row, "manifest_json")?,
        manifest_hash: row.try_get("manifest_hash")?,
        storage_path: row.try_get("storage_path")?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
    })
}

fn replay_from_row(row: &SqliteRow) -> StorageResult<VerificationReplay> {
    Ok(VerificationReplay {
        replay_id: row.try_get("replay_id")?,
        case_id: row.try_get("case_id")?,
        package_id: row.try_get("package_id")?,
        status: CheckStatus::from_str(row.try_get("status")?).map_err(StorageError::CorruptData)?,
        fresh_process: row.try_get("fresh_process")?,
        manifest_hash: row.try_get("manifest_hash")?,
        observed_hash: row.try_get("observed_hash")?,
        backend_run_id: row.try_get("backend_run_id")?,
        summary: row.try_get("summary")?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
    })
}

fn proof_search_from_row(row: &SqliteRow) -> StorageResult<ProofSearch> {
    Ok(ProofSearch {
        search_id: row.try_get("search_id")?,
        case_id: row.try_get("case_id")?,
        formalization_id: row.try_get("formalization_id")?,
        status: CheckStatus::from_str(row.try_get("status")?).map_err(StorageError::CorruptData)?,
        strategy: row.try_get("strategy")?,
        budget: rows::json(row, "budget_json")?,
        nodes_created: row.try_get("nodes_created")?,
        nodes_expanded: row.try_get("nodes_expanded")?,
        model_calls: row.try_get("model_calls")?,
        cancellation_epoch: row.try_get("cancellation_epoch")?,
        root_node_id: row.try_get("root_node_id")?,
        solution_node_id: row.try_get("solution_node_id")?,
        started_at: rows::timestamp(row.try_get("started_at")?)?,
        completed_at: rows::optional_timestamp(row.try_get("completed_at")?)?,
    })
}

fn proof_node_from_row(row: &SqliteRow) -> StorageResult<ProofNode> {
    Ok(ProofNode {
        node_id: row.try_get("node_id")?,
        search_id: row.try_get("search_id")?,
        parent_node_id: row.try_get("parent_node_id")?,
        depth: row.try_get("depth")?,
        state_id: row.try_get("state_id")?,
        goal: row.try_get("goal")?,
        local_context: rows::json(row, "local_context_json")?,
        tactic: row.try_get("tactic")?,
        score: row.try_get("score")?,
        status: ProofNodeStatus::from_str(row.try_get("status")?)
            .map_err(StorageError::CorruptData)?,
        diagnostic: row.try_get("diagnostic")?,
        cancellation_epoch: row.try_get("cancellation_epoch")?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
        updated_at: rows::timestamp(row.try_get("updated_at")?)?,
    })
}

fn proof_edge_from_row(row: &SqliteRow) -> StorageResult<ProofEdge> {
    Ok(ProofEdge {
        edge_id: row.try_get("edge_id")?,
        search_id: row.try_get("search_id")?,
        source_node_id: row.try_get("source_node_id")?,
        target_node_id: row.try_get("target_node_id")?,
        tactic: row.try_get("tactic")?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
    })
}

fn hint_from_row(row: &SqliteRow) -> StorageResult<ProofHint> {
    Ok(ProofHint {
        hint_id: row.try_get("hint_id")?,
        search_id: row.try_get("search_id")?,
        node_id: row.try_get("node_id")?,
        content: row.try_get("content")?,
        requested_by: row.try_get("requested_by")?,
        effective_after_expansion: row.try_get("effective_after_expansion")?,
        consumed_at: rows::optional_timestamp(row.try_get("consumed_at")?)?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
    })
}
