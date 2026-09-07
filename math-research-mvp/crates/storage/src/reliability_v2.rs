use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use research_domain::{
    Bottleneck, CandidateSubmission, ContextPacket, EntityRef, FactImpactRecord,
    PlanRevisionRecord, PlannerHealth, PlannerOutput, ResearchDelta, ResearchRound, Route,
    RouteProposal, RouteStatus, StorageHealth, Task, TaskAttempt, TaskContract, TaskStatus,
    TaskSteer, Verification, WorkerInstance, WorkerOutput,
};

#[derive(Debug, Clone)]
pub struct LocalTaskOffer {
    pub task: Task,
    pub attempt: TaskAttempt,
    pub worker_instance: WorkerInstance,
    pub contract: TaskContract,
    pub context_packet: ContextPacket,
    pub resume_checkpoint: Option<Value>,
    pub events: Vec<research_domain::DomainEvent>,
}

#[derive(Debug, Clone)]
pub struct LocalTaskLease {
    pub task: Task,
    pub attempt_id: String,
    pub worker_instance_id: String,
    pub lease_id: String,
    pub lease_token: String,
    pub lease_epoch: i64,
    pub expires_at: DateTime<Utc>,
    pub events: Vec<research_domain::DomainEvent>,
}

#[derive(Debug)]
pub enum LocalResultSubmission {
    Submitted {
        result_envelope_id: String,
        events: Vec<research_domain::DomainEvent>,
    },
    SteeringPending {
        steers: Vec<TaskSteer>,
    },
}

#[derive(Debug)]
pub struct LocalResultIngestion {
    pub output: WorkerOutput,
    pub verifications: Vec<Verification>,
    pub events: Vec<research_domain::DomainEvent>,
}

pub(crate) const STARTUP_ARTIFACT_AUDIT_BATCH_SIZE: i64 = 128;

#[derive(Debug)]
struct ArtifactIntegrityIssue {
    project_id: String,
    artifact_id: String,
    reason: String,
}

#[derive(Debug)]
struct ArtifactIntegrityTarget {
    project_id: String,
    artifact_id: String,
    storage_path: String,
    expected_sha256: String,
}

#[derive(Debug)]
struct ArtifactIntegrityObservation {
    target: ArtifactIntegrityTarget,
    checked_at: DateTime<Utc>,
    observed_sha256: Option<String>,
    issue: Option<String>,
}

#[derive(Debug)]
struct ArtifactIntegrityScan {
    mode: &'static str,
    scope_key: String,
    project_id: Option<String>,
    batch_limit: Option<i64>,
    cursor_start: Option<String>,
    cursor_end: Option<String>,
    completed_cycle: bool,
    priority_unverified_checked: usize,
    started_at: DateTime<Utc>,
    completed_at: DateTime<Utc>,
    observations: Vec<ArtifactIntegrityObservation>,
}
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::io::AsyncReadExt;

use crate::{
    PlanSaveResult, SqliteStore, StorageError, StorageResult, append_event, bump_revision,
    current_revision, entity, json_text, merged_route_attributes, new_id, route_attributes,
    route_mutations::{
        TaskLeaseExpiry, expire_task_lease_tx, merge_routes_tx, prune_route_tx, revive_route_tx,
    },
    rows,
    task_materialization::{V2TaskMaterialization, materialize_v2_task_tx},
    write::{UncommittedArtifactFile, insert_artifact_record_tx},
};

impl SqliteStore {
    pub async fn create_planner_context_packet(
        &self,
        project_id: &str,
        round_id: &str,
        source_revision: i64,
        content: &Value,
    ) -> StorageResult<(ContextPacket, research_domain::DomainEvent)> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "create_planner_context_packet",
            )
            .await?;
        let project = self.get_project(project_id).await?;
        if project.revision != source_revision {
            return Err(StorageError::RevisionConflict {
                expected: source_revision,
                actual: project.revision,
            });
        }
        let problem_excerpt = truncate_chars(
            &format!(
                "Target: {}\nAssumptions: {}\nOriginal problem:\n{}",
                project.contract.target_statement,
                project.contract.assumptions.join("; "),
                project.contract.original_problem
            ),
            8_000,
        );
        let known_fact_ids = content
            .get("active_facts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|fact| fact.get("fact_id").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let failure_pattern_ids = content
            .get("failure_patterns")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|pattern| pattern.get("pattern_id").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let uncertainty_ids = content
            .get("relevant_uncertainties")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|uncertainty| uncertainty.get("uncertainty_id").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let mut source_refs = vec![entity("round", round_id)];
        source_refs.extend(known_fact_ids.iter().map(|id| entity("fact", id)));
        source_refs.extend(
            failure_pattern_ids
                .iter()
                .map(|id| entity("failure_pattern", id)),
        );
        source_refs.extend(uncertainty_ids.iter().map(|id| entity("uncertainty", id)));
        if let Some(delta_id) = content
            .get("research_delta")
            .and_then(|delta| delta.get("delta_id"))
            .and_then(Value::as_str)
        {
            source_refs.push(entity("research_delta", delta_id));
        }
        let omitted = content
            .get("omitted")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let content_hash = sha256_json(content)?;
        let token_estimate =
            i64::try_from(serde_json::to_string(content)?.chars().count().div_ceil(4))
                .unwrap_or(i64::MAX);
        let context_packet_id = new_id("context");
        let now = Utc::now();
        let mut tx = self.pool().begin().await?;
        sqlx::query("INSERT INTO context_packets(context_packet_id,project_id,task_id,route_id,packet_kind,source_revision,problem_contract_excerpt,objective,known_fact_ids_json,bottleneck_id,route_progress_json,relevant_failure_pattern_ids_json,relevant_uncertainty_ids_json,source_refs_json,omitted_sections_json,token_estimate,content_json,content_hash,status,created_at) VALUES(?,?,NULL,NULL,'planner',?,?,?,?,NULL,'{}',?,?,?,?,?,?,?,'active',?)")
            .bind(&context_packet_id).bind(project_id).bind(source_revision).bind(&problem_excerpt)
            .bind("reconcile delta, routes, bottlenecks, and task portfolio")
            .bind(json_text(&known_fact_ids)?).bind(json_text(&failure_pattern_ids)?)
            .bind(json_text(&uncertainty_ids)?).bind(json_text(&source_refs)?).bind(json_text(&omitted)?)
            .bind(token_estimate).bind(json_text(content)?).bind(&content_hash).bind(now.to_rfc3339())
            .execute(&mut *tx).await?;
        let summary_id = new_id("summary");
        let summary_content = truncate_chars(&serde_json::to_string_pretty(content)?, 16_000);
        let summary_hash = hex::encode(Sha256::digest(summary_content.as_bytes()));
        let input_entity_ids = source_refs
            .iter()
            .map(|reference| reference.id.clone())
            .collect::<Vec<_>>();
        sqlx::query("INSERT OR IGNORE INTO context_summaries(summary_id,project_id,summary_kind,scope_kind,scope_id,input_entity_ids_json,input_revision,summarizer_version,content,content_hash,omitted_categories_json,status,rebuild_condition,created_at) VALUES(?,?,'project_digest','project',?,?,?,'deterministic-v2',?,?,?,'active','rebuild on any newer research delta',?)")
            .bind(&summary_id).bind(project_id).bind(project_id).bind(json_text(&input_entity_ids)?)
            .bind(source_revision).bind(&summary_content).bind(&summary_hash).bind(json_text(&omitted)?)
            .bind(now.to_rfc3339()).execute(&mut *tx).await?;
        let event = append_event(
            &mut tx,
            project_id,
            source_revision,
            "context.packet.created",
            entity("context_packet", &context_packet_id),
            json!({"packet_kind":"planner","round_id":round_id,"source_revision":source_revision,"content_hash":content_hash,"token_estimate":token_estimate}),
            Some(entity("round", round_id)),
        )
        .await?;
        tx.commit().await?;
        let packet = self.get_context_packet(&context_packet_id).await?;
        Ok((packet, event))
    }

    pub async fn collect_research_delta(&self, project_id: &str) -> StorageResult<ResearchDelta> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "collect_research_delta",
            )
            .await?;
        let project = self.get_project(project_id).await?;
        let from_revision: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(consumed_delta_to),0) FROM plan_revisions WHERE project_id=? AND status='committed'",
        )
        .bind(project_id)
        .fetch_one(self.pool())
        .await?;
        let to_revision = project.revision;
        if let Some(row) = sqlx::query(
            "SELECT * FROM research_deltas WHERE project_id=? AND from_revision=? AND to_revision=?",
        )
        .bind(project_id)
        .bind(from_revision)
        .bind(to_revision)
        .fetch_optional(self.pool())
        .await?
        {
            return research_delta_from_row(&row);
        }

        let rows = sqlx::query(
            "SELECT type,entity_json,data_json FROM events WHERE project_id=? AND project_revision>? AND project_revision<=? ORDER BY cursor",
        )
        .bind(project_id)
        .bind(from_revision)
        .bind(to_revision)
        .fetch_all(self.pool())
        .await?;
        let mut accepted_facts = BTreeSet::new();
        let mut rejected_candidates = BTreeSet::new();
        let mut proof_debts = BTreeSet::new();
        let mut solved_goals = BTreeSet::new();
        let mut reopened_goals = BTreeSet::new();
        let mut changed_uncertainties = BTreeSet::new();
        let mut failure_patterns = BTreeSet::new();
        let mut changed_sources = BTreeSet::new();
        let mut completed_attempts = BTreeSet::new();
        let mut failed_attempts = BTreeSet::new();
        let mut human_commands = BTreeSet::new();
        let mut route_changes = Vec::new();
        for row in rows {
            let event_type: String = row.try_get("type")?;
            let event_entity: EntityRef = serde_json::from_str(row.try_get("entity_json")?)?;
            let data: Value = serde_json::from_str(row.try_get("data_json")?)?;
            match event_type.as_str() {
                "fact.accepted" | "fact.reverified" => {
                    accepted_facts.insert(event_entity.id);
                }
                "verification.finished"
                    if data.get("verdict").and_then(Value::as_str) == Some("rejected") =>
                {
                    if let Some(id) = data.get("candidate_id").and_then(Value::as_str) {
                        rejected_candidates.insert(id.to_owned());
                    }
                }
                "verification.finding.recorded" => {
                    if let Some(id) = data.get("finding_id").and_then(Value::as_str) {
                        proof_debts.insert(id.to_owned());
                    }
                }
                "goal.solved" => {
                    solved_goals.insert(event_entity.id);
                }
                "goal.reopened" | "fact.revoked" | "fact.suspended" => {
                    if let Some(ids) = data.get("reopened_goal_ids").and_then(Value::as_array) {
                        reopened_goals
                            .extend(ids.iter().filter_map(Value::as_str).map(ToOwned::to_owned));
                    }
                }
                value if value.starts_with("uncertainty.") => {
                    changed_uncertainties.insert(event_entity.id);
                }
                "failure_pattern.created" => {
                    failure_patterns.insert(event_entity.id);
                }
                value if value.starts_with("source.") => {
                    changed_sources.insert(event_entity.id);
                }
                "task.completed" | "task.result_ingested" => {
                    completed_attempts.insert(
                        data.get("attempt_id")
                            .and_then(Value::as_str)
                            .unwrap_or(&event_entity.id)
                            .to_owned(),
                    );
                }
                "task.failed" | "task.attempt_failed" | "task.lease_expired" => {
                    failed_attempts.insert(
                        data.get("attempt_id")
                            .and_then(Value::as_str)
                            .unwrap_or(&event_entity.id)
                            .to_owned(),
                    );
                }
                value if value.starts_with("human_command.") => {
                    human_commands.insert(event_entity.id);
                }
                value if value.starts_with("route.") => route_changes.push(json!({
                    "event_type": event_type,
                    "route_id": event_entity.id,
                    "data": data,
                })),
                _ => {}
            }
        }
        let mut human_command_details = Vec::with_capacity(human_commands.len());
        for command_id in &human_commands {
            if let Some(row) = sqlx::query("SELECT command_id,type,target_kind,target_id,mode,payload_json,reason,requested_by,status,before_revision,after_revision,created_at,applied_at FROM human_commands WHERE project_id=? AND command_id=?")
                .bind(project_id)
                .bind(command_id)
                .fetch_optional(self.pool())
                .await?
            {
                human_command_details.push(json!({
                    "command_id": row.try_get::<String, _>("command_id")?,
                    "type": row.try_get::<String, _>("type")?,
                    "target_kind": row.try_get::<String, _>("target_kind")?,
                    "target_id": row.try_get::<String, _>("target_id")?,
                    "mode": row.try_get::<String, _>("mode")?,
                    "payload": serde_json::from_str::<Value>(row.try_get("payload_json")?)?,
                    "reason": row.try_get::<String, _>("reason")?,
                    "requested_by": row.try_get::<String, _>("requested_by")?,
                    "status": row.try_get::<String, _>("status")?,
                    "before_revision": row.try_get::<Option<i64>, _>("before_revision")?,
                    "after_revision": row.try_get::<Option<i64>, _>("after_revision")?,
                    "created_at": row.try_get::<String, _>("created_at")?,
                    "applied_at": row.try_get::<Option<String>, _>("applied_at")?,
                    "trust_class": "human_planning_directive_not_mathematical_fact",
                }));
            }
        }
        let created_at = Utc::now();
        let delta_id = new_id("delta");
        let payload = json!({
            "accepted_fact_ids": accepted_facts,
            "rejected_candidate_ids": rejected_candidates,
            "new_proof_debt_ids": proof_debts,
            "solved_goal_ids": solved_goals,
            "reopened_goal_ids": reopened_goals,
            "changed_uncertainty_ids": changed_uncertainties,
            "new_failure_pattern_ids": failure_patterns,
            "changed_source_ids": changed_sources,
            "completed_task_attempt_ids": completed_attempts,
            "failed_or_expired_attempt_ids": failed_attempts,
            "human_command_ids": human_commands,
            "human_commands": human_command_details,
            "route_state_changes": route_changes,
        });
        let mut tx = self.pool().begin().await?;
        sqlx::query("INSERT INTO research_deltas(delta_id,project_id,from_revision,to_revision,payload_json,status,created_at) VALUES(?,?,?,?,?,'open',?)")
            .bind(&delta_id).bind(project_id).bind(from_revision).bind(to_revision)
            .bind(json_text(&payload)?).bind(created_at.to_rfc3339()).execute(&mut *tx).await?;
        let event = append_event(
            &mut tx,
            project_id,
            to_revision,
            "planning.delta.created",
            entity("research_delta", &delta_id),
            json!({"from_revision":from_revision,"to_revision":to_revision}),
            None,
        )
        .await?;
        tx.commit().await?;
        let _ = event;
        delta_from_payload(
            delta_id,
            project_id,
            from_revision,
            to_revision,
            payload,
            "open".into(),
            None,
            created_at,
            None,
        )
    }

    pub async fn list_research_deltas(
        &self,
        project_id: &str,
    ) -> StorageResult<Vec<ResearchDelta>> {
        self.get_project(project_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM research_deltas WHERE project_id=? ORDER BY to_revision DESC,created_at DESC",
        )
        .bind(project_id)
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(research_delta_from_row).collect()
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn save_plan_v2(
        &self,
        project_id: &str,
        round: &ResearchRound,
        plan: &PlannerOutput,
        route_scores: &[f64],
        delta: &ResearchDelta,
        planner_mode: &str,
        planner_context_packet_id: Option<&str>,
    ) -> StorageResult<PlanSaveResult> {
        if route_scores.len() != plan.routes.len() {
            return Err(StorageError::InvalidTransition(format!(
                "V2 route scores must align with plan routes: expected {}, received {}",
                plan.routes.len(),
                route_scores.len()
            )));
        }
        if let Some(index) = route_scores.iter().position(|score| !score.is_finite()) {
            return Err(StorageError::InvalidTransition(format!(
                "V2 route score at index {index} must be finite"
            )));
        }
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "commit_plan_revision",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let actual = current_revision(&mut tx, project_id).await?;
        let expected = round.based_on_revision + 1;
        let planning_events_are_safe = match actual.cmp(&expected) {
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => true,
            std::cmp::Ordering::Greater => {
                let intervening = sqlx::query(
                "SELECT type,data_json FROM events WHERE project_id=? AND project_revision>? AND project_revision<=? ORDER BY project_revision",
            )
            .bind(project_id)
            .bind(expected)
            .bind(actual)
            .fetch_all(&mut *tx)
            .await?;
                let mut strategy_revision_events = 0_usize;
                let mut safe = actual == expected + 1;
                for event in intervening {
                    let event_type: String = event.try_get("type")?;
                    let data: Value = serde_json::from_str(event.try_get("data_json")?)?;
                    let same_round = data.get("round_id").and_then(Value::as_str)
                        == Some(round.round_id.as_str());
                    if event_type == "strategy.state.recorded" {
                        strategy_revision_events += 1;
                    }
                    safe &= same_round
                        && (event_type == "strategy.state.recorded"
                            || event_type.starts_with("planning.stage."));
                }
                safe && strategy_revision_events == 1
            }
        };
        if !planning_events_are_safe || delta.to_revision > actual {
            return Err(StorageError::RevisionConflict { expected, actual });
        }
        let round_status: String =
            sqlx::query_scalar("SELECT status FROM rounds WHERE round_id=? AND project_id=?")
                .bind(&round.round_id)
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?;
        if round_status != "planning" {
            return Err(StorageError::InvalidTransition(
                "V2 plan commit requires an active planning round".into(),
            ));
        }
        let project_row = sqlx::query(
            "SELECT problem_contract_json,budget_json,human_route_approval FROM projects WHERE project_id=?",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        let contract: research_domain::ProblemContract =
            serde_json::from_str(project_row.try_get("problem_contract_json")?)?;
        let budget: research_domain::Budget =
            serde_json::from_str(project_row.try_get("budget_json")?)?;
        let human_route_approval: bool = project_row.try_get("human_route_approval")?;
        let main_goal_id: String = sqlx::query_scalar(
            "SELECT goal_id FROM goals WHERE project_id=? ORDER BY priority DESC LIMIT 1",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        let ordinal: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(ordinal),0)+1 FROM plan_revisions WHERE project_id=?",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        let previous_plan_base: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(based_on_project_revision),0) FROM plan_revisions WHERE project_id=? AND status='committed'",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        let plan_revision_id = new_id("planrev");
        let now = Utc::now();
        sqlx::query("INSERT INTO plan_revisions(plan_revision_id,project_id,ordinal,round_id,based_on_project_revision,consumed_delta_from,consumed_delta_to,planner_mode,context_packet_id,route_decisions_json,bottleneck_updates_json,task_contract_ids_json,status,rationale,created_at) VALUES(?,?,?,?,?,?,?,?,?,'[]','[]','[]','preparing',?,?)")
            .bind(&plan_revision_id).bind(project_id).bind(ordinal).bind(&round.round_id).bind(actual)
            .bind(delta.from_revision).bind(delta.to_revision).bind(planner_mode).bind(planner_context_packet_id)
            .bind(&plan.rationale_summary).bind(now.to_rfc3339()).execute(&mut *tx).await?;

        // Reconcile routes that have accumulated terminal attempts without material progress.
        let stale_routes = sqlx::query("SELECT r.route_id,r.semantic_fingerprint,r.consecutive_no_progress_plans FROM routes r WHERE r.project_id=? AND r.status IN ('active','incubating','probation','blocked','revived') AND COALESCE(r.last_material_progress_revision,r.created_at_revision)<=? AND EXISTS (SELECT 1 FROM tasks t WHERE t.route_id=r.route_id) AND NOT EXISTS (SELECT 1 FROM tasks t WHERE t.route_id=r.route_id AND t.status IN ('queued','offered','leased','running','checkpointed','result_submitted','ingesting'))")
            .bind(project_id).bind(previous_plan_base).fetch_all(&mut *tx).await?;
        let mut route_decisions = Vec::new();
        for stale in stale_routes {
            let route_id: String = stale.try_get("route_id")?;
            let count: i64 = stale.try_get::<i64, _>("consecutive_no_progress_plans")? + 1;
            let status = if count >= 2 { "pruned" } else { "probation" };
            sqlx::query(
                "UPDATE routes SET consecutive_no_progress_plans=?,status=? WHERE route_id=?",
            )
            .bind(count)
            .bind(status)
            .bind(&route_id)
            .execute(&mut *tx)
            .await?;
            route_decisions.push(json!({"route_id":route_id,"decision":status,"reason":"no material progress after terminal attempts","consecutive_no_progress_plans":count}));
            if status == "pruned" {
                let fingerprint: String = stale
                    .try_get::<Option<String>, _>("semantic_fingerprint")?
                    .unwrap_or_else(|| route_id.clone());
                sqlx::query("INSERT INTO route_tombstones(tombstone_id,project_id,route_id,semantic_fingerprint,reason,revive_only_if_json,created_revision,created_at) VALUES(?,?,?,?,?,?,?,?)")
                    .bind(new_id("tombstone")).bind(project_id).bind(&route_id).bind(fingerprint)
                    .bind("route reached the no-material-progress limit")
                    .bind(json_text(&vec!["new verified evidence changes a failed premise","a linked bottleneck is resolved","explicit human revival"])?)
                    .bind(actual).bind(now.to_rfc3339()).execute(&mut *tx).await?;
                sqlx::query("UPDATE tasks SET status='cancelled',revision=revision+1,result_summary=COALESCE(result_summary,'route pruned after no material progress') WHERE route_id=? AND status IN ('queued','offered','leased','running','checkpointed')")
                    .bind(&route_id).execute(&mut *tx).await?;
            }
        }

        let active_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM routes WHERE project_id=? AND status IN ('incubating','active','blocked','probation','revived')")
            .bind(project_id).fetch_one(&mut *tx).await?;
        let mut live_count = usize::try_from(active_count.max(0)).unwrap_or(usize::MAX);
        let live_goal_rows = sqlx::query("SELECT target_goal_ids_json FROM routes WHERE project_id=? AND status IN ('incubating','active','blocked','probation','revived')")
            .bind(project_id).fetch_all(&mut *tx).await?;
        let mut live_per_goal = BTreeMap::<String, usize>::new();
        for row in live_goal_rows {
            for goal_id in
                serde_json::from_str::<Vec<String>>(row.try_get("target_goal_ids_json")?)?
            {
                *live_per_goal.entry(goal_id).or_default() += 1;
            }
        }
        let mut new_count = 0_usize;
        let mut accepted_human_proposals = Vec::<(String, String)>::new();
        let mut rejected_human_proposals = Vec::<(String, String)>::new();
        let mut pending_approval_route_ids = BTreeSet::<String>::new();
        let mut proposal_routes: Vec<Option<Route>> = Vec::with_capacity(plan.routes.len());
        let mut returned_routes = Vec::new();
        for (proposal_index, proposal) in plan.routes.iter().enumerate() {
            let score = route_scores[proposal_index];
            let target_goal_ids = if proposal.target_goal_ids.is_empty() {
                vec![main_goal_id.clone()]
            } else {
                proposal.target_goal_ids.clone()
            };
            let fingerprint = route_fingerprint(
                &target_goal_ids,
                &proposal.method_summary,
                &proposal.required_fact_ids,
            )?;
            let tombstoned: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM route_tombstones WHERE project_id=? AND semantic_fingerprint=? AND revived_revision IS NULL")
                .bind(project_id).bind(&fingerprint).fetch_one(&mut *tx).await?;
            if tombstoned > 0 {
                route_decisions.push(json!({"semantic_fingerprint":fingerprint,"decision":"deferred","reason":"matching route tombstone has no satisfied revival condition"}));
                proposal_routes.push(None);
                continue;
            }
            if let Some(row) = sqlx::query("SELECT * FROM routes WHERE project_id=? AND semantic_fingerprint=? AND status NOT IN ('merged','pruned','human_stopped') ORDER BY created_in_round LIMIT 1")
                .bind(project_id).bind(&fingerprint).fetch_optional(&mut *tx).await?
            {
                let route_id: String = row.try_get("route_id")?;
                if row.try_get::<String, _>("human_review")? == "pending" {
                    pending_approval_route_ids.insert(route_id.clone());
                }
                let attributes = merged_route_attributes(
                    row.try_get::<String, _>("attributes_json")?.as_str(),
                    proposal,
                )?;
                sqlx::query("UPDATE routes SET score=?,priority=?,attributes_json=?,status=CASE WHEN status='probation' THEN 'active' ELSE status END WHERE route_id=?")
                    .bind(score).bind(score).bind(attributes).bind(&route_id).execute(&mut *tx).await?;
                let refreshed = sqlx::query("SELECT * FROM routes WHERE route_id=?")
                    .bind(&route_id).fetch_one(&mut *tx).await?;
                let route = rows::route(&refreshed)?;
                route_decisions.push(json!({"route_id":route_id,"decision":"retained","reason":"semantic fingerprint matched a live route"}));
                accepted_human_proposals.extend(
                    accept_matching_human_proposals(
                        &mut tx,
                        project_id,
                        proposal,
                        &route_id,
                        &main_goal_id,
                        &now,
                    )
                    .await?,
                );
                proposal_routes.push(Some(route.clone()));
                returned_routes.push(route);
                continue;
            }
            let per_goal_full = target_goal_ids
                .iter()
                .any(|goal_id| live_per_goal.get(goal_id).copied().unwrap_or_default() >= 3);
            if live_count >= 6 || new_count >= 2 || per_goal_full {
                route_decisions.push(json!({"semantic_fingerprint":fingerprint,"decision":"deferred","reason":"hard route capacity or per-goal slot limit"}));
                proposal_routes.push(None);
                continue;
            }
            let family_key = route_family_key(&target_goal_ids, &proposal.method_summary)?;
            let family_id = if let Some(id) = sqlx::query_scalar::<_, String>(
                "SELECT family_id FROM route_families WHERE project_id=? AND semantic_key=?",
            )
            .bind(project_id)
            .bind(&family_key)
            .fetch_optional(&mut *tx)
            .await?
            {
                id
            } else {
                let id = new_id("routefamily");
                sqlx::query("INSERT INTO route_families(family_id,project_id,semantic_key,status,created_revision,updated_revision) VALUES(?,?,?,'active',?,?)")
                    .bind(&id).bind(project_id).bind(&family_key).bind(actual).bind(actual).execute(&mut *tx).await?;
                id
            };
            let same_family_active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM routes WHERE project_id=? AND family_id=? AND status IN ('incubating','active','blocked','probation','revived')")
                .bind(project_id).bind(&family_id).fetch_one(&mut *tx).await?;
            if same_family_active > 0 {
                let canonical_route_id: String = sqlx::query_scalar("SELECT route_id FROM routes WHERE project_id=? AND family_id=? AND status IN ('incubating','active','blocked','probation','revived') ORDER BY priority DESC,created_in_round LIMIT 1")
                    .bind(project_id).bind(&family_id).fetch_one(&mut *tx).await?;
                if let Some(merged_route_id) = sqlx::query_scalar::<_,String>("SELECT route_id FROM routes WHERE project_id=? AND semantic_fingerprint=? AND status='merged' AND merged_into=? ORDER BY created_in_round LIMIT 1")
                    .bind(project_id).bind(&fingerprint).bind(&canonical_route_id).fetch_optional(&mut *tx).await?
                {
                    route_decisions.push(json!({"route_id":merged_route_id,"semantic_fingerprint":fingerprint,"family_id":family_id,"decision":"deferred","merged_into":canonical_route_id,"reason":"same proposal was already provenance-merged into the canonical family route"}));
                } else {
                    let merged_route_id=new_id("route");
                    let attributes = route_attributes(proposal);
                    sqlx::query("INSERT INTO routes(route_id,project_id,title,method_summary,target_goal_ids_json,required_fact_ids_json,status,score,priority,cancellation_epoch,created_in_round,attributes_json,family_id,semantic_fingerprint,created_at_revision,merged_into,merge_reason,exit_criteria_json) VALUES(?,?,?,?,?,?,'merged',?,?,0,?,?,?,?,?,?,?,'[]')")
                        .bind(&merged_route_id).bind(project_id).bind(&proposal.title).bind(&proposal.method_summary)
                        .bind(json_text(&target_goal_ids)?).bind(json_text(&proposal.required_fact_ids)?).bind(score).bind(score)
                        .bind(round.number).bind(json_text(&attributes)?).bind(&family_id).bind(&fingerprint).bind(actual)
                        .bind(&canonical_route_id).bind("same semantic route family exceeded live-family capacity")
                        .execute(&mut *tx).await?;
                    route_decisions.push(json!({"route_id":merged_route_id,"semantic_fingerprint":fingerprint,"family_id":family_id,"decision":"merged","merged_into":canonical_route_id,"reason":"same-family live capacity is one; provenance retained as a merged route"}));
                }
                accepted_human_proposals.extend(
                    accept_matching_human_proposals(
                        &mut tx,
                        project_id,
                        proposal,
                        &canonical_route_id,
                        &main_goal_id,
                        &now,
                    )
                    .await?,
                );
                proposal_routes.push(None);
                continue;
            }
            let route_id = new_id("route");
            let attributes = route_attributes(proposal);
            let exit_criteria = vec![
                "target goals completed or refuted".to_owned(),
                "verified counterexample invalidates a necessary premise".to_owned(),
                "two consecutive plan revisions without material progress".to_owned(),
            ];
            let human_review = if human_route_approval {
                "pending"
            } else {
                "not_required"
            };
            sqlx::query("INSERT INTO routes(route_id,project_id,title,method_summary,target_goal_ids_json,required_fact_ids_json,status,score,priority,cancellation_epoch,created_in_round,attributes_json,family_id,semantic_fingerprint,created_at_revision,exit_criteria_json,human_review) VALUES(?,?,?,?,?,?,'active',?,?,0,?,?,?,?,?,?,?)")
                .bind(&route_id).bind(project_id).bind(&proposal.title).bind(&proposal.method_summary)
                .bind(json_text(&target_goal_ids)?).bind(json_text(&proposal.required_fact_ids)?)
                .bind(score).bind(score).bind(round.number).bind(json_text(&attributes)?)
                .bind(&family_id).bind(&fingerprint).bind(actual).bind(json_text(&exit_criteria)?).bind(human_review).execute(&mut *tx).await?;
            if human_route_approval {
                pending_approval_route_ids.insert(route_id.clone());
            }
            sqlx::query("UPDATE route_families SET canonical_route_id=COALESCE(canonical_route_id,?),updated_revision=? WHERE family_id=?")
                .bind(&route_id).bind(actual).bind(&family_id).execute(&mut *tx).await?;
            let hypothesis_id = new_id("hyp");
            sqlx::query("INSERT INTO hypotheses(hypothesis_id,project_id,kind,statement,status,route_id,created_in_round,attributes_json) VALUES(?,?,?,?,?,?,?,?)")
                .bind(&hypothesis_id).bind(project_id).bind("route").bind(&proposal.method_summary)
                .bind("active").bind(&route_id).bind(round.number).bind(json_text(&attributes)?).execute(&mut *tx).await?;
            for goal_id in &target_goal_ids {
                sqlx::query("INSERT OR IGNORE INTO hypothesis_edges(edge_id,project_id,source_id,target_id,kind) VALUES(?,?,?,?,?)")
                    .bind(new_id("edge")).bind(project_id).bind(&route_id).bind(goal_id).bind("targets").execute(&mut *tx).await?;
            }
            let route = Route {
                route_id: route_id.clone(),
                project_id: project_id.into(),
                title: proposal.title.clone(),
                method_summary: proposal.method_summary.clone(),
                target_goal_ids,
                required_fact_ids: proposal.required_fact_ids.clone(),
                status: RouteStatus::Active,
                score,
                priority: score,
                cancellation_epoch: 0,
                created_in_round: round.number,
                family_id: Some(family_id.clone()),
                semantic_fingerprint: Some(fingerprint.clone()),
                consecutive_no_progress_plans: 0,
                failed_attempt_count: 0,
                created_at_revision: actual,
                last_material_progress_revision: None,
                merged_into: None,
                merge_reason: None,
                exit_criteria,
                attributes,
            };
            route_decisions.push(json!({"route_id":route_id,"family_id":family_id,"semantic_fingerprint":fingerprint,"decision":"created","reason":"filled an available bottleneck route slot"}));
            accepted_human_proposals.extend(
                accept_matching_human_proposals(
                    &mut tx,
                    project_id,
                    proposal,
                    &route_id,
                    &main_goal_id,
                    &now,
                )
                .await?,
            );
            proposal_routes.push(Some(route.clone()));
            for goal_id in &route.target_goal_ids {
                *live_per_goal.entry(goal_id.clone()).or_default() += 1;
            }
            returned_routes.push(route);
            live_count += 1;
            new_count += 1;
        }

        let unmatched_proposals = sqlx::query(
            "SELECT proposal_id FROM human_route_proposals WHERE project_id=? AND status='queued' ORDER BY created_at,proposal_id",
        )
        .bind(project_id)
        .fetch_all(&mut *tx)
        .await?;
        for row in unmatched_proposals {
            let proposal_id: String = row.try_get("proposal_id")?;
            let reason = "Planner committed a plan without adopting a matching route after goal, dependency, duplicate, and capacity checks".to_owned();
            sqlx::query("UPDATE human_route_proposals SET status='rejected',decision_reason=?,decided_at=? WHERE proposal_id=? AND status='queued'")
                .bind(&reason)
                .bind(now.to_rfc3339())
                .bind(&proposal_id)
                .execute(&mut *tx)
                .await?;
            rejected_human_proposals.push((proposal_id, reason));
        }

        let open_bottlenecks = sqlx::query("SELECT * FROM bottlenecks WHERE project_id=? AND status='open' ORDER BY priority DESC,created_at")
            .bind(project_id).fetch_all(&mut *tx).await?;
        let mut tasks = Vec::new();
        let mut workers = Vec::new();
        let mut task_contract_ids = Vec::new();
        for assignment in &plan.assignments {
            let Some(Some(route)) = proposal_routes.get(assignment.route_index) else {
                continue;
            };
            // Pending human approval freezes admission, not materialization. Both
            // Planner routes and direct human routes use the same immutable V2
            // Task/ContextPacket/TaskContract constructor.
            if let Some(materialized) = materialize_v2_task_tx(
                &mut tx,
                V2TaskMaterialization {
                    project_id,
                    route,
                    assignment,
                    problem_contract: &contract,
                    budget: &budget,
                    round_number: round.number,
                    plan_revision_id: &plan_revision_id,
                    source_revision: actual,
                    source_command_id: None,
                    now: &now,
                },
            )
            .await?
            {
                task_contract_ids.push(materialized.task_contract_id);
                workers.push(materialized.worker);
                tasks.push(materialized.task);
            }
        }

        let pending_impacts = sqlx::query("SELECT impact_id,fact_id,closed_goal_ids_json,unblocked_route_ids_json,invalidated_task_ids_json,newly_enabled_task_templates_json,dominated_route_ids_json,resolved_uncertainty_ids_json FROM fact_impact_records WHERE project_id=? AND planner_disposition='pending' AND source_revision<=? ORDER BY source_revision")
            .bind(project_id).bind(delta.to_revision).fetch_all(&mut *tx).await?;
        for impact in pending_impacts {
            let impact_id: String = impact.try_get("impact_id")?;
            let fact_id: String = impact.try_get("fact_id")?;
            let closed: Vec<String> =
                serde_json::from_str(impact.try_get("closed_goal_ids_json")?)?;
            let unblocked: Vec<String> =
                serde_json::from_str(impact.try_get("unblocked_route_ids_json")?)?;
            let invalidated: Vec<String> =
                serde_json::from_str(impact.try_get("invalidated_task_ids_json")?)?;
            let newly_enabled: Vec<String> =
                serde_json::from_str(impact.try_get("newly_enabled_task_templates_json")?)?;
            let dominated: Vec<String> =
                serde_json::from_str(impact.try_get("dominated_route_ids_json")?)?;
            let resolved: Vec<String> =
                serde_json::from_str(impact.try_get("resolved_uncertainty_ids_json")?)?;
            let material = !closed.is_empty()
                || !unblocked.is_empty()
                || !invalidated.is_empty()
                || !newly_enabled.is_empty()
                || !dominated.is_empty()
                || !resolved.is_empty();
            let (disposition, reason, affected) = if material {
                let mut affected = closed.clone();
                affected.extend(unblocked);
                affected.extend(invalidated);
                affected.extend(newly_enabled);
                affected.extend(dominated);
                affected.extend(resolved.clone());
                (
                    "consumed",
                    "fact changed at least one goal, route, task, bottleneck template, or uncertainty",
                    affected,
                )
            } else {
                (
                    "deferred",
                    "active fact is not currently on the critical path",
                    Vec::<String>::new(),
                )
            };
            sqlx::query("INSERT INTO plan_fact_decisions(decision_id,project_id,plan_revision_id,fact_id,disposition,reason,affected_entity_ids_json,created_at) VALUES(?,?,?,?,?,?,?,?)")
                .bind(new_id("factdecision")).bind(project_id).bind(&plan_revision_id).bind(&fact_id)
                .bind(disposition).bind(reason).bind(json_text(&affected)?).bind(now.to_rfc3339()).execute(&mut *tx).await?;
            sqlx::query("UPDATE fact_impact_records SET planner_disposition=?,disposition_reason=?,applied_at=? WHERE impact_id=?")
                .bind(disposition).bind(reason).bind(now.to_rfc3339()).bind(&impact_id).execute(&mut *tx).await?;
        }

        let mut bottleneck_updates = Vec::new();
        for row in &open_bottlenecks {
            let bottleneck_id: String = row.try_get("bottleneck_id")?;
            let target_goal_ids: Vec<String> =
                serde_json::from_str(row.try_get("target_goal_ids_json")?)?;
            let covered = tasks.iter().any(|task| {
                task.goal_ids
                    .iter()
                    .any(|goal| target_goal_ids.contains(goal))
            });
            bottleneck_updates.push(json!({
                "bottleneck_id":bottleneck_id,
                "decision":if covered {"covered"} else {"deferred"}
            }));
        }
        let decided_route_ids = route_decisions
            .iter()
            .filter_map(|decision| decision.get("route_id").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>();
        let otherwise_retained = sqlx::query("SELECT route_id,status,family_id FROM routes WHERE project_id=? AND status IN ('incubating','active','blocked','probation','revived') ORDER BY route_id")
            .bind(project_id).fetch_all(&mut *tx).await?;
        for retained in otherwise_retained {
            let route_id: String = retained.try_get("route_id")?;
            if !decided_route_ids.contains(&route_id) {
                route_decisions.push(json!({
                    "route_id":route_id,
                    "decision":"retained",
                    "status":retained.try_get::<String,_>("status")?,
                    "family_id":retained.try_get::<Option<String>,_>("family_id")?,
                    "reason":"live route remained within capacity and had no invalidating delta",
                }));
            }
        }
        sqlx::query("UPDATE plan_revisions SET route_decisions_json=?,bottleneck_updates_json=?,task_contract_ids_json=?,status='committed',committed_at=? WHERE plan_revision_id=?")
            .bind(json_text(&route_decisions)?).bind(json_text(&bottleneck_updates)?).bind(json_text(&task_contract_ids)?)
            .bind(now.to_rfc3339()).bind(&plan_revision_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE research_deltas SET status='consumed',consumed_by_plan_revision_id=?,consumed_at=? WHERE delta_id=? AND status='open'")
            .bind(&plan_revision_id).bind(now.to_rfc3339()).bind(&delta.delta_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE rounds SET status='running' WHERE round_id=?")
            .bind(&round.round_id)
            .execute(&mut *tx)
            .await?;
        if human_route_approval && !pending_approval_route_ids.is_empty() {
            sqlx::query("UPDATE projects SET status='needs_human_review' WHERE project_id=? AND status='running'")
                .bind(project_id)
                .execute(&mut *tx)
                .await?;
        }
        let revision = bump_revision(&mut tx, project_id).await?;
        let mut events = Vec::new();
        events.push(append_event(&mut tx, project_id, revision, "planning.revision.committed", entity("plan_revision", &plan_revision_id), json!({"ordinal":ordinal,"planner_mode":planner_mode,"routes":returned_routes.len(),"tasks":tasks.len(),"delta_id":delta.delta_id}), Some(entity("round", &round.round_id))).await?);
        events.push(
            append_event(
                &mut tx,
                project_id,
                revision,
                "planning.delta.consumed",
                entity("research_delta", &delta.delta_id),
                json!({"plan_revision_id":plan_revision_id}),
                Some(entity("plan_revision", &plan_revision_id)),
            )
            .await?,
        );
        for decision in &route_decisions {
            if let (Some(route_id), Some(kind)) = (
                decision.get("route_id").and_then(Value::as_str),
                decision.get("decision").and_then(Value::as_str),
            ) {
                events.push(
                    append_event(
                        &mut tx,
                        project_id,
                        revision,
                        &format!("route.{kind}"),
                        entity("route", route_id),
                        decision.clone(),
                        Some(entity("plan_revision", &plan_revision_id)),
                    )
                    .await?,
                );
            }
        }
        for (proposal_id, route_id) in &accepted_human_proposals {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "route.proposal.accepted",
                    entity("route_proposal", proposal_id),
                    json!({"route_id":route_id}),
                    Some(entity("plan_revision", &plan_revision_id)),
                )
                .await?,
            );
        }
        for (proposal_id, reason) in &rejected_human_proposals {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "route.proposal.rejected",
                    entity("route_proposal", proposal_id),
                    json!({"reason":reason}),
                    Some(entity("plan_revision", &plan_revision_id)),
                )
                .await?,
            );
        }
        for task in &tasks {
            events.push(append_event(&mut tx, project_id, revision, "task.queued", entity("task", &task.task_id), json!({"route_id":task.route_id,"plan_revision_id":plan_revision_id,"task_signature":task.task_signature}), Some(entity("plan_revision", &plan_revision_id))).await?);
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "context.packet.created",
                    entity(
                        "context_packet",
                        task.context_packet_id.as_deref().unwrap_or_default(),
                    ),
                    json!({"task_id":task.task_id,"source_revision":actual}),
                    Some(entity("task", &task.task_id)),
                )
                .await?,
            );
        }
        events.extend(
            crate::suggestions::resolve_plan_suggestions(
                &mut tx,
                project_id,
                round.number,
                revision,
                Some(&plan_revision_id),
                &plan.suggestion_decisions,
                &now,
            )
            .await?,
        );
        tx.commit().await?;
        Ok(PlanSaveResult {
            round: ResearchRound {
                status: research_domain::RoundStatus::Running,
                ..round.clone()
            },
            routes: returned_routes,
            tasks,
            workers,
            events,
        })
    }

    pub async fn offer_local_task(
        &self,
        task: &Task,
        backend: &str,
        model: Option<&str>,
        working_directory: &str,
        capabilities: Value,
    ) -> StorageResult<LocalTaskOffer> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "offer_local_task",
            )
            .await?;
        let contract = self
            .get_task_contract(&task.project_id, &task.task_id)
            .await?;
        let context_packet = self.get_context_packet_for_task(&task.task_id).await?;
        if context_packet.status != "active" || context_packet.content_hash.is_empty() {
            return Err(StorageError::InvalidTransition(
                "task context packet is missing, invalid, or stale".into(),
            ));
        }
        let resume_checkpoint = sqlx::query("SELECT c.checkpoint_id,c.artifact_id,c.lease_epoch,c.summary,c.created_at,a.sha256,a.storage_path FROM task_checkpoints c JOIN artifacts a ON a.artifact_id=c.artifact_id WHERE c.project_id=? AND c.task_id=? ORDER BY c.created_at DESC,c.checkpoint_id DESC LIMIT 1")
            .bind(&task.project_id).bind(&task.task_id).fetch_optional(self.pool()).await?
            .map(|row| -> StorageResult<Value> { Ok(json!({
                "checkpoint_id":row.try_get::<String,_>("checkpoint_id")?,
                "artifact_id":row.try_get::<String,_>("artifact_id")?,
                "lease_epoch":row.try_get::<i64,_>("lease_epoch")?,
                "summary":row.try_get::<String,_>("summary")?,
                "created_at":row.try_get::<String,_>("created_at")?,
                "artifact_sha256":row.try_get::<String,_>("sha256")?,
                "artifact_storage_path":row.try_get::<String,_>("storage_path")?,
                "trust_rule":"A checkpoint is resumable execution state, not a mathematical premise."
            })) }).transpose()?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query("SELECT * FROM tasks WHERE project_id=? AND task_id=?")
            .bind(&task.project_id)
            .bind(&task.task_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "task",
                id: task.task_id.clone(),
            })?;
        let current = rows::task(&row)?;
        if current.revision != task.revision
            || current.route_id != task.route_id
            || current.plan_revision_id != task.plan_revision_id
            || current.route_cancellation_epoch != task.route_cancellation_epoch
            || current.context_packet_id != task.context_packet_id
        {
            return Err(StorageError::InvalidTransition(format!(
                "task {} offer used stale revision, plan, route epoch, or context",
                task.task_id
            )));
        }
        if !matches!(
            current.status,
            TaskStatus::Queued | TaskStatus::Assigned | TaskStatus::Open
        ) {
            return Err(StorageError::InvalidTransition(format!(
                "task {} cannot be offered from {}",
                current.task_id, current.status
            )));
        }
        let route = sqlx::query(
            "SELECT r.status,r.cancellation_epoch,r.human_review,p.status AS project_status \
             FROM routes r JOIN projects p ON p.project_id=r.project_id \
             WHERE r.project_id=? AND r.route_id=?",
        )
        .bind(&current.project_id)
        .bind(&current.route_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "route",
            id: current.route_id.clone(),
        })?;
        let route_status: String = route.try_get("status")?;
        let route_epoch: i64 = route.try_get("cancellation_epoch")?;
        let human_review: String = route.try_get("human_review")?;
        let project_status: String = route.try_get("project_status")?;
        if !crate::route_execution_is_released(&project_status, &human_review) {
            return Err(StorageError::InvalidTransition(format!(
                "task {} cannot be offered while project is {project_status} and route review is {human_review}",
                current.task_id
            )));
        }
        let budget_json: String =
            sqlx::query_scalar("SELECT budget_json FROM projects WHERE project_id=?")
                .bind(&current.project_id)
                .fetch_one(&mut *tx)
                .await?;
        let current_budget: research_domain::Budget = serde_json::from_str(&budget_json)?;
        crate::write::validate_project_budget(&current_budget)?;
        let active_execution_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM tasks WHERE project_id=? AND status IN ('offered','leased','running')",
        )
        .bind(&current.project_id)
        .fetch_one(&mut *tx)
        .await?;
        if active_execution_count >= i64::from(current_budget.max_parallel_workers) {
            return Err(StorageError::InvalidTransition(format!(
                "task {} cannot be offered because the project already has {active_execution_count} active executions (limit {})",
                current.task_id, current_budget.max_parallel_workers
            )));
        }
        if !matches!(
            route_status.as_str(),
            "incubating" | "active" | "probation" | "revived"
        ) || route_epoch != current.route_cancellation_epoch
        {
            return Err(StorageError::InvalidTransition(format!(
                "task {} route is not executable at epoch {}",
                current.task_id, current.route_cancellation_epoch
            )));
        }
        let plan_revision_id = current.plan_revision_id.as_deref().ok_or_else(|| {
            StorageError::InvalidTransition("task has no committed V2 plan revision".into())
        })?;
        let committed_plan: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM plan_revisions WHERE project_id=? AND plan_revision_id=? AND status='committed'",
        )
        .bind(&current.project_id)
        .bind(plan_revision_id)
        .fetch_one(&mut *tx)
        .await?;
        let context_packet_id = current.context_packet_id.as_deref().ok_or_else(|| {
            StorageError::InvalidTransition("task has no active context packet".into())
        })?;
        if committed_plan != 1
            || contract.plan_revision_id != plan_revision_id
            || contract.route_id != current.route_id
            || contract.route_cancellation_epoch != current.route_cancellation_epoch
            || contract.context_packet_id.as_deref() != Some(context_packet_id)
            || context_packet.context_packet_id != context_packet_id
            || context_packet.project_id != current.project_id
            || context_packet.task_id.as_deref() != Some(current.task_id.as_str())
            || context_packet.route_id.as_deref() != Some(current.route_id.as_str())
        {
            return Err(StorageError::InvalidTransition(format!(
                "task {} contract or context is stale",
                current.task_id
            )));
        }
        let active_lease_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_leases WHERE project_id=? AND task_id=? AND status='active'",
        )
        .bind(&current.project_id)
        .bind(&current.task_id)
        .fetch_one(&mut *tx)
        .await?;
        let active_attempt_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_attempts WHERE project_id=? AND task_id=? AND status NOT IN ('completed','failed','cancelled','orphaned')",
        )
        .bind(&current.project_id)
        .bind(&current.task_id)
        .fetch_one(&mut *tx)
        .await?;
        if active_lease_count != 0 || active_attempt_count != 0 {
            return Err(StorageError::InvalidTransition(format!(
                "task {} already has an active attempt or lease",
                current.task_id
            )));
        }
        let task = &current;
        let attempt_no: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(attempt_no),0)+1 FROM task_attempts WHERE task_id=?",
        )
        .bind(&task.task_id)
        .fetch_one(&mut *tx)
        .await?;
        if attempt_no > 3 {
            sqlx::query(
                "UPDATE tasks SET status='dead_lettered',revision=revision+1 WHERE task_id=?",
            )
            .bind(&task.task_id)
            .execute(&mut *tx)
            .await?;
            let revision = bump_revision(&mut tx, &task.project_id).await?;
            append_event(
                &mut tx,
                &task.project_id,
                revision,
                "task.dead_lettered",
                entity("task", &task.task_id),
                json!({"reason":"maximum_attempts_reached","max_attempts":3}),
                None,
            )
            .await?;
            tx.commit().await?;
            return Err(StorageError::InvalidTransition(
                "task reached the maximum of three attempts".into(),
            ));
        }
        let lease_epoch: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(lease_epoch),0)+1 FROM task_attempts WHERE task_id=?",
        )
        .bind(&task.task_id)
        .fetch_one(&mut *tx)
        .await?;
        let worker_instance_id = new_id("workerinstance");
        let attempt_id = new_id("attempt");
        let now = Utc::now();
        sqlx::query("INSERT INTO worker_instances(worker_instance_id,project_id,worker_id,backend,model,status,capability_json,working_directory,started_at) VALUES(?,?,?,?,?,'handshaking',?,?,?)")
            .bind(&worker_instance_id).bind(&task.project_id).bind(&task.worker_id).bind(backend).bind(model)
            .bind(json_text(&capabilities)?).bind(working_directory).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO task_attempts(attempt_id,project_id,task_id,worker_instance_id,attempt_no,status,lease_epoch,plan_revision_id,route_cancellation_epoch,context_packet_id,created_at) VALUES(?,?,?,?,?,'offered',?,?,?,?,?)")
            .bind(&attempt_id).bind(&task.project_id).bind(&task.task_id).bind(&worker_instance_id).bind(attempt_no)
            .bind(lease_epoch).bind(&task.plan_revision_id).bind(task.route_cancellation_epoch).bind(&task.context_packet_id)
            .bind(now.to_rfc3339()).execute(&mut *tx).await?;
        let claimed = sqlx::query("UPDATE tasks SET status='offered',revision=revision+1 WHERE task_id=? AND revision=? AND route_cancellation_epoch=? AND context_packet_id=? AND status IN ('queued','assigned','open')")
            .bind(&task.task_id).bind(task.revision).bind(task.route_cancellation_epoch).bind(context_packet_id).execute(&mut *tx).await?;
        if claimed.rows_affected() != 1 {
            return Err(StorageError::InvalidTransition(format!(
                "task {} changed while it was being offered",
                task.task_id
            )));
        }
        if let Some(worker_id) = &task.worker_id {
            sqlx::query(
                "UPDATE workers SET status='handshaking',last_heartbeat=? WHERE worker_id=?",
            )
            .bind(now.to_rfc3339())
            .bind(worker_id)
            .execute(&mut *tx)
            .await?;
        }
        let revision = bump_revision(&mut tx, &task.project_id).await?;
        let events = vec![
            append_event(
                &mut tx,
                &task.project_id,
                revision,
                "worker.spawn_requested",
                entity("worker_instance", &worker_instance_id),
                json!({"task_id":task.task_id,"attempt_id":attempt_id,"backend":backend}),
                Some(entity("task", &task.task_id)),
            )
            .await?,
            append_event(
                &mut tx,
                &task.project_id,
                revision,
                "task.offered",
                entity("task", &task.task_id),
                json!({"attempt_id":attempt_id,"attempt_no":attempt_no,"lease_epoch":lease_epoch}),
                Some(entity("worker_instance", &worker_instance_id)),
            )
            .await?,
        ];
        let refreshed = sqlx::query("SELECT * FROM tasks WHERE task_id=?")
            .bind(&task.task_id)
            .fetch_one(&mut *tx)
            .await?;
        let offered_task = rows::task(&refreshed)?;
        tx.commit().await?;
        Ok(LocalTaskOffer {
            task: offered_task,
            attempt: TaskAttempt {
                attempt_id,
                project_id: task.project_id.clone(),
                task_id: task.task_id.clone(),
                worker_instance_id: Some(worker_instance_id.clone()),
                attempt_no,
                status: "offered".into(),
                lease_epoch,
                plan_revision_id: task.plan_revision_id.clone(),
                route_cancellation_epoch: task.route_cancellation_epoch,
                context_packet_id: task.context_packet_id.clone(),
                failure_signature: None,
                failure_reason: None,
                started_at: None,
                completed_at: None,
                created_at: now,
            },
            worker_instance: WorkerInstance {
                worker_instance_id,
                project_id: task.project_id.clone(),
                worker_id: task.worker_id.clone(),
                backend: backend.into(),
                backend_version: None,
                model: model.map(ToOwned::to_owned),
                status: "handshaking".into(),
                capabilities,
                working_directory: Some(working_directory.into()),
                handshake: None,
                quarantine_reason: None,
                started_at: now,
                ready_at: None,
                last_heartbeat_at: None,
                exited_at: None,
            },
            contract,
            context_packet,
            resume_checkpoint,
            events,
        })
    }

    pub async fn accept_local_handshake(
        &self,
        offer: &LocalTaskOffer,
        backend_version: Option<&str>,
        handshake: Value,
        ttl_seconds: u64,
    ) -> StorageResult<LocalTaskLease> {
        let lease_token = format!("{}{}", new_id("token"), new_id("token"));
        let node_id = format!("local:{}", offer.task.project_id);
        self.accept_task_handshake(
            offer,
            &node_id,
            &lease_token,
            true,
            None,
            backend_version,
            handshake,
            ttl_seconds,
        )
        .await
    }

    pub(crate) async fn accept_distributed_handshake(
        &self,
        offer: &LocalTaskOffer,
        node: (&str, &str, i64),
        backend_version: Option<&str>,
        handshake: Value,
        ttl_seconds: u64,
    ) -> StorageResult<LocalTaskLease> {
        let (node_id, node_token, node_epoch) = node;
        self.accept_task_handshake(
            offer,
            node_id,
            node_token,
            false,
            Some(node_epoch),
            backend_version,
            handshake,
            ttl_seconds,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn accept_task_handshake(
        &self,
        offer: &LocalTaskOffer,
        node_id: &str,
        lease_token: &str,
        register_local_node: bool,
        expected_node_epoch: Option<i64>,
        backend_version: Option<&str>,
        handshake: Value,
        ttl_seconds: u64,
    ) -> StorageResult<LocalTaskLease> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "accept_task_handshake",
            )
            .await?;
        let token_hash = hash_secret(lease_token);
        let now = Utc::now();
        let ttl = i64::try_from(ttl_seconds.clamp(30, 3_600)).unwrap_or(3_600);
        let expires_at = now + chrono::Duration::seconds(ttl);
        let mut tx = self.pool().begin().await?;
        let current = sqlx::query("SELECT t.status,t.revision,t.route_cancellation_epoch,p.status AS project_status,r.status AS route_status,r.human_review FROM tasks t JOIN projects p ON p.project_id=t.project_id JOIN routes r ON r.route_id=t.route_id WHERE t.task_id=?")
        .bind(&offer.task.task_id)
        .fetch_one(&mut *tx)
        .await?;
        let status: String = current.try_get("status")?;
        let route_epoch: i64 = current.try_get("route_cancellation_epoch")?;
        let project_status: String = current.try_get("project_status")?;
        let route_status: String = current.try_get("route_status")?;
        let human_review: String = current.try_get("human_review")?;
        if status != "offered"
            || route_epoch != offer.task.route_cancellation_epoch
            || !crate::route_execution_is_released(&project_status, &human_review)
            || route_status != "active"
        {
            return Err(StorageError::LateSubmission(format!(
                "handshake is stale: project={project_status}, route={route_status}/{human_review}, task={status}, route epoch={route_epoch}"
            )));
        }
        let attempt_status: String = sqlx::query_scalar(
            "SELECT status FROM task_attempts WHERE attempt_id=? AND worker_instance_id=?",
        )
        .bind(&offer.attempt.attempt_id)
        .bind(&offer.worker_instance.worker_instance_id)
        .fetch_one(&mut *tx)
        .await?;
        if attempt_status != "offered" {
            return Err(StorageError::LateSubmission(
                "attempt is no longer offered".into(),
            ));
        }
        if register_local_node {
            sqlx::query("INSERT OR IGNORE INTO worker_nodes(node_id,display_name,capabilities_json,token_hash,status,node_epoch,registered_at,last_heartbeat) VALUES(?,?,'{\"local\":true}',?,'active',1,?,?)")
                .bind(node_id).bind("local orchestrator").bind(&token_hash).bind(now.to_rfc3339()).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        } else {
            let authenticated_node: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM worker_nodes WHERE node_id=? AND token_hash=? AND node_epoch=? AND status='active'",
            )
            .bind(node_id)
            .bind(&token_hash)
            .bind(expected_node_epoch.ok_or_else(|| {
                StorageError::CorruptData("distributed handshake omitted node epoch".into())
            })?)
            .fetch_one(&mut *tx)
            .await?;
            if authenticated_node != 1 {
                return Err(StorageError::LateSubmission(
                    "distributed worker node changed during handshake".into(),
                ));
            }
        }
        sqlx::query("UPDATE worker_nodes SET last_heartbeat=?,status='active' WHERE node_id=?")
            .bind(now.to_rfc3339())
            .bind(node_id)
            .execute(&mut *tx)
            .await?;
        let final_task_revision = current.try_get::<i64, _>("revision")? + 1;
        let lease_id = new_id("lease");
        sqlx::query("INSERT INTO task_leases(lease_id,project_id,task_id,node_id,task_revision,route_epoch,lease_epoch,status,leased_at,expires_at,attempt_id,worker_instance_id,lease_token_hash,offered_at,last_heartbeat_at) VALUES(?,?,?,?,?,?,?,'active',?,?,?,?,?,?,?)")
            .bind(&lease_id).bind(&offer.task.project_id).bind(&offer.task.task_id).bind(node_id)
            .bind(final_task_revision).bind(route_epoch).bind(offer.attempt.lease_epoch)
            .bind(now.to_rfc3339()).bind(expires_at.to_rfc3339()).bind(&offer.attempt.attempt_id)
            .bind(&offer.worker_instance.worker_instance_id).bind(&token_hash).bind(offer.attempt.created_at.to_rfc3339())
            .bind(now.to_rfc3339()).execute(&mut *tx).await?;
        sqlx::query("UPDATE task_attempts SET status='running',started_at=? WHERE attempt_id=? AND status='offered'")
            .bind(now.to_rfc3339()).bind(&offer.attempt.attempt_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE tasks SET status='running',revision=revision+1 WHERE task_id=? AND status='offered'")
            .bind(&offer.task.task_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE worker_instances SET status='running',backend_version=?,handshake_json=?,ready_at=?,last_heartbeat_at=? WHERE worker_instance_id=? AND status='handshaking'")
            .bind(backend_version).bind(json_text(&handshake)?).bind(now.to_rfc3339()).bind(now.to_rfc3339())
            .bind(&offer.worker_instance.worker_instance_id).execute(&mut *tx).await?;
        if let Some(worker_id) = &offer.task.worker_id {
            sqlx::query("UPDATE workers SET status='running',last_heartbeat=? WHERE worker_id=?")
                .bind(now.to_rfc3339())
                .bind(worker_id)
                .execute(&mut *tx)
                .await?;
        }
        let revision = bump_revision(&mut tx, &offer.task.project_id).await?;
        let events = vec![
            append_event(&mut tx, &offer.task.project_id, revision, "worker.handshake_succeeded", entity("worker_instance", &offer.worker_instance.worker_instance_id), json!({"attempt_id":offer.attempt.attempt_id,"context_hash":offer.context_packet.content_hash,"backend_version":backend_version}), Some(entity("task", &offer.task.task_id))).await?,
            append_event(&mut tx, &offer.task.project_id, revision, "task.lease_acquired", entity("task_lease", &lease_id), json!({"task_id":offer.task.task_id,"attempt_id":offer.attempt.attempt_id,"lease_epoch":offer.attempt.lease_epoch,"expires_at":expires_at}), Some(entity("worker_instance", &offer.worker_instance.worker_instance_id))).await?,
            append_event(&mut tx, &offer.task.project_id, revision, "task.started", entity("task", &offer.task.task_id), json!({"attempt_id":offer.attempt.attempt_id,"lease_id":lease_id}), Some(entity("task_lease", &lease_id))).await?,
        ];
        let row = sqlx::query("SELECT * FROM tasks WHERE task_id=?")
            .bind(&offer.task.task_id)
            .fetch_one(&mut *tx)
            .await?;
        let task = rows::task(&row)?;
        tx.commit().await?;
        Ok(LocalTaskLease {
            task,
            attempt_id: offer.attempt.attempt_id.clone(),
            worker_instance_id: offer.worker_instance.worker_instance_id.clone(),
            lease_id,
            lease_token: lease_token.into(),
            lease_epoch: offer.attempt.lease_epoch,
            expires_at,
            events,
        })
    }

    pub async fn heartbeat_local_lease(
        &self,
        lease: &LocalTaskLease,
        ttl_seconds: u64,
    ) -> StorageResult<()> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::Telemetry,
                "heartbeat_local_lease",
            )
            .await?;
        let now = Utc::now();
        let ttl = i64::try_from(ttl_seconds.clamp(30, 3_600)).unwrap_or(3_600);
        let expires_at = now + chrono::Duration::seconds(ttl);
        let updated = sqlx::query("UPDATE task_leases SET expires_at=?,last_heartbeat_at=? WHERE lease_id=? AND attempt_id=? AND lease_epoch=? AND lease_token_hash=? AND status='active' AND expires_at>=?")
            .bind(expires_at.to_rfc3339()).bind(now.to_rfc3339()).bind(&lease.lease_id).bind(&lease.attempt_id)
            .bind(lease.lease_epoch).bind(hash_secret(&lease.lease_token)).bind(now.to_rfc3339()).execute(self.pool()).await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::LateSubmission(
                "local lease heartbeat is stale or expired".into(),
            ));
        }
        sqlx::query("UPDATE worker_instances SET last_heartbeat_at=? WHERE worker_instance_id=? AND status='running'")
            .bind(now.to_rfc3339()).bind(&lease.worker_instance_id).execute(self.pool()).await?;
        Ok(())
    }

    pub async fn save_local_checkpoint(
        &self,
        lease: &LocalTaskLease,
        artifact_id: &str,
        summary: &str,
    ) -> StorageResult<research_domain::DomainEvent> {
        if summary.trim().is_empty() {
            return Err(StorageError::InvalidTransition(
                "checkpoint summary must not be empty".into(),
            ));
        }
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "save_task_checkpoint",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        validate_local_lease(&mut tx, lease, &["running", "checkpointed"]).await?;
        let artifact_project: Option<String> =
            sqlx::query_scalar("SELECT project_id FROM artifacts WHERE artifact_id=?")
                .bind(artifact_id)
                .fetch_optional(&mut *tx)
                .await?;
        if artifact_project.as_deref() != Some(lease.task.project_id.as_str()) {
            return Err(StorageError::InvalidTransition(
                "checkpoint artifact is missing or belongs to another project".into(),
            ));
        }
        let checkpoint_id = new_id("checkpoint");
        let now = Utc::now();
        sqlx::query("INSERT INTO task_checkpoints(checkpoint_id,project_id,task_id,attempt_id,artifact_id,lease_epoch,summary,created_at) VALUES(?,?,?,?,?,?,?,?)")
            .bind(&checkpoint_id).bind(&lease.task.project_id).bind(&lease.task.task_id)
            .bind(&lease.attempt_id).bind(artifact_id).bind(lease.lease_epoch).bind(summary.trim())
            .bind(now.to_rfc3339()).execute(&mut *tx).await?;
        sqlx::query("UPDATE tasks SET status='checkpointed' WHERE task_id=? AND status IN ('running','checkpointed')")
            .bind(&lease.task.task_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE task_attempts SET status='checkpointed' WHERE attempt_id=? AND status IN ('running','checkpointed')")
            .bind(&lease.attempt_id).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &lease.task.project_id).await?;
        let event = append_event(
            &mut tx,
            &lease.task.project_id,
            revision,
            "task.checkpoint_saved",
            entity("task_checkpoint", &checkpoint_id),
            json!({"task_id":lease.task.task_id,"attempt_id":lease.attempt_id,"artifact_id":artifact_id,"lease_epoch":lease.lease_epoch,"summary":summary.trim()}),
            Some(entity("task_lease", &lease.lease_id)),
        )
        .await?;
        tx.commit().await?;
        Ok(event)
    }

    pub async fn submit_local_result_envelope(
        &self,
        lease: &LocalTaskLease,
        output: &WorkerOutput,
    ) -> StorageResult<(String, Vec<research_domain::DomainEvent>)> {
        match self
            .submit_local_result_envelope_after_steers(lease, output, &[])
            .await?
        {
            LocalResultSubmission::Submitted {
                result_envelope_id,
                events,
            } => Ok((result_envelope_id, events)),
            LocalResultSubmission::SteeringPending { steers } => {
                Err(StorageError::InvalidTransition(format!(
                    "{} task steer(s) must be incorporated before result submission",
                    steers.len()
                )))
            }
        }
    }

    pub async fn submit_local_result_envelope_after_steers(
        &self,
        lease: &LocalTaskLease,
        output: &WorkerOutput,
        incorporated_steer_ids: &[String],
    ) -> StorageResult<LocalResultSubmission> {
        if output.summary.trim().is_empty() {
            return Err(StorageError::InvalidTransition(
                "result envelope requires a non-empty summary".into(),
            ));
        }
        let (payload, content_hash, idempotency_key) =
            worker_result_envelope_identity(&lease.attempt_id, output)?;
        if let Some(existing) = sqlx::query_scalar::<_, String>(
            "SELECT result_envelope_id FROM result_envelopes WHERE project_id=? AND idempotency_key=?",
        )
        .bind(&lease.task.project_id)
        .bind(&idempotency_key)
        .fetch_optional(self.pool())
        .await?
        {
            return Ok(LocalResultSubmission::Submitted {
                result_envelope_id: existing,
                events: Vec::new(),
            });
        }
        let bytes = serde_json::to_vec_pretty(output)?;
        let artifact = self
            .prepare_artifact_file(
                &lease.task.project_id,
                "worker_result_envelope_payload",
                &format!("{}-result.json", lease.attempt_id),
                &bytes,
                lease.task.round,
                vec![lease.task.task_id.clone(), lease.attempt_id.clone()],
            )
            .await?;
        let mut artifact_file = UncommittedArtifactFile::new(&artifact);
        let admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "submit_local_result_envelope",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        if let Some(existing) = sqlx::query_scalar::<_, String>(
            "SELECT result_envelope_id FROM result_envelopes WHERE project_id=? AND idempotency_key=?",
        )
        .bind(&lease.task.project_id)
        .bind(&idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            tx.rollback().await?;
            drop(admission);
            return Ok(LocalResultSubmission::Submitted {
                result_envelope_id: existing,
                events: Vec::new(),
            });
        }
        validate_local_lease(&mut tx, lease, &["running", "checkpointed"]).await?;
        let steer_rows = sqlx::query("SELECT * FROM task_steers WHERE project_id=? AND task_id=? AND expected_task_revision=? AND expected_route_epoch=? AND status='pending' ORDER BY created_at,steer_id")
            .bind(&lease.task.project_id)
            .bind(&lease.task.task_id)
            .bind(lease.task.revision)
            .bind(lease.task.route_cancellation_epoch)
            .fetch_all(&mut *tx)
            .await?;
        let pending_steers = steer_rows
            .iter()
            .map(crate::control_plane::task_steer_from_row)
            .collect::<StorageResult<Vec<_>>>()?;
        let incorporated = incorporated_steer_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if incorporated.len() != incorporated_steer_ids.len() {
            tx.rollback().await?;
            drop(admission);
            return Err(StorageError::InvalidTransition(
                "incorporated steer IDs contain duplicates".into(),
            ));
        }
        let pending_ids = pending_steers
            .iter()
            .map(|steer| steer.steer_id.as_str())
            .collect::<BTreeSet<_>>();
        if !incorporated.is_subset(&pending_ids) {
            tx.rollback().await?;
            drop(admission);
            return Err(StorageError::LateSubmission(
                "an incorporated steer is no longer pending for this task revision and route epoch"
                    .into(),
            ));
        }
        let unincorporated = pending_steers
            .iter()
            .filter(|steer| !incorporated.contains(steer.steer_id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !unincorporated.is_empty() {
            tx.rollback().await?;
            drop(admission);
            return Ok(LocalResultSubmission::SteeringPending {
                steers: unincorporated,
            });
        }
        insert_artifact_record_tx(&mut tx, &artifact).await?;
        let result_envelope_id = new_id("result");
        let now = Utc::now();
        sqlx::query("INSERT INTO result_envelopes(result_envelope_id,project_id,task_id,attempt_id,lease_id,lease_epoch,plan_revision_id,route_cancellation_epoch,outcome,envelope_json,result_artifact_id,content_hash,idempotency_key,status,submitted_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,'submitted',?)")
            .bind(&result_envelope_id).bind(&lease.task.project_id).bind(&lease.task.task_id).bind(&lease.attempt_id)
            .bind(&lease.lease_id).bind(lease.lease_epoch).bind(&lease.task.plan_revision_id).bind(lease.task.route_cancellation_epoch)
            .bind(if output.candidates.is_empty() {"partial"} else {"success"}).bind(json_text(&payload)?)
            .bind(&artifact.artifact_id).bind(&content_hash).bind(&idempotency_key).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        sqlx::query("UPDATE tasks SET status='result_submitted',revision=revision+1 WHERE task_id=? AND status IN ('running','checkpointed')")
            .bind(&lease.task.task_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE task_attempts SET status='result_submitted' WHERE attempt_id=? AND status IN ('running','checkpointed')")
            .bind(&lease.attempt_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE worker_instances SET status='draining' WHERE worker_instance_id=? AND status='running'")
            .bind(&lease.worker_instance_id).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &lease.task.project_id).await?;
        let mut events = vec![
            append_event(
                &mut tx,
                &lease.task.project_id,
                revision,
                "artifact.published",
                entity("artifact", &artifact.artifact_id),
                json!({"kind":artifact.kind,"filename":artifact.filename}),
                Some(entity("task_attempt", &lease.attempt_id)),
            )
            .await?,
        ];
        for steer in &pending_steers {
            sqlx::query("UPDATE task_steers SET status='applied',applied_at=? WHERE steer_id=? AND status='pending'")
                .bind(now.to_rfc3339())
                .bind(&steer.steer_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE human_commands SET status='applied',after_revision=?,affected_entities_json=?,applied_at=? WHERE command_id=? AND status='waiting_safe_point'")
                .bind(revision)
                .bind(json_text(&vec![entity("task", &lease.task.task_id)])?)
                .bind(now.to_rfc3339())
                .bind(&steer.command_id)
                .execute(&mut *tx)
                .await?;
            events.push(append_event(&mut tx, &lease.task.project_id, revision, "task.steer.applied", entity("task", &lease.task.task_id), json!({"steer_id":steer.steer_id,"content":steer.content,"task_revision":lease.task.revision,"route_epoch":lease.task.route_cancellation_epoch,"result_envelope_id":result_envelope_id}), Some(entity("command", &steer.command_id))).await?);
        }
        events.push(append_event(&mut tx, &lease.task.project_id, revision, "task.result_submitted", entity("result_envelope", &result_envelope_id), json!({"task_id":lease.task.task_id,"attempt_id":lease.attempt_id,"content_hash":content_hash,"artifact_id":artifact.artifact_id,"incorporated_steer_ids":incorporated_steer_ids}), Some(entity("task_lease", &lease.lease_id))).await?);
        tx.commit().await?;
        artifact_file.commit();
        Ok(LocalResultSubmission::Submitted {
            result_envelope_id,
            events,
        })
    }

    #[allow(clippy::too_many_lines)]
    pub async fn ingest_local_result_envelope(
        &self,
        result_envelope_id: &str,
    ) -> StorageResult<LocalResultIngestion> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "ingest_local_result_envelope",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let envelope = sqlx::query("SELECT * FROM result_envelopes WHERE result_envelope_id=?")
            .bind(result_envelope_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "result_envelope",
                id: result_envelope_id.into(),
            })?;
        let status: String = envelope.try_get("status")?;
        let output: WorkerOutput = serde_json::from_str(envelope.try_get("envelope_json")?)?;
        let project_id: String = envelope.try_get("project_id")?;
        let task_id: String = envelope.try_get("task_id")?;
        if status == "ingested" {
            let verifications =
                result_envelope_verifications_tx(&mut tx, &project_id, result_envelope_id).await?;
            tx.rollback().await?;
            return Ok(LocalResultIngestion {
                output,
                verifications,
                events: Vec::new(),
            });
        }
        if status != "submitted" {
            return Err(StorageError::InvalidTransition(format!(
                "result envelope is {status}"
            )));
        }
        let attempt_id: String = envelope.try_get("attempt_id")?;
        let lease_id: String = envelope.try_get("lease_id")?;
        let lease_epoch: i64 = envelope.try_get("lease_epoch")?;
        let route_epoch: i64 = envelope.try_get("route_cancellation_epoch")?;
        let task_row = sqlx::query("SELECT t.*,r.status AS joined_route_status,r.cancellation_epoch AS joined_route_epoch,r.human_review AS joined_route_human_review,p.status AS joined_project_status FROM tasks t JOIN routes r ON t.route_id=r.route_id JOIN projects p ON p.project_id=t.project_id WHERE t.task_id=?")
            .bind(&task_id).fetch_one(&mut *tx).await?;
        let task = rows::task(&task_row)?;
        let route_status: String = task_row.try_get("joined_route_status")?;
        let current_route_epoch: i64 = task_row.try_get("joined_route_epoch")?;
        let project_status: String = task_row.try_get("joined_project_status")?;
        let human_review: String = task_row.try_get("joined_route_human_review")?;
        let lease_status: String = sqlx::query_scalar(
            "SELECT status FROM task_leases WHERE lease_id=? AND attempt_id=? AND lease_epoch=?",
        )
        .bind(&lease_id)
        .bind(&attempt_id)
        .bind(lease_epoch)
        .fetch_one(&mut *tx)
        .await?;
        if task.status != TaskStatus::ResultSubmitted
            || !(project_status == "paused"
                || crate::route_execution_is_released(&project_status, &human_review))
            || route_status == "human_stopped"
            || current_route_epoch != route_epoch
            || !matches!(lease_status.as_str(), "active" | "expired")
        {
            sqlx::query("UPDATE result_envelopes SET status='stale',rejection_reason='lease, task, or route epoch changed' WHERE result_envelope_id=?")
                .bind(result_envelope_id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Err(StorageError::LateSubmission(
                "result became stale before ingestion".into(),
            ));
        }
        sqlx::query(
            "UPDATE tasks SET status='ingesting' WHERE task_id=? AND status='result_submitted'",
        )
        .bind(&task_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE task_attempts SET status='ingesting' WHERE attempt_id=? AND status='result_submitted'")
            .bind(&attempt_id).execute(&mut *tx).await?;
        let ingestion = crate::worker_output_ingestion::ingest_worker_output_tx(
            &mut tx,
            &task,
            &output,
            crate::worker_output_ingestion::WorkerOutputOrigin::Leased,
        )
        .await?;
        let crate::worker_output_ingestion::WorkerOutputIngestion {
            recorded_sources,
            rejected_source_drafts,
            capsule_events,
            source_reference_aliases,
            inserted_source_count,
            upgraded_source_count,
        } = ingestion;
        sqlx::query("UPDATE tasks SET status='completed',result_summary=?,revision=revision+1 WHERE task_id=? AND status='ingesting'")
            .bind(&output.summary).bind(&task_id).execute(&mut *tx).await?;
        let candidate_task_revision = task.revision.saturating_add(1);
        let mut candidate_inserts = Vec::with_capacity(output.candidates.len());
        for (candidate_index, draft) in output.candidates.iter().enumerate() {
            let submission = CandidateSubmission {
                task_id: task_id.clone(),
                route_id: task.route_id.clone(),
                target_goal_ids: if draft.target_goal_ids.is_empty() {
                    task.goal_ids.clone()
                } else {
                    draft.target_goal_ids.clone()
                },
                statement: draft.statement.clone(),
                assumptions: draft.assumptions.clone(),
                proof_markdown: draft.proof_markdown.clone(),
                dependency_fact_ids: draft.dependency_fact_ids.clone(),
                definitions_introduced: draft.definitions_introduced.clone(),
                external_source_ids: draft.external_source_ids.clone(),
                candidate_type: draft.candidate_type,
                task_revision: candidate_task_revision,
                route_cancellation_epoch: task.route_cancellation_epoch,
            };
            candidate_inserts.push(
                crate::candidate_ingestion::insert_worker_candidate_tx(
                    &mut tx,
                    &project_id,
                    submission,
                    &format!("worker-result-{result_envelope_id}-{candidate_index}"),
                    &source_reference_aliases,
                )
                .await?,
            );
        }
        let has_verifiable_candidate_submission = candidate_inserts
            .iter()
            .any(|insert| insert.candidate.status == research_domain::CandidateStatus::Submitted);
        let material_progress = has_verifiable_candidate_submission
            || !output.discoveries.is_empty()
            || !output.experiments.is_empty()
            || !recorded_sources.is_empty();
        sqlx::query("UPDATE task_attempts SET status='completed',completed_at=? WHERE attempt_id=? AND status='ingesting'")
            .bind(Utc::now().to_rfc3339()).bind(&attempt_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE task_leases SET status='completed',completed_at=? WHERE lease_id=? AND status='active'")
            .bind(Utc::now().to_rfc3339()).bind(&lease_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE result_envelopes SET status='ingested',ingested_at=? WHERE result_envelope_id=? AND status='submitted'")
            .bind(Utc::now().to_rfc3339()).bind(result_envelope_id).execute(&mut *tx).await?;
        if let Some(worker_id) = &task.worker_id {
            sqlx::query("UPDATE workers SET status='idle',current_task_id=NULL,current_route_id=NULL,last_heartbeat=? WHERE worker_id=?")
                .bind(Utc::now().to_rfc3339()).bind(worker_id).execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE worker_instances SET status='exited',exited_at=? WHERE worker_instance_id=(SELECT worker_instance_id FROM task_attempts WHERE attempt_id=?)")
            .bind(Utc::now().to_rfc3339()).bind(&attempt_id).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, &project_id).await?;
        if material_progress {
            let kind = if has_verifiable_candidate_submission {
                "candidate_submitted"
            } else if !output.discoveries.is_empty() {
                "discovery"
            } else if !output.experiments.is_empty() {
                "experiment"
            } else {
                "source"
            };
            sqlx::query("INSERT INTO route_progress_entries(progress_entry_id,project_id,route_id,source_entity_json,progress_kind,material,summary,evidence_ids_json,project_revision,created_at) VALUES(?,?,?,?,?,1,?,'[]',?,?)")
                .bind(new_id("routeprogress")).bind(&project_id).bind(&task.route_id)
                .bind(json_text(&entity("result_envelope", result_envelope_id))?).bind(kind).bind(&output.summary)
                .bind(revision).bind(Utc::now().to_rfc3339()).execute(&mut *tx).await?;
            sqlx::query("UPDATE routes SET last_material_progress_revision=?,consecutive_no_progress_plans=0,status=CASE WHEN status='probation' THEN 'active' ELSE status END WHERE route_id=?")
                .bind(revision).bind(&task.route_id).execute(&mut *tx).await?;
        }
        let verifications = candidate_inserts
            .iter()
            .map(|insert| insert.verification.clone())
            .collect::<Vec<_>>();
        let mut events = Vec::with_capacity(
            candidate_inserts.len()
                + rejected_source_drafts.len()
                + recorded_sources.len()
                + capsule_events.len()
                + 2,
        );
        for inserted in &candidate_inserts {
            if inserted.inserted {
                let rejected = inserted.source_reference_failure_id.is_some();
                let event_type = if rejected {
                    "claim.rejected_untrusted_source_reference"
                } else {
                    "claim.submitted"
                };
                events.push(
                    append_event(
                        &mut tx,
                        &project_id,
                        revision,
                        event_type,
                        entity("candidate", &inserted.candidate.candidate_id),
                        json!({
                            "verification_id":inserted.verification.verification_id,
                            "result_envelope_id":result_envelope_id,
                            "source_reference_audit":inserted.source_reference_audit,
                            "failure_id":inserted.source_reference_failure_id,
                        }),
                        Some(entity("task", &task_id)),
                    )
                    .await?,
                );
                if !rejected && inserted.source_reference_audit.is_some() {
                    events.push(
                        append_event(
                            &mut tx,
                            &project_id,
                            revision,
                            "claim.source_references.normalized",
                            entity("candidate", &inserted.candidate.candidate_id),
                            json!({
                                "verification_id":inserted.verification.verification_id,
                                "result_envelope_id":result_envelope_id,
                                "source_reference_audit":inserted.source_reference_audit,
                            }),
                            Some(entity("task", &task_id)),
                        )
                        .await?,
                    );
                }
            }
        }
        for (failure_id, reasons) in &rejected_source_drafts {
            events.push(
                append_event(
                    &mut tx,
                    &project_id,
                    revision,
                    "source.draft.rejected",
                    entity("failure", failure_id),
                    json!({"task_id":task_id,"route_id":task.route_id,"reasons":reasons}),
                    Some(entity("task", &task_id)),
                )
                .await?,
            );
        }
        for (source_id, title, admission) in &recorded_sources {
            events.push(
                append_event(
                    &mut tx,
                    &project_id,
                    revision,
                    admission.event_type(),
                    entity("source", source_id),
                    json!({"task_id":task_id,"route_id":task.route_id,"title":title,"trust":admission.stored_status()}),
                    Some(entity("task", &task_id)),
                )
                .await?,
            );
        }
        for (capsule_id, content_hash) in capsule_events {
            events.push(append_event(&mut tx,&project_id,revision,"experiment.capsule.recorded",entity("experiment_capsule",&capsule_id),json!({"task_id":task_id,"route_id":task.route_id,"content_hash":content_hash,"trust":"reported_unverified"}),Some(entity("task",&task_id))).await?);
        }
        if material_progress {
            events.push(
                append_event(
                    &mut tx,
                    &project_id,
                    revision,
                    "route.material_progress",
                    entity("route", &task.route_id),
                    json!({"task_id":task_id,"attempt_id":attempt_id,"summary":output.summary}),
                    Some(entity("result_envelope", result_envelope_id)),
                )
                .await?,
            );
        }
        let rejected_candidate_source_reference_count = candidate_inserts
            .iter()
            .filter(|insert| insert.source_reference_failure_id.is_some())
            .count();
        events.push(append_event(&mut tx,&project_id,revision,"task.result_ingested",entity("result_envelope",result_envelope_id),json!({"task_id":task_id,"attempt_id":attempt_id,"candidate_count":output.candidates.len(),"source_count":output.sources.len(),"inserted_source_count":inserted_source_count,"upgraded_source_count":upgraded_source_count,"rejected_source_count":rejected_source_drafts.len(),"rejected_candidate_source_reference_count":rejected_candidate_source_reference_count,"experiment_count":output.experiments.len()}),Some(entity("task_lease",&lease_id))).await?);
        tx.commit().await?;
        Ok(LocalResultIngestion {
            output,
            verifications,
            events,
        })
    }

    pub async fn fail_local_attempt(
        &self,
        lease: Option<&LocalTaskLease>,
        offer: &LocalTaskOffer,
        reason: &str,
    ) -> StorageResult<Vec<research_domain::DomainEvent>> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "fail_local_attempt",
            )
            .await?;
        let signature = hex::encode(Sha256::digest(normalize_semantic_text(reason).as_bytes()));
        let now = Utc::now();
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query(
            "SELECT p.status AS project_status,t.status AS task_status,t.revision AS task_revision,\
                    t.route_id,t.route_cancellation_epoch AS task_route_epoch,\
                    r.status AS route_status,r.cancellation_epoch AS route_epoch,r.human_review,\
                    a.status AS attempt_status,a.failure_signature,a.attempt_no,a.lease_epoch,\
                    a.route_cancellation_epoch AS attempt_route_epoch,a.worker_instance_id \
             FROM task_attempts a \
             JOIN tasks t ON t.task_id=a.task_id AND t.project_id=a.project_id \
             JOIN projects p ON p.project_id=t.project_id \
             JOIN routes r ON r.route_id=t.route_id AND r.project_id=t.project_id \
             WHERE a.attempt_id=? AND a.project_id=? AND a.task_id=?",
        )
        .bind(&offer.attempt.attempt_id)
        .bind(&offer.task.project_id)
        .bind(&offer.task.task_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            StorageError::LateSubmission("task attempt no longer exists in this task scope".into())
        })?;
        let attempt_status: String = row.try_get("attempt_status")?;
        let recorded_signature: Option<String> = row.try_get("failure_signature")?;
        if attempt_status == "failed" {
            tx.rollback().await?;
            if recorded_signature.as_deref() == Some(signature.as_str()) {
                return Ok(Vec::new());
            }
            return Err(StorageError::LateSubmission(
                "task attempt already failed with a different reason".into(),
            ));
        }

        let project_status: String = row.try_get("project_status")?;
        let task_status: String = row.try_get("task_status")?;
        let task_revision: i64 = row.try_get("task_revision")?;
        let route_id: String = row.try_get("route_id")?;
        let task_route_epoch: i64 = row.try_get("task_route_epoch")?;
        let route_status: String = row.try_get("route_status")?;
        let human_review: String = row.try_get("human_review")?;
        let route_epoch: i64 = row.try_get("route_epoch")?;
        let attempt_no: i64 = row.try_get("attempt_no")?;
        let attempt_lease_epoch: i64 = row.try_get("lease_epoch")?;
        let attempt_route_epoch: i64 = row.try_get("attempt_route_epoch")?;
        let worker_instance_id: Option<String> = row.try_get("worker_instance_id")?;
        let expected_revision = lease.map_or(offer.task.revision, |lease| lease.task.revision);
        let expected_task_state = if lease.is_some() {
            matches!(task_status.as_str(), "running" | "checkpointed")
                && matches!(attempt_status.as_str(), "running" | "checkpointed")
        } else {
            task_status == "offered" && attempt_status == "offered"
        };
        let latest_attempt_no: i64 =
            sqlx::query_scalar("SELECT MAX(attempt_no) FROM task_attempts WHERE task_id=?")
                .bind(&offer.task.task_id)
                .fetch_one(&mut *tx)
                .await?;
        let submitted_result_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM result_envelopes WHERE attempt_id=? AND status IN ('submitted','ingested')",
        )
        .bind(&offer.attempt.attempt_id)
        .fetch_one(&mut *tx)
        .await?;
        let identity_matches = offer.attempt.project_id == offer.task.project_id
            && offer.attempt.task_id == offer.task.task_id
            && offer.attempt.attempt_no == attempt_no
            && offer.attempt.lease_epoch == attempt_lease_epoch
            && offer.attempt.route_cancellation_epoch == attempt_route_epoch
            && offer.worker_instance.project_id == offer.task.project_id
            && worker_instance_id.as_deref()
                == Some(offer.worker_instance.worker_instance_id.as_str())
            && route_id == offer.task.route_id;
        if !crate::route_execution_is_released(&project_status, &human_review)
            || route_status != "active"
            || !expected_task_state
            || !identity_matches
            || task_revision != expected_revision
            || task_route_epoch != offer.task.route_cancellation_epoch
            || route_epoch != offer.task.route_cancellation_epoch
            || latest_attempt_no != attempt_no
            || submitted_result_count != 0
        {
            return Err(StorageError::LateSubmission(format!(
                "attempt failure is stale: project={project_status}, route={route_status}/{route_epoch}, task={task_status}/{task_revision}, attempt={attempt_status}/{attempt_no}"
            )));
        }

        if let Some(lease) = lease {
            if lease.task.project_id != offer.task.project_id
                || lease.task.task_id != offer.task.task_id
                || lease.attempt_id != offer.attempt.attempt_id
                || lease.worker_instance_id != offer.worker_instance.worker_instance_id
                || lease.lease_epoch != offer.attempt.lease_epoch
                || lease.task.route_cancellation_epoch != offer.task.route_cancellation_epoch
            {
                return Err(StorageError::LateSubmission(
                    "failure lease does not match its task offer".into(),
                ));
            }
            let lease_row = sqlx::query(
                "SELECT status,lease_token_hash,task_revision,route_epoch,lease_epoch \
                 FROM task_leases WHERE lease_id=? AND task_id=? AND attempt_id=? \
                 AND worker_instance_id=?",
            )
            .bind(&lease.lease_id)
            .bind(&offer.task.task_id)
            .bind(&offer.attempt.attempt_id)
            .bind(&offer.worker_instance.worker_instance_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StorageError::LateSubmission("failure lease no longer exists".into()))?;
            let lease_status: String = lease_row.try_get("status")?;
            let lease_token_hash: Option<String> = lease_row.try_get("lease_token_hash")?;
            let lease_task_revision: i64 = lease_row.try_get("task_revision")?;
            let lease_route_epoch: i64 = lease_row.try_get("route_epoch")?;
            let stored_lease_epoch: i64 = lease_row.try_get("lease_epoch")?;
            if lease_status != "active"
                || lease_token_hash.as_deref() != Some(hash_secret(&lease.lease_token).as_str())
                || lease_task_revision != expected_revision
                || lease_route_epoch != route_epoch
                || stored_lease_epoch != attempt_lease_epoch
            {
                return Err(StorageError::LateSubmission(
                    "failure lease token, revision, epoch, or lifecycle is stale".into(),
                ));
            }
        } else {
            let lease_count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM task_leases WHERE attempt_id=?")
                    .bind(&offer.attempt.attempt_id)
                    .fetch_one(&mut *tx)
                    .await?;
            if lease_count != 0 {
                return Err(StorageError::LateSubmission(
                    "unleased failure cannot replace an existing lease".into(),
                ));
            }
        }

        let same_failures: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_attempts WHERE task_id=? AND attempt_id<>? AND failure_signature=?",
        )
        .bind(&offer.task.task_id)
        .bind(&offer.attempt.attempt_id)
        .bind(&signature)
        .fetch_one(&mut *tx)
        .await?;
        let attempts: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM task_attempts WHERE task_id=?")
                .bind(&offer.task.task_id)
                .fetch_one(&mut *tx)
                .await?;
        let terminal = attempts >= 3 || same_failures >= 1;
        let next_status = if terminal { "dead_lettered" } else { "queued" };
        let attempt_update = if lease.is_some() {
            sqlx::query("UPDATE task_attempts SET status='failed',failure_signature=?,failure_reason=?,completed_at=? WHERE attempt_id=? AND status IN ('running','checkpointed')")
                .bind(&signature).bind(reason).bind(now.to_rfc3339()).bind(&offer.attempt.attempt_id).execute(&mut *tx).await?
        } else {
            sqlx::query("UPDATE task_attempts SET status='failed',failure_signature=?,failure_reason=?,completed_at=? WHERE attempt_id=? AND status='offered'")
                .bind(&signature).bind(reason).bind(now.to_rfc3339()).bind(&offer.attempt.attempt_id).execute(&mut *tx).await?
        };
        if attempt_update.rows_affected() != 1 {
            return Err(StorageError::LateSubmission(
                "attempt changed while its failure was being recorded".into(),
            ));
        }
        if let Some(lease) = lease {
            let lease_update = sqlx::query("UPDATE task_leases SET status='failed',completed_at=? WHERE lease_id=? AND attempt_id=? AND lease_epoch=? AND lease_token_hash=? AND status='active'")
                .bind(now.to_rfc3339()).bind(&lease.lease_id).bind(&offer.attempt.attempt_id)
                .bind(lease.lease_epoch).bind(hash_secret(&lease.lease_token)).execute(&mut *tx).await?;
            if lease_update.rows_affected() != 1 {
                return Err(StorageError::LateSubmission(
                    "lease changed while its failure was being recorded".into(),
                ));
            }
        }
        let task_update = if lease.is_some() {
            sqlx::query("UPDATE tasks SET status=?,revision=revision+1,result_summary=? WHERE task_id=? AND revision=? AND route_cancellation_epoch=? AND status IN ('running','checkpointed')")
                .bind(next_status).bind(reason).bind(&offer.task.task_id).bind(expected_revision)
                .bind(route_epoch).execute(&mut *tx).await?
        } else {
            sqlx::query("UPDATE tasks SET status=?,revision=revision+1,result_summary=? WHERE task_id=? AND revision=? AND route_cancellation_epoch=? AND status='offered'")
                .bind(next_status).bind(reason).bind(&offer.task.task_id).bind(expected_revision)
                .bind(route_epoch).execute(&mut *tx).await?
        };
        if task_update.rows_affected() != 1 {
            return Err(StorageError::LateSubmission(
                "task changed while its failure was being recorded".into(),
            ));
        }
        let route_update = sqlx::query("UPDATE routes SET failed_attempt_count=failed_attempt_count+1 WHERE project_id=? AND route_id=? AND status='active' AND cancellation_epoch=?")
            .bind(&offer.task.project_id).bind(&offer.task.route_id).bind(route_epoch)
            .execute(&mut *tx).await?;
        if route_update.rows_affected() != 1 {
            return Err(StorageError::LateSubmission(
                "route changed while its attempt failure was being recorded".into(),
            ));
        }
        let instance_update = sqlx::query("UPDATE worker_instances SET status=CASE WHEN ? THEN 'quarantined' ELSE 'exited' END,quarantine_reason=CASE WHEN ? THEN ? ELSE quarantine_reason END,exited_at=? WHERE worker_instance_id=? AND project_id=? AND status IN ('handshaking','running','checkpointing')")
            .bind(terminal).bind(terminal).bind(reason).bind(now.to_rfc3339())
            .bind(&offer.worker_instance.worker_instance_id).bind(&offer.task.project_id)
            .execute(&mut *tx).await?;
        if instance_update.rows_affected() != 1 {
            return Err(StorageError::LateSubmission(
                "worker instance changed while its attempt failure was being recorded".into(),
            ));
        }
        if let Some(worker_id) = &offer.task.worker_id {
            sqlx::query("UPDATE workers SET status=CASE WHEN ? THEN 'quarantined' ELSE 'backoff' END,current_task_id=NULL,current_route_id=NULL,last_heartbeat=? WHERE project_id=? AND worker_id=?")
                .bind(terminal).bind(now.to_rfc3339()).bind(&offer.task.project_id).bind(worker_id)
                .execute(&mut *tx).await?;
        }
        let revision = bump_revision(&mut tx, &offer.task.project_id).await?;
        let event_type = if terminal {
            "task.dead_lettered"
        } else {
            "task.attempt_failed"
        };
        let event = append_event(&mut tx,&offer.task.project_id,revision,event_type,entity("task_attempt",&offer.attempt.attempt_id),json!({"task_id":offer.task.task_id,"failure_signature":signature,"reason":reason,"next_status":next_status}),Some(entity("task",&offer.task.task_id))).await?;
        tx.commit().await?;
        Ok(vec![event])
    }

    pub async fn pending_result_envelopes(&self, project_id: &str) -> StorageResult<Vec<String>> {
        self.get_project(project_id).await?;
        Ok(sqlx::query_scalar("SELECT result_envelope_id FROM result_envelopes WHERE project_id=? AND status='submitted' ORDER BY submitted_at")
            .bind(project_id).fetch_all(self.pool()).await?)
    }

    pub async fn list_plan_revisions(
        &self,
        project_id: &str,
    ) -> StorageResult<Vec<PlanRevisionRecord>> {
        self.get_project(project_id).await?;
        let rows =
            sqlx::query("SELECT * FROM plan_revisions WHERE project_id=? ORDER BY ordinal DESC")
                .bind(project_id)
                .fetch_all(self.pool())
                .await?;
        rows.iter().map(plan_revision_from_row).collect()
    }

    pub async fn get_plan_revision(
        &self,
        project_id: &str,
        plan_revision_id: &str,
    ) -> StorageResult<PlanRevisionRecord> {
        let row =
            sqlx::query("SELECT * FROM plan_revisions WHERE project_id=? AND plan_revision_id=?")
                .bind(project_id)
                .bind(plan_revision_id)
                .fetch_optional(self.pool())
                .await?
                .ok_or_else(|| StorageError::NotFound {
                    kind: "plan_revision",
                    id: plan_revision_id.into(),
                })?;
        plan_revision_from_row(&row)
    }

    pub async fn list_bottlenecks(&self, project_id: &str) -> StorageResult<Vec<Bottleneck>> {
        self.get_project(project_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM bottlenecks WHERE project_id=? ORDER BY priority DESC,updated_at DESC",
        )
        .bind(project_id)
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(bottleneck_from_row).collect()
    }

    pub async fn fact_planning_impact(
        &self,
        project_id: &str,
        fact_id: &str,
    ) -> StorageResult<Vec<FactImpactRecord>> {
        let rows = sqlx::query(
            "SELECT * FROM fact_impact_records WHERE project_id=? AND fact_id=? ORDER BY source_revision DESC",
        )
        .bind(project_id)
        .bind(fact_id)
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(fact_impact_from_row).collect()
    }

    pub async fn get_task_contract(
        &self,
        project_id: &str,
        task_id: &str,
    ) -> StorageResult<TaskContract> {
        let row = sqlx::query("SELECT * FROM task_contracts WHERE project_id=? AND task_id=? ORDER BY contract_version DESC LIMIT 1")
            .bind(project_id).bind(task_id).fetch_optional(self.pool()).await?
            .ok_or_else(|| StorageError::NotFound { kind: "task_contract", id: task_id.into() })?;
        task_contract_from_row(&row)
    }

    pub async fn get_context_packet_for_task(&self, task_id: &str) -> StorageResult<ContextPacket> {
        let row = sqlx::query("SELECT * FROM context_packets WHERE task_id=? ORDER BY source_revision DESC,created_at DESC LIMIT 1")
            .bind(task_id).fetch_optional(self.pool()).await?
            .ok_or_else(|| StorageError::NotFound { kind: "context_packet", id: task_id.into() })?;
        context_packet_from_row(&row)
    }

    pub async fn list_task_attempts(
        &self,
        project_id: &str,
        task_id: &str,
    ) -> StorageResult<Vec<TaskAttempt>> {
        let rows = sqlx::query(
            "SELECT * FROM task_attempts WHERE project_id=? AND task_id=? ORDER BY attempt_no DESC",
        )
        .bind(project_id)
        .bind(task_id)
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(task_attempt_from_row).collect()
    }

    pub async fn task_attempt_history(
        &self,
        project_id: &str,
        task_id: &str,
    ) -> StorageResult<Value> {
        self.get_task(project_id, task_id).await?;
        let attempts = self.list_task_attempts(project_id, task_id).await?;
        let leases = sqlx::query("SELECT lease_id,attempt_id,worker_instance_id,node_id,lease_epoch,task_revision,route_epoch,status,offered_at,leased_at,expires_at,last_heartbeat_at,completed_at FROM task_leases WHERE project_id=? AND task_id=? ORDER BY lease_epoch DESC,leased_at DESC")
            .bind(project_id).bind(task_id).fetch_all(self.pool()).await?;
        let leases = leases
            .iter()
            .map(|row| {
                Ok(json!({
                    "lease_id":row.try_get::<String,_>("lease_id")?,
                    "attempt_id":row.try_get::<Option<String>,_>("attempt_id")?,
                    "worker_instance_id":row.try_get::<Option<String>,_>("worker_instance_id")?,
                    "node_id":row.try_get::<String,_>("node_id")?,
                    "lease_epoch":row.try_get::<i64,_>("lease_epoch")?,
                    "task_revision":row.try_get::<i64,_>("task_revision")?,
                    "route_epoch":row.try_get::<i64,_>("route_epoch")?,
                    "status":row.try_get::<String,_>("status")?,
                    "offered_at":row.try_get::<Option<String>,_>("offered_at")?,
                    "leased_at":row.try_get::<String,_>("leased_at")?,
                    "expires_at":row.try_get::<String,_>("expires_at")?,
                    "last_heartbeat_at":row.try_get::<Option<String>,_>("last_heartbeat_at")?,
                    "completed_at":row.try_get::<Option<String>,_>("completed_at")?,
                }))
            })
            .collect::<StorageResult<Vec<_>>>()?;
        let checkpoints=sqlx::query("SELECT c.checkpoint_id,c.attempt_id,c.artifact_id,c.lease_epoch,c.summary,c.created_at,a.sha256,a.storage_path FROM task_checkpoints c JOIN artifacts a ON a.artifact_id=c.artifact_id WHERE c.project_id=? AND c.task_id=? ORDER BY c.created_at DESC,c.checkpoint_id DESC")
            .bind(project_id).bind(task_id).fetch_all(self.pool()).await?;
        let checkpoints = checkpoints
            .iter()
            .map(|row| {
                Ok(json!({
                    "checkpoint_id":row.try_get::<String,_>("checkpoint_id")?,
                    "attempt_id":row.try_get::<String,_>("attempt_id")?,
                    "artifact_id":row.try_get::<String,_>("artifact_id")?,
                    "lease_epoch":row.try_get::<i64,_>("lease_epoch")?,
                    "summary":row.try_get::<String,_>("summary")?,
                    "created_at":row.try_get::<String,_>("created_at")?,
                    "artifact_sha256":row.try_get::<String,_>("sha256")?,
                    "artifact_storage_path":row.try_get::<String,_>("storage_path")?,
                }))
            })
            .collect::<StorageResult<Vec<_>>>()?;
        Ok(json!({
            "task_id":task_id,
            "attempts":attempts,
            "leases":leases,
            "checkpoints":checkpoints,
        }))
    }

    pub async fn list_worker_instances(
        &self,
        project_id: &str,
    ) -> StorageResult<Vec<WorkerInstance>> {
        self.get_project(project_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM worker_instances WHERE project_id=? ORDER BY started_at DESC",
        )
        .bind(project_id)
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(worker_instance_from_row).collect()
    }

    pub async fn list_route_families(&self, project_id: &str) -> StorageResult<Vec<Value>> {
        self.get_project(project_id).await?;
        let rows = sqlx::query("SELECT * FROM route_families WHERE project_id=? ORDER BY updated_revision DESC,family_id")
            .bind(project_id).fetch_all(self.pool()).await?;
        rows.iter()
            .map(|row| {
                Ok(json!({
                    "family_id":row.try_get::<String,_>("family_id")?,
                    "project_id":row.try_get::<String,_>("project_id")?,
                    "semantic_key":row.try_get::<String,_>("semantic_key")?,
                    "canonical_route_id":row.try_get::<Option<String>,_>("canonical_route_id")?,
                    "status":row.try_get::<String,_>("status")?,
                    "created_revision":row.try_get::<i64,_>("created_revision")?,
                    "updated_revision":row.try_get::<i64,_>("updated_revision")?,
                }))
            })
            .collect()
    }

    pub async fn list_route_progress(
        &self,
        project_id: &str,
        route_id: &str,
    ) -> StorageResult<Vec<Value>> {
        self.get_route(project_id, route_id).await?;
        let rows = sqlx::query("SELECT * FROM route_progress_entries WHERE project_id=? AND route_id=? ORDER BY project_revision,created_at")
            .bind(project_id).bind(route_id).fetch_all(self.pool()).await?;
        rows.iter().map(|row| Ok(json!({
            "progress_entry_id":row.try_get::<String,_>("progress_entry_id")?,
            "project_id":row.try_get::<String,_>("project_id")?,
            "route_id":row.try_get::<String,_>("route_id")?,
            "source_entity":serde_json::from_str::<Value>(row.try_get("source_entity_json")?)?,
            "progress_kind":row.try_get::<String,_>("progress_kind")?,
            "material":row.try_get::<bool,_>("material")?,
            "summary":row.try_get::<String,_>("summary")?,
            "evidence_ids":serde_json::from_str::<Value>(row.try_get("evidence_ids_json")?)?,
            "project_revision":row.try_get::<i64,_>("project_revision")?,
            "created_at":row.try_get::<String,_>("created_at")?,
        }))).collect()
    }

    pub async fn route_progress_digest(&self, route_id: &str) -> StorageResult<Value> {
        let row = sqlx::query("SELECT * FROM routes WHERE route_id=?")
            .bind(route_id)
            .fetch_optional(self.pool())
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "route",
                id: route_id.into(),
            })?;
        let route = rows::route(&row)?;
        let progress = self
            .list_route_progress(&route.project_id, route_id)
            .await?;
        Ok(json!({
            "project_id":route.project_id,
            "route_id":route.route_id,
            "family_id":route.family_id,
            "status":route.status,
            "last_material_progress_revision":route.last_material_progress_revision,
            "consecutive_no_progress_plans":route.consecutive_no_progress_plans,
            "failed_attempt_count":route.failed_attempt_count,
            "entries":progress,
            "trust_rule":"This digest summarizes route progress and is not a mathematical premise.",
        }))
    }

    pub async fn list_route_tombstones(&self, project_id: &str) -> StorageResult<Vec<Value>> {
        self.get_project(project_id).await?;
        let rows = sqlx::query("SELECT * FROM route_tombstones WHERE project_id=? ORDER BY created_revision DESC,created_at DESC")
            .bind(project_id).fetch_all(self.pool()).await?;
        rows.iter().map(|row| Ok(json!({
            "tombstone_id":row.try_get::<String,_>("tombstone_id")?,
            "route_id":row.try_get::<String,_>("route_id")?,
            "semantic_fingerprint":row.try_get::<String,_>("semantic_fingerprint")?,
            "reason":row.try_get::<String,_>("reason")?,
            "blocking_fact_pattern":row.try_get::<Option<String>,_>("blocking_fact_pattern")?,
            "revive_only_if":serde_json::from_str::<Value>(row.try_get("revive_only_if_json")?)?,
            "created_revision":row.try_get::<i64,_>("created_revision")?,
            "revived_revision":row.try_get::<Option<i64>,_>("revived_revision")?,
            "created_at":row.try_get::<String,_>("created_at")?,
        }))).collect()
    }

    pub async fn get_context_packet(
        &self,
        context_packet_id: &str,
    ) -> StorageResult<ContextPacket> {
        let row = sqlx::query("SELECT * FROM context_packets WHERE context_packet_id=?")
            .bind(context_packet_id)
            .fetch_optional(self.pool())
            .await?
            .ok_or_else(|| StorageError::NotFound {
                kind: "context_packet",
                id: context_packet_id.into(),
            })?;
        context_packet_from_row(&row)
    }

    pub async fn context_digest(&self, project_id: &str) -> StorageResult<Value> {
        let project = self.get_project(project_id).await?;
        let delta = self.collect_research_delta(project_id).await?;
        let bottlenecks = self.list_bottlenecks(project_id).await?;
        let obligations = self.list_proof_obligations(project_id).await?;
        let routes = self.list_routes(project_id).await?;
        let tasks = self.list_tasks(project_id).await?;
        let summaries = sqlx::query("SELECT summary_id,summary_kind,scope_kind,scope_id,input_entity_ids_json,input_revision,summarizer_version,content_hash,omitted_categories_json,status,rebuild_condition,created_at,invalidated_at FROM context_summaries WHERE project_id=? ORDER BY input_revision DESC,created_at DESC")
            .bind(project_id).fetch_all(self.pool()).await?;
        let summaries = summaries.iter().map(|row| Ok(json!({
            "summary_id":row.try_get::<String,_>("summary_id")?,
            "summary_kind":row.try_get::<String,_>("summary_kind")?,
            "scope_kind":row.try_get::<String,_>("scope_kind")?,
            "scope_id":row.try_get::<String,_>("scope_id")?,
            "input_entity_ids":serde_json::from_str::<Value>(row.try_get("input_entity_ids_json")?)?,
            "input_revision":row.try_get::<i64,_>("input_revision")?,
            "summarizer_version":row.try_get::<String,_>("summarizer_version")?,
            "content_hash":row.try_get::<String,_>("content_hash")?,
            "omitted_categories":serde_json::from_str::<Value>(row.try_get("omitted_categories_json")?)?,
            "status":row.try_get::<String,_>("status")?,
            "rebuild_condition":row.try_get::<Option<String>,_>("rebuild_condition")?,
            "created_at":row.try_get::<String,_>("created_at")?,
            "invalidated_at":row.try_get::<Option<String>,_>("invalidated_at")?,
        }))).collect::<StorageResult<Vec<_>>>()?;
        let fact_decision_rows = sqlx::query("SELECT d.decision_id,d.plan_revision_id,p.ordinal,d.fact_id,d.disposition,d.reason,d.affected_entity_ids_json,d.created_at FROM plan_fact_decisions d JOIN plan_revisions p ON p.plan_revision_id=d.plan_revision_id WHERE d.project_id=? ORDER BY p.ordinal DESC,d.created_at DESC LIMIT 100")
            .bind(project_id)
            .fetch_all(self.pool())
            .await?;
        let recent_plan_fact_decisions = fact_decision_rows
            .iter()
            .map(|row| {
                Ok(json!({
                    "decision_id":row.try_get::<String,_>("decision_id")?,
                    "plan_revision_id":row.try_get::<String,_>("plan_revision_id")?,
                    "plan_ordinal":row.try_get::<i64,_>("ordinal")?,
                    "fact_id":row.try_get::<String,_>("fact_id")?,
                    "disposition":row.try_get::<String,_>("disposition")?,
                    "reason":row.try_get::<String,_>("reason")?,
                    "affected_entity_ids":serde_json::from_str::<Value>(row.try_get("affected_entity_ids_json")?)?,
                    "created_at":row.try_get::<String,_>("created_at")?,
                }))
            })
            .collect::<StorageResult<Vec<_>>>()?;
        Ok(json!({
            "project_id":project_id,
            "project_revision":project.revision,
            "research_delta":delta,
            "open_bottlenecks":bottlenecks.into_iter().filter(|item| item.status=="open").collect::<Vec<_>>(),
            "open_proof_obligations":obligations.into_iter().filter(|item| matches!(item.status,research_domain::ProofObligationStatus::Open|research_domain::ProofObligationStatus::Blocked)).collect::<Vec<_>>(),
            "live_routes":routes.into_iter().filter(|item| matches!(item.status.to_string().as_str(),"incubating"|"active"|"blocked"|"probation"|"revived")).collect::<Vec<_>>(),
            "live_tasks":tasks.into_iter().filter(|item| matches!(item.status,TaskStatus::Queued|TaskStatus::Offered|TaskStatus::Leased|TaskStatus::Running|TaskStatus::Checkpointed|TaskStatus::ResultSubmitted|TaskStatus::Ingesting)).collect::<Vec<_>>(),
            "summaries":summaries,
            "recent_plan_fact_decisions":recent_plan_fact_decisions,
            "trust_rule":"This digest is a planning and proof-coverage view, not a mathematical premise. Advisory obligations are non-binding; retrieve active facts by fact_id.",
        }))
    }

    pub async fn prune_route_v2(
        &self,
        project_id: &str,
        route_id: &str,
        expected_revision: i64,
        reason: &str,
    ) -> StorageResult<research_domain::DomainEvent> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "prune_route",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let actual = current_revision(&mut tx, project_id).await?;
        if actual != expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: expected_revision,
                actual,
            });
        }
        let now = Utc::now();
        let mutation =
            prune_route_tx(&mut tx, project_id, route_id, actual + 1, reason, &now).await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            "route.pruned",
            entity("route", route_id),
            json!({"reason":reason,"source":"direct_v2","tombstone_id":mutation.tombstone_id}),
            None,
        )
        .await?;
        tx.commit().await?;
        Ok(event)
    }

    pub async fn merge_routes_v2(
        &self,
        project_id: &str,
        canonical_route_id: &str,
        source_route_ids: &[String],
        expected_revision: i64,
        reason: &str,
    ) -> StorageResult<Vec<research_domain::DomainEvent>> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "merge_routes",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let actual = current_revision(&mut tx, project_id).await?;
        if actual != expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: expected_revision,
                actual,
            });
        }
        let now = Utc::now();
        let merged = merge_routes_tx(
            &mut tx,
            project_id,
            canonical_route_id,
            source_route_ids,
            reason,
            &now,
        )
        .await?;
        let revision = actual + 1;
        let mut events = Vec::new();
        for source in &merged.routes {
            events.push(
                append_event(
                    &mut tx,
                    project_id,
                    revision,
                    "route.merged",
                    entity("route", source),
                    json!({"canonical_route_id":canonical_route_id,"reason":reason}),
                    Some(entity("route", canonical_route_id)),
                )
                .await?,
            );
        }
        let bumped = bump_revision(&mut tx, project_id).await?;
        debug_assert_eq!(bumped, revision);
        tx.commit().await?;
        Ok(events)
    }

    pub async fn revive_route_v2(
        &self,
        project_id: &str,
        route_id: &str,
        expected_revision: i64,
        reason: &str,
        evidence_ids: &[String],
    ) -> StorageResult<research_domain::DomainEvent> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::HumanSafety,
                "revive_route",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let actual = current_revision(&mut tx, project_id).await?;
        if actual != expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: expected_revision,
                actual,
            });
        }
        revive_route_tx(
            &mut tx,
            project_id,
            route_id,
            actual + 1,
            reason,
            evidence_ids,
        )
        .await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            "route.revived",
            entity("route", route_id),
            json!({"reason":reason,"evidence_ids":evidence_ids}),
            None,
        )
        .await?;
        tx.commit().await?;
        Ok(event)
    }

    pub async fn planner_health(&self, project_id: &str) -> StorageResult<PlannerHealth> {
        self.get_project(project_id).await?;
        let row = sqlx::query("SELECT * FROM planner_health WHERE project_id=?")
            .bind(project_id)
            .fetch_one(self.pool())
            .await?;
        planner_health_from_row(&row)
    }

    pub async fn planner_call_permitted(&self, project_id: &str) -> StorageResult<bool> {
        let health = self.planner_health(project_id).await?;
        if health.circuit_state == "closed" || health.circuit_state == "half_open" {
            return Ok(true);
        }
        let Some(cooldown_until) = health.cooldown_until else {
            return Ok(false);
        };
        if cooldown_until > Utc::now() {
            return Ok(false);
        }
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "planner_circuit_probe",
            )
            .await?;
        let changed = sqlx::query("UPDATE planner_health SET circuit_state='half_open',last_probe_at=?,updated_at=? WHERE project_id=? AND circuit_state='open' AND cooldown_until<=?")
            .bind(Utc::now().to_rfc3339()).bind(Utc::now().to_rfc3339()).bind(project_id)
            .bind(Utc::now().to_rfc3339()).execute(self.pool()).await?;
        Ok(changed.rows_affected() == 1)
    }

    pub async fn record_planner_success(
        &self,
        project_id: &str,
    ) -> StorageResult<research_domain::DomainEvent> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "record_planner_success",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        sqlx::query("UPDATE planner_health SET circuit_state='closed',consecutive_failures=0,last_failure_reason=NULL,opened_at=NULL,cooldown_until=NULL,updated_at=? WHERE project_id=?")
            .bind(Utc::now().to_rfc3339()).bind(project_id).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            "planner.circuit.closed",
            entity("planner_health", project_id),
            json!({"reason":"planner call succeeded"}),
            None,
        )
        .await?;
        tx.commit().await?;
        Ok(event)
    }

    pub async fn record_planner_failure(
        &self,
        project_id: &str,
        reason: &str,
    ) -> StorageResult<research_domain::DomainEvent> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "record_planner_failure",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        let row = sqlx::query(
            "SELECT consecutive_failures,circuit_state FROM planner_health WHERE project_id=?",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        let failures = row.try_get::<i64, _>("consecutive_failures")? + 1;
        let previous: String = row.try_get("circuit_state")?;
        let opens = failures >= 2 || previous == "half_open";
        let state = if opens { "open" } else { "closed" };
        let now = Utc::now();
        let cooldown = opens.then(|| now + chrono::Duration::minutes(5));
        sqlx::query("UPDATE planner_health SET circuit_state=?,consecutive_failures=?,last_failure_reason=?,opened_at=CASE WHEN ? THEN COALESCE(opened_at,?) ELSE opened_at END,cooldown_until=?,updated_at=? WHERE project_id=?")
            .bind(state).bind(failures).bind(reason).bind(opens).bind(now.to_rfc3339())
            .bind(cooldown.map(|value| value.to_rfc3339())).bind(now.to_rfc3339()).bind(project_id)
            .execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            if opens { "planner.circuit.opened" } else { "planner.call.failed" },
            entity("planner_health", project_id),
            json!({"reason":reason,"consecutive_failures":failures,"circuit_state":state,"cooldown_until":cooldown}),
            None,
        )
        .await?;
        tx.commit().await?;
        Ok(event)
    }

    pub async fn record_planner_degraded_waiting(
        &self,
        project_id: &str,
        reason: &str,
    ) -> StorageResult<research_domain::DomainEvent> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::PlanningOrCheckpoint,
                "record_planner_degraded_waiting",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        // No legal work remains: yield durably instead of burning empty rounds.
        // Never overwrite a concurrent user stop or a completed/budget terminal state.
        let paused = sqlx::query("UPDATE projects SET status='paused',updated_at=? WHERE project_id=? AND status='running'")
            .bind(Utc::now().to_rfc3339()).bind(project_id).execute(&mut *tx).await?.rows_affected() == 1;
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            "planner.degraded_waiting",
            entity("project", project_id),
            json!({"reason":reason,"new_assignments":0,"paused":paused,"retry_policy":"resume after planner recovery"}),
            None,
        )
        .await?;
        tx.commit().await?;
        Ok(event)
    }

    pub async fn storage_health(&self) -> StorageResult<StorageHealth> {
        let writer = self.state_writer_status();
        let outbox_pending: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM event_outbox WHERE status='pending'")
                .fetch_one(self.pool())
                .await?;
        let oldest: Option<String> =
            sqlx::query_scalar("SELECT MIN(available_at) FROM event_outbox WHERE status='pending'")
                .fetch_one(self.pool())
                .await?;
        let orphaned_task_attempts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_attempts a LEFT JOIN task_leases l ON a.attempt_id=l.attempt_id AND l.status='active' WHERE a.status IN ('offered','leased','running','checkpointed','result_submitted','ingesting') AND l.lease_id IS NULL AND NOT (a.status IN ('result_submitted','ingesting') AND EXISTS (SELECT 1 FROM result_envelopes e WHERE e.attempt_id=a.attempt_id AND e.status='submitted'))")
            .fetch_one(self.pool()).await?;
        let expired_active_leases: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_leases WHERE status='active' AND expires_at<?",
        )
        .bind(Utc::now().to_rfc3339())
        .fetch_one(self.pool())
        .await?;
        let last_row =
            sqlx::query("SELECT * FROM reconciliation_runs ORDER BY started_at DESC LIMIT 1")
                .fetch_optional(self.pool())
                .await?;
        let last_reconciliation = last_row.map(|row| {
            json!({
                "reconciliation_run_id": row.try_get::<String,_>("reconciliation_run_id").ok(),
                "project_id": row.try_get::<Option<String>,_>("project_id").ok().flatten(),
                "trigger_kind": row.try_get::<String,_>("trigger_kind").ok(),
                "status": row.try_get::<String,_>("status").ok(),
                "started_at": row.try_get::<String,_>("started_at").ok(),
                "completed_at": row.try_get::<Option<String>,_>("completed_at").ok().flatten(),
            })
        });
        let oldest_age = oldest
            .as_deref()
            .map(timestamp)
            .transpose()?
            .map(|time| (Utc::now() - time).num_seconds().max(0));
        Ok(StorageHealth {
            mode: "sqlite_single_writer_read_pool".into(),
            state_writer_status: writer.status.into(),
            state_writer_last_command_kind: writer.last_command_kind,
            state_writer_queue_depth: writer.queue_depth,
            state_writer_queue_capacity: writer.queue_capacity,
            state_writer_admitted_total: i64::try_from(writer.admitted_total).unwrap_or(i64::MAX),
            state_writer_completed_total: i64::try_from(writer.completed_total).unwrap_or(i64::MAX),
            state_writer_queue_wait_seconds: writer.queue_wait_seconds,
            state_writer_last_transaction_seconds: writer.last_transaction_seconds,
            outbox_pending,
            outbox_oldest_age_seconds: oldest_age,
            orphaned_task_attempts,
            expired_active_leases,
            last_reconciliation,
        })
    }

    pub async fn run_reconciliation(
        &self,
        project_id: Option<&str>,
        trigger_kind: &str,
    ) -> StorageResult<Value> {
        if let Some(project_id) = project_id {
            self.get_project(project_id).await?;
        }
        // Artifact I/O stays outside the StateWriter admission and database transaction.
        // A service startup checks a bounded batch: never-audited/failed/hash-changed
        // artifacts are selected before a cursor-based rotation over verified history.
        // Explicit operator reconciliation retains the full audit behavior.
        let integrity_scan = match trigger_kind {
            "watchdog_tick" => ArtifactIntegrityScan::skipped(project_id),
            "service_startup" => {
                scan_artifact_integrity_incremental(
                    self,
                    project_id,
                    STARTUP_ARTIFACT_AUDIT_BATCH_SIZE,
                )
                .await?
            }
            _ => scan_artifact_integrity_full(self, project_id).await?,
        };
        let integrity_issues = integrity_scan.issues();
        // A verification worker may have durably submitted its envelope immediately before
        // the process stopped. Ingest those envelopes while their original fencing data is
        // still intact; resetting the case first would destroy a valid, replayable result.
        // Do this after the slower artifact scan to keep the gap before writer admission small.
        let mut recovered_verification_repairs = Vec::new();
        if trigger_kind == "service_startup" {
            let pending = if let Some(project_id) = project_id {
                self.pending_verification_result_envelopes(project_id)
                    .await?
            } else {
                sqlx::query_scalar("SELECT verification_result_envelope_id FROM verification_result_envelopes WHERE status='submitted' ORDER BY submitted_at")
                    .fetch_all(self.pool())
                    .await?
            };
            for envelope_id in pending {
                match self.ingest_verification_result(&envelope_id).await {
                    Ok((_, events)) => {
                        if !events.is_empty() {
                            recovered_verification_repairs.push(json!({
                                "kind":"verification_result_recovered",
                                "verification_result_envelope_id":envelope_id,
                            }));
                        }
                    }
                    Err(StorageError::LateSubmission(reason)) => {
                        recovered_verification_repairs.push(json!({
                            "kind":"stale_verification_result_rejected",
                            "verification_result_envelope_id":envelope_id,
                            "reason":reason,
                        }));
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "run_reconciliation",
            )
            .await?;
        let run_id = new_id("reconciliation");
        let started_at = Utc::now();
        let mut tx = self.pool().begin().await?;
        sqlx::query("INSERT INTO reconciliation_runs(reconciliation_run_id,project_id,trigger_kind,status,started_at) VALUES(?,?,?,'running',?)")
            .bind(&run_id).bind(project_id).bind(trigger_kind).bind(started_at.to_rfc3339()).execute(&mut *tx).await?;
        let scope = project_id.unwrap_or("");
        let reconciliation_now = Utc::now();
        let now = reconciliation_now.to_rfc3339();
        if trigger_kind == "service_startup" {
            if project_id.is_some() {
                sqlx::query("UPDATE task_leases SET expires_at='1970-01-01T00:00:00+00:00' WHERE project_id=? AND status='active' AND node_id LIKE 'local:%'")
                    .bind(scope).execute(&mut *tx).await?;
                sqlx::query("UPDATE verification_task_leases SET expires_at='1970-01-01T00:00:00+00:00' WHERE project_id=? AND status='active'")
                    .bind(scope).execute(&mut *tx).await?;
            } else {
                sqlx::query("UPDATE task_leases SET expires_at='1970-01-01T00:00:00+00:00' WHERE status='active' AND node_id LIKE 'local:%'")
                    .execute(&mut *tx).await?;
                sqlx::query("UPDATE verification_task_leases SET expires_at='1970-01-01T00:00:00+00:00' WHERE status='active'")
                    .execute(&mut *tx).await?;
            }
        }
        let expired = if project_id.is_some() {
            sqlx::query("SELECT project_id,lease_id,task_id,attempt_id,worker_instance_id FROM task_leases WHERE project_id=? AND status='active' AND expires_at<?")
                .bind(scope).bind(&now).fetch_all(&mut *tx).await?
        } else {
            sqlx::query("SELECT project_id,lease_id,task_id,attempt_id,worker_instance_id FROM task_leases WHERE status='active' AND expires_at<?")
                .bind(&now).fetch_all(&mut *tx).await?
        };
        let mut repairs = recovered_verification_repairs;
        persist_artifact_integrity_scan(&mut tx, &integrity_scan).await?;
        if trigger_kind == "service_startup" {
            let interrupted_calls = if project_id.is_some() {
                sqlx::query("UPDATE usage_records SET outcome='failed',error_kind='service_restart',error_message='model call was still running when the service restarted' WHERE project_id=? AND outcome='running'")
                    .bind(scope).execute(&mut *tx).await?
            } else {
                sqlx::query("UPDATE usage_records SET outcome='failed',error_kind='service_restart',error_message='model call was still running when the service restarted' WHERE outcome='running'")
                    .execute(&mut *tx).await?
            };
            if interrupted_calls.rows_affected() > 0 {
                repairs.push(json!({
                    "kind":"interrupted_model_calls_closed",
                    "count":interrupted_calls.rows_affected(),
                }));
            }
        }
        let mut repair_events: BTreeMap<String, Vec<(String, EntityRef, Value)>> = BTreeMap::new();
        let abandoned_offer_cutoff = if trigger_kind == "service_startup" {
            now.clone()
        } else {
            (Utc::now() - chrono::Duration::minutes(5)).to_rfc3339()
        };
        let abandoned_offers = if project_id.is_some() {
            sqlx::query("SELECT a.project_id,a.attempt_id,a.task_id,a.worker_instance_id FROM task_attempts a JOIN tasks t ON t.task_id=a.task_id JOIN projects p ON p.project_id=a.project_id JOIN routes r ON r.project_id=t.project_id AND r.route_id=t.route_id WHERE a.project_id=? AND a.status='offered' AND t.status='offered' AND p.status IN ('running','needs_human_review') AND r.human_review IN ('approved','not_required') AND r.status IN ('incubating','active','probation','revived') AND a.created_at<=? AND NOT EXISTS (SELECT 1 FROM task_leases l WHERE l.attempt_id=a.attempt_id) AND NOT EXISTS (SELECT 1 FROM task_attempts newer WHERE newer.task_id=a.task_id AND newer.attempt_no>a.attempt_no)")
                .bind(scope).bind(&abandoned_offer_cutoff).fetch_all(&mut *tx).await?
        } else {
            sqlx::query("SELECT a.project_id,a.attempt_id,a.task_id,a.worker_instance_id FROM task_attempts a JOIN tasks t ON t.task_id=a.task_id JOIN projects p ON p.project_id=a.project_id JOIN routes r ON r.project_id=t.project_id AND r.route_id=t.route_id WHERE a.status='offered' AND t.status='offered' AND p.status IN ('running','needs_human_review') AND r.human_review IN ('approved','not_required') AND r.status IN ('incubating','active','probation','revived') AND a.created_at<=? AND NOT EXISTS (SELECT 1 FROM task_leases l WHERE l.attempt_id=a.attempt_id) AND NOT EXISTS (SELECT 1 FROM task_attempts newer WHERE newer.task_id=a.task_id AND newer.attempt_no>a.attempt_no)")
                .bind(&abandoned_offer_cutoff).fetch_all(&mut *tx).await?
        };
        for row in abandoned_offers {
            let repair_project_id: String = row.try_get("project_id")?;
            let attempt_id: String = row.try_get("attempt_id")?;
            let task_id: String = row.try_get("task_id")?;
            let worker_instance_id: Option<String> = row.try_get("worker_instance_id")?;
            sqlx::query("UPDATE task_attempts SET status='orphaned',failure_reason='offer was not followed by a worker handshake',completed_at=? WHERE attempt_id=? AND status='offered'")
                .bind(&now).bind(&attempt_id).execute(&mut *tx).await?;
            sqlx::query("UPDATE tasks SET status=CASE WHEN (SELECT COUNT(*) FROM task_attempts WHERE task_id=?)>=3 THEN 'dead_lettered' ELSE 'queued' END,revision=revision+1,result_summary='abandoned worker offer recovered' WHERE task_id=? AND status='offered'")
                .bind(&task_id).bind(&task_id).execute(&mut *tx).await?;
            if let Some(worker_instance_id) = &worker_instance_id {
                sqlx::query("UPDATE worker_instances SET status='exited',quarantine_reason='offer was not followed by a worker handshake',exited_at=? WHERE worker_instance_id=? AND status NOT IN ('exited','quarantined')")
                    .bind(&now).bind(worker_instance_id).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE workers SET status='backoff',current_task_id=NULL,current_route_id=NULL,last_heartbeat=? WHERE project_id=? AND current_task_id=?")
                .bind(&now).bind(&repair_project_id).bind(&task_id).execute(&mut *tx).await?;
            repairs.push(json!({"kind":"abandoned_task_offer_recovered","task_id":task_id,"attempt_id":attempt_id,"worker_instance_id":worker_instance_id}));
            repair_events.entry(repair_project_id).or_default().push((
                "task.offer_recovered".into(),
                entity("task_attempt", &attempt_id),
                json!({"task_id":task_id,"attempt_id":attempt_id,"reason":"worker handshake was never committed"}),
            ));
        }
        for row in expired {
            let repair_project_id: String = row.try_get("project_id")?;
            let lease_id: String = row.try_get("lease_id")?;
            let task_id: String = row.try_get("task_id")?;
            let attempt_id: Option<String> = row.try_get("attempt_id")?;
            let worker_instance_id: Option<String> = row.try_get("worker_instance_id")?;
            let Some(outcome) = expire_task_lease_tx(
                &mut tx,
                TaskLeaseExpiry {
                    project: &repair_project_id,
                    lease: &lease_id,
                    task: &task_id,
                    attempt: attempt_id.as_deref(),
                    worker_instance: worker_instance_id.as_deref(),
                },
                &reconciliation_now,
            )
            .await?
            else {
                continue;
            };
            let has_submitted_envelope = outcome.replayable_result;
            repairs.push(json!({"kind":if has_submitted_envelope {"expired_lease_with_replayable_result"} else {"expired_lease"},"lease_id":lease_id,"task_id":task_id,"attempt_id":attempt_id,"worker_instance_id":worker_instance_id}));
            repair_events.entry(repair_project_id).or_default().push((
                "task.lease_expired".into(),
                entity("task_lease", &lease_id),
                json!({"task_id":task_id,"attempt_id":attempt_id,"worker_instance_id":worker_instance_id,"replayable_result":has_submitted_envelope}),
            ));
        }
        let expired_verification_leases = if project_id.is_some() {
            sqlx::query("SELECT project_id,verification_lease_id,case_id,attempt_id,worker_instance_id FROM verification_task_leases WHERE project_id=? AND status='active' AND expires_at<?")
                .bind(scope).bind(&now).fetch_all(&mut *tx).await?
        } else {
            sqlx::query("SELECT project_id,verification_lease_id,case_id,attempt_id,worker_instance_id FROM verification_task_leases WHERE status='active' AND expires_at<?")
                .bind(&now).fetch_all(&mut *tx).await?
        };
        for row in expired_verification_leases {
            let repair_project_id: String = row.try_get("project_id")?;
            let lease_id: String = row.try_get("verification_lease_id")?;
            let case_id: String = row.try_get("case_id")?;
            let attempt_id: String = row.try_get("attempt_id")?;
            let worker_instance_id: String = row.try_get("worker_instance_id")?;
            let replayable:i64=sqlx::query_scalar("SELECT COUNT(*) FROM verification_result_envelopes WHERE verification_lease_id=? AND status='submitted'")
                .bind(&lease_id).fetch_one(&mut *tx).await?;
            sqlx::query("UPDATE verification_task_leases SET status='expired',completed_at=? WHERE verification_lease_id=?")
                .bind(&now).bind(&lease_id).execute(&mut *tx).await?;
            if replayable == 0 || trigger_kind == "service_startup" {
                sqlx::query("UPDATE verification_attempts SET status='orphaned',error_kind='lease_expired',error_message='verification lease expired during reconciliation',completed_at=? WHERE attempt_id=? AND status NOT IN ('completed','failed','orphaned')")
                    .bind(&now).bind(&attempt_id).execute(&mut *tx).await?;
            }
            if replayable > 0 && trigger_kind == "service_startup" {
                sqlx::query("UPDATE verification_result_envelopes SET status='stale',rejection_reason='owning verification stage was interrupted by service restart' WHERE verification_lease_id=? AND status='submitted'")
                    .bind(&lease_id).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE worker_instances SET status='unhealthy',quarantine_reason='verification lease expired',exited_at=? WHERE worker_instance_id=?")
                .bind(&now).bind(&worker_instance_id).execute(&mut *tx).await?;
            repairs.push(json!({"kind":if replayable>0 && trigger_kind!="service_startup" {"expired_verification_lease_with_replayable_result"} else {"expired_verification_lease"},"verification_lease_id":lease_id,"case_id":case_id,"attempt_id":attempt_id,"worker_instance_id":worker_instance_id}));
            repair_events.entry(repair_project_id).or_default().push((
                "verification.lease_expired".into(),
                entity("verification_task_lease", &lease_id),
                json!({"case_id":case_id,"attempt_id":attempt_id,"worker_instance_id":worker_instance_id,"replayable_result":replayable>0}),
            ));
        }
        if trigger_kind == "service_startup" {
            let interrupted = if project_id.is_some() {
                sqlx::query("SELECT v.project_id,v.verification_id,v.candidate_id,vc.case_id FROM verifications v LEFT JOIN verification_cases vc ON vc.verification_id=v.verification_id WHERE v.project_id=? AND v.status='verifying'")
                    .bind(scope).fetch_all(&mut *tx).await?
            } else {
                sqlx::query("SELECT v.project_id,v.verification_id,v.candidate_id,vc.case_id FROM verifications v LEFT JOIN verification_cases vc ON vc.verification_id=v.verification_id WHERE v.status='verifying'")
                    .fetch_all(&mut *tx).await?
            };
            for row in interrupted {
                let repair_project_id: String = row.try_get("project_id")?;
                let verification_id: String = row.try_get("verification_id")?;
                let candidate_id: String = row.try_get("candidate_id")?;
                let case_id: Option<String> = row.try_get("case_id")?;
                sqlx::query("UPDATE verifications SET status='submitted',started_at=NULL WHERE verification_id=? AND status='verifying'")
                    .bind(&verification_id).execute(&mut *tx).await?;
                sqlx::query("UPDATE candidates SET status='submitted' WHERE candidate_id=? AND status='verifying'")
                    .bind(&candidate_id).execute(&mut *tx).await?;
                if let Some(case_id) = &case_id {
                    let recovered_results: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM verification_result_envelopes e JOIN verification_attempts a ON a.attempt_id=e.attempt_id JOIN verification_task_leases l ON l.verification_lease_id=e.verification_lease_id JOIN verification_cases c ON c.case_id=e.case_id WHERE e.case_id=? AND e.status='ingested' AND a.status='completed' AND l.status='completed' AND e.cancellation_epoch=c.cancellation_epoch AND e.cancellation_epoch=a.cancellation_epoch AND e.cancellation_epoch=l.cancellation_epoch AND e.lease_epoch=a.lease_epoch AND e.lease_epoch=l.lease_epoch")
                        .bind(case_id).fetch_one(&mut *tx).await?;
                    if recovered_results > 0 {
                        // Keep the epoch so the already-ingested reviewer/backend result remains
                        // replayable. The orchestration layer restarts from precheck and reuses
                        // completed worker stages instead of issuing duplicate model calls.
                        sqlx::query("UPDATE verification_cases SET stage=CASE WHEN snapshot_id IS NULL THEN 'intake' ELSE 'precheck' END,achieved_acceptance=NULL,completed_at=NULL,updated_at=? WHERE case_id=?")
                            .bind(&now).bind(case_id).execute(&mut *tx).await?;
                        sqlx::query("UPDATE verification_attempts SET status='orphaned',error_kind='service_restart',error_message='verification worker interrupted by service restart',completed_at=? WHERE case_id=? AND status NOT IN ('completed','failed','orphaned') AND NOT EXISTS (SELECT 1 FROM verification_result_envelopes e WHERE e.attempt_id=verification_attempts.attempt_id AND e.status='ingested')")
                            .bind(&now).bind(case_id).execute(&mut *tx).await?;
                    } else {
                        sqlx::query("UPDATE verification_cases SET stage=CASE WHEN snapshot_id IS NULL THEN 'intake' ELSE 'precheck' END,cancellation_epoch=cancellation_epoch+1,achieved_acceptance=NULL,completed_at=NULL,updated_at=? WHERE case_id=?")
                            .bind(&now).bind(case_id).execute(&mut *tx).await?;
                    }
                    sqlx::query("UPDATE verification_task_leases SET status='expired',completed_at=? WHERE case_id=? AND status='active'")
                        .bind(&now).bind(case_id).execute(&mut *tx).await?;
                    if recovered_results == 0 {
                        sqlx::query("UPDATE verification_result_envelopes SET status='stale',rejection_reason='verification stage interrupted by service restart' WHERE case_id=? AND status='submitted'")
                            .bind(case_id).execute(&mut *tx).await?;
                    }
                    repairs.push(json!({"kind":if recovered_results>0 {"verification_resumed_from_ingested_result"} else {"verification_reset_for_restart"},"verification_id":verification_id,"case_id":case_id,"candidate_id":candidate_id,"recovered_result_count":recovered_results}));
                } else {
                    repairs.push(json!({"kind":"verification_reset_for_restart","verification_id":verification_id,"candidate_id":candidate_id}));
                }
                repair_events.entry(repair_project_id).or_default().push((
                    "verification.reset_after_restart".into(),
                    entity("verification", &verification_id),
                    json!({"candidate_id":candidate_id,"case_id":case_id}),
                ));
            }
        }
        // Older builds persisted Worker-provided citation keys in
        // CandidateSubmission.external_source_ids. Repair those rows before the
        // orchestration layer claims submitted verifications. A deterministic
        // missing/ambiguous reference is terminally untrusted, not a project-wide
        // reconciliation error that will be retried forever.
        let legacy_source_references = if project_id.is_some() {
            sqlx::query("SELECT c.project_id,c.candidate_id,c.submission_json,v.verification_id,vc.snapshot_id,vs.candidate_hash AS snapshot_candidate_hash FROM candidates c JOIN verifications v ON v.candidate_id=c.candidate_id AND v.project_id=c.project_id LEFT JOIN verification_cases vc ON vc.verification_id=v.verification_id LEFT JOIN verification_snapshots vs ON vs.snapshot_id=vc.snapshot_id AND vs.case_id=vc.case_id WHERE c.project_id=? AND c.status='submitted' AND v.status='submitted' ORDER BY c.created_at,c.candidate_id")
                .bind(scope).fetch_all(&mut *tx).await?
        } else {
            sqlx::query("SELECT c.project_id,c.candidate_id,c.submission_json,v.verification_id,vc.snapshot_id,vs.candidate_hash AS snapshot_candidate_hash FROM candidates c JOIN verifications v ON v.candidate_id=c.candidate_id AND v.project_id=c.project_id LEFT JOIN verification_cases vc ON vc.verification_id=v.verification_id LEFT JOIN verification_snapshots vs ON vs.snapshot_id=vc.snapshot_id AND vs.case_id=vc.case_id WHERE c.status='submitted' AND v.status='submitted' ORDER BY c.project_id,c.created_at,c.candidate_id")
                .fetch_all(&mut *tx).await?
        };
        for row in legacy_source_references {
            let repair_project_id: String = row.try_get("project_id")?;
            let candidate_id: String = row.try_get("candidate_id")?;
            let verification_id: String = row.try_get("verification_id")?;
            let snapshot_id: Option<String> = row.try_get("snapshot_id")?;
            let snapshot_candidate_hash: Option<String> = row.try_get("snapshot_candidate_hash")?;
            let submission_json: String = row.try_get("submission_json")?;
            let mut submission: CandidateSubmission = serde_json::from_str(&submission_json)?;
            let observed_candidate_hash = hex::encode(Sha256::digest(submission_json.as_bytes()));
            let snapshot_integrity_rejection = match (
                snapshot_id.as_deref(),
                snapshot_candidate_hash.as_deref(),
            ) {
                (Some(snapshot_id), Some(expected_candidate_hash))
                    if !expected_candidate_hash.eq_ignore_ascii_case(&observed_candidate_hash) =>
                {
                    Some(crate::candidate_ingestion::SourceReferenceRejection {
                        summary: "candidate content no longer matches its immutable verification snapshot".into(),
                        audit: json!({
                            "disposition":"rejected_untrusted",
                            "reason":"immutable_verification_snapshot_candidate_hash_mismatch",
                            "snapshot_id":snapshot_id,
                            "expected_candidate_hash":expected_candidate_hash,
                            "observed_candidate_hash":observed_candidate_hash,
                        }),
                    })
                }
                _ => None,
            };
            let snapshot_integrity_failure = snapshot_integrity_rejection.is_some();
            let resolution = if let Some(rejection) = snapshot_integrity_rejection {
                Err(rejection)
            } else {
                if submission.external_source_ids.is_empty() {
                    continue;
                }
                crate::candidate_ingestion::normalize_source_references_tx(
                    &mut tx,
                    &repair_project_id,
                    &submission.external_source_ids,
                    &[],
                )
                .await?
            };
            let resolution = match resolution {
                Ok(normalization)
                    if snapshot_id.is_some() && normalization.requires_alias_rewrite =>
                {
                    Err(crate::candidate_ingestion::SourceReferenceRejection {
                        summary: "candidate source aliases require rewriting an immutable verification snapshot".into(),
                        audit: json!({
                            "disposition":"rejected_untrusted",
                            "reason":"immutable_verification_snapshot_source_alias_mismatch",
                            "snapshot_id":snapshot_id.as_deref(),
                            "proposed_normalization":normalization.audit,
                        }),
                    })
                }
                other => other,
            };
            match resolution {
                Ok(normalization) if normalization.changed && snapshot_id.is_none() => {
                    let original_source_references = submission.external_source_ids.clone();
                    submission.external_source_ids = normalization.source_ids;
                    sqlx::query("UPDATE candidates SET submission_json=? WHERE candidate_id=? AND status='submitted'")
                        .bind(json_text(&submission)?).bind(&candidate_id).execute(&mut *tx).await?;
                    repairs.push(json!({"kind":"candidate_source_references_normalized","candidate_id":candidate_id,"verification_id":verification_id,"source_reference_audit":normalization.audit}));
                    repair_events.entry(repair_project_id).or_default().push((
                        "claim.source_references.normalized".into(),
                        entity("candidate", &candidate_id),
                        json!({"verification_id":verification_id,"original_source_references":original_source_references,"normalized_source_ids":submission.external_source_ids,"source_reference_audit":normalization.audit}),
                    ));
                }
                Ok(_) => {}
                Err(rejection) => {
                    let (failure_type, error_kind, repair_kind, event_type) =
                        if snapshot_integrity_failure {
                            (
                                "verification_snapshot_candidate_hash_mismatch",
                                "verification_snapshot_candidate_hash_mismatch",
                                "verification_snapshot_candidate_hash_mismatch_terminalized",
                                "verification.snapshot_candidate_hash_mismatch",
                            )
                        } else {
                            (
                                "invalid_candidate_source_reference",
                                "invalid_source_reference",
                                "candidate_source_references_rejected_untrusted",
                                "claim.rejected_untrusted_source_reference",
                            )
                        };
                    let failure_id = new_id("failure");
                    let report =
                        crate::candidate_ingestion::untrusted_source_reference_report(&rejection);
                    sqlx::query("UPDATE candidates SET status='unknown' WHERE candidate_id=? AND status='submitted'")
                        .bind(&candidate_id).execute(&mut *tx).await?;
                    sqlx::query("UPDATE verifications SET status='unknown',report_json=?,started_at=COALESCE(started_at,?),completed_at=? WHERE verification_id=? AND status='submitted'")
                        .bind(json_text(&report)?).bind(&now).bind(&now).bind(&verification_id).execute(&mut *tx).await?;
                    sqlx::query("UPDATE verification_cases SET stage='unknown',cancellation_epoch=cancellation_epoch+1,achieved_acceptance=NULL,completed_at=?,updated_at=? WHERE verification_id=? AND stage NOT IN ('committed','rejected','unknown','failed','cancelled')")
                        .bind(&now).bind(&now).bind(&verification_id).execute(&mut *tx).await?;
                    sqlx::query("UPDATE verification_task_leases SET status='cancelled',completed_at=? WHERE case_id IN (SELECT case_id FROM verification_cases WHERE verification_id=?) AND status='active'")
                        .bind(&now).bind(&verification_id).execute(&mut *tx).await?;
                    sqlx::query("UPDATE verification_attempts SET status='failed',error_kind=?,error_message=?,completed_at=? WHERE case_id IN (SELECT case_id FROM verification_cases WHERE verification_id=?) AND status NOT IN ('completed','failed','orphaned','cancelled')")
                        .bind(error_kind).bind(&rejection.summary).bind(&now).bind(&verification_id).execute(&mut *tx).await?;
                    sqlx::query("UPDATE verification_result_envelopes SET status='stale',rejection_reason=? WHERE case_id IN (SELECT case_id FROM verification_cases WHERE verification_id=?) AND status='submitted'")
                        .bind(&rejection.summary).bind(&verification_id).execute(&mut *tx).await?;
                    sqlx::query("INSERT INTO failures(failure_id,project_id,route_id,task_id,failure_type,summary,repairable,raw_json,created_at) VALUES(?,?,?,?,?,?,?,?,?)")
                        .bind(&failure_id).bind(&repair_project_id).bind(&submission.route_id).bind(&submission.task_id)
                        .bind(failure_type).bind(&rejection.summary).bind(true)
                        .bind(json_text(&json!({"candidate_id":candidate_id,"verification_id":verification_id,"submission":submission,"source_reference_audit":rejection.audit,"recovered_by":"reconciliation"}))?)
                        .bind(&now).execute(&mut *tx).await?;
                    repairs.push(json!({"kind":repair_kind,"candidate_id":candidate_id,"verification_id":verification_id,"failure_id":failure_id,"source_reference_audit":rejection.audit}));
                    repair_events.entry(repair_project_id).or_default().push((
                        event_type.into(),
                        entity("candidate", &candidate_id),
                        json!({"verification_id":verification_id,"failure_id":failure_id,"source_reference_audit":rejection.audit,"recovered_by":"reconciliation"}),
                    ));
                }
            }
        }
        for issue in integrity_issues {
            sqlx::query("UPDATE projects SET status='needs_human_review',updated_at=? WHERE project_id=? AND status NOT IN ('success','partial_success','environment_failed','refuted','stopped_by_human','error')")
                .bind(&now).bind(&issue.project_id).execute(&mut *tx).await?;
            repairs.push(json!({"kind":"artifact_integrity_error","artifact_id":issue.artifact_id,"reason":issue.reason}));
            repair_events.entry(issue.project_id).or_default().push((
                "artifact.integrity_error".into(),
                entity("artifact", &issue.artifact_id),
                json!({"reason":issue.reason}),
            ));
        }
        let duplicate_attempts = if project_id.is_some() {
            sqlx::query("WITH ranked AS (SELECT a.project_id,a.task_id,a.attempt_id,ROW_NUMBER() OVER (PARTITION BY a.task_id ORDER BY CASE WHEN EXISTS (SELECT 1 FROM result_envelopes e WHERE e.attempt_id=a.attempt_id AND e.status='submitted') THEN 1 ELSE 0 END DESC,a.lease_epoch DESC,a.attempt_no DESC,a.attempt_id DESC) AS attempt_rank FROM task_attempts a WHERE a.project_id=? AND a.status IN ('offered','leased','running','checkpointed','result_submitted','ingesting')) SELECT project_id,task_id,attempt_id FROM ranked WHERE attempt_rank>1 AND NOT EXISTS (SELECT 1 FROM result_envelopes e WHERE e.attempt_id=ranked.attempt_id AND e.status='submitted')")
                .bind(scope).fetch_all(&mut *tx).await?
        } else {
            sqlx::query("WITH ranked AS (SELECT a.project_id,a.task_id,a.attempt_id,ROW_NUMBER() OVER (PARTITION BY a.task_id ORDER BY CASE WHEN EXISTS (SELECT 1 FROM result_envelopes e WHERE e.attempt_id=a.attempt_id AND e.status='submitted') THEN 1 ELSE 0 END DESC,a.lease_epoch DESC,a.attempt_no DESC,a.attempt_id DESC) AS attempt_rank FROM task_attempts a WHERE a.status IN ('offered','leased','running','checkpointed','result_submitted','ingesting')) SELECT project_id,task_id,attempt_id FROM ranked WHERE attempt_rank>1 AND NOT EXISTS (SELECT 1 FROM result_envelopes e WHERE e.attempt_id=ranked.attempt_id AND e.status='submitted')")
                .fetch_all(&mut *tx).await?
        };
        for row in duplicate_attempts {
            let repair_project_id: String = row.try_get("project_id")?;
            let task_id: String = row.try_get("task_id")?;
            let attempt_id: String = row.try_get("attempt_id")?;
            sqlx::query("UPDATE task_attempts SET status='orphaned',failure_reason='superseded duplicate active attempt during reconciliation',completed_at=? WHERE attempt_id=?")
                .bind(&now).bind(&attempt_id).execute(&mut *tx).await?;
            sqlx::query("UPDATE task_leases SET status='expired',completed_at=? WHERE attempt_id=? AND status='active'")
                .bind(&now).bind(&attempt_id).execute(&mut *tx).await?;
            repairs.push(json!({"kind":"duplicate_active_attempt_orphaned","task_id":task_id,"attempt_id":attempt_id}));
            repair_events.entry(repair_project_id).or_default().push((
                "task.attempt_orphaned".into(),
                entity("task_attempt", &attempt_id),
                json!({"task_id":task_id,"reason":"duplicate_active_attempt"}),
            ));
        }
        let completed_without_terminal_event = if project_id.is_some() {
            sqlx::query("SELECT t.project_id,t.task_id FROM tasks t WHERE t.project_id=? AND t.status='completed' AND NOT EXISTS (SELECT 1 FROM events e WHERE e.project_id=t.project_id AND e.type='task.result_ingested' AND json_extract(e.data_json,'$.task_id')=t.task_id)")
                .bind(scope).fetch_all(&mut *tx).await?
        } else {
            sqlx::query("SELECT t.project_id,t.task_id FROM tasks t WHERE t.status='completed' AND NOT EXISTS (SELECT 1 FROM events e WHERE e.project_id=t.project_id AND e.type='task.result_ingested' AND json_extract(e.data_json,'$.task_id')=t.task_id)")
                .fetch_all(&mut *tx).await?
        };
        for row in completed_without_terminal_event {
            let repair_project_id: String = row.try_get("project_id")?;
            let task_id: String = row.try_get("task_id")?;
            repairs.push(json!({"kind":"missing_terminal_event_repaired","task_id":task_id}));
            repair_events.entry(repair_project_id).or_default().push((
                "task.result_ingested".into(),
                entity("task", &task_id),
                json!({"task_id":task_id,"recovered_by":"reconciliation"}),
            ));
        }
        let replayable = if project_id.is_some() {
            sqlx::query("SELECT result_envelope_id FROM result_envelopes WHERE project_id=? AND status='submitted'")
                .bind(scope).fetch_all(&mut *tx).await?
        } else {
            sqlx::query("SELECT result_envelope_id FROM result_envelopes WHERE status='submitted'")
                .fetch_all(&mut *tx)
                .await?
        };
        repairs.extend(
            replayable
                .iter()
                .filter_map(|row| row.try_get::<String, _>("result_envelope_id").ok())
                .map(|id| json!({"kind":"result_ingestion_pending","result_envelope_id":id})),
        );
        let replayable_verification = if project_id.is_some() {
            sqlx::query("SELECT verification_result_envelope_id FROM verification_result_envelopes WHERE project_id=? AND status='submitted'")
                .bind(scope).fetch_all(&mut *tx).await?
        } else {
            sqlx::query("SELECT verification_result_envelope_id FROM verification_result_envelopes WHERE status='submitted'")
                .fetch_all(&mut *tx).await?
        };
        repairs.extend(replayable_verification.iter().filter_map(|row| row.try_get::<String,_>("verification_result_envelope_id").ok())
            .map(|id| json!({"kind":"verification_result_ingestion_pending","verification_result_envelope_id":id})));
        let inactive_routes_without_tasks = if project_id.is_some() {
            sqlx::query("SELECT route_id FROM routes r WHERE project_id=? AND status IN ('incubating','active','probation','revived') AND NOT EXISTS (SELECT 1 FROM tasks t WHERE t.route_id=r.route_id AND t.status IN ('queued','offered','leased','running','checkpointed','result_submitted','ingesting'))")
                .bind(scope).fetch_all(&mut *tx).await?
        } else {
            sqlx::query("SELECT route_id FROM routes r WHERE status IN ('incubating','active','probation','revived') AND NOT EXISTS (SELECT 1 FROM tasks t WHERE t.route_id=r.route_id AND t.status IN ('queued','offered','leased','running','checkpointed','result_submitted','ingesting'))")
                .fetch_all(&mut *tx).await?
        };
        repairs.extend(
            inactive_routes_without_tasks
                .iter()
                .filter_map(|row| row.try_get::<String, _>("route_id").ok())
                .map(|id| json!({"kind":"route_requires_planner_reconcile","route_id":id})),
        );
        let completed_at = Utc::now();
        let integrity_errors = integrity_scan.issue_values();
        let artifact_integrity_audit = integrity_scan.summary();
        sqlx::query("UPDATE reconciliation_runs SET status='completed',repairs_json=?,integrity_errors_json=?,completed_at=? WHERE reconciliation_run_id=?")
            .bind(json_text(&repairs)?)
            .bind(json_text(&integrity_errors)?)
            .bind(completed_at.to_rfc3339())
            .bind(&run_id)
            .execute(&mut *tx)
            .await?;
        if let Some(project_id) = project_id {
            repair_events.entry(project_id.into()).or_default().push((
                "storage.reconciliation.completed".into(),
                entity("reconciliation_run", &run_id),
                json!({"repair_count":repairs.len(),"artifact_integrity_audit":artifact_integrity_audit.clone()}),
            ));
        }
        for (repair_project_id, events) in repair_events {
            let revision = bump_revision(&mut tx, &repair_project_id).await?;
            for (event_type, event_entity, data) in events {
                append_event(
                    &mut tx,
                    &repair_project_id,
                    revision,
                    &event_type,
                    event_entity,
                    data,
                    Some(entity("reconciliation_run", &run_id)),
                )
                .await?;
            }
        }
        tx.commit().await?;
        Ok(json!({
            "reconciliation_run_id":run_id,
            "status":"completed",
            "repairs":repairs,
            "artifact_integrity_audit":artifact_integrity_audit,
            "started_at":started_at,
            "completed_at":completed_at,
        }))
    }
}

impl ArtifactIntegrityScan {
    fn skipped(project_id: Option<&str>) -> Self {
        let now = Utc::now();
        Self {
            mode: "skipped",
            scope_key: artifact_audit_scope_key(project_id),
            project_id: project_id.map(ToOwned::to_owned),
            batch_limit: None,
            cursor_start: None,
            cursor_end: None,
            completed_cycle: false,
            priority_unverified_checked: 0,
            started_at: now,
            completed_at: now,
            observations: Vec::new(),
        }
    }

    fn issues(&self) -> Vec<ArtifactIntegrityIssue> {
        self.observations
            .iter()
            .filter_map(|observation| {
                observation
                    .issue
                    .as_ref()
                    .map(|reason| ArtifactIntegrityIssue {
                        project_id: observation.target.project_id.clone(),
                        artifact_id: observation.target.artifact_id.clone(),
                        reason: reason.clone(),
                    })
            })
            .collect()
    }

    fn issue_values(&self) -> Vec<Value> {
        self.observations
            .iter()
            .filter_map(|observation| {
                observation.issue.as_ref().map(|reason| {
                    json!({
                        "project_id":observation.target.project_id,
                        "artifact_id":observation.target.artifact_id,
                        "reason":reason,
                    })
                })
            })
            .collect()
    }

    fn summary(&self) -> Value {
        let issue_count = self
            .observations
            .iter()
            .filter(|observation| observation.issue.is_some())
            .count();
        let priority_queue_saturated = self.batch_limit.is_some_and(|limit| {
            self.priority_unverified_checked >= usize::try_from(limit).unwrap_or(usize::MAX)
        });
        json!({
            "mode":self.mode,
            "scope_key":self.scope_key,
            "project_id":self.project_id,
            "batch_limit":self.batch_limit,
            "checked_count":self.observations.len(),
            "priority_unverified_checked":self.priority_unverified_checked,
            "priority_queue_saturated":priority_queue_saturated,
            "issue_count":issue_count,
            "cursor_start":self.cursor_start,
            "cursor_end":self.cursor_end,
            "completed_cycle":self.completed_cycle,
            "started_at":self.started_at,
            "completed_at":self.completed_at,
            "trust_semantics":"only status=verified with a matching current artifact hash is verified; unscanned artifacts remain unverified",
        })
    }
}

async fn scan_artifact_integrity_incremental(
    store: &SqliteStore,
    project_id: Option<&str>,
    batch_limit: i64,
) -> StorageResult<ArtifactIntegrityScan> {
    if batch_limit < 1 {
        return Err(StorageError::InvalidTransition(
            "artifact integrity audit batch must be positive".into(),
        ));
    }
    let started_at = Utc::now();
    let scope_key = artifact_audit_scope_key(project_id);
    let cursor_start: Option<String> = sqlx::query_scalar(
        "SELECT last_artifact_id FROM artifact_integrity_audit_cursors WHERE scope_key=?",
    )
    .bind(&scope_key)
    .fetch_optional(store.read_pool())
    .await?
    .flatten();

    // Priority work is bounded too, but it is selected first.  Never-checked rows
    // precede prior failures; failed rows then rotate by oldest check time so one
    // permanently missing file cannot starve the rest of the backlog.
    let priority_rows = if let Some(project_id) = project_id {
        sqlx::query("SELECT a.project_id,a.artifact_id,a.storage_path,a.sha256 FROM artifacts a LEFT JOIN artifact_integrity_audit_state s ON s.artifact_id=a.artifact_id WHERE a.project_id=? AND (s.artifact_id IS NULL OR s.status<>'verified' OR s.last_error IS NOT NULL OR s.last_verified_sha256 IS NULL OR lower(s.last_verified_sha256)<>lower(a.sha256)) ORDER BY CASE WHEN s.last_checked_at IS NULL THEN 0 ELSE 1 END,COALESCE(s.last_checked_at,''),a.artifact_id LIMIT ?")
            .bind(project_id)
            .bind(batch_limit)
            .fetch_all(store.read_pool())
            .await?
    } else {
        sqlx::query("SELECT a.project_id,a.artifact_id,a.storage_path,a.sha256 FROM artifacts a LEFT JOIN artifact_integrity_audit_state s ON s.artifact_id=a.artifact_id WHERE s.artifact_id IS NULL OR s.status<>'verified' OR s.last_error IS NOT NULL OR s.last_verified_sha256 IS NULL OR lower(s.last_verified_sha256)<>lower(a.sha256) ORDER BY CASE WHEN s.last_checked_at IS NULL THEN 0 ELSE 1 END,COALESCE(s.last_checked_at,''),a.artifact_id LIMIT ?")
            .bind(batch_limit)
            .fetch_all(store.read_pool())
            .await?
    };
    let priority_unverified_checked = priority_rows.len();
    let mut targets = artifact_targets(priority_rows)?;
    let remaining = batch_limit.saturating_sub(i64::try_from(targets.len()).unwrap_or(i64::MAX));
    let mut cursor_end = cursor_start.clone();
    let mut completed_cycle = false;

    if remaining > 0 {
        let after_rows =
            select_verified_artifacts(store, project_id, cursor_start.as_deref(), false, remaining)
                .await?;
        let after_count = i64::try_from(after_rows.len()).unwrap_or(i64::MAX);
        let mut verified_targets = artifact_targets(after_rows)?;
        if after_count < remaining {
            // Reaching the tail completes a rotation.  If there was an old cursor,
            // use the remaining capacity from the beginning without duplicating the
            // strictly-after portion selected above.
            completed_cycle = true;
            if cursor_start.is_some() {
                let wrap_rows = select_verified_artifacts(
                    store,
                    project_id,
                    cursor_start.as_deref(),
                    true,
                    remaining - after_count,
                )
                .await?;
                verified_targets.extend(artifact_targets(wrap_rows)?);
            }
        }
        if let Some(last) = verified_targets.last() {
            cursor_end = Some(last.artifact_id.clone());
        }
        targets.extend(verified_targets);
    }

    let observations = inspect_artifact_targets(targets).await;
    Ok(ArtifactIntegrityScan {
        mode: "incremental_startup",
        scope_key,
        project_id: project_id.map(ToOwned::to_owned),
        batch_limit: Some(batch_limit),
        cursor_start,
        cursor_end,
        completed_cycle,
        priority_unverified_checked,
        started_at,
        completed_at: Utc::now(),
        observations,
    })
}

async fn scan_artifact_integrity_full(
    store: &SqliteStore,
    project_id: Option<&str>,
) -> StorageResult<ArtifactIntegrityScan> {
    let started_at = Utc::now();
    let scope_key = artifact_audit_scope_key(project_id);
    let cursor_start: Option<String> = sqlx::query_scalar(
        "SELECT last_artifact_id FROM artifact_integrity_audit_cursors WHERE scope_key=?",
    )
    .bind(&scope_key)
    .fetch_optional(store.read_pool())
    .await?
    .flatten();
    let rows = if let Some(project_id) = project_id {
        sqlx::query("SELECT project_id,artifact_id,storage_path,sha256 FROM artifacts WHERE project_id=? ORDER BY artifact_id")
            .bind(project_id)
            .fetch_all(store.read_pool())
            .await?
    } else {
        sqlx::query(
            "SELECT project_id,artifact_id,storage_path,sha256 FROM artifacts ORDER BY artifact_id",
        )
        .fetch_all(store.read_pool())
        .await?
    };
    let targets = artifact_targets(rows)?;
    let cursor_end = targets.last().map(|target| target.artifact_id.clone());
    let observations = inspect_artifact_targets(targets).await;
    Ok(ArtifactIntegrityScan {
        mode: "full",
        scope_key,
        project_id: project_id.map(ToOwned::to_owned),
        batch_limit: None,
        cursor_start,
        cursor_end,
        completed_cycle: true,
        priority_unverified_checked: 0,
        started_at,
        completed_at: Utc::now(),
        observations,
    })
}

async fn select_verified_artifacts(
    store: &SqliteStore,
    project_id: Option<&str>,
    cursor: Option<&str>,
    wrap: bool,
    limit: i64,
) -> StorageResult<Vec<sqlx::sqlite::SqliteRow>> {
    if limit < 1 {
        return Ok(Vec::new());
    }
    let cursor = cursor.unwrap_or("");
    let rows = match (project_id, wrap) {
        (Some(project_id), false) => sqlx::query("SELECT a.project_id,a.artifact_id,a.storage_path,a.sha256 FROM artifacts a JOIN artifact_integrity_audit_state s ON s.artifact_id=a.artifact_id WHERE a.project_id=? AND s.status='verified' AND s.last_error IS NULL AND lower(s.last_verified_sha256)=lower(a.sha256) AND a.artifact_id>? ORDER BY a.artifact_id LIMIT ?")
            .bind(project_id).bind(cursor).bind(limit).fetch_all(store.read_pool()).await?,
        (Some(project_id), true) => sqlx::query("SELECT a.project_id,a.artifact_id,a.storage_path,a.sha256 FROM artifacts a JOIN artifact_integrity_audit_state s ON s.artifact_id=a.artifact_id WHERE a.project_id=? AND s.status='verified' AND s.last_error IS NULL AND lower(s.last_verified_sha256)=lower(a.sha256) AND a.artifact_id<=? ORDER BY a.artifact_id LIMIT ?")
            .bind(project_id).bind(cursor).bind(limit).fetch_all(store.read_pool()).await?,
        (None, false) => sqlx::query("SELECT a.project_id,a.artifact_id,a.storage_path,a.sha256 FROM artifacts a JOIN artifact_integrity_audit_state s ON s.artifact_id=a.artifact_id WHERE s.status='verified' AND s.last_error IS NULL AND lower(s.last_verified_sha256)=lower(a.sha256) AND a.artifact_id>? ORDER BY a.artifact_id LIMIT ?")
            .bind(cursor).bind(limit).fetch_all(store.read_pool()).await?,
        (None, true) => sqlx::query("SELECT a.project_id,a.artifact_id,a.storage_path,a.sha256 FROM artifacts a JOIN artifact_integrity_audit_state s ON s.artifact_id=a.artifact_id WHERE s.status='verified' AND s.last_error IS NULL AND lower(s.last_verified_sha256)=lower(a.sha256) AND a.artifact_id<=? ORDER BY a.artifact_id LIMIT ?")
            .bind(cursor).bind(limit).fetch_all(store.read_pool()).await?,
    };
    Ok(rows)
}

fn artifact_targets(
    rows: Vec<sqlx::sqlite::SqliteRow>,
) -> StorageResult<Vec<ArtifactIntegrityTarget>> {
    rows.iter()
        .map(|row| {
            Ok(ArtifactIntegrityTarget {
                project_id: row.try_get("project_id")?,
                artifact_id: row.try_get("artifact_id")?,
                storage_path: row.try_get("storage_path")?,
                expected_sha256: row.try_get("sha256")?,
            })
        })
        .collect()
}

async fn inspect_artifact_targets(
    targets: Vec<ArtifactIntegrityTarget>,
) -> Vec<ArtifactIntegrityObservation> {
    let mut observations = Vec::with_capacity(targets.len());
    for target in targets {
        let (observed_sha256, issue) = inspect_artifact_file(&target).await;
        observations.push(ArtifactIntegrityObservation {
            target,
            checked_at: Utc::now(),
            observed_sha256,
            issue,
        });
    }
    observations
}

async fn inspect_artifact_file(
    target: &ArtifactIntegrityTarget,
) -> (Option<String>, Option<String>) {
    let mut file = match tokio::fs::File::open(&target.storage_path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (None, Some("artifact_file_missing".into()));
        }
        Err(error) => return (None, Some(format!("artifact_file_unreadable:{error}"))),
    };
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        match file.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => digest.update(&buffer[..read]),
            Err(error) => {
                return (None, Some(format!("artifact_file_unreadable:{error}")));
            }
        }
    }
    let observed = hex::encode(digest.finalize());
    let issue = (!observed.eq_ignore_ascii_case(&target.expected_sha256)).then(|| {
        format!(
            "sha256_mismatch:expected={}:observed={observed}",
            target.expected_sha256
        )
    });
    (Some(observed), issue)
}

async fn persist_artifact_integrity_scan(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    scan: &ArtifactIntegrityScan,
) -> StorageResult<()> {
    if scan.mode == "skipped" {
        return Ok(());
    }
    for observation in &scan.observations {
        let verified = observation.issue.is_none();
        let status = if verified { "verified" } else { "failed" };
        let verified_sha256 = if verified {
            observation.observed_sha256.as_deref()
        } else {
            None
        };
        let verified_at = verified.then(|| observation.checked_at.to_rfc3339());
        let checked_at = observation.checked_at.to_rfc3339();
        sqlx::query("INSERT INTO artifact_integrity_audit_state(artifact_id,project_id,status,last_checked_sha256,last_verified_sha256,last_checked_at,last_verified_at,last_error,check_count,updated_at) VALUES(?,?,?,?,?,?,?,?,1,?) ON CONFLICT(artifact_id) DO UPDATE SET project_id=excluded.project_id,status=excluded.status,last_checked_sha256=excluded.last_checked_sha256,last_verified_sha256=CASE WHEN excluded.status='verified' THEN excluded.last_verified_sha256 ELSE artifact_integrity_audit_state.last_verified_sha256 END,last_checked_at=excluded.last_checked_at,last_verified_at=CASE WHEN excluded.status='verified' THEN excluded.last_verified_at ELSE artifact_integrity_audit_state.last_verified_at END,last_error=excluded.last_error,check_count=artifact_integrity_audit_state.check_count+1,updated_at=excluded.updated_at")
            .bind(&observation.target.artifact_id)
            .bind(&observation.target.project_id)
            .bind(status)
            .bind(observation.observed_sha256.as_deref())
            .bind(verified_sha256)
            .bind(&checked_at)
            .bind(verified_at)
            .bind(observation.issue.as_deref())
            .bind(&checked_at)
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("INSERT INTO artifact_integrity_audit_cursors(scope_key,project_id,last_artifact_id,completed_cycles,last_batch_checked_count,last_batch_started_at,last_batch_completed_at,updated_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(scope_key) DO UPDATE SET project_id=excluded.project_id,last_artifact_id=COALESCE(excluded.last_artifact_id,artifact_integrity_audit_cursors.last_artifact_id),completed_cycles=artifact_integrity_audit_cursors.completed_cycles+excluded.completed_cycles,last_batch_checked_count=excluded.last_batch_checked_count,last_batch_started_at=excluded.last_batch_started_at,last_batch_completed_at=excluded.last_batch_completed_at,updated_at=excluded.updated_at")
        .bind(&scan.scope_key)
        .bind(scan.project_id.as_deref())
        .bind(scan.cursor_end.as_deref())
        .bind(i64::from(scan.completed_cycle))
        .bind(i64::try_from(scan.observations.len()).unwrap_or(i64::MAX))
        .bind(scan.started_at.to_rfc3339())
        .bind(scan.completed_at.to_rfc3339())
        .bind(scan.completed_at.to_rfc3339())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn artifact_audit_scope_key(project_id: Option<&str>) -> String {
    project_id.map_or_else(|| "all".into(), |id| format!("project:{id}"))
}

fn research_delta_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<ResearchDelta> {
    delta_from_payload(
        row.try_get("delta_id")?,
        row.try_get::<String, _>("project_id")?,
        row.try_get("from_revision")?,
        row.try_get("to_revision")?,
        serde_json::from_str(row.try_get("payload_json")?)?,
        row.try_get("status")?,
        row.try_get("consumed_by_plan_revision_id")?,
        timestamp(row.try_get("created_at")?)?,
        optional_timestamp(row.try_get("consumed_at")?)?,
    )
}

#[allow(clippy::too_many_arguments)]
fn delta_from_payload(
    delta_id: String,
    project_id: impl Into<String>,
    from_revision: i64,
    to_revision: i64,
    payload: Value,
    status: String,
    consumed_by_plan_revision_id: Option<String>,
    created_at: DateTime<Utc>,
    consumed_at: Option<DateTime<Utc>>,
) -> StorageResult<ResearchDelta> {
    let strings = |key: &str| -> StorageResult<Vec<String>> {
        serde_json::from_value(payload.get(key).cloned().unwrap_or_else(|| json!([])))
            .map_err(StorageError::from)
    };
    Ok(ResearchDelta {
        delta_id,
        project_id: project_id.into(),
        from_revision,
        to_revision,
        accepted_fact_ids: strings("accepted_fact_ids")?,
        rejected_candidate_ids: strings("rejected_candidate_ids")?,
        new_proof_debt_ids: strings("new_proof_debt_ids")?,
        solved_goal_ids: strings("solved_goal_ids")?,
        reopened_goal_ids: strings("reopened_goal_ids")?,
        changed_uncertainty_ids: strings("changed_uncertainty_ids")?,
        new_failure_pattern_ids: strings("new_failure_pattern_ids")?,
        changed_source_ids: strings("changed_source_ids")?,
        completed_task_attempt_ids: strings("completed_task_attempt_ids")?,
        failed_or_expired_attempt_ids: strings("failed_or_expired_attempt_ids")?,
        human_command_ids: strings("human_command_ids")?,
        human_commands: serde_json::from_value(
            payload
                .get("human_commands")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )?,
        route_state_changes: serde_json::from_value(
            payload
                .get("route_state_changes")
                .cloned()
                .unwrap_or_else(|| json!([])),
        )?,
        status,
        consumed_by_plan_revision_id,
        created_at,
        consumed_at,
    })
}

fn plan_revision_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<PlanRevisionRecord> {
    Ok(PlanRevisionRecord {
        plan_revision_id: row.try_get("plan_revision_id")?,
        project_id: row.try_get("project_id")?,
        ordinal: row.try_get("ordinal")?,
        round_id: row.try_get("round_id")?,
        based_on_project_revision: row.try_get("based_on_project_revision")?,
        consumed_delta_from: row.try_get("consumed_delta_from")?,
        consumed_delta_to: row.try_get("consumed_delta_to")?,
        planner_mode: row.try_get("planner_mode")?,
        context_packet_id: row.try_get("context_packet_id")?,
        route_decisions: serde_json::from_str(row.try_get("route_decisions_json")?)?,
        bottleneck_updates: serde_json::from_str(row.try_get("bottleneck_updates_json")?)?,
        task_contract_ids: serde_json::from_str(row.try_get("task_contract_ids_json")?)?,
        status: row.try_get("status")?,
        rationale: row.try_get("rationale")?,
        created_at: timestamp(row.try_get("created_at")?)?,
        committed_at: optional_timestamp(row.try_get("committed_at")?)?,
    })
}

fn bottleneck_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<Bottleneck> {
    Ok(Bottleneck {
        bottleneck_id: row.try_get("bottleneck_id")?,
        project_id: row.try_get("project_id")?,
        target_goal_ids: serde_json::from_str(row.try_get("target_goal_ids_json")?)?,
        kind: row.try_get("kind")?,
        precise_statement: row.try_get("precise_statement")?,
        completion_contract: serde_json::from_str(row.try_get("completion_contract_json")?)?,
        evidence_ids: serde_json::from_str(row.try_get("evidence_ids_json")?)?,
        blocked_route_ids: serde_json::from_str(row.try_get("blocked_route_ids_json")?)?,
        attempted_task_ids: serde_json::from_str(row.try_get("attempted_task_ids_json")?)?,
        failure_pattern_ids: serde_json::from_str(row.try_get("failure_pattern_ids_json")?)?,
        repair_action_ids: serde_json::from_str(row.try_get("repair_action_ids_json")?)?,
        priority: row.try_get("priority")?,
        status: row.try_get("status")?,
        reopen_condition: row.try_get("reopen_condition")?,
        created_revision: row.try_get("created_revision")?,
        updated_revision: row.try_get("updated_revision")?,
        created_at: timestamp(row.try_get("created_at")?)?,
        updated_at: timestamp(row.try_get("updated_at")?)?,
    })
}

fn fact_impact_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<FactImpactRecord> {
    Ok(FactImpactRecord {
        impact_id: row.try_get("impact_id")?,
        project_id: row.try_get("project_id")?,
        fact_id: row.try_get("fact_id")?,
        source_revision: row.try_get("source_revision")?,
        closed_goal_ids: serde_json::from_str(row.try_get("closed_goal_ids_json")?)?,
        unblocked_route_ids: serde_json::from_str(row.try_get("unblocked_route_ids_json")?)?,
        invalidated_task_ids: serde_json::from_str(row.try_get("invalidated_task_ids_json")?)?,
        newly_enabled_task_templates: serde_json::from_str(
            row.try_get("newly_enabled_task_templates_json")?,
        )?,
        dominated_route_ids: serde_json::from_str(row.try_get("dominated_route_ids_json")?)?,
        resolved_uncertainty_ids: serde_json::from_str(
            row.try_get("resolved_uncertainty_ids_json")?,
        )?,
        planner_disposition: row.try_get("planner_disposition")?,
        disposition_reason: row.try_get("disposition_reason")?,
        created_at: timestamp(row.try_get("created_at")?)?,
        applied_at: optional_timestamp(row.try_get("applied_at")?)?,
    })
}

fn task_contract_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<TaskContract> {
    let mut contract: TaskContract = serde_json::from_str(row.try_get("contract_json")?)?;
    contract.task_contract_id = row.try_get("task_contract_id")?;
    contract.project_id = row.try_get("project_id")?;
    contract.task_id = row.try_get("task_id")?;
    contract.plan_revision_id = row.try_get("plan_revision_id")?;
    contract.contract_version = row.try_get("contract_version")?;
    contract.content_hash = row.try_get("content_hash")?;
    contract.created_at = timestamp(row.try_get("created_at")?)?;
    Ok(contract)
}

fn context_packet_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<ContextPacket> {
    Ok(ContextPacket {
        context_packet_id: row.try_get("context_packet_id")?,
        project_id: row.try_get("project_id")?,
        task_id: row.try_get("task_id")?,
        route_id: row.try_get("route_id")?,
        packet_kind: row.try_get("packet_kind")?,
        source_revision: row.try_get("source_revision")?,
        problem_contract_excerpt: row.try_get("problem_contract_excerpt")?,
        objective: row.try_get("objective")?,
        known_fact_ids: serde_json::from_str(row.try_get("known_fact_ids_json")?)?,
        bottleneck_id: row.try_get("bottleneck_id")?,
        route_progress: serde_json::from_str(row.try_get("route_progress_json")?)?,
        relevant_failure_pattern_ids: serde_json::from_str(
            row.try_get("relevant_failure_pattern_ids_json")?,
        )?,
        relevant_uncertainty_ids: serde_json::from_str(
            row.try_get("relevant_uncertainty_ids_json")?,
        )?,
        source_refs: serde_json::from_str(row.try_get("source_refs_json")?)?,
        omitted_sections: serde_json::from_str(row.try_get("omitted_sections_json")?)?,
        token_estimate: row.try_get("token_estimate")?,
        content: serde_json::from_str(row.try_get("content_json")?)?,
        content_hash: row.try_get("content_hash")?,
        status: row.try_get("status")?,
        invalidation_reason: row.try_get("invalidation_reason")?,
        created_at: timestamp(row.try_get("created_at")?)?,
    })
}

fn task_attempt_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<TaskAttempt> {
    Ok(TaskAttempt {
        attempt_id: row.try_get("attempt_id")?,
        project_id: row.try_get("project_id")?,
        task_id: row.try_get("task_id")?,
        worker_instance_id: row.try_get("worker_instance_id")?,
        attempt_no: row.try_get("attempt_no")?,
        status: row.try_get("status")?,
        lease_epoch: row.try_get("lease_epoch")?,
        plan_revision_id: row.try_get("plan_revision_id")?,
        route_cancellation_epoch: row.try_get("route_cancellation_epoch")?,
        context_packet_id: row.try_get("context_packet_id")?,
        failure_signature: row.try_get("failure_signature")?,
        failure_reason: row.try_get("failure_reason")?,
        started_at: optional_timestamp(row.try_get("started_at")?)?,
        completed_at: optional_timestamp(row.try_get("completed_at")?)?,
        created_at: timestamp(row.try_get("created_at")?)?,
    })
}

fn worker_instance_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<WorkerInstance> {
    Ok(WorkerInstance {
        worker_instance_id: row.try_get("worker_instance_id")?,
        project_id: row.try_get("project_id")?,
        worker_id: row.try_get("worker_id")?,
        backend: row.try_get("backend")?,
        backend_version: row.try_get("backend_version")?,
        model: row.try_get("model")?,
        status: row.try_get("status")?,
        capabilities: serde_json::from_str(row.try_get("capability_json")?)?,
        working_directory: row.try_get("working_directory")?,
        handshake: row
            .try_get::<Option<String>, _>("handshake_json")?
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        quarantine_reason: row.try_get("quarantine_reason")?,
        started_at: timestamp(row.try_get("started_at")?)?,
        ready_at: optional_timestamp(row.try_get("ready_at")?)?,
        last_heartbeat_at: optional_timestamp(row.try_get("last_heartbeat_at")?)?,
        exited_at: optional_timestamp(row.try_get("exited_at")?)?,
    })
}

fn planner_health_from_row(row: &sqlx::sqlite::SqliteRow) -> StorageResult<PlannerHealth> {
    Ok(PlannerHealth {
        project_id: row.try_get("project_id")?,
        circuit_state: row.try_get("circuit_state")?,
        consecutive_failures: row.try_get("consecutive_failures")?,
        last_failure_reason: row.try_get("last_failure_reason")?,
        opened_at: optional_timestamp(row.try_get("opened_at")?)?,
        cooldown_until: optional_timestamp(row.try_get("cooldown_until")?)?,
        last_probe_at: optional_timestamp(row.try_get("last_probe_at")?)?,
        updated_at: timestamp(row.try_get("updated_at")?)?,
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

async fn validate_local_lease(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    lease: &LocalTaskLease,
    allowed_task_statuses: &[&str],
) -> StorageResult<()> {
    let row = sqlx::query(
        "SELECT l.status AS lease_status,l.expires_at,l.lease_token_hash,\
                t.status AS task_status,t.revision,t.route_cancellation_epoch,\
                a.status AS attempt_status,a.lease_epoch AS attempt_lease_epoch,\
                r.cancellation_epoch AS route_epoch,r.status AS route_status \
         FROM task_leases l \
         JOIN tasks t ON t.task_id=l.task_id \
         JOIN task_attempts a ON a.attempt_id=l.attempt_id \
         JOIN routes r ON r.route_id=t.route_id \
         WHERE l.lease_id=? AND l.task_id=? AND l.attempt_id=? \
           AND l.worker_instance_id=? AND l.lease_epoch=?",
    )
    .bind(&lease.lease_id)
    .bind(&lease.task.task_id)
    .bind(&lease.attempt_id)
    .bind(&lease.worker_instance_id)
    .bind(lease.lease_epoch)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| StorageError::LateSubmission("local lease no longer exists".into()))?;

    let lease_status: String = row.try_get("lease_status")?;
    let lease_token_hash: Option<String> = row.try_get("lease_token_hash")?;
    let expires_at = timestamp(row.try_get("expires_at")?)?;
    let task_status: String = row.try_get("task_status")?;
    let task_revision: i64 = row.try_get("revision")?;
    let task_route_epoch: i64 = row.try_get("route_cancellation_epoch")?;
    let attempt_status: String = row.try_get("attempt_status")?;
    let attempt_lease_epoch: i64 = row.try_get("attempt_lease_epoch")?;
    let route_epoch: i64 = row.try_get("route_epoch")?;
    let route_status: String = row.try_get("route_status")?;
    let attempt_status_allowed = matches!(attempt_status.as_str(), "running" | "checkpointed");

    if lease_status != "active"
        || expires_at < Utc::now()
        || lease_token_hash.as_deref() != Some(hash_secret(&lease.lease_token).as_str())
        || !allowed_task_statuses.contains(&task_status.as_str())
        || !attempt_status_allowed
        || attempt_lease_epoch != lease.lease_epoch
        || task_revision != lease.task.revision
        || task_route_epoch != lease.task.route_cancellation_epoch
        || route_epoch != lease.task.route_cancellation_epoch
        || matches!(route_status.as_str(), "human_stopped" | "merged" | "pruned")
    {
        return Err(StorageError::LateSubmission(
            "lease token, epoch, task revision, route epoch, or lifecycle state is stale".into(),
        ));
    }
    Ok(())
}

async fn result_envelope_verifications_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
    result_envelope_id: &str,
) -> StorageResult<Vec<Verification>> {
    let idempotency_prefix = format!("worker-result-{result_envelope_id}-%");
    let rows = sqlx::query("SELECT v.* FROM verifications v JOIN candidates c ON c.candidate_id=v.candidate_id WHERE c.project_id=? AND c.idempotency_key LIKE ? ORDER BY c.created_at,c.candidate_id")
        .bind(project_id)
        .bind(idempotency_prefix)
        .fetch_all(&mut **tx)
        .await?;
    rows.iter().map(rows::verification).collect()
}

fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

pub(crate) fn sha256_json(value: &Value) -> StorageResult<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}

pub(crate) fn worker_result_envelope_identity(
    attempt_id: &str,
    output: &WorkerOutput,
) -> StorageResult<(Value, String, String)> {
    let payload = serde_json::to_value(output)?;
    let content_hash = sha256_json(&payload)?;
    let idempotency_key = format!("result:{attempt_id}:{content_hash}");
    Ok((payload, content_hash, idempotency_key))
}

async fn accept_matching_human_proposals(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project_id: &str,
    proposal: &RouteProposal,
    route_id: &str,
    default_goal_id: &str,
    now: &DateTime<Utc>,
) -> StorageResult<Vec<(String, String)>> {
    let proposal_goals = if proposal.target_goal_ids.is_empty() {
        vec![default_goal_id.to_owned()]
    } else {
        proposal.target_goal_ids.clone()
    };
    let expected_fingerprint = route_fingerprint(
        &proposal_goals,
        &proposal.method_summary,
        &proposal.required_fact_ids,
    )?;
    let rows = sqlx::query("SELECT proposal_id,target_goal_ids_json,required_fact_ids_json,method_summary FROM human_route_proposals WHERE project_id=? AND status='queued'")
        .bind(project_id)
        .fetch_all(&mut **tx)
        .await?;
    let mut proposal_ids = Vec::new();
    for row in rows {
        let mut goals: Vec<String> = serde_json::from_str(row.try_get("target_goal_ids_json")?)?;
        if goals.is_empty() {
            goals.push(default_goal_id.to_owned());
        }
        let required_fact_ids: Vec<String> =
            serde_json::from_str(row.try_get("required_fact_ids_json")?)?;
        let method_summary: String = row.try_get("method_summary")?;
        if route_fingerprint(&goals, &method_summary, &required_fact_ids)? == expected_fingerprint {
            proposal_ids.push(row.try_get::<String, _>("proposal_id")?);
        }
    }
    for proposal_id in &proposal_ids {
        sqlx::query("UPDATE human_route_proposals SET status='accepted',route_id=?,decision_reason='accepted by Planner after dependency, capacity, and duplicate checks',decided_at=? WHERE proposal_id=? AND status='queued'")
            .bind(route_id)
            .bind(now.to_rfc3339())
            .bind(proposal_id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(proposal_ids
        .into_iter()
        .map(|proposal_id| (proposal_id, route_id.to_owned()))
        .collect())
}

pub(crate) fn route_fingerprint(
    target_goal_ids: &[String],
    method_summary: &str,
    required_fact_ids: &[String],
) -> StorageResult<String> {
    let mut goals = target_goal_ids.to_vec();
    goals.sort();
    goals.dedup();
    let mut facts = required_fact_ids.to_vec();
    facts.sort();
    facts.dedup();
    sha256_json(&json!({
        "target_goal_ids":goals,
        "method_skeleton":normalize_semantic_text(method_summary),
        "required_fact_ids":facts,
    }))
}

pub(crate) fn route_family_key(
    target_goal_ids: &[String],
    method_summary: &str,
) -> StorageResult<String> {
    let mut goals = target_goal_ids.to_vec();
    goals.sort();
    goals.dedup();
    let method = normalize_semantic_text(method_summary);
    let family_terms = method.split_whitespace().take(16).collect::<Vec<_>>();
    sha256_json(&json!({"target_goal_ids":goals,"family_terms":family_terms}))
}

fn normalize_semantic_text(value: &str) -> String {
    value
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let mut output = value.chars().take(limit).collect::<String>();
    output.push_str("\n[omitted: original problem exceeded task-packet excerpt budget]");
    output
}
