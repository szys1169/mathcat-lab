use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use research_domain::{
    AssignmentDraft, Budget, ProblemContract, Route, Task, TaskStatus, Worker, WorkerStatus,
};
use serde_json::{Value, json};
use sqlx::Row;

use crate::{
    StorageResult, entity, json_text, new_id,
    reliability_v2::{sha256_json, truncate_chars},
};

pub(crate) struct MaterializedV2Task {
    pub task: Task,
    pub worker: Worker,
    pub task_contract_id: String,
}

pub(crate) struct V2TaskMaterialization<'a> {
    pub project_id: &'a str,
    pub route: &'a Route,
    pub assignment: &'a AssignmentDraft,
    pub problem_contract: &'a ProblemContract,
    pub budget: &'a Budget,
    pub round_number: i64,
    pub plan_revision_id: &'a str,
    pub source_revision: i64,
    pub source_command_id: Option<&'a str>,
    pub now: &'a DateTime<Utc>,
}

/// Create `Worker`, `Task`, `ContextPacket`, and immutable `TaskContract` as one part of
/// the caller's transaction. Planner and human-authored routes intentionally
/// share this implementation so their trust and timeout contracts cannot drift.
#[allow(clippy::too_many_lines)]
pub(crate) async fn materialize_v2_task_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    input: V2TaskMaterialization<'_>,
) -> StorageResult<Option<MaterializedV2Task>> {
    let V2TaskMaterialization {
        project_id,
        route,
        assignment,
        problem_contract,
        budget,
        round_number,
        plan_revision_id,
        source_revision,
        source_command_id,
        now,
    } = input;
    let goal_ids = if assignment.goal_ids.is_empty() {
        route.target_goal_ids.clone()
    } else {
        assignment.goal_ids.clone()
    };
    let bottleneck_id = {
        let rows = sqlx::query("SELECT bottleneck_id,target_goal_ids_json FROM bottlenecks WHERE project_id=? AND status='open' ORDER BY priority DESC,created_at")
            .bind(project_id)
            .fetch_all(&mut **tx)
            .await?;
        rows.into_iter().find_map(|row| {
            let ids = serde_json::from_str::<Vec<String>>(
                row.try_get::<String, _>("target_goal_ids_json")
                    .ok()?
                    .as_str(),
            )
            .ok()?;
            ids.iter()
                .any(|id| goal_ids.contains(id))
                .then(|| row.try_get::<String, _>("bottleneck_id").ok())
                .flatten()
        })
    };
    let completion = json!({
        "success_outputs": [assignment.completion_contract],
        "partial_outputs": ["a strictly narrower named subproblem with an explicit implication or equivalence"],
        "reject_outputs": ["generic discussion", "a repeated failed proof without new evidence", "an unstated strengthening of assumptions"],
    });
    let signature = sha256_json(&json!({
        "project_id": project_id,
        "goal_ids": goal_ids,
        "bottleneck_id": bottleneck_id,
        "task_kind": assignment.worker_role,
        "required_fact_ids": route.required_fact_ids,
        "completion_contract_version": 1,
        "route_cancellation_epoch": route.cancellation_epoch,
    }))?;
    let duplicate: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE project_id=? AND task_signature=? AND status IN ('queued','offered','leased','running','checkpointed','result_submitted','ingesting')")
        .bind(project_id)
        .bind(&signature)
        .fetch_one(&mut **tx)
        .await?;
    if duplicate > 0 {
        return Ok(None);
    }

    let task_id = new_id("task");
    let worker_id = new_id("worker");
    sqlx::query("INSERT INTO workers(worker_id,project_id,role,backend,status,current_task_id,current_route_id,last_heartbeat) VALUES(?,?,?,?,?,?,?,?)")
        .bind(&worker_id)
        .bind(project_id)
        .bind(&assignment.worker_role)
        .bind("codex_cli")
        .bind(WorkerStatus::Ready.to_string())
        .bind(&task_id)
        .bind(&route.route_id)
        .bind(now.to_rfc3339())
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO tasks(task_id,project_id,route_id,worker_id,worker_role,goal_ids_json,objective,completion_contract,status,priority,revision,route_cancellation_epoch,round,plan_revision_id,task_signature) VALUES(?,?,?,?,?,?,?,?,?,?,1,?,?,?,?)")
        .bind(&task_id)
        .bind(project_id)
        .bind(&route.route_id)
        .bind(&worker_id)
        .bind(&assignment.worker_role)
        .bind(json_text(&goal_ids)?)
        .bind(&assignment.objective)
        .bind(&assignment.completion_contract)
        .bind(TaskStatus::Queued.to_string())
        .bind(assignment.priority)
        .bind(route.cancellation_epoch)
        .bind(round_number)
        .bind(plan_revision_id)
        .bind(&signature)
        .execute(&mut **tx)
        .await?;

    let fact_rows = sqlx::query("SELECT fact_id,statement,assumptions_json,evidence_level FROM facts WHERE project_id=? AND status='active' ORDER BY created_at DESC")
        .bind(project_id)
        .fetch_all(&mut **tx)
        .await?;
    let required_fact_ids = route.required_fact_ids.iter().collect::<BTreeSet<_>>();
    let fact_digests = fact_rows
        .iter()
        .filter_map(|row| {
            let fact_id = row.try_get::<String, _>("fact_id").ok()?;
            required_fact_ids.contains(&fact_id).then(|| {
                Ok(json!({
                    "fact_id": fact_id,
                    "statement": row.try_get::<String, _>("statement")?,
                    "assumptions": serde_json::from_str::<Value>(row.try_get("assumptions_json")?)?,
                    "evidence_level": row.try_get::<String, _>("evidence_level")?,
                }))
            })
        })
        .collect::<StorageResult<Vec<_>>>()?;
    let known_fact_ids = fact_digests
        .iter()
        .filter_map(|value| {
            value
                .get("fact_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    let failure_rows = sqlx::query("SELECT pattern_id,title,summary FROM failure_patterns WHERE project_id=? ORDER BY created_at DESC LIMIT 8")
        .bind(project_id)
        .fetch_all(&mut **tx)
        .await?;
    let failure_digests = failure_rows
        .iter()
        .map(|row| {
            Ok(json!({
                "pattern_id": row.try_get::<String, _>("pattern_id")?,
                "title": row.try_get::<String, _>("title")?,
                "summary": row.try_get::<String, _>("summary")?,
            }))
        })
        .collect::<StorageResult<Vec<_>>>()?;
    let failure_ids = failure_digests
        .iter()
        .filter_map(|value| {
            value
                .get("pattern_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    let uncertainty_rows = sqlx::query("SELECT uncertainty_id,description,severity FROM uncertainties WHERE project_id=? AND status IN ('open','investigating') ORDER BY CASE severity WHEN 'critical' THEN 0 WHEN 'high' THEN 1 ELSE 2 END,created_at LIMIT 12")
        .bind(project_id)
        .fetch_all(&mut **tx)
        .await?;
    let uncertainty_digests = uncertainty_rows
        .iter()
        .map(|row| {
            Ok(json!({
                "uncertainty_id": row.try_get::<String, _>("uncertainty_id")?,
                "description": row.try_get::<String, _>("description")?,
                "severity": row.try_get::<String, _>("severity")?,
            }))
        })
        .collect::<StorageResult<Vec<_>>>()?;
    let uncertainty_ids = uncertainty_digests
        .iter()
        .filter_map(|value| {
            value
                .get("uncertainty_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    let problem_excerpt = truncate_chars(
        &format!(
            "Target: {}\nAssumptions: {}\nOriginal problem:\n{}",
            problem_contract.target_statement,
            problem_contract.assumptions.join("; "),
            problem_contract.original_problem
        ),
        8_000,
    );
    let source = if source_command_id.is_some() {
        "human"
    } else {
        "planner"
    };
    let packet_content = json!({
        "problem_contract": {"version": problem_contract.version, "excerpt": problem_excerpt},
        "task": {"task_id": task_id, "objective": assignment.objective, "goal_ids": goal_ids, "completion_contract": completion},
        "route": {"route_id": route.route_id, "method_summary": route.method_summary, "required_fact_ids": route.required_fact_ids, "exit_criteria": route.exit_criteria, "source": source},
        "bottleneck_id": bottleneck_id,
        "known_facts": fact_digests,
        "relevant_failure_patterns": failure_digests,
        "relevant_uncertainties": uncertainty_digests,
        "trust_rule": "Only active facts identified by fact_id are mathematical premises. Route text is an execution directive, not a fact.",
    });
    let packet_hash = sha256_json(&packet_content)?;
    let packet_id = new_id("context");
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
    if let Some(command_id) = source_command_id {
        source_refs.push(entity("command", command_id));
    }
    source_refs.extend(known_fact_ids.iter().map(|id| entity("fact", id)));
    source_refs.extend(failure_ids.iter().map(|id| entity("failure_pattern", id)));
    source_refs.extend(uncertainty_ids.iter().map(|id| entity("uncertainty", id)));
    if let Some(id) = &bottleneck_id {
        source_refs.push(entity("bottleneck", id));
    }
    sqlx::query("INSERT INTO context_packets(context_packet_id,project_id,task_id,route_id,packet_kind,source_revision,problem_contract_excerpt,objective,known_fact_ids_json,bottleneck_id,route_progress_json,relevant_failure_pattern_ids_json,relevant_uncertainty_ids_json,source_refs_json,omitted_sections_json,token_estimate,content_json,content_hash,status,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?, ?,?,?, ?,?,?,?,'active',?)")
        .bind(&packet_id)
        .bind(project_id)
        .bind(&task_id)
        .bind(&route.route_id)
        .bind("task")
        .bind(source_revision)
        .bind(&problem_excerpt)
        .bind(&assignment.objective)
        .bind(json_text(&known_fact_ids)?)
        .bind(&bottleneck_id)
        .bind(json_text(&json!({"last_material_progress_revision":route.last_material_progress_revision}))?)
        .bind(json_text(&failure_ids)?)
        .bind(json_text(&uncertainty_ids)?)
        .bind(json_text(&source_refs)?)
        .bind(json_text(&omitted)?)
        .bind(token_estimate)
        .bind(json_text(&packet_content)?)
        .bind(&packet_hash)
        .bind(now.to_rfc3339())
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE tasks SET context_packet_id=? WHERE task_id=?")
        .bind(&packet_id)
        .bind(&task_id)
        .execute(&mut **tx)
        .await?;

    let task_contract_id = new_id("taskcontract");
    let mut contract_value = json!({
        "task_contract_id": task_contract_id,
        "project_id": project_id,
        "task_id": task_id,
        "plan_revision_id": plan_revision_id,
        "contract_version": 1,
        "route_id": route.route_id,
        "target_goal_ids": goal_ids,
        "bottleneck_id": bottleneck_id,
        "task_kind": assignment.worker_role,
        "precise_objective": assignment.objective,
        "allowed_input_ids": route.required_fact_ids,
        "context_packet_id": packet_id,
        "allowed_tools": ["read_workspace", "bounded_computation", "submit_artifact", "submit_candidate"],
        "forbidden_actions": ["write_domain_database", "write_fact_graph", "change_problem_contract", "claim_formal_verification_without_kernel_evidence"],
        "completion_contract": completion,
        "budget": {"max_minutes": budget.max_minutes_per_task, "max_model_calls": budget.max_model_calls_per_task},
        "checkpoint_policy": {"required_before_minutes": (budget.max_minutes_per_task / 2).max(1), "save_on_cancellation": true},
        "retry_policy": {"max_attempts": 3, "same_failure_signature_retries": 1},
        "fallback_policy": {"on_schema_error":"one repair then new session", "on_crash":"resume_checkpoint_then_backup_backend", "on_budget":"partial_or_unknown"},
        "route_cancellation_epoch": route.cancellation_epoch,
        "content_hash": "",
        "created_at": now,
    });
    let contract_hash = sha256_json(&contract_value)?;
    contract_value["content_hash"] = json!(contract_hash);
    sqlx::query("INSERT INTO task_contracts(task_contract_id,project_id,task_id,plan_revision_id,contract_version,contract_json,content_hash,created_at) VALUES(?,?,?,?,1,?,?,?)")
        .bind(&task_contract_id)
        .bind(project_id)
        .bind(&task_id)
        .bind(plan_revision_id)
        .bind(json_text(&contract_value)?)
        .bind(&contract_hash)
        .bind(now.to_rfc3339())
        .execute(&mut **tx)
        .await?;
    if let Some(id) = &bottleneck_id {
        sqlx::query("UPDATE bottlenecks SET attempted_task_ids_json=CASE WHEN EXISTS (SELECT 1 FROM json_each(attempted_task_ids_json) WHERE value=?) THEN attempted_task_ids_json ELSE json_insert(attempted_task_ids_json,'$[#]',?) END,updated_revision=?,updated_at=? WHERE bottleneck_id=?")
            .bind(&task_id)
            .bind(&task_id)
            .bind(source_revision)
            .bind(now.to_rfc3339())
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(Some(MaterializedV2Task {
        task: Task {
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
            round: round_number,
            result_summary: None,
            plan_revision_id: Some(plan_revision_id.into()),
            task_signature: Some(signature),
            context_packet_id: Some(packet_id),
        },
        worker: Worker {
            worker_id,
            project_id: project_id.into(),
            role: assignment.worker_role.clone(),
            backend: "codex_cli".into(),
            status: WorkerStatus::Ready,
            current_task_id: Some(task_id),
            current_route_id: Some(route.route_id.clone()),
            session_id: None,
            last_heartbeat: Some(*now),
        },
        task_contract_id,
    }))
}
