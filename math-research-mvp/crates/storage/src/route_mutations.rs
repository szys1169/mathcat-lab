use chrono::{DateTime, Utc};
use sqlx::{Row, Sqlite, Transaction};

use crate::{StorageError, StorageResult, json_text, new_id};

pub(crate) struct PrunedRoute {
    pub tombstone_id: String,
    pub cancelled_task_ids: Vec<String>,
    pub cancelled_worker_ids: Vec<String>,
}

pub(crate) struct MergedRoutes {
    pub routes: Vec<String>,
    pub cancelled_tasks: Vec<String>,
    pub cancelled_workers: Vec<String>,
}

pub(crate) struct StoppedRoute {
    pub cancelled_task_ids: Vec<String>,
    pub cancelled_worker_ids: Vec<String>,
}

pub(crate) struct TaskLeaseExpiry<'a> {
    pub project: &'a str,
    pub lease: &'a str,
    pub task: &'a str,
    pub attempt: Option<&'a str>,
    pub worker_instance: Option<&'a str>,
}

pub(crate) struct TaskLeaseExpiryOutcome {
    pub replayable_result: bool,
}

/// Expires one active task lease and applies the single authoritative recovery
/// transition used by both distributed leasing and periodic reconciliation.
pub(crate) async fn expire_task_lease_tx(
    tx: &mut Transaction<'_, Sqlite>,
    lease: TaskLeaseExpiry<'_>,
    now: &DateTime<Utc>,
) -> StorageResult<Option<TaskLeaseExpiryOutcome>> {
    let changed = sqlx::query(
        "UPDATE task_leases SET status='expired',completed_at=? WHERE lease_id=? AND status='active'",
    )
    .bind(now.to_rfc3339())
    .bind(lease.lease)
    .execute(&mut **tx)
    .await?;
    if changed.rows_affected() == 0 {
        return Ok(None);
    }
    let replayable_result = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM result_envelopes WHERE lease_id=? AND status='submitted'",
    )
    .bind(lease.lease)
    .fetch_one(&mut **tx)
    .await?
        > 0;
    if !replayable_result {
        if let Some(attempt_id) = lease.attempt {
            sqlx::query("UPDATE task_attempts SET status='orphaned',failure_reason='task lease expired',completed_at=? WHERE attempt_id=? AND status NOT IN ('completed','failed','blocked','orphaned','cancelled')")
                .bind(now.to_rfc3339())
                .bind(attempt_id)
                .execute(&mut **tx)
                .await?;
        }
        if let Some(worker_instance_id) = lease.worker_instance {
            sqlx::query("UPDATE worker_instances SET status='unhealthy',quarantine_reason='task lease expired',exited_at=? WHERE worker_instance_id=? AND status NOT IN ('exited','quarantined')")
                .bind(now.to_rfc3339())
                .bind(worker_instance_id)
                .execute(&mut **tx)
                .await?;
        }
        sqlx::query("UPDATE tasks SET status=CASE WHEN (SELECT COUNT(*) FROM task_attempts WHERE task_id=?)>=3 THEN 'dead_lettered' ELSE 'queued' END,revision=revision+1,result_summary='task lease expired before result submission' WHERE task_id=? AND status IN ('offered','leased','running','checkpointed','result_submitted','ingesting')")
            .bind(lease.task)
            .bind(lease.task)
            .execute(&mut **tx)
            .await?;
        sqlx::query("UPDATE workers SET status='backoff',current_task_id=NULL,current_route_id=NULL,last_heartbeat=? WHERE project_id=? AND current_task_id=? AND status NOT IN ('stopped','offline','quarantined','exited')")
            .bind(now.to_rfc3339())
            .bind(lease.project)
            .bind(lease.task)
            .execute(&mut **tx)
            .await?;
    }
    Ok(Some(TaskLeaseExpiryOutcome { replayable_result }))
}

pub(crate) async fn prune_route_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    route_id: &str,
    applied_revision: i64,
    reason: &str,
    now: &DateTime<Utc>,
) -> StorageResult<PrunedRoute> {
    if reason.trim().is_empty() {
        return Err(StorageError::InvalidTransition(
            "route pruning requires a reason".into(),
        ));
    }
    let row = sqlx::query(
        "SELECT status,semantic_fingerprint FROM routes WHERE project_id=? AND route_id=?",
    )
    .bind(project_id)
    .bind(route_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| StorageError::NotFound {
        kind: "route",
        id: route_id.into(),
    })?;
    let status: String = row.try_get("status")?;
    if !matches!(
        status.as_str(),
        "incubating" | "active" | "blocked" | "probation" | "revived" | "paused"
    ) {
        return Err(StorageError::InvalidTransition(format!(
            "cannot prune route {route_id} from {status}"
        )));
    }
    let fingerprint: Option<String> = row.try_get("semantic_fingerprint")?;
    sqlx::query("UPDATE routes SET status='pruned',human_review=CASE WHEN human_review='pending' THEN 'rejected' ELSE human_review END,cancellation_epoch=cancellation_epoch+1,merge_reason=? WHERE project_id=? AND route_id=?")
        .bind(reason.trim())
        .bind(project_id)
        .bind(route_id)
        .execute(&mut **tx)
        .await?;
    let cancelled =
        cancel_route_execution_tx(tx, project_id, route_id, "cancelled", "route pruned", now)
            .await?;
    resume_project_after_route_review_tx(tx, project_id).await?;

    let tombstone_id = new_id("routetombstone");
    sqlx::query("INSERT INTO route_tombstones(tombstone_id,project_id,route_id,semantic_fingerprint,reason,revive_only_if_json,created_revision,created_at) VALUES(?,?,?,?,?,?,?,?)")
        .bind(&tombstone_id)
        .bind(project_id)
        .bind(route_id)
        .bind(fingerprint.unwrap_or_else(|| route_id.into()))
        .bind(reason.trim())
        .bind(json_text(&vec![
            "new verified evidence changes a failed premise",
            "a linked bottleneck is resolved",
            "explicit authorized revival",
        ])?)
        .bind(applied_revision)
        .bind(now.to_rfc3339())
        .execute(&mut **tx)
        .await?;
    Ok(PrunedRoute {
        tombstone_id,
        cancelled_task_ids: cancelled.task_ids,
        cancelled_worker_ids: cancelled.worker_ids,
    })
}

pub(crate) async fn merge_routes_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    canonical_route_id: &str,
    source_route_ids: &[String],
    reason: &str,
    now: &DateTime<Utc>,
) -> StorageResult<MergedRoutes> {
    if source_route_ids.is_empty() || reason.trim().is_empty() {
        return Err(StorageError::InvalidTransition(
            "route merge requires sources and a reason".into(),
        ));
    }
    if source_route_ids
        .iter()
        .any(|source| source == canonical_route_id)
    {
        return Err(StorageError::InvalidTransition(
            "canonical route cannot also be a source route".into(),
        ));
    }
    let canonical_exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM routes WHERE project_id=? AND route_id=? AND status IN ('incubating','active','blocked','probation','revived')")
        .bind(project_id)
        .bind(canonical_route_id)
        .fetch_one(&mut **tx)
        .await?;
    if canonical_exists != 1 {
        return Err(StorageError::InvalidTransition(
            "canonical route is not live".into(),
        ));
    }

    let mut merged = Vec::with_capacity(source_route_ids.len());
    let mut cancelled_task_ids = Vec::new();
    let mut cancelled_worker_ids = Vec::new();
    for source_route_id in source_route_ids {
        let changed = sqlx::query("UPDATE routes SET status='merged',merged_into=?,merge_reason=?,human_review=CASE WHEN human_review='pending' THEN 'rejected' ELSE human_review END,cancellation_epoch=cancellation_epoch+1 WHERE project_id=? AND route_id=? AND status IN ('incubating','active','blocked','probation','revived','paused')")
            .bind(canonical_route_id)
            .bind(reason.trim())
            .bind(project_id)
            .bind(source_route_id)
            .execute(&mut **tx)
            .await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidTransition(format!(
                "source route {source_route_id} is missing or not mergeable"
            )));
        }
        let cancelled = cancel_route_execution_tx(
            tx,
            project_id,
            source_route_id,
            "cancelled",
            &format!("route merged into {canonical_route_id}"),
            now,
        )
        .await?;
        cancelled_task_ids.extend(cancelled.task_ids);
        cancelled_worker_ids.extend(cancelled.worker_ids);
        merged.push(source_route_id.clone());
    }
    resume_project_after_route_review_tx(tx, project_id).await?;
    cancelled_task_ids.sort();
    cancelled_task_ids.dedup();
    cancelled_worker_ids.sort();
    cancelled_worker_ids.dedup();
    Ok(MergedRoutes {
        routes: merged,
        cancelled_tasks: cancelled_task_ids,
        cancelled_workers: cancelled_worker_ids,
    })
}

pub(crate) async fn stop_route_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    route_id: &str,
    reason: &str,
    now: &DateTime<Utc>,
) -> StorageResult<StoppedRoute> {
    let changed = sqlx::query("UPDATE routes SET status='human_stopped',cancellation_epoch=cancellation_epoch+1,merge_reason=? WHERE project_id=? AND route_id=? AND status IN ('proposed','incubating','active','blocked','probation','paused','revived')")
        .bind(reason)
        .bind(project_id)
        .bind(route_id)
        .execute(&mut **tx)
        .await?;
    if changed.rows_affected() != 1 {
        return Err(StorageError::InvalidTransition(format!(
            "route {route_id} is missing or cannot be stopped"
        )));
    }
    let cancelled =
        cancel_route_execution_tx(tx, project_id, route_id, "human_stopped", reason, now).await?;
    Ok(StoppedRoute {
        cancelled_task_ids: cancelled.task_ids,
        cancelled_worker_ids: cancelled.worker_ids,
    })
}

pub(crate) async fn revive_route_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    route_id: &str,
    applied_revision: i64,
    reason: &str,
    evidence_ids: &[String],
) -> StorageResult<()> {
    if reason.trim().is_empty() || evidence_ids.is_empty() {
        return Err(StorageError::InvalidTransition(
            "route revival requires a reason and evidence IDs".into(),
        ));
    }
    let changed = sqlx::query("UPDATE routes SET status='revived',cancellation_epoch=cancellation_epoch+1,consecutive_no_progress_plans=0,merge_reason=NULL WHERE project_id=? AND route_id=? AND status='pruned'")
        .bind(project_id)
        .bind(route_id)
        .execute(&mut **tx)
        .await?;
    if changed.rows_affected() != 1 {
        return Err(StorageError::InvalidTransition(
            "only a pruned route can be revived".into(),
        ));
    }
    sqlx::query("UPDATE route_tombstones SET revived_revision=? WHERE project_id=? AND route_id=? AND revived_revision IS NULL")
        .bind(applied_revision)
        .bind(project_id)
        .bind(route_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

struct CancelledRouteExecution {
    task_ids: Vec<String>,
    worker_ids: Vec<String>,
}

pub(crate) struct CancelledTaskExecution {
    pub worker_ids: Vec<String>,
}

pub(crate) async fn cancel_task_execution_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    task_id: &str,
    next_task_status: &str,
    summary: &str,
    now: &DateTime<Utc>,
) -> StorageResult<CancelledTaskExecution> {
    let task_worker_id: Option<String> =
        sqlx::query_scalar("SELECT worker_id FROM tasks WHERE project_id=? AND task_id=?")
            .bind(project_id)
            .bind(task_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "task",
                id: task_id.into(),
            })?;
    let mut worker_ids = sqlx::query_scalar::<_, String>(
        "SELECT worker_id FROM workers WHERE project_id=? AND current_task_id=?",
    )
    .bind(project_id)
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    if let Some(worker_id) = task_worker_id {
        worker_ids.push(worker_id);
    }
    worker_ids.sort();
    worker_ids.dedup();

    let changed = sqlx::query("UPDATE tasks SET status=?,revision=revision+1,result_summary=COALESCE(result_summary,?) WHERE project_id=? AND task_id=? AND status IN ('open','queued','assigned','offered','leased','running','checkpointed','result_submitted','ingesting','paused','blocked')")
        .bind(next_task_status)
        .bind(summary)
        .bind(project_id)
        .bind(task_id)
        .execute(&mut **tx)
        .await?;
    if changed.rows_affected() != 1 {
        return Err(StorageError::InvalidTransition(format!(
            "task {task_id} is no longer cancellable"
        )));
    }
    sqlx::query("UPDATE task_leases SET status='cancelled',completed_at=? WHERE project_id=? AND task_id=? AND status='active'")
        .bind(now.to_rfc3339())
        .bind(project_id)
        .bind(task_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE result_envelopes SET status='stale',rejection_reason=? WHERE project_id=? AND task_id=? AND status='submitted'")
        .bind(summary)
        .bind(project_id)
        .bind(task_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE task_steers SET status='stale',applied_at=? WHERE project_id=? AND task_id=? AND status='pending'")
        .bind(now.to_rfc3339())
        .bind(project_id)
        .bind(task_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE human_commands SET status='failed',error=?,applied_at=? WHERE project_id=? AND status='waiting_safe_point' AND command_id IN (SELECT command_id FROM task_steers WHERE project_id=? AND task_id=?)")
        .bind(summary)
        .bind(now.to_rfc3339())
        .bind(project_id)
        .bind(project_id)
        .bind(task_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE task_attempts SET status='cancelled',failure_reason=COALESCE(failure_reason,?),completed_at=? WHERE project_id=? AND task_id=? AND status NOT IN ('completed','failed','cancelled','orphaned')")
        .bind(summary)
        .bind(now.to_rfc3339())
        .bind(project_id)
        .bind(task_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE worker_instances SET status='exited',exited_at=? WHERE project_id=? AND worker_instance_id IN (SELECT worker_instance_id FROM task_attempts WHERE project_id=? AND task_id=?) AND status IN ('spawn_requested','starting','handshaking','ready','lease_accepted','running','checkpointing','result_submitted','draining')")
        .bind(now.to_rfc3339())
        .bind(project_id)
        .bind(project_id)
        .bind(task_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE workers SET status='idle',current_task_id=NULL,current_route_id=NULL WHERE project_id=? AND current_task_id=?")
        .bind(project_id)
        .bind(task_id)
        .execute(&mut **tx)
        .await?;
    Ok(CancelledTaskExecution { worker_ids })
}

async fn cancel_route_execution_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    route_id: &str,
    terminal_task_status: &str,
    summary: &str,
    now: &DateTime<Utc>,
) -> StorageResult<CancelledRouteExecution> {
    let rows = sqlx::query("SELECT task_id FROM tasks WHERE project_id=? AND route_id=? AND status IN ('open','queued','assigned','offered','leased','running','checkpointed','result_submitted','ingesting','paused','blocked') ORDER BY task_id")
        .bind(project_id)
        .bind(route_id)
        .fetch_all(&mut **tx)
        .await?;
    let task_ids = rows
        .iter()
        .map(|row| row.try_get::<String, _>("task_id"))
        .collect::<Result<Vec<_>, _>>()?;
    let mut worker_ids = Vec::new();
    for task_id in &task_ids {
        worker_ids.extend(
            cancel_task_execution_tx(tx, project_id, task_id, terminal_task_status, summary, now)
                .await?
                .worker_ids,
        );
    }
    worker_ids.sort();
    worker_ids.dedup();
    Ok(CancelledRouteExecution {
        task_ids,
        worker_ids,
    })
}

async fn resume_project_after_route_review_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
) -> StorageResult<()> {
    sqlx::query("UPDATE projects SET status='running' WHERE project_id=? AND status='needs_human_review' AND NOT EXISTS (SELECT 1 FROM routes WHERE project_id=? AND human_review='pending' AND status NOT IN ('merged','pruned','human_stopped'))")
        .bind(project_id)
        .bind(project_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
