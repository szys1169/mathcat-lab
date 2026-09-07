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
            "SELECT vc.project_id,vc.stage,vc.cancellation_epoch,vc.snapshot_id,vs.content_hash AS snapshot_hash,vp.max_attempts,p.status AS project_status FROM verification_cases vc JOIN verification_policies vp ON vp.policy_id=vc.policy_id JOIN projects p ON p.project_id=vc.project_id LEFT JOIN verification_snapshots vs ON vs.snapshot_id=vc.snapshot_id AND vs.case_id=vc.case_id WHERE vc.case_id=?",
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
        let snapshot_id: Option<String> = case.try_get("snapshot_id")?;
        let snapshot_hash: Option<String> = case.try_get("snapshot_hash")?;
        let max_attempts: i64 = case.try_get("max_attempts")?;
        let project_status: String = case.try_get("project_status")?;
        if !crate::verification_case_execution_is_released(&mut tx, case_id).await? {
            return Err(StorageError::InvalidTransition(format!(
                "verification workers cannot be offered while project is {project_status}"
            )));
        }
        if max_attempts < 1 {
            return Err(StorageError::CorruptData(format!(
                "verification case {case_id} has invalid max_attempts {max_attempts}"
            )));
        }
        if verification_stage_is_terminal(&stage) {
            return Err(StorageError::InvalidTransition(format!(
                "verification case is {stage}"
            )));
        }
        let (snapshot_id, snapshot_hash) = snapshot_id.zip(snapshot_hash).ok_or_else(|| {
            StorageError::InvalidTransition(format!(
                "verification case {case_id} has no immutable snapshot"
            ))
        })?;
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence),0)+1 FROM verification_attempts WHERE case_id=?",
        )
        .bind(case_id)
        .fetch_one(&mut *tx)
        .await?;
        let attempts_for_kind: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM verification_attempts WHERE case_id=? AND kind=?",
        )
        .bind(case_id)
        .bind(kind)
        .fetch_one(&mut *tx)
        .await?;
        if attempts_for_kind >= max_attempts {
            return Err(StorageError::BudgetExhausted(format!(
                "verification case {case_id} exhausted {max_attempts} attempt(s) for {kind}"
            )));
        }
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
            "retry_policy":{"max_attempts":max_attempts,"same_failure_signature_retries":1},
            "cancellation_epoch":cancellation_epoch,
            "snapshot_id":snapshot_id,
            "snapshot_hash":snapshot_hash,
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
        let current = sqlx::query("SELECT vc.cancellation_epoch,vc.stage,p.status AS project_status FROM verification_cases vc JOIN projects p ON p.project_id=vc.project_id WHERE vc.case_id=? AND vc.project_id=?")
            .bind(&offer.case_id)
            .bind(&offer.project_id)
            .fetch_one(&mut *tx)
            .await?;
        let current_epoch: i64 = current.try_get("cancellation_epoch")?;
        let stage: String = current.try_get("stage")?;
        let project_status: String = current.try_get("project_status")?;
        if current_epoch != offer.cancellation_epoch {
            return Err(StorageError::LateSubmission(
                "verification case epoch changed before handshake".into(),
            ));
        }
        if !crate::verification_case_execution_is_released(&mut tx, &offer.case_id).await?
            || verification_stage_is_terminal(&stage)
        {
            return Err(StorageError::LateSubmission(format!(
                "verification handshake crossed a lifecycle boundary: project={project_status}, case={stage}"
            )));
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
        let status: String = row.try_get("status")?;
        let envelope_json: String = row.try_get("envelope_json")?;
        let payload = serde_json::from_str::<Value>(&envelope_json);
        if status == "ingested" {
            let payload = payload?;
            let expected_content_hash: String = row.try_get("content_hash")?;
            if hash_json(&payload)? != expected_content_hash {
                return Err(StorageError::CorruptData(format!(
                    "ingested verification result {envelope_id} failed its content hash"
                )));
            }
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
        let expected_content_hash: String = row.try_get("content_hash")?;
        let mut rejection_reason = match &payload {
            Ok(payload) if hash_json(payload)? == expected_content_hash => None,
            Ok(_) => Some("verification result content hash mismatch".to_owned()),
            Err(error) => Some(format!(
                "verification result payload is invalid JSON: {error}"
            )),
        };
        let chain = sqlx::query(
            "SELECT a.case_id AS attempt_case_id,a.kind AS attempt_kind,a.status AS attempt_status,a.cancellation_epoch AS attempt_cancellation_epoch,a.lease_epoch AS attempt_lease_epoch,a.input_hash,a.output_hash,a.context_packet_id,l.project_id AS lease_project_id,l.case_id AS lease_case_id,l.attempt_id AS lease_attempt_id,l.lease_epoch AS stored_lease_epoch,l.cancellation_epoch AS lease_cancellation_epoch,l.status AS lease_status,c.project_id AS case_project_id,c.snapshot_id,c.cancellation_epoch AS case_cancellation_epoch,c.stage AS case_stage,s.content_hash AS snapshot_hash,cp.project_id AS context_project_id,cp.content_json AS context_json,cp.content_hash AS context_hash,cp.status AS context_status,tc.project_id AS contract_project_id,tc.case_id AS contract_case_id,tc.contract_json,tc.content_hash AS contract_hash FROM verification_attempts a JOIN verification_task_leases l ON l.verification_lease_id=? JOIN verification_cases c ON c.case_id=? LEFT JOIN verification_snapshots s ON s.snapshot_id=c.snapshot_id AND s.case_id=c.case_id LEFT JOIN context_packets cp ON cp.context_packet_id=a.context_packet_id LEFT JOIN verification_task_contracts tc ON tc.attempt_id=a.attempt_id WHERE a.attempt_id=?",
        )
        .bind(&lease_id)
        .bind(&case_id)
        .bind(&attempt_id)
        .fetch_optional(&mut *tx)
        .await?;
        if rejection_reason.is_none() {
            rejection_reason = match chain.as_ref() {
                None => Some("verification result ownership chain is missing".to_owned()),
                Some(chain) => validate_result_ownership(
                    chain,
                    &project_id,
                    &case_id,
                    &attempt_id,
                    lease_epoch,
                    cancellation_epoch,
                    &expected_content_hash,
                )?,
            };
        }
        if let Some(reason) = rejection_reason {
            sqlx::query("UPDATE verification_result_envelopes SET status='stale',rejection_reason=? WHERE verification_result_envelope_id=? AND status='submitted'")
                .bind(&reason).bind(envelope_id).execute(&mut *tx).await?;
            let revision = bump_revision(&mut tx, &project_id).await?;
            append_event(
                &mut tx,
                &project_id,
                revision,
                "verification.worker.result_rejected",
                entity("verification_result_envelope", envelope_id),
                json!({
                    "case_id":case_id,
                    "attempt_id":attempt_id,
                    "lease_epoch":lease_epoch,
                    "cancellation_epoch":cancellation_epoch,
                    "reason":reason,
                }),
                Some(entity("verification_task_lease", &lease_id)),
            )
            .await?;
            tx.commit().await?;
            return Err(StorageError::LateSubmission(reason));
        }
        let payload = payload.map_err(|error| {
            StorageError::CorruptData(format!(
                "validated verification result payload could not be decoded: {error}"
            ))
        })?;
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

    pub async fn replay_ingested_verification_result(
        &self,
        case_id: &str,
        kind: &str,
        context: &Value,
    ) -> StorageResult<Option<(String, Value)>> {
        let expected_context_hash = hash_json(context)?;
        let row = sqlx::query(
            "SELECT e.attempt_id,e.envelope_json,e.content_hash FROM verification_result_envelopes e JOIN verification_attempts a ON a.attempt_id=e.attempt_id JOIN verification_cases c ON c.case_id=e.case_id JOIN verification_task_leases l ON l.verification_lease_id=e.verification_lease_id JOIN verification_task_contracts tc ON tc.attempt_id=a.attempt_id JOIN verification_snapshots s ON s.snapshot_id=c.snapshot_id AND s.case_id=c.case_id JOIN context_packets cp ON cp.context_packet_id=a.context_packet_id WHERE e.case_id=? AND a.kind=? AND cp.content_hash=? AND e.status='ingested' AND a.status='completed' AND l.status='completed' AND e.project_id=c.project_id AND e.cancellation_epoch=c.cancellation_epoch AND e.cancellation_epoch=a.cancellation_epoch AND e.cancellation_epoch=l.cancellation_epoch AND e.lease_epoch=a.lease_epoch AND e.lease_epoch=l.lease_epoch AND json_extract(tc.contract_json,'$.snapshot_id')=c.snapshot_id AND json_extract(tc.contract_json,'$.snapshot_hash')=s.content_hash ORDER BY e.ingested_at DESC,e.verification_result_envelope_id DESC LIMIT 1",
        )
        .bind(case_id)
        .bind(kind)
        .bind(expected_context_hash)
        .fetch_optional(self.pool())
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let attempt_id: String = row.try_get("attempt_id")?;
        let payload: Value = serde_json::from_str(row.try_get("envelope_json")?)?;
        let expected_hash: String = row.try_get("content_hash")?;
        if hash_json(&payload)? != expected_hash {
            return Err(StorageError::CorruptData(format!(
                "ingested verification result for attempt {attempt_id} failed its content hash"
            )));
        }
        Ok(Some((attempt_id, payload)))
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
        let row = sqlx::query(
            "SELECT a.status AS attempt_status,a.failure_signature,a.sequence,a.kind,\
                    a.cancellation_epoch AS attempt_cancellation_epoch,a.lease_epoch AS attempt_lease_epoch,\
                    a.worker_instance_id,c.project_id,c.stage AS case_stage,\
                    c.cancellation_epoch AS case_cancellation_epoch,p.status AS project_status,\
                    wi.status AS worker_instance_status \
             FROM verification_attempts a \
             JOIN verification_cases c ON c.case_id=a.case_id \
             JOIN projects p ON p.project_id=c.project_id \
             JOIN worker_instances wi ON wi.worker_instance_id=a.worker_instance_id \
             WHERE a.attempt_id=? AND a.case_id=?",
        )
        .bind(&offer.attempt_id)
        .bind(&offer.case_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            StorageError::LateSubmission(
                "verification attempt no longer exists in this case scope".into(),
            )
        })?;
        let attempt_status: String = row.try_get("attempt_status")?;
        let recorded_signature: Option<String> = row.try_get("failure_signature")?;
        if attempt_status == "failed" {
            tx.rollback().await?;
            if recorded_signature.as_deref() == Some(signature.as_str()) {
                return Ok(Vec::new());
            }
            return Err(StorageError::LateSubmission(
                "verification attempt already failed with a different reason".into(),
            ));
        }

        let sequence: i64 = row.try_get("sequence")?;
        let kind: String = row.try_get("kind")?;
        let attempt_cancellation_epoch: i64 = row.try_get("attempt_cancellation_epoch")?;
        let attempt_lease_epoch: i64 = row.try_get("attempt_lease_epoch")?;
        let worker_instance_id: String = row.try_get("worker_instance_id")?;
        let project_id: String = row.try_get("project_id")?;
        let case_stage: String = row.try_get("case_stage")?;
        let case_cancellation_epoch: i64 = row.try_get("case_cancellation_epoch")?;
        let project_status: String = row.try_get("project_status")?;
        let worker_instance_status: String = row.try_get("worker_instance_status")?;
        let route_released =
            crate::verification_case_execution_is_released(&mut tx, &offer.case_id).await?;
        let latest_sequence: i64 = sqlx::query_scalar(
            "SELECT MAX(sequence) FROM verification_attempts WHERE case_id=? AND kind=?",
        )
        .bind(&offer.case_id)
        .bind(&kind)
        .fetch_one(&mut *tx)
        .await?;
        let expected_attempt_status = if lease.is_some() {
            "running"
        } else {
            "offered"
        };
        let expected_worker_status = if lease.is_some() {
            "running"
        } else {
            "handshaking"
        };
        let identity_matches = project_id == offer.project_id
            && kind == offer.kind
            && sequence == offer.sequence
            && attempt_cancellation_epoch == offer.cancellation_epoch
            && attempt_lease_epoch == offer.lease_epoch
            && worker_instance_id == offer.worker_instance_id;
        if !route_released
            || verification_stage_is_terminal(&case_stage)
            || case_cancellation_epoch != offer.cancellation_epoch
            || latest_sequence != offer.sequence
            || attempt_status != expected_attempt_status
            || worker_instance_status != expected_worker_status
            || !identity_matches
        {
            return Err(StorageError::LateSubmission(format!(
                "verification failure is stale: project={project_status}, case={case_stage}/{case_cancellation_epoch}, attempt={attempt_status}/{sequence}, latest={latest_sequence}, worker={worker_instance_status}"
            )));
        }

        if let Some(lease) = lease {
            if lease.project_id != offer.project_id
                || lease.case_id != offer.case_id
                || lease.attempt_id != offer.attempt_id
                || lease.worker_instance_id != offer.worker_instance_id
                || lease.lease_epoch != offer.lease_epoch
                || lease.cancellation_epoch != offer.cancellation_epoch
            {
                return Err(StorageError::LateSubmission(
                    "verification failure lease does not match its offer".into(),
                ));
            }
            let lease_row = sqlx::query(
                "SELECT project_id,case_id,attempt_id,worker_instance_id,lease_epoch,\
                        cancellation_epoch,lease_token_hash,status \
                 FROM verification_task_leases WHERE verification_lease_id=?",
            )
            .bind(&lease.verification_lease_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                StorageError::LateSubmission("verification failure lease no longer exists".into())
            })?;
            let lease_matches = lease_row.try_get::<String, _>("project_id")? == offer.project_id
                && lease_row.try_get::<String, _>("case_id")? == offer.case_id
                && lease_row.try_get::<String, _>("attempt_id")? == offer.attempt_id
                && lease_row.try_get::<String, _>("worker_instance_id")?
                    == offer.worker_instance_id
                && lease_row.try_get::<i64, _>("lease_epoch")? == offer.lease_epoch
                && lease_row.try_get::<i64, _>("cancellation_epoch")? == offer.cancellation_epoch
                && lease_row.try_get::<String, _>("lease_token_hash")?
                    == hash_secret(&lease.lease_token)
                && lease_row.try_get::<String, _>("status")? == "active";
            if !lease_matches {
                return Err(StorageError::LateSubmission(
                    "verification failure lease token, epoch, ownership, or lifecycle is stale"
                        .into(),
                ));
            }
        } else {
            let lease_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM verification_task_leases WHERE attempt_id=?",
            )
            .bind(&offer.attempt_id)
            .fetch_one(&mut *tx)
            .await?;
            if lease_count != 0 {
                return Err(StorageError::LateSubmission(
                    "unleased verification failure cannot replace an existing lease".into(),
                ));
            }
        }

        let attempt_update = sqlx::query("UPDATE verification_attempts SET status='failed',failure_signature=?,error_kind='runtime_unavailable',error_message=?,completed_at=? WHERE attempt_id=? AND case_id=? AND status=? AND sequence=? AND cancellation_epoch=? AND lease_epoch=?")
            .bind(&signature)
            .bind(reason)
            .bind(now.to_rfc3339())
            .bind(&offer.attempt_id)
            .bind(&offer.case_id)
            .bind(expected_attempt_status)
            .bind(offer.sequence)
            .bind(offer.cancellation_epoch)
            .bind(offer.lease_epoch)
            .execute(&mut *tx)
            .await?;
        if attempt_update.rows_affected() != 1 {
            return Err(StorageError::LateSubmission(
                "verification attempt changed while its failure was being recorded".into(),
            ));
        }
        if let Some(lease) = lease {
            let lease_update = sqlx::query("UPDATE verification_task_leases SET status='failed',completed_at=? WHERE verification_lease_id=? AND project_id=? AND case_id=? AND attempt_id=? AND worker_instance_id=? AND lease_epoch=? AND cancellation_epoch=? AND lease_token_hash=? AND status='active'")
                .bind(now.to_rfc3339())
                .bind(&lease.verification_lease_id)
                .bind(&offer.project_id)
                .bind(&offer.case_id)
                .bind(&offer.attempt_id)
                .bind(&offer.worker_instance_id)
                .bind(offer.lease_epoch)
                .bind(offer.cancellation_epoch)
                .bind(hash_secret(&lease.lease_token))
                .execute(&mut *tx)
                .await?;
            if lease_update.rows_affected() != 1 {
                return Err(StorageError::LateSubmission(
                    "verification lease changed while its failure was being recorded".into(),
                ));
            }
        }
        let worker_update = sqlx::query("UPDATE worker_instances SET status='exited',quarantine_reason=?,exited_at=? WHERE worker_instance_id=? AND project_id=? AND status=?")
            .bind(reason)
            .bind(now.to_rfc3339())
            .bind(&offer.worker_instance_id)
            .bind(&offer.project_id)
            .bind(expected_worker_status)
            .execute(&mut *tx)
            .await?;
        if worker_update.rows_affected() != 1 {
            return Err(StorageError::LateSubmission(
                "verification worker changed while its failure was being recorded".into(),
            ));
        }
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

#[allow(clippy::too_many_arguments)]
fn validate_result_ownership(
    row: &sqlx::sqlite::SqliteRow,
    project_id: &str,
    case_id: &str,
    attempt_id: &str,
    lease_epoch: i64,
    cancellation_epoch: i64,
    result_content_hash: &str,
) -> StorageResult<Option<String>> {
    let attempt_case_id: String = row.try_get("attempt_case_id")?;
    let attempt_kind: String = row.try_get("attempt_kind")?;
    let attempt_status: String = row.try_get("attempt_status")?;
    let attempt_cancellation_epoch: i64 = row.try_get("attempt_cancellation_epoch")?;
    let attempt_lease_epoch: i64 = row.try_get("attempt_lease_epoch")?;
    let input_hash: String = row.try_get("input_hash")?;
    let output_hash: Option<String> = row.try_get("output_hash")?;
    let context_packet_id: Option<String> = row.try_get("context_packet_id")?;
    let lease_project_id: String = row.try_get("lease_project_id")?;
    let lease_case_id: String = row.try_get("lease_case_id")?;
    let lease_attempt_id: String = row.try_get("lease_attempt_id")?;
    let stored_lease_epoch: i64 = row.try_get("stored_lease_epoch")?;
    let lease_cancellation_epoch: i64 = row.try_get("lease_cancellation_epoch")?;
    let lease_status: String = row.try_get("lease_status")?;
    let case_project_id: String = row.try_get("case_project_id")?;
    let snapshot_id: Option<String> = row.try_get("snapshot_id")?;
    let case_cancellation_epoch: i64 = row.try_get("case_cancellation_epoch")?;
    let case_stage: String = row.try_get("case_stage")?;
    let snapshot_hash: Option<String> = row.try_get("snapshot_hash")?;
    let context_project_id: Option<String> = row.try_get("context_project_id")?;
    let context_json: Option<String> = row.try_get("context_json")?;
    let context_hash: Option<String> = row.try_get("context_hash")?;
    let context_status: Option<String> = row.try_get("context_status")?;
    let contract_project_id: Option<String> = row.try_get("contract_project_id")?;
    let contract_case_id: Option<String> = row.try_get("contract_case_id")?;
    let contract_json: Option<String> = row.try_get("contract_json")?;
    let contract_hash: Option<String> = row.try_get("contract_hash")?;

    if attempt_case_id != case_id
        || lease_project_id != project_id
        || lease_case_id != case_id
        || lease_attempt_id != attempt_id
        || case_project_id != project_id
        || context_project_id.as_deref() != Some(project_id)
        || contract_project_id.as_deref() != Some(project_id)
        || contract_case_id.as_deref() != Some(case_id)
    {
        return Ok(Some(
            "verification result case or project ownership changed".into(),
        ));
    }
    if attempt_status != "result_submitted"
        || !matches!(lease_status.as_str(), "active" | "expired")
    {
        return Ok(Some(
            "verification attempt or lease is not awaiting ingestion".into(),
        ));
    }
    if attempt_lease_epoch != lease_epoch
        || stored_lease_epoch != lease_epoch
        || attempt_cancellation_epoch != cancellation_epoch
        || lease_cancellation_epoch != cancellation_epoch
        || case_cancellation_epoch != cancellation_epoch
    {
        return Ok(Some(
            "verification lease or cancellation epoch changed".into(),
        ));
    }
    if output_hash.as_deref() != Some(result_content_hash) {
        return Ok(Some(
            "verification attempt output hash does not match its result envelope".into(),
        ));
    }
    if verification_stage_is_terminal(&case_stage) {
        return Ok(Some(format!(
            "verification case is already terminal ({case_stage})"
        )));
    }
    let (Some(snapshot_id), Some(snapshot_hash)) = (snapshot_id, snapshot_hash) else {
        return Ok(Some(
            "verification result is not bound to an immutable snapshot".into(),
        ));
    };
    let (Some(context_packet_id), Some(context_json), Some(context_hash)) =
        (context_packet_id, context_json, context_hash)
    else {
        return Ok(Some("verification result context packet is missing".into()));
    };
    if context_status.as_deref() != Some("active") {
        return Ok(Some(
            "verification result context packet is no longer active".into(),
        ));
    }
    let context: Value = match serde_json::from_str(&context_json) {
        Ok(context) => context,
        Err(error) => {
            return Ok(Some(format!(
                "verification context packet is invalid JSON: {error}"
            )));
        }
    };
    if hash_json(&context)? != context_hash {
        return Ok(Some(
            "verification context packet content hash mismatch".into(),
        ));
    }
    let (Some(contract_json), Some(contract_hash)) = (contract_json, contract_hash) else {
        return Ok(Some("verification task contract is missing".into()));
    };
    let contract: Value = match serde_json::from_str(&contract_json) {
        Ok(contract) => contract,
        Err(error) => {
            return Ok(Some(format!(
                "verification task contract is invalid JSON: {error}"
            )));
        }
    };
    if hash_json(&contract)? != contract_hash {
        return Ok(Some(
            "verification task contract content hash mismatch".into(),
        ));
    }
    let contract_matches = contract.get("case_id").and_then(Value::as_str) == Some(case_id)
        && contract.get("attempt_id").and_then(Value::as_str) == Some(attempt_id)
        && contract.get("task_kind").and_then(Value::as_str) == Some(attempt_kind.as_str())
        && contract.get("context_packet_id").and_then(Value::as_str)
            == Some(context_packet_id.as_str())
        && contract.get("cancellation_epoch").and_then(Value::as_i64) == Some(cancellation_epoch)
        && contract.get("snapshot_id").and_then(Value::as_str) == Some(snapshot_id.as_str())
        && contract.get("snapshot_hash").and_then(Value::as_str) == Some(snapshot_hash.as_str());
    if !contract_matches {
        return Ok(Some(
            "verification task contract no longer matches its case, snapshot, or attempt".into(),
        ));
    }
    let expected_input_hash = hash_json(&json!({
        "context_hash": context_hash,
        "contract_hash": contract_hash,
    }))?;
    if input_hash != expected_input_hash {
        return Ok(Some("verification attempt input hash mismatch".into()));
    }
    Ok(None)
}

async fn validate_lease(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    lease: &VerificationWorkerLease,
) -> StorageResult<()> {
    if !crate::verification_case_execution_is_released(tx, &lease.case_id).await? {
        return Err(StorageError::LateSubmission(
            "verification route is no longer released".into(),
        ));
    }
    let valid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM verification_task_leases l JOIN verification_attempts a ON a.attempt_id=l.attempt_id JOIN verification_cases c ON c.case_id=l.case_id WHERE l.verification_lease_id=? AND l.attempt_id=? AND l.worker_instance_id=? AND l.lease_epoch=? AND l.lease_token_hash=? AND l.status='active' AND l.expires_at>=? AND a.status='running' AND c.cancellation_epoch=? AND c.stage NOT IN ('committed','rejected','unknown','failed','cancelled')")
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

fn verification_stage_is_terminal(stage: &str) -> bool {
    matches!(
        stage,
        "committed" | "rejected" | "unknown" | "failed" | "cancelled"
    )
}
