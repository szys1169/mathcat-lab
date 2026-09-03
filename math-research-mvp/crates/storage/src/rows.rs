use std::str::FromStr;

use chrono::{DateTime, Utc};
use research_domain::{
    Artifact, Candidate, CandidateStatus, ExperimentCapsule, Fact, FactStatus, Goal, GoalStatus,
    HumanCommand, Hypothesis, Project, ProjectStatus, ResearchRound, Route, RouteStatus,
    SourceRecord, Task, TaskStatus, Uncertainty, UncertaintyStatus, Verification, Worker,
    WorkerStatus,
};
use serde::de::DeserializeOwned;
use sqlx::{Row, sqlite::SqliteRow};

use crate::{StorageError, StorageResult};

pub(crate) fn json<T: DeserializeOwned>(row: &SqliteRow, column: &str) -> StorageResult<T> {
    Ok(serde_json::from_str(row.try_get(column)?)?)
}

pub(crate) fn timestamp(value: &str) -> StorageResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|v| v.with_timezone(&Utc))
        .map_err(|error| StorageError::CorruptData(error.to_string()))
}

pub(crate) fn optional_timestamp(value: Option<String>) -> StorageResult<Option<DateTime<Utc>>> {
    value.as_deref().map(timestamp).transpose()
}

pub(crate) fn experiment_capsule(row: &SqliteRow) -> StorageResult<ExperimentCapsule> {
    Ok(ExperimentCapsule {
        capsule_id: row.try_get("capsule_id")?,
        project_id: row.try_get("project_id")?,
        task_id: row.try_get("task_id")?,
        route_id: row.try_get("route_id")?,
        language: row.try_get("language")?,
        program_text: row.try_get("program_text")?,
        input: json(row, "input_json")?,
        environment: json(row, "environment_json")?,
        stdout: row.try_get("stdout")?,
        stderr: row.try_get("stderr")?,
        exit_code: row.try_get("exit_code")?,
        artifacts: json(row, "artifacts_json")?,
        conclusion_mapping: json(row, "conclusion_mapping_json")?,
        replay_command: json(row, "replay_command_json")?,
        status: row.try_get("status")?,
        content_hash: row.try_get("content_hash")?,
        created_at: timestamp(row.try_get("created_at")?)?,
        replayed_at: optional_timestamp(row.try_get("replayed_at")?)?,
    })
}

pub(crate) fn project(row: &SqliteRow) -> StorageResult<Project> {
    Ok(Project {
        project_id: row.try_get("project_id")?,
        name: row.try_get("name")?,
        contract: json(row, "problem_contract_json")?,
        status: ProjectStatus::from_str(row.try_get("status")?)
            .map_err(StorageError::CorruptData)?,
        revision: row.try_get("revision")?,
        current_round: row.try_get("current_round")?,
        budget: json(row, "budget_json")?,
        created_at: timestamp(row.try_get("created_at")?)?,
        updated_at: timestamp(row.try_get("updated_at")?)?,
    })
}

pub(crate) fn round(row: &SqliteRow) -> StorageResult<ResearchRound> {
    Ok(ResearchRound {
        round_id: row.try_get("round_id")?,
        project_id: row.try_get("project_id")?,
        number: row.try_get("number")?,
        status: research_domain::RoundStatus::from_str(row.try_get("status")?)
            .map_err(StorageError::CorruptData)?,
        based_on_revision: row.try_get("based_on_revision")?,
        started_at: optional_timestamp(row.try_get("started_at")?)?,
        completed_at: optional_timestamp(row.try_get("completed_at")?)?,
        summary: row.try_get("summary")?,
    })
}

pub(crate) fn route(row: &SqliteRow) -> StorageResult<Route> {
    Ok(Route {
        route_id: row.try_get("route_id")?,
        project_id: row.try_get("project_id")?,
        title: row.try_get("title")?,
        method_summary: row.try_get("method_summary")?,
        target_goal_ids: json(row, "target_goal_ids_json")?,
        required_fact_ids: json(row, "required_fact_ids_json")?,
        status: RouteStatus::from_str(row.try_get("status")?).map_err(StorageError::CorruptData)?,
        score: row.try_get("score")?,
        priority: row.try_get("priority")?,
        cancellation_epoch: row.try_get("cancellation_epoch")?,
        created_in_round: row.try_get("created_in_round")?,
        family_id: row.try_get("family_id")?,
        semantic_fingerprint: row.try_get("semantic_fingerprint")?,
        consecutive_no_progress_plans: row.try_get("consecutive_no_progress_plans")?,
        failed_attempt_count: row.try_get("failed_attempt_count")?,
        created_at_revision: row.try_get("created_at_revision")?,
        last_material_progress_revision: row.try_get("last_material_progress_revision")?,
        merged_into: row.try_get("merged_into")?,
        merge_reason: row.try_get("merge_reason")?,
        exit_criteria: json(row, "exit_criteria_json")?,
        attributes: json(row, "attributes_json")?,
    })
}

pub(crate) fn task(row: &SqliteRow) -> StorageResult<Task> {
    Ok(Task {
        task_id: row.try_get("task_id")?,
        project_id: row.try_get("project_id")?,
        route_id: row.try_get("route_id")?,
        worker_id: row.try_get("worker_id")?,
        worker_role: row.try_get("worker_role")?,
        goal_ids: json(row, "goal_ids_json")?,
        objective: row.try_get("objective")?,
        completion_contract: row.try_get("completion_contract")?,
        status: TaskStatus::from_str(row.try_get("status")?).map_err(StorageError::CorruptData)?,
        priority: row.try_get("priority")?,
        revision: row.try_get("revision")?,
        route_cancellation_epoch: row.try_get("route_cancellation_epoch")?,
        round: row.try_get("round")?,
        result_summary: row.try_get("result_summary")?,
        plan_revision_id: row.try_get("plan_revision_id")?,
        task_signature: row.try_get("task_signature")?,
        context_packet_id: row.try_get("context_packet_id")?,
    })
}

pub(crate) fn worker(row: &SqliteRow) -> StorageResult<Worker> {
    Ok(Worker {
        worker_id: row.try_get("worker_id")?,
        project_id: row.try_get("project_id")?,
        role: row.try_get("role")?,
        backend: row.try_get("backend")?,
        status: WorkerStatus::from_str(row.try_get("status")?)
            .map_err(StorageError::CorruptData)?,
        current_task_id: row.try_get("current_task_id")?,
        current_route_id: row.try_get("current_route_id")?,
        session_id: row.try_get("session_id")?,
        last_heartbeat: optional_timestamp(row.try_get("last_heartbeat")?)?,
    })
}

pub(crate) fn goal(row: &SqliteRow) -> StorageResult<Goal> {
    Ok(Goal {
        goal_id: row.try_get("goal_id")?,
        project_id: row.try_get("project_id")?,
        statement: row.try_get("statement")?,
        parent_goal_ids: json(row, "parent_goal_ids_json")?,
        status: GoalStatus::from_str(row.try_get("status")?).map_err(StorageError::CorruptData)?,
        priority: row.try_get("priority")?,
        blocked_by: json(row, "blocked_by_json")?,
        solved_by_fact_id: row.try_get("solved_by_fact_id")?,
        created_in_round: row.try_get("created_in_round")?,
    })
}

pub(crate) fn hypothesis(row: &SqliteRow) -> StorageResult<Hypothesis> {
    Ok(Hypothesis {
        hypothesis_id: row.try_get("hypothesis_id")?,
        project_id: row.try_get("project_id")?,
        kind: row.try_get("kind")?,
        statement: row.try_get("statement")?,
        status: row.try_get("status")?,
        route_id: row.try_get("route_id")?,
        promoted_to_fact_id: row.try_get("promoted_to_fact_id")?,
        created_in_round: row.try_get("created_in_round")?,
        attributes: json(row, "attributes_json")?,
    })
}

pub(crate) fn fact(row: &SqliteRow) -> StorageResult<Fact> {
    Ok(Fact {
        fact_id: row.try_get("fact_id")?,
        project_id: row.try_get("project_id")?,
        statement: row.try_get("statement")?,
        assumptions: json(row, "assumptions_json")?,
        proof_markdown: row.try_get("proof_markdown")?,
        dependency_fact_ids: json(row, "dependency_fact_ids_json")?,
        definitions_introduced: json(row, "definitions_introduced_json")?,
        external_source_ids: json(row, "external_source_ids_json")?,
        verification_ids: json(row, "verification_ids_json")?,
        evidence_level: row.try_get("evidence_level")?,
        created_by: row.try_get("created_by")?,
        status: FactStatus::from_str(row.try_get("status")?).map_err(StorageError::CorruptData)?,
        content_hash: row.try_get("content_hash")?,
        created_at: timestamp(row.try_get("created_at")?)?,
    })
}

pub(crate) fn candidate(row: &SqliteRow) -> StorageResult<Candidate> {
    Ok(Candidate {
        candidate_id: row.try_get("candidate_id")?,
        project_id: row.try_get("project_id")?,
        submission: json(row, "submission_json")?,
        status: CandidateStatus::from_str(row.try_get("status")?)
            .map_err(StorageError::CorruptData)?,
        created_at: timestamp(row.try_get("created_at")?)?,
    })
}

pub(crate) fn verification(row: &SqliteRow) -> StorageResult<Verification> {
    let report = row
        .try_get::<Option<String>, _>("report_json")?
        .map(|value| serde_json::from_str(&value))
        .transpose()?;
    Ok(Verification {
        verification_id: row.try_get("verification_id")?,
        candidate_id: row.try_get("candidate_id")?,
        project_id: row.try_get("project_id")?,
        status: CandidateStatus::from_str(row.try_get("status")?)
            .map_err(StorageError::CorruptData)?,
        report,
        started_at: optional_timestamp(row.try_get("started_at")?)?,
        completed_at: optional_timestamp(row.try_get("completed_at")?)?,
    })
}

pub(crate) fn uncertainty(row: &SqliteRow) -> StorageResult<Uncertainty> {
    Ok(Uncertainty {
        uncertainty_id: row.try_get("uncertainty_id")?,
        project_id: row.try_get("project_id")?,
        description: row.try_get("description")?,
        uncertainty_type: row.try_get("type")?,
        severity: row.try_get("severity")?,
        affects_goal_ids: json(row, "affects_goal_ids_json")?,
        affects_route_ids: json(row, "affects_route_ids_json")?,
        introduced_by: row.try_get("introduced_by")?,
        resolution_methods: json(row, "resolution_methods_json")?,
        status: UncertaintyStatus::from_str(row.try_get("status")?)
            .map_err(StorageError::CorruptData)?,
        resolved_by: row.try_get("resolved_by")?,
        created_at: timestamp(row.try_get("created_at")?)?,
    })
}

pub(crate) fn source(row: &SqliteRow) -> StorageResult<SourceRecord> {
    Ok(SourceRecord {
        source_id: row.try_get("source_id")?,
        project_id: row.try_get("project_id")?,
        title: row.try_get("title")?,
        authors: json(row, "authors_json")?,
        url: row.try_get("url")?,
        normalized_url: row.try_get("normalized_url")?,
        identifier_kind: row.try_get("identifier_kind")?,
        identifier_value: row.try_get("identifier_value")?,
        citation_key: row.try_get("citation_key")?,
        theorem_reference: row.try_get("theorem_reference")?,
        statement_excerpt: row.try_get("statement_excerpt")?,
        assumptions: json(row, "assumptions_json")?,
        applicability: row.try_get("applicability")?,
        status: row.try_get("status")?,
        origin_task_id: row.try_get("origin_task_id")?,
        origin_route_id: row.try_get("origin_route_id")?,
        fulltext_artifact_id: row.try_get("fulltext_artifact_id")?,
        provenance: json(row, "provenance_json")?,
        disposition_reason: row.try_get("disposition_reason")?,
        retrieved_at: timestamp(row.try_get("retrieved_at")?)?,
    })
}

pub(crate) fn command(row: &SqliteRow) -> StorageResult<HumanCommand> {
    Ok(HumanCommand {
        command_id: row.try_get("command_id")?,
        project_id: row.try_get("project_id")?,
        command_type: row.try_get("type")?,
        target_kind: row.try_get("target_kind")?,
        target_id: row.try_get("target_id")?,
        mode: research_domain::CommandMode::from_str(row.try_get("mode")?)
            .map_err(StorageError::CorruptData)?,
        payload: json(row, "payload_json")?,
        expected_project_revision: row.try_get("expected_project_revision")?,
        idempotency_key: row.try_get("idempotency_key")?,
        reason: row.try_get("reason")?,
        requested_by: row.try_get("requested_by")?,
        status: research_domain::CommandStatus::from_str(row.try_get("status")?)
            .map_err(StorageError::CorruptData)?,
        before_revision: row.try_get("before_revision")?,
        after_revision: row.try_get("after_revision")?,
        affected_entities: json(row, "affected_entities_json")?,
        error: row.try_get("error")?,
        created_at: timestamp(row.try_get("created_at")?)?,
        applied_at: optional_timestamp(row.try_get("applied_at")?)?,
    })
}

pub(crate) fn artifact(row: &SqliteRow) -> StorageResult<Artifact> {
    Ok(Artifact {
        artifact_id: row.try_get("artifact_id")?,
        project_id: row.try_get("project_id")?,
        kind: row.try_get("kind")?,
        media_type: row.try_get("media_type")?,
        filename: row.try_get("filename")?,
        size: row.try_get("size")?,
        sha256: row.try_get("sha256")?,
        created_in_round: row.try_get("created_in_round")?,
        related_entity_ids: json(row, "related_entity_ids_json")?,
        created_at: timestamp(row.try_get("created_at")?)?,
        storage_path: row.try_get("storage_path")?,
    })
}
