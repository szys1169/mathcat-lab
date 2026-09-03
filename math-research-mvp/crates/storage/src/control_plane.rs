use chrono::{DateTime, Utc};
use research_domain::{DomainEvent, HumanQuestion, TaskSteer, UsageSummary};
use serde_json::{Value, json};
use sqlx::Row;

use crate::{
    SqliteStore, StorageError, StorageResult, append_event, bump_revision, entity, json_text,
    new_id,
};

impl SqliteStore {
    pub async fn list_task_steers(&self, task_id: &str) -> StorageResult<Vec<TaskSteer>> {
        let rows =
            sqlx::query("SELECT * FROM task_steers WHERE task_id=? ORDER BY created_at,steer_id")
                .bind(task_id)
                .fetch_all(self.pool())
                .await?;
        rows.iter().map(task_steer_from_row).collect()
    }

    pub async fn consume_pending_task_steers(
        &self,
        project_id: &str,
        task_id: &str,
        task_revision: i64,
        route_epoch: i64,
    ) -> StorageResult<(Vec<TaskSteer>, Vec<DomainEvent>)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "consume_task_steers",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let current = sqlx::query(
            "SELECT revision,route_cancellation_epoch FROM tasks WHERE project_id=? AND task_id=?",
        )
        .bind(project_id)
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "task",
            id: task_id.into(),
        })?;
        let current_revision: i64 = current.try_get("revision")?;
        let current_epoch: i64 = current.try_get("route_cancellation_epoch")?;
        if current_revision != task_revision || current_epoch != route_epoch {
            return Err(StorageError::LateSubmission(format!(
                "task {task_id} moved from revision/epoch {task_revision}/{route_epoch} to {current_revision}/{current_epoch}"
            )));
        }
        let rows = sqlx::query("SELECT * FROM task_steers WHERE project_id=? AND task_id=? AND status='pending' ORDER BY created_at,steer_id")
            .bind(project_id).bind(task_id).fetch_all(&mut *tx).await?;
        if rows.is_empty() {
            tx.rollback().await?;
            return Ok((Vec::new(), Vec::new()));
        }
        let now = Utc::now();
        let revision = bump_revision(&mut tx, project_id).await?;
        let mut steers = Vec::new();
        let mut events = Vec::new();
        for row in rows {
            let mut steer = task_steer_from_row(&row)?;
            if steer.expected_task_revision != task_revision
                || steer.expected_route_epoch != route_epoch
            {
                sqlx::query("UPDATE task_steers SET status='stale',applied_at=? WHERE steer_id=?")
                    .bind(now.to_rfc3339())
                    .bind(&steer.steer_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("UPDATE human_commands SET status='failed',error='task revision or route epoch changed',applied_at=? WHERE command_id=?")
                    .bind(now.to_rfc3339()).bind(&steer.command_id).execute(&mut *tx).await?;
                events.push(append_event(&mut tx, project_id, revision, "task.steer.rejected_stale", entity("task", task_id), json!({"steer_id":steer.steer_id,"expected_task_revision":steer.expected_task_revision,"expected_route_epoch":steer.expected_route_epoch}), Some(entity("command", &steer.command_id))).await?);
                continue;
            }
            sqlx::query("UPDATE task_steers SET status='applied',applied_at=? WHERE steer_id=?")
                .bind(now.to_rfc3339())
                .bind(&steer.steer_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE human_commands SET status='applied',after_revision=?,affected_entities_json=?,applied_at=? WHERE command_id=?")
                .bind(revision).bind(json_text(&vec![entity("task", task_id)])?)
                .bind(now.to_rfc3339()).bind(&steer.command_id).execute(&mut *tx).await?;
            steer.status = "applied".into();
            steer.applied_at = Some(now);
            events.push(append_event(&mut tx, project_id, revision, "task.steer.applied", entity("task", task_id), json!({"steer_id":steer.steer_id,"content":steer.content,"task_revision":task_revision,"route_epoch":route_epoch}), Some(entity("command", &steer.command_id))).await?);
            steers.push(steer);
        }
        tx.commit().await?;
        Ok((steers, events))
    }

    pub async fn usage_summary(&self, project_id: &str) -> StorageResult<UsageSummary> {
        self.get_project(project_id).await?;
        let total = sqlx::query("SELECT COALESCE(SUM(model_calls),0) AS model_calls,COALESCE(SUM(input_tokens),0) AS input_tokens,COALESCE(SUM(output_tokens),0) AS output_tokens,COALESCE(SUM(elapsed_ms),0) AS elapsed_ms FROM usage_records WHERE project_id=?")
            .bind(project_id).fetch_one(self.pool()).await?;
        let rows = sqlx::query("SELECT round,task_id,worker_id,model,COALESCE(SUM(model_calls),0) AS model_calls,COALESCE(SUM(input_tokens),0) AS input_tokens,COALESCE(SUM(output_tokens),0) AS output_tokens,COALESCE(SUM(elapsed_ms),0) AS elapsed_ms FROM usage_records WHERE project_id=? GROUP BY round,task_id,worker_id,model ORDER BY round,task_id")
            .bind(project_id).fetch_all(self.pool()).await?;
        let by_scope = rows.iter().map(|row| Ok(json!({
            "round":row.try_get::<Option<i64>,_>("round")?,"task_id":row.try_get::<Option<String>,_>("task_id")?,
            "worker_id":row.try_get::<Option<String>,_>("worker_id")?,"model":row.try_get::<Option<String>,_>("model")?,
            "model_calls":row.try_get::<i64,_>("model_calls")?,"input_tokens":row.try_get::<i64,_>("input_tokens")?,
            "output_tokens":row.try_get::<i64,_>("output_tokens")?,"elapsed_ms":row.try_get::<i64,_>("elapsed_ms")?
        }))).collect::<StorageResult<Vec<_>>>()?;
        Ok(UsageSummary {
            project_id: project_id.into(),
            model_calls: total.try_get("model_calls")?,
            input_tokens: total.try_get("input_tokens")?,
            output_tokens: total.try_get("output_tokens")?,
            elapsed_ms: total.try_get("elapsed_ms")?,
            // Codex CLI does not currently expose a stable billable-cost field here.
            estimated_cost_usd: None,
            by_scope,
        })
    }

    pub async fn list_budget_overrides(&self, project_id: &str) -> StorageResult<Vec<Value>> {
        let rows = sqlx::query("SELECT * FROM budget_overrides WHERE project_id=? ORDER BY created_at,budget_override_id")
            .bind(project_id).fetch_all(self.pool()).await?;
        rows.iter().map(|row| Ok(json!({
            "budget_override_id":row.try_get::<String,_>("budget_override_id")?,
            "scope_kind":row.try_get::<String,_>("scope_kind")?,"scope_id":row.try_get::<String,_>("scope_id")?,
            "limits":serde_json::from_str::<Value>(row.try_get("limits_json")?)?,"reason":row.try_get::<String,_>("reason")?,
            "requested_by":row.try_get::<String,_>("requested_by")?,"created_at":row.try_get::<String,_>("created_at")?
        }))).collect()
    }

    pub async fn create_human_question(
        &self,
        project_id: &str,
        question: &str,
        options: Vec<Value>,
        blocking_entity_ids: Vec<String>,
        asked_by: &str,
        timeout_at: Option<DateTime<Utc>>,
    ) -> StorageResult<(HumanQuestion, DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "create_human_question",
            )
            .await?;
        if question.trim().is_empty() || blocking_entity_ids.is_empty() {
            return Err(StorageError::InvalidTransition(
                "a human question needs text and at least one blocking entity".into(),
            ));
        }
        let mut tx = self.pool().begin().await?;
        let question_id = new_id("question");
        let now = Utc::now();
        sqlx::query("INSERT INTO human_questions(question_id,project_id,question,options_json,blocking_entity_ids_json,status,answer_json,asked_by,answered_by,timeout_at,created_at,answered_at) VALUES(?,?,?,?,?,'open',NULL,?,NULL,?,?,NULL)")
            .bind(&question_id).bind(project_id).bind(question).bind(json_text(&options)?)
            .bind(json_text(&blocking_entity_ids)?).bind(asked_by)
            .bind(timeout_at.map(|value| value.to_rfc3339())).bind(now.to_rfc3339())
            .execute(&mut *tx).await?;
        sqlx::query("UPDATE projects SET status='needs_human_review' WHERE project_id=? AND status NOT IN ('stopped_by_human','error')")
            .bind(project_id).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(&mut tx, project_id, revision, "human_question.asked", entity("human_question", &question_id), json!({"question":question,"options":options,"blocking_entity_ids":blocking_entity_ids,"timeout_at":timeout_at}), None).await?;
        let value = HumanQuestion {
            question_id,
            project_id: project_id.into(),
            question: question.into(),
            options,
            blocking_entity_ids,
            status: "open".into(),
            answer: None,
            asked_by: asked_by.into(),
            answered_by: None,
            timeout_at,
            created_at: now,
            answered_at: None,
        };
        tx.commit().await?;
        Ok((value, event))
    }

    pub async fn list_human_questions(
        &self,
        project_id: &str,
    ) -> StorageResult<Vec<HumanQuestion>> {
        let rows = sqlx::query(
            "SELECT * FROM human_questions WHERE project_id=? ORDER BY created_at,question_id",
        )
        .bind(project_id)
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(human_question_from_row).collect()
    }
}

fn task_steer_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<TaskSteer> {
    Ok(TaskSteer {
        steer_id: row.try_get("steer_id")?,
        project_id: row.try_get("project_id")?,
        task_id: row.try_get("task_id")?,
        command_id: row.try_get("command_id")?,
        content: row.try_get("content")?,
        expected_task_revision: row.try_get("expected_task_revision")?,
        expected_route_epoch: row.try_get("expected_route_epoch")?,
        status: row.try_get("status")?,
        created_at: timestamp(row.try_get("created_at")?)?,
        applied_at: optional_timestamp(row.try_get("applied_at")?)?,
    })
}

fn human_question_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<HumanQuestion> {
    Ok(HumanQuestion {
        question_id: row.try_get("question_id")?,
        project_id: row.try_get("project_id")?,
        question: row.try_get("question")?,
        options: serde_json::from_str(row.try_get("options_json")?)?,
        blocking_entity_ids: serde_json::from_str(row.try_get("blocking_entity_ids_json")?)?,
        status: row.try_get("status")?,
        answer: row
            .try_get::<Option<String>, _>("answer_json")?
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        asked_by: row.try_get("asked_by")?,
        answered_by: row.try_get("answered_by")?,
        timeout_at: optional_timestamp(row.try_get("timeout_at")?)?,
        created_at: timestamp(row.try_get("created_at")?)?,
        answered_at: optional_timestamp(row.try_get("answered_at")?)?,
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
