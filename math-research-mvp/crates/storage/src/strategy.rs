use chrono::Utc;
use research_domain::{DomainEvent, StrategyDirectorOutput};
use serde_json::{Value, json};
use sqlx::Row;

use crate::{SqliteStore, StorageResult, append_event, bump_revision, entity, json_text, new_id};

impl SqliteStore {
    pub async fn record_strategy_state(
        &self,
        project_id: &str,
        round_id: &str,
        input_hash: &str,
        audit_kind: &str,
        trigger_reasons: &[String],
        output: &StrategyDirectorOutput,
    ) -> StorageResult<(Value, Option<DomainEvent>)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "record_strategy_state",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        if let Some(row) = sqlx::query(
            "SELECT * FROM strategy_states WHERE project_id=? AND round_id=? AND input_hash=?",
        )
        .bind(project_id)
        .bind(round_id)
        .bind(input_hash)
        .fetch_optional(&mut *tx)
        .await?
        {
            let state = strategy_state_from_row(&row)?;
            tx.rollback().await?;
            return Ok((state, None));
        }
        let strategy_state_id = new_id("strategy");
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO strategy_states(strategy_state_id,project_id,round_id,input_hash,audit_kind,trigger_reasons_json,fixed_goal,verdict_summary,proof_skeleton_json,route_portfolio_json,interface_debts_json,central_missing_bridge,method_vs_proposition_failure,dangerous_shortcuts_json,strategy_directives_json,literature_priorities_json,macro_replan_required,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&strategy_state_id)
            .bind(project_id)
            .bind(round_id)
            .bind(input_hash)
            .bind(audit_kind)
            .bind(json_text(trigger_reasons)?)
            .bind(&output.fixed_goal)
            .bind(&output.verdict_summary)
            .bind(json_text(&output.proof_skeleton)?)
            .bind(json_text(&output.route_portfolio)?)
            .bind(json_text(&output.interface_debts)?)
            .bind(&output.central_missing_bridge)
            .bind(&output.method_vs_proposition_failure)
            .bind(json_text(&output.dangerous_shortcuts)?)
            .bind(json_text(&output.strategy_directives)?)
            .bind(json_text(&output.literature_priorities)?)
            .bind(output.macro_replan_required)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            "strategy.state.recorded",
            entity("strategy_state", &strategy_state_id),
            json!({
                "round_id":round_id,
                "audit_kind":audit_kind,
                "trigger_reasons":trigger_reasons,
                "central_missing_bridge":output.central_missing_bridge,
                "macro_replan_required":output.macro_replan_required,
            }),
            Some(entity("round", round_id)),
        )
        .await?;
        let row = sqlx::query("SELECT * FROM strategy_states WHERE strategy_state_id=?")
            .bind(&strategy_state_id)
            .fetch_one(&mut *tx)
            .await?;
        let state = strategy_state_from_row(&row)?;
        tx.commit().await?;
        Ok((state, Some(event)))
    }

    pub async fn list_strategy_states(&self, project_id: &str) -> StorageResult<Vec<Value>> {
        let rows = sqlx::query("SELECT * FROM strategy_states WHERE project_id=? ORDER BY created_at,strategy_state_id")
            .bind(project_id)
            .fetch_all(self.read_pool())
            .await?;
        rows.iter().map(strategy_state_from_row).collect()
    }

    pub async fn latest_strategy_state(&self, project_id: &str) -> StorageResult<Option<Value>> {
        let row = sqlx::query("SELECT * FROM strategy_states WHERE project_id=? ORDER BY created_at DESC,strategy_state_id DESC LIMIT 1")
            .bind(project_id)
            .fetch_optional(self.read_pool())
            .await?;
        row.as_ref().map(strategy_state_from_row).transpose()
    }
}

fn strategy_state_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<Value> {
    Ok(json!({
        "strategy_state_id":row.try_get::<String,_>("strategy_state_id")?,
        "project_id":row.try_get::<String,_>("project_id")?,
        "round_id":row.try_get::<String,_>("round_id")?,
        "input_hash":row.try_get::<String,_>("input_hash")?,
        "audit_kind":row.try_get::<String,_>("audit_kind")?,
        "trigger_reasons":serde_json::from_str::<Value>(row.try_get("trigger_reasons_json")?)?,
        "fixed_goal":row.try_get::<String,_>("fixed_goal")?,
        "verdict_summary":row.try_get::<String,_>("verdict_summary")?,
        "proof_skeleton":serde_json::from_str::<Value>(row.try_get("proof_skeleton_json")?)?,
        "route_portfolio":serde_json::from_str::<Value>(row.try_get("route_portfolio_json")?)?,
        "interface_debts":serde_json::from_str::<Value>(row.try_get("interface_debts_json")?)?,
        "central_missing_bridge":row.try_get::<String,_>("central_missing_bridge")?,
        "method_vs_proposition_failure":row.try_get::<String,_>("method_vs_proposition_failure")?,
        "dangerous_shortcuts":serde_json::from_str::<Value>(row.try_get("dangerous_shortcuts_json")?)?,
        "strategy_directives":serde_json::from_str::<Value>(row.try_get("strategy_directives_json")?)?,
        "literature_priorities":serde_json::from_str::<Value>(row.try_get("literature_priorities_json")?)?,
        "macro_replan_required":row.try_get::<bool,_>("macro_replan_required")?,
        "created_at":row.try_get::<String,_>("created_at")?,
    }))
}
