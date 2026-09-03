use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgPoolOptions};

use crate::{StorageError, StorageResult, new_id};

#[derive(Debug, Clone)]
pub struct PostgresStateCommitter {
    pool: PgPool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostgresLease {
    pub lease_id: String,
    pub project_id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub worker_instance_id: String,
    pub lease_token: String,
    pub lease_epoch: i64,
    pub task_revision: i64,
    pub route_epoch: i64,
    pub expires_at: DateTime<Utc>,
    pub task_payload: Value,
    pub contract: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostgresPlanTask {
    pub task_id: String,
    pub route_id: String,
    pub task_signature: String,
    pub priority: f64,
    pub route_cancellation_epoch: i64,
    pub context_packet_id: Option<String>,
    pub contract: Value,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostgresPlanCommit {
    pub expected_project_revision: i64,
    pub ordinal: i64,
    pub consumed_delta_from: i64,
    pub consumed_delta_to: i64,
    pub planner_mode: String,
    pub payload: Value,
    pub tasks: Vec<PostgresPlanTask>,
}

impl PostgresStateCommitter {
    pub async fn connect(database_url: &str, max_connections: u32) -> StorageResult<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections.max(2))
            .connect(database_url)
            .await?;
        sqlx::migrate!("../../migrations-postgres")
            .run(&pool)
            .await?;
        Ok(Self { pool })
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn lease_next_task(
        &self,
        project_id: &str,
        node_id: &str,
        worker_instance_id: &str,
        ttl_seconds: u64,
    ) -> StorageResult<Option<PostgresLease>> {
        let mut retry = 0_u32;
        loop {
            match self
                .lease_next_task_once(project_id, node_id, worker_instance_id, ttl_seconds)
                .await
            {
                Err(error) if retry < 3 && retryable_postgres_transaction(&error) => {
                    retry += 1;
                    tokio::time::sleep(retry_delay(retry)).await;
                }
                result => return result,
            }
        }
    }

    async fn lease_next_task_once(
        &self,
        project_id: &str,
        node_id: &str,
        worker_instance_id: &str,
        ttl_seconds: u64,
    ) -> StorageResult<Option<PostgresLease>> {
        let mut tx = self.pool.begin().await?;
        expire_pg_leases(&mut tx, project_id).await?;
        let row = sqlx::query(
            "SELECT t.task_id,t.revision,t.route_cancellation_epoch,t.payload_json,t.contract_json \
             FROM tasks t JOIN routes r ON r.route_id=t.route_id \
             WHERE t.project_id=$1 AND t.status='queued' \
               AND r.status IN ('incubating','active','blocked','probation','revived') \
             ORDER BY t.priority DESC,t.created_at,t.task_id \
             FOR UPDATE OF t SKIP LOCKED LIMIT 1",
        )
        .bind(project_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let task_id: String = row.try_get("task_id")?;
        let attempt_no: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(attempt_no),0)+1 FROM task_attempts WHERE task_id=$1",
        )
        .bind(&task_id)
        .fetch_one(&mut *tx)
        .await?;
        if attempt_no > 3 {
            sqlx::query(
                "UPDATE tasks SET status='dead_lettered',revision=revision+1 WHERE task_id=$1",
            )
            .bind(&task_id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(None);
        }
        let lease_epoch: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(lease_epoch),0)+1 FROM task_attempts WHERE task_id=$1",
        )
        .bind(&task_id)
        .fetch_one(&mut *tx)
        .await?;
        let task_revision: i64 = row.try_get::<i64, _>("revision")? + 1;
        let route_epoch: i64 = row.try_get("route_cancellation_epoch")?;
        let attempt_id = new_id("attempt");
        let lease_id = new_id("lease");
        let lease_token = format!("{}{}", new_id("token"), new_id("token"));
        let now = Utc::now();
        let expires_at =
            now + Duration::seconds(i64::try_from(ttl_seconds.clamp(30, 3_600)).unwrap_or(3_600));
        sqlx::query("INSERT INTO task_attempts(attempt_id,project_id,task_id,worker_instance_id,attempt_no,lease_epoch,status,started_at) VALUES($1,$2,$3,$4,$5,$6,'running',$7)")
            .bind(&attempt_id).bind(project_id).bind(&task_id).bind(worker_instance_id)
            .bind(attempt_no).bind(lease_epoch).bind(now).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO task_leases(lease_id,project_id,task_id,attempt_id,worker_instance_id,node_id,lease_epoch,task_revision,route_epoch,token_hash,status,leased_at,expires_at,last_heartbeat_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'active',$11,$12,$11)")
            .bind(&lease_id).bind(project_id).bind(&task_id).bind(&attempt_id).bind(worker_instance_id)
            .bind(node_id).bind(lease_epoch).bind(task_revision).bind(route_epoch).bind(hash_secret(&lease_token))
            .bind(now).bind(expires_at).execute(&mut *tx).await?;
        sqlx::query("UPDATE tasks SET status='running',revision=revision+1 WHERE task_id=$1 AND status='queued'")
            .bind(&task_id).execute(&mut *tx).await?;
        let revision = bump_pg_revision(&mut tx, project_id).await?;
        append_pg_event(&mut tx,project_id,revision,"task.lease_acquired",json!({"kind":"task_lease","id":lease_id}),json!({"task_id":task_id,"attempt_id":attempt_id,"worker_instance_id":worker_instance_id,"lease_epoch":lease_epoch,"expires_at":expires_at})).await?;
        tx.commit().await?;
        Ok(Some(PostgresLease {
            lease_id,
            project_id: project_id.into(),
            task_id,
            attempt_id,
            worker_instance_id: worker_instance_id.into(),
            lease_token,
            lease_epoch,
            task_revision,
            route_epoch,
            expires_at,
            task_payload: row.try_get("payload_json")?,
            contract: row.try_get("contract_json")?,
        }))
    }

    pub async fn renew_lease(
        &self,
        lease: &PostgresLease,
        ttl_seconds: u64,
    ) -> StorageResult<DateTime<Utc>> {
        let expires_at = Utc::now()
            + Duration::seconds(i64::try_from(ttl_seconds.clamp(30, 3_600)).unwrap_or(3_600));
        let changed = sqlx::query("UPDATE task_leases SET expires_at=$1,last_heartbeat_at=now() WHERE lease_id=$2 AND attempt_id=$3 AND lease_epoch=$4 AND token_hash=$5 AND status='active' AND expires_at>=now()")
            .bind(expires_at).bind(&lease.lease_id).bind(&lease.attempt_id).bind(lease.lease_epoch)
            .bind(hash_secret(&lease.lease_token)).execute(&self.pool).await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::LateSubmission(
                "PostgreSQL lease is stale or expired".into(),
            ));
        }
        Ok(expires_at)
    }

    pub async fn submit_result_envelope(
        &self,
        lease: &PostgresLease,
        outcome: &str,
        payload: &Value,
    ) -> StorageResult<String> {
        let mut retry = 0_u32;
        loop {
            match self
                .submit_result_envelope_once(lease, outcome, payload)
                .await
            {
                Err(error) if retry < 3 && retryable_postgres_transaction(&error) => {
                    retry += 1;
                    tokio::time::sleep(retry_delay(retry)).await;
                }
                result => return result,
            }
        }
    }

    async fn submit_result_envelope_once(
        &self,
        lease: &PostgresLease,
        outcome: &str,
        payload: &Value,
    ) -> StorageResult<String> {
        let content_hash = hex::encode(Sha256::digest(serde_json::to_vec(payload)?));
        let idempotency_key = format!("result:{}:{content_hash}", lease.attempt_id);
        let mut tx = self.pool.begin().await?;
        if let Some(existing)=sqlx::query_scalar::<_,String>("SELECT result_envelope_id FROM result_envelopes WHERE project_id=$1 AND idempotency_key=$2")
            .bind(&lease.project_id).bind(&idempotency_key).fetch_optional(&mut *tx).await?
        {
            tx.rollback().await?;
            return Ok(existing);
        }
        let valid:i64=sqlx::query_scalar("SELECT COUNT(*) FROM task_leases l JOIN tasks t ON t.task_id=l.task_id JOIN routes r ON r.route_id=t.route_id WHERE l.lease_id=$1 AND l.attempt_id=$2 AND l.lease_epoch=$3 AND l.token_hash=$4 AND l.status='active' AND l.expires_at>=now() AND t.status IN ('running','checkpointed') AND t.revision=$5 AND t.route_cancellation_epoch=$6 AND r.cancellation_epoch=$6")
            .bind(&lease.lease_id).bind(&lease.attempt_id).bind(lease.lease_epoch).bind(hash_secret(&lease.lease_token))
            .bind(lease.task_revision).bind(lease.route_epoch).fetch_one(&mut *tx).await?;
        if valid != 1 {
            return Err(StorageError::LateSubmission(
                "PostgreSQL result lease, task revision, or route epoch is stale".into(),
            ));
        }
        let result_envelope_id = new_id("result");
        sqlx::query("INSERT INTO result_envelopes(result_envelope_id,project_id,task_id,attempt_id,lease_id,lease_epoch,route_cancellation_epoch,content_hash,idempotency_key,outcome,envelope_json,status,submitted_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'submitted',now())")
            .bind(&result_envelope_id).bind(&lease.project_id).bind(&lease.task_id).bind(&lease.attempt_id)
            .bind(&lease.lease_id).bind(lease.lease_epoch).bind(lease.route_epoch).bind(&content_hash)
            .bind(&idempotency_key).bind(outcome).bind(payload).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE tasks SET status='result_submitted',revision=revision+1 WHERE task_id=$1",
        )
        .bind(&lease.task_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE task_attempts SET status='result_submitted' WHERE attempt_id=$1")
            .bind(&lease.attempt_id)
            .execute(&mut *tx)
            .await?;
        let revision = bump_pg_revision(&mut tx, &lease.project_id).await?;
        append_pg_event(&mut tx,&lease.project_id,revision,"task.result_submitted",json!({"kind":"result_envelope","id":result_envelope_id}),json!({"task_id":lease.task_id,"attempt_id":lease.attempt_id,"content_hash":content_hash})).await?;
        tx.commit().await?;
        Ok(result_envelope_id)
    }

    pub async fn ingest_result_envelope(&self, result_envelope_id: &str) -> StorageResult<Value> {
        let mut retry = 0_u32;
        loop {
            match self.ingest_result_envelope_once(result_envelope_id).await {
                Err(error) if retry < 3 && retryable_postgres_transaction(&error) => {
                    retry += 1;
                    tokio::time::sleep(retry_delay(retry)).await;
                }
                result => return result,
            }
        }
    }

    async fn ingest_result_envelope_once(&self, result_envelope_id: &str) -> StorageResult<Value> {
        let mut tx = self.pool.begin().await?;
        let row=sqlx::query("SELECT e.*,t.status AS task_status,t.route_cancellation_epoch AS task_route_epoch,r.cancellation_epoch AS route_epoch FROM result_envelopes e JOIN tasks t ON t.task_id=e.task_id JOIN routes r ON r.route_id=t.route_id WHERE e.result_envelope_id=$1 FOR UPDATE OF e,t")
            .bind(result_envelope_id).fetch_optional(&mut *tx).await?
            .ok_or_else(|| StorageError::NotFound{kind:"result_envelope",id:result_envelope_id.into()})?;
        let payload: Value = row.try_get("envelope_json")?;
        let status: String = row.try_get("status")?;
        if status == "ingested" {
            tx.rollback().await?;
            return Ok(payload);
        }
        let task_status: String = row.try_get("task_status")?;
        let envelope_epoch: i64 = row.try_get("route_cancellation_epoch")?;
        let task_epoch: i64 = row.try_get("task_route_epoch")?;
        let route_epoch: i64 = row.try_get("route_epoch")?;
        if status != "submitted"
            || task_status != "result_submitted"
            || envelope_epoch != task_epoch
            || envelope_epoch != route_epoch
        {
            return Err(StorageError::LateSubmission(
                "PostgreSQL envelope became stale before ingestion".into(),
            ));
        }
        let project_id: String = row.try_get("project_id")?;
        let task_id: String = row.try_get("task_id")?;
        let attempt_id: String = row.try_get("attempt_id")?;
        let lease_id: String = row.try_get("lease_id")?;
        sqlx::query("UPDATE tasks SET status='completed',revision=revision+1 WHERE task_id=$1")
            .bind(&task_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE task_attempts SET status='completed',completed_at=now() WHERE attempt_id=$1",
        )
        .bind(&attempt_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE task_leases SET status='completed',completed_at=now() WHERE lease_id=$1",
        )
        .bind(&lease_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE result_envelopes SET status='ingested',ingested_at=now() WHERE result_envelope_id=$1").bind(result_envelope_id).execute(&mut *tx).await?;
        let revision = bump_pg_revision(&mut tx, &project_id).await?;
        append_pg_event(
            &mut tx,
            &project_id,
            revision,
            "task.result_ingested",
            json!({"kind":"result_envelope","id":result_envelope_id}),
            json!({"task_id":task_id,"attempt_id":attempt_id}),
        )
        .await?;
        tx.commit().await?;
        Ok(payload)
    }

    pub async fn commit_plan_revision(
        &self,
        project_id: &str,
        commit: &PostgresPlanCommit,
    ) -> StorageResult<String> {
        let mut retry = 0_u32;
        loop {
            match self.commit_plan_revision_once(project_id, commit).await {
                Err(error) if retry < 3 && retryable_postgres_transaction(&error) => {
                    retry += 1;
                    tokio::time::sleep(retry_delay(retry)).await;
                }
                result => return result,
            }
        }
    }

    async fn commit_plan_revision_once(
        &self,
        project_id: &str,
        commit: &PostgresPlanCommit,
    ) -> StorageResult<String> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(project_id)
            .execute(&mut *tx)
            .await?;
        let actual: i64 =
            sqlx::query_scalar("SELECT revision FROM projects WHERE project_id=$1 FOR UPDATE")
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?;
        if actual != commit.expected_project_revision {
            return Err(StorageError::RevisionConflict {
                expected: commit.expected_project_revision,
                actual,
            });
        }
        let plan_revision_id = new_id("planrev");
        sqlx::query("INSERT INTO plan_revisions(plan_revision_id,project_id,ordinal,based_on_project_revision,consumed_delta_from,consumed_delta_to,planner_mode,payload_json,status,committed_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'committed',now())")
            .bind(&plan_revision_id).bind(project_id).bind(commit.ordinal).bind(commit.expected_project_revision)
            .bind(commit.consumed_delta_from).bind(commit.consumed_delta_to).bind(&commit.planner_mode).bind(&commit.payload)
            .execute(&mut *tx).await?;
        for task in &commit.tasks {
            sqlx::query("INSERT INTO tasks(task_id,project_id,route_id,plan_revision_id,task_signature,status,priority,route_cancellation_epoch,context_packet_id,contract_json,payload_json) VALUES($1,$2,$3,$4,$5,'queued',$6,$7,$8,$9,$10) ON CONFLICT DO NOTHING")
                .bind(&task.task_id).bind(project_id).bind(&task.route_id).bind(&plan_revision_id).bind(&task.task_signature)
                .bind(task.priority).bind(task.route_cancellation_epoch).bind(&task.context_packet_id).bind(&task.contract).bind(&task.payload)
                .execute(&mut *tx).await?;
        }
        let revision = bump_pg_revision(&mut tx, project_id).await?;
        append_pg_event(&mut tx,project_id,revision,"planning.revision.committed",json!({"kind":"plan_revision","id":plan_revision_id}),json!({"ordinal":commit.ordinal,"planner_mode":commit.planner_mode,"task_count":commit.tasks.len()})).await?;
        tx.commit().await?;
        Ok(plan_revision_id)
    }
}

async fn expire_pg_leases(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> StorageResult<()> {
    let rows=sqlx::query("UPDATE task_leases SET status='expired',completed_at=now() WHERE project_id=$1 AND status='active' AND expires_at<now() RETURNING lease_id,task_id,attempt_id")
        .bind(project_id).fetch_all(&mut **tx).await?;
    for row in rows {
        let lease_id: String = row.try_get("lease_id")?;
        let task_id: String = row.try_get("task_id")?;
        let attempt_id: String = row.try_get("attempt_id")?;
        let replayable_result: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM result_envelopes WHERE lease_id=$1 AND status='submitted'",
        )
        .bind(&lease_id)
        .fetch_one(&mut **tx)
        .await?;
        if replayable_result > 0 {
            continue;
        }
        sqlx::query("UPDATE task_attempts SET status='orphaned',completed_at=now(),failure_reason='lease expired' WHERE attempt_id=$1 AND status NOT IN ('completed','failed')")
            .bind(attempt_id).execute(&mut **tx).await?;
        sqlx::query("UPDATE tasks SET status=CASE WHEN (SELECT COUNT(*) FROM task_attempts WHERE task_id=$1)>=3 THEN 'dead_lettered' ELSE 'queued' END,revision=revision+1 WHERE task_id=$1 AND status IN ('leased','running','checkpointed')")
            .bind(task_id).execute(&mut **tx).await?;
    }
    Ok(())
}

async fn bump_pg_revision(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> StorageResult<i64> {
    Ok(sqlx::query_scalar("UPDATE projects SET revision=revision+1,updated_at=now() WHERE project_id=$1 RETURNING revision")
        .bind(project_id).fetch_one(&mut **tx).await?)
}

async fn append_pg_event(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
    revision: i64,
    event_type: &str,
    entity: Value,
    data: Value,
) -> StorageResult<()> {
    let event_id = new_id("event");
    let occurred_at = Utc::now();
    let payload = json!({"event_id":event_id,"project_id":project_id,"project_revision":revision,"type":event_type,"entity":entity,"data":data,"occurred_at":occurred_at});
    sqlx::query("INSERT INTO events(event_id,project_id,project_revision,event_type,entity_json,data_json,occurred_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(&event_id).bind(project_id).bind(revision).bind(event_type).bind(&entity).bind(&data).bind(occurred_at).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO event_outbox(outbox_id,event_id,project_id,event_type,payload_json) VALUES($1,$2,$3,$4,$5)")
        .bind(new_id("outbox")).bind(event_id).bind(project_id).bind(event_type).bind(payload).execute(&mut **tx).await?;
    Ok(())
}

fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

fn retryable_postgres_transaction(error: &StorageError) -> bool {
    let StorageError::Database(error) = error else {
        return false;
    };
    error.as_database_error().is_some_and(|database_error| {
        matches!(database_error.code().as_deref(), Some("40P01" | "40001"))
    })
}

fn retry_delay(retry: u32) -> std::time::Duration {
    let exponent = retry.saturating_sub(1).min(3);
    std::time::Duration::from_millis(25 * 2_u64.pow(exponent))
}
