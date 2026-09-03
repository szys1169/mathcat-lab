use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

pub type Attributes = BTreeMap<String, Value>;

macro_rules! string_enum {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => string_enum!(@snake $variant)),+ }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = String;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value { $(string_enum!(@snake $variant) => Ok(Self::$variant)),+, _ => Err(format!("invalid {}: {value}", stringify!($name))) }
            }
        }
    };
    (@snake Created) => { "created" };
    (@snake Running) => { "running" };
    (@snake Paused) => { "paused" };
    (@snake Success) => { "success" };
    (@snake PartialSuccess) => { "partial_success" };
    (@snake Refuted) => { "refuted" };
    (@snake NeedsHumanReview) => { "needs_human_review" };
    (@snake StoppedByHuman) => { "stopped_by_human" };
    (@snake Error) => { "error" };
    (@snake Proposed) => { "proposed" };
    (@snake Active) => { "active" };
    (@snake Completed) => { "completed" };
    (@snake Failed) => { "failed" };
    (@snake HumanStopped) => { "human_stopped" };
    (@snake Superseded) => { "superseded" };
    (@snake Open) => { "open" };
    (@snake Assigned) => { "assigned" };
    (@snake Blocked) => { "blocked" };
    (@snake Rejected) => { "rejected" };
    (@snake Cancelled) => { "cancelled" };
    (@snake Idle) => { "idle" };
    (@snake Stopped) => { "stopped" };
    (@snake Offline) => { "offline" };
    (@snake Solved) => { "solved" };
    (@snake Submitted) => { "submitted" };
    (@snake Accepted) => { "accepted" };
    (@snake Prechecking) => { "prechecking" };
    (@snake Verifying) => { "verifying" };
    (@snake Unknown) => { "unknown" };
    (@snake PromotedToFact) => { "promoted_to_fact" };
    (@snake Queued) => { "queued" };
    (@snake Validated) => { "validated" };
    (@snake WaitingSafePoint) => { "waiting_safe_point" };
    (@snake Applied) => { "applied" };
    (@snake Investigating) => { "investigating" };
    (@snake Resolved) => { "resolved" };
    (@snake AcceptedRisk) => { "accepted_risk" };
    (@snake Obsolete) => { "obsolete" };
    (@snake Planning) => { "planning" };
    (@snake Publishing) => { "publishing" };
    (@snake Interrupted) => { "interrupted" };
    (@snake NextRound) => { "next_round" };
    (@snake SafePoint) => { "safe_point" };
    (@snake Immediate) => { "immediate" };
    (@snake Theorem) => { "theorem" };
    (@snake Lemma) => { "lemma" };
    (@snake Proposition) => { "proposition" };
    (@snake Counterexample) => { "counterexample" };
    (@snake Observation) => { "observation" };
    (@snake ActiveFact) => { "active" };
    (@snake Suspended) => { "suspended" };
    (@snake Challenged) => { "challenged" };
    (@snake Revoked) => { "revoked" };
    (@snake Intake) => { "intake" };
    (@snake Snapshotting) => { "snapshotting" };
    (@snake Precheck) => { "precheck" };
    (@snake Review) => { "review" };
    (@snake Formalization) => { "formalization" };
    (@snake ProofSearch) => { "proof_search" };
    (@snake Adjudication) => { "adjudication" };
    (@snake Packaging) => { "packaging" };
    (@snake Replay) => { "replay" };
    (@snake CommitReady) => { "commit_ready" };
    (@snake Committed) => { "committed" };
    (@snake Exploratory) => { "exploratory" };
    (@snake StandardReview) => { "standard_review" };
    (@snake ToolAssisted) => { "tool_assisted" };
    (@snake FormalRequired) => { "formal_required" };
    (@snake CriticalCertification) => { "critical_certification" };
    (@snake Reviewed) => { "reviewed" };
    (@snake ComputationallyCertified) => { "computationally_certified" };
    (@snake FormallyVerified) => { "formally_verified" };
    (@snake FullyCertified) => { "fully_certified" };
    (@snake Equivalent) => { "equivalent" };
    (@snake FormalStronger) => { "formal_stronger" };
    (@snake FormalWeaker) => { "formal_weaker" };
    (@snake Incomparable) => { "incomparable" };
    (@snake Ambiguous) => { "ambiguous" };
    (@snake Misaligned) => { "misaligned" };
    (@snake Passed) => { "passed" };
    (@snake Inconclusive) => { "inconclusive" };
    (@snake Skipped) => { "skipped" };
    (@snake Expanded) => { "expanded" };
    (@snake Closed) => { "closed" };
    (@snake Pruned) => { "pruned" };
    (@snake Incubating) => { "incubating" };
    (@snake Merged) => { "merged" };
    (@snake Probation) => { "probation" };
    (@snake Revived) => { "revived" };
    (@snake Offered) => { "offered" };
    (@snake Leased) => { "leased" };
    (@snake Checkpointed) => { "checkpointed" };
    (@snake ResultSubmitted) => { "result_submitted" };
    (@snake Ingesting) => { "ingesting" };
    (@snake DeadLettered) => { "dead_lettered" };
    (@snake SpawnRequested) => { "spawn_requested" };
    (@snake Starting) => { "starting" };
    (@snake Handshaking) => { "handshaking" };
    (@snake Ready) => { "ready" };
    (@snake LeaseAccepted) => { "lease_accepted" };
    (@snake Checkpointing) => { "checkpointing" };
    (@snake Unhealthy) => { "unhealthy" };
    (@snake Terminating) => { "terminating" };
    (@snake Draining) => { "draining" };
    (@snake StartupFailed) => { "startup_failed" };
    (@snake Backoff) => { "backoff" };
    (@snake Quarantined) => { "quarantined" };
    (@snake Exited) => { "exited" };
}

string_enum!(ProjectStatus {
    Created,
    Running,
    Paused,
    Success,
    PartialSuccess,
    Refuted,
    NeedsHumanReview,
    StoppedByHuman,
    Error,
});
string_enum!(RoundStatus {
    Created,
    Planning,
    Running,
    Verifying,
    Publishing,
    Completed,
    Interrupted,
    Failed
});
string_enum!(RouteStatus {
    Proposed,
    Incubating,
    Active,
    Blocked,
    Probation,
    Paused,
    Completed,
    Refuted,
    Failed,
    Merged,
    Pruned,
    Revived,
    HumanStopped,
    Superseded
});
string_enum!(TaskStatus {
    Open,
    Queued,
    Assigned,
    Offered,
    Leased,
    Running,
    Checkpointed,
    ResultSubmitted,
    Ingesting,
    Paused,
    Blocked,
    Completed,
    Rejected,
    Cancelled,
    DeadLettered,
    HumanStopped
});
string_enum!(WorkerStatus {
    Idle,
    SpawnRequested,
    Starting,
    Handshaking,
    Ready,
    LeaseAccepted,
    Running,
    Checkpointing,
    ResultSubmitted,
    Draining,
    StartupFailed,
    Backoff,
    Unhealthy,
    Terminating,
    Quarantined,
    Exited,
    Paused,
    Stopped,
    Failed,
    Offline
});
string_enum!(GoalStatus {
    Open,
    Assigned,
    Running,
    Blocked,
    Solved,
    Refuted,
    Superseded
});
string_enum!(CandidateStatus {
    Submitted,
    Prechecking,
    Verifying,
    Accepted,
    Rejected,
    Unknown,
    PromotedToFact,
    Superseded
});
string_enum!(VerificationVerdict {
    Accepted,
    Rejected,
    Unknown
});
string_enum!(CommandStatus {
    Queued,
    Validated,
    WaitingSafePoint,
    Applied,
    Rejected,
    Superseded,
    Failed
});
string_enum!(CommandMode {
    NextRound,
    SafePoint,
    Immediate
});
string_enum!(UncertaintyStatus {
    Open,
    Investigating,
    Resolved,
    AcceptedRisk,
    Obsolete
});
string_enum!(FactStatus {
    ActiveFact,
    Challenged,
    Suspended,
    Revoked
});
string_enum!(CandidateType {
    Theorem,
    Lemma,
    Proposition,
    Counterexample,
    Observation
});

string_enum!(VerificationStage {
    Intake,
    Snapshotting,
    Precheck,
    Review,
    Formalization,
    ProofSearch,
    Adjudication,
    Packaging,
    Replay,
    CommitReady,
    Committed,
    Rejected,
    Unknown,
    Failed,
    Cancelled,
});
string_enum!(VerificationProfile {
    Exploratory,
    StandardReview,
    ToolAssisted,
    FormalRequired,
    CriticalCertification,
});
string_enum!(AcceptanceClass {
    Reviewed,
    ComputationallyCertified,
    FormallyVerified,
    FullyCertified,
});
string_enum!(AlignmentRelation {
    Equivalent,
    FormalStronger,
    FormalWeaker,
    Incomparable,
    Ambiguous,
    Misaligned,
});
string_enum!(CheckStatus {
    Queued,
    Running,
    Passed,
    Failed,
    Unknown,
    Error,
    Skipped,
    Cancelled,
});
string_enum!(ProofNodeStatus {
    Open,
    Running,
    Expanded,
    Closed,
    Failed,
    Pruned,
    Cancelled,
});

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Budget {
    pub max_rounds: u32,
    pub max_parallel_workers: u32,
    pub max_minutes_per_task: u32,
    pub max_model_calls_per_task: u32,
    pub max_total_model_calls: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_rounds: 12,
            max_parallel_workers: 3,
            max_minutes_per_task: 45,
            max_model_calls_per_task: 4,
            max_total_model_calls: 120,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProblemContract {
    pub original_problem: String,
    pub target_statement: String,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default = "default_success_criteria")]
    pub success_criteria: String,
    #[serde(default = "default_contract_version")]
    pub version: i64,
}

fn default_success_criteria() -> String {
    "目标陈述得到 accepted 裁决，依赖闭包均为 active，且无阻塞不确定性".into()
}

const fn default_contract_version() -> i64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Project {
    pub project_id: String,
    pub name: String,
    pub contract: ProblemContract,
    pub status: ProjectStatus,
    pub revision: i64,
    pub current_round: i64,
    pub budget: Budget,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ResearchRound {
    pub round_id: String,
    pub project_id: String,
    pub number: i64,
    pub status: RoundStatus,
    pub based_on_revision: i64,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Route {
    pub route_id: String,
    pub project_id: String,
    pub title: String,
    pub method_summary: String,
    pub target_goal_ids: Vec<String>,
    pub required_fact_ids: Vec<String>,
    pub status: RouteStatus,
    pub score: f64,
    pub priority: f64,
    pub cancellation_epoch: i64,
    pub created_in_round: i64,
    pub family_id: Option<String>,
    pub semantic_fingerprint: Option<String>,
    pub consecutive_no_progress_plans: i64,
    pub failed_attempt_count: i64,
    pub created_at_revision: i64,
    pub last_material_progress_revision: Option<i64>,
    pub merged_into: Option<String>,
    pub merge_reason: Option<String>,
    pub exit_criteria: Vec<String>,
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Task {
    pub task_id: String,
    pub project_id: String,
    pub route_id: String,
    pub worker_id: Option<String>,
    pub worker_role: String,
    pub goal_ids: Vec<String>,
    pub objective: String,
    pub completion_contract: String,
    pub status: TaskStatus,
    pub priority: f64,
    pub revision: i64,
    pub route_cancellation_epoch: i64,
    pub round: i64,
    pub result_summary: Option<String>,
    pub plan_revision_id: Option<String>,
    pub task_signature: Option<String>,
    pub context_packet_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Worker {
    pub worker_id: String,
    pub project_id: String,
    pub role: String,
    pub backend: String,
    pub status: WorkerStatus,
    pub current_task_id: Option<String>,
    pub current_route_id: Option<String>,
    pub session_id: Option<String>,
    pub last_heartbeat: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Goal {
    pub goal_id: String,
    pub project_id: String,
    pub statement: String,
    pub parent_goal_ids: Vec<String>,
    pub status: GoalStatus,
    pub priority: f64,
    pub blocked_by: Vec<String>,
    pub solved_by_fact_id: Option<String>,
    pub created_in_round: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Hypothesis {
    pub hypothesis_id: String,
    pub project_id: String,
    pub kind: String,
    pub statement: String,
    pub status: String,
    pub route_id: Option<String>,
    pub promoted_to_fact_id: Option<String>,
    pub created_in_round: i64,
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Fact {
    pub fact_id: String,
    pub project_id: String,
    pub statement: String,
    pub assumptions: Vec<String>,
    pub proof_markdown: String,
    pub dependency_fact_ids: Vec<String>,
    #[schema(value_type = Object)]
    pub definitions_introduced: Attributes,
    pub external_source_ids: Vec<String>,
    pub verification_ids: Vec<String>,
    pub evidence_level: String,
    pub created_by: String,
    pub status: FactStatus,
    pub content_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CandidateSubmission {
    pub task_id: String,
    pub route_id: String,
    #[serde(default)]
    pub target_goal_ids: Vec<String>,
    pub statement: String,
    #[serde(default)]
    pub assumptions: Vec<String>,
    pub proof_markdown: String,
    #[serde(default)]
    pub dependency_fact_ids: Vec<String>,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub definitions_introduced: Attributes,
    #[serde(default)]
    pub external_source_ids: Vec<String>,
    pub candidate_type: CandidateType,
    pub task_revision: i64,
    pub route_cancellation_epoch: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Candidate {
    pub candidate_id: String,
    pub project_id: String,
    pub submission: CandidateSubmission,
    pub status: CandidateStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VerificationGap {
    pub location: String,
    #[serde(rename = "type")]
    pub gap_type: String,
    pub issue: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VerificationReport {
    pub verdict: VerificationVerdict,
    pub summary: String,
    #[serde(default)]
    pub critical_errors: Vec<String>,
    #[serde(default)]
    pub gaps: Vec<VerificationGap>,
    #[serde(default)]
    pub uncertainties: Vec<String>,
    #[serde(default)]
    pub repair_actions: Vec<String>,
    #[serde(default)]
    pub checked_fact_ids: Vec<String>,
    #[serde(default)]
    pub checked_source_ids: Vec<String>,
    pub evidence_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Verification {
    pub verification_id: String,
    pub candidate_id: String,
    pub project_id: String,
    pub status: CandidateStatus,
    pub report: Option<VerificationReport>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// The immutable policy decision applied to one verification case.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[allow(clippy::struct_excessive_bools)]
pub struct VerificationPolicy {
    pub policy_id: String,
    pub project_id: String,
    pub name: String,
    pub profile: VerificationProfile,
    pub required_acceptance: AcceptanceClass,
    #[serde(default)]
    pub required_checks: Vec<String>,
    pub independent_reviewer_count: u32,
    pub require_citation_review: bool,
    pub require_adversarial_review: bool,
    pub require_alignment_review: bool,
    pub require_fresh_replay: bool,
    pub max_attempts: u32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VerificationCase {
    pub case_id: String,
    pub verification_id: String,
    pub candidate_id: String,
    pub project_id: String,
    pub policy_id: String,
    pub snapshot_id: Option<String>,
    pub profile: VerificationProfile,
    pub required_acceptance: AcceptanceClass,
    pub achieved_acceptance: Option<AcceptanceClass>,
    pub stage: VerificationStage,
    pub risk_score: f64,
    #[serde(default)]
    pub risk_reasons: Vec<String>,
    pub cancellation_epoch: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VerificationSnapshot {
    pub snapshot_id: String,
    pub case_id: String,
    pub project_id: String,
    pub candidate_hash: String,
    pub project_revision: i64,
    pub contract_version: i64,
    #[serde(default)]
    pub dependency_hashes: BTreeMap<String, String>,
    #[serde(default)]
    pub source_hashes: BTreeMap<String, String>,
    pub policy_hash: String,
    pub toolchain_hash: Option<String>,
    pub content_hash: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VerificationAttempt {
    pub attempt_id: String,
    pub case_id: String,
    pub sequence: i64,
    pub kind: String,
    pub status: CheckStatus,
    pub backend: Option<String>,
    pub cancellation_epoch: i64,
    pub input_hash: String,
    pub output_hash: Option<String>,
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VerificationCheck {
    pub check_id: String,
    pub case_id: String,
    pub attempt_id: Option<String>,
    pub kind: String,
    pub status: CheckStatus,
    pub mandatory: bool,
    pub summary: String,
    pub details: Value,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VerificationFinding {
    pub finding_id: String,
    pub case_id: String,
    pub check_id: Option<String>,
    pub reviewer_kind: String,
    pub severity: String,
    pub category: String,
    pub location: Option<String>,
    pub claim: String,
    pub rationale: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VerificationEvidence {
    pub evidence_id: String,
    pub case_id: String,
    pub check_id: Option<String>,
    pub dimension: String,
    pub kind: String,
    pub uri: Option<String>,
    pub sha256: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SemanticVariable {
    pub name: String,
    pub type_description: String,
    pub lean_type: Option<String>,
    pub binder_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SemanticContract {
    pub contract_id: String,
    pub case_id: String,
    pub version: i64,
    pub natural_language_statement: String,
    #[serde(default)]
    pub variables: Vec<SemanticVariable>,
    #[serde(default)]
    pub assumptions: Vec<String>,
    pub conclusion: String,
    #[serde(default)]
    pub definitions: BTreeMap<String, String>,
    #[serde(default)]
    pub boundary_conditions: Vec<String>,
    #[serde(default)]
    pub ambiguity_notes: Vec<String>,
    pub content_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Formalization {
    pub formalization_id: String,
    pub case_id: String,
    pub semantic_contract_id: String,
    pub theorem_name: String,
    pub lean_statement: String,
    pub lean_source: String,
    pub mapping: Value,
    pub source_hash: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SemanticContractDraft {
    #[serde(default)]
    pub variables: Vec<SemanticVariable>,
    #[serde(default)]
    pub assumptions: Vec<String>,
    pub conclusion: String,
    #[serde(default)]
    pub definitions: Vec<SemanticDefinition>,
    #[serde(default)]
    pub boundary_conditions: Vec<String>,
    #[serde(default)]
    pub ambiguity_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SemanticDefinition {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FormalizationMapping {
    pub natural_component: String,
    pub lean_component: String,
    pub relation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FormalizerOutput {
    pub semantic_contract: SemanticContractDraft,
    pub theorem_name: String,
    pub lean_statement: String,
    pub lean_source: String,
    pub mapping: Vec<FormalizationMapping>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AlignmentReview {
    pub alignment_id: String,
    pub case_id: String,
    pub formalization_id: String,
    pub relation: AlignmentRelation,
    pub reviewer_kind: String,
    pub rationale: String,
    #[serde(default)]
    pub missing_assumptions: Vec<String>,
    #[serde(default)]
    pub extra_assumptions: Vec<String>,
    pub confidence: f64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AlignmentReviewerOutput {
    pub relation: AlignmentRelation,
    pub rationale: String,
    #[serde(default)]
    pub missing_assumptions: Vec<String>,
    #[serde(default)]
    pub extra_assumptions: Vec<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BackendRun {
    pub backend_run_id: String,
    pub case_id: String,
    pub attempt_id: Option<String>,
    pub backend: String,
    pub backend_version: String,
    pub status: CheckStatus,
    pub command: Vec<String>,
    pub working_directory: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub diagnostics: Value,
    #[serde(default)]
    pub axioms: Vec<String>,
    pub elapsed_ms: i64,
    pub input_hash: String,
    pub output_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VerificationPackage {
    pub package_id: String,
    pub case_id: String,
    pub manifest: Value,
    pub manifest_hash: String,
    pub storage_path: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct VerificationReplay {
    pub replay_id: String,
    pub case_id: String,
    pub package_id: String,
    pub status: CheckStatus,
    pub fresh_process: bool,
    pub manifest_hash: String,
    pub observed_hash: String,
    pub backend_run_id: Option<String>,
    pub summary: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProofSearchBudget {
    pub max_nodes: u32,
    pub max_depth: u32,
    pub max_seconds: u64,
    pub max_model_calls: u32,
    pub beam_width: u32,
}

impl Default for ProofSearchBudget {
    fn default() -> Self {
        Self {
            max_nodes: 256,
            max_depth: 24,
            max_seconds: 15 * 60,
            max_model_calls: 32,
            beam_width: 8,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProofSearch {
    pub search_id: String,
    pub case_id: String,
    pub formalization_id: String,
    pub status: CheckStatus,
    pub strategy: String,
    pub budget: ProofSearchBudget,
    pub nodes_created: i64,
    pub nodes_expanded: i64,
    pub model_calls: i64,
    pub cancellation_epoch: i64,
    pub root_node_id: Option<String>,
    pub solution_node_id: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProofNode {
    pub node_id: String,
    pub search_id: String,
    pub parent_node_id: Option<String>,
    pub depth: i64,
    pub state_id: Option<i64>,
    pub goal: String,
    pub local_context: Value,
    pub tactic: Option<String>,
    pub score: f64,
    pub status: ProofNodeStatus,
    pub diagnostic: Option<String>,
    pub cancellation_epoch: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProofEdge {
    pub edge_id: String,
    pub search_id: String,
    pub source_node_id: String,
    pub target_node_id: String,
    pub tactic: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProofHint {
    pub hint_id: String,
    pub search_id: String,
    pub node_id: Option<String>,
    pub content: String,
    pub requested_by: String,
    pub effective_after_expansion: i64,
    pub consumed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TacticCandidate {
    pub tactic: String,
    pub rationale_summary: String,
    pub expected_goal_reduction: f64,
    pub premise_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TacticProposalOutput {
    pub candidates: Vec<TacticCandidate>,
    pub retrieval_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FactAssurance {
    pub fact_id: String,
    pub case_id: String,
    pub acceptance_class: AcceptanceClass,
    pub snapshot_hash: String,
    pub package_id: Option<String>,
    pub replay_id: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub invalidated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Uncertainty {
    pub uncertainty_id: String,
    pub project_id: String,
    pub description: String,
    #[serde(rename = "type")]
    pub uncertainty_type: String,
    pub severity: String,
    pub affects_goal_ids: Vec<String>,
    pub affects_route_ids: Vec<String>,
    pub introduced_by: String,
    pub resolution_methods: Vec<String>,
    pub status: UncertaintyStatus,
    pub resolved_by: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SourceRecord {
    pub source_id: String,
    pub project_id: String,
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
    pub url: Option<String>,
    pub normalized_url: Option<String>,
    pub identifier_kind: Option<String>,
    pub identifier_value: Option<String>,
    pub citation_key: Option<String>,
    pub theorem_reference: Option<String>,
    pub statement_excerpt: Option<String>,
    #[serde(default)]
    pub assumptions: Vec<String>,
    pub applicability: String,
    pub status: String,
    pub origin_task_id: Option<String>,
    pub origin_route_id: Option<String>,
    pub fulltext_artifact_id: Option<String>,
    pub provenance: Value,
    pub disposition_reason: Option<String>,
    pub retrieved_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SourceDraft {
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
    pub url: Option<String>,
    pub citation_key: Option<String>,
    pub theorem_reference: Option<String>,
    pub statement_excerpt: Option<String>,
    #[serde(default)]
    pub assumptions: Vec<String>,
    pub applicability: String,
    pub status: String,
    #[serde(default)]
    pub retrieval_query: Option<String>,
    #[serde(default)]
    pub document_version: Option<String>,
    #[serde(default)]
    pub fulltext_path: Option<String>,
    #[serde(default)]
    pub fulltext_sha256: Option<String>,
    #[serde(default)]
    pub fulltext_artifact_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FailurePattern {
    pub pattern_id: String,
    pub project_id: String,
    pub title: String,
    pub summary: String,
    pub source_failure_ids: Vec<String>,
    pub confidence: f64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HumanCommand {
    pub command_id: String,
    pub project_id: String,
    #[serde(rename = "type")]
    pub command_type: String,
    pub target_kind: String,
    pub target_id: String,
    pub mode: CommandMode,
    pub payload: Value,
    pub expected_project_revision: i64,
    pub idempotency_key: String,
    pub reason: String,
    pub requested_by: String,
    pub status: CommandStatus,
    pub before_revision: Option<i64>,
    pub after_revision: Option<i64>,
    pub affected_entities: Vec<EntityRef>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub applied_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EntityRef {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DomainEvent {
    pub event_id: String,
    pub cursor: i64,
    pub project_id: String,
    pub project_revision: i64,
    #[serde(rename = "type")]
    pub event_type: String,
    pub entity: EntityRef,
    pub data: Value,
    pub caused_by: Option<EntityRef>,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Artifact {
    pub artifact_id: String,
    pub project_id: String,
    pub kind: String,
    pub media_type: String,
    pub filename: String,
    pub size: i64,
    pub sha256: String,
    pub created_in_round: i64,
    pub related_entity_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing)]
    pub storage_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PublicationRun {
    pub publication_id: String,
    pub project_id: String,
    pub idempotency_key: String,
    pub allow_partial: bool,
    pub source_revision: i64,
    pub source_packet_hash: String,
    pub status: String,
    pub attempt_count: i64,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GraphNode {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub status: String,
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GraphEdge {
    pub id: String,
    pub source: String,
    pub target: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GraphProjection {
    pub graph_type: String,
    pub revision: i64,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProjectSnapshot {
    pub project: Project,
    pub current_round: Option<ResearchRound>,
    pub workers: Vec<Worker>,
    pub tasks: Vec<Task>,
    pub routes: Vec<Route>,
    pub goals: Vec<Goal>,
    pub hypotheses: Vec<Hypothesis>,
    pub facts: Vec<Fact>,
    pub uncertainties: Vec<Uncertainty>,
    pub sources: Vec<SourceRecord>,
    pub project_revision: i64,
    pub event_cursor: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[allow(clippy::struct_excessive_bools)]
pub struct BoardCapabilities {
    pub can_edit_problem: bool,
    pub can_propose_route: bool,
    pub can_approve_route: bool,
    pub can_control_project: bool,
    pub can_control_tasks: bool,
    pub can_govern_facts: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BoardSummary {
    pub current_round: i64,
    pub active_routes: usize,
    pub active_tasks: usize,
    pub open_goals: usize,
    pub accepted_facts: usize,
    pub pending_candidates: usize,
    pub blocking_uncertainties: usize,
    pub pending_human_questions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BoardRoute {
    pub route_id: String,
    pub title: String,
    pub method_summary: String,
    pub status: String,
    pub human_review: String,
    pub progress: f64,
    pub target_goal_ids: Vec<String>,
    pub active_task_ids: Vec<String>,
    pub blocking_uncertainty_ids: Vec<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BoardClaim {
    pub claim_id: String,
    pub kind: String,
    pub statement: String,
    pub status: String,
    pub origin_route_id: Option<String>,
    pub verification_id: Option<String>,
    pub fact_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ResearchBoardView {
    pub schema_version: i64,
    pub project_id: String,
    pub agent: String,
    pub mode: String,
    pub status: String,
    pub revision: i64,
    pub event_cursor: i64,
    pub problem: ProblemContract,
    pub summary: BoardSummary,
    pub routes: Vec<BoardRoute>,
    pub goals: Vec<Goal>,
    pub claims: Vec<BoardClaim>,
    pub failed_routes: Vec<BoardRoute>,
    pub human_questions: Vec<Value>,
    pub uncertainties: Vec<Uncertainty>,
    pub tasks: Vec<Task>,
    pub workers: Vec<Worker>,
    pub verification_queue: Vec<Verification>,
    pub artifacts: Vec<Artifact>,
    pub timeline: Vec<DomainEvent>,
    pub graph: Option<GraphProjection>,
    pub capabilities: BoardCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProblemRevisionRequest {
    pub expected_revision: i64,
    pub target_statement: String,
    #[serde(default)]
    pub assumptions: Vec<String>,
    pub success_criteria: String,
    pub change_reason: String,
    #[serde(default)]
    pub replan: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProblemRevisionResult {
    pub command_id: String,
    pub contract_version: i64,
    pub status: String,
    pub affected_entities: Vec<EntityRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HumanRouteProposalRequest {
    pub expected_revision: i64,
    pub title: String,
    pub method_summary: String,
    #[serde(default)]
    pub target_goal_ids: Vec<String>,
    #[serde(default)]
    pub required_fact_ids: Vec<String>,
    #[serde(default)]
    pub known_risks: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HumanRouteProposal {
    pub proposal_id: String,
    pub project_id: String,
    pub title: String,
    pub method_summary: String,
    pub target_goal_ids: Vec<String>,
    pub required_fact_ids: Vec<String>,
    pub known_risks: Vec<String>,
    pub reason: String,
    pub proposed_by: String,
    pub command_id: String,
    pub status: String,
    pub route_id: Option<String>,
    pub decision_reason: Option<String>,
    pub created_revision: i64,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HumanRouteProposalResult {
    pub proposal_id: String,
    pub command_id: String,
    pub status: String,
    pub route_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RouteProposal {
    pub title: String,
    pub method_summary: String,
    #[serde(default)]
    pub target_goal_ids: Vec<String>,
    #[serde(default)]
    pub required_fact_ids: Vec<String>,
    #[serde(default)]
    pub expected_subgoals: Vec<String>,
    pub expected_goal_progress: f64,
    pub uncertainty_reduction: f64,
    pub human_suggestion_alignment: f64,
    pub evidence_support: f64,
    pub route_diversity: f64,
    pub verifiability: f64,
    pub novelty: f64,
    pub failure_similarity_penalty: f64,
    pub cost_penalty: f64,
    #[serde(default)]
    pub risks: Vec<String>,
}

impl RouteProposal {
    #[must_use]
    pub fn score(&self) -> f64 {
        0.25 * self.expected_goal_progress
            + 0.20 * self.uncertainty_reduction
            + 0.15 * self.human_suggestion_alignment
            + 0.15 * self.evidence_support
            + 0.10 * self.route_diversity
            + 0.10 * self.verifiability
            + 0.05 * self.novelty
            - self.failure_similarity_penalty
            - self.cost_penalty
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AssignmentDraft {
    pub route_index: usize,
    pub worker_role: String,
    #[serde(default)]
    pub strategic_role: String,
    #[serde(default)]
    pub addresses_interface_debt: bool,
    #[serde(default)]
    pub goal_ids: Vec<String>,
    pub objective: String,
    pub completion_contract: String,
    pub priority: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PlannerOutput {
    pub rationale_summary: String,
    pub routes: Vec<RouteProposal>,
    pub assignments: Vec<AssignmentDraft>,
    #[serde(default)]
    pub targeted_uncertainty_ids: Vec<String>,
    #[serde(default)]
    pub suggestion_decisions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RouteGeneratorOutput {
    pub rationale_summary: String,
    pub routes: Vec<RouteProposal>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[allow(clippy::struct_excessive_bools)]
pub struct RouteReflection {
    pub route_index: usize,
    pub changes_problem: bool,
    pub uses_unverified_claims: bool,
    pub conflicts_with_facts: bool,
    pub repeats_failure_pattern: bool,
    pub has_verifiable_milestone: bool,
    pub risk_score: f64,
    #[serde(default)]
    pub goal_closure_leverage: f64,
    #[serde(default)]
    pub generality_gain: f64,
    #[serde(default)]
    pub assumption_debt: f64,
    #[serde(default)]
    pub bridge_centrality: f64,
    #[serde(default)]
    pub architecture_fit: f64,
    #[serde(default)]
    pub unjustified_narrowing: bool,
    #[serde(default)]
    pub remaining_goal_gaps_if_successful: Vec<String>,
    pub blockers: Vec<String>,
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StrategyRouteState {
    pub title: String,
    pub mechanism: String,
    pub mathematical_frontier: String,
    pub decisive_obstacle: String,
    #[serde(default)]
    pub evidence_for: Vec<String>,
    #[serde(default)]
    pub evidence_against: Vec<String>,
    pub status: String,
    pub revisit_condition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StrategyInterfaceDebt {
    pub interface_name: String,
    pub input_required: String,
    pub output_available: String,
    #[serde(default)]
    pub missing_matches: Vec<String>,
    pub failure_if_ignored: String,
    #[serde(default)]
    pub affected_goal_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StrategyDirectorOutput {
    pub verdict_summary: String,
    pub fixed_goal: String,
    #[serde(default)]
    pub proof_skeleton: Vec<String>,
    #[serde(default)]
    pub route_portfolio: Vec<StrategyRouteState>,
    #[serde(default)]
    pub interface_debts: Vec<StrategyInterfaceDebt>,
    pub central_missing_bridge: String,
    pub method_vs_proposition_failure: String,
    #[serde(default)]
    pub dangerous_shortcuts: Vec<String>,
    #[serde(default)]
    pub strategy_directives: Vec<String>,
    #[serde(default)]
    pub literature_priorities: Vec<String>,
    pub macro_replan_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReflectionOutput {
    pub summary: String,
    pub reviews: Vec<RouteReflection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SupervisorOutput {
    pub rationale_summary: String,
    pub assignments: Vec<AssignmentDraft>,
    pub targeted_uncertainty_ids: Vec<String>,
    pub suggestion_decisions: Vec<String>,
    pub deferred_route_indices: Vec<usize>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub struct RankingWeights {
    pub expected_goal_progress: f64,
    pub uncertainty_reduction: f64,
    pub human_suggestion_alignment: f64,
    pub evidence_support: f64,
    pub route_diversity: f64,
    pub verifiability: f64,
    pub novelty: f64,
    pub goal_closure_leverage: f64,
    pub generality_gain: f64,
    pub bridge_centrality: f64,
    pub architecture_fit: f64,
    pub assumption_debt_penalty: f64,
    pub unjustified_narrowing_penalty: f64,
}

impl Default for RankingWeights {
    fn default() -> Self {
        Self {
            expected_goal_progress: 0.25,
            uncertainty_reduction: 0.20,
            human_suggestion_alignment: 0.15,
            evidence_support: 0.15,
            route_diversity: 0.10,
            verifiability: 0.10,
            novelty: 0.05,
            goal_closure_leverage: 0.20,
            generality_gain: 0.10,
            bridge_centrality: 0.10,
            architecture_fit: 0.10,
            assumption_debt_penalty: 0.25,
            unjustified_narrowing_penalty: 0.30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CandidateDraft {
    pub statement: String,
    #[serde(default)]
    pub assumptions: Vec<String>,
    pub proof_markdown: String,
    #[serde(default)]
    pub dependency_fact_ids: Vec<String>,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub definitions_introduced: Attributes,
    #[serde(default)]
    pub external_source_ids: Vec<String>,
    pub candidate_type: CandidateType,
    #[serde(default)]
    pub target_goal_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DiscoveryDraft {
    pub kind: String,
    pub statement: String,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FailureDraft {
    pub failure_type: String,
    pub summary: String,
    pub repairable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkerOutput {
    pub summary: String,
    #[serde(default)]
    pub discoveries: Vec<DiscoveryDraft>,
    #[serde(default)]
    pub candidates: Vec<CandidateDraft>,
    #[serde(default)]
    pub failures: Vec<FailureDraft>,
    #[serde(default)]
    pub uncertainties: Vec<String>,
    #[serde(default)]
    pub sources: Vec<SourceDraft>,
    #[serde(default)]
    pub experiments: Vec<ExperimentDraft>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ExperimentDraft {
    pub language: String,
    pub program_text: String,
    pub input: Value,
    pub environment: Value,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub artifacts: Vec<Value>,
    pub conclusion_mapping: Value,
    pub replay_command: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ExperimentCapsule {
    pub capsule_id: String,
    pub project_id: String,
    pub task_id: Option<String>,
    pub route_id: Option<String>,
    pub language: String,
    pub program_text: String,
    pub input: Value,
    pub environment: Value,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub artifacts: Vec<Value>,
    pub conclusion_mapping: Value,
    pub replay_command: Vec<String>,
    pub status: String,
    pub content_hash: String,
    pub created_at: DateTime<Utc>,
    pub replayed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FactChallenge {
    pub challenge_id: String,
    pub project_id: String,
    pub fact_id: String,
    pub kind: String,
    pub reason: String,
    pub status: String,
    pub requested_by: String,
    pub verification_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FactImpact {
    pub fact_id: String,
    pub affected_fact_ids: Vec<String>,
    pub reopened_goal_ids: Vec<String>,
    pub paused_route_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FactGovernanceResult {
    pub fact: Fact,
    pub challenge: Option<FactChallenge>,
    pub verification_id: Option<String>,
    pub impact: FactImpact,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TaskSteer {
    pub steer_id: String,
    pub project_id: String,
    pub task_id: String,
    pub command_id: String,
    pub content: String,
    pub expected_task_revision: i64,
    pub expected_route_epoch: i64,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub applied_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HumanQuestion {
    pub question_id: String,
    pub project_id: String,
    pub question: String,
    pub options: Vec<Value>,
    pub blocking_entity_ids: Vec<String>,
    pub status: String,
    pub answer: Option<Value>,
    pub asked_by: String,
    pub answered_by: Option<String>,
    pub timeout_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub answered_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UsageSummary {
    pub project_id: String,
    pub model_calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub elapsed_ms: i64,
    pub estimated_cost_usd: Option<f64>,
    pub by_scope: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Actor {
    pub actor_id: String,
    pub display_name: String,
    pub role: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SecurityAuditEntry {
    pub audit_id: String,
    pub project_id: Option<String>,
    pub actor_id: Option<String>,
    pub action: String,
    pub target_kind: String,
    pub target_id: String,
    pub decision: String,
    pub reason: Option<String>,
    pub request_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProjectFactImport {
    pub import_id: String,
    pub target_project_id: String,
    pub source_project_id: String,
    pub source_fact_id: String,
    pub content_hash: String,
    pub assurance_snapshot: Value,
    pub status: String,
    pub imported_by: String,
    pub created_at: DateTime<Utc>,
    pub invalidated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkerNode {
    pub node_id: String,
    pub display_name: String,
    pub capabilities: Value,
    pub status: String,
    pub node_epoch: i64,
    pub registered_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TaskLease {
    pub lease_id: String,
    pub project_id: String,
    pub task_id: String,
    pub node_id: String,
    pub task_revision: i64,
    pub route_epoch: i64,
    pub lease_epoch: i64,
    pub status: String,
    pub leased_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ResearchDelta {
    pub delta_id: String,
    pub project_id: String,
    pub from_revision: i64,
    pub to_revision: i64,
    pub accepted_fact_ids: Vec<String>,
    pub rejected_candidate_ids: Vec<String>,
    pub new_proof_debt_ids: Vec<String>,
    pub solved_goal_ids: Vec<String>,
    pub reopened_goal_ids: Vec<String>,
    pub changed_uncertainty_ids: Vec<String>,
    pub new_failure_pattern_ids: Vec<String>,
    pub changed_source_ids: Vec<String>,
    pub completed_task_attempt_ids: Vec<String>,
    pub failed_or_expired_attempt_ids: Vec<String>,
    pub human_command_ids: Vec<String>,
    pub route_state_changes: Vec<Value>,
    pub status: String,
    pub consumed_by_plan_revision_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FactImpactRecord {
    pub impact_id: String,
    pub project_id: String,
    pub fact_id: String,
    pub source_revision: i64,
    pub closed_goal_ids: Vec<String>,
    pub unblocked_route_ids: Vec<String>,
    pub invalidated_task_ids: Vec<String>,
    pub newly_enabled_task_templates: Vec<String>,
    pub dominated_route_ids: Vec<String>,
    pub resolved_uncertainty_ids: Vec<String>,
    pub planner_disposition: String,
    pub disposition_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub applied_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Bottleneck {
    pub bottleneck_id: String,
    pub project_id: String,
    pub target_goal_ids: Vec<String>,
    pub kind: String,
    pub precise_statement: String,
    #[schema(value_type = Object)]
    pub completion_contract: Value,
    pub evidence_ids: Vec<String>,
    pub blocked_route_ids: Vec<String>,
    pub attempted_task_ids: Vec<String>,
    pub failure_pattern_ids: Vec<String>,
    pub repair_action_ids: Vec<String>,
    pub priority: f64,
    pub status: String,
    pub reopen_condition: Option<String>,
    pub created_revision: i64,
    pub updated_revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PlanRevisionRecord {
    pub plan_revision_id: String,
    pub project_id: String,
    pub ordinal: i64,
    pub round_id: Option<String>,
    pub based_on_project_revision: i64,
    pub consumed_delta_from: i64,
    pub consumed_delta_to: i64,
    pub planner_mode: String,
    pub context_packet_id: Option<String>,
    pub route_decisions: Vec<Value>,
    pub bottleneck_updates: Vec<Value>,
    pub task_contract_ids: Vec<String>,
    pub status: String,
    pub rationale: String,
    pub created_at: DateTime<Utc>,
    pub committed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TaskContract {
    pub task_contract_id: String,
    pub project_id: String,
    pub task_id: String,
    pub plan_revision_id: String,
    pub contract_version: i64,
    pub route_id: String,
    pub target_goal_ids: Vec<String>,
    pub bottleneck_id: Option<String>,
    pub task_kind: String,
    pub precise_objective: String,
    pub allowed_input_ids: Vec<String>,
    pub context_packet_id: Option<String>,
    pub allowed_tools: Vec<String>,
    pub forbidden_actions: Vec<String>,
    #[schema(value_type = Object)]
    pub completion_contract: Value,
    #[schema(value_type = Object)]
    pub budget: Value,
    #[schema(value_type = Object)]
    pub checkpoint_policy: Value,
    #[schema(value_type = Object)]
    pub retry_policy: Value,
    #[schema(value_type = Object)]
    pub fallback_policy: Value,
    pub route_cancellation_epoch: i64,
    pub content_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ContextPacket {
    pub context_packet_id: String,
    pub project_id: String,
    pub task_id: Option<String>,
    pub route_id: Option<String>,
    pub packet_kind: String,
    pub source_revision: i64,
    pub problem_contract_excerpt: String,
    pub objective: String,
    pub known_fact_ids: Vec<String>,
    pub bottleneck_id: Option<String>,
    #[schema(value_type = Object)]
    pub route_progress: Value,
    pub relevant_failure_pattern_ids: Vec<String>,
    pub relevant_uncertainty_ids: Vec<String>,
    pub source_refs: Vec<EntityRef>,
    pub omitted_sections: Vec<Value>,
    pub token_estimate: i64,
    #[schema(value_type = Object)]
    pub content: Value,
    pub content_hash: String,
    pub status: String,
    pub invalidation_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkerInstance {
    pub worker_instance_id: String,
    pub project_id: String,
    pub worker_id: Option<String>,
    pub backend: String,
    pub backend_version: Option<String>,
    pub model: Option<String>,
    pub status: String,
    #[schema(value_type = Object)]
    pub capabilities: Value,
    pub working_directory: Option<String>,
    #[schema(value_type = Option<Object>)]
    pub handshake: Option<Value>,
    pub quarantine_reason: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ready_at: Option<DateTime<Utc>>,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub exited_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TaskAttempt {
    pub attempt_id: String,
    pub project_id: String,
    pub task_id: String,
    pub worker_instance_id: Option<String>,
    pub attempt_no: i64,
    pub status: String,
    pub lease_epoch: i64,
    pub plan_revision_id: Option<String>,
    pub route_cancellation_epoch: i64,
    pub context_packet_id: Option<String>,
    pub failure_signature: Option<String>,
    pub failure_reason: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkerResultEnvelope {
    pub result_envelope_id: String,
    pub project_id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub lease_id: String,
    pub lease_token: String,
    pub lease_epoch: i64,
    pub plan_revision_id: Option<String>,
    pub route_cancellation_epoch: i64,
    pub outcome: String,
    #[schema(value_type = Object)]
    pub payload: Value,
    pub result_artifact_id: Option<String>,
    pub content_hash: String,
    pub idempotency_key: String,
    pub status: String,
    pub rejection_reason: Option<String>,
    pub submitted_at: DateTime<Utc>,
    pub ingested_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PlannerHealth {
    pub project_id: String,
    pub circuit_state: String,
    pub consecutive_failures: i64,
    pub last_failure_reason: Option<String>,
    pub opened_at: Option<DateTime<Utc>>,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub last_probe_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StorageHealth {
    pub mode: String,
    pub state_writer_queue_depth: i64,
    pub state_writer_queue_capacity: i64,
    pub state_writer_admitted_total: i64,
    pub state_writer_completed_total: i64,
    pub state_writer_queue_wait_seconds: f64,
    pub state_writer_last_transaction_seconds: f64,
    pub sqlite_busy_total: i64,
    pub outbox_pending: i64,
    pub outbox_oldest_age_seconds: Option<i64>,
    pub orphaned_task_attempts: i64,
    pub expired_active_leases: i64,
    pub last_reconciliation: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::RouteProposal;

    #[test]
    fn route_score_uses_configured_architecture_weights() {
        let route = RouteProposal {
            title: "route".into(),
            method_summary: "method".into(),
            target_goal_ids: vec![],
            required_fact_ids: vec![],
            expected_subgoals: vec![],
            expected_goal_progress: 1.0,
            uncertainty_reduction: 1.0,
            human_suggestion_alignment: 1.0,
            evidence_support: 1.0,
            route_diversity: 1.0,
            verifiability: 1.0,
            novelty: 1.0,
            failure_similarity_penalty: 0.1,
            cost_penalty: 0.2,
            risks: vec![],
        };
        assert!((route.score() - 0.7).abs() < f64::EPSILON);
    }
}
