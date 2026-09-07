use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use research_domain::{CommandStatus, DomainEvent, SuggestionDecision, SuggestionDisposition};
use serde_json::json;
use sqlx::{Row, Sqlite, Transaction};

use crate::{StorageResult, append_event, entity, json_text};

pub(crate) async fn resolve_plan_suggestions(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    effective_round: i64,
    revision: i64,
    plan_revision_id: Option<&str>,
    decisions: &[SuggestionDecision],
    now: &DateTime<Utc>,
) -> StorageResult<Vec<DomainEvent>> {
    let pending_rows = sqlx::query(
        "SELECT command_id,suggestion_id FROM suggestions WHERE project_id=? AND status='pending' AND effective_round<=? ORDER BY suggestion_id",
    )
    .bind(project_id)
    .bind(effective_round)
    .fetch_all(&mut **tx)
    .await?;
    let pending = pending_rows
        .iter()
        .map(|row| {
            Ok((
                row.try_get::<String, _>("suggestion_id")?,
                row.try_get::<String, _>("command_id")?,
            ))
        })
        .collect::<StorageResult<BTreeMap<_, _>>>()?;
    let pending_ids = pending.keys().cloned().collect::<BTreeSet<_>>();
    let mut indexed = BTreeMap::<&str, Vec<&SuggestionDecision>>::new();
    for decision in decisions {
        indexed
            .entry(decision.suggestion_id.as_str())
            .or_default()
            .push(decision);
    }

    let parent = || plan_revision_id.map(|id| entity("plan_revision", id));
    let unresolved_entity = || {
        plan_revision_id.map_or_else(
            || entity("project", project_id),
            |id| entity("plan_revision", id),
        )
    };
    let mut events = Vec::new();
    for (suggestion_id, candidates) in &indexed {
        if !pending_ids.contains(*suggestion_id) {
            events.push(
                append_event(
                    tx,
                    project_id,
                    revision,
                    "suggestion.decision_unresolved",
                    unresolved_entity(),
                    json!({
                        "reason":"unknown_suggestion_id",
                        "suggestion_id":suggestion_id,
                        "decision_count":candidates.len(),
                        "plan_revision_id":plan_revision_id,
                    }),
                    None,
                )
                .await?,
            );
        }
    }

    for (suggestion_id, command_id) in pending {
        let Some(candidates) = indexed.get(suggestion_id.as_str()) else {
            events.push(
                append_event(
                    tx,
                    project_id,
                    revision,
                    "suggestion.decision_unresolved",
                    entity("suggestion", &suggestion_id),
                    json!({
                        "reason":"missing_decision",
                        "suggestion_id":suggestion_id,
                        "plan_revision_id":plan_revision_id,
                    }),
                    parent(),
                )
                .await?,
            );
            continue;
        };
        if candidates.len() != 1 {
            events.push(
                append_event(
                    tx,
                    project_id,
                    revision,
                    "suggestion.decision_unresolved",
                    entity("suggestion", &suggestion_id),
                    json!({
                        "reason":"duplicate_decision",
                        "suggestion_id":suggestion_id,
                        "decision_count":candidates.len(),
                        "plan_revision_id":plan_revision_id,
                    }),
                    parent(),
                )
                .await?,
            );
            continue;
        }
        let decision = candidates[0];
        if decision.rationale.trim().is_empty() {
            events.push(
                append_event(
                    tx,
                    project_id,
                    revision,
                    "suggestion.decision_unresolved",
                    entity("suggestion", &suggestion_id),
                    json!({
                        "reason":"empty_rationale",
                        "suggestion_id":suggestion_id,
                        "plan_revision_id":plan_revision_id,
                    }),
                    parent(),
                )
                .await?,
            );
            continue;
        }

        let serialized = json_text(decision)?;
        match decision.disposition {
            SuggestionDisposition::Applied => {
                sqlx::query(
                    "UPDATE suggestions SET status='applied',decision=? WHERE suggestion_id=? AND status='pending'",
                )
                .bind(&serialized)
                .bind(&suggestion_id)
                .execute(&mut **tx)
                .await?;
                sqlx::query("UPDATE human_commands SET status=?,after_revision=?,affected_entities_json=?,applied_at=? WHERE command_id=? AND status='validated'")
                    .bind(CommandStatus::Applied.to_string())
                    .bind(revision)
                    .bind(json_text(&vec![entity("suggestion", &suggestion_id)])?)
                    .bind(now.to_rfc3339())
                    .bind(&command_id)
                    .execute(&mut **tx)
                    .await?;
                events.push(
                    append_event(
                        tx,
                        project_id,
                        revision,
                        "human_command.applied",
                        entity("command", &command_id),
                        json!({
                            "suggestion_id":suggestion_id,
                            "decision":decision,
                            "plan_revision_id":plan_revision_id,
                        }),
                        parent(),
                    )
                    .await?,
                );
            }
            SuggestionDisposition::Rejected => {
                sqlx::query(
                    "UPDATE suggestions SET status='rejected',decision=? WHERE suggestion_id=? AND status='pending'",
                )
                .bind(&serialized)
                .bind(&suggestion_id)
                .execute(&mut **tx)
                .await?;
                sqlx::query("UPDATE human_commands SET status=?,after_revision=?,affected_entities_json=?,applied_at=? WHERE command_id=? AND status='validated'")
                    .bind(CommandStatus::Rejected.to_string())
                    .bind(revision)
                    .bind(json_text(&vec![entity("suggestion", &suggestion_id)])?)
                    .bind(now.to_rfc3339())
                    .bind(&command_id)
                    .execute(&mut **tx)
                    .await?;
                events.push(
                    append_event(
                        tx,
                        project_id,
                        revision,
                        "human_command.rejected",
                        entity("command", &command_id),
                        json!({
                            "suggestion_id":suggestion_id,
                            "decision":decision,
                            "plan_revision_id":plan_revision_id,
                        }),
                        parent(),
                    )
                    .await?,
                );
            }
            SuggestionDisposition::Deferred => {
                sqlx::query("UPDATE suggestions SET decision=?,effective_round=? WHERE suggestion_id=? AND status='pending'")
                    .bind(&serialized)
                    .bind(effective_round + 1)
                    .bind(&suggestion_id)
                    .execute(&mut **tx)
                    .await?;
                events.push(
                    append_event(
                        tx,
                        project_id,
                        revision,
                        "suggestion.deferred",
                        entity("suggestion", &suggestion_id),
                        json!({
                            "suggestion_id":suggestion_id,
                            "decision":decision,
                            "effective_round":effective_round + 1,
                            "plan_revision_id":plan_revision_id,
                        }),
                        parent(),
                    )
                    .await?,
                );
            }
        }
    }
    Ok(events)
}
