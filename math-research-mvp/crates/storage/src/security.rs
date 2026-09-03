use chrono::{DateTime, Utc};
use research_domain::{Actor, SecurityAuditEntry};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{SqliteStore, StorageError, StorageResult, new_id};

impl SqliteStore {
    pub async fn actor_count(&self) -> StorageResult<i64> {
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM actors")
            .fetch_one(self.pool())
            .await?)
    }

    pub async fn bootstrap_admin(
        &self,
        actor_id: &str,
        display_name: &str,
        token: &str,
    ) -> StorageResult<Actor> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "bootstrap_admin",
            )
            .await?;
        if actor_id.trim().is_empty() || display_name.trim().is_empty() || token.len() < 16 {
            return Err(StorageError::InvalidTransition(
                "bootstrap requires actor_id, display_name, and a token of at least 16 characters"
                    .into(),
            ));
        }
        let mut tx = self.pool().begin().await?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM actors")
            .fetch_one(&mut *tx)
            .await?;
        if count != 0 {
            return Err(StorageError::InvalidTransition(
                "an actor already exists; bootstrap is closed".into(),
            ));
        }
        let now = Utc::now();
        sqlx::query("INSERT INTO actors(actor_id,display_name,role,status,created_at) VALUES(?,?,'admin','active',?)")
            .bind(actor_id).bind(display_name).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO actor_tokens(token_id,actor_id,token_hash,created_at,revoked_at) VALUES(?,?,?,?,NULL)")
            .bind(new_id("token")).bind(actor_id).bind(hash_secret(token)).bind(now.to_rfc3339())
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Actor {
            actor_id: actor_id.into(),
            display_name: display_name.into(),
            role: "admin".into(),
            status: "active".into(),
            created_at: now,
        })
    }

    pub async fn create_actor(
        &self,
        actor_id: &str,
        display_name: &str,
        role: &str,
        token: &str,
    ) -> StorageResult<Actor> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "create_actor",
            )
            .await?;
        if !matches!(
            role,
            "viewer" | "researcher" | "reviewer" | "operator" | "admin"
        ) || token.len() < 16
        {
            return Err(StorageError::InvalidTransition(
                "invalid role or short token".into(),
            ));
        }
        let now = Utc::now();
        let mut tx = self.pool().begin().await?;
        sqlx::query("INSERT INTO actors(actor_id,display_name,role,status,created_at) VALUES(?,?,?,'active',?)")
            .bind(actor_id).bind(display_name).bind(role).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO actor_tokens(token_id,actor_id,token_hash,created_at,revoked_at) VALUES(?,?,?,?,NULL)")
            .bind(new_id("token")).bind(actor_id).bind(hash_secret(token)).bind(now.to_rfc3339())
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Actor {
            actor_id: actor_id.into(),
            display_name: display_name.into(),
            role: role.into(),
            status: "active".into(),
            created_at: now,
        })
    }

    pub async fn authenticate_actor(&self, actor_id: &str, token: &str) -> StorageResult<Actor> {
        let row = sqlx::query("SELECT a.* FROM actors a JOIN actor_tokens t ON a.actor_id=t.actor_id WHERE a.actor_id=? AND a.status='active' AND t.token_hash=? AND t.revoked_at IS NULL")
            .bind(actor_id).bind(hash_secret(token)).fetch_optional(self.pool()).await?
            .ok_or_else(|| StorageError::InvalidTransition("invalid actor credentials".into()))?;
        actor_from_row(&row)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_security_audit(
        &self,
        project_id: Option<&str>,
        actor_id: Option<&str>,
        action: &str,
        target_kind: &str,
        target_id: &str,
        decision: &str,
        reason: Option<&str>,
        request_hash: Option<&str>,
    ) -> StorageResult<SecurityAuditEntry> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::Telemetry,
                "record_security_audit",
            )
            .await?;
        let entry = SecurityAuditEntry {
            audit_id: new_id("audit"),
            project_id: project_id.map(str::to_owned),
            actor_id: actor_id.map(str::to_owned),
            action: action.into(),
            target_kind: target_kind.into(),
            target_id: target_id.into(),
            decision: decision.into(),
            reason: reason.map(str::to_owned),
            request_hash: request_hash.map(str::to_owned),
            created_at: Utc::now(),
        };
        sqlx::query("INSERT INTO security_audit(audit_id,project_id,actor_id,action,target_kind,target_id,decision,reason,request_hash,created_at) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(&entry.audit_id).bind(&entry.project_id).bind(&entry.actor_id).bind(&entry.action)
            .bind(&entry.target_kind).bind(&entry.target_id).bind(&entry.decision).bind(&entry.reason)
            .bind(&entry.request_hash).bind(entry.created_at.to_rfc3339()).execute(self.pool()).await?;
        Ok(entry)
    }

    pub async fn list_security_audit(
        &self,
        project_id: &str,
    ) -> StorageResult<Vec<SecurityAuditEntry>> {
        let rows = sqlx::query(
            "SELECT * FROM security_audit WHERE project_id=? ORDER BY created_at,audit_id",
        )
        .bind(project_id)
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(audit_from_row).collect()
    }
}

fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

fn actor_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<Actor> {
    Ok(Actor {
        actor_id: row.try_get("actor_id")?,
        display_name: row.try_get("display_name")?,
        role: row.try_get("role")?,
        status: row.try_get("status")?,
        created_at: timestamp(row.try_get("created_at")?)?,
    })
}

fn audit_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<SecurityAuditEntry> {
    Ok(SecurityAuditEntry {
        audit_id: row.try_get("audit_id")?,
        project_id: row.try_get("project_id")?,
        actor_id: row.try_get("actor_id")?,
        action: row.try_get("action")?,
        target_kind: row.try_get("target_kind")?,
        target_id: row.try_get("target_id")?,
        decision: row.try_get("decision")?,
        reason: row.try_get("reason")?,
        request_hash: row.try_get("request_hash")?,
        created_at: timestamp(row.try_get("created_at")?)?,
    })
}

fn timestamp(value: &str) -> StorageResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| StorageError::CorruptData(error.to_string()))
}
