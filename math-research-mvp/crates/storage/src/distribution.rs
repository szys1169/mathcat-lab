use chrono::{DateTime, Duration, Utc};
use research_domain::{DomainEvent, Task, TaskLease, WorkerNode, WorkerOutput};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{
    SqliteStore, StorageError, StorageResult, append_event, bump_revision, entity, json_text,
    new_id, rows,
};

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
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "lease_distributed_task",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let now = Utc::now();
        let expired = sqlx::query("SELECT lease_id,task_id,task_revision,route_epoch FROM task_leases WHERE project_id=? AND status='active' AND expires_at<?")
            .bind(project_id).bind(now.to_rfc3339()).fetch_all(&mut *tx).await?;
        let mut events = Vec::new();
        for row in &expired {
            let lease_id: String = row.try_get("lease_id")?;
            let task_id: String = row.try_get("task_id")?;
            sqlx::query("UPDATE task_leases SET status='expired' WHERE lease_id=?")
                .bind(&lease_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE tasks SET status=CASE WHEN worker_id IS NULL THEN 'open' ELSE 'assigned' END,revision=revision+1 WHERE task_id=? AND status='running' AND revision=? AND route_cancellation_epoch=?")
                .bind(&task_id).bind(row.try_get::<i64,_>("task_revision")?).bind(row.try_get::<i64,_>("route_epoch")?).execute(&mut *tx).await?;
            sqlx::query(
                "UPDATE workers SET status='idle' WHERE current_task_id=? AND status='running'",
            )
            .bind(&task_id)
            .execute(&mut *tx)
            .await?;
        }
        let task_rows = sqlx::query("SELECT t.* FROM tasks t JOIN routes r ON t.route_id=r.route_id WHERE t.project_id=? AND t.status IN ('open','assigned') AND r.status='active' AND NOT EXISTS (SELECT 1 FROM task_leases l WHERE l.task_id=t.task_id AND l.status='active') ORDER BY t.priority DESC,t.task_id")
            .bind(project_id).fetch_all(&mut *tx).await?;
        let selected = task_rows.iter().find(|row| {
            let role = row.try_get::<String, _>("worker_role").unwrap_or_default();
            roles.is_empty() || roles.contains(&role.as_str())
        });
        let Some(selected) = selected else {
            if expired.is_empty() {
                tx.rollback().await?;
            } else {
                let revision = bump_revision(&mut tx, project_id).await?;
                events.push(
                    append_event(
                        &mut tx,
                        project_id,
                        revision,
                        "task_lease.expired_batch",
                        entity("project", project_id),
                        json!({"count":expired.len()}),
                        None,
                    )
                    .await?,
                );
                tx.commit().await?;
            }
            return Ok((None, events));
        };
        let task_id: String = selected.try_get("task_id")?;
        let claimed = sqlx::query("UPDATE tasks SET status='running',revision=revision+1 WHERE task_id=? AND status IN ('open','assigned')")
            .bind(&task_id).execute(&mut *tx).await?;
        if claimed.rows_affected() != 1 {
            return Err(StorageError::LateSubmission(
                "task was claimed by another executor".into(),
            ));
        }
        sqlx::query("UPDATE workers SET status='running',last_heartbeat=? WHERE current_task_id=?")
            .bind(now.to_rfc3339())
            .bind(&task_id)
            .execute(&mut *tx)
            .await?;
        let task_row = sqlx::query("SELECT * FROM tasks WHERE task_id=?")
            .bind(&task_id)
            .fetch_one(&mut *tx)
            .await?;
        let task = rows::task(&task_row)?;
        let lease_epoch: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(lease_epoch),0)+1 FROM task_leases WHERE task_id=?",
        )
        .bind(&task_id)
        .fetch_one(&mut *tx)
        .await?;
        let lease_id = new_id("lease");
        let ttl = i64::try_from(ttl_seconds.clamp(10, 3600)).unwrap_or(3600);
        let expires_at = now + Duration::seconds(ttl);
        sqlx::query("INSERT INTO task_leases(lease_id,project_id,task_id,node_id,task_revision,route_epoch,lease_epoch,status,leased_at,expires_at,completed_at) VALUES(?,?,?,?,?,?,?,'active',?,?,NULL)")
            .bind(&lease_id).bind(project_id).bind(&task_id).bind(node_id).bind(task.revision)
            .bind(task.route_cancellation_epoch).bind(lease_epoch).bind(now.to_rfc3339()).bind(expires_at.to_rfc3339())
            .execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        events.push(append_event(&mut tx, project_id, revision, "task_lease.created", entity("task_lease", &lease_id), json!({"task_id":task_id,"node_id":node_id,"task_revision":task.revision,"route_epoch":task.route_cancellation_epoch,"lease_epoch":lease_epoch,"expires_at":expires_at}), Some(entity("task", &task_id))).await?);
        let lease = TaskLease {
            lease_id,
            project_id: project_id.into(),
            task_id,
            node_id: node_id.into(),
            task_revision: task.revision,
            route_epoch: task.route_cancellation_epoch,
            lease_epoch,
            status: "active".into(),
            leased_at: now,
            expires_at,
            completed_at: None,
        };
        tx.commit().await?;
        Ok((Some((lease, task)), events))
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
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::Telemetry,
                "renew_distributed_task_lease",
            )
            .await?;
        let now = Utc::now();
        let expires_at =
            now + Duration::seconds(i64::try_from(ttl_seconds.clamp(10, 3600)).unwrap_or(3600));
        let updated = sqlx::query("UPDATE task_leases SET expires_at=? WHERE lease_id=? AND node_id=? AND lease_epoch=? AND status='active' AND expires_at>=?")
            .bind(expires_at.to_rfc3339()).bind(lease_id).bind(node_id).bind(lease_epoch).bind(now.to_rfc3339()).execute(self.pool()).await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::LateSubmission(
                "lease is expired, stale, or owned by another node".into(),
            ));
        }
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
        lease_id: &str,
        node_id: &str,
        token: &str,
        node_epoch: i64,
        lease_epoch: i64,
        output: &WorkerOutput,
    ) -> StorageResult<Vec<DomainEvent>> {
        authenticate_node(self, node_id, token, node_epoch).await?;
        let lease = self.get_task_lease(lease_id).await?;
        if lease.node_id != node_id
            || lease.lease_epoch != lease_epoch
            || lease.status != "active"
            || lease.expires_at < Utc::now()
        {
            return Err(StorageError::LateSubmission(
                "lease is expired, stale, or owned by another node".into(),
            ));
        }
        let task = self.get_task(&lease.project_id, &lease.task_id).await?;
        if task.revision != lease.task_revision
            || task.route_cancellation_epoch != lease.route_epoch
        {
            return Err(StorageError::LateSubmission(
                "task revision or route epoch changed after lease".into(),
            ));
        }
        let mut events = self.record_worker_output(&task, output).await?;
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "complete_distributed_task_lease",
            )
            .await?;
        let now = Utc::now();
        let mut tx = self.pool().begin().await?;
        let updated = sqlx::query("UPDATE task_leases SET status='completed',completed_at=? WHERE lease_id=? AND status='active' AND lease_epoch=?")
            .bind(now.to_rfc3339()).bind(lease_id).bind(lease_epoch).execute(&mut *tx).await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::LateSubmission(
                "lease completion raced with another submitter".into(),
            ));
        }
        let revision = bump_revision(&mut tx, &lease.project_id).await?;
        events.push(
            append_event(
                &mut tx,
                &lease.project_id,
                revision,
                "task_lease.completed",
                entity("task_lease", lease_id),
                json!({"task_id":lease.task_id,"node_id":node_id,"lease_epoch":lease_epoch}),
                Some(entity("task", &lease.task_id)),
            )
            .await?,
        );
        tx.commit().await?;
        Ok(events)
    }
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
