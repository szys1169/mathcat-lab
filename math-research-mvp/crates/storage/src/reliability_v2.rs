use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use research_domain::{
    Bottleneck, ContextPacket, EntityRef, FactImpactRecord, PlanRevisionRecord, PlannerHealth,
    PlannerOutput, ResearchDelta, ResearchRound, Route, RouteProposal, RouteStatus, StorageHealth,
    Task, TaskAttempt, TaskContract, TaskStatus, Worker, WorkerInstance, WorkerOutput,
    WorkerStatus,
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
struct ArtifactIntegrityIssue {
    project_id: String,
    artifact_id: String,
    reason: String,
}
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{
    PlanSaveResult, SqliteStore, StorageError, StorageResult, append_event, bump_revision,
    current_revision, entity, json_text, new_id, rows,
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

    #[allow(clippy::too_many_lines)]
    pub async fn save_plan_v2(
        &self,
        project_id: &str,
        round: &ResearchRound,
        plan: &PlannerOutput,
        delta: &ResearchDelta,
        planner_mode: &str,
        planner_context_packet_id: Option<&str>,
    ) -> StorageResult<PlanSaveResult> {
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
        for proposal in &plan.routes {
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
                sqlx::query("UPDATE routes SET score=?,priority=?,status=CASE WHEN status='probation' THEN 'active' ELSE status END WHERE route_id=?")
                    .bind(proposal.score()).bind(proposal.score()).bind(&route_id).execute(&mut *tx).await?;
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
                    let attributes: BTreeMap<String, Value> =
                        BTreeMap::from([("risks".into(), json!(proposal.risks))]);
                    sqlx::query("INSERT INTO routes(route_id,project_id,title,method_summary,target_goal_ids_json,required_fact_ids_json,status,score,priority,cancellation_epoch,created_in_round,attributes_json,family_id,semantic_fingerprint,created_at_revision,merged_into,merge_reason,exit_criteria_json) VALUES(?,?,?,?,?,?,'merged',?,?,0,?,?,?,?,?,?,?,'[]')")
                        .bind(&merged_route_id).bind(project_id).bind(&proposal.title).bind(&proposal.method_summary)
                        .bind(json_text(&target_goal_ids)?).bind(json_text(&proposal.required_fact_ids)?).bind(proposal.score()).bind(proposal.score())
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
            let score = proposal.score();
            let attributes = BTreeMap::from([("risks".into(), json!(proposal.risks))]);
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
        let mut seen_signatures = BTreeSet::new();
        for assignment in &plan.assignments {
            let Some(Some(route)) = proposal_routes.get(assignment.route_index) else {
                continue;
            };
            if pending_approval_route_ids.contains(&route.route_id) {
                continue;
            }
            let goal_ids = if assignment.goal_ids.is_empty() {
                route.target_goal_ids.clone()
            } else {
                assignment.goal_ids.clone()
            };
            let bottleneck_id = open_bottlenecks.iter().find_map(|row| {
                let ids: Vec<String> =
                    serde_json::from_str(row.try_get("target_goal_ids_json").ok()?).ok()?;
                ids.iter()
                    .any(|id| goal_ids.contains(id))
                    .then(|| row.try_get::<String, _>("bottleneck_id").ok())
                    .flatten()
            });
            let completion = json!({
                "success_outputs":[assignment.completion_contract],
                "partial_outputs":["a strictly narrower named subproblem with an explicit implication or equivalence"],
                "reject_outputs":["generic discussion","a repeated failed proof without new evidence","an unstated strengthening of assumptions"],
            });
            let signature_input = json!({
                "project_id":project_id,
                "goal_ids":goal_ids,
                "bottleneck_id":bottleneck_id,
                "task_kind":assignment.worker_role,
                "required_fact_ids":route.required_fact_ids,
                "completion_contract_version":1,
                "route_cancellation_epoch":route.cancellation_epoch,
            });
            let signature = sha256_json(&signature_input)?;
            if !seen_signatures.insert(signature.clone()) {
                continue;
            }
            let duplicate: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE project_id=? AND task_signature=? AND status IN ('queued','offered','leased','running','checkpointed','result_submitted','ingesting')")
                .bind(project_id).bind(&signature).fetch_one(&mut *tx).await?;
            if duplicate > 0 {
                continue;
            }
            let task_id = new_id("task");
            let worker_id = new_id("worker");
            sqlx::query("INSERT INTO workers(worker_id,project_id,role,backend,status,current_task_id,current_route_id,last_heartbeat) VALUES(?,?,?,?,?,?,?,?)")
                .bind(&worker_id).bind(project_id).bind(&assignment.worker_role).bind("codex_cli")
                .bind(WorkerStatus::Ready.to_string()).bind(&task_id).bind(&route.route_id)
                .bind(Utc::now().to_rfc3339()).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO tasks(task_id,project_id,route_id,worker_id,worker_role,goal_ids_json,objective,completion_contract,status,priority,revision,route_cancellation_epoch,round,plan_revision_id,task_signature) VALUES(?,?,?,?,?,?,?,?,?,?,1,?,?,?,?)")
                .bind(&task_id).bind(project_id).bind(&route.route_id).bind(&worker_id)
                .bind(&assignment.worker_role).bind(json_text(&goal_ids)?).bind(&assignment.objective)
                .bind(&assignment.completion_contract).bind(TaskStatus::Queued.to_string()).bind(assignment.priority)
                .bind(route.cancellation_epoch).bind(round.number).bind(&plan_revision_id).bind(&signature).execute(&mut *tx).await?;

            let packet_id = new_id("context");
            let fact_rows = sqlx::query("SELECT fact_id,statement,assumptions_json,evidence_level FROM facts WHERE project_id=? AND status='active' ORDER BY created_at DESC")
                .bind(project_id).fetch_all(&mut *tx).await?;
            let required_fact_ids = route.required_fact_ids.iter().collect::<BTreeSet<_>>();
            let fact_digests = fact_rows
                .iter()
                .filter_map(|row| {
                    let fact_id = row.try_get::<String, _>("fact_id").ok()?;
                    required_fact_ids.contains(&fact_id).then(|| Ok(json!({
                    "fact_id":fact_id,
                    "statement":row.try_get::<String,_>("statement")?,
                    "assumptions":serde_json::from_str::<Value>(row.try_get("assumptions_json")?)?,
                    "evidence_level":row.try_get::<String,_>("evidence_level")?,
                })))
                })
                .collect::<StorageResult<Vec<_>>>()?;
            let known_fact_ids = fact_digests
                .iter()
                .filter_map(|value| {
                    value
                        .get("fact_id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .collect::<Vec<_>>();
            let failure_rows = sqlx::query("SELECT pattern_id,title,summary FROM failure_patterns WHERE project_id=? ORDER BY created_at DESC LIMIT 8")
                .bind(project_id).fetch_all(&mut *tx).await?;
            let failure_digests = failure_rows.iter().map(|row| Ok(json!({"pattern_id":row.try_get::<String,_>("pattern_id")?,"title":row.try_get::<String,_>("title")?,"summary":row.try_get::<String,_>("summary")?}))).collect::<StorageResult<Vec<_>>>()?;
            let failure_ids = failure_digests
                .iter()
                .filter_map(|value| {
                    value
                        .get("pattern_id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .collect::<Vec<_>>();
            let uncertainty_rows = sqlx::query("SELECT uncertainty_id,description,severity FROM uncertainties WHERE project_id=? AND status IN ('open','investigating') ORDER BY CASE severity WHEN 'critical' THEN 0 WHEN 'high' THEN 1 ELSE 2 END,created_at LIMIT 12")
                .bind(project_id).fetch_all(&mut *tx).await?;
            let uncertainty_digests = uncertainty_rows.iter().map(|row| Ok(json!({"uncertainty_id":row.try_get::<String,_>("uncertainty_id")?,"description":row.try_get::<String,_>("description")?,"severity":row.try_get::<String,_>("severity")?}))).collect::<StorageResult<Vec<_>>>()?;
            let uncertainty_ids = uncertainty_digests
                .iter()
                .filter_map(|value| {
                    value
                        .get("uncertainty_id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .collect::<Vec<_>>();
            let problem_excerpt = truncate_chars(
                &format!(
                    "Target: {}\nAssumptions: {}\nOriginal problem:\n{}",
                    contract.target_statement,
                    contract.assumptions.join("; "),
                    contract.original_problem
                ),
                8_000,
            );
            let packet_content = json!({
                "problem_contract":{"version":contract.version,"excerpt":problem_excerpt},
                "task":{"task_id":task_id,"objective":assignment.objective,"goal_ids":goal_ids,"completion_contract":completion},
                "route":{"route_id":route.route_id,"method_summary":route.method_summary,"required_fact_ids":route.required_fact_ids,"exit_criteria":route.exit_criteria},
                "bottleneck_id":bottleneck_id,
                "known_facts":fact_digests,
                "relevant_failure_patterns":failure_digests,
                "relevant_uncertainties":uncertainty_digests,
                "trust_rule":"Only active facts identified by fact_id are mathematical premises. Summaries and route text are not facts.",
            });
            let packet_hash = sha256_json(&packet_content)?;
            let token_estimate = i64::try_from(
                serde_json::to_string(&packet_content)?
                    .chars()
                    .count()
                    .div_ceil(4),
            )
            .unwrap_or(i64::MAX);
            let omitted = vec![
                json!({"category":"raw_artifacts","reason":"available only by explicit artifact retrieval"}),
                json!({"category":"full_project_history","reason":"V2 task packets use a minimal relevant view"}),
            ];
            let mut source_refs = vec![entity("route", &route.route_id)];
            source_refs.extend(known_fact_ids.iter().map(|id| entity("fact", id)));
            source_refs.extend(failure_ids.iter().map(|id| entity("failure_pattern", id)));
            source_refs.extend(uncertainty_ids.iter().map(|id| entity("uncertainty", id)));
            if let Some(bottleneck_id) = &bottleneck_id {
                source_refs.push(entity("bottleneck", bottleneck_id));
            }
            sqlx::query("INSERT INTO context_packets(context_packet_id,project_id,task_id,route_id,packet_kind,source_revision,problem_contract_excerpt,objective,known_fact_ids_json,bottleneck_id,route_progress_json,relevant_failure_pattern_ids_json,relevant_uncertainty_ids_json,source_refs_json,omitted_sections_json,token_estimate,content_json,content_hash,status,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?, ?,?,?, ?,?,?,?,'active',?)")
                .bind(&packet_id).bind(project_id).bind(&task_id).bind(&route.route_id).bind("task")
                .bind(actual).bind(&problem_excerpt).bind(&assignment.objective).bind(json_text(&known_fact_ids)?)
                .bind(&bottleneck_id).bind(json_text(&json!({"last_material_progress_revision":route.last_material_progress_revision}))?)
                .bind(json_text(&failure_ids)?).bind(json_text(&uncertainty_ids)?).bind(json_text(&source_refs)?).bind(json_text(&omitted)?)
                .bind(token_estimate).bind(json_text(&packet_content)?).bind(&packet_hash).bind(now.to_rfc3339()).execute(&mut *tx).await?;
            sqlx::query("UPDATE tasks SET context_packet_id=? WHERE task_id=?")
                .bind(&packet_id)
                .bind(&task_id)
                .execute(&mut *tx)
                .await?;

            let task_contract_id = new_id("taskcontract");
            let mut contract_value = json!({
                "task_contract_id":task_contract_id,
                "project_id":project_id,
                "task_id":task_id,
                "plan_revision_id":plan_revision_id,
                "contract_version":1,
                "route_id":route.route_id,
                "target_goal_ids":goal_ids,
                "bottleneck_id":bottleneck_id,
                "task_kind":assignment.worker_role,
                "precise_objective":assignment.objective,
                "allowed_input_ids":route.required_fact_ids,
                "context_packet_id":packet_id,
                "allowed_tools":["read_workspace","bounded_computation","submit_artifact","submit_candidate"],
                "forbidden_actions":["write_domain_database","write_fact_graph","change_problem_contract","claim_formal_verification_without_kernel_evidence"],
                "completion_contract":completion,
                "budget":{"max_minutes":budget.max_minutes_per_task,"max_model_calls":budget.max_model_calls_per_task},
                "checkpoint_policy":{"required_before_minutes":(budget.max_minutes_per_task/2).max(1),"save_on_cancellation":true},
                "retry_policy":{"max_attempts":3,"same_failure_signature_retries":1},
                "fallback_policy":{"on_schema_error":"one repair then new session","on_crash":"resume_checkpoint_then_backup_backend","on_budget":"partial_or_unknown"},
                "route_cancellation_epoch":route.cancellation_epoch,
                "content_hash":"",
                "created_at":now,
            });
            let contract_hash = sha256_json(&contract_value)?;
            contract_value["content_hash"] = json!(contract_hash);
            sqlx::query("INSERT INTO task_contracts(task_contract_id,project_id,task_id,plan_revision_id,contract_version,contract_json,content_hash,created_at) VALUES(?,?,?,?,1,?,?,?)")
                .bind(&task_contract_id).bind(project_id).bind(&task_id).bind(&plan_revision_id)
                .bind(json_text(&contract_value)?).bind(&contract_hash).bind(now.to_rfc3339()).execute(&mut *tx).await?;
            task_contract_ids.push(task_contract_id);
            if let Some(bottleneck_id) = &bottleneck_id {
                sqlx::query("UPDATE bottlenecks SET attempted_task_ids_json=CASE WHEN EXISTS (SELECT 1 FROM json_each(attempted_task_ids_json) WHERE value=?) THEN attempted_task_ids_json ELSE json_insert(attempted_task_ids_json,'$[#]',?) END,updated_revision=?,updated_at=? WHERE bottleneck_id=?")
                    .bind(&task_id).bind(&task_id).bind(actual).bind(now.to_rfc3339()).bind(bottleneck_id)
                    .execute(&mut *tx).await?;
            }
            tasks.push(Task {
                task_id: task_id.clone(),
                project_id: project_id.into(),
                route_id: route.route_id.clone(),
                worker_id: Some(worker_id.clone()),
                worker_role: assignment.worker_role.clone(),
                goal_ids,
                objective: assignment.objective.clone(),
                completion_contract: assignment.completion_contract.clone(),
                status: TaskStatus::Queued,
                priority: assignment.priority,
                revision: 1,
                route_cancellation_epoch: route.cancellation_epoch,
                round: round.number,
                result_summary: None,
                plan_revision_id: Some(plan_revision_id.clone()),
                task_signature: Some(signature),
                context_packet_id: Some(packet_id),
            });
            workers.push(Worker {
                worker_id,
                project_id: project_id.into(),
                role: assignment.worker_role.clone(),
                backend: "codex_cli".into(),
                status: WorkerStatus::Ready,
                current_task_id: Some(task_id),
                current_route_id: Some(route.route_id.clone()),
                session_id: None,
                last_heartbeat: Some(now),
            });
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
        let pending_suggestions = sqlx::query("SELECT command_id,suggestion_id FROM suggestions WHERE project_id=? AND status='pending' AND effective_round<=?")
            .bind(project_id).bind(round.number).fetch_all(&mut *tx).await?;
        for item in pending_suggestions {
            let command_id: String = item.try_get("command_id")?;
            let suggestion_id: String = item.try_get("suggestion_id")?;
            let decision = plan
                .suggestion_decisions
                .iter()
                .find(|decision| decision.contains(&suggestion_id))
                .cloned()
                .or_else(|| plan.suggestion_decisions.first().cloned())
                .unwrap_or_else(|| {
                    "considered by V2 planner; no explicit disposition returned".into()
                });
            sqlx::query("UPDATE suggestions SET status='applied',decision=? WHERE suggestion_id=?")
                .bind(&decision)
                .bind(&suggestion_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE human_commands SET status='applied',after_revision=?,affected_entities_json=?,applied_at=? WHERE command_id=?")
                .bind(revision).bind(json_text(&vec![entity("suggestion",&suggestion_id)])?)
                .bind(now.to_rfc3339()).bind(&command_id).execute(&mut *tx).await?;
            events.push(append_event(&mut tx,project_id,revision,"human_command.applied",entity("command",&command_id),json!({"suggestion_id":suggestion_id,"decision":decision,"plan_revision_id":plan_revision_id}),Some(entity("plan_revision",&plan_revision_id))).await?);
        }
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
        if !matches!(
            current.status,
            TaskStatus::Queued | TaskStatus::Assigned | TaskStatus::Open
        ) {
            return Err(StorageError::InvalidTransition(format!(
                "task {} cannot be offered from {}",
                current.task_id, current.status
            )));
        }
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
        sqlx::query("UPDATE tasks SET status='offered',revision=revision+1 WHERE task_id=? AND status IN ('queued','assigned','open')")
            .bind(&task.task_id).execute(&mut *tx).await?;
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
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "accept_local_handshake",
            )
            .await?;
        let lease_token = format!("{}{}", new_id("token"), new_id("token"));
        let token_hash = hash_secret(&lease_token);
        let now = Utc::now();
        let ttl = i64::try_from(ttl_seconds.clamp(30, 3_600)).unwrap_or(3_600);
        let expires_at = now + chrono::Duration::seconds(ttl);
        let node_id = format!("local:{}", offer.task.project_id);
        let mut tx = self.pool().begin().await?;
        let current = sqlx::query(
            "SELECT status,revision,route_cancellation_epoch FROM tasks WHERE task_id=?",
        )
        .bind(&offer.task.task_id)
        .fetch_one(&mut *tx)
        .await?;
        let status: String = current.try_get("status")?;
        let route_epoch: i64 = current.try_get("route_cancellation_epoch")?;
        if status != "offered" || route_epoch != offer.task.route_cancellation_epoch {
            return Err(StorageError::LateSubmission(format!(
                "handshake is stale: task status={status}, route epoch={route_epoch}"
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
        sqlx::query("INSERT OR IGNORE INTO worker_nodes(node_id,display_name,capabilities_json,token_hash,status,node_epoch,registered_at,last_heartbeat) VALUES(?,?,'{\"local\":true}',?,'active',1,?,?)")
            .bind(&node_id).bind("local orchestrator").bind(&token_hash).bind(now.to_rfc3339()).bind(now.to_rfc3339()).execute(&mut *tx).await?;
        sqlx::query("UPDATE worker_nodes SET last_heartbeat=?,status='active' WHERE node_id=?")
            .bind(now.to_rfc3339())
            .bind(&node_id)
            .execute(&mut *tx)
            .await?;
        let final_task_revision = current.try_get::<i64, _>("revision")? + 1;
        let lease_id = new_id("lease");
        sqlx::query("INSERT INTO task_leases(lease_id,project_id,task_id,node_id,task_revision,route_epoch,lease_epoch,status,leased_at,expires_at,attempt_id,worker_instance_id,lease_token_hash,offered_at,last_heartbeat_at) VALUES(?,?,?,?,?,?,?,'active',?,?,?,?,?,?,?)")
            .bind(&lease_id).bind(&offer.task.project_id).bind(&offer.task.task_id).bind(&node_id)
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
            lease_token,
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
        sqlx::query("INSERT INTO worker_heartbeats(heartbeat_id,project_id,worker_instance_id,attempt_id,lease_epoch,health_json,occurred_at) VALUES(?,?,?,?,?,'{\"status\":\"ok\"}',?)")
            .bind(new_id("heartbeat")).bind(&lease.task.project_id).bind(&lease.worker_instance_id)
            .bind(&lease.attempt_id).bind(lease.lease_epoch).bind(now.to_rfc3339()).execute(self.pool()).await?;
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
        if output.summary.trim().is_empty() {
            return Err(StorageError::InvalidTransition(
                "result envelope requires a non-empty summary".into(),
            ));
        }
        let payload = serde_json::to_value(output)?;
        let content_hash = sha256_json(&payload)?;
        let idempotency_key = format!("result:{}:{}", lease.attempt_id, content_hash);
        if let Some(existing) = sqlx::query_scalar::<_, String>(
            "SELECT result_envelope_id FROM result_envelopes WHERE project_id=? AND idempotency_key=?",
        )
        .bind(&lease.task.project_id)
        .bind(&idempotency_key)
        .fetch_optional(self.pool())
        .await?
        {
            return Ok((existing, Vec::new()));
        }
        let bytes = serde_json::to_vec_pretty(output)?;
        let (artifact, artifact_event) = self
            .store_artifact(
                &lease.task.project_id,
                "worker_result_envelope_payload",
                &format!("{}-result.json", lease.attempt_id),
                &bytes,
                lease.task.round,
                vec![lease.task.task_id.clone(), lease.attempt_id.clone()],
            )
            .await?;
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::FactOrLease,
                "submit_local_result_envelope",
            )
            .await?;
        let mut tx = self.pool().begin().await?;
        validate_local_lease(&mut tx, lease, &["running", "checkpointed"]).await?;
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
        let event = append_event(&mut tx, &lease.task.project_id, revision, "task.result_submitted", entity("result_envelope", &result_envelope_id), json!({"task_id":lease.task.task_id,"attempt_id":lease.attempt_id,"content_hash":content_hash,"artifact_id":artifact.artifact_id}), Some(entity("task_lease", &lease.lease_id))).await?;
        tx.commit().await?;
        Ok((result_envelope_id, vec![artifact_event, event]))
    }

    #[allow(clippy::too_many_lines)]
    pub async fn ingest_local_result_envelope(
        &self,
        result_envelope_id: &str,
    ) -> StorageResult<(WorkerOutput, Vec<research_domain::DomainEvent>)> {
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
        if status == "ingested" {
            tx.rollback().await?;
            return Ok((output, Vec::new()));
        }
        if status != "submitted" {
            return Err(StorageError::InvalidTransition(format!(
                "result envelope is {status}"
            )));
        }
        let project_id: String = envelope.try_get("project_id")?;
        let task_id: String = envelope.try_get("task_id")?;
        let attempt_id: String = envelope.try_get("attempt_id")?;
        let lease_id: String = envelope.try_get("lease_id")?;
        let lease_epoch: i64 = envelope.try_get("lease_epoch")?;
        let route_epoch: i64 = envelope.try_get("route_cancellation_epoch")?;
        let task_row = sqlx::query("SELECT t.*,r.status AS joined_route_status,r.cancellation_epoch AS joined_route_epoch FROM tasks t JOIN routes r ON t.route_id=r.route_id WHERE t.task_id=?")
            .bind(&task_id).fetch_one(&mut *tx).await?;
        let task = rows::task(&task_row)?;
        let route_status: String = task_row.try_get("joined_route_status")?;
        let current_route_epoch: i64 = task_row.try_get("joined_route_epoch")?;
        let lease_status: String = sqlx::query_scalar(
            "SELECT status FROM task_leases WHERE lease_id=? AND attempt_id=? AND lease_epoch=?",
        )
        .bind(&lease_id)
        .bind(&attempt_id)
        .bind(lease_epoch)
        .fetch_one(&mut *tx)
        .await?;
        if task.status != TaskStatus::ResultSubmitted
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
        for discovery in &output.discoveries {
            sqlx::query("INSERT INTO hypotheses(hypothesis_id,project_id,kind,statement,status,route_id,created_in_round,attributes_json) VALUES(?,?,?,?,?,?,?,?)")
                .bind(new_id("hyp")).bind(&project_id).bind(&discovery.kind).bind(&discovery.statement)
                .bind("proposed").bind(&task.route_id).bind(task.round).bind(json_text(&discovery.attributes)?).execute(&mut *tx).await?;
        }
        for failure in &output.failures {
            sqlx::query("INSERT INTO failures(failure_id,project_id,route_id,task_id,failure_type,summary,repairable,raw_json,created_at) VALUES(?,?,?,?,?,?,?,?,?)")
                .bind(new_id("failure")).bind(&project_id).bind(&task.route_id).bind(&task_id)
                .bind(&failure.failure_type).bind(&failure.summary).bind(failure.repairable).bind(json_text(failure)?)
                .bind(Utc::now().to_rfc3339()).execute(&mut *tx).await?;
        }
        for description in &output.uncertainties {
            sqlx::query("INSERT INTO uncertainties(uncertainty_id,project_id,description,type,severity,affects_goal_ids_json,affects_route_ids_json,introduced_by,resolution_methods_json,status,created_at) VALUES(?,?,?,?,?,?,?,?,?,'open',?)")
                .bind(new_id("unc")).bind(&project_id).bind(description).bind("worker_uncertainty").bind("medium")
                .bind(json_text(&task.goal_ids)?).bind(json_text(&vec![task.route_id.clone()])?).bind(&task_id).bind("[]")
                .bind(Utc::now().to_rfc3339()).execute(&mut *tx).await?;
        }
        let mut inserted_sources = Vec::<(String, String)>::new();
        let mut rejected_source_drafts = Vec::<(String, Vec<&'static str>)>::new();
        for source in &output.sources {
            let (normalized_url, identifier_kind, identifier_value) =
                crate::normalized_source_identity(source.url.as_deref());
            let mut reasons = Vec::new();
            let fulltext_artifact_valid = match &source.fulltext_artifact_id {
                Some(artifact_id) => sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM artifacts WHERE artifact_id=? AND project_id=? AND kind='source_fulltext' AND (? IS NULL OR lower(sha256)=lower(?))")
                    .bind(artifact_id).bind(&project_id).bind(&source.fulltext_sha256).bind(&source.fulltext_sha256)
                    .fetch_one(&mut *tx).await? == 1,
                None => task.worker_role != "literature_researcher" || source.status == "not_applicable",
            };
            if source.title.trim().is_empty() {
                reasons.push("missing_title");
            }
            if normalized_url.is_none()
                && source
                    .citation_key
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
            {
                reasons.push("missing_stable_identifier");
            }
            if source
                .theorem_reference
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                reasons.push("missing_theorem_or_content_locator");
            }
            if source
                .statement_excerpt
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                reasons.push("missing_statement_excerpt");
            }
            if !fulltext_artifact_valid {
                reasons.push("missing_or_invalid_fulltext_artifact");
            }
            if !reasons.is_empty() {
                let failure_id = new_id("failure");
                sqlx::query("INSERT INTO failures(failure_id,project_id,route_id,task_id,failure_type,summary,repairable,raw_json,created_at) VALUES(?,?,?,?,?,?,?,?,?)")
                    .bind(&failure_id).bind(&project_id).bind(&task.route_id).bind(&task_id)
                    .bind("invalid_source_draft").bind(format!("Rejected literature source draft: {}", reasons.join(", ")))
                    .bind(true).bind(json_text(&json!({"source":source,"reasons":reasons}))?)
                    .bind(Utc::now().to_rfc3339()).execute(&mut *tx).await?;
                sqlx::query("INSERT INTO source_ingestion_records(ingestion_id,project_id,task_id,route_id,source_id,disposition,reasons_json,normalized_url,identifier_kind,identifier_value,raw_json,created_at) VALUES(?,?,?,?,NULL,'rejected',?,?,?,?,?,?)")
                    .bind(new_id("sourceingest")).bind(&project_id).bind(&task_id).bind(&task.route_id)
                    .bind(json_text(&reasons)?).bind(&normalized_url).bind(&identifier_kind).bind(&identifier_value)
                    .bind(json_text(source)?).bind(Utc::now().to_rfc3339()).execute(&mut *tx).await?;
                rejected_source_drafts.push((failure_id, reasons));
                continue;
            }
            let duplicate_source_id = sqlx::query_scalar::<_, String>("SELECT source_id FROM sources WHERE project_id=? AND ((? IS NOT NULL AND normalized_url=?) OR (? IS NOT NULL AND identifier_kind=? AND identifier_value=?)) LIMIT 1")
                .bind(&project_id).bind(&normalized_url).bind(&normalized_url).bind(&identifier_value)
                .bind(&identifier_kind).bind(&identifier_value).fetch_optional(&mut *tx).await?;
            if let Some(source_id) = duplicate_source_id {
                sqlx::query("INSERT INTO source_ingestion_records(ingestion_id,project_id,task_id,route_id,source_id,disposition,reasons_json,normalized_url,identifier_kind,identifier_value,raw_json,created_at) VALUES(?,?,?,?,?,'duplicate','[\"same_normalized_identifier\"]',?,?,?,?,?)")
                    .bind(new_id("sourceingest")).bind(&project_id).bind(&task_id).bind(&task.route_id).bind(source_id)
                    .bind(&normalized_url).bind(&identifier_kind).bind(&identifier_value).bind(json_text(source)?)
                    .bind(Utc::now().to_rfc3339()).execute(&mut *tx).await?;
                continue;
            }
            let source_id = new_id("source");
            sqlx::query("INSERT INTO sources(source_id,project_id,title,authors_json,url,normalized_url,identifier_kind,identifier_value,citation_key,theorem_reference,statement_excerpt,assumptions_json,applicability,status,origin_task_id,origin_route_id,fulltext_artifact_id,provenance_json,retrieved_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,'reported_unverified',?,?,?,?,?)")
                .bind(&source_id).bind(&project_id).bind(source.title.trim()).bind(json_text(&source.authors)?)
                .bind(&source.url).bind(&normalized_url).bind(&identifier_kind).bind(&identifier_value)
                .bind(&source.citation_key).bind(&source.theorem_reference).bind(&source.statement_excerpt)
                .bind(json_text(&source.assumptions)?).bind(&source.applicability).bind(&task_id).bind(&task.route_id)
                .bind(&source.fulltext_artifact_id)
                .bind(json_text(&json!({"ingestion":"leased_worker_result","trust":"reported_unverified","retrieval_query":source.retrieval_query,"document_version":source.document_version,"fulltext_sha256":source.fulltext_sha256}))?)
                .bind(Utc::now().to_rfc3339()).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO source_ingestion_records(ingestion_id,project_id,task_id,route_id,source_id,disposition,reasons_json,normalized_url,identifier_kind,identifier_value,raw_json,created_at) VALUES(?,?,?,?,?,'inserted','[]',?,?,?,?,?)")
                .bind(new_id("sourceingest")).bind(&project_id).bind(&task_id).bind(&task.route_id).bind(&source_id)
                .bind(&normalized_url).bind(&identifier_kind).bind(&identifier_value).bind(json_text(source)?)
                .bind(Utc::now().to_rfc3339()).execute(&mut *tx).await?;
            inserted_sources.push((source_id, source.title.trim().to_owned()));
        }
        let mut capsule_events = Vec::new();
        for experiment in &output.experiments {
            let canonical = serde_json::to_value(experiment)?;
            let content_hash = sha256_json(&canonical)?;
            let capsule_id = new_id("capsule");
            sqlx::query("INSERT INTO experiment_capsules(capsule_id,project_id,task_id,route_id,language,program_text,input_json,environment_json,stdout,stderr,exit_code,artifacts_json,conclusion_mapping_json,replay_command_json,status,content_hash,created_at,replayed_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,'reported_unverified',?,?,NULL)")
                .bind(&capsule_id).bind(&project_id).bind(&task_id).bind(&task.route_id).bind(&experiment.language)
                .bind(&experiment.program_text).bind(json_text(&experiment.input)?).bind(json_text(&experiment.environment)?)
                .bind(&experiment.stdout).bind(&experiment.stderr).bind(experiment.exit_code).bind(json_text(&experiment.artifacts)?)
                .bind(json_text(&experiment.conclusion_mapping)?).bind(json_text(&experiment.replay_command)?)
                .bind(&content_hash).bind(Utc::now().to_rfc3339()).execute(&mut *tx).await?;
            capsule_events.push((capsule_id, content_hash));
        }
        let material_progress = !output.candidates.is_empty()
            || !output.discoveries.is_empty()
            || !output.experiments.is_empty()
            || !inserted_sources.is_empty();
        sqlx::query("UPDATE tasks SET status='completed',result_summary=?,revision=revision+1 WHERE task_id=? AND status='ingesting'")
            .bind(&output.summary).bind(&task_id).execute(&mut *tx).await?;
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
            let kind = if !output.candidates.is_empty() {
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
        let mut events = Vec::new();
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
        for (source_id, title) in &inserted_sources {
            events.push(
                append_event(
                    &mut tx,
                    &project_id,
                    revision,
                    "source.reported",
                    entity("source", source_id),
                    json!({"task_id":task_id,"route_id":task.route_id,"title":title,"trust":"reported_unverified"}),
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
        events.push(append_event(&mut tx,&project_id,revision,"task.result_ingested",entity("result_envelope",result_envelope_id),json!({"task_id":task_id,"attempt_id":attempt_id,"candidate_count":output.candidates.len(),"source_count":output.sources.len(),"inserted_source_count":inserted_sources.len(),"rejected_source_count":rejected_source_drafts.len(),"experiment_count":output.experiments.len()}),Some(entity("task_lease",&lease_id))).await?);
        tx.commit().await?;
        Ok((output, events))
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
        let same_failures: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_attempts WHERE task_id=? AND failure_signature=?",
        )
        .bind(&offer.task.task_id)
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
        sqlx::query("UPDATE task_attempts SET status='failed',failure_signature=?,failure_reason=?,completed_at=? WHERE attempt_id=? AND status NOT IN ('completed','failed')")
            .bind(&signature).bind(reason).bind(now.to_rfc3339()).bind(&offer.attempt.attempt_id).execute(&mut *tx).await?;
        if let Some(lease) = lease {
            sqlx::query("UPDATE task_leases SET status='failed',completed_at=? WHERE lease_id=? AND status='active'")
                .bind(now.to_rfc3339()).bind(&lease.lease_id).execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE tasks SET status=?,revision=revision+1,result_summary=? WHERE task_id=? AND status NOT IN ('completed','cancelled','human_stopped')")
            .bind(next_status).bind(reason).bind(&offer.task.task_id).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE routes SET failed_attempt_count=failed_attempt_count+1 WHERE route_id=?",
        )
        .bind(&offer.task.route_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE worker_instances SET status=CASE WHEN ? THEN 'quarantined' ELSE 'exited' END,quarantine_reason=CASE WHEN ? THEN ? ELSE quarantine_reason END,exited_at=? WHERE worker_instance_id=?")
            .bind(terminal).bind(terminal).bind(reason).bind(now.to_rfc3339()).bind(&offer.worker_instance.worker_instance_id).execute(&mut *tx).await?;
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
        Ok(json!({
            "project_id":project_id,
            "project_revision":project.revision,
            "research_delta":delta,
            "open_bottlenecks":bottlenecks.into_iter().filter(|item| item.status=="open").collect::<Vec<_>>(),
            "live_routes":routes.into_iter().filter(|item| matches!(item.status.to_string().as_str(),"incubating"|"active"|"blocked"|"probation"|"revived")).collect::<Vec<_>>(),
            "live_tasks":tasks.into_iter().filter(|item| matches!(item.status,TaskStatus::Queued|TaskStatus::Offered|TaskStatus::Leased|TaskStatus::Running|TaskStatus::Checkpointed|TaskStatus::ResultSubmitted|TaskStatus::Ingesting)).collect::<Vec<_>>(),
            "summaries":summaries,
            "trust_rule":"This digest is a planning view, not a mathematical premise. Retrieve active facts by fact_id.",
        }))
    }

    pub async fn prune_route_v2(
        &self,
        project_id: &str,
        route_id: &str,
        expected_revision: i64,
        reason: &str,
    ) -> StorageResult<research_domain::DomainEvent> {
        if reason.trim().is_empty() {
            return Err(StorageError::InvalidTransition(
                "route pruning requires a reason".into(),
            ));
        }
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
        let row = sqlx::query(
            "SELECT status,semantic_fingerprint FROM routes WHERE project_id=? AND route_id=?",
        )
        .bind(project_id)
        .bind(route_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StorageError::NotFound {
            kind: "route",
            id: route_id.into(),
        })?;
        let status: String = row.try_get("status")?;
        if matches!(status.as_str(), "human_stopped" | "merged" | "pruned") {
            return Err(StorageError::InvalidTransition(format!(
                "route {route_id} is already {status}"
            )));
        }
        let fingerprint: Option<String> = row.try_get("semantic_fingerprint")?;
        sqlx::query("UPDATE routes SET status='pruned',cancellation_epoch=cancellation_epoch+1,merge_reason=? WHERE route_id=?")
            .bind(reason).bind(route_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE tasks SET status='cancelled',revision=revision+1,result_summary=? WHERE route_id=? AND status IN ('queued','offered','leased','running','checkpointed','result_submitted','ingesting')")
            .bind(format!("route pruned: {reason}")).bind(route_id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO route_tombstones(tombstone_id,project_id,route_id,semantic_fingerprint,reason,revive_only_if_json,created_revision,created_at) VALUES(?,?,?,?,?,?,?,?)")
            .bind(new_id("tombstone")).bind(project_id).bind(route_id).bind(fingerprint.unwrap_or_else(|| route_id.into()))
            .bind(reason).bind(json_text(&vec!["new verified evidence","resolved linked bottleneck","explicit authorized revival"])?).bind(actual+1)
            .bind(Utc::now().to_rfc3339()).execute(&mut *tx).await?;
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            "route.pruned",
            entity("route", route_id),
            json!({"reason":reason,"source":"human_command"}),
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
        if source_route_ids.is_empty() || reason.trim().is_empty() {
            return Err(StorageError::InvalidTransition(
                "route merge requires sources and a reason".into(),
            ));
        }
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
        let canonical_exists:i64 = sqlx::query_scalar("SELECT COUNT(*) FROM routes WHERE project_id=? AND route_id=? AND status IN ('incubating','active','blocked','probation','revived')")
            .bind(project_id).bind(canonical_route_id).fetch_one(&mut *tx).await?;
        if canonical_exists != 1 {
            return Err(StorageError::InvalidTransition(
                "canonical route is not live".into(),
            ));
        }
        let revision = actual + 1;
        let mut events = Vec::new();
        for source in source_route_ids
            .iter()
            .filter(|source| source.as_str() != canonical_route_id)
        {
            let changed=sqlx::query("UPDATE routes SET status='merged',merged_into=?,merge_reason=?,cancellation_epoch=cancellation_epoch+1 WHERE project_id=? AND route_id=? AND status IN ('incubating','active','blocked','probation','revived')")
                .bind(canonical_route_id).bind(reason).bind(project_id).bind(source).execute(&mut *tx).await?;
            if changed.rows_affected() != 1 {
                return Err(StorageError::InvalidTransition(format!(
                    "source route {source} is not mergeable"
                )));
            }
            sqlx::query("UPDATE tasks SET status='cancelled',revision=revision+1,result_summary=? WHERE route_id=? AND status IN ('queued','offered','leased','running','checkpointed','result_submitted','ingesting')")
                .bind(format!("route merged into {canonical_route_id}: {reason}")).bind(source).execute(&mut *tx).await?;
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
        if events.is_empty() {
            return Err(StorageError::InvalidTransition(
                "route merge produced no changes".into(),
            ));
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
        if reason.trim().is_empty() || evidence_ids.is_empty() {
            return Err(StorageError::InvalidTransition(
                "route revival requires a reason and evidence IDs".into(),
            ));
        }
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
        let changed=sqlx::query("UPDATE routes SET status='revived',cancellation_epoch=cancellation_epoch+1,consecutive_no_progress_plans=0,merge_reason=NULL WHERE project_id=? AND route_id=? AND status='pruned'")
            .bind(project_id).bind(route_id).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidTransition(
                "only a pruned route can be revived".into(),
            ));
        }
        sqlx::query("UPDATE route_tombstones SET revived_revision=? WHERE project_id=? AND route_id=? AND revived_revision IS NULL")
            .bind(actual+1).bind(project_id).bind(route_id).execute(&mut *tx).await?;
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
        let revision = bump_revision(&mut tx, project_id).await?;
        let event = append_event(
            &mut tx,
            project_id,
            revision,
            "planner.degraded_waiting",
            entity("project", project_id),
            json!({"reason":reason,"new_assignments":0}),
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
        let orphaned_task_attempts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_attempts a LEFT JOIN task_leases l ON a.attempt_id=l.attempt_id AND l.status='active' WHERE a.status IN ('leased','running','checkpointed','result_submitted','ingesting') AND l.lease_id IS NULL AND NOT (a.status IN ('result_submitted','ingesting') AND EXISTS (SELECT 1 FROM result_envelopes e WHERE e.attempt_id=a.attempt_id AND e.status='submitted'))")
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
            state_writer_queue_depth: writer.queue_depth,
            state_writer_queue_capacity: writer.queue_capacity,
            state_writer_admitted_total: i64::try_from(writer.admitted_total).unwrap_or(i64::MAX),
            state_writer_completed_total: i64::try_from(writer.completed_total).unwrap_or(i64::MAX),
            state_writer_queue_wait_seconds: writer.queue_wait_seconds,
            state_writer_last_transaction_seconds: writer.last_transaction_seconds,
            sqlite_busy_total: i64::try_from(writer.sqlite_busy_total).unwrap_or(i64::MAX),
            outbox_pending,
            outbox_oldest_age_seconds: oldest_age,
            orphaned_task_attempts,
            expired_active_leases,
            last_reconciliation,
        })
    }

    pub async fn mark_outbox_delivered(&self, event_id: &str) -> StorageResult<()> {
        let _admission = self
            .admit_write(
                crate::state_writer::WritePriority::Telemetry,
                "mark_outbox_delivered",
            )
            .await?;
        let changed = sqlx::query("UPDATE event_outbox SET status='delivered',delivered_at=?,attempt_count=attempt_count+1,last_error=NULL WHERE event_id=? AND status='pending'")
            .bind(Utc::now().to_rfc3339()).bind(event_id).execute(self.pool()).await?;
        if changed.rows_affected() == 0 {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM event_outbox WHERE event_id=? AND status='delivered'",
            )
            .bind(event_id)
            .fetch_one(self.pool())
            .await?;
            if exists == 0 {
                return Err(StorageError::NotFound {
                    kind: "outbox_event",
                    id: event_id.into(),
                });
            }
        }
        Ok(())
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
        // Periodic watchdog ticks focus on leases; startup/manual runs also perform the
        // more expensive immutable artifact verification.
        let integrity_issues = if trigger_kind == "watchdog_tick" {
            Vec::new()
        } else {
            scan_artifact_integrity(self, project_id).await?
        };
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
        let now = Utc::now().to_rfc3339();
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
            sqlx::query("SELECT project_id,lease_id,task_id,attempt_id FROM task_leases WHERE project_id=? AND status='active' AND expires_at<?")
                .bind(scope).bind(&now).fetch_all(&mut *tx).await?
        } else {
            sqlx::query("SELECT project_id,lease_id,task_id,attempt_id FROM task_leases WHERE status='active' AND expires_at<?")
                .bind(&now).fetch_all(&mut *tx).await?
        };
        let mut repairs = Vec::new();
        let mut repair_events: BTreeMap<String, Vec<(String, EntityRef, Value)>> = BTreeMap::new();
        for row in expired {
            let repair_project_id: String = row.try_get("project_id")?;
            let lease_id: String = row.try_get("lease_id")?;
            let task_id: String = row.try_get("task_id")?;
            let attempt_id: Option<String> = row.try_get("attempt_id")?;
            sqlx::query(
                "UPDATE task_leases SET status='expired' WHERE lease_id=? AND status='active'",
            )
            .bind(&lease_id)
            .execute(&mut *tx)
            .await?;
            let has_submitted_envelope: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM result_envelopes WHERE lease_id=? AND status='submitted'",
            )
            .bind(&lease_id)
            .fetch_one(&mut *tx)
            .await?;
            if let Some(attempt_id) = &attempt_id {
                if has_submitted_envelope == 0 {
                    sqlx::query("UPDATE task_attempts SET status='orphaned',failure_reason='lease expired during reconciliation',completed_at=? WHERE attempt_id=? AND status NOT IN ('completed','failed','blocked','orphaned')")
                        .bind(&now).bind(attempt_id).execute(&mut *tx).await?;
                }
            }
            if has_submitted_envelope == 0 {
                sqlx::query("UPDATE tasks SET status=CASE WHEN (SELECT COUNT(*) FROM task_attempts WHERE task_id=? )>=3 THEN 'dead_lettered' ELSE 'queued' END,revision=revision+1 WHERE task_id=? AND status IN ('offered','leased','running','checkpointed','result_submitted','ingesting')")
                    .bind(&task_id).bind(&task_id).execute(&mut *tx).await?;
            }
            repairs.push(json!({"kind":if has_submitted_envelope>0 {"expired_lease_with_replayable_result"} else {"expired_lease"},"lease_id":lease_id,"task_id":task_id,"attempt_id":attempt_id}));
            repair_events.entry(repair_project_id).or_default().push((
                "task.lease_expired".into(),
                entity("task_lease", &lease_id),
                json!({"task_id":task_id,"attempt_id":attempt_id,"replayable_result":has_submitted_envelope>0}),
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
                    sqlx::query("UPDATE verification_cases SET stage=CASE WHEN snapshot_id IS NULL THEN 'intake' ELSE 'precheck' END,cancellation_epoch=cancellation_epoch+1,achieved_acceptance=NULL,completed_at=NULL,updated_at=? WHERE case_id=?")
                        .bind(&now).bind(case_id).execute(&mut *tx).await?;
                    sqlx::query("UPDATE verification_task_leases SET status='expired',completed_at=? WHERE case_id=? AND status='active'")
                        .bind(&now).bind(case_id).execute(&mut *tx).await?;
                    sqlx::query("UPDATE verification_result_envelopes SET status='stale',rejection_reason='verification stage interrupted by service restart' WHERE case_id=? AND status='submitted'")
                        .bind(case_id).execute(&mut *tx).await?;
                    repairs.push(json!({"kind":"verification_reset_for_restart","verification_id":verification_id,"case_id":case_id,"candidate_id":candidate_id}));
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
        for issue in integrity_issues {
            sqlx::query("UPDATE projects SET status='needs_human_review',updated_at=? WHERE project_id=? AND status NOT IN ('success','partial_success','refuted','stopped_by_human','error')")
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
        sqlx::query("UPDATE reconciliation_runs SET status='completed',repairs_json=?,completed_at=? WHERE reconciliation_run_id=?")
            .bind(json_text(&repairs)?).bind(completed_at.to_rfc3339()).bind(&run_id).execute(&mut *tx).await?;
        if let Some(project_id) = project_id {
            repair_events.entry(project_id.into()).or_default().push((
                "storage.reconciliation.completed".into(),
                entity("reconciliation_run", &run_id),
                json!({"repair_count":repairs.len()}),
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
        Ok(
            json!({"reconciliation_run_id":run_id,"status":"completed","repairs":repairs,"started_at":started_at,"completed_at":completed_at}),
        )
    }
}

async fn scan_artifact_integrity(
    store: &SqliteStore,
    project_id: Option<&str>,
) -> StorageResult<Vec<ArtifactIntegrityIssue>> {
    let rows = if let Some(project_id) = project_id {
        sqlx::query(
            "SELECT project_id,artifact_id,storage_path,sha256 FROM artifacts WHERE project_id=?",
        )
        .bind(project_id)
        .fetch_all(store.pool())
        .await?
    } else {
        sqlx::query("SELECT project_id,artifact_id,storage_path,sha256 FROM artifacts")
            .fetch_all(store.pool())
            .await?
    };
    let mut issues = Vec::new();
    for row in rows {
        let project_id: String = row.try_get("project_id")?;
        let artifact_id: String = row.try_get("artifact_id")?;
        let storage_path: String = row.try_get("storage_path")?;
        let expected: String = row.try_get("sha256")?;
        let reason = match tokio::fs::read(&storage_path).await {
            Ok(content) => {
                let observed = hex::encode(Sha256::digest(&content));
                (observed != expected)
                    .then(|| format!("sha256_mismatch:expected={expected}:observed={observed}"))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Some("artifact_file_missing".into())
            }
            Err(error) => Some(format!("artifact_file_unreadable:{error}")),
        };
        if let Some(reason) = reason {
            issues.push(ArtifactIntegrityIssue {
                project_id,
                artifact_id,
                reason,
            });
        }
    }
    Ok(issues)
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

fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

fn sha256_json(value: &Value) -> StorageResult<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
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

fn route_fingerprint(
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

fn route_family_key(target_goal_ids: &[String], method_summary: &str) -> StorageResult<String> {
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

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let mut output = value.chars().take(limit).collect::<String>();
    output.push_str("\n[omitted: original problem exceeded task-packet excerpt budget]");
    output
}

#[allow(dead_code)]
fn stable_content_hash(value: &Value) -> StorageResult<String> {
    sha256_json(value)
}
