// The router and generated contract intentionally enumerate the complete P0 surface in one place.
#![allow(clippy::too_many_arguments, clippy::too_many_lines)]
#![recursion_limit = "256"]

use std::{convert::Infallible, path::PathBuf};

use async_stream::stream;
use axum::{
    Json, Router,
    body::Body,
    extract::{
        Path, Query, Request, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use research_core::{CoreError, ResearchService};
use research_domain::{
    BoardCapabilities, Budget, CandidateSubmission, CommandMode, HumanRouteProposalRequest,
    ProblemContract, ProblemRevisionRequest,
};
use research_storage::{BoardInclude, CommandDraft, SqliteStore, StorageError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_stream::Stream;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use ulid::Ulid;
use utoipa::ToSchema;

#[derive(Clone)]
pub struct AppState {
    pub service: ResearchService,
}

pub fn router(service: ResearchService) -> Router {
    let state = AppState { service };
    Router::new()
        .route("/health", get(health))
        .route("/api/openapi.json", get(openapi))
        .route("/api/v1/actors/bootstrap", post(bootstrap_actor))
        .route("/api/v1/actors", post(create_actor))
        .route("/api/v1/worker-nodes", post(register_worker_node))
        .route(
            "/api/v1/worker-nodes/{node_id}/heartbeat",
            post(heartbeat_worker_node),
        )
        .route(
            "/api/v1/projects/{project_id}/distributed/leases/next",
            post(lease_distributed_task),
        )
        .route("/api/v1/task-leases/{lease_id}", get(get_task_lease))
        .route(
            "/api/v1/task-leases/{lease_id}/renew",
            post(renew_task_lease),
        )
        .route(
            "/api/v1/task-leases/{lease_id}/complete",
            post(complete_task_lease),
        )
        .route("/api/v1/projects", post(create_project))
        .route("/api/v1/projects/{project_id}", get(get_project))
        .route("/api/v1/projects/{project_id}/status", get(project_status))
        .route("/api/v1/projects/{project_id}/snapshot", get(snapshot))
        .route("/api/v1/projects/{project_id}/board", get(board))
        .route(
            "/api/v1/projects/{project_id}/problem-revisions",
            post(revise_problem),
        )
        .route(
            "/api/v1/projects/{project_id}/route-proposals",
            get(route_proposals).post(propose_route),
        )
        .route("/api/v1/projects/{project_id}/latest", get(latest))
        .route(
            "/api/v1/projects/{project_id}/publications",
            get(publications).post(create_publication),
        )
        .route(
            "/api/v1/publications/{publication_id}",
            get(publication),
        )
        .route(
            "/api/v1/projects/{project_id}/goals/{goal_id}/closures",
            get(goal_closures),
        )
        .route("/api/v1/projects/{project_id}/rounds", get(rounds))
        .route(
            "/api/v1/projects/{project_id}/rounds/current",
            get(current_round),
        )
        .route("/api/v1/projects/{project_id}/workers", get(workers))
        .route(
            "/api/v1/projects/{project_id}/workers/{worker_id}",
            get(worker),
        )
        .route(
            "/api/v1/projects/{project_id}/tasks",
            get(tasks).post(create_human_task),
        )
        .route("/api/v1/projects/{project_id}/tasks/{task_id}", get(task))
        .route(
            "/api/v1/projects/{project_id}/tasks/{task_id}/attempts",
            get(task_attempts),
        )
        .route(
            "/api/v1/projects/{project_id}/tasks/{task_id}/contract",
            get(task_contract),
        )
        .route(
            "/api/v1/tasks/{task_id}/context-packet",
            get(task_context_packet),
        )
        .route(
            "/api/v1/projects/{project_id}/tasks/{task_id}/steers",
            get(task_steers),
        )
        .route(
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/steer",
            post(steer_task),
        )
        .route(
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/pause",
            post(pause_task),
        )
        .route(
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/resume",
            post(resume_task),
        )
        .route(
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/reassign",
            post(reassign_task),
        )
        .route(
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/cancel",
            post(cancel_task),
        )
        .route(
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/set-priority",
            post(set_task_priority),
        )
        .route(
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/retry",
            post(retry_task),
        )
        .route(
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/rebuild-context",
            post(rebuild_task_context),
        )
        .route(
            "/api/v1/tasks/{task_id}/commands/rebuild-context",
            post(rebuild_task_context_by_task),
        )
        .route("/api/v1/projects/{project_id}/routes", get(routes))
        .route(
            "/api/v1/projects/{project_id}/routes/{route_id}/commands/approve",
            post(approve_route),
        )
        .route(
            "/api/v1/projects/{project_id}/route-families",
            get(route_families),
        )
        .route(
            "/api/v1/projects/{project_id}/route-tombstones",
            get(route_tombstones),
        )
        .route(
            "/api/v1/projects/{project_id}/routes/{route_id}",
            get(route),
        )
        .route(
            "/api/v1/projects/{project_id}/routes/{route_id}/progress-ledger",
            get(route_progress_ledger),
        )
        .route(
            "/api/v1/routes/{route_id}/progress-digest",
            get(route_progress_digest),
        )
        .route(
            "/api/v1/projects/{project_id}/worker-instances",
            get(worker_instances),
        )
        .route(
            "/api/v1/projects/{project_id}/planning/delta",
            get(planning_delta),
        )
        .route(
            "/api/v1/projects/{project_id}/planning/revisions",
            get(planning_revisions),
        )
        .route(
            "/api/v1/projects/{project_id}/planning/revisions/{plan_revision_id}",
            get(planning_revision),
        )
        .route(
            "/api/v1/projects/{project_id}/strategy/states",
            get(strategy_states),
        )
        .route(
            "/api/v1/projects/{project_id}/strategy/latest",
            get(latest_strategy_state),
        )
        .route(
            "/api/v1/projects/{project_id}/bottlenecks",
            get(bottlenecks),
        )
        .route(
            "/api/v1/projects/{project_id}/facts/{fact_id}/planning-impact",
            get(fact_planning_impact),
        )
        .route(
            "/api/v1/projects/{project_id}/planner/health",
            get(planner_health),
        )
        .route(
            "/api/v1/projects/{project_id}/context/digest",
            get(context_digest),
        )
        .route("/api/v1/system/storage/health", get(storage_health))
        .route(
            "/api/v1/system/state-writer/status",
            get(state_writer_status),
        )
        .route(
            "/api/v1/system/reconciliation/commands/run",
            post(run_reconciliation),
        )
        .route(
            "/api/v1/projects/{project_id}/graphs/{graph_type}",
            get(graph),
        )
        .route(
            "/api/v1/projects/{project_id}/graphs/combined/delta",
            get(graph_delta),
        )
        .route(
            "/api/v1/projects/{project_id}/verifications",
            get(verifications),
        )
        .route("/api/v1/verifications/{verification_id}", get(verification))
        .route(
            "/api/v1/verifications/{verification_id}/case",
            get(verification_case_by_verification),
        )
        .route(
            "/api/v1/verification-cases/{case_id}",
            get(verification_case),
        )
        .route(
            "/api/v1/verification-cases/{case_id}/snapshot",
            get(verification_snapshot),
        )
        .route(
            "/api/v1/verification-cases/{case_id}/checks",
            get(verification_checks),
        )
        .route(
            "/api/v1/verification-cases/{case_id}/findings",
            get(verification_findings),
        )
        .route(
            "/api/v1/verification-cases/{case_id}/evidence",
            get(verification_evidence),
        )
        .route(
            "/api/v1/verification-cases/{case_id}/semantic-contract",
            get(semantic_contract),
        )
        .route(
            "/api/v1/verification-cases/{case_id}/formalization",
            get(formalization),
        )
        .route(
            "/api/v1/verification-cases/{case_id}/alignment-reviews",
            get(alignment_reviews),
        )
        .route(
            "/api/v1/verification-cases/{case_id}/backend-runs",
            get(backend_runs),
        )
        .route(
            "/api/v1/verification-cases/{case_id}/package",
            get(verification_package),
        )
        .route(
            "/api/v1/verification-cases/{case_id}/replays",
            get(verification_replays),
        )
        .route(
            "/api/v1/formalizations/{formalization_id}",
            get(get_formalization),
        )
        .route(
            "/api/v1/formalizations/{formalization_id}/semantic-contract",
            get(formalization_semantic_contract),
        )
        .route(
            "/api/v1/formalizations/{formalization_id}/alignment",
            get(formalization_alignment),
        )
        .route(
            "/api/v1/formalizations/{formalization_id}/goals",
            get(formalization_goals),
        )
        .route(
            "/api/v1/formalizations/{formalization_id}/proof-tree",
            get(formalization_proof_tree),
        )
        .route(
            "/api/v1/formalizations/{formalization_id}/attempts",
            get(formalization_attempts),
        )
        .route(
            "/api/v1/formalizations/{formalization_id}/hints",
            post(add_formalization_hint),
        )
        .route(
            "/api/v1/formalizations/{formalization_id}/proof-nodes/{node_id}/commands/prune",
            post(prune_formalization_branch),
        )
        .route(
            "/api/v1/formalizations/{formalization_id}/commands/cancel",
            post(cancel_formalization_search),
        )
        .route(
            "/api/v1/projects/{project_id}/uncertainties",
            get(uncertainties),
        )
        .route("/api/v1/projects/{project_id}/sources", get(sources))
        .route(
            "/api/v1/projects/{project_id}/sources/ingestions",
            get(source_ingestions),
        )
        .route(
            "/api/v1/projects/{project_id}/experiment-capsules",
            get(experiment_capsules),
        )
        .route(
            "/api/v1/experiment-capsules/{capsule_id}",
            get(experiment_capsule),
        )
        .route(
            "/api/v1/projects/{project_id}/failure-patterns",
            get(failure_patterns),
        )
        .route("/api/v1/facts/{fact_id}", get(fact))
        .route("/api/v1/facts/catalog", get(fact_catalog))
        .route(
            "/api/v1/projects/{project_id}/fact-imports",
            get(fact_imports).post(import_fact),
        )
        .route("/api/v1/facts/{fact_id}/assurance", get(fact_assurance))
        .route(
            "/api/v1/facts/{fact_id}/verification-history",
            get(fact_verification_history),
        )
        .route(
            "/api/v1/facts/{fact_id}/formalization",
            get(fact_formalization),
        )
        .route(
            "/api/v1/facts/{fact_id}/dependency-closure",
            get(fact_dependency_closure),
        )
        .route("/api/v1/facts/{fact_id}/impact", get(fact_impact))
        .route(
            "/api/v1/facts/{fact_id}/commands/challenge",
            post(challenge_fact),
        )
        .route(
            "/api/v1/facts/{fact_id}/commands/reverify",
            post(reverify_fact),
        )
        .route(
            "/api/v1/facts/{fact_id}/commands/request-formalization",
            post(request_fact_formalization),
        )
        .route(
            "/api/v1/facts/{fact_id}/commands/request-independent-proof",
            post(request_independent_proof),
        )
        .route(
            "/api/v1/facts/{fact_id}/commands/suspend",
            post(suspend_fact),
        )
        .route("/api/v1/facts/{fact_id}/commands/revoke", post(revoke_fact))
        .route("/api/v1/projects/{project_id}/events", get(events))
        .route("/api/v1/projects/{project_id}/ws", get(websocket_events))
        .route(
            "/api/v1/projects/{project_id}/security-audit",
            get(security_audit),
        )
        .route("/api/v1/projects/{project_id}/artifacts", get(artifacts))
        .route(
            "/api/v1/projects/{project_id}/artifacts/{artifact_id}",
            get(artifact),
        )
        .route(
            "/api/v1/projects/{project_id}/artifacts/{artifact_id}/content",
            get(artifact_content),
        )
        .route(
            "/api/v1/projects/{project_id}/reports/latest",
            get(latest_report_content),
        )
        .route(
            "/api/v1/projects/{project_id}/commands/{command_id}",
            get(command),
        )
        .route(
            "/api/v1/projects/{project_id}/commands/start",
            post(start_project),
        )
        .route(
            "/api/v1/projects/{project_id}/commands/pause",
            post(pause_project),
        )
        .route(
            "/api/v1/projects/{project_id}/commands/resume",
            post(resume_project),
        )
        .route(
            "/api/v1/projects/{project_id}/commands/stop",
            post(stop_project),
        )
        .route(
            "/api/v1/projects/{project_id}/commands/replan",
            post(replan),
        )
        .route(
            "/api/v1/projects/{project_id}/suggestions",
            post(add_suggestion),
        )
        .route(
            "/api/v1/projects/{project_id}/routes/{route_id}/commands/pause",
            post(pause_route),
        )
        .route(
            "/api/v1/projects/{project_id}/routes/{route_id}/commands/resume",
            post(resume_route),
        )
        .route(
            "/api/v1/projects/{project_id}/routes/{route_id}/commands/stop",
            post(stop_route),
        )
        .route(
            "/api/v1/projects/{project_id}/routes/{route_id}/commands/prune",
            post(prune_route),
        )
        .route(
            "/api/v1/projects/{project_id}/routes/commands/merge",
            post(merge_routes),
        )
        .route(
            "/api/v1/projects/{project_id}/routes/{route_id}/commands/revive",
            post(revive_route),
        )
        .route(
            "/api/v1/projects/{project_id}/workers/{worker_id}/commands/stop",
            post(stop_worker),
        )
        .route(
            "/api/v1/projects/{project_id}/workers/{worker_id}/commands/pause",
            post(pause_worker),
        )
        .route(
            "/api/v1/projects/{project_id}/workers/{worker_id}/commands/resume",
            post(resume_worker),
        )
        .route(
            "/api/v1/projects/{project_id}/worker-instances/{worker_instance_id}/commands/quarantine",
            post(quarantine_worker_instance),
        )
        .route("/api/v1/projects/{project_id}/usage", get(usage_summary))
        .route(
            "/api/v1/projects/{project_id}/budgets",
            get(budget_overrides),
        )
        .route(
            "/api/v1/projects/{project_id}/commands/adjust-budget",
            post(adjust_budget),
        )
        .route(
            "/api/v1/projects/{project_id}/questions",
            get(human_questions),
        )
        .route(
            "/api/v1/projects/{project_id}/questions/{question_id}/commands/answer",
            post(answer_question),
        )
        .route(
            "/api/v1/projects/{project_id}/candidates",
            post(submit_candidate),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            authentication_gate,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn authentication_gate(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let path = request.uri().path();
    let public_endpoint = path == "/health"
        || path == "/api/openapi.json"
        || path == "/api/v1/actors/bootstrap"
        || (path.starts_with("/api/v1/worker-nodes/") && path.ends_with("/heartbeat"))
        || (path.starts_with("/api/v1/projects/") && path.ends_with("/distributed/leases/next"))
        || (path.starts_with("/api/v1/task-leases/")
            && (path.ends_with("/renew") || path.ends_with("/complete")));
    if request.method() != Method::OPTIONS && !public_endpoint {
        authorize_actor(
            state.service.store(),
            request.headers(),
            None,
            "api_access",
            "http_path",
            path,
            "viewer",
        )
        .await?;
    }
    Ok(next.run(request).await)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiEnvelope<T: Serialize> {
    pub data: Option<T>,
    pub meta: ResponseMeta,
    pub error: Option<ApiErrorBody>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ResponseMeta {
    pub request_id: String,
    pub project_revision: Option<i64>,
    pub event_cursor: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
    pub details: Value,
    pub retryable: bool,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    details: Value,
    retryable: bool,
}

impl ApiError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message: message.into(),
            details: json!({}),
            retryable: false,
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "credentials_required",
            message: message.into(),
            details: json!({}),
            retryable: false,
        }
    }
}

impl From<StorageError> for ApiError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::NotFound { kind, id } => Self {
                status: StatusCode::NOT_FOUND,
                code: "entity_not_found",
                message: format!("{kind} {id} was not found"),
                details: json!({"kind":kind,"id":id}),
                retryable: false,
            },
            StorageError::RevisionConflict { expected, actual } => Self {
                status: StatusCode::CONFLICT,
                code: "stale_project_revision",
                message: "project state changed after the client snapshot".into(),
                details: json!({"expected_revision":expected,"actual_revision":actual}),
                retryable: true,
            },
            StorageError::IdempotencyConflict(message) => Self {
                status: StatusCode::CONFLICT,
                code: "idempotency_conflict",
                message,
                details: json!({}),
                retryable: false,
            },
            StorageError::InvalidProblemRevision(message) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_problem_revision",
                message,
                details: json!({}),
                retryable: false,
            },
            StorageError::InvalidRouteProposal(message) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_route_proposal",
                message,
                details: json!({}),
                retryable: false,
            },
            StorageError::LateSubmission(message) => Self {
                status: StatusCode::CONFLICT,
                code: "stale_worker_submission",
                message,
                details: json!({}),
                retryable: false,
            },
            StorageError::InvalidDependency(id) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_fact_dependency",
                message: format!("fact dependency is missing or inactive: {id}"),
                details: json!({"fact_id":id}),
                retryable: false,
            },
            StorageError::InvalidTransition(message) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "invalid_transition",
                message,
                details: json!({}),
                retryable: false,
            },
            StorageError::DependencyCycle => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "fact_dependency_cycle",
                message: "fact dependency cycle detected".into(),
                details: json!({}),
                retryable: false,
            },
            StorageError::BudgetExhausted(message) => Self {
                status: StatusCode::TOO_MANY_REQUESTS,
                code: "model_call_budget_exhausted",
                message,
                details: json!({}),
                retryable: false,
            },
            other => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "storage_error",
                message: other.to_string(),
                details: json!({}),
                retryable: true,
            },
        }
    }
}

impl From<CoreError> for ApiError {
    fn from(error: CoreError) -> Self {
        match error {
            CoreError::Storage(error) => error.into(),
            CoreError::Agent(error) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "runtime_unavailable",
                message: error.to_string(),
                details: json!({}),
                retryable: true,
            },
            CoreError::InvalidAgentOutput(message) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "invalid_agent_output",
                message,
                details: json!({}),
                retryable: true,
            },
            other => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "internal_error",
                message: other.to_string(),
                details: json!({}),
                retryable: false,
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let envelope = ApiEnvelope::<Value> {
            data: None,
            meta: ResponseMeta {
                request_id: request_id(),
                project_revision: None,
                event_cursor: None,
            },
            error: Some(ApiErrorBody {
                code: self.code.into(),
                message: self.message,
                details: self.details,
                retryable: self.retryable,
            }),
        };
        (self.status, Json(envelope)).into_response()
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateProjectRequest {
    pub name: String,
    pub problem: String,
    pub target_statement: Option<String>,
    #[serde(default)]
    pub assumptions: Vec<String>,
    pub success_criteria: Option<String>,
    #[serde(default)]
    pub budget: Budget,
    #[serde(default)]
    pub human_route_approval: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreatePublicationRequest {
    #[serde(default)]
    pub allow_partial: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BootstrapActorRequest {
    pub actor_id: String,
    pub display_name: String,
    pub token: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateActorRequest {
    pub actor_id: String,
    pub display_name: String,
    pub role: String,
    pub token: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ImportFactRequest {
    pub content_hash: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RegisterWorkerNodeRequest {
    pub node_id: String,
    pub display_name: String,
    #[schema(value_type = Object)]
    pub capabilities: Value,
    pub token: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct NodeHeartbeatRequest {
    pub node_epoch: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LeaseTaskRequest {
    pub node_id: String,
    pub node_epoch: i64,
    #[serde(default = "default_lease_ttl")]
    pub ttl_seconds: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RenewLeaseRequest {
    pub node_id: String,
    pub node_epoch: i64,
    pub lease_epoch: i64,
    #[serde(default = "default_lease_ttl")]
    pub ttl_seconds: u64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CompleteLeaseRequest {
    pub node_id: String,
    pub node_epoch: i64,
    pub lease_epoch: i64,
    pub output: research_domain::WorkerOutput,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ReconciliationRequest {
    pub project_id: Option<String>,
    #[serde(default = "default_reconciliation_trigger")]
    pub trigger_kind: String,
}

fn default_reconciliation_trigger() -> String {
    "admin_api".into()
}

const fn default_lease_ttl() -> u64 {
    120
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CommandRequest {
    pub expected_revision: i64,
    #[serde(default)]
    pub reason: String,
    #[serde(default = "empty_object")]
    #[schema(value_type = Object)]
    pub payload: Value,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SuggestionRequest {
    pub expected_revision: i64,
    pub content: String,
    pub target_route_id: Option<String>,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Deserialize)]
pub struct EventQuery {
    #[serde(default)]
    pub after: i64,
}

#[derive(Debug, Deserialize)]
pub struct BoardQuery {
    #[serde(default = "default_timeline_limit")]
    pub timeline_limit: i64,
    #[serde(default = "default_board_include")]
    pub include: String,
    #[serde(default = "default_board_schema_version")]
    pub schema_version: i64,
}

const fn default_timeline_limit() -> i64 {
    100
}

fn default_board_include() -> String {
    "summary".into()
}

const fn default_board_schema_version() -> i64 {
    1
}

#[derive(Debug, Deserialize)]
pub struct TaskQuery {
    pub status: Option<String>,
    pub route_id: Option<String>,
    pub worker_id: Option<String>,
}

fn empty_object() -> Value {
    json!({})
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({"status":"ok","backend":state.service.backend_name()}))
}

async fn openapi() -> Json<Value> {
    Json(openapi_document())
}

async fn bootstrap_actor(
    State(state): State<AppState>,
    Json(body): Json<BootstrapActorRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<research_domain::Actor>>), ApiError> {
    let actor = state
        .service
        .store()
        .bootstrap_admin(&body.actor_id, &body.display_name, &body.token)
        .await?;
    state
        .service
        .store()
        .record_security_audit(
            None,
            Some(&actor.actor_id),
            "bootstrap_actor",
            "actor",
            &actor.actor_id,
            "allowed",
            Some("first local administrator"),
            None,
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiEnvelope {
            data: Some(actor),
            meta: ResponseMeta {
                request_id: request_id(),
                project_revision: None,
                event_cursor: None,
            },
            error: None,
        }),
    ))
}

async fn create_actor(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateActorRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<research_domain::Actor>>), ApiError> {
    let actor = authorize_actor(
        state.service.store(),
        &headers,
        None,
        "create_actor",
        "actor",
        &body.actor_id,
        "admin",
    )
    .await?;
    let created = state
        .service
        .store()
        .create_actor(&body.actor_id, &body.display_name, &body.role, &body.token)
        .await?;
    state
        .service
        .store()
        .record_security_audit(
            None,
            Some(&actor.actor_id),
            "create_actor",
            "actor",
            &body.actor_id,
            "allowed",
            None,
            None,
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiEnvelope {
            data: Some(created),
            meta: ResponseMeta {
                request_id: request_id(),
                project_revision: None,
                event_cursor: None,
            },
            error: None,
        }),
    ))
}

async fn register_worker_node(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RegisterWorkerNodeRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<research_domain::WorkerNode>>), ApiError> {
    let actor = authorize_actor(
        state.service.store(),
        &headers,
        None,
        "register_worker_node",
        "worker_node",
        &body.node_id,
        "admin",
    )
    .await?;
    let node = state
        .service
        .store()
        .register_worker_node(
            &body.node_id,
            &body.display_name,
            body.capabilities,
            &body.token,
        )
        .await?;
    state
        .service
        .store()
        .record_security_audit(
            None,
            Some(&actor.actor_id),
            "register_worker_node",
            "worker_node",
            &node.node_id,
            "allowed",
            None,
            None,
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiEnvelope {
            data: Some(node),
            meta: ResponseMeta {
                request_id: request_id(),
                project_revision: None,
                event_cursor: None,
            },
            error: None,
        }),
    ))
}

async fn heartbeat_worker_node(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<NodeHeartbeatRequest>,
) -> Result<Json<ApiEnvelope<research_domain::WorkerNode>>, ApiError> {
    let token = worker_node_token(&headers)?;
    let node = state
        .service
        .store()
        .heartbeat_worker_node(&node_id, token, body.node_epoch)
        .await?;
    Ok(Json(ApiEnvelope {
        data: Some(node),
        meta: ResponseMeta {
            request_id: request_id(),
            project_revision: None,
            event_cursor: None,
        },
        error: None,
    }))
}

async fn lease_distributed_task(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<LeaseTaskRequest>,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let token = worker_node_token(&headers)?;
    let leased = state
        .service
        .lease_next_distributed_task(
            &project_id,
            &body.node_id,
            token,
            body.node_epoch,
            body.ttl_seconds,
        )
        .await?;
    Ok(Json(
        success(&state.service, &project_id, json!({"leased":leased})).await?,
    ))
}

async fn get_task_lease(
    State(state): State<AppState>,
    Path(lease_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ApiEnvelope<research_domain::TaskLease>>, ApiError> {
    let lease = state.service.store().get_task_lease(&lease_id).await?;
    authorize_actor(
        state.service.store(),
        &headers,
        Some(&lease.project_id),
        "get_task_lease",
        "task_lease",
        &lease_id,
        "viewer",
    )
    .await?;
    let project_id = lease.project_id.clone();
    Ok(Json(success(&state.service, &project_id, lease).await?))
}

async fn renew_task_lease(
    State(state): State<AppState>,
    Path(lease_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<RenewLeaseRequest>,
) -> Result<Json<ApiEnvelope<research_domain::TaskLease>>, ApiError> {
    let token = worker_node_token(&headers)?;
    let lease = state
        .service
        .store()
        .renew_task_lease(
            &lease_id,
            &body.node_id,
            token,
            body.node_epoch,
            body.lease_epoch,
            body.ttl_seconds,
        )
        .await?;
    let project_id = lease.project_id.clone();
    Ok(Json(success(&state.service, &project_id, lease).await?))
}

async fn complete_task_lease(
    State(state): State<AppState>,
    Path(lease_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CompleteLeaseRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<Value>>), ApiError> {
    let token = worker_node_token(&headers)?;
    let lease = state.service.store().get_task_lease(&lease_id).await?;
    state
        .service
        .complete_distributed_task(
            &lease_id,
            &body.node_id,
            token,
            body.node_epoch,
            body.lease_epoch,
            &body.output,
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(
            success(
                &state.service,
                &lease.project_id,
                json!({"lease_id":lease_id,"status":"completed"}),
            )
            .await?,
        ),
    ))
}

async fn create_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateProjectRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = authorize_actor(
        state.service.store(),
        &headers,
        None,
        "create_project",
        "project",
        "new",
        "researcher",
    )
    .await?;
    let target = body
        .target_statement
        .unwrap_or_else(|| body.problem.clone());
    let contract = ProblemContract {
        original_problem: body.problem,
        target_statement: target,
        assumptions: body.assumptions,
        success_criteria: body.success_criteria.unwrap_or_else(|| {
            "目标陈述得到 accepted 裁决，依赖闭包均为 active，且无阻塞不确定性".into()
        }),
        version: 1,
    };
    let project = state
        .service
        .create_project_with_route_approval(
            body.name,
            contract,
            body.budget,
            body.human_route_approval,
        )
        .await?;
    let project_id = project.project_id.clone();
    state
        .service
        .store()
        .record_security_audit(
            Some(&project_id),
            Some(&actor.actor_id),
            "create_project",
            "project",
            &project_id,
            "allowed",
            None,
            None,
        )
        .await?;
    let response = success(&state.service, &project_id, project).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

async fn create_publication(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreatePublicationRequest>,
) -> Result<
    (
        StatusCode,
        Json<ApiEnvelope<research_core::PublicationResult>>,
    ),
    ApiError,
> {
    let key = idempotency_key(&headers)?;
    let actor = authorize_actor(
        state.service.store(),
        &headers,
        Some(&project_id),
        "create_publication",
        "project",
        &project_id,
        "researcher",
    )
    .await?;
    let result = state
        .service
        .publish_paper_idempotent(&project_id, body.allow_partial, &key)
        .await?;
    state
        .service
        .store()
        .record_security_audit(
            Some(&project_id),
            Some(&actor.actor_id),
            "create_publication",
            "project",
            &project_id,
            "allowed",
            None,
            Some(&key),
        )
        .await?;
    Ok((
        StatusCode::OK,
        Json(success(&state.service, &project_id, result).await?),
    ))
}

async fn publications(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::PublicationRun>>>, ApiError> {
    let runs = state.service.store().list_publications(&project_id).await?;
    Ok(Json(success(&state.service, &project_id, runs).await?))
}

async fn publication(
    State(state): State<AppState>,
    Path(publication_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::PublicationRun>>, ApiError> {
    let run = state
        .service
        .store()
        .get_publication(&publication_id)
        .await?;
    let project_id = run.project_id.clone();
    Ok(Json(success(&state.service, &project_id, run).await?))
}

async fn goal_closures(
    State(state): State<AppState>,
    Path((project_id, goal_id)): Path<(String, String)>,
) -> Result<Json<ApiEnvelope<Vec<Value>>>, ApiError> {
    let closures = state
        .service
        .store()
        .list_goal_closures(&project_id, &goal_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, closures).await?))
}

macro_rules! project_get {
    ($name:ident, $method:ident, $ty:ty) => {
        async fn $name(
            State(state): State<AppState>,
            Path(project_id): Path<String>,
        ) -> Result<Json<ApiEnvelope<$ty>>, ApiError> {
            let data = state.service.store().$method(&project_id).await?;
            Ok(Json(success(&state.service, &project_id, data).await?))
        }
    };
}

project_get!(get_project, get_project, research_domain::Project);
project_get!(snapshot, snapshot, research_domain::ProjectSnapshot);
project_get!(rounds, list_rounds, Vec<research_domain::ResearchRound>);
project_get!(workers, list_workers, Vec<research_domain::Worker>);
project_get!(routes, list_routes, Vec<research_domain::Route>);
project_get!(
    route_proposals,
    list_human_route_proposals,
    Vec<research_domain::HumanRouteProposal>
);
project_get!(strategy_states, list_strategy_states, Vec<Value>);
project_get!(latest_strategy_state, latest_strategy_state, Option<Value>);
project_get!(
    verifications,
    list_verifications,
    Vec<research_domain::Verification>
);
project_get!(
    uncertainties,
    list_uncertainties,
    Vec<research_domain::Uncertainty>
);
project_get!(sources, list_sources, Vec<research_domain::SourceRecord>);
project_get!(source_ingestions, list_source_ingestions, Vec<Value>);
project_get!(
    experiment_capsules,
    list_experiment_capsules,
    Vec<research_domain::ExperimentCapsule>
);
project_get!(failure_patterns, list_failure_patterns, Vec<Value>);
project_get!(artifacts, list_artifacts, Vec<research_domain::Artifact>);
project_get!(usage_summary, usage_summary, research_domain::UsageSummary);
project_get!(budget_overrides, list_budget_overrides, Vec<Value>);
project_get!(
    human_questions,
    list_human_questions,
    Vec<research_domain::HumanQuestion>
);
project_get!(
    fact_imports,
    list_fact_imports,
    Vec<research_domain::ProjectFactImport>
);

async fn board(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<BoardQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if query.schema_version != 1 {
        return Err(ApiError::invalid(format!(
            "unsupported board schema_version {}; only version 1 is available",
            query.schema_version
        )));
    }
    if !(0..=500).contains(&query.timeline_limit) {
        return Err(ApiError::invalid(
            "timeline_limit must be between 0 and 500",
        ));
    }
    let actor = authorize_actor(
        state.service.store(),
        &headers,
        Some(&project_id),
        "read_board",
        "project",
        &project_id,
        "viewer",
    )
    .await?;
    let mut include = BoardInclude::default();
    for item in query
        .include
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        match item {
            "summary" => {}
            "tasks" => include.tasks = true,
            "workers" => include.workers = true,
            "verification" => include.verification = true,
            "artifacts" => include.artifacts = true,
            "graph" => include.graph = true,
            unknown => {
                return Err(ApiError::invalid(format!(
                    "unknown board include module {unknown}"
                )));
            }
        }
    }
    let rank = role_rank(&actor.role);
    let capabilities = BoardCapabilities {
        can_edit_problem: rank >= role_rank("researcher"),
        can_propose_route: rank >= role_rank("researcher"),
        can_approve_route: rank >= role_rank("researcher"),
        can_control_project: rank >= role_rank("researcher"),
        can_control_tasks: rank >= role_rank("researcher"),
        can_govern_facts: rank >= role_rank("reviewer"),
    };
    let board = state
        .service
        .store()
        .research_board(&project_id, query.timeline_limit, include, capabilities)
        .await?;
    let etag = format!(
        "W/\"mathcat-board-v1-{}-r{}\"",
        board.project_id, board.revision
    );
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        == Some(etag.as_str())
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        response.headers_mut().insert(
            header::ETAG,
            HeaderValue::from_str(&etag)
                .map_err(|_| ApiError::invalid("invalid project id for ETag"))?,
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        return Ok(response);
    }
    let meta = ResponseMeta {
        request_id: request_id(),
        project_revision: Some(board.revision),
        event_cursor: Some(board.event_cursor),
    };
    let mut response = Json(ApiEnvelope {
        data: Some(board),
        meta,
        error: None,
    })
    .into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag)
            .map_err(|_| ApiError::invalid("invalid project id for ETag"))?,
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok(response)
}

async fn revise_problem(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ProblemRevisionRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let key = idempotency_key(&headers)?;
    let actor = authorize_actor(
        state.service.store(),
        &headers,
        Some(&project_id),
        "revise_problem_contract",
        "project",
        &project_id,
        "researcher",
    )
    .await?;
    let result = state
        .service
        .revise_problem_contract(&project_id, &body, &actor.actor_id, &key)
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiEnvelope {
            data: Some(result.data),
            meta: ResponseMeta {
                request_id: request_id(),
                project_revision: Some(result.project_revision),
                event_cursor: Some(result.event_cursor),
            },
            error: None,
        }),
    ))
}

async fn propose_route(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<HumanRouteProposalRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let key = idempotency_key(&headers)?;
    let actor = authorize_actor(
        state.service.store(),
        &headers,
        Some(&project_id),
        "propose_route",
        "project",
        &project_id,
        "researcher",
    )
    .await?;
    let result = state
        .service
        .propose_human_route(&project_id, &body, &actor.actor_id, &key)
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiEnvelope {
            data: Some(result.data),
            meta: ResponseMeta {
                request_id: request_id(),
                project_revision: Some(result.project_revision),
                event_cursor: Some(result.event_cursor),
            },
            error: None,
        }),
    ))
}

async fn project_status(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let snapshot = state.service.store().snapshot(&project_id).await?;
    let model_calls = state.service.store().total_model_calls(&project_id).await?;
    let model_call_limit = i64::from(snapshot.project.budget.max_total_model_calls);
    let data = json!({
        "project_id": project_id, "status": snapshot.project.status, "current_round": snapshot.project.current_round,
        "revision": snapshot.project_revision, "event_cursor": snapshot.event_cursor,
        "workers": {"total":snapshot.workers.len(),"running":snapshot.workers.iter().filter(|w| w.status.to_string()=="running").count()},
        "tasks": {"total":snapshot.tasks.len(),"running":snapshot.tasks.iter().filter(|t| t.status.to_string()=="running").count()},
        "facts": snapshot.facts.len(), "open_uncertainties": snapshot.uncertainties.iter().filter(|u| matches!(u.status, research_domain::UncertaintyStatus::Open | research_domain::UncertaintyStatus::Investigating)).count(),
        "model_calls":{"used":model_calls,"limit":model_call_limit,"remaining":(model_call_limit-model_calls).max(0)}
    });
    Ok(Json(
        success(&state.service, &snapshot.project.project_id, data).await?,
    ))
}

async fn experiment_capsule(
    State(state): State<AppState>,
    Path(capsule_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::ExperimentCapsule>>, ApiError> {
    let data = state
        .service
        .store()
        .get_experiment_capsule(&capsule_id)
        .await?;
    let project_id = data.project_id.clone();
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn latest(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::Artifact>>, ApiError> {
    let data = state.service.store().latest_report(&project_id).await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn current_round(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<ApiEnvelope<Option<research_domain::ResearchRound>>>, ApiError> {
    state.service.store().get_project(&project_id).await?;
    let data = state.service.store().current_round(&project_id).await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn worker(
    State(state): State<AppState>,
    Path((project_id, worker_id)): Path<(String, String)>,
) -> Result<Json<ApiEnvelope<research_domain::Worker>>, ApiError> {
    let data = state
        .service
        .store()
        .get_worker(&project_id, &worker_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn tasks(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<TaskQuery>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::Task>>>, ApiError> {
    let mut data = state.service.store().list_tasks(&project_id).await?;
    if let Some(statuses) = query.status {
        let allowed: Vec<_> = statuses.split(',').collect();
        data.retain(|task| allowed.contains(&task.status.as_str()));
    }
    if let Some(route_id) = query.route_id {
        data.retain(|task| task.route_id == route_id);
    }
    if let Some(worker_id) = query.worker_id {
        data.retain(|task| task.worker_id.as_deref() == Some(&worker_id));
    }
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn task(
    State(state): State<AppState>,
    Path((project_id, task_id)): Path<(String, String)>,
) -> Result<Json<ApiEnvelope<research_domain::Task>>, ApiError> {
    let data = state
        .service
        .store()
        .get_task(&project_id, &task_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn task_attempts(
    State(state): State<AppState>,
    Path((project_id, task_id)): Path<(String, String)>,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let data = state
        .service
        .store()
        .task_attempt_history(&project_id, &task_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn task_contract(
    State(state): State<AppState>,
    Path((project_id, task_id)): Path<(String, String)>,
) -> Result<Json<ApiEnvelope<research_domain::TaskContract>>, ApiError> {
    let data = state
        .service
        .store()
        .get_task_contract(&project_id, &task_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn task_context_packet(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::ContextPacket>>, ApiError> {
    let data = state
        .service
        .store()
        .get_context_packet_for_task(&task_id)
        .await?;
    let project_id = data.project_id.clone();
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn task_steers(
    State(state): State<AppState>,
    Path((project_id, task_id)): Path<(String, String)>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::TaskSteer>>>, ApiError> {
    state
        .service
        .store()
        .get_task(&project_id, &task_id)
        .await?;
    let data = state.service.store().list_task_steers(&task_id).await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn route(
    State(state): State<AppState>,
    Path((project_id, route_id)): Path<(String, String)>,
) -> Result<Json<ApiEnvelope<research_domain::Route>>, ApiError> {
    let data = state
        .service
        .store()
        .get_route(&project_id, &route_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn route_families(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<Value>>>, ApiError> {
    let data = state
        .service
        .store()
        .list_route_families(&project_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn route_tombstones(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<Value>>>, ApiError> {
    let data = state
        .service
        .store()
        .list_route_tombstones(&project_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn route_progress_ledger(
    State(state): State<AppState>,
    Path((project_id, route_id)): Path<(String, String)>,
) -> Result<Json<ApiEnvelope<Vec<Value>>>, ApiError> {
    let data = state
        .service
        .store()
        .list_route_progress(&project_id, &route_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn route_progress_digest(
    State(state): State<AppState>,
    Path(route_id): Path<String>,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let data = state
        .service
        .store()
        .route_progress_digest(&route_id)
        .await?;
    let project_id = data
        .get("project_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid("route progress digest is missing project_id"))?;
    Ok(Json(
        success(&state.service, project_id, data.clone()).await?,
    ))
}

async fn worker_instances(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::WorkerInstance>>>, ApiError> {
    let data = state
        .service
        .store()
        .list_worker_instances(&project_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn planning_delta(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::ResearchDelta>>, ApiError> {
    let data = state
        .service
        .store()
        .collect_research_delta(&project_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn planning_revisions(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::PlanRevisionRecord>>>, ApiError> {
    let data = state
        .service
        .store()
        .list_plan_revisions(&project_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn planning_revision(
    State(state): State<AppState>,
    Path((project_id, plan_revision_id)): Path<(String, String)>,
) -> Result<Json<ApiEnvelope<research_domain::PlanRevisionRecord>>, ApiError> {
    let data = state
        .service
        .store()
        .get_plan_revision(&project_id, &plan_revision_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn bottlenecks(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::Bottleneck>>>, ApiError> {
    let data = state.service.store().list_bottlenecks(&project_id).await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn fact_planning_impact(
    State(state): State<AppState>,
    Path((project_id, fact_id)): Path<(String, String)>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::FactImpactRecord>>>, ApiError> {
    let data = state
        .service
        .store()
        .fact_planning_impact(&project_id, &fact_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn planner_health(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::PlannerHealth>>, ApiError> {
    let data = state.service.store().planner_health(&project_id).await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn context_digest(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let data = state.service.store().context_digest(&project_id).await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn storage_health(
    State(state): State<AppState>,
) -> Result<Json<ApiEnvelope<research_domain::StorageHealth>>, ApiError> {
    let data = state.service.store().storage_health().await?;
    Ok(Json(ApiEnvelope {
        data: Some(data),
        meta: ResponseMeta {
            request_id: request_id(),
            project_revision: None,
            event_cursor: None,
        },
        error: None,
    }))
}

async fn state_writer_status(
    State(state): State<AppState>,
) -> Json<ApiEnvelope<research_storage::StateWriterSnapshot>> {
    Json(ApiEnvelope {
        data: Some(state.service.store().state_writer_status()),
        meta: ResponseMeta {
            request_id: request_id(),
            project_revision: None,
            event_cursor: None,
        },
        error: None,
    })
}

async fn run_reconciliation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ReconciliationRequest>,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let target_id = body.project_id.as_deref().unwrap_or("system");
    let actor = authorize_actor(
        state.service.store(),
        &headers,
        body.project_id.as_deref(),
        "run_reconciliation",
        "system",
        target_id,
        "admin",
    )
    .await?;
    let data = state
        .service
        .store()
        .run_reconciliation(body.project_id.as_deref(), &body.trigger_kind)
        .await?;
    state
        .service
        .store()
        .record_security_audit(
            body.project_id.as_deref(),
            Some(&actor.actor_id),
            "run_reconciliation",
            "system",
            target_id,
            "allowed",
            None,
            None,
        )
        .await?;
    if let Some(project_id) = body.project_id {
        Ok(Json(success(&state.service, &project_id, data).await?))
    } else {
        Ok(Json(ApiEnvelope {
            data: Some(data),
            meta: ResponseMeta {
                request_id: request_id(),
                project_revision: None,
                event_cursor: None,
            },
            error: None,
        }))
    }
}

async fn graph(
    State(state): State<AppState>,
    Path((project_id, graph_type)): Path<(String, String)>,
) -> Result<Json<ApiEnvelope<research_domain::GraphProjection>>, ApiError> {
    let data = state
        .service
        .store()
        .graph(&project_id, &graph_type)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn graph_delta(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<EventQuery>,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let graph = state.service.store().graph(&project_id, "combined").await?;
    let events = state
        .service
        .store()
        .list_events_after(&project_id, query.after, 5000)
        .await?;
    let snapshot = state.service.store().snapshot(&project_id).await?;
    let data = json!({
        "from_cursor": query.after,
        "to_cursor": snapshot.event_cursor,
        "revision": snapshot.project_revision,
        "upsert_nodes": graph.nodes,
        "upsert_edges": graph.edges,
        "removed_node_ids": [],
        "removed_edge_ids": [],
        "events": events,
        "snapshot_assisted": true,
    });
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn verification(
    State(state): State<AppState>,
    Path(verification_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::Verification>>, ApiError> {
    let data = state
        .service
        .store()
        .get_verification(&verification_id)
        .await?;
    let project_id = data.project_id.clone();
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn verification_case_by_verification(
    State(state): State<AppState>,
    Path(verification_id): Path<String>,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let case = state
        .service
        .store()
        .verification_case_for_verification(&verification_id)
        .await?;
    verification_case_envelope(&state.service, case).await
}

async fn verification_case(
    State(state): State<AppState>,
    Path(case_id): Path<String>,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let case = state
        .service
        .store()
        .get_verification_case(&case_id)
        .await?;
    verification_case_envelope(&state.service, case).await
}

async fn verification_case_envelope(
    service: &ResearchService,
    case: research_domain::VerificationCase,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let policy = service.store().verification_policy(&case.case_id).await?;
    let checks = service
        .store()
        .list_verification_checks(&case.case_id)
        .await?;
    let findings = service
        .store()
        .list_verification_findings(&case.case_id)
        .await?;
    let evidence = service
        .store()
        .list_verification_evidence(&case.case_id)
        .await?;
    let project_id = case.project_id.clone();
    Ok(Json(
        success(
            service,
            &project_id,
            json!({
                "case": case,
                "policy": policy,
                "checks": checks,
                "findings": findings,
                "evidence": evidence,
            }),
        )
        .await?,
    ))
}

async fn verification_snapshot(
    State(state): State<AppState>,
    Path(case_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::VerificationSnapshot>>, ApiError> {
    let data = state
        .service
        .store()
        .get_verification_snapshot(&case_id)
        .await?;
    let project_id = data.project_id.clone();
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn verification_checks(
    State(state): State<AppState>,
    Path(case_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::VerificationCheck>>>, ApiError> {
    let case = state
        .service
        .store()
        .get_verification_case(&case_id)
        .await?;
    let data = state
        .service
        .store()
        .list_verification_checks(&case_id)
        .await?;
    Ok(Json(success(&state.service, &case.project_id, data).await?))
}

async fn verification_findings(
    State(state): State<AppState>,
    Path(case_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::VerificationFinding>>>, ApiError> {
    let case = state
        .service
        .store()
        .get_verification_case(&case_id)
        .await?;
    let data = state
        .service
        .store()
        .list_verification_findings(&case_id)
        .await?;
    Ok(Json(success(&state.service, &case.project_id, data).await?))
}

async fn verification_evidence(
    State(state): State<AppState>,
    Path(case_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::VerificationEvidence>>>, ApiError> {
    let case = state
        .service
        .store()
        .get_verification_case(&case_id)
        .await?;
    let data = state
        .service
        .store()
        .list_verification_evidence(&case_id)
        .await?;
    Ok(Json(success(&state.service, &case.project_id, data).await?))
}

async fn semantic_contract(
    State(state): State<AppState>,
    Path(case_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::SemanticContract>>, ApiError> {
    let case = state
        .service
        .store()
        .get_verification_case(&case_id)
        .await?;
    let data = state
        .service
        .store()
        .get_semantic_contract(&case_id)
        .await?;
    Ok(Json(success(&state.service, &case.project_id, data).await?))
}

async fn formalization(
    State(state): State<AppState>,
    Path(case_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::Formalization>>, ApiError> {
    let case = state
        .service
        .store()
        .get_verification_case(&case_id)
        .await?;
    let data = state.service.store().get_formalization(&case_id).await?;
    Ok(Json(success(&state.service, &case.project_id, data).await?))
}

async fn alignment_reviews(
    State(state): State<AppState>,
    Path(case_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::AlignmentReview>>>, ApiError> {
    let case = state
        .service
        .store()
        .get_verification_case(&case_id)
        .await?;
    let data = state
        .service
        .store()
        .list_alignment_reviews(&case_id)
        .await?;
    Ok(Json(success(&state.service, &case.project_id, data).await?))
}

async fn backend_runs(
    State(state): State<AppState>,
    Path(case_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::BackendRun>>>, ApiError> {
    let case = state
        .service
        .store()
        .get_verification_case(&case_id)
        .await?;
    let data = state.service.store().list_backend_runs(&case_id).await?;
    Ok(Json(success(&state.service, &case.project_id, data).await?))
}

async fn verification_package(
    State(state): State<AppState>,
    Path(case_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::VerificationPackage>>, ApiError> {
    let case = state
        .service
        .store()
        .get_verification_case(&case_id)
        .await?;
    let data = state
        .service
        .store()
        .get_verification_package(&case_id)
        .await?;
    Ok(Json(success(&state.service, &case.project_id, data).await?))
}

async fn verification_replays(
    State(state): State<AppState>,
    Path(case_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::VerificationReplay>>>, ApiError> {
    let case = state
        .service
        .store()
        .get_verification_case(&case_id)
        .await?;
    let data = state
        .service
        .store()
        .list_verification_replays(&case_id)
        .await?;
    Ok(Json(success(&state.service, &case.project_id, data).await?))
}

#[derive(Debug, Deserialize)]
struct ProofTreeQuery {
    cursor: Option<String>,
    #[serde(default = "default_proof_tree_limit")]
    limit: u32,
}

const fn default_proof_tree_limit() -> u32 {
    50
}

#[derive(Debug, Deserialize, ToSchema)]
struct ProofHintScope {
    #[serde(default)]
    goal_id: Option<String>,
    #[serde(default)]
    proof_node_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct ProofHintRequest {
    kind: String,
    content: String,
    #[serde(default)]
    scope: Option<ProofHintScope>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default = "default_human_author")]
    #[serde(rename = "author")]
    _author: String,
}

fn default_human_author() -> String {
    "human".into()
}

#[derive(Debug, Deserialize, ToSchema)]
struct ProofSearchCommandRequest {
    expected_cancellation_epoch: i64,
    #[serde(default = "default_human_author")]
    #[serde(rename = "requested_by")]
    _requested_by: String,
    #[serde(default)]
    reason: Option<String>,
}

async fn get_formalization(
    State(state): State<AppState>,
    Path(formalization_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::Formalization>>, ApiError> {
    let data = state
        .service
        .store()
        .get_formalization_by_id(&formalization_id)
        .await?;
    let case = state
        .service
        .store()
        .get_verification_case(&data.case_id)
        .await?;
    Ok(Json(success(&state.service, &case.project_id, data).await?))
}

async fn formalization_semantic_contract(
    State(state): State<AppState>,
    Path(formalization_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::SemanticContract>>, ApiError> {
    let formalization = state
        .service
        .store()
        .get_formalization_by_id(&formalization_id)
        .await?;
    let case = state
        .service
        .store()
        .get_verification_case(&formalization.case_id)
        .await?;
    let data = state
        .service
        .store()
        .get_semantic_contract(&formalization.case_id)
        .await?;
    Ok(Json(success(&state.service, &case.project_id, data).await?))
}

async fn formalization_alignment(
    State(state): State<AppState>,
    Path(formalization_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::AlignmentReview>>>, ApiError> {
    let formalization = state
        .service
        .store()
        .get_formalization_by_id(&formalization_id)
        .await?;
    let case = state
        .service
        .store()
        .get_verification_case(&formalization.case_id)
        .await?;
    let data = state
        .service
        .store()
        .list_alignment_reviews(&formalization.case_id)
        .await?
        .into_iter()
        .filter(|review| review.formalization_id == formalization_id)
        .collect();
    Ok(Json(success(&state.service, &case.project_id, data).await?))
}

async fn formalization_goals(
    State(state): State<AppState>,
    Path(formalization_id): Path<String>,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let formalization = state
        .service
        .store()
        .get_formalization_by_id(&formalization_id)
        .await?;
    let case = state
        .service
        .store()
        .get_verification_case(&formalization.case_id)
        .await?;
    let search = state
        .service
        .store()
        .get_formalization_proof_search(&formalization_id)
        .await?;
    let goals = state
        .service
        .store()
        .list_open_proof_nodes(&search.search_id)
        .await?;
    Ok(Json(
        success(
            &state.service,
            &case.project_id,
            json!({"search":search,"open_nodes":goals}),
        )
        .await?,
    ))
}

async fn formalization_proof_tree(
    State(state): State<AppState>,
    Path(formalization_id): Path<String>,
    Query(query): Query<ProofTreeQuery>,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let formalization = state
        .service
        .store()
        .get_formalization_by_id(&formalization_id)
        .await?;
    let case = state
        .service
        .store()
        .get_verification_case(&formalization.case_id)
        .await?;
    let search = state
        .service
        .store()
        .get_formalization_proof_search(&formalization_id)
        .await?;
    let nodes = state
        .service
        .store()
        .list_proof_nodes(&search.search_id, query.cursor.as_deref(), query.limit)
        .await?;
    let edges = state
        .service
        .store()
        .list_proof_edges(&search.search_id)
        .await?;
    let hints = state
        .service
        .store()
        .list_proof_hints(&search.search_id)
        .await?;
    let next_cursor = (nodes.len() == usize::try_from(query.limit.clamp(1, 200)).unwrap_or(200))
        .then(|| nodes.last().map(|node| node.node_id.clone()))
        .flatten();
    Ok(Json(
        success(
            &state.service,
            &case.project_id,
            json!({
                "search":search,"nodes":nodes,"edges":edges,"hints":hints,"next_cursor":next_cursor
            }),
        )
        .await?,
    ))
}

async fn formalization_attempts(
    State(state): State<AppState>,
    Path(formalization_id): Path<String>,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let formalization = state
        .service
        .store()
        .get_formalization_by_id(&formalization_id)
        .await?;
    let case = state
        .service
        .store()
        .get_verification_case(&formalization.case_id)
        .await?;
    let search = state
        .service
        .store()
        .get_formalization_proof_search(&formalization_id)
        .await?;
    let nodes = state
        .service
        .store()
        .list_proof_nodes(&search.search_id, None, 200)
        .await?;
    let attempts = nodes
        .into_iter()
        .filter(|node| node.tactic.is_some())
        .map(|node| json!({"node_id":node.node_id,"parent_node_id":node.parent_node_id,"tactic":node.tactic,"status":node.status,"diagnostic":node.diagnostic,"created_at":node.created_at}))
        .collect::<Vec<_>>();
    Ok(Json(
        success(
            &state.service,
            &case.project_id,
            json!({"search_id":search.search_id,"attempts":attempts}),
        )
        .await?,
    ))
}

async fn add_formalization_hint(
    State(state): State<AppState>,
    Path(formalization_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ProofHintRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<research_domain::ProofHint>>), ApiError> {
    let key = idempotency_key(&headers)?;
    let formalization = state
        .service
        .store()
        .get_formalization_by_id(&formalization_id)
        .await?;
    let case = state
        .service
        .store()
        .get_verification_case(&formalization.case_id)
        .await?;
    let actor = authorize_actor(
        state.service.store(),
        &headers,
        Some(&case.project_id),
        "add_proof_hint",
        "formalization",
        &formalization_id,
        "researcher",
    )
    .await?;
    let search = state
        .service
        .store()
        .get_formalization_proof_search(&formalization_id)
        .await?;
    let node_id = request
        .scope
        .as_ref()
        .and_then(|scope| scope.proof_node_id.as_deref());
    let data = state
        .service
        .add_proof_hint(
            &search.search_id,
            node_id,
            &request.kind,
            &request.content,
            &actor.actor_id,
            &key,
        )
        .await?;
    state
        .service
        .store()
        .record_security_audit(
            Some(&case.project_id),
            Some(&actor.actor_id),
            "add_proof_hint",
            "formalization",
            &formalization_id,
            "allowed",
            Some(&request.content),
            Some(&key),
        )
        .await?;
    let _ = request
        .scope
        .as_ref()
        .and_then(|scope| scope.goal_id.as_deref());
    let _ = request.priority;
    Ok((
        StatusCode::ACCEPTED,
        Json(success(&state.service, &case.project_id, data).await?),
    ))
}

async fn prune_formalization_branch(
    State(state): State<AppState>,
    Path((formalization_id, node_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<ProofSearchCommandRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<research_domain::ProofSearch>>), ApiError> {
    let key = idempotency_key(&headers)?;
    let formalization = state
        .service
        .store()
        .get_formalization_by_id(&formalization_id)
        .await?;
    let case = state
        .service
        .store()
        .get_verification_case(&formalization.case_id)
        .await?;
    let actor = authorize_actor(
        state.service.store(),
        &headers,
        Some(&case.project_id),
        "prune_proof_branch",
        "proof_node",
        &node_id,
        "operator",
    )
    .await?;
    let search = state
        .service
        .store()
        .get_formalization_proof_search(&formalization_id)
        .await?;
    let data = state
        .service
        .prune_proof_branch(
            &search.search_id,
            &node_id,
            request.expected_cancellation_epoch,
            &actor.actor_id,
            &key,
        )
        .await?;
    state
        .service
        .store()
        .record_security_audit(
            Some(&case.project_id),
            Some(&actor.actor_id),
            "prune_proof_branch",
            "proof_node",
            &node_id,
            "allowed",
            request.reason.as_deref(),
            Some(&key),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(success(&state.service, &case.project_id, data).await?),
    ))
}

async fn cancel_formalization_search(
    State(state): State<AppState>,
    Path(formalization_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ProofSearchCommandRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<research_domain::ProofSearch>>), ApiError> {
    let key = idempotency_key(&headers)?;
    let formalization = state
        .service
        .store()
        .get_formalization_by_id(&formalization_id)
        .await?;
    let case = state
        .service
        .store()
        .get_verification_case(&formalization.case_id)
        .await?;
    let actor = authorize_actor(
        state.service.store(),
        &headers,
        Some(&case.project_id),
        "cancel_proof_search",
        "formalization",
        &formalization_id,
        "operator",
    )
    .await?;
    let search = state
        .service
        .store()
        .get_formalization_proof_search(&formalization_id)
        .await?;
    let data = state
        .service
        .cancel_proof_search(
            &search.search_id,
            request.expected_cancellation_epoch,
            &actor.actor_id,
            &key,
        )
        .await?;
    state
        .service
        .store()
        .record_security_audit(
            Some(&case.project_id),
            Some(&actor.actor_id),
            "cancel_proof_search",
            "formalization",
            &formalization_id,
            "allowed",
            request.reason.as_deref(),
            Some(&key),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(success(&state.service, &case.project_id, data).await?),
    ))
}

async fn fact(
    State(state): State<AppState>,
    Path(fact_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::Fact>>, ApiError> {
    let data = state.service.store().get_fact(&fact_id).await?;
    let project_id = data.project_id.clone();
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn fact_catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiEnvelope<Vec<Value>>>, ApiError> {
    authorize_actor(
        state.service.store(),
        &headers,
        None,
        "list_fact_catalog",
        "fact_catalog",
        "global",
        "viewer",
    )
    .await?;
    let data = state.service.store().list_catalog_facts().await?;
    Ok(Json(ApiEnvelope {
        data: Some(data),
        meta: ResponseMeta {
            request_id: request_id(),
            project_revision: None,
            event_cursor: None,
        },
        error: None,
    }))
}

async fn import_fact(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ImportFactRequest>,
) -> Result<
    (
        StatusCode,
        Json<ApiEnvelope<research_domain::ProjectFactImport>>,
    ),
    ApiError,
> {
    let key = idempotency_key(&headers)?;
    let actor = authorize_actor(
        state.service.store(),
        &headers,
        Some(&project_id),
        "import_fact",
        "project",
        &project_id,
        "researcher",
    )
    .await?;
    let data = state
        .service
        .import_catalog_fact(&project_id, &body.content_hash, &actor.actor_id)
        .await?;
    state
        .service
        .store()
        .record_security_audit(
            Some(&project_id),
            Some(&actor.actor_id),
            "import_fact",
            "fact_import",
            &data.import_id,
            "allowed",
            None,
            Some(&key),
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(success(&state.service, &project_id, data).await?),
    ))
}

async fn fact_impact(
    State(state): State<AppState>,
    Path(fact_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::FactImpact>>, ApiError> {
    let fact = state.service.store().get_fact(&fact_id).await?;
    let impact = state.service.store().fact_impact_detail(&fact_id).await?;
    Ok(Json(
        success(&state.service, &fact.project_id, impact).await?,
    ))
}

async fn fact_assurance(
    State(state): State<AppState>,
    Path(fact_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::FactAssurance>>>, ApiError> {
    let fact = state.service.store().get_fact(&fact_id).await?;
    let data = state.service.store().list_fact_assurances(&fact_id).await?;
    Ok(Json(success(&state.service, &fact.project_id, data).await?))
}

async fn fact_verification_history(
    State(state): State<AppState>,
    Path(fact_id): Path<String>,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let fact = state.service.store().get_fact(&fact_id).await?;
    let data = state
        .service
        .store()
        .fact_verification_history(&fact_id)
        .await?;
    Ok(Json(success(&state.service, &fact.project_id, data).await?))
}

async fn fact_formalization(
    State(state): State<AppState>,
    Path(fact_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::Formalization>>, ApiError> {
    let fact = state.service.store().get_fact(&fact_id).await?;
    let data = state
        .service
        .store()
        .get_fact_formalization(&fact_id)
        .await?;
    Ok(Json(success(&state.service, &fact.project_id, data).await?))
}

async fn fact_dependency_closure(
    State(state): State<AppState>,
    Path(fact_id): Path<String>,
) -> Result<Json<ApiEnvelope<Vec<Value>>>, ApiError> {
    let fact = state.service.store().get_fact(&fact_id).await?;
    let data = state
        .service
        .store()
        .fact_dependency_closure(&fact_id)
        .await?;
    Ok(Json(success(&state.service, &fact.project_id, data).await?))
}

#[derive(Debug, Deserialize)]
struct FactGovernanceRequest {
    reason: String,
}

async fn security_audit(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ApiEnvelope<Vec<research_domain::SecurityAuditEntry>>>, ApiError> {
    authorize_actor(
        state.service.store(),
        &headers,
        Some(&project_id),
        "read_security_audit",
        "project",
        &project_id,
        "operator",
    )
    .await?;
    let data = state
        .service
        .store()
        .list_security_audit(&project_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

type FactGovernanceResponse = (
    StatusCode,
    Json<ApiEnvelope<research_domain::FactGovernanceResult>>,
);

async fn govern_fact_command(
    state: AppState,
    fact_id: String,
    headers: HeaderMap,
    body: FactGovernanceRequest,
    action: &'static str,
) -> Result<FactGovernanceResponse, ApiError> {
    let key = idempotency_key(&headers)?;
    let fact = state.service.store().get_fact(&fact_id).await?;
    let actor = authorize_actor(
        state.service.store(),
        &headers,
        Some(&fact.project_id),
        action,
        "fact",
        &fact_id,
        if matches!(action, "revoke" | "suspend") {
            "operator"
        } else {
            "researcher"
        },
    )
    .await?;
    let data = state
        .service
        .govern_fact(&fact_id, action, &body.reason, &actor.actor_id, &key)
        .await?;
    let project_id = data.fact.project_id.clone();
    state
        .service
        .store()
        .record_security_audit(
            Some(&project_id),
            Some(&actor.actor_id),
            action,
            "fact",
            &fact_id,
            "allowed",
            Some(&body.reason),
            Some(&key),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(success(&state.service, &project_id, data).await?),
    ))
}

macro_rules! fact_governance_handler {
    ($name:ident, $action:literal) => {
        async fn $name(
            State(state): State<AppState>,
            Path(fact_id): Path<String>,
            headers: HeaderMap,
            Json(body): Json<FactGovernanceRequest>,
        ) -> Result<FactGovernanceResponse, ApiError> {
            govern_fact_command(state, fact_id, headers, body, $action).await
        }
    };
}

fact_governance_handler!(challenge_fact, "challenge");
fact_governance_handler!(reverify_fact, "reverify");
fact_governance_handler!(request_fact_formalization, "request_formalization");
fact_governance_handler!(request_independent_proof, "request_independent_proof");
fact_governance_handler!(suspend_fact, "suspend");
fact_governance_handler!(revoke_fact, "revoke");

async fn command(
    State(state): State<AppState>,
    Path((project_id, command_id)): Path<(String, String)>,
) -> Result<Json<ApiEnvelope<research_domain::HumanCommand>>, ApiError> {
    let data = state
        .service
        .store()
        .get_command(&project_id, &command_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn artifact(
    State(state): State<AppState>,
    Path((project_id, artifact_id)): Path<(String, String)>,
) -> Result<Json<ApiEnvelope<research_domain::Artifact>>, ApiError> {
    let data = state
        .service
        .store()
        .get_artifact(&project_id, &artifact_id)
        .await?;
    Ok(Json(success(&state.service, &project_id, data).await?))
}

async fn artifact_content(
    State(state): State<AppState>,
    Path((project_id, artifact_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let artifact = state
        .service
        .store()
        .get_artifact(&project_id, &artifact_id)
        .await?;
    content_response(
        state.service.store().artifact_root().to_path_buf(),
        artifact,
    )
    .await
}

async fn latest_report_content(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Response, ApiError> {
    let artifact = state.service.store().latest_report(&project_id).await?;
    content_response(
        state.service.store().artifact_root().to_path_buf(),
        artifact,
    )
    .await
}

async fn content_response(
    root: PathBuf,
    artifact: research_domain::Artifact,
) -> Result<Response, ApiError> {
    let canonical_root = tokio::fs::canonicalize(root)
        .await
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    let canonical_path = tokio::fs::canonicalize(&artifact.storage_path)
        .await
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(ApiError::invalid("artifact path escaped configured root"));
    }
    let bytes = tokio::fs::read(canonical_path)
        .await
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&artifact.media_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", artifact.filename))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    Ok(response)
}

type CommandResponse = (StatusCode, Json<ApiEnvelope<research_domain::HumanCommand>>);

async fn start_project(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    project_command(state, path, headers, body, "start_project").await
}
async fn pause_project(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    project_command(state, path, headers, body, "pause_project").await
}
async fn resume_project(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    project_command(state, path, headers, body, "resume_project").await
}
async fn stop_project(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    project_command(state, path, headers, body, "stop_project").await
}
async fn replan(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    project_command(state, path, headers, body, "trigger_replan").await
}

async fn create_human_task(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        body,
        "create_task",
        "project",
        &project_id,
        CommandMode::Immediate,
    )
    .await
}

async fn adjust_budget(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        body,
        "adjust_budget",
        "project",
        &project_id,
        CommandMode::Immediate,
    )
    .await
}

async fn answer_question(
    State(state): State<AppState>,
    Path((project_id, question_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        body,
        "answer_question",
        "human_question",
        &question_id,
        CommandMode::Immediate,
    )
    .await
}

async fn task_command(
    State(state): State<AppState>,
    Path((project_id, task_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
    command_type: &'static str,
    mode: CommandMode,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        body,
        command_type,
        "task",
        &task_id,
        mode,
    )
    .await
}

macro_rules! task_command_handler {
    ($name:ident, $command:literal, $mode:expr) => {
        async fn $name(
            state: State<AppState>,
            path: Path<(String, String)>,
            headers: HeaderMap,
            body: Json<CommandRequest>,
        ) -> Result<CommandResponse, ApiError> {
            task_command(state, path, headers, body, $command, $mode).await
        }
    };
}

task_command_handler!(steer_task, "steer_task", CommandMode::SafePoint);
task_command_handler!(pause_task, "pause_task", CommandMode::Immediate);
task_command_handler!(resume_task, "resume_task", CommandMode::Immediate);
task_command_handler!(reassign_task, "reassign_task", CommandMode::Immediate);
task_command_handler!(cancel_task, "cancel_task", CommandMode::Immediate);
task_command_handler!(
    set_task_priority,
    "set_task_priority",
    CommandMode::Immediate
);
task_command_handler!(retry_task, "retry_task", CommandMode::Immediate);
task_command_handler!(
    rebuild_task_context,
    "rebuild_task_context",
    CommandMode::Immediate
);

async fn rebuild_task_context_by_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    let packet = state
        .service
        .store()
        .get_context_packet_for_task(&task_id)
        .await?;
    issue_command(
        &state.service,
        &packet.project_id,
        &headers,
        body,
        "rebuild_task_context",
        "task",
        &task_id,
        CommandMode::Immediate,
    )
    .await
}

async fn project_command(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
    command_type: &'static str,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        body,
        command_type,
        "project",
        &project_id,
        CommandMode::Immediate,
    )
    .await
}

async fn pause_route(
    state: State<AppState>,
    path: Path<(String, String)>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    route_command(state, path, headers, body, "pause_route").await
}
async fn resume_route(
    state: State<AppState>,
    path: Path<(String, String)>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    route_command(state, path, headers, body, "resume_route").await
}
async fn stop_route(
    state: State<AppState>,
    path: Path<(String, String)>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    route_command(state, path, headers, body, "stop_route").await
}
async fn approve_route(
    state: State<AppState>,
    path: Path<(String, String)>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    route_command(state, path, headers, body, "approve_route").await
}
async fn prune_route(
    state: State<AppState>,
    path: Path<(String, String)>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    route_command(state, path, headers, body, "prune_route").await
}
async fn revive_route(
    state: State<AppState>,
    path: Path<(String, String)>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    route_command(state, path, headers, body, "revive_route").await
}

async fn merge_routes(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        body,
        "merge_routes",
        "project",
        &project_id,
        CommandMode::Immediate,
    )
    .await
}

async fn route_command(
    State(state): State<AppState>,
    Path((project_id, route_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
    command_type: &'static str,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        body,
        command_type,
        "route",
        &route_id,
        CommandMode::Immediate,
    )
    .await
}

async fn stop_worker(
    State(state): State<AppState>,
    Path((project_id, worker_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        body,
        "stop_worker",
        "worker",
        &worker_id,
        CommandMode::Immediate,
    )
    .await
}

async fn pause_worker(
    State(state): State<AppState>,
    Path((project_id, worker_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        body,
        "pause_worker",
        "worker",
        &worker_id,
        CommandMode::Immediate,
    )
    .await
}

async fn resume_worker(
    State(state): State<AppState>,
    Path((project_id, worker_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        body,
        "resume_worker",
        "worker",
        &worker_id,
        CommandMode::Immediate,
    )
    .await
}

async fn quarantine_worker_instance(
    State(state): State<AppState>,
    Path((project_id, worker_instance_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        body,
        "quarantine_worker_instance",
        "worker_instance",
        &worker_instance_id,
        CommandMode::Immediate,
    )
    .await
}

async fn add_suggestion(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SuggestionRequest>,
) -> Result<CommandResponse, ApiError> {
    let command_body = CommandRequest {
        expected_revision: body.expected_revision,
        reason: body.reason,
        payload: json!({"content":body.content,"target_route_id":body.target_route_id}),
    };
    issue_command(
        &state.service,
        &project_id,
        &headers,
        command_body,
        "add_suggestion",
        "project",
        &project_id,
        CommandMode::NextRound,
    )
    .await
}

async fn issue_command(
    service: &ResearchService,
    project_id: &str,
    headers: &HeaderMap,
    body: CommandRequest,
    command_type: &str,
    target_kind: &str,
    target_id: &str,
    mode: CommandMode,
) -> Result<CommandResponse, ApiError> {
    let key = idempotency_key(headers)?;
    let actor = authorize_actor(
        service.store(),
        headers,
        Some(project_id),
        command_type,
        target_kind,
        target_id,
        command_minimum_role(command_type),
    )
    .await?;
    let command = service
        .submit_command(
            project_id,
            CommandDraft {
                command_type: command_type.into(),
                target_kind: target_kind.into(),
                target_id: target_id.into(),
                mode,
                payload: body.payload.clone(),
                expected_project_revision: body.expected_revision,
                idempotency_key: key.clone(),
                reason: body.reason.clone(),
                requested_by: actor.actor_id.clone(),
            },
        )
        .await?;
    service
        .store()
        .record_security_audit(
            Some(project_id),
            Some(&actor.actor_id),
            command_type,
            target_kind,
            target_id,
            "allowed",
            Some(&body.reason),
            Some(&key),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(success(service, project_id, command).await?),
    ))
}

async fn submit_candidate(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CandidateSubmission>,
) -> Result<impl IntoResponse, ApiError> {
    let key = idempotency_key(&headers)?;
    let actor = authorize_actor(
        state.service.store(),
        &headers,
        Some(&project_id),
        "submit_candidate",
        "project",
        &project_id,
        "researcher",
    )
    .await?;
    let receipt = state
        .service
        .submit_candidate(&project_id, body, &key)
        .await?;
    state
        .service
        .store()
        .record_security_audit(
            Some(&project_id),
            Some(&actor.actor_id),
            "submit_candidate",
            "candidate",
            &receipt.candidate.candidate_id,
            "allowed",
            None,
            Some(&key),
        )
        .await?;
    let data = json!({"candidate_id":receipt.candidate.candidate_id,"verification_id":receipt.verification.verification_id,"status":"queued"});
    Ok((
        StatusCode::ACCEPTED,
        Json(success(&state.service, &project_id, data).await?),
    ))
}

async fn websocket_events(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<EventQuery>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    authorize_actor(
        state.service.store(),
        &headers,
        Some(&project_id),
        "subscribe_websocket",
        "project",
        &project_id,
        "viewer",
    )
    .await?;
    state.service.store().get_project(&project_id).await?;
    Ok(ws.on_upgrade(move |socket| {
        websocket_session(socket, state.service, project_id, query.after)
    }))
}

async fn websocket_session(
    socket: WebSocket,
    service: ResearchService,
    project_id: String,
    mut cursor: i64,
) {
    let (mut sender, mut incoming) = socket.split();
    if let Ok(history) = service
        .store()
        .list_events_after(&project_id, cursor, 5000)
        .await
    {
        for event in history {
            cursor = cursor.max(event.cursor);
            if send_ws_json(&mut sender, json!({"type":"event","data":event}))
                .await
                .is_err()
            {
                return;
            }
        }
    }
    let mut events = service.subscribe();
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(15));
    loop {
        tokio::select! {
            received = events.recv() => match received {
                Ok(event) if event.project_id == project_id && event.cursor > cursor => {
                    cursor = event.cursor;
                    if send_ws_json(&mut sender, json!({"type":"event","data":event})).await.is_err() { break; }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let latest = service.store().snapshot(&project_id).await.ok().map_or(cursor, |snapshot| snapshot.event_cursor);
                    let _ = send_ws_json(&mut sender, json!({"type":"resync_required","after":cursor,"latest":latest})).await;
                    break;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            message = incoming.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(request) = serde_json::from_str::<Value>(&text) {
                        match request.get("type").and_then(Value::as_str) {
                            Some("ping") => { if send_ws_json(&mut sender, json!({"type":"pong","cursor":cursor})).await.is_err() { break; } }
                            Some("resume") => {
                                let after = request.get("after").and_then(Value::as_i64).unwrap_or(cursor);
                                match service.store().list_events_after(&project_id, after, 5000).await {
                                    Ok(history) => for event in history {
                                        cursor = cursor.max(event.cursor);
                                        if send_ws_json(&mut sender, json!({"type":"event","data":event})).await.is_err() { return; }
                                    },
                                    Err(_) => { let _ = send_ws_json(&mut sender, json!({"type":"error","code":"resume_failed"})).await; }
                                }
                            }
                            _ => { let _ = send_ws_json(&mut sender, json!({"type":"error","code":"unsupported_message"})).await; }
                        }
                    }
                }
                Some(Ok(Message::Ping(payload))) => { if sender.send(Message::Pong(payload)).await.is_err() { break; } }
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                _ => {}
            },
            _ = heartbeat.tick() => {
                let usage = service.store().usage_summary(&project_id).await.ok();
                if send_ws_json(&mut sender, json!({"type":"heartbeat","cursor":cursor,"usage":usage})).await.is_err() { break; }
            }
        }
    }
}

async fn send_ws_json(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    value: Value,
) -> Result<(), axum::Error> {
    sender.send(Message::Text(value.to_string().into())).await
}

async fn events(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<EventQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    state.service.store().get_project(&project_id).await?;
    let history = state
        .service
        .store()
        .list_events_after(&project_id, query.after, 1000)
        .await?;
    let mut receiver = state.service.subscribe();
    let service = state.service.clone();
    let output = stream! {
        let mut cursor = query.after;
        for event in history {
            cursor = cursor.max(event.cursor);
            yield Ok(Event::default().id(event.cursor.to_string()).event(event.event_type.clone()).json_data(&event).unwrap_or_else(|_| Event::default().event("serialization.error")));
        }
        loop {
            match receiver.recv().await {
                Ok(event) if event.project_id == project_id && event.cursor > cursor => {
                    cursor = event.cursor;
                    yield Ok(Event::default().id(event.cursor.to_string()).event(event.event_type.clone()).json_data(&event).unwrap_or_else(|_| Event::default().event("serialization.error")));
                }
                Ok(_) => {}
                Err(broadcast_error) => {
                    if matches!(broadcast_error, tokio::sync::broadcast::error::RecvError::Closed) { break; }
                    if let Ok(events) = service.store().list_events_after(&project_id, cursor, 1000).await {
                        for event in events {
                            cursor = event.cursor;
                            yield Ok(Event::default().id(event.cursor.to_string()).event(event.event_type.clone()).json_data(&event).unwrap_or_else(|_| Event::default().event("serialization.error")));
                        }
                    }
                }
            }
        }
    };
    Ok(Sse::new(output).keep_alive(KeepAlive::default()))
}

async fn success<T: Serialize>(
    service: &ResearchService,
    project_id: &str,
    data: T,
) -> Result<ApiEnvelope<T>, ApiError> {
    let project = service.store().get_project(project_id).await?;
    let cursor = service.store().latest_cursor(project_id).await?;
    Ok(ApiEnvelope {
        data: Some(data),
        meta: ResponseMeta {
            request_id: request_id(),
            project_revision: Some(project.revision),
            event_cursor: Some(cursor),
        },
        error: None,
    })
}

fn idempotency_key(headers: &HeaderMap) -> Result<String, ApiError> {
    headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ApiError::invalid("Idempotency-Key header is required"))
}

fn worker_node_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get("X-Worker-Token")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::unauthorized("X-Worker-Token header is required"))
}

async fn authorize_actor(
    store: &SqliteStore,
    headers: &HeaderMap,
    project_id: Option<&str>,
    action: &str,
    target_kind: &str,
    target_id: &str,
    minimum_role: &str,
) -> Result<research_domain::Actor, ApiError> {
    if store.actor_count().await? == 0 {
        return Ok(research_domain::Actor {
            actor_id: "local-unconfigured-admin".into(),
            display_name: "Local unconfigured administrator".into(),
            role: "admin".into(),
            status: "active".into(),
            created_at: Utc::now(),
        });
    }
    let actor_id = headers
        .get("X-Actor-Id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError {
            status: StatusCode::UNAUTHORIZED,
            code: "actor_credentials_required",
            message: "X-Actor-Id and Authorization: Bearer credentials are required".into(),
            details: json!({}),
            retryable: false,
        })?;
    let token = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError {
            status: StatusCode::UNAUTHORIZED,
            code: "actor_credentials_required",
            message: "Authorization: Bearer credentials are required".into(),
            details: json!({}),
            retryable: false,
        })?;
    let actor = store
        .authenticate_actor(actor_id, token)
        .await
        .map_err(|_| ApiError {
            status: StatusCode::UNAUTHORIZED,
            code: "invalid_actor_credentials",
            message: "actor credentials are invalid".into(),
            details: json!({}),
            retryable: false,
        })?;
    if role_rank(&actor.role) < role_rank(minimum_role) {
        store
            .record_security_audit(
                project_id,
                Some(&actor.actor_id),
                action,
                target_kind,
                target_id,
                "denied",
                Some("insufficient role"),
                None,
            )
            .await?;
        return Err(ApiError {
            status: StatusCode::FORBIDDEN,
            code: "insufficient_role",
            message: format!("{minimum_role} role is required"),
            details: json!({"actual_role":actor.role}),
            retryable: false,
        });
    }
    Ok(actor)
}

fn role_rank(role: &str) -> u8 {
    match role {
        "researcher" => 1,
        "reviewer" => 2,
        "operator" => 3,
        "admin" => 4,
        _ => 0,
    }
}

fn command_minimum_role(command_type: &str) -> &'static str {
    match command_type {
        "stop_project"
        | "stop_route"
        | "stop_worker"
        | "pause_worker"
        | "resume_worker"
        | "adjust_budget"
        | "cancel_task"
        | "prune_route"
        | "merge_routes"
        | "revive_route"
        | "quarantine_worker_instance" => "operator",
        _ => "researcher",
    }
}

fn request_id() -> String {
    format!("req_{}", Ulid::new())
}

#[must_use]
pub fn openapi_document() -> Value {
    let mut paths = serde_json::Map::new();
    let entries = [
        (
            "/api/v1/actors/bootstrap",
            "post",
            "Bootstrap the first administrator",
        ),
        (
            "/api/v1/actors",
            "post",
            "Create a local actor and access token",
        ),
        (
            "/api/v1/worker-nodes",
            "post",
            "Register a distributed worker node",
        ),
        (
            "/api/v1/worker-nodes/{node_id}/heartbeat",
            "post",
            "Authenticate and heartbeat a worker node epoch",
        ),
        (
            "/api/v1/projects/{project_id}/distributed/leases/next",
            "post",
            "Lease the next capability-compatible task",
        ),
        ("/api/v1/task-leases/{lease_id}", "get", "Get a task lease"),
        (
            "/api/v1/task-leases/{lease_id}/renew",
            "post",
            "Renew an active task lease with epoch checks",
        ),
        (
            "/api/v1/task-leases/{lease_id}/complete",
            "post",
            "Commit remote worker output with stale-result rejection",
        ),
        ("/api/v1/projects", "post", "Create project"),
        ("/api/v1/projects/{project_id}", "get", "Get project"),
        (
            "/api/v1/projects/{project_id}/publications",
            "get",
            "List idempotent publication runs",
        ),
        (
            "/api/v1/projects/{project_id}/publications",
            "post",
            "Create or replay a revision-pinned publication run",
        ),
        (
            "/api/v1/publications/{publication_id}",
            "get",
            "Get a publication run",
        ),
        (
            "/api/v1/projects/{project_id}/goals/{goal_id}/closures",
            "get",
            "List auditable Goal Completion evidence",
        ),
        (
            "/api/v1/projects/{project_id}/status",
            "get",
            "Get aggregate status",
        ),
        (
            "/api/v1/projects/{project_id}/snapshot",
            "get",
            "Get consistent snapshot",
        ),
        (
            "/api/v1/projects/{project_id}/board",
            "get",
            "Get the atomic MathCat Lab board projection",
        ),
        (
            "/api/v1/projects/{project_id}/problem-revisions",
            "post",
            "Create an auditable problem-contract revision and invalidate stale work",
        ),
        (
            "/api/v1/projects/{project_id}/route-proposals",
            "get",
            "List auditable human route proposals",
        ),
        (
            "/api/v1/projects/{project_id}/route-proposals",
            "post",
            "Propose a route for Planner validation",
        ),
        (
            "/api/v1/projects/{project_id}/latest",
            "get",
            "Get latest report metadata",
        ),
        ("/api/v1/projects/{project_id}/rounds", "get", "List rounds"),
        (
            "/api/v1/projects/{project_id}/rounds/current",
            "get",
            "Get current round",
        ),
        (
            "/api/v1/projects/{project_id}/workers",
            "get",
            "List workers",
        ),
        (
            "/api/v1/projects/{project_id}/workers/{worker_id}",
            "get",
            "Get worker",
        ),
        ("/api/v1/projects/{project_id}/tasks", "get", "List tasks"),
        (
            "/api/v1/projects/{project_id}/tasks",
            "post",
            "Create a human-assigned task",
        ),
        (
            "/api/v1/projects/{project_id}/tasks/{task_id}",
            "get",
            "Get task",
        ),
        (
            "/api/v1/projects/{project_id}/tasks/{task_id}/attempts",
            "get",
            "List bounded task attempts and failure signatures",
        ),
        (
            "/api/v1/projects/{project_id}/tasks/{task_id}/contract",
            "get",
            "Get the immutable task contract",
        ),
        (
            "/api/v1/tasks/{task_id}/context-packet",
            "get",
            "Get the exact context packet delivered to a worker",
        ),
        (
            "/api/v1/projects/{project_id}/tasks/{task_id}/steers",
            "get",
            "List queued and applied task steering",
        ),
        (
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/steer",
            "post",
            "Queue task guidance for the next safe point",
        ),
        (
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/pause",
            "post",
            "Pause a task",
        ),
        (
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/resume",
            "post",
            "Resume a task",
        ),
        (
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/reassign",
            "post",
            "Reassign a task",
        ),
        (
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/cancel",
            "post",
            "Cancel a task and reject stale output",
        ),
        (
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/set-priority",
            "post",
            "Change task priority",
        ),
        (
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/retry",
            "post",
            "Queue a bounded retry under the task retry policy",
        ),
        (
            "/api/v1/projects/{project_id}/tasks/{task_id}/commands/rebuild-context",
            "post",
            "Rebuild an invalid task context as a new immutable packet and contract version",
        ),
        (
            "/api/v1/tasks/{task_id}/commands/rebuild-context",
            "post",
            "Rebuild task context using the architecture-compatible task-scoped path",
        ),
        ("/api/v1/projects/{project_id}/routes", "get", "List routes"),
        (
            "/api/v1/projects/{project_id}/routes/{route_id}/commands/approve",
            "post",
            "Approve budget eligibility for a route when human approval is enabled",
        ),
        (
            "/api/v1/projects/{project_id}/routes/{route_id}",
            "get",
            "Get route",
        ),
        (
            "/api/v1/projects/{project_id}/route-families",
            "get",
            "List semantic route families and canonical routes",
        ),
        (
            "/api/v1/projects/{project_id}/route-tombstones",
            "get",
            "List pruned route fingerprints and revival conditions",
        ),
        (
            "/api/v1/projects/{project_id}/routes/{route_id}/progress-ledger",
            "get",
            "List material route progress entries",
        ),
        (
            "/api/v1/routes/{route_id}/progress-digest",
            "get",
            "Get a provenance-aware route progress digest",
        ),
        (
            "/api/v1/projects/{project_id}/worker-instances",
            "get",
            "List worker process instances, handshakes, and health",
        ),
        (
            "/api/v1/projects/{project_id}/planning/delta",
            "get",
            "Get the current or most recently consumed research delta",
        ),
        (
            "/api/v1/projects/{project_id}/planning/revisions",
            "get",
            "List atomic plan revisions",
        ),
        (
            "/api/v1/projects/{project_id}/planning/revisions/{plan_revision_id}",
            "get",
            "Get route, fact, bottleneck, and task decisions for one plan revision",
        ),
        (
            "/api/v1/projects/{project_id}/strategy/states",
            "get",
            "List immutable Strategy Director states and macro audits",
        ),
        (
            "/api/v1/projects/{project_id}/strategy/latest",
            "get",
            "Get the latest whole-proof strategy state",
        ),
        (
            "/api/v1/projects/{project_id}/bottlenecks",
            "get",
            "List the persistent bottleneck register",
        ),
        (
            "/api/v1/projects/{project_id}/facts/{fact_id}/planning-impact",
            "get",
            "Explain how planning consumed or deferred a fact",
        ),
        (
            "/api/v1/projects/{project_id}/planner/health",
            "get",
            "Get planner circuit-breaker and degraded-mode health",
        ),
        (
            "/api/v1/projects/{project_id}/context/digest",
            "get",
            "Get the provenance-aware project planning digest",
        ),
        (
            "/api/v1/system/storage/health",
            "get",
            "Get state queue, outbox, lease, and reconciliation health",
        ),
        (
            "/api/v1/system/state-writer/status",
            "get",
            "Get bounded SQLite StateWriter status",
        ),
        (
            "/api/v1/system/reconciliation/commands/run",
            "post",
            "Run an authorized consistency reconciliation scan",
        ),
        (
            "/api/v1/projects/{project_id}/graphs/{graph_type}",
            "get",
            "Get goal, hypothesis, fact, or six-kind combined graph",
        ),
        (
            "/api/v1/projects/{project_id}/graphs/combined/delta",
            "get",
            "Recover a snapshot-assisted combined graph delta from an event cursor",
        ),
        (
            "/api/v1/projects/{project_id}/verifications",
            "get",
            "List verifications",
        ),
        (
            "/api/v1/verifications/{verification_id}",
            "get",
            "Get verification",
        ),
        (
            "/api/v1/verifications/{verification_id}/case",
            "get",
            "Get trusted verification case",
        ),
        (
            "/api/v1/verification-cases/{case_id}",
            "get",
            "Get verification case with policy and evidence",
        ),
        (
            "/api/v1/verification-cases/{case_id}/snapshot",
            "get",
            "Get immutable verification snapshot",
        ),
        (
            "/api/v1/verification-cases/{case_id}/checks",
            "get",
            "List verification checks",
        ),
        (
            "/api/v1/verification-cases/{case_id}/findings",
            "get",
            "List verification findings",
        ),
        (
            "/api/v1/verification-cases/{case_id}/evidence",
            "get",
            "List verification evidence",
        ),
        (
            "/api/v1/verification-cases/{case_id}/semantic-contract",
            "get",
            "Get semantic contract",
        ),
        (
            "/api/v1/verification-cases/{case_id}/formalization",
            "get",
            "Get Lean formalization",
        ),
        (
            "/api/v1/verification-cases/{case_id}/alignment-reviews",
            "get",
            "List semantic alignment reviews",
        ),
        (
            "/api/v1/verification-cases/{case_id}/backend-runs",
            "get",
            "List verification backend runs",
        ),
        (
            "/api/v1/verification-cases/{case_id}/package",
            "get",
            "Get content-addressed verification package",
        ),
        (
            "/api/v1/verification-cases/{case_id}/replays",
            "get",
            "List independent replays",
        ),
        (
            "/api/v1/formalizations/{formalization_id}",
            "get",
            "Get formalization and current source",
        ),
        (
            "/api/v1/formalizations/{formalization_id}/semantic-contract",
            "get",
            "Get formalization semantic contract",
        ),
        (
            "/api/v1/formalizations/{formalization_id}/alignment",
            "get",
            "Get formalization alignment reviews",
        ),
        (
            "/api/v1/formalizations/{formalization_id}/goals",
            "get",
            "List current open Pantograph goals",
        ),
        (
            "/api/v1/formalizations/{formalization_id}/proof-tree",
            "get",
            "Page through proof tree nodes and edges",
        ),
        (
            "/api/v1/formalizations/{formalization_id}/attempts",
            "get",
            "List visible tactic attempts",
        ),
        (
            "/api/v1/formalizations/{formalization_id}/hints",
            "post",
            "Add a hint for the next proof-node expansion",
        ),
        (
            "/api/v1/formalizations/{formalization_id}/proof-nodes/{node_id}/commands/prune",
            "post",
            "Prune a proof branch immediately",
        ),
        (
            "/api/v1/formalizations/{formalization_id}/commands/cancel",
            "post",
            "Cancel an interactive proof search",
        ),
        (
            "/api/v1/projects/{project_id}/uncertainties",
            "get",
            "List uncertainties",
        ),
        (
            "/api/v1/projects/{project_id}/sources",
            "get",
            "List unverified source leads",
        ),
        (
            "/api/v1/projects/{project_id}/sources/ingestions",
            "get",
            "List source insertion, deduplication, and rejection provenance",
        ),
        (
            "/api/v1/projects/{project_id}/experiment-capsules",
            "get",
            "List content-addressed unverified experiment capsules",
        ),
        (
            "/api/v1/experiment-capsules/{capsule_id}",
            "get",
            "Get an exact experiment capsule and replay metadata",
        ),
        (
            "/api/v1/projects/{project_id}/failure-patterns",
            "get",
            "List failure patterns",
        ),
        ("/api/v1/facts/{fact_id}", "get", "Get fact"),
        (
            "/api/v1/facts/catalog",
            "get",
            "List the global fact catalog",
        ),
        (
            "/api/v1/projects/{project_id}/fact-imports",
            "get",
            "List cross-project fact imports",
        ),
        (
            "/api/v1/projects/{project_id}/fact-imports",
            "post",
            "Import a fact and its complete assured dependency closure",
        ),
        (
            "/api/v1/facts/{fact_id}/assurance",
            "get",
            "List current and historical fact assurances",
        ),
        (
            "/api/v1/facts/{fact_id}/verification-history",
            "get",
            "List verification, challenge, replay, and assurance history",
        ),
        (
            "/api/v1/facts/{fact_id}/formalization",
            "get",
            "Get the latest fact formalization",
        ),
        (
            "/api/v1/facts/{fact_id}/dependency-closure",
            "get",
            "Get dependency closure with assurance levels",
        ),
        ("/api/v1/facts/{fact_id}/impact", "get", "Get fact impact"),
        (
            "/api/v1/facts/{fact_id}/commands/challenge",
            "post",
            "Challenge a fact and enqueue independent reverification",
        ),
        (
            "/api/v1/facts/{fact_id}/commands/reverify",
            "post",
            "Reverify a fact without overwriting its assurance history",
        ),
        (
            "/api/v1/facts/{fact_id}/commands/request-formalization",
            "post",
            "Request a fresh Lean formalization and certification",
        ),
        (
            "/api/v1/facts/{fact_id}/commands/request-independent-proof",
            "post",
            "Request a separately snapshotted proof with an extra independent review",
        ),
        (
            "/api/v1/facts/{fact_id}/commands/suspend",
            "post",
            "Suspend a fact and invalidate downstream use",
        ),
        (
            "/api/v1/facts/{fact_id}/commands/revoke",
            "post",
            "Revoke a fact and apply dependency impact",
        ),
        (
            "/api/v1/projects/{project_id}/events",
            "get",
            "Subscribe to SSE events",
        ),
        (
            "/api/v1/projects/{project_id}/ws",
            "get",
            "Resume a bidirectional WebSocket event stream with heartbeat usage",
        ),
        (
            "/api/v1/projects/{project_id}/security-audit",
            "get",
            "List append-only authorization decisions",
        ),
        (
            "/api/v1/projects/{project_id}/artifacts",
            "get",
            "List artifacts",
        ),
        (
            "/api/v1/projects/{project_id}/artifacts/{artifact_id}",
            "get",
            "Get artifact metadata",
        ),
        (
            "/api/v1/projects/{project_id}/artifacts/{artifact_id}/content",
            "get",
            "Download artifact",
        ),
        (
            "/api/v1/projects/{project_id}/reports/latest",
            "get",
            "Download latest report",
        ),
        (
            "/api/v1/projects/{project_id}/commands/{command_id}",
            "get",
            "Get command status",
        ),
        (
            "/api/v1/projects/{project_id}/commands/start",
            "post",
            "Start project",
        ),
        (
            "/api/v1/projects/{project_id}/commands/pause",
            "post",
            "Pause project",
        ),
        (
            "/api/v1/projects/{project_id}/commands/resume",
            "post",
            "Resume project",
        ),
        (
            "/api/v1/projects/{project_id}/commands/stop",
            "post",
            "Stop project",
        ),
        (
            "/api/v1/projects/{project_id}/commands/replan",
            "post",
            "Trigger replan",
        ),
        (
            "/api/v1/projects/{project_id}/suggestions",
            "post",
            "Add next-round suggestion",
        ),
        (
            "/api/v1/projects/{project_id}/routes/{route_id}/commands/pause",
            "post",
            "Pause route",
        ),
        (
            "/api/v1/projects/{project_id}/routes/{route_id}/commands/resume",
            "post",
            "Resume route",
        ),
        (
            "/api/v1/projects/{project_id}/routes/{route_id}/commands/stop",
            "post",
            "Stop route",
        ),
        (
            "/api/v1/projects/{project_id}/routes/{route_id}/commands/prune",
            "post",
            "Prune and tombstone a route",
        ),
        (
            "/api/v1/projects/{project_id}/routes/commands/merge",
            "post",
            "Merge duplicate routes into a canonical route",
        ),
        (
            "/api/v1/projects/{project_id}/routes/{route_id}/commands/revive",
            "post",
            "Revive a tombstoned route with explicit evidence",
        ),
        (
            "/api/v1/projects/{project_id}/workers/{worker_id}/commands/stop",
            "post",
            "Stop worker",
        ),
        (
            "/api/v1/projects/{project_id}/workers/{worker_id}/commands/pause",
            "post",
            "Pause worker task intake",
        ),
        (
            "/api/v1/projects/{project_id}/workers/{worker_id}/commands/resume",
            "post",
            "Resume worker task intake",
        ),
        (
            "/api/v1/projects/{project_id}/worker-instances/{worker_instance_id}/commands/quarantine",
            "post",
            "Quarantine a faulty worker instance and expire its lease",
        ),
        (
            "/api/v1/projects/{project_id}/usage",
            "get",
            "Get measured model-call and token usage",
        ),
        (
            "/api/v1/projects/{project_id}/budgets",
            "get",
            "List scoped budget overrides",
        ),
        (
            "/api/v1/projects/{project_id}/commands/adjust-budget",
            "post",
            "Adjust a scoped budget without dropping below consumed usage",
        ),
        (
            "/api/v1/projects/{project_id}/questions",
            "get",
            "List proactive human clarification questions",
        ),
        (
            "/api/v1/projects/{project_id}/questions/{question_id}/commands/answer",
            "post",
            "Answer a clarification question and resume affected work",
        ),
        (
            "/api/v1/projects/{project_id}/candidates",
            "post",
            "Submit candidate claim",
        ),
    ];
    for (path, method, summary) in entries {
        let mut parameters: Vec<Value> = path
            .split('/')
            .filter_map(|segment| {
                segment
                    .strip_prefix('{')
                    .and_then(|value| value.strip_suffix('}'))
                    .map(|name| {
                        json!({"name":name,"in":"path","required":true,"schema":{"type":"string"}})
                    })
            })
            .collect();
        if path.ends_with("/board") {
            parameters.extend([
                json!({"name":"timeline_limit","in":"query","required":false,"schema":{"type":"integer","minimum":0,"maximum":500,"default":100}}),
                json!({"name":"include","in":"query","required":false,"schema":{"type":"string","default":"summary"}}),
                json!({"name":"schema_version","in":"query","required":false,"schema":{"type":"integer","const":1,"default":1}}),
                json!({"name":"If-None-Match","in":"header","required":false,"schema":{"type":"string"}}),
            ]);
        }
        let idempotent_write = method == "post"
            && (path.contains("/commands/")
                || path.ends_with("/candidates")
                || path.ends_with("/suggestions")
                || path.ends_with("/hints")
                || path.ends_with("/fact-imports")
                || path.ends_with("/problem-revisions")
                || path.ends_with("/route-proposals")
                || path == "/api/v1/projects/{project_id}/tasks");
        if idempotent_write {
            parameters.push(json!({
                "name":"Idempotency-Key","in":"header","required":true,
                "schema":{"type":"string","minLength":1},
                "description":"Unique key used to deduplicate retried writes."
            }));
        }
        let success_status = if method == "get" && path.ends_with("/ws") {
            "101"
        } else if method == "post"
            && matches!(
                path,
                "/api/v1/projects"
                    | "/api/v1/actors/bootstrap"
                    | "/api/v1/actors"
                    | "/api/v1/worker-nodes"
                    | "/api/v1/projects/{project_id}/fact-imports"
            )
        {
            "201"
        } else if method == "post" {
            "202"
        } else {
            "200"
        };
        let success_content = if path.ends_with("/events") {
            json!({"text/event-stream":{"schema":{"type":"string"}}})
        } else if path.ends_with("/content") || path.ends_with("/reports/latest") {
            json!({"application/octet-stream":{"schema":{"type":"string","contentEncoding":"binary"}}})
        } else if path.ends_with("/board") {
            json!({"application/json":{"schema":{"$ref":"#/components/schemas/BoardEnvelope"}}})
        } else {
            json!({"application/json":{"schema":{"$ref":"#/components/schemas/SuccessEnvelope"}}})
        };
        let mut responses = serde_json::Map::new();
        responses.insert(
            success_status.into(),
            json!({"description":"Success","content":success_content}),
        );
        if path.ends_with("/board") {
            responses.insert(
                "304".into(),
                json!({"description":"Board revision is unchanged"}),
            );
        }
        responses.insert(
            "default".into(),
            json!({"description":"Error envelope","content":{"application/json":{"schema":{"$ref":"#/components/schemas/ErrorEnvelope"}}}}),
        );
        let mut operation = json!({
            "summary":summary,
            "parameters":parameters,
            "responses":responses,
        });
        if path == "/api/v1/actors/bootstrap" {
            operation["security"] = json!([]);
        } else if (path.starts_with("/api/v1/worker-nodes/") && path.ends_with("/heartbeat"))
            || path.ends_with("/distributed/leases/next")
            || path.ends_with("/renew")
            || path.ends_with("/complete")
        {
            operation["security"] = json!([{"WorkerToken":[]}]);
        }
        if method == "post" {
            let schema = if path == "/api/v1/projects" {
                "CreateProjectRequest"
            } else if path == "/api/v1/actors/bootstrap" {
                "BootstrapActorRequest"
            } else if path == "/api/v1/actors" {
                "CreateActorRequest"
            } else if path == "/api/v1/worker-nodes" {
                "RegisterWorkerNodeRequest"
            } else if path.ends_with("/heartbeat") {
                "NodeHeartbeatRequest"
            } else if path.ends_with("/distributed/leases/next") {
                "LeaseTaskRequest"
            } else if path.ends_with("/renew") {
                "RenewLeaseRequest"
            } else if path.ends_with("/complete") {
                "CompleteLeaseRequest"
            } else if path.ends_with("/reconciliation/commands/run") {
                "ReconciliationRequest"
            } else if path.ends_with("/fact-imports") {
                "ImportFactRequest"
            } else if path.starts_with("/api/v1/facts/") && path.contains("/commands/") {
                "FactGovernanceRequest"
            } else if path.ends_with("/publications") {
                "CreatePublicationRequest"
            } else if path.ends_with("/candidates") {
                "CandidateSubmission"
            } else if path.ends_with("/suggestions") {
                "SuggestionRequest"
            } else if path.ends_with("/problem-revisions") {
                "ProblemRevisionRequest"
            } else if path.ends_with("/route-proposals") {
                "HumanRouteProposalRequest"
            } else if path.contains("/formalizations/") && path.ends_with("/hints") {
                "ProofHintRequest"
            } else if path.contains("/formalizations/") {
                "ProofSearchCommandRequest"
            } else {
                "CommandRequest"
            };
            operation["requestBody"] = json!({
                "required":true,
                "content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{schema}")}}}
            });
        }
        let item = paths.entry(path).or_insert_with(|| json!({}));
        if let Some(path_item) = item.as_object_mut() {
            path_item.insert(method.into(), operation);
        }
    }
    json!({
        "openapi":"3.1.0",
        "info":{"title":"Math Research Agent API","version":"0.3.0","description":"Authenticated query, command, verification, event, artifact, fact-governance, and distributed-worker boundary."},
        "servers":[{"url":"/"}],
        "paths":paths,
        "components":{"securitySchemes":{
            "ActorBearer":{"type":"http","scheme":"bearer"},
            "ActorId":{"type":"apiKey","in":"header","name":"X-Actor-Id"},
            "WorkerToken":{"type":"apiKey","in":"header","name":"X-Worker-Token"}
        },"schemas":{
            "ResponseMeta":{"type":"object","required":["request_id","project_revision","event_cursor"],"properties":{
                "request_id":{"type":"string"},"project_revision":{"type":["integer","null"],"format":"int64"},"event_cursor":{"type":["integer","null"],"format":"int64"}
            }},
            "ApiErrorBody":{"type":"object","required":["code","message","details","retryable"],"properties":{
                "code":{"type":"string"},"message":{"type":"string"},"details":{},"retryable":{"type":"boolean"}
            }},
            "SuccessEnvelope":{"type":"object","required":["data","meta","error"],"properties":{
                "data":{},"meta":{"$ref":"#/components/schemas/ResponseMeta"},"error":{"type":"null"}
            }},
            "BoardEnvelope":{"type":"object","additionalProperties":false,"required":["data","meta","error"],"properties":{
                "data":{"$ref":"#/components/schemas/ResearchBoardView"},"meta":{"$ref":"#/components/schemas/ResponseMeta"},"error":{"type":"null"}
            }},
            "ErrorEnvelope":{"type":"object","required":["data","meta","error"],"properties":{
                "data":{"type":"null"},"meta":{"$ref":"#/components/schemas/ResponseMeta"},"error":{"$ref":"#/components/schemas/ApiErrorBody"}
            }},
            "Budget":{"type":"object","required":["max_rounds","max_parallel_workers","max_minutes_per_task","max_model_calls_per_task","max_total_model_calls"],"properties":{
                "max_rounds":{"type":"integer","minimum":1},"max_parallel_workers":{"type":"integer","minimum":1},"max_minutes_per_task":{"type":"integer","minimum":1},"max_model_calls_per_task":{"type":"integer","minimum":1},"max_total_model_calls":{"type":"integer","minimum":1}
            }},
            "CreateProjectRequest":{"type":"object","additionalProperties":false,"required":["name","problem"],"properties":{
                "name":{"type":"string","minLength":1},"problem":{"type":"string","minLength":1},"target_statement":{"type":["string","null"]},"assumptions":{"type":"array","items":{"type":"string"}},"success_criteria":{"type":["string","null"]},"budget":{"$ref":"#/components/schemas/Budget"},"human_route_approval":{"type":"boolean","default":false}
            }},
            "CreatePublicationRequest":{"type":"object","additionalProperties":false,"properties":{
                "allow_partial":{"type":"boolean","default":false}
            }},
            "BootstrapActorRequest":{"type":"object","additionalProperties":false,"required":["actor_id","display_name","token"],"properties":{
                "actor_id":{"type":"string"},"display_name":{"type":"string"},"token":{"type":"string","minLength":16}
            }},
            "CreateActorRequest":{"type":"object","additionalProperties":false,"required":["actor_id","display_name","role","token"],"properties":{
                "actor_id":{"type":"string"},"display_name":{"type":"string"},"role":{"type":"string","enum":["viewer","researcher","reviewer","operator","admin"]},"token":{"type":"string","minLength":16}
            }},
            "RegisterWorkerNodeRequest":{"type":"object","additionalProperties":false,"required":["node_id","display_name","capabilities","token"],"properties":{
                "node_id":{"type":"string"},"display_name":{"type":"string"},"capabilities":{"type":"object"},"token":{"type":"string","minLength":16}
            }},
            "NodeHeartbeatRequest":{"type":"object","additionalProperties":false,"required":["node_epoch"],"properties":{
                "node_epoch":{"type":"integer","format":"int64"}
            }},
            "LeaseTaskRequest":{"type":"object","additionalProperties":false,"required":["node_id","node_epoch"],"properties":{
                "node_id":{"type":"string"},"node_epoch":{"type":"integer","format":"int64"},"ttl_seconds":{"type":"integer","minimum":10,"maximum":3600}
            }},
            "RenewLeaseRequest":{"type":"object","additionalProperties":false,"required":["node_id","node_epoch","lease_epoch"],"properties":{
                "node_id":{"type":"string"},"node_epoch":{"type":"integer","format":"int64"},"lease_epoch":{"type":"integer","format":"int64"},"ttl_seconds":{"type":"integer","minimum":10,"maximum":3600}
            }},
            "CompleteLeaseRequest":{"type":"object","additionalProperties":false,"required":["node_id","node_epoch","lease_epoch","output"],"properties":{
                "node_id":{"type":"string"},"node_epoch":{"type":"integer","format":"int64"},"lease_epoch":{"type":"integer","format":"int64"},"output":{"type":"object"}
            }},
            "ReconciliationRequest":{"type":"object","additionalProperties":false,"properties":{
                "project_id":{"type":["string","null"]},"trigger_kind":{"type":"string","default":"admin_api"}
            }},
            "ImportFactRequest":{"type":"object","additionalProperties":false,"required":["content_hash"],"properties":{
                "content_hash":{"type":"string"}
            }},
            "FactGovernanceRequest":{"type":"object","additionalProperties":false,"required":["reason"],"properties":{
                "reason":{"type":"string","minLength":1}
            }},
            "CommandRequest":{"type":"object","additionalProperties":false,"required":["expected_revision"],"properties":{
                "expected_revision":{"type":"integer","format":"int64"},"reason":{"type":"string"},"payload":{"type":"object"}
            }},
            "SuggestionRequest":{"type":"object","additionalProperties":false,"required":["expected_revision","content","target_route_id"],"properties":{
                "expected_revision":{"type":"integer","format":"int64"},"content":{"type":"string","minLength":1},"target_route_id":{"type":["string","null"]},"reason":{"type":"string"}
            }},
            "ProblemRevisionRequest":{"type":"object","additionalProperties":false,"required":["expected_revision","target_statement","assumptions","success_criteria","change_reason","replan"],"properties":{
                "expected_revision":{"type":"integer","format":"int64"},"target_statement":{"type":"string","minLength":1},"assumptions":{"type":"array","items":{"type":"string"}},"success_criteria":{"type":"string","minLength":1},"change_reason":{"type":"string","minLength":1},"replan":{"type":"boolean"}
            }},
            "HumanRouteProposalRequest":{"type":"object","additionalProperties":false,"required":["expected_revision","title","method_summary","target_goal_ids","required_fact_ids","known_risks","reason"],"properties":{
                "expected_revision":{"type":"integer","format":"int64"},"title":{"type":"string","minLength":1},"method_summary":{"type":"string","minLength":1},"target_goal_ids":{"type":"array","items":{"type":"string"}},"required_fact_ids":{"type":"array","items":{"type":"string"}},"known_risks":{"type":"array","items":{"type":"string"}},"reason":{"type":"string","minLength":1}
            }},
            "BoardCapabilities":{"type":"object","additionalProperties":false,"required":["can_edit_problem","can_propose_route","can_approve_route","can_control_project","can_control_tasks","can_govern_facts"],"properties":{
                "can_edit_problem":{"type":"boolean"},"can_propose_route":{"type":"boolean"},"can_approve_route":{"type":"boolean"},"can_control_project":{"type":"boolean"},"can_control_tasks":{"type":"boolean"},"can_govern_facts":{"type":"boolean"}
            }},
            "ResearchBoardView":{"type":"object","additionalProperties":true,"required":["schema_version","project_id","agent","mode","status","revision","event_cursor","problem","summary","routes","goals","claims","failed_routes","human_questions","uncertainties","tasks","workers","verification_queue","artifacts","timeline","graph","capabilities"],"properties":{
                "schema_version":{"type":"integer","const":1},"project_id":{"type":"string"},"agent":{"type":"string","const":"mathcat"},"mode":{"type":"string"},"status":{"type":"string"},"revision":{"type":"integer","format":"int64"},"event_cursor":{"type":"integer","format":"int64"},"problem":{"type":"object"},"summary":{"type":"object"},"routes":{"type":"array","items":{"type":"object"}},"goals":{"type":"array","items":{"type":"object"}},"claims":{"type":"array","items":{"type":"object"}},"failed_routes":{"type":"array","items":{"type":"object"}},"human_questions":{"type":"array","items":{"type":"object"}},"uncertainties":{"type":"array","items":{"type":"object"}},"tasks":{"type":"array","items":{"type":"object"}},"workers":{"type":"array","items":{"type":"object"}},"verification_queue":{"type":"array","items":{"type":"object"}},"artifacts":{"type":"array","items":{"type":"object"}},"timeline":{"type":"array","items":{"type":"object"}},"graph":{"type":["object","null"]},"capabilities":{"$ref":"#/components/schemas/BoardCapabilities"}
            }},
            "CandidateSubmission":{"type":"object","additionalProperties":false,"required":["task_id","route_id","target_goal_ids","statement","assumptions","proof_markdown","dependency_fact_ids","definitions_introduced","external_source_ids","candidate_type","task_revision","route_cancellation_epoch"],"properties":{
                "task_id":{"type":"string"},"route_id":{"type":"string"},"target_goal_ids":{"type":"array","items":{"type":"string"}},"statement":{"type":"string","minLength":1},"assumptions":{"type":"array","items":{"type":"string"}},"proof_markdown":{"type":"string","minLength":1},"dependency_fact_ids":{"type":"array","items":{"type":"string"}},"definitions_introduced":{"type":"object"},"external_source_ids":{"type":"array","items":{"type":"string"}},"candidate_type":{"type":"string","enum":["theorem","lemma","proposition","counterexample","observation"]},"task_revision":{"type":"integer","format":"int64"},"route_cancellation_epoch":{"type":"integer","format":"int64"}
            }},
            "ProofHintRequest":{"type":"object","additionalProperties":false,"required":["kind","content"],"properties":{
                "kind":{"type":"string","enum":["use_lemma","unfold_definition","try_strategy","avoid_tactic","focus_goal","add_intermediate"]},
                "content":{"type":"string","minLength":1},
                "scope":{"type":["object","null"],"additionalProperties":false,"properties":{"goal_id":{"type":["string","null"]},"proof_node_id":{"type":["string","null"]}}},
                "priority":{"type":["string","null"]},"author":{"type":"string"}
            }},
            "ProofSearchCommandRequest":{"type":"object","additionalProperties":false,"required":["expected_cancellation_epoch"],"properties":{
                "expected_cancellation_epoch":{"type":"integer","format":"int64"},"requested_by":{"type":"string"},"reason":{"type":["string","null"]}
            }}
        }},
        "security":[{"ActorBearer":[],"ActorId":[]}]
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use futures::{SinkExt, StreamExt};
    use research_core::ResearchConfig;
    use research_storage::SqliteStore;
    use research_worker_runtime::MockBackend;
    use tokio_tungstenite::{connect_async, tungstenite::Message as ClientMessage};
    use tower::ServiceExt;

    use super::*;

    async fn app() -> (Router, ResearchService, tempfile::TempDir) {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
            .await
            .expect("store");
        let service = ResearchService::new(
            store,
            Arc::new(MockBackend::default()),
            ResearchConfig {
                runtime_root: temp.path().join("runtime"),
                output_root: temp.path().join("output"),
                model: None,
                lean_project_root: None,
                proof_search_budget: research_domain::ProofSearchBudget::default(),
                ranking_weights: research_domain::RankingWeights::default(),
                planner_timeout_seconds: 5,
                worker_timeout_seconds: 5,
                verifier_timeout_seconds: 5,
            },
        );
        (router(service.clone()), service, temp)
    }

    #[tokio::test]
    async fn create_then_read_snapshot_through_http_contract() {
        let (app, _service, _temp) = app().await;
        let request = Request::builder().method("POST").uri("/api/v1/projects").header("content-type", "application/json")
            .body(Body::from(json!({"name":"arithmetic","problem":"Prove 1+1=2","budget":{"max_rounds":1,"max_parallel_workers":1,"max_minutes_per_task":1,"max_model_calls_per_task":1,"max_total_model_calls":4}}).to_string())).expect("request");
        let response = app.clone().oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::CREATED);
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json");
        let project_id = body
            .pointer("/data/project_id")
            .and_then(Value::as_str)
            .expect("project id");
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/projects/{project_id}/snapshot"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let snapshot: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json");
        assert_eq!(
            snapshot
                .pointer("/data/goals/0/statement")
                .and_then(Value::as_str),
            Some("Prove 1+1=2")
        );
        assert!(
            snapshot
                .pointer("/meta/event_cursor")
                .and_then(Value::as_i64)
                .is_some()
        );
    }

    #[tokio::test]
    async fn commands_require_idempotency_key() {
        let (app, service, _temp) = app().await;
        let project = service
            .create_project(
                "arithmetic".into(),
                ProblemContract {
                    original_problem: "p".into(),
                    target_statement: "p".into(),
                    assumptions: vec![],
                    success_criteria: "accepted".into(),
                    version: 1,
                },
                Budget::default(),
            )
            .await
            .expect("project");
        let request = Request::builder()
            .method("POST")
            .uri(format!(
                "/api/v1/projects/{}/commands/start",
                project.project_id
            ))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"expected_revision":project.revision,"reason":"test"}).to_string(),
            ))
            .expect("request");
        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json");
        assert_eq!(
            body.pointer("/error/code").and_then(Value::as_str),
            Some("invalid_request")
        );
    }

    #[tokio::test]
    async fn board_contract_supports_etag_idempotent_proposals_and_problem_revisions() {
        let (app, service, _temp) = app().await;
        let project = service
            .create_project(
                "board-api".into(),
                ProblemContract {
                    original_problem: "prove p".into(),
                    target_statement: "p".into(),
                    assumptions: vec!["A".into()],
                    success_criteria: "accepted".into(),
                    version: 1,
                },
                Budget::default(),
            )
            .await
            .expect("project");
        service
            .store()
            .store_artifact(
                &project.project_id,
                "board-test",
                "visible.txt",
                b"safe board content",
                0,
                vec![],
            )
            .await
            .expect("artifact");
        let board_uri = format!(
            "/api/v1/projects/{}/board?include=summary,artifacts,graph",
            project.project_id
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&board_uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("board");
        assert_eq!(response.status(), StatusCode::OK);
        let etag = response
            .headers()
            .get(header::ETAG)
            .expect("etag")
            .to_str()
            .expect("etag text")
            .to_owned();
        let board: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json");
        assert_eq!(board.pointer("/data/schema_version"), Some(&json!(1)));
        assert_eq!(
            board.pointer("/data/revision"),
            board.pointer("/meta/project_revision")
        );
        assert_eq!(
            board.pointer("/data/event_cursor"),
            board.pointer("/meta/event_cursor")
        );
        assert_eq!(
            board
                .pointer("/data/artifacts/0/filename")
                .and_then(Value::as_str),
            Some("visible.txt")
        );
        assert!(board.pointer("/data/artifacts/0/storage_path").is_none());
        let not_modified = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&board_uri)
                    .header(header::IF_NONE_MATCH, etag)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("304");
        assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);

        let revision = board
            .pointer("/data/revision")
            .and_then(Value::as_i64)
            .expect("revision");
        let proposal_body = json!({
            "expected_revision":revision,
            "title":"minimal counterexample",
            "method_summary":"assume a minimal counterexample and reduce it",
            "target_goal_ids":[],
            "required_fact_ids":[],
            "known_risks":["minimality may not preserve A"],
            "reason":"human whiteboard proposal"
        });
        let proposal_uri = format!("/api/v1/projects/{}/route-proposals", project.project_id);
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&proposal_uri)
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "board-proposal-1")
                    .body(Body::from(proposal_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("proposal");
        assert_eq!(first.status(), StatusCode::ACCEPTED);
        let first_body: Value =
            serde_json::from_slice(&to_bytes(first.into_body(), usize::MAX).await.expect("body"))
                .expect("json");
        let first_proposal_id = first_body
            .pointer("/data/proposal_id")
            .and_then(Value::as_str)
            .expect("proposal id")
            .to_owned();
        let replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&proposal_uri)
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "board-proposal-1")
                    .body(Body::from(proposal_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("replay");
        let replay_body: Value = serde_json::from_slice(
            &to_bytes(replay.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json");
        assert_eq!(
            replay_body
                .pointer("/data/proposal_id")
                .and_then(Value::as_str),
            Some(first_proposal_id.as_str())
        );
        assert_eq!(
            replay_body.pointer("/meta/project_revision"),
            first_body.pointer("/meta/project_revision")
        );
        assert_eq!(
            replay_body.pointer("/meta/event_cursor"),
            first_body.pointer("/meta/event_cursor")
        );
        let mut conflicting = proposal_body.clone();
        conflicting["title"] = json!("different proposal");
        let conflict = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&proposal_uri)
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "board-proposal-1")
                    .body(Body::from(conflicting.to_string()))
                    .expect("request"),
            )
            .await
            .expect("conflict");
        assert_eq!(conflict.status(), StatusCode::CONFLICT);

        let current = service
            .store()
            .get_project(&project.project_id)
            .await
            .expect("project");
        let revision_body = json!({
            "expected_revision":current.revision,
            "target_statement":"p under B",
            "assumptions":["B"],
            "success_criteria":"accepted with active dependency closure",
            "change_reason":"A was too strong",
            "replan":true
        });
        let revised = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/projects/{}/problem-revisions",
                        project.project_id
                    ))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "board-revision-1")
                    .body(Body::from(revision_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("revision");
        assert_eq!(revised.status(), StatusCode::ACCEPTED);
        let revised_project = service
            .store()
            .get_project(&project.project_id)
            .await
            .expect("project");
        assert_eq!(revised_project.contract.original_problem, "prove p");
        assert_eq!(revised_project.contract.version, 2);
        assert_eq!(revised_project.contract.target_statement, "p under B");
        let stale = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/projects/{}/problem-revisions",
                        project.project_id
                    ))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "board-revision-stale")
                    .body(Body::from(revision_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("stale revision");
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        let contract = openapi_document();
        assert!(
            contract
                .pointer("/paths/~1api~1v1~1projects~1{project_id}~1board/get/responses/304")
                .is_some()
        );
        assert_eq!(
            contract
                .pointer("/paths/~1api~1v1~1projects~1{project_id}~1route-proposals/post/requestBody/content/application~1json/schema/$ref")
                .and_then(Value::as_str),
            Some("#/components/schemas/HumanRouteProposalRequest")
        );
    }

    #[tokio::test]
    async fn publication_api_is_idempotent_and_queryable_without_model_work() {
        let (app, service, _temp) = app().await;
        let project = service
            .create_project(
                "publication-api".into(),
                ProblemContract {
                    original_problem: "prove p".into(),
                    target_statement: "p".into(),
                    assumptions: vec![],
                    success_criteria: "accepted".into(),
                    version: 1,
                },
                Budget::default(),
            )
            .await
            .expect("project");
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!(
                            "/api/v1/projects/{}/publications",
                            project.project_id
                        ))
                        .header("content-type", "application/json")
                        .header("Idempotency-Key", "publication-api-test")
                        .body(Body::from(json!({"allow_partial":false}).to_string()))
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::OK);
            let body: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body"),
            )
            .expect("json");
            assert_eq!(
                body.pointer("/data/status").and_then(Value::as_str),
                Some("blocked_by_evidence")
            );
        }
        let runs = service
            .store()
            .list_publications(&project.project_id)
            .await
            .expect("publication runs");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "blocked_by_evidence");
        assert!(runs[0].result.is_some());
    }

    #[tokio::test]
    async fn exposes_openapi_31_document() {
        let (app, _service, _temp) = app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/openapi.json")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json");
        assert_eq!(body.get("openapi").and_then(Value::as_str), Some("3.1.0"));
        assert!(body.pointer("/paths/~1api~1v1~1projects/post").is_some());
        assert_eq!(
            body.pointer(
                "/paths/~1api~1v1~1projects~1{project_id}~1commands~1start/post/parameters/1/name"
            )
            .and_then(Value::as_str),
            Some("Idempotency-Key")
        );
        assert_eq!(
            body.pointer("/paths/~1api~1v1~1projects~1{project_id}/get/parameters/0/in")
                .and_then(Value::as_str),
            Some("path")
        );
        assert!(
            body.pointer("/paths/~1api~1v1~1worker-nodes~1{node_id}~1heartbeat/post")
                .is_some()
        );
        assert!(
            body.pointer(
                "/paths/~1api~1v1~1facts~1{fact_id}~1commands~1request-independent-proof/post"
            )
            .is_some()
        );
        assert!(
            body.pointer(
                "/paths/~1api~1v1~1projects~1{project_id}~1routes~1{route_id}~1commands~1prune/post"
            )
            .is_some()
        );
        assert!(
            body.pointer("/paths/~1api~1v1~1projects~1{project_id}~1routes~1commands~1merge/post")
                .is_some()
        );
        assert!(
            body.pointer(
                "/paths/~1api~1v1~1projects~1{project_id}~1tasks~1{task_id}~1commands~1retry/post"
            )
            .is_some()
        );
        assert!(
            body.pointer(
                "/paths/~1api~1v1~1projects~1{project_id}~1worker-instances~1{worker_instance_id}~1commands~1quarantine/post"
            )
            .is_some()
        );
        assert!(
            body.pointer("/components/securitySchemes/ActorBearer")
                .is_some()
        );
    }

    #[tokio::test]
    async fn websocket_replays_cursor_and_accepts_bidirectional_ping() {
        let (app, service, _temp) = app().await;
        let project = service
            .create_project(
                "websocket smoke".into(),
                ProblemContract {
                    original_problem: "p".into(),
                    target_statement: "p".into(),
                    assumptions: vec![],
                    success_criteria: "accepted".into(),
                    version: 1,
                },
                Budget::default(),
            )
            .await
            .expect("project");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let url = format!(
            "ws://{address}/api/v1/projects/{}/ws?after=0",
            project.project_id
        );
        let (mut socket, _) = connect_async(url).await.expect("websocket connect");
        let replayed = socket
            .next()
            .await
            .expect("replayed message")
            .expect("valid replayed message")
            .into_text()
            .expect("text");
        let replayed: Value = serde_json::from_str(&replayed).expect("json");
        assert_eq!(replayed.get("type").and_then(Value::as_str), Some("event"));
        assert_eq!(
            replayed.pointer("/data/type").and_then(Value::as_str),
            Some("project.created")
        );
        socket
            .send(ClientMessage::Text(
                json!({"type":"ping"}).to_string().into(),
            ))
            .await
            .expect("ping");
        let mut received_pong = false;
        for _ in 0..3 {
            let message = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
                .await
                .expect("websocket response timeout")
                .expect("websocket response")
                .expect("valid websocket response");
            if let Ok(text) = message.into_text()
                && serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|value| value.get("type").and_then(Value::as_str).map(str::to_owned))
                    .as_deref()
                    == Some("pong")
            {
                received_pong = true;
                break;
            }
        }
        assert!(received_pong);
        server.abort();
    }

    #[tokio::test]
    async fn configured_actor_auth_protects_reads_and_enforces_roles() {
        let (app, service, _temp) = app().await;
        let existing = service
            .create_project(
                "existing".into(),
                ProblemContract {
                    original_problem: "p".into(),
                    target_statement: "p".into(),
                    assumptions: vec![],
                    success_criteria: "accepted".into(),
                    version: 1,
                },
                Budget::default(),
            )
            .await
            .expect("project");
        service
            .store()
            .bootstrap_admin("admin", "Admin", "admin-token-123456789")
            .await
            .expect("admin");
        service
            .store()
            .create_actor("viewer", "Viewer", "viewer", "viewer-token-12345678")
            .await
            .expect("viewer");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/projects/{}", existing.project_id))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/projects/{}", existing.project_id))
                    .header("X-Actor-Id", "viewer")
                    .header("Authorization", "Bearer viewer-token-12345678")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);

        let project_body = json!({"name":"forbidden","problem":"p"}).to_string();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/projects")
                    .header("content-type", "application/json")
                    .header("X-Actor-Id", "viewer")
                    .header("Authorization", "Bearer viewer-token-12345678")
                    .body(Body::from(project_body.clone()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/projects")
                    .header("content-type", "application/json")
                    .header("X-Actor-Id", "admin")
                    .header("Authorization", "Bearer admin-token-123456789")
                    .body(Body::from(project_body))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::CREATED);
    }
}
