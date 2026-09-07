// The router and generated contract intentionally enumerate the complete P0 surface in one place.
#![allow(clippy::too_many_arguments, clippy::too_many_lines)]
#![recursion_limit = "256"]

pub mod research_v2;

use std::{
    convert::Infallible,
    path::{Component, PathBuf},
    sync::LazyLock,
};

use async_stream::stream;
use axum::{
    Json, Router,
    body::Body,
    extract::{
        Extension, Path, Query, Request, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{MethodRouter, get, post},
};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use research_core::{CoreError, ResearchService, render_problem_document_markdown};
use research_domain::{
    Actor, BoardCapabilities, Budget, CandidateSubmission, CommandMode, HumanRouteCreateRequest,
    HumanRouteProposalRequest, ProblemContract, ProblemDocument, ProblemRevisionRequest,
    ReviewMode,
};
use research_storage::{
    BoardInclude, CommandDraft, ProblemDraftConfirmationRequest, SqliteStore, StorageError,
    TaskLeaseCompletionRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_stream::Stream;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use ulid::Ulid;

#[derive(Clone)]
pub struct AppState {
    pub service: ResearchService,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum EndpointMethod {
    Get,
    Post,
}

impl EndpointMethod {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
        }
    }

    fn matches(self, method: &Method) -> bool {
        matches!(
            (self, method),
            (Self::Get, &Method::GET) | (Self::Post, &Method::POST)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EndpointAuth {
    Public,
    Actor,
    WorkerToken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EndpointResponse {
    Json,
    Board,
    ObligationGraph,
    EventStream,
    Binary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EndpointDescriptor {
    path: &'static str,
    method: EndpointMethod,
    summary: &'static str,
    auth: EndpointAuth,
    request_schema: Option<&'static str>,
    idempotency_required: bool,
    success_status: &'static str,
    response: EndpointResponse,
    documented: bool,
}

const fn endpoint_descriptor(
    path: &'static str,
    method: EndpointMethod,
    summary: &'static str,
    auth: EndpointAuth,
    request_schema: Option<&'static str>,
    idempotency_required: bool,
    success_status: &'static str,
    response: EndpointResponse,
    documented: bool,
) -> EndpointDescriptor {
    EndpointDescriptor {
        path,
        method,
        summary,
        auth,
        request_schema,
        idempotency_required,
        success_status,
        response,
        documented,
    }
}

struct ContractEndpoint {
    descriptor: EndpointDescriptor,
    method_router: MethodRouter<AppState>,
}

impl ContractEndpoint {
    fn new(descriptor: EndpointDescriptor, method_router: MethodRouter<AppState>) -> Self {
        Self {
            descriptor,
            method_router,
        }
    }
}

struct ApiContract {
    router: Router<AppState>,
    descriptors: Vec<EndpointDescriptor>,
}

impl ApiContract {
    fn new() -> Self {
        Self {
            router: Router::new(),
            descriptors: Vec::new(),
        }
    }

    fn add(mut self, endpoint: ContractEndpoint) -> Self {
        self.router = self
            .router
            .route(endpoint.descriptor.path, endpoint.method_router);
        self.descriptors.push(endpoint.descriptor);
        self
    }
}

macro_rules! endpoint {
    (hidden public get $path:literal => $handler:ident, $summary:literal) => {
        ContractEndpoint::new(
            endpoint_descriptor(
                $path,
                EndpointMethod::Get,
                $summary,
                EndpointAuth::Public,
                None,
                false,
                "200",
                EndpointResponse::Json,
                false,
            ),
            get($handler),
        )
    };
    (public created $path:literal => $handler:ident, $summary:literal, $schema:literal) => {
        ContractEndpoint::new(
            endpoint_descriptor(
                $path,
                EndpointMethod::Post,
                $summary,
                EndpointAuth::Public,
                Some($schema),
                false,
                "201",
                EndpointResponse::Json,
                true,
            ),
            post($handler),
        )
    };
    (worker post $path:literal => $handler:ident, $summary:literal, $schema:literal) => {
        ContractEndpoint::new(
            endpoint_descriptor(
                $path,
                EndpointMethod::Post,
                $summary,
                EndpointAuth::WorkerToken,
                Some($schema),
                false,
                "202",
                EndpointResponse::Json,
                true,
            ),
            post($handler),
        )
    };
    (actor get $path:literal => $handler:ident, $summary:literal) => {
        ContractEndpoint::new(
            endpoint_descriptor(
                $path,
                EndpointMethod::Get,
                $summary,
                EndpointAuth::Actor,
                None,
                false,
                "200",
                EndpointResponse::Json,
                true,
            ),
            get($handler),
        )
    };
    (actor board $path:literal => $handler:ident, $summary:literal) => {
        ContractEndpoint::new(
            endpoint_descriptor(
                $path,
                EndpointMethod::Get,
                $summary,
                EndpointAuth::Actor,
                None,
                false,
                "200",
                EndpointResponse::Board,
                true,
            ),
            get($handler),
        )
    };
    (actor obligation_graph $path:literal => $handler:ident, $summary:literal) => {
        ContractEndpoint::new(
            endpoint_descriptor(
                $path,
                EndpointMethod::Get,
                $summary,
                EndpointAuth::Actor,
                None,
                false,
                "200",
                EndpointResponse::ObligationGraph,
                true,
            ),
            get($handler),
        )
    };
    (actor events $path:literal => $handler:ident, $summary:literal) => {
        ContractEndpoint::new(
            endpoint_descriptor(
                $path,
                EndpointMethod::Get,
                $summary,
                EndpointAuth::Actor,
                None,
                false,
                "200",
                EndpointResponse::EventStream,
                true,
            ),
            get($handler),
        )
    };
    (actor websocket $path:literal => $handler:ident, $summary:literal) => {
        ContractEndpoint::new(
            endpoint_descriptor(
                $path,
                EndpointMethod::Get,
                $summary,
                EndpointAuth::Actor,
                None,
                false,
                "101",
                EndpointResponse::Json,
                true,
            ),
            get($handler),
        )
    };
    (actor download $path:literal => $handler:ident, $summary:literal) => {
        ContractEndpoint::new(
            endpoint_descriptor(
                $path,
                EndpointMethod::Get,
                $summary,
                EndpointAuth::Actor,
                None,
                false,
                "200",
                EndpointResponse::Binary,
                true,
            ),
            get($handler),
        )
    };
    (actor post $path:literal => $handler:ident, $summary:literal, $schema:literal) => {
        ContractEndpoint::new(
            endpoint_descriptor(
                $path,
                EndpointMethod::Post,
                $summary,
                EndpointAuth::Actor,
                Some($schema),
                false,
                "202",
                EndpointResponse::Json,
                true,
            ),
            post($handler),
        )
    };
    (actor created $path:literal => $handler:ident, $summary:literal, $schema:literal) => {
        ContractEndpoint::new(
            endpoint_descriptor(
                $path,
                EndpointMethod::Post,
                $summary,
                EndpointAuth::Actor,
                Some($schema),
                false,
                "201",
                EndpointResponse::Json,
                true,
            ),
            post($handler),
        )
    };
    (actor command $path:literal => $handler:ident, $summary:literal, $schema:literal) => {
        ContractEndpoint::new(
            endpoint_descriptor(
                $path,
                EndpointMethod::Post,
                $summary,
                EndpointAuth::Actor,
                Some($schema),
                true,
                "202",
                EndpointResponse::Json,
                true,
            ),
            post($handler),
        )
    };
    (actor idempotent_created $path:literal => $handler:ident, $summary:literal, $schema:literal) => {
        ContractEndpoint::new(
            endpoint_descriptor(
                $path,
                EndpointMethod::Post,
                $summary,
                EndpointAuth::Actor,
                Some($schema),
                true,
                "201",
                EndpointResponse::Json,
                true,
            ),
            post($handler),
        )
    };
}

fn endpoint_contract() -> ApiContract {
    ApiContract::new()
        .add(endpoint!(hidden public get "/health" => health, "Health check"))
        .add(endpoint!(hidden public get "/api/openapi.json" => openapi, "OpenAPI document"))
        .add(endpoint!(public created "/api/v1/actors/bootstrap" => bootstrap_actor, "Bootstrap the first administrator", "BootstrapActorRequest"))
        .add(endpoint!(actor created "/api/v1/actors" => create_actor, "Create a local actor and access token", "CreateActorRequest"))
        .add(endpoint!(actor created "/api/v1/worker-nodes" => register_worker_node, "Register a distributed worker node", "RegisterWorkerNodeRequest"))
        .add(endpoint!(worker post "/api/v1/worker-nodes/{node_id}/heartbeat" => heartbeat_worker_node, "Authenticate and heartbeat a worker node epoch", "NodeHeartbeatRequest"))
        .add(endpoint!(worker post "/api/v1/projects/{project_id}/distributed/leases/next" => lease_distributed_task, "Lease the next capability-compatible task", "LeaseTaskRequest"))
        .add(endpoint!(actor get "/api/v1/task-leases/{lease_id}" => get_task_lease, "Get a task lease"))
        .add(endpoint!(worker post "/api/v1/task-leases/{lease_id}/renew" => renew_task_lease, "Renew an active task lease with epoch checks", "RenewLeaseRequest"))
        .add(endpoint!(worker post "/api/v1/task-leases/{lease_id}/steers" => poll_task_steers, "Poll pending safe-point steering for an active task lease", "LeaseSteersRequest"))
        .add(endpoint!(worker post "/api/v1/task-leases/{lease_id}/complete" => complete_task_lease, "Commit remote worker output with stale-result rejection", "CompleteLeaseRequest"))
        .add(endpoint!(actor command "/api/v1/problem-drafts" => generate_problem_draft, "Begin generating a reviewable problem document from a prompt and bounded workspace material", "GenerateProblemDraftRequest"))
        .add(endpoint!(actor get "/api/v1/problem-drafts/{draft_id}" => get_problem_draft, "Get a generated problem draft for human review"))
        .add(endpoint!(actor command "/api/v1/problem-drafts/{draft_id}/commands/cancel" => cancel_problem_draft, "Cancel the active generation attempt for a problem draft", "ProblemDraftControlRequest"))
        .add(endpoint!(actor command "/api/v1/problem-drafts/{draft_id}/commands/retry" => retry_problem_draft, "Retry a failed problem draft as a new durable attempt", "ProblemDraftControlRequest"))
        .add(endpoint!(actor command "/api/v1/problem-drafts/{draft_id}/commands/confirm" => confirm_problem_draft, "Confirm a problem document, create its project, and optionally start research", "ConfirmProblemDraftRequest"))
        .add(endpoint!(actor created "/api/v1/projects" => create_project, "Create project", "CreateProjectRequest"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}" => get_project, "Get project"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/status" => project_status, "Get aggregate status"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/snapshot" => snapshot, "Get consistent snapshot"))
        .add(endpoint!(actor board "/api/v1/projects/{project_id}/board" => board, "Get the atomic MathCat Lab board projection"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/problem-revisions" => revise_problem, "Create an auditable problem-contract revision and invalidate stale work", "ProblemRevisionRequest"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/route-proposals" => route_proposals, "List auditable human route proposals"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/route-proposals" => propose_route, "Propose a route for Planner validation", "HumanRouteProposalRequest"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/latest" => latest, "Get latest report metadata"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/publications" => publications, "List idempotent publication runs"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/publications" => create_publication, "Create or replay a revision-pinned publication run", "CreatePublicationRequest"))
        .add(endpoint!(actor get "/api/v1/publications/{publication_id}" => publication, "Get a publication run"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/goals/{goal_id}/closures" => goal_closures, "List auditable Goal Completion evidence"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/rounds" => rounds, "List rounds"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/rounds/current" => current_round, "Get current round"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/workers" => workers, "List workers"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/workers/{worker_id}" => worker, "Get worker"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/tasks" => tasks, "List tasks"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/tasks" => create_human_task, "Create a human-assigned task", "CommandRequest"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/tasks/{task_id}" => task, "Get task"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/tasks/{task_id}/attempts" => task_attempts, "List bounded task attempts and failure signatures"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/tasks/{task_id}/contract" => task_contract, "Get the immutable task contract"))
        .add(endpoint!(actor get "/api/v1/tasks/{task_id}/context-packet" => task_context_packet, "Get the exact context packet delivered to a worker"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/tasks/{task_id}/steers" => task_steers, "List queued and applied task steering"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/tasks/{task_id}/commands/steer" => steer_task, "Queue task guidance for the next safe point", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/tasks/{task_id}/commands/pause" => pause_task, "Pause a task", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/tasks/{task_id}/commands/resume" => resume_task, "Resume a task", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/tasks/{task_id}/commands/reassign" => reassign_task, "Reassign a task", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/tasks/{task_id}/commands/cancel" => cancel_task, "Cancel a task and reject stale output", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/tasks/{task_id}/commands/set-priority" => set_task_priority, "Change task priority", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/tasks/{task_id}/commands/retry" => retry_task, "Queue a bounded retry under the task retry policy", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/tasks/{task_id}/commands/rebuild-context" => rebuild_task_context, "Rebuild an invalid task context as a new immutable packet and contract version", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/tasks/{task_id}/commands/rebuild-context" => rebuild_task_context_by_task, "Rebuild task context using the architecture-compatible task-scoped path", "CommandRequest"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/routes" => routes, "List routes"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/routes" => create_human_route, "Create and immediately schedule an auditable human-authored route", "HumanRouteCreateRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/routes/{route_id}/commands/approve" => approve_route, "Approve budget eligibility for a route when human approval is enabled", "CommandRequest"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/route-families" => route_families, "List semantic route families and canonical routes"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/route-tombstones" => route_tombstones, "List pruned route fingerprints and revival conditions"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/routes/{route_id}" => route, "Get route"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/routes/{route_id}/progress-ledger" => route_progress_ledger, "List material route progress entries"))
        .add(endpoint!(actor get "/api/v1/routes/{route_id}/progress-digest" => route_progress_digest, "Get a provenance-aware route progress digest"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/worker-instances" => worker_instances, "List worker process instances, handshakes, and health"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/planning/delta" => planning_delta, "Get the current or most recently consumed research delta"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/planning/revisions" => planning_revisions, "List atomic plan revisions"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/planning/revisions/{plan_revision_id}" => planning_revision, "Get route, fact, bottleneck, and task decisions for one plan revision"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/strategy/states" => strategy_states, "List immutable Strategy Director states and macro audits"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/strategy/latest" => latest_strategy_state, "Get the latest whole-proof strategy state"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/bottlenecks" => bottlenecks, "List the persistent bottleneck register"))
        .add(endpoint!(actor obligation_graph "/api/v1/projects/{project_id}/obligations" => proof_obligations, "Get the proof-obligation graph and candidate coverage ledger"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/facts/{fact_id}/planning-impact" => fact_planning_impact, "Explain how planning consumed or deferred a fact"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/planner/health" => planner_health, "Get planner circuit-breaker and degraded-mode health"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/context/digest" => context_digest, "Get the provenance-aware project planning digest"))
        .add(endpoint!(actor get "/api/v1/system/storage/health" => storage_health, "Get state queue, outbox, lease, and reconciliation health"))
        .add(endpoint!(actor get "/api/v1/system/state-writer/status" => state_writer_status, "Get bounded SQLite StateWriter status"))
        .add(endpoint!(actor post "/api/v1/system/reconciliation/commands/run" => run_reconciliation, "Run an authorized consistency reconciliation scan", "ReconciliationRequest"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/graphs/{graph_type}" => graph, "Get goal, hypothesis, fact, or six-kind combined graph"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/graphs/combined/delta" => graph_delta, "Recover a snapshot-assisted combined graph delta from an event cursor"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/verifications" => verifications, "List verifications"))
        .add(endpoint!(actor get "/api/v1/verifications/{verification_id}" => verification, "Get verification"))
        .add(endpoint!(actor get "/api/v1/verifications/{verification_id}/case" => verification_case_by_verification, "Get trusted verification case"))
        .add(endpoint!(actor get "/api/v1/verification-cases/{case_id}" => verification_case, "Get verification case with policy and evidence"))
        .add(endpoint!(actor get "/api/v1/verification-cases/{case_id}/snapshot" => verification_snapshot, "Get immutable verification snapshot"))
        .add(endpoint!(actor get "/api/v1/verification-cases/{case_id}/checks" => verification_checks, "List verification checks"))
        .add(endpoint!(actor get "/api/v1/verification-cases/{case_id}/findings" => verification_findings, "List verification findings"))
        .add(endpoint!(actor get "/api/v1/verification-cases/{case_id}/evidence" => verification_evidence, "List verification evidence"))
        .add(endpoint!(actor get "/api/v1/verification-cases/{case_id}/semantic-contract" => semantic_contract, "Get semantic contract"))
        .add(endpoint!(actor get "/api/v1/verification-cases/{case_id}/formalization" => formalization, "Get Lean formalization"))
        .add(endpoint!(actor get "/api/v1/verification-cases/{case_id}/alignment-reviews" => alignment_reviews, "List semantic alignment reviews"))
        .add(endpoint!(actor get "/api/v1/verification-cases/{case_id}/backend-runs" => backend_runs, "List verification backend runs"))
        .add(endpoint!(actor get "/api/v1/verification-cases/{case_id}/package" => verification_package, "Get content-addressed verification package"))
        .add(endpoint!(actor get "/api/v1/verification-cases/{case_id}/replays" => verification_replays, "List independent replays"))
        .add(endpoint!(actor get "/api/v1/formalizations/{formalization_id}" => get_formalization, "Get formalization and current source"))
        .add(endpoint!(actor get "/api/v1/formalizations/{formalization_id}/semantic-contract" => formalization_semantic_contract, "Get formalization semantic contract"))
        .add(endpoint!(actor get "/api/v1/formalizations/{formalization_id}/alignment" => formalization_alignment, "Get formalization alignment reviews"))
        .add(endpoint!(actor get "/api/v1/formalizations/{formalization_id}/goals" => formalization_goals, "List current open Pantograph goals"))
        .add(endpoint!(actor get "/api/v1/formalizations/{formalization_id}/proof-tree" => formalization_proof_tree, "Page through proof tree nodes and edges"))
        .add(endpoint!(actor get "/api/v1/formalizations/{formalization_id}/attempts" => formalization_attempts, "List visible tactic attempts"))
        .add(endpoint!(actor command "/api/v1/formalizations/{formalization_id}/hints" => add_formalization_hint, "Add a hint for the next proof-node expansion", "ProofHintRequest"))
        .add(endpoint!(actor command "/api/v1/formalizations/{formalization_id}/proof-nodes/{node_id}/commands/prune" => prune_formalization_branch, "Prune a proof branch immediately", "ProofSearchCommandRequest"))
        .add(endpoint!(actor command "/api/v1/formalizations/{formalization_id}/commands/cancel" => cancel_formalization_search, "Cancel an interactive proof search", "ProofSearchCommandRequest"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/uncertainties" => uncertainties, "List uncertainties"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/sources" => sources, "List unverified source leads"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/sources/ingestions" => source_ingestions, "List source insertion, deduplication, and rejection provenance"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/experiment-capsules" => experiment_capsules, "List content-addressed unverified experiment capsules"))
        .add(endpoint!(actor get "/api/v1/experiment-capsules/{capsule_id}" => experiment_capsule, "Get an exact experiment capsule and replay metadata"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/failure-patterns" => failure_patterns, "List failure patterns"))
        .add(endpoint!(actor get "/api/v1/facts/{fact_id}" => fact, "Get fact"))
        .add(endpoint!(actor get "/api/v1/facts/catalog" => fact_catalog, "List the global fact catalog"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/fact-imports" => fact_imports, "List cross-project fact imports"))
        .add(endpoint!(actor idempotent_created "/api/v1/projects/{project_id}/fact-imports" => import_fact, "Import a fact and its complete assured dependency closure", "ImportFactRequest"))
        .add(endpoint!(actor get "/api/v1/facts/{fact_id}/assurance" => fact_assurance, "List current and historical fact assurances"))
        .add(endpoint!(actor get "/api/v1/facts/{fact_id}/verification-history" => fact_verification_history, "List verification, challenge, replay, and assurance history"))
        .add(endpoint!(actor get "/api/v1/facts/{fact_id}/formalization" => fact_formalization, "Get the latest fact formalization"))
        .add(endpoint!(actor get "/api/v1/facts/{fact_id}/dependency-closure" => fact_dependency_closure, "Get dependency closure with assurance levels"))
        .add(endpoint!(actor get "/api/v1/facts/{fact_id}/impact" => fact_impact, "Get fact impact"))
        .add(endpoint!(actor command "/api/v1/facts/{fact_id}/commands/challenge" => challenge_fact, "Challenge a fact and enqueue independent reverification", "FactGovernanceRequest"))
        .add(endpoint!(actor command "/api/v1/facts/{fact_id}/commands/reverify" => reverify_fact, "Reverify a fact without overwriting its assurance history", "FactGovernanceRequest"))
        .add(endpoint!(actor command "/api/v1/facts/{fact_id}/commands/request-formalization" => request_fact_formalization, "Request a fresh Lean formalization and certification", "FactGovernanceRequest"))
        .add(endpoint!(actor command "/api/v1/facts/{fact_id}/commands/request-independent-proof" => request_independent_proof, "Request a separately snapshotted proof with an extra independent review", "FactGovernanceRequest"))
        .add(endpoint!(actor command "/api/v1/facts/{fact_id}/commands/suspend" => suspend_fact, "Suspend a fact and invalidate downstream use", "FactGovernanceRequest"))
        .add(endpoint!(actor command "/api/v1/facts/{fact_id}/commands/revoke" => revoke_fact, "Revoke a fact and apply dependency impact", "FactGovernanceRequest"))
        .add(endpoint!(actor events "/api/v1/projects/{project_id}/events" => events, "Subscribe to SSE events"))
        .add(endpoint!(actor websocket "/api/v1/projects/{project_id}/ws" => websocket_events, "Resume a bidirectional WebSocket event stream with heartbeat usage"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/security-audit" => security_audit, "List append-only authorization decisions"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/artifacts" => artifacts, "List artifacts"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/artifacts/{artifact_id}" => artifact, "Get artifact metadata"))
        .add(endpoint!(actor download "/api/v1/projects/{project_id}/artifacts/{artifact_id}/content" => artifact_content, "Download artifact"))
        .add(endpoint!(actor download "/api/v1/projects/{project_id}/reports/latest" => latest_report_content, "Download latest report"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/commands/{command_id}" => command, "Get command status"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/commands/start" => start_project, "Start project", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/commands/pause" => pause_project, "Pause project", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/commands/resume" => resume_project, "Resume project", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/commands/stop" => stop_project, "Stop project", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/commands/replan" => replan, "Trigger replan", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/commands/goal-review" => goal_review, "Cancel unfinished work and immediately start a focused goal review", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/commands/review-policy" => review_policy, "Update the durable human review policy", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/commands/settings" => research_settings, "Atomically update project budget and human review policy", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/suggestions" => add_suggestion, "Add next-round suggestion", "SuggestionRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/routes/{route_id}/commands/pause" => pause_route, "Pause route", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/routes/{route_id}/commands/resume" => resume_route, "Resume route", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/routes/{route_id}/commands/stop" => stop_route, "Stop route", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/routes/{route_id}/commands/prune" => prune_route, "Prune and tombstone a route", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/routes/commands/merge" => merge_routes, "Merge duplicate routes into a canonical route", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/routes/{route_id}/commands/revive" => revive_route, "Revive a tombstoned route with explicit evidence", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/workers/{worker_id}/commands/stop" => stop_worker, "Stop worker", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/workers/{worker_id}/commands/pause" => pause_worker, "Pause worker task intake", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/workers/{worker_id}/commands/resume" => resume_worker, "Resume worker task intake", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/worker-instances/{worker_instance_id}/commands/quarantine" => quarantine_worker_instance, "Quarantine a faulty worker instance and expire its lease", "CommandRequest"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/usage" => usage_summary, "Get measured model-call and token usage"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/budgets" => budget_overrides, "List scoped budget overrides"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/commands/adjust-budget" => adjust_budget, "Adjust a scoped budget without dropping below consumed usage", "CommandRequest"))
        .add(endpoint!(actor get "/api/v1/projects/{project_id}/questions" => human_questions, "List proactive human clarification questions"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/questions/{question_id}/commands/answer" => answer_question, "Answer a clarification question and resume affected work", "CommandRequest"))
        .add(endpoint!(actor command "/api/v1/projects/{project_id}/candidates" => submit_candidate, "Submit candidate claim", "CandidateSubmission"))
}

static ENDPOINT_DESCRIPTORS: LazyLock<Vec<EndpointDescriptor>> =
    LazyLock::new(|| endpoint_contract().descriptors);

fn endpoint_policy(method: &Method, path: &str) -> Option<&'static EndpointDescriptor> {
    ENDPOINT_DESCRIPTORS
        .iter()
        .find(|endpoint| endpoint.method.matches(method) && route_path_matches(endpoint.path, path))
}

fn endpoint_auth(method: &Method, path: &str) -> EndpointAuth {
    endpoint_policy(method, path).map_or(EndpointAuth::Actor, |endpoint| endpoint.auth)
}

fn route_path_matches(template: &str, actual: &str) -> bool {
    let mut template_segments = template.split('/');
    let mut actual_segments = actual.split('/');
    loop {
        match (template_segments.next(), actual_segments.next()) {
            (Some(template_segment), Some(actual_segment)) => {
                let is_parameter =
                    template_segment.starts_with('{') && template_segment.ends_with('}');
                if (is_parameter && actual_segment.is_empty())
                    || (!is_parameter && template_segment != actual_segment)
                {
                    return false;
                }
            }
            (None, None) => return true,
            _ => return false,
        }
    }
}

pub fn router(service: ResearchService) -> Router {
    let state = AppState { service };
    endpoint_contract()
        .router
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
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let path = request.uri().path().to_owned();
    let auth = endpoint_auth(request.method(), &path);
    if request.method() != Method::OPTIONS && auth == EndpointAuth::Actor {
        let actor = authenticate_actor(state.service.store(), request.headers()).await?;
        authorize_actor(
            state.service.store(),
            &actor,
            None,
            "api_access",
            "http_path",
            &path,
            "viewer",
        )
        .await?;
        request.extensions_mut().insert(actor);
    }
    Ok(next.run(request).await)
}

#[derive(Debug, Serialize)]
pub struct ApiEnvelope<T: Serialize> {
    pub data: Option<T>,
    pub meta: ResponseMeta,
    pub error: Option<ApiErrorBody>,
}

#[derive(Debug, Serialize)]
pub struct ResponseMeta {
    pub request_id: String,
    pub project_revision: Option<i64>,
    pub event_cursor: Option<i64>,
}

#[derive(Debug, Serialize)]
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
            CoreError::InvalidProblemMaterial(message) => Self {
                status: StatusCode::BAD_REQUEST,
                code: "invalid_problem_material",
                message,
                details: json!({}),
                retryable: false,
            },
            CoreError::Io(_) => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "filesystem_error",
                message: "a filesystem operation failed".into(),
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    pub review_mode: Option<ReviewMode>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateProblemDraftRequest {
    pub prompt: String,
    #[serde(default = "default_context_directory")]
    pub context_dir: String,
}

fn default_context_directory() -> String {
    ".".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmProblemDraftRequest {
    pub expected_revision: i64,
    pub expected_document_hash: String,
    pub document: Option<ProblemDocument>,
    #[serde(default)]
    pub acknowledge_material_warnings: bool,
    #[serde(default = "default_start_after_confirmation")]
    pub start: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProblemDraftControlRequest {
    pub expected_revision: i64,
}

const fn default_start_after_confirmation() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct ProblemDraftView {
    pub draft: research_domain::ProblemDraft,
    pub preview_markdown: Option<String>,
}

impl From<research_domain::ProblemDraft> for ProblemDraftView {
    fn from(draft: research_domain::ProblemDraft) -> Self {
        let preview_markdown = draft
            .document
            .as_ref()
            .map(render_problem_document_markdown);
        Self {
            draft,
            preview_markdown,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ProblemDraftConfirmationView {
    pub confirmation: research_domain::ProblemDraftConfirmation,
    pub preview_markdown: String,
}

#[derive(Debug, Serialize)]
pub struct ProblemDraftControlView {
    pub draft: research_domain::ProblemDraft,
    pub preview_markdown: Option<String>,
    pub replayed: bool,
}

impl ProblemDraftControlView {
    fn new(draft: research_domain::ProblemDraft, replayed: bool) -> Self {
        let preview_markdown = draft
            .document
            .as_ref()
            .map(render_problem_document_markdown);
        Self {
            draft,
            preview_markdown,
            replayed,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePublicationRequest {
    #[serde(default)]
    pub allow_partial: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapActorRequest {
    pub actor_id: String,
    pub display_name: String,
    pub token: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateActorRequest {
    pub actor_id: String,
    pub display_name: String,
    pub role: String,
    pub token: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportFactRequest {
    pub content_hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterWorkerNodeRequest {
    pub node_id: String,
    pub display_name: String,
    pub capabilities: Value,
    pub token: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeHeartbeatRequest {
    pub node_epoch: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseTaskRequest {
    pub node_id: String,
    pub node_epoch: i64,
    #[serde(default = "default_lease_ttl")]
    pub ttl_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenewLeaseRequest {
    pub node_id: String,
    pub node_epoch: i64,
    pub lease_epoch: i64,
    #[serde(default = "default_lease_ttl")]
    pub ttl_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseSteersRequest {
    pub node_id: String,
    pub node_epoch: i64,
    pub lease_epoch: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteLeaseRequest {
    pub node_id: String,
    pub node_epoch: i64,
    pub lease_epoch: i64,
    pub output: research_domain::WorkerOutput,
    #[serde(default)]
    pub incorporated_steer_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRequest {
    pub expected_revision: i64,
    #[serde(default)]
    pub reason: String,
    #[serde(default = "empty_object")]
    pub payload: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    Extension(actor): Extension<Actor>,
    Json(body): Json<CreateActorRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<research_domain::Actor>>), ApiError> {
    authorize_actor(
        state.service.store(),
        &actor,
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
    Extension(actor): Extension<Actor>,
    Json(body): Json<RegisterWorkerNodeRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<research_domain::WorkerNode>>), ApiError> {
    authorize_actor(
        state.service.store(),
        &actor,
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
    Extension(actor): Extension<Actor>,
) -> Result<Json<ApiEnvelope<research_domain::TaskLease>>, ApiError> {
    let lease = state.service.store().get_task_lease(&lease_id).await?;
    authorize_actor(
        state.service.store(),
        &actor,
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

async fn poll_task_steers(
    State(state): State<AppState>,
    Path(lease_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<LeaseSteersRequest>,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let token = worker_node_token(&headers)?;
    let steers = state
        .service
        .store()
        .pending_distributed_task_steers(
            &lease_id,
            &body.node_id,
            token,
            body.node_epoch,
            body.lease_epoch,
        )
        .await?;
    let lease = state.service.store().get_task_lease(&lease_id).await?;
    Ok(Json(
        success(
            &state.service,
            &lease.project_id,
            json!({"lease_id":lease_id,"steers":steers}),
        )
        .await?,
    ))
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
        .complete_distributed_task(TaskLeaseCompletionRequest {
            lease_id: &lease_id,
            node_id: &body.node_id,
            token,
            node_epoch: body.node_epoch,
            lease_epoch: body.lease_epoch,
            output: &body.output,
            incorporated_steer_ids: &body.incorporated_steer_ids,
        })
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

fn relative_context_directory(value: &str) -> Result<PathBuf, ApiError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ApiError::invalid("context_dir must not be empty"));
    }
    let path = PathBuf::from(trimmed);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(ApiError::invalid(
            "context_dir must remain relative to the configured problem material root",
        ));
    }
    Ok(path)
}

async fn generate_problem_draft(
    State(state): State<AppState>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<GenerateProblemDraftRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<ProblemDraftView>>), ApiError> {
    let key = idempotency_key(&headers)?;
    authorize_actor(
        state.service.store(),
        &actor,
        None,
        "generate_problem_draft",
        "problem_draft",
        "new",
        "researcher",
    )
    .await?;
    if body.prompt.trim().is_empty() {
        return Err(ApiError::invalid("prompt must not be empty"));
    }
    let context_dir = relative_context_directory(&body.context_dir)?;
    let draft = state
        .service
        .begin_problem_draft_generation(&actor.actor_id, &key, body.prompt.trim(), &context_dir)
        .await?;
    state
        .service
        .store()
        .record_security_audit(
            None,
            Some(&actor.actor_id),
            "generate_problem_draft",
            "problem_draft",
            &draft.draft_id,
            "allowed",
            None,
            Some(&key),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(unscoped_success(ProblemDraftView::from(draft))),
    ))
}

async fn get_problem_draft(
    State(state): State<AppState>,
    Path(draft_id): Path<String>,
    Extension(actor): Extension<Actor>,
) -> Result<Json<ApiEnvelope<ProblemDraftView>>, ApiError> {
    authorize_actor(
        state.service.store(),
        &actor,
        None,
        "read_problem_draft",
        "problem_draft",
        &draft_id,
        "viewer",
    )
    .await?;
    let draft = state.service.store().get_problem_draft(&draft_id).await?;
    Ok(Json(unscoped_success(ProblemDraftView::from(draft))))
}

async fn cancel_problem_draft(
    State(state): State<AppState>,
    Path(draft_id): Path<String>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<ProblemDraftControlRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<ProblemDraftControlView>>), ApiError> {
    let key = idempotency_key(&headers)?;
    authorize_actor(
        state.service.store(),
        &actor,
        None,
        "cancel_problem_draft",
        "problem_draft",
        &draft_id,
        "researcher",
    )
    .await?;
    let (draft, replayed) = state
        .service
        .cancel_problem_draft(&actor.actor_id, &key, &draft_id, body.expected_revision)
        .await?;
    state
        .service
        .store()
        .record_security_audit(
            None,
            Some(&actor.actor_id),
            "cancel_problem_draft",
            "problem_draft",
            &draft_id,
            "allowed",
            None,
            Some(&key),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(unscoped_success(ProblemDraftControlView::new(
            draft, replayed,
        ))),
    ))
}

async fn retry_problem_draft(
    State(state): State<AppState>,
    Path(draft_id): Path<String>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<ProblemDraftControlRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<ProblemDraftControlView>>), ApiError> {
    let key = idempotency_key(&headers)?;
    authorize_actor(
        state.service.store(),
        &actor,
        None,
        "retry_problem_draft",
        "problem_draft",
        &draft_id,
        "researcher",
    )
    .await?;
    let (draft, replayed) = state
        .service
        .begin_problem_draft_retry(&actor.actor_id, &key, &draft_id, body.expected_revision)
        .await?;
    state
        .service
        .store()
        .record_security_audit(
            None,
            Some(&actor.actor_id),
            "retry_problem_draft",
            "problem_draft",
            &draft_id,
            "allowed",
            None,
            Some(&key),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(unscoped_success(ProblemDraftControlView::new(
            draft, replayed,
        ))),
    ))
}

async fn confirm_problem_draft(
    State(state): State<AppState>,
    Path(draft_id): Path<String>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<ConfirmProblemDraftRequest>,
) -> Result<(StatusCode, Json<ApiEnvelope<ProblemDraftConfirmationView>>), ApiError> {
    let key = idempotency_key(&headers)?;
    authorize_actor(
        state.service.store(),
        &actor,
        None,
        "confirm_problem_draft",
        "problem_draft",
        &draft_id,
        "researcher",
    )
    .await?;
    let result = state
        .service
        .confirm_problem_draft(ProblemDraftConfirmationRequest {
            draft_id: &draft_id,
            expected_revision: body.expected_revision,
            expected_document_hash: &body.expected_document_hash,
            idempotency_key: &key,
            requested_by: &actor.actor_id,
            start: body.start,
            edited_document: body.document.as_ref(),
            acknowledge_material_warnings: body.acknowledge_material_warnings,
        })
        .await?;
    let project_id = result.project.project_id.clone();
    let preview_markdown = result
        .draft
        .document
        .as_ref()
        .map(render_problem_document_markdown)
        .ok_or_else(|| ApiError::invalid("confirmed problem draft has no document"))?;
    state
        .service
        .store()
        .record_security_audit(
            Some(&project_id),
            Some(&actor.actor_id),
            "confirm_problem_draft",
            "problem_draft",
            &draft_id,
            "allowed",
            None,
            Some(&key),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(
            success(
                &state.service,
                &project_id,
                ProblemDraftConfirmationView {
                    confirmation: result,
                    preview_markdown,
                },
            )
            .await?,
        ),
    ))
}

async fn create_project(
    State(state): State<AppState>,
    Extension(actor): Extension<Actor>,
    Json(body): Json<CreateProjectRequest>,
) -> Result<impl IntoResponse, ApiError> {
    authorize_actor(
        state.service.store(),
        &actor,
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
    let project = if let Some(review_mode) = body.review_mode {
        state
            .service
            .create_project_with_review_mode(body.name, contract, body.budget, review_mode)
            .await?
    } else {
        state
            .service
            .create_project_with_route_approval(
                body.name,
                contract,
                body.budget,
                body.human_route_approval,
            )
            .await?
    };
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
    Extension(actor): Extension<Actor>,
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
    authorize_actor(
        state.service.store(),
        &actor,
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
    Extension(actor): Extension<Actor>,
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
    authorize_actor(
        state.service.store(),
        &actor,
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
        can_create_route: rank >= role_rank("researcher"),
        can_approve_route: rank >= role_rank("researcher"),
        can_force_goal_review: rank >= role_rank("researcher"),
        can_manage_settings: rank >= role_rank("operator"),
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
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<ProblemRevisionRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let key = idempotency_key(&headers)?;
    authorize_actor(
        state.service.store(),
        &actor,
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
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<HumanRouteProposalRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let key = idempotency_key(&headers)?;
    authorize_actor(
        state.service.store(),
        &actor,
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

async fn create_human_route(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<HumanRouteCreateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let key = idempotency_key(&headers)?;
    authorize_actor(
        state.service.store(),
        &actor,
        Some(&project_id),
        "create_human_route",
        "project",
        &project_id,
        "researcher",
    )
    .await?;
    let result = state
        .service
        .create_human_route(&project_id, &body, &actor.actor_id, &key)
        .await?;
    state
        .service
        .store()
        .record_security_audit(
            Some(&project_id),
            Some(&actor.actor_id),
            "create_human_route",
            "route",
            &result.data.route_id,
            "allowed",
            Some(&body.reason),
            Some(&key),
        )
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

async fn proof_obligations(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<ApiEnvelope<research_domain::ProofObligationGraph>>, ApiError> {
    let (data, project_revision, event_cursor) = state
        .service
        .store()
        .proof_obligation_graph_snapshot(&project_id)
        .await?;
    Ok(Json(ApiEnvelope {
        data: Some(data),
        meta: ResponseMeta {
            request_id: request_id(),
            project_revision: Some(project_revision),
            event_cursor: Some(event_cursor),
        },
        error: None,
    }))
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
    Extension(actor): Extension<Actor>,
    Json(body): Json<ReconciliationRequest>,
) -> Result<Json<ApiEnvelope<Value>>, ApiError> {
    let target_id = body.project_id.as_deref().unwrap_or("system");
    authorize_actor(
        state.service.store(),
        &actor,
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProofHintScope {
    #[serde(default)]
    goal_id: Option<String>,
    #[serde(default)]
    proof_node_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    Extension(actor): Extension<Actor>,
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
    authorize_actor(
        state.service.store(),
        &actor,
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
    Extension(actor): Extension<Actor>,
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
    authorize_actor(
        state.service.store(),
        &actor,
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
    Extension(actor): Extension<Actor>,
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
    authorize_actor(
        state.service.store(),
        &actor,
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
    Extension(actor): Extension<Actor>,
) -> Result<Json<ApiEnvelope<Vec<Value>>>, ApiError> {
    authorize_actor(
        state.service.store(),
        &actor,
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
    Extension(actor): Extension<Actor>,
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
    authorize_actor(
        state.service.store(),
        &actor,
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
#[serde(deny_unknown_fields)]
struct FactGovernanceRequest {
    reason: String,
}

async fn security_audit(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Extension(actor): Extension<Actor>,
) -> Result<Json<ApiEnvelope<Vec<research_domain::SecurityAuditEntry>>>, ApiError> {
    authorize_actor(
        state.service.store(),
        &actor,
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
    actor: Actor,
    headers: HeaderMap,
    body: FactGovernanceRequest,
    action: &'static str,
) -> Result<FactGovernanceResponse, ApiError> {
    let key = idempotency_key(&headers)?;
    let fact = state.service.store().get_fact(&fact_id).await?;
    authorize_actor(
        state.service.store(),
        &actor,
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
            Extension(actor): Extension<Actor>,
            headers: HeaderMap,
            Json(body): Json<FactGovernanceRequest>,
        ) -> Result<FactGovernanceResponse, ApiError> {
            govern_fact_command(state, fact_id, actor, headers, body, $action).await
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
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    project_command(state, path, actor, headers, body, "start_project").await
}
async fn pause_project(
    state: State<AppState>,
    path: Path<String>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    project_command(state, path, actor, headers, body, "pause_project").await
}
async fn resume_project(
    state: State<AppState>,
    path: Path<String>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    project_command(state, path, actor, headers, body, "resume_project").await
}
async fn stop_project(
    state: State<AppState>,
    path: Path<String>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    project_command(state, path, actor, headers, body, "stop_project").await
}
async fn replan(
    state: State<AppState>,
    path: Path<String>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    project_command(state, path, actor, headers, body, "trigger_replan").await
}

async fn goal_review(
    state: State<AppState>,
    path: Path<String>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    project_command(state, path, actor, headers, body, "goal_review").await
}

async fn review_policy(
    state: State<AppState>,
    path: Path<String>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    project_command(state, path, actor, headers, body, "review_policy").await
}

async fn research_settings(
    state: State<AppState>,
    path: Path<String>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    project_command(state, path, actor, headers, body, "research_settings").await
}

async fn create_human_task(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        &actor,
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
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        &actor,
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
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        &actor,
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
    actor: Actor,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
    command_type: &'static str,
    mode: CommandMode,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        &actor,
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
            Extension(actor): Extension<Actor>,
            headers: HeaderMap,
            body: Json<CommandRequest>,
        ) -> Result<CommandResponse, ApiError> {
            task_command(state, path, actor, headers, body, $command, $mode).await
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
    Extension(actor): Extension<Actor>,
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
        &actor,
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
    actor: Actor,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
    command_type: &'static str,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        &actor,
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
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    route_command(state, path, actor, headers, body, "pause_route").await
}
async fn resume_route(
    state: State<AppState>,
    path: Path<(String, String)>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    route_command(state, path, actor, headers, body, "resume_route").await
}
async fn stop_route(
    state: State<AppState>,
    path: Path<(String, String)>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    route_command(state, path, actor, headers, body, "stop_route").await
}
async fn approve_route(
    state: State<AppState>,
    path: Path<(String, String)>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    route_command(state, path, actor, headers, body, "approve_route").await
}
async fn prune_route(
    state: State<AppState>,
    path: Path<(String, String)>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    route_command(state, path, actor, headers, body, "prune_route").await
}
async fn revive_route(
    state: State<AppState>,
    path: Path<(String, String)>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    body: Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    route_command(state, path, actor, headers, body, "revive_route").await
}

async fn merge_routes(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        &actor,
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
    actor: Actor,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
    command_type: &'static str,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        &actor,
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
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        &actor,
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
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        &actor,
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
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        &actor,
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
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<CommandRequest>,
) -> Result<CommandResponse, ApiError> {
    issue_command(
        &state.service,
        &project_id,
        &headers,
        &actor,
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
    Extension(actor): Extension<Actor>,
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
        &actor,
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
    actor: &Actor,
    body: CommandRequest,
    command_type: &str,
    target_kind: &str,
    target_id: &str,
    mode: CommandMode,
) -> Result<CommandResponse, ApiError> {
    let key = idempotency_key(headers)?;
    authorize_actor(
        service.store(),
        actor,
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
    Extension(actor): Extension<Actor>,
    headers: HeaderMap,
    Json(body): Json<CandidateSubmission>,
) -> Result<impl IntoResponse, ApiError> {
    let key = idempotency_key(&headers)?;
    authorize_actor(
        state.service.store(),
        &actor,
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
    Extension(actor): Extension<Actor>,
) -> Result<impl IntoResponse, ApiError> {
    authorize_actor(
        state.service.store(),
        &actor,
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

fn unscoped_success<T: Serialize>(data: T) -> ApiEnvelope<T> {
    ApiEnvelope {
        data: Some(data),
        meta: ResponseMeta {
            request_id: request_id(),
            project_revision: None,
            event_cursor: None,
        },
        error: None,
    }
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

async fn authenticate_actor(store: &SqliteStore, headers: &HeaderMap) -> Result<Actor, ApiError> {
    if store.actor_count().await? == 0 {
        return Ok(Actor {
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
        .get(axum::http::header::AUTHORIZATION)
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
    store
        .authenticate_actor(actor_id, token)
        .await
        .map_err(|_| ApiError {
            status: StatusCode::UNAUTHORIZED,
            code: "invalid_actor_credentials",
            message: "actor credentials are invalid".into(),
            details: json!({}),
            retryable: false,
        })
}

async fn authorize_actor(
    store: &SqliteStore,
    actor: &Actor,
    project_id: Option<&str>,
    action: &str,
    target_kind: &str,
    target_id: &str,
    minimum_role: &str,
) -> Result<(), ApiError> {
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
    Ok(())
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
        | "review_policy"
        | "research_settings"
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
    for endpoint in ENDPOINT_DESCRIPTORS
        .iter()
        .filter(|endpoint| endpoint.documented)
    {
        let path = endpoint.path;
        let method = endpoint.method.as_str();
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
        if endpoint.response == EndpointResponse::Board {
            parameters.extend([
                json!({"name":"timeline_limit","in":"query","required":false,"schema":{"type":"integer","minimum":0,"maximum":500,"default":100}}),
                json!({"name":"include","in":"query","required":false,"schema":{"type":"string","default":"summary"}}),
                json!({"name":"schema_version","in":"query","required":false,"schema":{"type":"integer","const":1,"default":1}}),
                json!({"name":"If-None-Match","in":"header","required":false,"schema":{"type":"string"}}),
            ]);
        }
        if endpoint.idempotency_required {
            parameters.push(json!({
                "name":"Idempotency-Key","in":"header","required":true,
                "schema":{"type":"string","minLength":1},
                "description":"Unique key used to deduplicate retried writes."
            }));
        }
        let success_content = match endpoint.response {
            EndpointResponse::EventStream => {
                json!({"text/event-stream":{"schema":{"type":"string"}}})
            }
            EndpointResponse::Binary => {
                json!({"application/octet-stream":{"schema":{"type":"string","contentEncoding":"binary"}}})
            }
            EndpointResponse::Board => {
                json!({"application/json":{"schema":{"$ref":"#/components/schemas/BoardEnvelope"}}})
            }
            EndpointResponse::ObligationGraph => {
                json!({"application/json":{"schema":{"$ref":"#/components/schemas/ProofObligationGraphEnvelope"}}})
            }
            EndpointResponse::Json => {
                json!({"application/json":{"schema":{"$ref":"#/components/schemas/SuccessEnvelope"}}})
            }
        };
        let mut responses = serde_json::Map::new();
        responses.insert(
            endpoint.success_status.into(),
            json!({"description":"Success","content":success_content}),
        );
        if endpoint.response == EndpointResponse::Board {
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
            "summary":endpoint.summary,
            "parameters":parameters,
            "responses":responses,
        });
        match endpoint.auth {
            EndpointAuth::Public => operation["security"] = json!([]),
            EndpointAuth::WorkerToken => {
                operation["security"] = json!([{"WorkerToken":[]}]);
            }
            EndpointAuth::Actor => {}
        }
        if let Some(schema) = endpoint.request_schema {
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
            "ProofObligationGraphEnvelope":{"type":"object","additionalProperties":false,"required":["data","meta","error"],"properties":{
                "data":{"$ref":"#/components/schemas/ProofObligationGraph"},"meta":{"$ref":"#/components/schemas/ResponseMeta"},"error":{"type":"null"}
            }},
            "ErrorEnvelope":{"type":"object","required":["data","meta","error"],"properties":{
                "data":{"type":"null"},"meta":{"$ref":"#/components/schemas/ResponseMeta"},"error":{"$ref":"#/components/schemas/ApiErrorBody"}
            }},
            "Budget":{"type":"object","required":["max_rounds","max_parallel_workers","max_minutes_per_task","max_model_calls_per_task","max_total_model_calls"],"properties":{
                "max_rounds":{"type":"integer","minimum":1},"max_parallel_workers":{"type":"integer","minimum":1,"maximum":16},"max_minutes_per_task":{"type":"integer","minimum":1,"maximum":1440},"max_model_calls_per_task":{"type":"integer","minimum":1},"max_total_model_calls":{"type":"integer","minimum":1}
            }},
            "ProofObligation":{"type":"object","additionalProperties":false,"required":["obligation_id","project_id","goal_id","parent_obligation_id","source_kind","statement","completion_criteria","necessity","status","priority","source_verification_id","source_bottleneck_id","source_fingerprint","provenance","satisfied_by_fact_id","created_revision","updated_revision","created_at","updated_at"],"properties":{
                "obligation_id":{"type":"string"},"project_id":{"type":"string"},"goal_id":{"type":["string","null"]},"parent_obligation_id":{"type":["string","null"]},"source_kind":{"type":"string"},"statement":{"type":"string"},"completion_criteria":{"type":"string"},"necessity":{"type":"string","enum":["required","advisory"]},"status":{"type":"string","enum":["open","satisfied","blocked","obsolete"]},"priority":{"type":"number"},"source_verification_id":{"type":["string","null"]},"source_bottleneck_id":{"type":["string","null"]},"source_fingerprint":{"type":"string"},"provenance":{},"satisfied_by_fact_id":{"type":["string","null"]},"created_revision":{"type":"integer","format":"int64"},"updated_revision":{"type":"integer","format":"int64"},"created_at":{"type":"string","format":"date-time"},"updated_at":{"type":"string","format":"date-time"}
            }},
            "ObligationEdge":{"type":"object","additionalProperties":false,"required":["edge_id","project_id","source_obligation_id","target_obligation_id","kind","created_at"],"properties":{
                "edge_id":{"type":"string"},"project_id":{"type":"string"},"source_obligation_id":{"type":"string"},"target_obligation_id":{"type":"string"},"kind":{"type":"string","enum":["depends_on","refines"]},"created_at":{"type":"string","format":"date-time"}
            }},
            "CandidateObligationCoverage":{"type":"object","additionalProperties":false,"required":["coverage_id","project_id","candidate_id","obligation_id","verification_id","disposition","fact_id","rationale","created_at"],"properties":{
                "coverage_id":{"type":"string"},"project_id":{"type":"string"},"candidate_id":{"type":"string"},"obligation_id":{"type":"string"},"verification_id":{"type":"string"},"disposition":{"type":"string","enum":["supports","satisfies","insufficient","unknown"]},"fact_id":{"type":["string","null"]},"rationale":{"type":"string"},"created_at":{"type":"string","format":"date-time"}
            }},
            "ProofObligationGraph":{"type":"object","additionalProperties":false,"required":["obligations","edges","coverage"],"properties":{
                "obligations":{"type":"array","items":{"$ref":"#/components/schemas/ProofObligation"}},"edges":{"type":"array","items":{"$ref":"#/components/schemas/ObligationEdge"}},"coverage":{"type":"array","items":{"$ref":"#/components/schemas/CandidateObligationCoverage"}}
            }},
            "CreateProjectRequest":{"type":"object","additionalProperties":false,"required":["name","problem"],"properties":{
                "name":{"type":"string","minLength":1},"problem":{"type":"string","minLength":1},"target_statement":{"type":["string","null"]},"assumptions":{"type":"array","items":{"type":"string"}},"success_criteria":{"type":["string","null"]},"budget":{"$ref":"#/components/schemas/Budget"},"human_route_approval":{"type":"boolean","default":false,"description":"Legacy compatibility flag; review_mode takes precedence when supplied."},"review_mode":{"type":["string","null"],"enum":["automatic","balanced","strict",null]}
            }},
            "ProblemAssumption":{"type":"object","additionalProperties":false,"required":["statement","provenance"],"properties":{
                "statement":{"type":"string","minLength":1},"provenance":{"type":"string","enum":["prompt","material","inferred"]}
            }},
            "ProblemMaterial":{"type":"object","additionalProperties":false,"required":["relative_path","media_type","byte_size","included_bytes","sha256","status","warning"],"properties":{
                "relative_path":{"type":"string"},"media_type":{"type":"string"},"byte_size":{"type":"integer","minimum":0},"included_bytes":{"type":"integer","minimum":0},"sha256":{"type":"string"},"status":{"type":"string"},"warning":{"type":["string","null"]}
            }},
            "ProblemDocument":{"type":"object","additionalProperties":false,"required":["name","problem","target_statement","success_criteria","budget","budget_rationale"],"properties":{
                "name":{"type":"string","minLength":1},"problem":{"type":"string","minLength":1},"target_statement":{"type":"string","minLength":1},"assumptions":{"type":"array","items":{"$ref":"#/components/schemas/ProblemAssumption"}},"success_criteria":{"type":"string","minLength":1},"budget":{"$ref":"#/components/schemas/Budget"},"human_route_approval":{"type":"boolean","default":false},"budget_rationale":{"type":"string"},"generation_notes":{"type":"array","items":{"type":"string"}},"unresolved_questions":{"type":"array","items":{"type":"string"}},"material_references":{"type":"array","items":{"type":"string"}}
            }},
            "ProblemDraft":{"type":"object","additionalProperties":false,"required":["draft_id","requested_by","creation_idempotency_key","creation_request_hash","prompt","material_directory","materials","status","revision","document","document_hash","material_manifest_hash","model","input_tokens","output_tokens","elapsed_ms","error_kind","error_message","confirmation_idempotency_key","confirmation_request_hash","confirmed_project_id","start_command_id","created_at","updated_at","generation_completed_at","confirmed_at"],"properties":{
                "draft_id":{"type":"string"},"requested_by":{"type":"string"},"creation_idempotency_key":{"type":"string"},"creation_request_hash":{"type":"string"},"prompt":{"type":"string"},"material_directory":{"type":"string"},"materials":{"type":"array","items":{"$ref":"#/components/schemas/ProblemMaterial"}},"status":{"type":"string","enum":["generating","awaiting_confirmation","confirmed","failed"]},"revision":{"type":"integer","format":"int64"},"document":{"oneOf":[{"$ref":"#/components/schemas/ProblemDocument"},{"type":"null"}]},"document_hash":{"type":["string","null"]},"material_manifest_hash":{"type":"string"},"model":{"type":["string","null"]},"input_tokens":{"type":"integer","format":"int64"},"output_tokens":{"type":"integer","format":"int64"},"elapsed_ms":{"type":"integer","format":"int64"},"error_kind":{"type":["string","null"]},"error_message":{"type":["string","null"]},"confirmation_idempotency_key":{"type":["string","null"]},"confirmation_request_hash":{"type":["string","null"]},"confirmed_project_id":{"type":["string","null"]},"start_command_id":{"type":["string","null"]},"created_at":{"type":"string","format":"date-time"},"updated_at":{"type":"string","format":"date-time"},"generation_completed_at":{"type":["string","null"],"format":"date-time"},"confirmed_at":{"type":["string","null"],"format":"date-time"}
            }},
            "ProblemDraftConfirmation":{"type":"object","additionalProperties":false,"required":["draft","project","start_command","replayed"],"properties":{
                "draft":{"$ref":"#/components/schemas/ProblemDraft"},"project":{"type":"object"},"start_command":{"type":["object","null"]},"replayed":{"type":"boolean"}
            }},
            "ProblemDraftView":{"type":"object","additionalProperties":false,"required":["draft","preview_markdown"],"properties":{
                "draft":{"$ref":"#/components/schemas/ProblemDraft"},"preview_markdown":{"type":["string","null"]}
            }},
            "ProblemDraftConfirmationView":{"type":"object","additionalProperties":false,"required":["confirmation","preview_markdown"],"properties":{
                "confirmation":{"$ref":"#/components/schemas/ProblemDraftConfirmation"},"preview_markdown":{"type":"string"}
            }},
            "ProblemDraftControlView":{"type":"object","additionalProperties":false,"required":["draft","preview_markdown","replayed"],"properties":{
                "draft":{"$ref":"#/components/schemas/ProblemDraft"},"preview_markdown":{"type":["string","null"]},"replayed":{"type":"boolean"}
            }},
            "GenerateProblemDraftRequest":{"type":"object","additionalProperties":false,"required":["prompt"],"properties":{
                "prompt":{"type":"string","minLength":1},"context_dir":{"type":"string","default":"."}
            }},
            "ProblemDraftControlRequest":{"type":"object","additionalProperties":false,"required":["expected_revision"],"properties":{
                "expected_revision":{"type":"integer","format":"int64","minimum":1}
            }},
            "ConfirmProblemDraftRequest":{"type":"object","additionalProperties":false,"required":["expected_revision","expected_document_hash"],"properties":{
                "expected_revision":{"type":"integer","format":"int64","minimum":1},"expected_document_hash":{"type":"string","minLength":1},"document":{"oneOf":[{"$ref":"#/components/schemas/ProblemDocument"},{"type":"null"}]},"acknowledge_material_warnings":{"type":"boolean","default":false},"start":{"type":"boolean","default":true}
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
            "LeaseSteersRequest":{"type":"object","additionalProperties":false,"required":["node_id","node_epoch","lease_epoch"],"properties":{
                "node_id":{"type":"string"},"node_epoch":{"type":"integer","format":"int64"},"lease_epoch":{"type":"integer","format":"int64"}
            }},
            "CompleteLeaseRequest":{"type":"object","additionalProperties":false,"required":["node_id","node_epoch","lease_epoch","output"],"properties":{
                "node_id":{"type":"string"},"node_epoch":{"type":"integer","format":"int64"},"lease_epoch":{"type":"integer","format":"int64"},"output":{"type":"object"},"incorporated_steer_ids":{"type":"array","items":{"type":"string"},"default":[]}
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
            "SuggestionRequest":{"type":"object","additionalProperties":false,"required":["expected_revision","content"],"properties":{
                "expected_revision":{"type":"integer","format":"int64"},"content":{"type":"string","minLength":1},"target_route_id":{"type":["string","null"]},"reason":{"type":"string"}
            }},
            "ProblemRevisionRequest":{"type":"object","additionalProperties":false,"required":["expected_revision","target_statement","success_criteria","change_reason"],"properties":{
                "expected_revision":{"type":"integer","format":"int64"},"target_statement":{"type":"string","minLength":1},"assumptions":{"type":"array","items":{"type":"string"}},"success_criteria":{"type":"string","minLength":1},"change_reason":{"type":"string","minLength":1},"replan":{"type":"boolean"}
            }},
            "HumanRouteProposalRequest":{"type":"object","additionalProperties":false,"required":["expected_revision","title","method_summary","reason"],"properties":{
                "expected_revision":{"type":"integer","format":"int64"},"title":{"type":"string","minLength":1},"method_summary":{"type":"string","minLength":1},"target_goal_ids":{"type":"array","items":{"type":"string"}},"required_fact_ids":{"type":"array","items":{"type":"string"}},"known_risks":{"type":"array","items":{"type":"string"}},"reason":{"type":"string","minLength":1}
            }},
            "HumanRouteCreateRequest":{"type":"object","additionalProperties":false,"required":["expected_revision","title","method_summary","objective","completion_contract","reason"],"properties":{
                "expected_revision":{"type":"integer","format":"int64"},"title":{"type":"string","minLength":1},"method_summary":{"type":"string","minLength":1},"approach_kind":{"type":"string"},"route_role":{"type":"string"},"plain_language_summary":{"type":"string"},"steps":{"type":"array","items":{"type":"string"}},"target_goal_ids":{"type":"array","items":{"type":"string"}},"required_fact_ids":{"type":"array","items":{"type":"string"}},"known_risks":{"type":"array","items":{"type":"string"}},"worker_role":{"type":"string","default":"prover"},"objective":{"type":"string","minLength":1},"completion_contract":{"type":"string","minLength":1},"priority":{"type":"number","minimum":0,"maximum":1,"default":0.9},"reason":{"type":"string","minLength":1}
            }},
            "BoardCapabilities":{"type":"object","additionalProperties":false,"required":["can_edit_problem","can_propose_route","can_create_route","can_approve_route","can_force_goal_review","can_manage_settings","can_control_project","can_control_tasks","can_govern_facts"],"properties":{
                "can_edit_problem":{"type":"boolean"},"can_propose_route":{"type":"boolean"},"can_create_route":{"type":"boolean"},"can_approve_route":{"type":"boolean"},"can_force_goal_review":{"type":"boolean"},"can_manage_settings":{"type":"boolean"},"can_control_project":{"type":"boolean"},"can_control_tasks":{"type":"boolean"},"can_govern_facts":{"type":"boolean"}
            }},
            "BoardPlanningSuggestion":{"type":"object","additionalProperties":false,"required":["suggestion_id","content","target_route_id","status","decision","created_in_round","effective_round"],"properties":{"suggestion_id":{"type":"string"},"content":{"type":"string"},"target_route_id":{"type":["string","null"]},"status":{"type":"string"},"decision":{"type":["object","null"]},"created_in_round":{"type":"integer","format":"int64"},"effective_round":{"type":"integer","format":"int64"}}},
            "ResearchBoardView":{"type":"object","additionalProperties":true,"required":["schema_version","project_id","agent","mode","status","revision","event_cursor","problem","settings","summary","routes","goals","claims","failed_routes","planning_suggestions","human_questions","uncertainties","tasks","workers","verification_queue","artifacts","timeline","graph","capabilities"],"properties":{
                "schema_version":{"type":"integer","const":1},"project_id":{"type":"string"},"agent":{"type":"string","const":"mathcat"},"mode":{"type":"string"},"status":{"type":"string"},"revision":{"type":"integer","format":"int64"},"event_cursor":{"type":"integer","format":"int64"},"problem":{"type":"object"},"settings":{"type":"object","required":["budget","review_mode","human_route_approval","running_task_policy"],"properties":{"budget":{"$ref":"#/components/schemas/Budget"},"review_mode":{"type":"string","enum":["automatic","balanced","strict"]},"human_route_approval":{"type":"boolean"},"running_task_policy":{"type":"string"}}},"summary":{"type":"object"},"routes":{"type":"array","items":{"type":"object"}},"goals":{"type":"array","items":{"type":"object"}},"claims":{"type":"array","items":{"type":"object"}},"failed_routes":{"type":"array","items":{"type":"object"}},"planning_suggestions":{"type":"array","items":{"$ref":"#/components/schemas/BoardPlanningSuggestion"}},"human_questions":{"type":"array","items":{"type":"object"}},"uncertainties":{"type":"array","items":{"type":"object"}},"tasks":{"type":"array","items":{"type":"object"}},"workers":{"type":"array","items":{"type":"object"}},"verification_queue":{"type":"array","items":{"type":"object"}},"artifacts":{"type":"array","items":{"type":"object"}},"timeline":{"type":"array","items":{"type":"object"}},"graph":{"type":["object","null"]},"capabilities":{"$ref":"#/components/schemas/BoardCapabilities"}
            }},
            "CandidateSubmission":{"type":"object","additionalProperties":false,"required":["task_id","route_id","statement","proof_markdown","candidate_type","task_revision","route_cancellation_epoch"],"properties":{
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
    use std::{collections::BTreeSet, fs, sync::Arc};

    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use futures::{SinkExt, StreamExt};
    use research_core::ResearchConfig;
    use research_storage::{ProblemDraftBeginRequest, SqliteStore};
    use research_worker_runtime::MockBackend;
    use tokio_tungstenite::{connect_async, tungstenite::Message as ClientMessage};
    use tower::ServiceExt;

    use super::*;

    async fn app_with_backend(
        backend: MockBackend,
    ) -> (Router, ResearchService, tempfile::TempDir) {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::connect("sqlite::memory:", temp.path().join("artifacts"))
            .await
            .expect("store");
        let service = ResearchService::new(
            store,
            Arc::new(backend),
            ResearchConfig {
                runtime_root: temp.path().join("runtime"),
                output_root: temp.path().join("output"),
                material_root: temp.path().to_path_buf(),
                model: None,
                lean_project_root: None,
                planner_timeout_seconds: 5,
                worker_timeout_seconds: 5,
                verifier_timeout_seconds: 5,
                problem_generator_timeout_seconds: 5,
                ..ResearchConfig::default()
            },
        );
        (router(service.clone()), service, temp)
    }

    async fn app() -> (Router, ResearchService, tempfile::TempDir) {
        app_with_backend(MockBackend::default()).await
    }

    fn generated_problem_document() -> Value {
        json!({
            "name":"Finite graph extremal question",
            "problem":"Let G be a finite simple graph. Determine whether the stated bound holds.",
            "target_statement":"For every finite simple graph G satisfying A, invariant f(G) is at most n.",
            "assumptions":[{"statement":"G is finite and simple.","provenance":"prompt"}],
            "success_criteria":"The exact target receives an accepted verdict, every dependency is active, and no blocking uncertainty remains.",
            "budget":{
                "max_rounds":1,
                "max_parallel_workers":1,
                "max_minutes_per_task":1,
                "max_model_calls_per_task":1,
                "max_total_model_calls":1
            },
            "human_route_approval":false,
            "budget_rationale":"A bounded first pass is sufficient for this narrow fixture.",
            "generation_notes":[],
            "unresolved_questions":[],
            "material_references":["a.md"]
        })
    }

    #[tokio::test]
    async fn problem_draft_http_flow_is_authenticated_idempotent_and_confirmation_gated() {
        let (app, service, temp) =
            app_with_backend(MockBackend::from_responses([generated_problem_document()])).await;
        fs::create_dir(temp.path().join("notes")).expect("notes directory");
        fs::write(temp.path().join("notes/a.md"), "finite graph background")
            .expect("material fixture");
        fs::write(temp.path().join("notes/blob.pdf"), b"%PDF excluded fixture")
            .expect("unsupported material fixture");

        let bootstrap = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/actors/bootstrap")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "actor_id":"admin",
                            "display_name":"Admin",
                            "token":"admin-token-123456789"
                        })
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("bootstrap response");
        assert_eq!(bootstrap.status(), StatusCode::CREATED);

        for (actor_id, display_name, role, token) in [
            (
                "researcher",
                "Researcher",
                "researcher",
                "researcher-token-123456789",
            ),
            ("viewer", "Viewer", "viewer", "viewer-token-123456789"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/actors")
                        .header("content-type", "application/json")
                        .header("X-Actor-Id", "admin")
                        .header("Authorization", "Bearer admin-token-123456789")
                        .body(Body::from(
                            json!({
                                "actor_id":actor_id,
                                "display_name":display_name,
                                "role":role,
                                "token":token
                            })
                            .to_string(),
                        ))
                        .expect("request"),
                )
                .await
                .expect("create actor response");
            assert_eq!(response.status(), StatusCode::CREATED);
        }

        let request_body = json!({
            "prompt":"study the finite graph bound",
            "context_dir":"notes"
        });
        let viewer_forbidden = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/problem-drafts")
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "draft-http-viewer")
                    .header("X-Actor-Id", "viewer")
                    .header("Authorization", "Bearer viewer-token-123456789")
                    .body(Body::from(request_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("viewer response");
        assert_eq!(viewer_forbidden.status(), StatusCode::FORBIDDEN);

        let missing_key = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/problem-drafts")
                    .header("content-type", "application/json")
                    .header("X-Actor-Id", "researcher")
                    .header("Authorization", "Bearer researcher-token-123456789")
                    .body(Body::from(request_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("missing idempotency response");
        assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);

        let unknown_field = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/problem-drafts")
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "draft-http-unknown")
                    .header("X-Actor-Id", "researcher")
                    .header("Authorization", "Bearer researcher-token-123456789")
                    .body(Body::from(
                        json!({
                            "prompt":"study the finite graph bound",
                            "context_dir":"notes",
                            "unexpected":true
                        })
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("unknown field response");
        assert!(!unknown_field.status().is_success());

        let generated = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/problem-drafts")
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "draft-http-1")
                    .header("X-Actor-Id", "researcher")
                    .header("Authorization", "Bearer researcher-token-123456789")
                    .body(Body::from(request_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("generation response");
        assert_eq!(generated.status(), StatusCode::ACCEPTED);
        let generated_bytes = to_bytes(generated.into_body(), usize::MAX)
            .await
            .expect("generated body");
        let generated_text = String::from_utf8(generated_bytes.to_vec()).expect("UTF-8 response");
        assert!(
            !generated_text.contains(&temp.path().display().to_string()),
            "API responses must not expose the configured absolute material root"
        );
        let initial: Value = serde_json::from_str(&generated_text).expect("generation json");
        assert_eq!(
            initial
                .pointer("/data/draft/status")
                .and_then(Value::as_str),
            Some("generating")
        );
        assert_eq!(
            initial
                .pointer("/data/draft/material_directory")
                .and_then(Value::as_str),
            Some("notes")
        );
        assert_eq!(
            initial
                .pointer("/data/draft/materials/0/relative_path")
                .and_then(Value::as_str),
            Some("a.md")
        );
        assert!(
            initial
                .pointer("/data/preview_markdown")
                .is_some_and(Value::is_null)
        );
        assert!(
            initial
                .pointer("/data/draft/confirmed_project_id")
                .is_some_and(Value::is_null),
            "generation must not create a project before confirmation"
        );
        let draft_id = initial
            .pointer("/data/draft/draft_id")
            .and_then(Value::as_str)
            .expect("draft id")
            .to_owned();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let draft = service
                    .store()
                    .get_problem_draft(&draft_id)
                    .await
                    .expect("poll draft");
                if draft.status != research_domain::ProblemDraftStatus::Generating {
                    break draft;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("background generation completed");
        assert_eq!(
            completed.status,
            research_domain::ProblemDraftStatus::AwaitingConfirmation
        );
        let preview_markdown = completed
            .document
            .as_ref()
            .map(render_problem_document_markdown);
        let generated = json!({"data":{"draft":completed,"preview_markdown":preview_markdown}});
        let revision = generated
            .pointer("/data/draft/revision")
            .and_then(Value::as_i64)
            .expect("draft revision");
        let document_hash = generated
            .pointer("/data/draft/document_hash")
            .and_then(Value::as_str)
            .expect("document hash")
            .to_owned();

        let replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/problem-drafts")
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "draft-http-1")
                    .header("X-Actor-Id", "researcher")
                    .header("Authorization", "Bearer researcher-token-123456789")
                    .body(Body::from(request_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("generation replay");
        assert_eq!(replay.status(), StatusCode::ACCEPTED);
        let replay: Value = serde_json::from_slice(
            &to_bytes(replay.into_body(), usize::MAX)
                .await
                .expect("replay body"),
        )
        .expect("replay json");
        assert_eq!(
            replay
                .pointer("/data/draft/draft_id")
                .and_then(Value::as_str),
            Some(draft_id.as_str())
        );

        let read = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/problem-drafts/{draft_id}"))
                    .header("X-Actor-Id", "viewer")
                    .header("Authorization", "Bearer viewer-token-123456789")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("draft read");
        assert_eq!(read.status(), StatusCode::OK);
        let read: Value = serde_json::from_slice(
            &to_bytes(read.into_body(), usize::MAX)
                .await
                .expect("read body"),
        )
        .expect("read json");
        assert_eq!(
            read.pointer("/data/preview_markdown"),
            generated.pointer("/data/preview_markdown")
        );

        let mut edited_document = generated
            .pointer("/data/draft/document")
            .expect("generated document")
            .clone();
        edited_document["name"] = json!("Human-reviewed graph question");
        let confirmation_body = json!({
            "expected_revision":revision,
            "expected_document_hash":document_hash,
            "document":edited_document,
            "acknowledge_material_warnings":true,
            "start":false
        });
        let missing_confirmation_key = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/problem-drafts/{draft_id}/commands/confirm"
                    ))
                    .header("content-type", "application/json")
                    .header("X-Actor-Id", "researcher")
                    .header("Authorization", "Bearer researcher-token-123456789")
                    .body(Body::from(confirmation_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("missing confirmation idempotency response");
        assert_eq!(missing_confirmation_key.status(), StatusCode::BAD_REQUEST);

        let viewer_confirmation = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/problem-drafts/{draft_id}/commands/confirm"
                    ))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "confirm-http-viewer")
                    .header("X-Actor-Id", "viewer")
                    .header("Authorization", "Bearer viewer-token-123456789")
                    .body(Body::from(confirmation_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("viewer confirmation response");
        assert_eq!(viewer_confirmation.status(), StatusCode::FORBIDDEN);

        let mut unacknowledged_body = confirmation_body.clone();
        unacknowledged_body["acknowledge_material_warnings"] = json!(false);
        let unacknowledged = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/problem-drafts/{draft_id}/commands/confirm"
                    ))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "confirm-http-unacknowledged")
                    .header("X-Actor-Id", "researcher")
                    .header("Authorization", "Bearer researcher-token-123456789")
                    .body(Body::from(unacknowledged_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("unacknowledged warning response");
        assert_eq!(unacknowledged.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let confirmed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/problem-drafts/{draft_id}/commands/confirm"
                    ))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "confirm-http-1")
                    .header("X-Actor-Id", "researcher")
                    .header("Authorization", "Bearer researcher-token-123456789")
                    .body(Body::from(confirmation_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("confirmation response");
        assert_eq!(confirmed.status(), StatusCode::ACCEPTED);
        let confirmed: Value = serde_json::from_slice(
            &to_bytes(confirmed.into_body(), usize::MAX)
                .await
                .expect("confirmation body"),
        )
        .expect("confirmation json");
        assert_eq!(
            confirmed
                .pointer("/data/confirmation/draft/status")
                .and_then(Value::as_str),
            Some("confirmed")
        );
        assert_eq!(
            confirmed
                .pointer("/data/confirmation/project/status")
                .and_then(Value::as_str),
            Some("created")
        );
        assert_eq!(
            confirmed
                .pointer("/data/confirmation/project/name")
                .and_then(Value::as_str),
            Some("Human-reviewed graph question")
        );
        assert!(
            confirmed
                .pointer("/data/confirmation/start_command")
                .is_some_and(Value::is_null)
        );
        let project_id = confirmed
            .pointer("/data/confirmation/project/project_id")
            .and_then(Value::as_str)
            .expect("project id")
            .to_owned();
        service
            .store()
            .get_project(&project_id)
            .await
            .expect("confirmation created project");

        let confirmation_replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/problem-drafts/{draft_id}/commands/confirm"
                    ))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "confirm-http-1")
                    .header("X-Actor-Id", "researcher")
                    .header("Authorization", "Bearer researcher-token-123456789")
                    .body(Body::from(confirmation_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("confirmation replay");
        assert_eq!(confirmation_replay.status(), StatusCode::ACCEPTED);
        let confirmation_replay: Value = serde_json::from_slice(
            &to_bytes(confirmation_replay.into_body(), usize::MAX)
                .await
                .expect("confirmation replay body"),
        )
        .expect("confirmation replay json");
        assert_eq!(
            confirmation_replay
                .pointer("/data/confirmation/project/project_id")
                .and_then(Value::as_str),
            Some(project_id.as_str())
        );
        assert_eq!(
            confirmation_replay
                .pointer("/data/confirmation/replayed")
                .and_then(Value::as_bool),
            Some(true)
        );

        let mut conflicting_confirmation = confirmation_body;
        conflicting_confirmation["start"] = json!(true);
        let conflict = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/problem-drafts/{draft_id}/commands/confirm"
                    ))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "confirm-http-1")
                    .header("X-Actor-Id", "researcher")
                    .header("Authorization", "Bearer researcher-token-123456789")
                    .body(Body::from(conflicting_confirmation.to_string()))
                    .expect("request"),
            )
            .await
            .expect("confirmation conflict");
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn problem_draft_cancel_and_retry_commands_enforce_auth_and_idempotency() {
        let backend = MockBackend::default();
        let backend_control = backend.clone();
        let (app, service, temp) = app_with_backend(backend).await;
        service
            .store()
            .bootstrap_admin("admin", "Admin", "admin-token-123456789")
            .await
            .expect("bootstrap admin");
        for (actor_id, role, token) in [
            ("researcher", "researcher", "researcher-token-123456789"),
            ("other", "researcher", "other-researcher-token-123456789"),
            ("viewer", "viewer", "viewer-token-123456789"),
        ] {
            service
                .store()
                .create_actor(actor_id, actor_id, role, token)
                .await
                .expect("create actor");
        }
        fs::create_dir(temp.path().join("empty-materials")).expect("empty material scope");
        let materials = Vec::new();
        let (generating, _) = service
            .store()
            .begin_problem_draft(ProblemDraftBeginRequest {
                requested_by: "researcher",
                idempotency_key: "control-draft-create",
                prompt: "formulate a finite graph problem",
                material_directory: "empty-materials",
                materials: &materials,
                model: None,
            })
            .await
            .expect("generating draft");
        let cancel_body = json!({"expected_revision":generating.revision}).to_string();

        let viewer_denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/problem-drafts/{}/commands/cancel",
                        generating.draft_id
                    ))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "viewer-cancel")
                    .header("X-Actor-Id", "viewer")
                    .header("Authorization", "Bearer viewer-token-123456789")
                    .body(Body::from(cancel_body.clone()))
                    .expect("request"),
            )
            .await
            .expect("viewer cancel response");
        assert_eq!(viewer_denied.status(), StatusCode::FORBIDDEN);

        let other_denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/problem-drafts/{}/commands/cancel",
                        generating.draft_id
                    ))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "other-cancel")
                    .header("X-Actor-Id", "other")
                    .header("Authorization", "Bearer other-researcher-token-123456789")
                    .body(Body::from(cancel_body.clone()))
                    .expect("request"),
            )
            .await
            .expect("other requester cancel response");
        assert_eq!(other_denied.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let cancel = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/problem-drafts/{}/commands/cancel",
                        generating.draft_id
                    ))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "control-cancel")
                    .header("X-Actor-Id", "researcher")
                    .header("Authorization", "Bearer researcher-token-123456789")
                    .body(Body::from(cancel_body.clone()))
                    .expect("request"),
            )
            .await
            .expect("cancel response");
        assert_eq!(cancel.status(), StatusCode::ACCEPTED);
        let cancelled: Value = serde_json::from_slice(
            &to_bytes(cancel.into_body(), usize::MAX)
                .await
                .expect("cancel body"),
        )
        .expect("cancel json");
        assert_eq!(
            cancelled
                .pointer("/data/draft/status")
                .and_then(Value::as_str),
            Some("failed")
        );
        assert_eq!(
            cancelled
                .pointer("/data/draft/error_kind")
                .and_then(Value::as_str),
            Some("cancelled")
        );
        let cancelled_revision = cancelled
            .pointer("/data/draft/revision")
            .and_then(Value::as_i64)
            .expect("cancelled revision");

        let cancel_replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/problem-drafts/{}/commands/cancel",
                        generating.draft_id
                    ))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "control-cancel")
                    .header("X-Actor-Id", "researcher")
                    .header("Authorization", "Bearer researcher-token-123456789")
                    .body(Body::from(cancel_body))
                    .expect("request"),
            )
            .await
            .expect("cancel replay response");
        let cancel_replay: Value = serde_json::from_slice(
            &to_bytes(cancel_replay.into_body(), usize::MAX)
                .await
                .expect("cancel replay body"),
        )
        .expect("cancel replay json");
        assert_eq!(
            cancel_replay
                .pointer("/data/replayed")
                .and_then(Value::as_bool),
            Some(true)
        );

        let mut retry_output = generated_problem_document();
        retry_output["material_references"] = json!([]);
        backend_control.push(retry_output).await;
        let retry_body = json!({"expected_revision":cancelled_revision}).to_string();
        let retry = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/problem-drafts/{}/commands/retry",
                        generating.draft_id
                    ))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "control-retry")
                    .header("X-Actor-Id", "researcher")
                    .header("Authorization", "Bearer researcher-token-123456789")
                    .body(Body::from(retry_body.clone()))
                    .expect("request"),
            )
            .await
            .expect("retry response");
        assert_eq!(retry.status(), StatusCode::ACCEPTED);
        let retried: Value = serde_json::from_slice(
            &to_bytes(retry.into_body(), usize::MAX)
                .await
                .expect("retry body"),
        )
        .expect("retry json");
        assert_eq!(
            retried
                .pointer("/data/draft/status")
                .and_then(Value::as_str),
            Some("generating")
        );
        assert_eq!(
            retried
                .pointer("/data/draft/draft_id")
                .and_then(Value::as_str),
            Some(generating.draft_id.as_str())
        );
        let retried_terminal = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let draft = service
                    .store()
                    .get_problem_draft(&generating.draft_id)
                    .await
                    .expect("poll retried draft");
                if draft.status != research_domain::ProblemDraftStatus::Generating {
                    break draft;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("background retry completed");
        assert_eq!(
            retried_terminal.status,
            research_domain::ProblemDraftStatus::AwaitingConfirmation
        );

        let retry_replay = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/problem-drafts/{}/commands/retry",
                        generating.draft_id
                    ))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "control-retry")
                    .header("X-Actor-Id", "researcher")
                    .header("Authorization", "Bearer researcher-token-123456789")
                    .body(Body::from(retry_body))
                    .expect("request"),
            )
            .await
            .expect("retry replay response");
        let retry_replay: Value = serde_json::from_slice(
            &to_bytes(retry_replay.into_body(), usize::MAX)
                .await
                .expect("retry replay body"),
        )
        .expect("retry replay json");
        assert_eq!(
            retry_replay
                .pointer("/data/replayed")
                .and_then(Value::as_bool),
            Some(true)
        );
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
    async fn create_project_review_mode_precedes_the_legacy_approval_boolean() {
        let (app, _service, _temp) = app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/projects")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "name":"automatic review project",
                            "problem":"Prove P",
                            "budget":{
                                "max_rounds":2,
                                "max_parallel_workers":2,
                                "max_minutes_per_task":20,
                                "max_model_calls_per_task":2,
                                "max_total_model_calls":20
                            },
                            "human_route_approval":true,
                            "review_mode":"automatic"
                        })
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::CREATED);
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json");
        assert_eq!(
            body.pointer("/data/review_mode").and_then(Value::as_str),
            Some("automatic")
        );
        assert_eq!(
            body.pointer("/data/human_route_approval")
                .and_then(Value::as_bool),
            Some(false)
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
    async fn human_collaboration_endpoints_accept_the_ui_contracts_and_are_documented() {
        let (app, service, _temp) = app().await;
        let project = service
            .create_project_with_review_mode(
                "human route api".into(),
                ProblemContract {
                    original_problem: "Prove P".into(),
                    target_statement: "P".into(),
                    assumptions: vec![],
                    success_criteria: "accepted".into(),
                    version: 1,
                },
                Budget::default(),
                ReviewMode::Strict,
            )
            .await
            .expect("strict project");
        let route_body = json!({
            "expected_revision":project.revision,
            "title":"Human localization route",
            "method_summary":"Study P after localization at every prime",
            "approach_kind":"reduction",
            "route_role":"primary",
            "plain_language_summary":"Reduce the global claim to local checks.",
            "steps":["localize the statement","prove the local-to-global bridge"],
            "target_goal_ids":[],
            "required_fact_ids":[],
            "known_risks":["the bridge may need finite generation"],
            "worker_role":"human_collaborator",
            "objective":"Prove the exact local-to-global bridge.",
            "completion_contract":"A checkable proof, counterexample, or named blocker.",
            "priority":0.95,
            "reason":"Execute the route authored on the research whiteboard."
        });
        let route_uri = format!("/api/v1/projects/{}/routes", project.project_id);
        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&route_uri)
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "api-human-route-1")
                    .body(Body::from(route_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("human route response");
        assert_eq!(created.status(), StatusCode::ACCEPTED);
        let created: Value = serde_json::from_slice(
            &to_bytes(created.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json");
        assert_eq!(
            created
                .pointer("/data/execution_started")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert!(
            created
                .pointer("/data/route_id")
                .and_then(Value::as_str)
                .is_some()
        );
        assert!(
            created
                .pointer("/data/task_id")
                .and_then(Value::as_str)
                .is_some()
        );

        let settings_project = service
            .create_project(
                "settings api".into(),
                ProblemContract {
                    original_problem: "Prove Q".into(),
                    target_statement: "Q".into(),
                    assumptions: vec![],
                    success_criteria: "accepted".into(),
                    version: 1,
                },
                Budget::default(),
            )
            .await
            .expect("settings project");
        let settings = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/projects/{}/commands/settings",
                        settings_project.project_id
                    ))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "api-settings-1")
                    .body(Body::from(
                        json!({
                            "expected_revision":settings_project.revision,
                            "reason":"whiteboard settings",
                            "payload":{
                                "limits":{
                                    "max_rounds":8,
                                    "max_parallel_workers":4,
                                    "max_minutes_per_task":30,
                                    "max_model_calls_per_task":3,
                                    "max_total_model_calls":80
                                },
                                "review_mode":"automatic"
                            }
                        })
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("settings response");
        assert_eq!(settings.status(), StatusCode::ACCEPTED);
        let settings: Value = serde_json::from_slice(
            &to_bytes(settings.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json");
        assert_eq!(
            settings.pointer("/data/type").and_then(Value::as_str),
            Some("research_settings")
        );
        assert_eq!(
            settings
                .pointer("/data/payload/review_mode")
                .and_then(Value::as_str),
            Some("automatic")
        );

        let focus_project = service
            .create_project(
                "goal review api".into(),
                ProblemContract {
                    original_problem: "Prove R".into(),
                    target_statement: "R".into(),
                    assumptions: vec![],
                    success_criteria: "accepted".into(),
                    version: 1,
                },
                Budget::default(),
            )
            .await
            .expect("goal review project");
        let focus = "从交换代数的局部化角度重新讨论主目标";
        let goal_review = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/v1/projects/{}/commands/goal-review",
                        focus_project.project_id
                    ))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "api-goal-review-1")
                    .body(Body::from(
                        json!({
                            "expected_revision":focus_project.revision,
                            "reason":"force a new goal discussion",
                            "payload":{"focus":focus}
                        })
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("goal review response");
        assert_eq!(goal_review.status(), StatusCode::ACCEPTED);
        let goal_review: Value = serde_json::from_slice(
            &to_bytes(goal_review.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json");
        assert_eq!(
            goal_review
                .pointer("/data/payload/focus")
                .and_then(Value::as_str),
            Some(focus)
        );

        let document = openapi_document();
        for path in [
            "/paths/~1api~1v1~1projects~1{project_id}~1routes/post",
            "/paths/~1api~1v1~1projects~1{project_id}~1commands~1goal-review/post",
            "/paths/~1api~1v1~1projects~1{project_id}~1commands~1review-policy/post",
            "/paths/~1api~1v1~1projects~1{project_id}~1commands~1settings/post",
        ] {
            assert!(
                document.pointer(path).is_some(),
                "missing OpenAPI path {path}"
            );
        }
        assert_eq!(
            document
                .pointer("/paths/~1api~1v1~1projects~1{project_id}~1routes/post/requestBody/content/application~1json/schema/$ref")
                .and_then(Value::as_str),
            Some("#/components/schemas/HumanRouteCreateRequest")
        );
        assert!(
            document
                .pointer("/components/schemas/ResearchBoardView/properties/planning_suggestions/items/$ref")
                .is_some()
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

    #[test]
    fn endpoint_descriptors_are_unique_and_drive_openapi_metadata() {
        assert_eq!(ENDPOINT_DESCRIPTORS.len(), 150);
        let mut registered = BTreeSet::new();
        for endpoint in ENDPOINT_DESCRIPTORS.iter() {
            assert!(
                registered.insert((endpoint.path, endpoint.method)),
                "duplicate endpoint descriptor: {} {}",
                endpoint.method.as_str(),
                endpoint.path
            );
        }

        let document = openapi_document();
        let paths = document["paths"].as_object().expect("OpenAPI paths");
        let documented: BTreeSet<_> = ENDPOINT_DESCRIPTORS
            .iter()
            .filter(|endpoint| endpoint.documented)
            .map(|endpoint| {
                (
                    endpoint.path.to_owned(),
                    endpoint.method.as_str().to_owned(),
                )
            })
            .collect();
        let mut generated = BTreeSet::new();
        for (path, path_item) in paths {
            for method in ["get", "post"] {
                if path_item.get(method).is_some() {
                    generated.insert((path.clone(), method.to_owned()));
                }
            }
        }
        assert_eq!(documented.len(), 148);
        assert_eq!(generated, documented);

        for endpoint in ENDPOINT_DESCRIPTORS
            .iter()
            .filter(|endpoint| endpoint.documented)
        {
            let operation = &paths[endpoint.path][endpoint.method.as_str()];
            assert_eq!(
                operation["summary"].as_str(),
                Some(endpoint.summary),
                "summary drift for {} {}",
                endpoint.method.as_str(),
                endpoint.path
            );
            assert!(
                operation["responses"]
                    .get(endpoint.success_status)
                    .is_some(),
                "success status drift for {} {}",
                endpoint.method.as_str(),
                endpoint.path
            );
            let documents_idempotency = operation["parameters"]
                .as_array()
                .expect("operation parameters")
                .iter()
                .any(|parameter| parameter["name"] == "Idempotency-Key");
            assert_eq!(
                documents_idempotency,
                endpoint.idempotency_required,
                "idempotency drift for {} {}",
                endpoint.method.as_str(),
                endpoint.path
            );
            match endpoint.auth {
                EndpointAuth::Public => assert_eq!(operation["security"], json!([])),
                EndpointAuth::WorkerToken => {
                    assert_eq!(operation["security"], json!([{"WorkerToken":[]}]));
                }
                EndpointAuth::Actor => assert!(operation.get("security").is_none()),
            }
            match endpoint.request_schema {
                Some(schema) => assert_eq!(
                    operation["requestBody"]["content"]["application/json"]["schema"]["$ref"]
                        .as_str(),
                    Some(format!("#/components/schemas/{schema}").as_str())
                ),
                None => assert!(operation.get("requestBody").is_none()),
            }
        }

        let publication = endpoint_policy(&Method::POST, "/api/v1/projects/project-1/publications")
            .expect("publication endpoint");
        assert!(publication.idempotency_required);
        let generation = endpoint_policy(&Method::POST, "/api/v1/problem-drafts")
            .expect("problem generation endpoint");
        assert!(generation.idempotency_required);
        assert_eq!(generation.success_status, "202");
        let confirmation = endpoint_policy(
            &Method::POST,
            "/api/v1/problem-drafts/draft-1/commands/confirm",
        )
        .expect("problem confirmation endpoint");
        assert!(confirmation.idempotency_required);
        assert_eq!(confirmation.success_status, "202");
        for command in ["cancel", "retry"] {
            let control = endpoint_policy(
                &Method::POST,
                &format!("/api/v1/problem-drafts/draft-1/commands/{command}"),
            )
            .expect("problem draft control endpoint");
            assert!(control.idempotency_required);
            assert_eq!(control.success_status, "202");
        }
        let reconciliation =
            endpoint_policy(&Method::POST, "/api/v1/system/reconciliation/commands/run")
                .expect("reconciliation endpoint");
        assert!(!reconciliation.idempotency_required);
    }

    #[test]
    fn request_schema_defaults_and_unknown_field_policy_match_serde() {
        fn required_fields(document: &Value, schema: &str) -> BTreeSet<String> {
            document["components"]["schemas"][schema]["required"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        }

        let document = openapi_document();
        assert_eq!(
            required_fields(&document, "GenerateProblemDraftRequest"),
            ["prompt"].into_iter().map(str::to_owned).collect()
        );
        assert_eq!(
            required_fields(&document, "ConfirmProblemDraftRequest"),
            ["expected_document_hash", "expected_revision"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
        assert_eq!(
            required_fields(&document, "ProblemDraftControlRequest"),
            ["expected_revision"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
        assert_eq!(
            required_fields(&document, "ProblemRevisionRequest"),
            [
                "change_reason",
                "expected_revision",
                "success_criteria",
                "target_statement",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );
        assert_eq!(
            required_fields(&document, "HumanRouteProposalRequest"),
            ["expected_revision", "method_summary", "reason", "title"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
        assert_eq!(
            required_fields(&document, "CandidateSubmission"),
            [
                "candidate_type",
                "proof_markdown",
                "route_cancellation_epoch",
                "route_id",
                "statement",
                "task_id",
                "task_revision",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );

        let problem: ProblemRevisionRequest = serde_json::from_value(json!({
            "expected_revision":1,
            "target_statement":"p",
            "success_criteria":"prove p",
            "change_reason":"clarify"
        }))
        .expect("defaulted problem revision fields");
        assert!(problem.assumptions.is_empty());
        assert!(!problem.replan);

        let generated: GenerateProblemDraftRequest = serde_json::from_value(json!({
            "prompt":"study the local notes"
        }))
        .expect("defaulted problem generation fields");
        assert_eq!(generated.context_dir, ".");
        let confirmation: ConfirmProblemDraftRequest = serde_json::from_value(json!({
            "expected_revision":2,
            "expected_document_hash":"sha256"
        }))
        .expect("defaulted confirmation fields");
        assert!(confirmation.start);
        assert!(!confirmation.acknowledge_material_warnings);
        assert!(confirmation.document.is_none());
        let control: ProblemDraftControlRequest = serde_json::from_value(json!({
            "expected_revision":2
        }))
        .expect("problem draft control request");
        assert_eq!(control.expected_revision, 2);
        assert!(
            serde_json::from_value::<ProblemDraftControlRequest>(json!({
                "expected_revision":2,
                "unexpected":true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ConfirmProblemDraftRequest>(json!({
                "expected_revision":2,
                "expected_document_hash":"sha256",
                "unexpected":true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ProblemRevisionRequest>(json!({
                "expected_revision":1,
                "target_statement":"p",
                "success_criteria":"prove p",
                "change_reason":"clarify",
                "undocumented":true
            }))
            .is_err()
        );

        let route: HumanRouteProposalRequest = serde_json::from_value(json!({
            "expected_revision":1,
            "title":"direct",
            "method_summary":"prove directly",
            "reason":"independent route"
        }))
        .expect("defaulted route proposal fields");
        assert!(route.target_goal_ids.is_empty());
        assert!(route.required_fact_ids.is_empty());
        assert!(route.known_risks.is_empty());

        let candidate: CandidateSubmission = serde_json::from_value(json!({
            "task_id":"task-1",
            "route_id":"route-1",
            "statement":"p",
            "proof_markdown":"proof",
            "candidate_type":"theorem",
            "task_revision":1,
            "route_cancellation_epoch":0
        }))
        .expect("defaulted candidate fields");
        assert!(candidate.target_goal_ids.is_empty());
        assert!(candidate.assumptions.is_empty());
        assert!(candidate.dependency_fact_ids.is_empty());
        assert!(candidate.definitions_introduced.is_empty());
        assert!(candidate.external_source_ids.is_empty());
        assert!(
            serde_json::from_value::<CreatePublicationRequest>(
                json!({"allow_partial":false,"undocumented":true})
            )
            .is_err()
        );
    }

    #[test]
    fn endpoint_auth_matches_exact_route_templates_and_fails_closed() {
        assert_eq!(
            endpoint_auth(&Method::POST, "/api/v1/actors/bootstrap"),
            EndpointAuth::Public
        );
        assert_eq!(
            endpoint_auth(&Method::POST, "/api/v1/worker-nodes/node-1/heartbeat"),
            EndpointAuth::WorkerToken
        );
        assert_eq!(
            endpoint_auth(
                &Method::POST,
                "/api/v1/projects/project-1/distributed/leases/next"
            ),
            EndpointAuth::WorkerToken
        );
        assert_eq!(
            endpoint_auth(&Method::GET, "/api/v1/task-leases/lease-1"),
            EndpointAuth::Actor
        );
        for command in ["cancel", "retry"] {
            assert_eq!(
                endpoint_auth(
                    &Method::POST,
                    &format!("/api/v1/problem-drafts/draft-1/commands/{command}")
                ),
                EndpointAuth::Actor
            );
        }
        assert_eq!(
            endpoint_auth(&Method::POST, "/api/v1/problem-drafts"),
            EndpointAuth::Actor
        );
        assert_eq!(
            endpoint_auth(&Method::GET, "/api/v1/problem-drafts/draft-1"),
            EndpointAuth::Actor
        );
        assert_eq!(
            endpoint_auth(
                &Method::POST,
                "/api/v1/problem-drafts/draft-1/commands/confirm"
            ),
            EndpointAuth::Actor
        );
        assert_eq!(
            endpoint_auth(&Method::POST, "/api/v1/worker-nodes/node-1/heartbeat/extra"),
            EndpointAuth::Actor,
            "an unknown lookalike path must not inherit public worker access"
        );
        assert_eq!(
            endpoint_auth(&Method::GET, "/api/v1/actors/bootstrap"),
            EndpointAuth::Actor,
            "access policy is method-specific"
        );
        assert!(!route_path_matches(
            "/api/v1/task-leases/{lease_id}",
            "/api/v1/task-leases/"
        ));
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
        assert!(
            body.pointer("/paths/~1api~1v1~1problem-drafts/post")
                .is_some()
        );
        assert!(
            body.pointer("/paths/~1api~1v1~1problem-drafts~1{draft_id}/get")
                .is_some()
        );
        assert!(
            body.pointer("/paths/~1api~1v1~1problem-drafts~1{draft_id}~1commands~1confirm/post")
                .is_some()
        );
        assert!(
            body.pointer("/paths/~1api~1v1~1problem-drafts~1{draft_id}~1commands~1cancel/post")
                .is_some()
        );
        assert!(
            body.pointer("/paths/~1api~1v1~1problem-drafts~1{draft_id}~1commands~1retry/post")
                .is_some()
        );
        assert!(body.pointer("/paths/~1api~1v1~1projects/post").is_some());
        assert!(
            body.pointer("/paths/~1api~1v1~1projects~1{project_id}~1obligations/get")
                .is_some()
        );
        assert_eq!(
            body.pointer("/paths/~1api~1v1~1projects~1{project_id}~1obligations/get/responses/200/content/application~1json/schema/$ref")
                .and_then(Value::as_str),
            Some("#/components/schemas/ProofObligationGraphEnvelope")
        );
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
        let suggestion_required = body
            .pointer("/components/schemas/SuggestionRequest/required")
            .and_then(Value::as_array)
            .expect("SuggestionRequest required fields");
        assert!(
            suggestion_required
                .iter()
                .any(|value| value == "expected_revision")
        );
        assert!(suggestion_required.iter().any(|value| value == "content"));
        assert!(
            !suggestion_required
                .iter()
                .any(|value| value == "target_route_id"),
            "optional target_route_id must not be advertised as required"
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
    async fn handler_authorization_reuses_the_actor_authenticated_by_middleware() {
        async fn strip_actor_credentials(
            mut request: axum::extract::Request,
            next: Next,
        ) -> Response {
            request.headers_mut().remove("X-Actor-Id");
            request.headers_mut().remove(header::AUTHORIZATION);
            next.run(request).await
        }

        async fn cached_admin_probe(
            State(state): State<AppState>,
            Extension(actor): Extension<Actor>,
        ) -> Result<Json<Actor>, ApiError> {
            authorize_actor(
                state.service.store(),
                &actor,
                None,
                "cached_admin_probe",
                "system",
                "probe",
                "admin",
            )
            .await?;
            Ok(Json(actor))
        }

        let (_app, service, _temp) = app().await;
        service
            .store()
            .bootstrap_admin("admin", "Admin", "admin-token-123456789")
            .await
            .expect("admin");
        let state = AppState { service };
        let probe = Router::new()
            .route("/private-auth-probe", get(cached_admin_probe))
            .layer(middleware::from_fn(strip_actor_credentials))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                authentication_gate,
            ))
            .with_state(state);

        let response = probe
            .oneshot(
                Request::builder()
                    .uri("/private-auth-probe")
                    .header("X-Actor-Id", "admin")
                    .header("Authorization", "Bearer admin-token-123456789")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body: Actor = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("actor json");
        assert_eq!(body.actor_id, "admin");
        assert_eq!(body.role, "admin");
    }

    #[tokio::test]
    async fn public_worker_heartbeat_still_uses_only_the_worker_token() {
        let (app, service, _temp) = app().await;
        service
            .store()
            .bootstrap_admin("admin", "Admin", "admin-token-123456789")
            .await
            .expect("admin");
        service
            .store()
            .register_worker_node(
                "worker-node",
                "Worker node",
                json!({"local":false}),
                "worker-token-123456789",
            )
            .await
            .expect("worker node");

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/worker-nodes/worker-node/heartbeat")
                    .header("content-type", "application/json")
                    .header("X-Worker-Token", "worker-token-123456789")
                    .body(Body::from(json!({"node_epoch":1}).to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
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
