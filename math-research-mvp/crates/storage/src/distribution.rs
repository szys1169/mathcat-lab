use chrono::{DateTime, Utc};
use research_domain::{
    DomainEvent, Task, TaskLease, TaskSteer, Verification, WorkerNode, WorkerOutput,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{
    LocalResultSubmission, LocalTaskLease, SqliteStore, StorageError, StorageResult, append_event,
    bump_revision, entity, json_text,
    reliability_v2::worker_result_envelope_identity,
    route_mutations::{TaskLeaseExpiry, expire_task_lease_tx},
    rows,
};

#[derive(Debug)]
pub struct TaskLeaseCompletion {
    pub events: Vec<DomainEvent>,
    pub verifications: Vec<Verification>,
}

/// Authenticated, fenced input for committing one distributed worker result.
pub struct TaskLeaseCompletionRequest<'a> {
    pub lease_id: &'a str,
    pub node_id: &'a str,
    pub token: &'a str,
    pub node_epoch: i64,
    pub lease_epoch: i64,
    pub output: &'a WorkerOutput,
    pub incorporated_steer_ids: &'a [String],
}

impl SqliteStore {
    pub async fn register_worker_node(
        &self,
        node_id: &str,
        display_name: &str,
        capabilities: Value,
        token: &str,
    ) -> StorageResult<WorkerNode> {
        if node_id.trim().is_empty() || display_name.trim().is_empty() || token.len() < 16 {
            return Err(StorageError::InvalidTransition(
                "node registration requires identifiers and a token of at least 16 characters"
                    .into(),
            ));
        }
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "register_worker_node",
            )
            .await?;
        let now = Utc::now();
        sqlx::query("INSERT INTO worker_nodes(node_id,display_name,capabilities_json,token_hash,status,node_epoch,registered_at,last_heartbeat) VALUES(?,?,?,?, 'active',1,?,?)")
            .bind(node_id).bind(display_name).bind(json_text(&capabilities)?).bind(hash_secret(token))
            .bind(now.to_rfc3339()).bind(now.to_rfc3339()).execute(self.pool()).await?;
        Ok(WorkerNode {
            node_id: node_id.into(),
            display_name: display_name.into(),
            capabilities,
            status: "active".into(),
            node_epoch: 1,
            registered_at: now,
            last_heartbeat: now,
        })
    }

    pub async fn heartbeat_worker_node(
        &self,
        node_id: &str,
        token: &str,
        expected_epoch: i64,
    ) -> StorageResult<WorkerNode> {
        authenticate_node(self, node_id, token, expected_epoch).await?;
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::Telemetry,
                "heartbeat_worker_node",
            )
            .await?;
        sqlx::query("UPDATE worker_nodes SET last_heartbeat=?,status='active' WHERE node_id=? AND node_epoch=?")
            .bind(Utc::now().to_rfc3339()).bind(node_id).bind(expected_epoch).execute(self.pool()).await?;
        self.get_worker_node(node_id).await
    }

    pub async fn get_worker_node(&self, node_id: &str) -> StorageResult<WorkerNode> {
        let row = sqlx::query("SELECT * FROM worker_nodes WHERE node_id=?")
            .bind(node_id)
            .fetch_optional(self.pool())
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "worker_node",
                id: node_id.into(),
            })?;
        worker_node_from_row(&row)
    }

    pub async fn lease_next_task(
        &self,
        project_id: &str,
        node_id: &str,
        token: &str,
        expected_node_epoch: i64,
        ttl_seconds: u64,
    ) -> StorageResult<(Option<(TaskLease, Task)>, Vec<DomainEvent>)> {
        authenticate_node(self, node_id, token, expected_node_epoch).await?;
        let node = self.get_worker_node(node_id).await?;
        let roles = node
            .capabilities
            .get("roles")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut events = self.expire_distributed_task_leases(project_id).await?;
        let task_rows = sqlx::query("SELECT t.* FROM tasks t JOIN routes r ON t.route_id=r.route_id JOIN projects p ON p.project_id=t.project_id WHERE t.project_id=? AND t.status IN ('queued','open','assigned') AND r.status='active' AND p.status IN ('running','needs_human_review') AND r.human_review IN ('approved','not_required') AND t.context_packet_id IS NOT NULL AND EXISTS (SELECT 1 FROM context_packets c WHERE c.context_packet_id=t.context_packet_id AND c.status='active') AND EXISTS (SELECT 1 FROM task_contracts c WHERE c.task_id=t.task_id) AND (SELECT COUNT(*) FROM task_attempts a WHERE a.task_id=t.task_id)<3 AND NOT EXISTS (SELECT 1 FROM task_leases l WHERE l.task_id=t.task_id AND l.status='active') ORDER BY t.priority DESC,t.task_id")
            .bind(project_id).fetch_all(self.pool()).await?;
        for selected in task_rows.iter().filter(|row| {
            let role = row.try_get::<String, _>("worker_role").unwrap_or_default();
            roles.is_empty() || roles.contains(&role.as_str())
        }) {
            let task = rows::task(selected)?;
            let offer = match self
                .offer_local_task(
                    &task,
                    "distributed_worker",
                    None,
                    &format!("distributed:{node_id}"),
                    node.capabilities.clone(),
                )
                .await
            {
                Ok(offer) => offer,
                Err(StorageError::InvalidTransition(reason))
                    if reason.contains("cannot be offered") =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            };
            events.extend(offer.events.clone());
            let local_lease = match self
                .accept_distributed_handshake(
                    &offer,
                    (node_id, token, expected_node_epoch),
                    Some("distributed-worker-v2"),
                    json!({
                        "node_id": node_id,
                        "node_epoch": expected_node_epoch,
                        "context_hash": offer.context_packet.content_hash,
                        "task_contract_hash": offer.contract.content_hash,
                    }),
                    ttl_seconds,
                )
                .await
            {
                Ok(lease) => lease,
                Err(error) => {
                    if let Ok(failure_events) = self
                        .fail_local_attempt(
                            None,
                            &offer,
                            &format!("distributed worker handshake failed: {error}"),
                        )
                        .await
                    {
                        events.extend(failure_events);
                    }
                    return Err(error);
                }
            };
            events.extend(local_lease.events.clone());
            let lease = self.get_task_lease(&local_lease.lease_id).await?;
            return Ok((Some((lease, local_lease.task)), events));
        }
        Ok((None, events))
    }

    pub async fn renew_task_lease(
        &self,
        lease_id: &str,
        node_id: &str,
        token: &str,
        node_epoch: i64,
        lease_epoch: i64,
        ttl_seconds: u64,
    ) -> StorageResult<TaskLease> {
        authenticate_node(self, node_id, token, node_epoch).await?;
        let (_, local_lease) =
            load_distributed_lease(self, lease_id, node_id, token, lease_epoch, true).await?;
        self.heartbeat_local_lease(&local_lease, ttl_seconds)
            .await?;
        self.get_task_lease(lease_id).await
    }

    pub async fn get_task_lease(&self, lease_id: &str) -> StorageResult<TaskLease> {
        let row = sqlx::query("SELECT * FROM task_leases WHERE lease_id=?")
            .bind(lease_id)
            .fetch_optional(self.pool())
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "task_lease",
                id: lease_id.into(),
            })?;
        task_lease_from_row(&row)
    }

    pub async fn complete_task_lease(
        &self,
        request: TaskLeaseCompletionRequest<'_>,
    ) -> StorageResult<TaskLeaseCompletion> {
        let TaskLeaseCompletionRequest {
            lease_id,
            node_id,
            token,
            node_epoch,
            lease_epoch,
            output,
            incorporated_steer_ids,
        } = request;
        authenticate_node(self, node_id, token, node_epoch).await?;
        let (lease, lease_view) =
            load_distributed_lease(self, lease_id, node_id, token, lease_epoch, false).await?;
        let (_, _, idempotency_key) =
            worker_result_envelope_identity(&lease_view.attempt_id, output)?;
        if let Some(existing) = sqlx::query_scalar::<_, String>(
            "SELECT result_envelope_id FROM result_envelopes WHERE project_id=? AND idempotency_key=?",
        )
        .bind(&lease.project_id)
        .bind(&idempotency_key)
        .fetch_optional(self.pool())
        .await?
        {
            let ingestion = self.ingest_local_result_envelope(&existing).await?;
            return Ok(TaskLeaseCompletion {
                events: ingestion.events,
                verifications: ingestion.verifications,
            });
        }
        let (_, current_lease_view) =
            load_distributed_lease(self, lease_id, node_id, token, lease_epoch, true).await?;
        let submitted = self
            .submit_local_result_envelope_after_steers(
                &current_lease_view,
                output,
                incorporated_steer_ids,
            )
            .await?;
        let (result_envelope_id, mut events) = match submitted {
            LocalResultSubmission::Submitted {
                result_envelope_id,
                events,
            } => (result_envelope_id, events),
            LocalResultSubmission::SteeringPending { steers } => {
                return Err(StorageError::InvalidTransition(format!(
                    "result omitted {} pending steer(s): {}",
                    steers.len(),
                    steers
                        .iter()
                        .map(|steer| steer.steer_id.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                )));
            }
        };
        let ingestion = self
            .ingest_local_result_envelope(&result_envelope_id)
            .await?;
        events.extend(ingestion.events);
        Ok(TaskLeaseCompletion {
            events,
            verifications: ingestion.verifications,
        })
    }

    pub async fn pending_distributed_task_steers(
        &self,
        lease_id: &str,
        node_id: &str,
        token: &str,
        node_epoch: i64,
        lease_epoch: i64,
    ) -> StorageResult<Vec<TaskSteer>> {
        authenticate_node(self, node_id, token, node_epoch).await?;
        let (_, lease) =
            load_distributed_lease(self, lease_id, node_id, token, lease_epoch, true).await?;
        let (steers, _) = self
            .pending_task_steers(
                &lease.task.project_id,
                &lease.task.task_id,
                lease.task.revision,
                lease.task.route_cancellation_epoch,
            )
            .await?;
        Ok(steers)
    }

    async fn expire_distributed_task_leases(
        &self,
        project_id: &str,
    ) -> StorageResult<Vec<DomainEvent>> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "expire_distributed_task_leases",
            )
            .await?;
        let now = Utc::now();
        let mut tx = self.pool().begin().await?;
        let expired = sqlx::query("SELECT lease_id,task_id,attempt_id,worker_instance_id FROM task_leases WHERE project_id=? AND status='active' AND expires_at<?")
            .bind(project_id).bind(now.to_rfc3339()).fetch_all(&mut *tx).await?;
        if expired.is_empty() {
            tx.rollback().await?;
            return Ok(Vec::new());
        }
        let mut event_data = Vec::with_capacity(expired.len());
        for row in expired {
            let lease_id: String = row.try_get("lease_id")?;
            let task_id: String = row.try_get("task_id")?;
            let attempt_id: Option<String> = row.try_get("attempt_id")?;
            let worker_instance_id: Option<String> = row.try_get("worker_instance_id")?;
            let Some(outcome) = expire_task_lease_tx(
                &mut tx,
                TaskLeaseExpiry {
                    project: project_id,
                    lease: &lease_id,
                    task: &task_id,
                    attempt: attempt_id.as_deref(),
                    worker_instance: worker_instance_id.as_deref(),
                },
                &now,
            )
            .await?
            else {
                continue;
            };
            event_data.push((lease_id, task_id, attempt_id, outcome.replayable_result));
        }
        let revision = bump_revision(&mut tx, project_id).await?;
        let mut events = Vec::with_capacity(event_data.len());
        for (lease_id, task_id, attempt_id, replayable_result) in event_data {
            events.push(append_event(&mut tx, project_id, revision, "task.lease_expired", entity("task_lease", &lease_id), json!({"task_id":task_id,"attempt_id":attempt_id,"replayable_result":replayable_result}), Some(entity("task", &task_id))).await?);
        }
        tx.commit().await?;
        Ok(events)
    }
}

async fn load_distributed_lease(
    store: &SqliteStore,
    lease_id: &str,
    node_id: &str,
    node_token: &str,
    lease_epoch: i64,
    require_current: bool,
) -> StorageResult<(TaskLease, LocalTaskLease)> {
    let row = sqlx::query("SELECT * FROM task_leases WHERE lease_id=?")
        .bind(lease_id)
        .fetch_optional(store.pool())
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "task_lease",
            id: lease_id.into(),
        })?;
    let lease = task_lease_from_row(&row)?;
    let attempt_id: Option<String> = row.try_get("attempt_id")?;
    let worker_instance_id: Option<String> = row.try_get("worker_instance_id")?;
    let lease_token_hash: Option<String> = row.try_get("lease_token_hash")?;
    if lease.node_id != node_id
        || lease.lease_epoch != lease_epoch
        || lease_token_hash.as_deref() != Some(hash_secret(node_token).as_str())
    {
        return Err(StorageError::LateSubmission(
            "lease token, epoch, or owning node is stale".into(),
        ));
    }
    let attempt_id = attempt_id.ok_or_else(|| {
        StorageError::LateSubmission("distributed lease has no V2 task attempt".into())
    })?;
    let worker_instance_id = worker_instance_id.ok_or_else(|| {
        StorageError::LateSubmission("distributed lease has no V2 worker instance".into())
    })?;
    let task = store.get_task(&lease.project_id, &lease.task_id).await?;
    if require_current
        && (lease.status != "active"
            || lease.expires_at < Utc::now()
            || task.revision != lease.task_revision
            || task.route_cancellation_epoch != lease.route_epoch)
    {
        return Err(StorageError::LateSubmission(
            "lease is expired or its task revision or route epoch is stale".into(),
        ));
    }
    let local_lease = LocalTaskLease {
        task,
        attempt_id,
        worker_instance_id,
        lease_id: lease.lease_id.clone(),
        lease_token: node_token.into(),
        lease_epoch: lease.lease_epoch,
        expires_at: lease.expires_at,
        events: Vec::new(),
    };
    Ok((lease, local_lease))
}

async fn authenticate_node(
    store: &SqliteStore,
    node_id: &str,
    token: &str,
    expected_epoch: i64,
) -> StorageResult<()> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM worker_nodes WHERE node_id=? AND token_hash=? AND node_epoch=? AND status='active'")
        .bind(node_id).bind(hash_secret(token)).bind(expected_epoch).fetch_one(store.pool()).await?;
    if count != 1 {
        return Err(StorageError::InvalidTransition(
            "invalid worker-node credentials or epoch".into(),
        ));
    }
    Ok(())
}

fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

fn worker_node_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<WorkerNode> {
    Ok(WorkerNode {
        node_id: row.try_get("node_id")?,
        display_name: row.try_get("display_name")?,
        capabilities: serde_json::from_str(row.try_get("capabilities_json")?)?,
        status: row.try_get("status")?,
        node_epoch: row.try_get("node_epoch")?,
        registered_at: timestamp(row.try_get("registered_at")?)?,
        last_heartbeat: timestamp(row.try_get("last_heartbeat")?)?,
    })
}

fn task_lease_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<TaskLease> {
    Ok(TaskLease {
        lease_id: row.try_get("lease_id")?,
        project_id: row.try_get("project_id")?,
        task_id: row.try_get("task_id")?,
        node_id: row.try_get("node_id")?,
        task_revision: row.try_get("task_revision")?,
        route_epoch: row.try_get("route_epoch")?,
        lease_epoch: row.try_get("lease_epoch")?,
        status: row.try_get("status")?,
        leased_at: timestamp(row.try_get("leased_at")?)?,
        expires_at: timestamp(row.try_get("expires_at")?)?,
        completed_at: optional_timestamp(row.try_get("completed_at")?)?,
    })
}

fn timestamp(value: &str) -> StorageResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| StorageError::CorruptData(error.to_string()))
}
fn optional_timestamp(value: Option<String>) -> StorageResult<Option<DateTime<Utc>>> {
    value.as_deref().map(timestamp).transpose()
}
