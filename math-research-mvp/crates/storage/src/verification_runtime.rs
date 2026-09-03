use chrono::{DateTime, Utc};
use research_domain::{ContextPacket, DomainEvent};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{
    SqliteStore, StorageError, StorageResult, append_event, bump_revision, current_revision,
    entity, json_text, new_id,
};

#[derive(Debug, Clone)]
pub struct VerificationWorkerOffer {
    pub project_id: String,
    pub case_id: String,
    pub attempt_id: String,
    pub worker_instance_id: String,
    pub kind: String,
    pub sequence: i64,
    pub lease_epoch: i64,
    pub cancellation_epoch: i64,
    pub context_packet: ContextPacket,
    pub contract: Value,
    pub contract_hash: String,
    pub events: Vec<DomainEvent>,
}

#[derive(Debug, Clone)]
pub struct VerificationWorkerLease {
    pub project_id: String,
    pub case_id: String,
    pub attempt_id: String,
    pub worker_instance_id: String,
    pub verification_lease_id: String,
    pub lease_token: String,
    pub lease_epoch: i64,
    pub cancellation_epoch: i64,
    pub expires_at: DateTime<Utc>,
    pub events: Vec<DomainEvent>,
}

impl SqliteStore {
    pub async fn pending_verification_result_envelopes(
        &self,
        project_id: &str,
    ) -> StorageResult<Vec<String>> {
        self.get_project(project_id).await?;
        Ok(sqlx::query_scalar("SELECT verification_result_envelope_id FROM verification_result_envelopes WHERE project_id=? AND status='submitted' ORDER BY submitted_at")
            .bind(project_id).fetch_all(self.pool()).await?)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn offer_verification_worker(
        &self,
        case_id: &str,
        kind: &str,
        backend: &str,
        model: Option<&str>,
        working_directory: &str,
        content: &Value,
        allowed_fact_ids: &[String],
        completion_contract: &Value,
    ) -> StorageResult<VerificationWorkerOffer> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "offer_verification_worker",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let case = sqlx::query(
            "SELECT project_id,stage,cancellation_epoch FROM verification_cases WHERE case_id=?",
        )
        .bind(case_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "verification_case",
            id: case_id.into(),
        })?;
        let project_id: String = case.try_get("project_id")?;
        let stage: String = case.try_get("stage")?;
        let cancellation_epoch: i64 = case.try_get("cancellation_epoch")?;
        if matches!(
            stage.as_str(),
            "committed" | "rejected" | "unknown" | "cancelled"
        ) {
            return Err(StorageError::InvalidTransition(format!(
                "verification case is {stage}"
            )));
        }
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence),0)+1 FROM verification_attempts WHERE case_id=?",
        )
        .bind(case_id)
        .fetch_one(&mut *tx)
        .await?;
        let lease_epoch: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(lease_epoch),0)+1 FROM verification_attempts WHERE case_id=? AND kind=?")
            .bind(case_id).bind(kind).fetch_one(&mut *tx).await?;
        let revision = current_revision(&mut tx, &project_id).await?;
        let attempt_id = new_id("verattempt");
        let worker_instance_id = new_id("workerinstance");
        let context_packet_id = new_id("context");
        let content_hash = hash_json(content)?;
        let token_estimate =
            i64::try_from(serde_json::to_string(content)?.chars().count().div_ceil(4))
                .unwrap_or(i64::MAX);
        let now = Utc::now();
        let problem_excerpt = content
            .get("problem_contract")
            .map_or_else(|| "verification snapshot".into(), ToString::to_string);
        let source_refs = vec![entity("verification_case", case_id)];
        sqlx::query("INSERT INTO context_packets(context_packet_id,project_id,packet_kind,source_revision,problem_contract_excerpt,objective,known_fact_ids_json,route_progress_json,relevant_failure_pattern_ids_json,relevant_uncertainty_ids_json,source_refs_json,omitted_sections_json,token_estimate,content_json,content_hash,status,created_at) VALUES(?,?,'verification',?,?,?,?,'{}','[]','[]',?,'[]',?,?,?,'active',?)")
            .bind(&context_packet_id).bind(&project_id).bind(revision).bind(&problem_excerpt).bind(kind)
            .bind(json_text(allowed_fact_ids)?).bind(json_text(&source_refs)?).bind(token_estimate)
            .bind(json_text(content)?).bind(&content_hash).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        let contract = json!({
            "case_id":case_id,"attempt_id":attempt_id,"task_kind":kind,
            "precise_objective":format!("independently execute verification stage {kind}"),
            "allowed_input_ids":allowed_fact_ids,"context_packet_id":context_packet_id,
            "allowed_tools":["read_verification_snapshot","submit_verification_result"],
            "forbidden_actions":["write_fact_graph","read_other_reviewer_results","claim_acceptance_without_adjudicator"],
            "completion_contract":completion_contract,
            "retry_policy":{"max_attempts":3,"same_failure_signature_retries":1},
            "cancellation_epoch":cancellation_epoch,
        });
        let contract_hash = hash_json(&contract)?;
        let input_hash =
            hash_json(&json!({"context_hash":content_hash,"contract_hash":contract_hash}))?;
        sqlx::query("INSERT INTO worker_instances(worker_instance_id,project_id,backend,model,status,capability_json,working_directory,started_at) VALUES(?,?,?,?, 'handshaking','{}',?,?)")
            .bind(&worker_instance_id).bind(&project_id).bind(backend).bind(model).bind(working_directory)
            .bind(now.to_rfc3339()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO verification_attempts(attempt_id,case_id,sequence,kind,status,backend,cancellation_epoch,input_hash,worker_instance_id,context_packet_id,lease_epoch) VALUES(?,?,?,?, 'offered',?,?,?,?,?,?)")
            .bind(&attempt_id).bind(case_id).bind(sequence).bind(kind).bind(backend).bind(cancellation_epoch)
            .bind(&input_hash).bind(&worker_instance_id).bind(&context_packet_id).bind(lease_epoch)
            .execute(&mut *tx).await?;
        sqlx::query("INSERT INTO verification_task_contracts(verification_task_contract_id,project_id,case_id,attempt_id,contract_json,content_hash,created_at) VALUES(?,?,?,?,?,?,?)")
            .bind(new_id("verificationcontract")).bind(&project_id).bind(case_id).bind(&attempt_id)
            .bind(json_text(&contract)?).bind(&contract_hash).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        let project_revision = bump_revision(&mut tx, &project_id).await?;
        let events=vec![
            append_event(&mut tx,&project_id,project_revision,"verification.worker.offered",entity("verification_attempt",&attempt_id),json!({"case_id":case_id,"kind":kind,"worker_instance_id":worker_instance_id,"lease_epoch":lease_epoch}),Some(entity("verification_case",case_id))).await?,
            append_event(&mut tx,&project_id,project_revision,"context.packet.created",entity("context_packet",&context_packet_id),json!({"packet_kind":"verification","case_id":case_id,"attempt_id":attempt_id,"content_hash":content_hash}),Some(entity("verification_attempt",&attempt_id))).await?,
        ];
        tx.commit().await?;
        let context_packet = self.get_context_packet(&context_packet_id).await?;
        Ok(VerificationWorkerOffer {
            project_id,
            case_id: case_id.into(),
            attempt_id,
            worker_instance_id,
            kind: kind.into(),
            sequence,
            lease_epoch,
            cancellation_epoch,
            context_packet,
            contract,
            contract_hash,
            events,
        })
    }

    pub async fn accept_verification_worker_handshake(
        &self,
        offer: &VerificationWorkerOffer,
        backend_version: Option<&str>,
        handshake: &Value,
        ttl_seconds: u64,
    ) -> StorageResult<VerificationWorkerLease> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "accept_verification_worker_handshake",
            )
            .await?;
        let lease_token = format!("{}{}", new_id("token"), new_id("token"));
        let now = Utc::now();
        let expires_at = now
            + chrono::Duration::seconds(
                i64::try_from(ttl_seconds.clamp(30, 3_600)).unwrap_or(3_600),
            );
        let verification_lease_id = new_id("verlease");
        let mut tx = self.pool().begin().await?;
        let current_epoch: i64 =
            sqlx::query_scalar("SELECT cancellation_epoch FROM verification_cases WHERE case_id=?")
                .bind(&offer.case_id)
                .fetch_one(&mut *tx)
                .await?;
        if current_epoch != offer.cancellation_epoch {
            return Err(StorageError::LateSubmission(
                "verification case epoch changed before handshake".into(),
            ));
        }
        let changed=sqlx::query("UPDATE verification_attempts SET status='running',started_at=? WHERE attempt_id=? AND status='offered'")
            .bind(now.to_rfc3339()).bind(&offer.attempt_id).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::LateSubmission(
                "verification offer is stale".into(),
            ));
        }
        sqlx::query("INSERT INTO verification_task_leases(verification_lease_id,project_id,case_id,attempt_id,worker_instance_id,lease_epoch,cancellation_epoch,lease_token_hash,status,offered_at,leased_at,expires_at,last_heartbeat_at) VALUES(?,?,?,?,?,?,?,?,'active',?,?,?,?)")
            .bind(&verification_lease_id).bind(&offer.project_id).bind(&offer.case_id).bind(&offer.attempt_id)
            .bind(&offer.worker_instance_id).bind(offer.lease_epoch).bind(offer.cancellation_epoch).bind(hash_secret(&lease_token))
            .bind(offer.context_packet.created_at.to_rfc3339()).bind(now.to_rfc3339()).bind(expires_at.to_rfc3339()).bind(now.to_rfc3339())
            .execute(&mut *tx).await?;
        sqlx::query("UPDATE worker_instances SET status='running',backend_version=?,handshake_json=?,ready_at=?,last_heartbeat_at=? WHERE worker_instance_id=? AND status='handshaking'")
            .bind(backend_version).bind(json_text(handshake)?).bind(now.to_rfc3339()).bind(now.to_rfc3339()).bind(&offer.worker_instance_id)
            .execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &offer.project_id).await?;
        let events=vec![append_event(&mut tx,&offer.project_id,revision,"verification.worker.lease_acquired",entity("verification_task_lease",&verification_lease_id),json!({"case_id":offer.case_id,"attempt_id":offer.attempt_id,"lease_epoch":offer.lease_epoch,"expires_at":expires_at}),Some(entity("worker_instance",&offer.worker_instance_id))).await?];
        tx.commit().await?;
        Ok(VerificationWorkerLease {
            project_id: offer.project_id.clone(),
            case_id: offer.case_id.clone(),
            attempt_id: offer.attempt_id.clone(),
            worker_instance_id: offer.worker_instance_id.clone(),
            verification_lease_id,
            lease_token,
            lease_epoch: offer.lease_epoch,
            cancellation_epoch: offer.cancellation_epoch,
            expires_at,
            events,
        })
    }

    pub async fn submit_verification_result(
        &self,
        lease: &VerificationWorkerLease,
        outcome: &str,
        payload: &Value,
    ) -> StorageResult<(String, Vec<DomainEvent>)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "submit_verification_result",
            )
            .await?;
        let content_hash = hash_json(payload)?;
        let idempotency_key = format!("verification-result:{}:{content_hash}", lease.attempt_id);
        if let Some(existing)=sqlx::query_scalar::<_,String>("SELECT verification_result_envelope_id FROM verification_result_envelopes WHERE project_id=? AND idempotency_key=?")
            .bind(&lease.project_id).bind(&idempotency_key).fetch_optional(self.pool()).await?
        { return Ok((existing,Vec::new())); }
        let mut tx = self.pool().begin().await?;
        validate_lease(&mut tx, lease).await?;
        let envelope_id = new_id("verresult");
        let now = Utc::now();
        sqlx::query("INSERT INTO verification_result_envelopes(verification_result_envelope_id,project_id,case_id,attempt_id,verification_lease_id,lease_epoch,cancellation_epoch,outcome,envelope_json,content_hash,idempotency_key,status,submitted_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,'submitted',?)")
            .bind(&envelope_id).bind(&lease.project_id).bind(&lease.case_id).bind(&lease.attempt_id).bind(&lease.verification_lease_id)
            .bind(lease.lease_epoch).bind(lease.cancellation_epoch).bind(outcome).bind(json_text(payload)?).bind(&content_hash).bind(&idempotency_key)
            .bind(now.to_rfc3339()).execute(&mut *tx).await?;
        sqlx::query("UPDATE verification_attempts SET status='result_submitted',output_hash=? WHERE attempt_id=? AND status='running'")
            .bind(&content_hash).bind(&lease.attempt_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE worker_instances SET status='draining' WHERE worker_instance_id=?")
            .bind(&lease.worker_instance_id)
            .execute(&mut *tx)
            .await?;
        let revision = bump_revision(&mut tx, &lease.project_id).await?;
        let event=append_event(&mut tx,&lease.project_id,revision,"verification.worker.result_submitted",entity("verification_result_envelope",&envelope_id),json!({"case_id":lease.case_id,"attempt_id":lease.attempt_id,"content_hash":content_hash}),Some(entity("verification_task_lease",&lease.verification_lease_id))).await?;
        tx.commit().await?;
        Ok((envelope_id, vec![event]))
    }

    pub async fn heartbeat_verification_worker(
        &self,
        lease: &VerificationWorkerLease,
        ttl_seconds: u64,
    ) -> StorageResult<()> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::Telemetry,
                "heartbeat_verification_worker",
            )
            .await?;
        let now = Utc::now();
        let expires_at = now
            + chrono::Duration::seconds(
                i64::try_from(ttl_seconds.clamp(30, 3_600)).unwrap_or(3_600),
            );
        let changed = sqlx::query("UPDATE verification_task_leases SET expires_at=?,last_heartbeat_at=? WHERE verification_lease_id=? AND attempt_id=? AND lease_epoch=? AND lease_token_hash=? AND status='active' AND expires_at>=?")
            .bind(expires_at.to_rfc3339()).bind(now.to_rfc3339()).bind(&lease.verification_lease_id)
            .bind(&lease.attempt_id).bind(lease.lease_epoch).bind(hash_secret(&lease.lease_token))
            .bind(now.to_rfc3339()).execute(self.pool()).await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::LateSubmission(
                "verification heartbeat lease is stale or expired".into(),
            ));
        }
        sqlx::query("UPDATE worker_instances SET last_heartbeat_at=? WHERE worker_instance_id=? AND status='running'")
            .bind(now.to_rfc3339()).bind(&lease.worker_instance_id).execute(self.pool()).await?;
        Ok(())
    }

    pub async fn ingest_verification_result(
        &self,
        envelope_id: &str,
    ) -> StorageResult<(Value, Vec<DomainEvent>)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "ingest_verification_result",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query(
            "SELECT * FROM verification_result_envelopes WHERE verification_result_envelope_id=?",
        )
        .bind(envelope_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "verification_result_envelope",
            id: envelope_id.into(),
        })?;
        let payload: Value = serde_json::from_str(row.try_get("envelope_json")?)?;
        let status: String = row.try_get("status")?;
        if status == "ingested" {
            tx.rollback().await?;
            return Ok((payload, Vec::new()));
        }
        if status != "submitted" {
            return Err(StorageError::InvalidTransition(format!(
                "verification result is {status}"
            )));
        }
        let project_id: String = row.try_get("project_id")?;
        let case_id: String = row.try_get("case_id")?;
        let attempt_id: String = row.try_get("attempt_id")?;
        let lease_id: String = row.try_get("verification_lease_id")?;
        let lease_epoch: i64 = row.try_get("lease_epoch")?;
        let cancellation_epoch: i64 = row.try_get("cancellation_epoch")?;
        let valid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM verification_task_leases l JOIN verification_attempts a ON a.attempt_id=l.attempt_id JOIN verification_cases c ON c.case_id=l.case_id WHERE l.verification_lease_id=? AND l.status IN ('active','expired') AND l.lease_epoch=? AND l.cancellation_epoch=? AND a.status='result_submitted' AND c.cancellation_epoch=?")
            .bind(&lease_id).bind(lease_epoch).bind(cancellation_epoch).bind(cancellation_epoch).fetch_one(&mut *tx).await?;
        if valid != 1 {
            sqlx::query("UPDATE verification_result_envelopes SET status='stale',rejection_reason='lease or cancellation epoch changed' WHERE verification_result_envelope_id=?")
                .bind(envelope_id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Err(StorageError::LateSubmission(
                "verification result became stale".into(),
            ));
        }
        let now = Utc::now();
        sqlx::query(
            "UPDATE verification_attempts SET status='completed',completed_at=? WHERE attempt_id=?",
        )
        .bind(now.to_rfc3339())
        .bind(&attempt_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE verification_task_leases SET status='completed',completed_at=? WHERE verification_lease_id=?")
            .bind(now.to_rfc3339()).bind(&lease_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE verification_result_envelopes SET status='ingested',ingested_at=? WHERE verification_result_envelope_id=?")
            .bind(now.to_rfc3339()).bind(envelope_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE worker_instances SET status='exited',exited_at=? WHERE worker_instance_id=(SELECT worker_instance_id FROM verification_attempts WHERE attempt_id=?)")
            .bind(now.to_rfc3339()).bind(&attempt_id).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &project_id).await?;
        let event = append_event(
            &mut tx,
            &project_id,
            revision,
            "verification.worker.result_ingested",
            entity("verification_result_envelope", envelope_id),
            json!({"case_id":case_id,"attempt_id":attempt_id}),
            Some(entity("verification_task_lease", &lease_id)),
        )
        .await?;
        tx.commit().await?;
        Ok((payload, vec![event]))
    }

    pub async fn fail_verification_worker(
        &self,
        offer: &VerificationWorkerOffer,
        lease: Option<&VerificationWorkerLease>,
        reason: &str,
    ) -> StorageResult<Vec<DomainEvent>> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "fail_verification_worker",
            )
            .await?;
        let signature = hex::encode(Sha256::digest(
            reason
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
                .as_bytes(),
        ));
        let now = Utc::now();
        let mut tx = self.pool().begin().await?;
        sqlx::query("UPDATE verification_attempts SET status='failed',failure_signature=?,error_kind='runtime_unavailable',error_message=?,completed_at=? WHERE attempt_id=? AND status NOT IN ('completed','failed')")
            .bind(&signature).bind(reason).bind(now.to_rfc3339()).bind(&offer.attempt_id).execute(&mut *tx).await?;
        if let Some(lease) = lease {
            sqlx::query("UPDATE verification_task_leases SET status='failed',completed_at=? WHERE verification_lease_id=? AND status='active'")
                .bind(now.to_rfc3339()).bind(&lease.verification_lease_id).execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE worker_instances SET status='exited',quarantine_reason=?,exited_at=? WHERE worker_instance_id=?")
            .bind(reason).bind(now.to_rfc3339()).bind(&offer.worker_instance_id).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &offer.project_id).await?;
        let event = append_event(
            &mut tx,
            &offer.project_id,
            revision,
            "verification.worker.failed",
            entity("verification_attempt", &offer.attempt_id),
            json!({"case_id":offer.case_id,"failure_signature":signature,"reason":reason}),
            Some(entity("worker_instance", &offer.worker_instance_id)),
        )
        .await?;
        tx.commit().await?;
        Ok(vec![event])
    }
}

async fn validate_lease(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    lease: &VerificationWorkerLease,
) -> StorageResult<()> {
    let valid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM verification_task_leases l JOIN verification_attempts a ON a.attempt_id=l.attempt_id JOIN verification_cases c ON c.case_id=l.case_id WHERE l.verification_lease_id=? AND l.attempt_id=? AND l.worker_instance_id=? AND l.lease_epoch=? AND l.lease_token_hash=? AND l.status='active' AND l.expires_at>=? AND a.status='running' AND c.cancellation_epoch=?")
        .bind(&lease.verification_lease_id).bind(&lease.attempt_id).bind(&lease.worker_instance_id).bind(lease.lease_epoch)
        .bind(hash_secret(&lease.lease_token)).bind(Utc::now().to_rfc3339()).bind(lease.cancellation_epoch)
        .fetch_one(&mut **tx).await?;
    if valid != 1 {
        return Err(StorageError::LateSubmission(
            "verification lease is stale or expired".into(),
        ));
    }
    Ok(())
}

fn hash_json(value: &Value) -> StorageResult<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}
fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}
