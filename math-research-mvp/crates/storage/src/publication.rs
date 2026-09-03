use chrono::Utc;
use research_domain::{DomainEvent, PublicationRun};
use serde_json::{Value, json};
use sqlx::Row;

use crate::{
    SqliteStore, StorageError, StorageResult, append_event, bump_revision, entity, json_text,
    new_id, rows,
};

impl SqliteStore {
    pub async fn list_goal_closures(
        &self,
        project_id: &str,
        goal_id: &str,
    ) -> StorageResult<Vec<Value>> {
        let rows = sqlx::query("SELECT closure_id,goal_id,fact_id,verification_id,check_id,outcome,evidence_json,created_at FROM goal_closure_records WHERE project_id=? AND goal_id=? ORDER BY created_at,closure_id")
            .bind(project_id).bind(goal_id).fetch_all(self.pool()).await?;
        rows.iter()
            .map(|row| {
                Ok(json!({
                    "closure_id":row.try_get::<String,_>("closure_id")?,
                    "goal_id":row.try_get::<String,_>("goal_id")?,
                    "fact_id":row.try_get::<String,_>("fact_id")?,
                    "verification_id":row.try_get::<String,_>("verification_id")?,
                    "check_id":row.try_get::<String,_>("check_id")?,
                    "outcome":row.try_get::<String,_>("outcome")?,
                    "evidence":serde_json::from_str::<Value>(row.try_get("evidence_json")?)?,
                    "created_at":row.try_get::<String,_>("created_at")?,
                }))
            })
            .collect()
    }

    pub async fn begin_publication(
        &self,
        project_id: &str,
        idempotency_key: &str,
        allow_partial: bool,
        source_revision: i64,
        source_packet_hash: &str,
    ) -> StorageResult<(PublicationRun, Option<DomainEvent>)> {
        if idempotency_key.trim().is_empty() {
            return Err(StorageError::InvalidTransition(
                "publication Idempotency-Key must not be empty".into(),
            ));
        }
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "begin_publication",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        if let Some(row) =
            sqlx::query("SELECT * FROM publication_runs WHERE project_id=? AND idempotency_key=?")
                .bind(project_id)
                .bind(idempotency_key)
                .fetch_optional(&mut *tx)
                .await?
        {
            let run = publication_from_row(&row)?;
            if run.allow_partial != allow_partial || run.source_packet_hash != source_packet_hash {
                return Err(StorageError::InvalidTransition(
                    "publication idempotency key was reused with different inputs".into(),
                ));
            }
            if run.status != "running" {
                tx.rollback().await?;
                return Ok((run, None));
            }
            let now = Utc::now();
            sqlx::query("UPDATE publication_runs SET attempt_count=attempt_count+1,updated_at=? WHERE publication_id=? AND status='running'")
                .bind(now.to_rfc3339()).bind(&run.publication_id).execute(&mut *tx).await?;
            let revision = bump_revision(&mut tx, project_id).await?;
            let event = append_event(
                &mut tx,
                project_id,
                revision,
                "publication.resumed",
                entity("publication", &run.publication_id),
                json!({"previous_attempt_count":run.attempt_count,"source_revision":source_revision}),
                None,
            )
            .await?;
            let updated = sqlx::query("SELECT * FROM publication_runs WHERE publication_id=?")
                .bind(&run.publication_id)
                .fetch_one(&mut *tx)
                .await?;
            let resumed = publication_from_row(&updated)?;
            tx.commit().await?;
            return Ok((resumed, Some(event)));
        }
        let publication_id = new_id("publication");
        let now = Utc::now();
        sqlx::query("INSERT INTO publication_runs(publication_id,project_id,idempotency_key,allow_partial,source_revision,source_packet_hash,status,created_at,updated_at) VALUES(?,?,?,?,?,?,'running',?,?)")
            .bind(&publication_id).bind(project_id).bind(idempotency_key).bind(allow_partial)
            .bind(source_revision).bind(source_packet_hash).bind(now.to_rfc3339()).bind(now.to_rfc3339())
            .execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            "publication.started",
            entity("publication", &publication_id),
            json!({"allow_partial":allow_partial,"source_revision":source_revision,"source_packet_hash":source_packet_hash}),
            None,
        )
        .await?;
        tx.commit().await?;
        Ok((
            PublicationRun {
                publication_id,
                project_id: project_id.into(),
                idempotency_key: idempotency_key.into(),
                allow_partial,
                source_revision,
                source_packet_hash: source_packet_hash.into(),
                status: "running".into(),
                attempt_count: 1,
                result: None,
                error: None,
                created_at: now,
                updated_at: now,
                completed_at: None,
            },
            Some(event),
        ))
    }

    pub async fn complete_publication(
        &self,
        publication_id: &str,
        status: &str,
        result: Option<&Value>,
        error: Option<&str>,
    ) -> StorageResult<(PublicationRun, Option<DomainEvent>)> {
        if !matches!(status, "ready" | "blocked_by_evidence" | "failed") {
            return Err(StorageError::InvalidTransition(format!(
                "invalid publication terminal status {status}"
            )));
        }
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "complete_publication",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query("SELECT * FROM publication_runs WHERE publication_id=?")
            .bind(publication_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "publication",
                id: publication_id.into(),
            })?;
        let current = publication_from_row(&row)?;
        if current.status != "running" {
            tx.rollback().await?;
            return Ok((current, None));
        }
        let now = Utc::now();
        sqlx::query("UPDATE publication_runs SET status=?,result_json=?,error=?,updated_at=?,completed_at=? WHERE publication_id=? AND status='running'")
            .bind(status).bind(result.map(json_text).transpose()?).bind(error)
            .bind(now.to_rfc3339()).bind(now.to_rfc3339()).bind(publication_id)
            .execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &current.project_id).await?;
        let event = append_event(
            &mut tx,
            &current.project_id,
            revision,
            &format!("publication.{status}"),
            entity("publication", publication_id),
            json!({"status":status,"error":error}),
            None,
        )
        .await?;
        let updated = sqlx::query("SELECT * FROM publication_runs WHERE publication_id=?")
            .bind(publication_id)
            .fetch_one(&mut *tx)
            .await?;
        let run = publication_from_row(&updated)?;
        tx.commit().await?;
        Ok((run, Some(event)))
    }

    pub async fn get_publication(&self, publication_id: &str) -> StorageResult<PublicationRun> {
        let row = sqlx::query("SELECT * FROM publication_runs WHERE publication_id=?")
            .bind(publication_id)
            .fetch_optional(self.pool())
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "publication",
                id: publication_id.into(),
            })?;
        publication_from_row(&row)
    }

    pub async fn list_publications(&self, project_id: &str) -> StorageResult<Vec<PublicationRun>> {
        sqlx::query("SELECT * FROM publication_runs WHERE project_id=? ORDER BY created_at DESC")
            .bind(project_id)
            .fetch_all(self.pool())
            .await?
            .iter()
            .map(publication_from_row)
            .collect()
    }
}

fn publication_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<PublicationRun> {
    let result_json: Option<String> = row.try_get("result_json")?;
    Ok(PublicationRun {
        publication_id: row.try_get("publication_id")?,
        project_id: row.try_get("project_id")?,
        idempotency_key: row.try_get("idempotency_key")?,
        allow_partial: row.try_get("allow_partial")?,
        source_revision: row.try_get("source_revision")?,
        source_packet_hash: row.try_get("source_packet_hash")?,
        status: row.try_get("status")?,
        attempt_count: row.try_get("attempt_count")?,
        result: result_json
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        error: row.try_get("error")?,
        created_at: rows::timestamp(row.try_get("created_at")?)?,
        updated_at: rows::timestamp(row.try_get("updated_at")?)?,
        completed_at: rows::optional_timestamp(row.try_get("completed_at")?)?,
    })
}
