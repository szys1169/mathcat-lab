use chrono::{DateTime, Utc};
use research_domain::{DomainEvent, ProjectFactImport};
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction};

use crate::{
    SqliteStore, StorageError, StorageResult, append_event, bump_revision, entity, json_text,
    new_id,
};

impl SqliteStore {
    pub async fn import_catalog_fact(
        &self,
        target_project_id: &str,
        content_hash: &str,
        imported_by: &str,
    ) -> StorageResult<(ProjectFactImport, Option<DomainEvent>)> {
        self.get_project(target_project_id).await?;
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "import_catalog_fact",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let catalog = sqlx::query(
            "SELECT * FROM global_fact_catalog WHERE content_hash=? AND status='active'",
        )
        .bind(content_hash)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "catalog_fact",
            id: content_hash.into(),
        })?;
        let source_project_id: String = catalog.try_get("source_project_id")?;
        let source_fact_id: String = catalog.try_get("source_fact_id")?;
        if source_project_id == target_project_id {
            return Err(StorageError::InvalidTransition(
                "a project cannot import its own fact".into(),
            ));
        }

        let dependency_rows = sqlx::query(
            "WITH RECURSIVE dependencies(fact_id) AS (SELECT source_id FROM fact_edges WHERE project_id=? AND target_id=? UNION SELECT e.source_id FROM fact_edges e JOIN dependencies d ON e.target_id=d.fact_id WHERE e.project_id=?) SELECT DISTINCT fact_id FROM dependencies ORDER BY fact_id",
        )
        .bind(&source_project_id)
        .bind(&source_fact_id)
        .bind(&source_project_id)
        .fetch_all(&mut *tx)
        .await?;
        let mut closure_fact_ids = vec![source_fact_id.clone()];
        for row in dependency_rows {
            let dependency_id: String = row.try_get("fact_id")?;
            if !closure_fact_ids.contains(&dependency_id) {
                closure_fact_ids.push(dependency_id);
            }
        }

        let now = Utc::now();
        let mut created = Vec::new();
        for fact_id in &closure_fact_ids {
            let catalog_row = sqlx::query(
                "SELECT * FROM global_fact_catalog WHERE source_fact_id=? AND status='active'",
            )
            .bind(fact_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                StorageError::InvalidTransition(format!(
                    "dependency closure fact {fact_id} is absent or inactive in the catalog"
                ))
            })?;
            let dependency_hash: String = catalog_row.try_get("content_hash")?;
            let dependency_source_project: String = catalog_row.try_get("source_project_id")?;
            if dependency_source_project != source_project_id {
                return Err(StorageError::CorruptData(format!(
                    "dependency closure fact {fact_id} belongs to another source project"
                )));
            }
            let assurance = sqlx::query("SELECT fact_id,case_id,acceptance_class,snapshot_hash,package_id,replay_id,status,created_at,invalidated_at FROM fact_assurances WHERE fact_id=? AND status='active' ORDER BY created_at DESC LIMIT 1")
                .bind(fact_id).fetch_optional(&mut *tx).await?
                .ok_or_else(|| StorageError::InvalidTransition(format!("catalog fact {fact_id} has no active assurance")))?;
            let assurance_snapshot = json!({
                "fact_id":assurance.try_get::<String,_>("fact_id")?,"case_id":assurance.try_get::<String,_>("case_id")?,
                "acceptance_class":assurance.try_get::<String,_>("acceptance_class")?,"snapshot_hash":assurance.try_get::<String,_>("snapshot_hash")?,
                "package_id":assurance.try_get::<Option<String>,_>("package_id")?,"replay_id":assurance.try_get::<Option<String>,_>("replay_id")?,
                "captured_at":now,
            });
            let existing = sqlx::query("SELECT import_id,status FROM project_fact_imports WHERE target_project_id=? AND content_hash=?")
                .bind(target_project_id).bind(&dependency_hash).fetch_optional(&mut *tx).await?;
            if let Some(existing) = existing {
                let import_id: String = existing.try_get("import_id")?;
                let status: String = existing.try_get("status")?;
                if status != "active" {
                    sqlx::query("UPDATE project_fact_imports SET assurance_snapshot_json=?,status='active',imported_by=?,created_at=?,invalidated_at=NULL WHERE import_id=?")
                        .bind(json_text(&assurance_snapshot)?).bind(imported_by).bind(now.to_rfc3339()).bind(&import_id)
                        .execute(&mut *tx).await?;
                    created.push(json!({"import_id":import_id,"source_fact_id":fact_id,"content_hash":dependency_hash,"reactivated":true}));
                }
            } else {
                let import_id = new_id("factimport");
                sqlx::query("INSERT INTO project_fact_imports(import_id,target_project_id,source_project_id,source_fact_id,content_hash,assurance_snapshot_json,status,imported_by,created_at,invalidated_at) VALUES(?,?,?,?,?,?,'active',?,?,NULL)")
                    .bind(&import_id).bind(target_project_id).bind(&source_project_id).bind(fact_id)
                    .bind(&dependency_hash).bind(json_text(&assurance_snapshot)?).bind(imported_by).bind(now.to_rfc3339())
                    .execute(&mut *tx).await?;
                created.push(json!({"import_id":import_id,"source_fact_id":fact_id,"content_hash":dependency_hash}));
            }
        }
        let root_row = sqlx::query(
            "SELECT * FROM project_fact_imports WHERE target_project_id=? AND content_hash=?",
        )
        .bind(target_project_id)
        .bind(content_hash)
        .fetch_one(&mut *tx)
        .await?;
        let value = import_from_row(&root_row)?;
        if created.is_empty() {
            tx.rollback().await?;
            return Ok((value, None));
        }
        let revision = bump_revision(&mut tx, target_project_id).await?;
        let event = append_event(&mut tx, target_project_id, revision, "fact_import.created", entity("fact_import", &value.import_id), json!({"source_project_id":source_project_id,"source_fact_id":source_fact_id,"content_hash":content_hash,"dependency_closure":created}), Some(entity("fact", &source_fact_id))).await?;
        tx.commit().await?;
        Ok((value, Some(event)))
    }

    pub async fn list_fact_imports(
        &self,
        project_id: &str,
    ) -> StorageResult<Vec<ProjectFactImport>> {
        let rows = sqlx::query("SELECT * FROM project_fact_imports WHERE target_project_id=? ORDER BY created_at,import_id")
            .bind(project_id).fetch_all(self.pool()).await?;
        rows.iter().map(import_from_row).collect()
    }

    pub async fn list_catalog_facts(&self) -> StorageResult<Vec<Value>> {
        let rows = sqlx::query("SELECT c.*,a.acceptance_class FROM global_fact_catalog c LEFT JOIN fact_assurances a ON a.fact_id=c.source_fact_id AND a.status='active' ORDER BY c.updated_at,c.content_hash")
            .fetch_all(self.pool()).await?;
        rows.iter().map(|row| Ok(json!({
            "content_hash":row.try_get::<String,_>("content_hash")?,"source_project_id":row.try_get::<String,_>("source_project_id")?,
            "source_fact_id":row.try_get::<String,_>("source_fact_id")?,"statement":row.try_get::<String,_>("statement")?,
            "status":row.try_get::<String,_>("status")?,"acceptance_class":row.try_get::<Option<String>,_>("acceptance_class")?,
            "updated_at":row.try_get::<String,_>("updated_at")?
        }))).collect()
    }
}

pub(crate) async fn invalidate_imports_for_source(
    tx: &mut Transaction<'_, Sqlite>,
    source_fact_id: &str,
) -> StorageResult<Vec<DomainEvent>> {
    let rows = sqlx::query("SELECT import_id,target_project_id FROM project_fact_imports WHERE source_fact_id=? AND status='active'")
        .bind(source_fact_id).fetch_all(&mut **tx).await?;
    let now = Utc::now();
    let mut events = Vec::new();
    for row in rows {
        let import_id: String = row.try_get("import_id")?;
        let target_project_id: String = row.try_get("target_project_id")?;
        let impact =
            crate::fact_governance::compute_fact_impact_tx(tx, &target_project_id, source_fact_id)
                .await?;
        sqlx::query("UPDATE project_fact_imports SET status='invalidated',invalidated_at=? WHERE import_id=?")
            .bind(now.to_rfc3339()).bind(&import_id).execute(&mut **tx).await?;
        for fact_id in &impact.affected_fact_ids {
            sqlx::query(
                "UPDATE facts SET status='suspended' WHERE fact_id=? AND status!='revoked'",
            )
            .bind(fact_id)
            .execute(&mut **tx)
            .await?;
            sqlx::query("UPDATE fact_assurances SET status='invalidated',invalidated_at=? WHERE fact_id=? AND status='active'")
                .bind(now.to_rfc3339()).bind(fact_id).execute(&mut **tx).await?;
        }
        for goal_id in &impact.reopened_goal_ids {
            sqlx::query("UPDATE goals SET status='open',solved_by_fact_id=NULL WHERE goal_id=?")
                .bind(goal_id)
                .execute(&mut **tx)
                .await?;
        }
        for route_id in &impact.paused_route_ids {
            sqlx::query("UPDATE routes SET status='paused',cancellation_epoch=cancellation_epoch+1 WHERE route_id=? AND status='active'")
                .bind(route_id).execute(&mut **tx).await?;
            sqlx::query("UPDATE tasks SET status='cancelled',revision=revision+1 WHERE route_id=? AND status IN ('open','assigned','running','blocked')")
                .bind(route_id).execute(&mut **tx).await?;
        }
        sqlx::query("UPDATE projects SET status='needs_human_review' WHERE project_id=? AND status NOT IN ('stopped_by_human','error')")
            .bind(&target_project_id).execute(&mut **tx).await?;
        let revision = bump_revision(tx, &target_project_id).await?;
        events.push(
            append_event(
                tx,
                &target_project_id,
                revision,
                "fact_import.invalidated",
                entity("fact_import", &import_id),
                json!({"source_fact_id":source_fact_id,"impact":impact}),
                Some(entity("fact", source_fact_id)),
            )
            .await?,
        );
    }
    Ok(events)
}

fn import_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<ProjectFactImport> {
    Ok(ProjectFactImport {
        import_id: row.try_get("import_id")?,
        target_project_id: row.try_get("target_project_id")?,
        source_project_id: row.try_get("source_project_id")?,
        source_fact_id: row.try_get("source_fact_id")?,
        content_hash: row.try_get("content_hash")?,
        assurance_snapshot: serde_json::from_str(row.try_get("assurance_snapshot_json")?)?,
        status: row.try_get("status")?,
        imported_by: row.try_get("imported_by")?,
        created_at: timestamp(row.try_get("created_at")?)?,
        invalidated_at: optional_timestamp(row.try_get("invalidated_at")?)?,
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
