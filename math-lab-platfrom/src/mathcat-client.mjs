import crypto from "node:crypto";
import { extractPrimaryResearchTarget } from "./research-context.mjs";

const makeKey = () => crypto.randomUUID();
const text = (value, fallback = "") => String(value ?? fallback);
const list = (value) => Array.isArray(value) ? value : [];
const REVIEW_MODES = new Set(["automatic", "balanced", "strict"]);

function positiveInteger(value, field) {
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < 1) throw new Error(`${field} must be a positive integer.`);
  return parsed;
}

function featureError(error, feature) {
  if ([404, 405].includes(error?.status)) {
    const unsupported = new Error(`当前 MathCat 后端不支持“${feature}”。请先更新本地 MathCat，再重试。`);
    unsupported.status = 409;
    unsupported.cause = error;
    return unsupported;
  }
  return error;
}

function mapBudget(value) {
  if (!value || typeof value !== "object") return null;
  const budget = {
    maxRounds: Number(value.max_rounds),
    maxParallelWorkers: Number(value.max_parallel_workers),
    maxMinutesPerTask: Number(value.max_minutes_per_task),
    maxModelCallsPerTask: Number(value.max_model_calls_per_task),
    maxTotalModelCalls: Number(value.max_total_model_calls)
  };
  return Object.values(budget).every((item) => Number.isFinite(item)) ? budget : null;
}

function readableSuggestionDecision(value) {
  if (value == null || value === "") return { rationale: "", disposition: "" };
  let parsed = value;
  if (typeof value === "string") {
    try { parsed = JSON.parse(value); }
    catch { return { rationale: value, disposition: "" }; }
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return { rationale: text(parsed), disposition: "" };
  return {
    rationale: text(parsed.rationale ?? parsed.reason ?? parsed.summary),
    disposition: text(parsed.disposition)
  };
}

function optionValue(option, index) {
  if (option && typeof option === "object" && !Array.isArray(option)) {
    return {
      value: text(option.value ?? option.id, `option_${index + 1}`),
      label: text(option.label ?? option.title ?? option.value, `选项 ${index + 1}`),
      description: text(option.description ?? option.reason)
    };
  }
  return { value: text(option, `option_${index + 1}`), label: text(option, `选项 ${index + 1}`), description: "" };
}

function eventText(event) {
  const data = event?.data && typeof event.data === "object" ? event.data : {};
  return text(data.message ?? data.summary ?? data.reason ?? data.title, `${text(event?.type ?? event?.event_type, "研究状态更新")} · ${text(event?.entity?.kind, "project")}`);
}

const SAFE_EVENT_DATA_KEYS = new Set([
  "message", "summary", "reason", "title", "status", "stage", "from", "to", "action", "kind", "backend", "verdict",
  "focus",
  "round_id", "number", "attempt_number", "soft_timeout_seconds", "hard_timeout_seconds", "elapsed_ms", "next_status",
  "route_id", "task_id", "worker_id", "candidate_id", "verification_id", "case_id", "attempt_id", "plan_revision_id",
  "routes", "tasks", "planner_mode", "mandatory", "mathematical_endorsement", "retryable", "replayable_result", "gap_count"
]);

function safeEventValue(value, depth = 0) {
  if (value == null || typeof value === "boolean" || typeof value === "number") return value;
  if (typeof value === "string") return value.slice(0, 2_000);
  if (depth >= 2) return undefined;
  if (Array.isArray(value)) return value.slice(0, 24).map((item) => safeEventValue(item, depth + 1)).filter((item) => item !== undefined);
  if (typeof value === "object") {
    return Object.fromEntries(Object.entries(value).slice(0, 24).map(([key, item]) => [key, safeEventValue(item, depth + 1)]).filter(([, item]) => item !== undefined));
  }
  return undefined;
}

function safeEntity(value) {
  if (!value || typeof value !== "object") return null;
  const kind = text(value.kind).slice(0, 80);
  const id = text(value.id).slice(0, 240);
  return kind || id ? { kind, id } : null;
}

function mapTimelineEvent(event) {
  const type = text(event?.type ?? event?.event_type, "research.status.updated");
  const rawData = event?.data && typeof event.data === "object" && !Array.isArray(event.data) ? event.data : {};
  const data = Object.fromEntries(Object.entries(rawData).filter(([key]) => SAFE_EVENT_DATA_KEYS.has(key)).map(([key, value]) => [key, safeEventValue(value)]).filter(([, value]) => value !== undefined));
  return {
    id: text(event?.event_id ?? event?.id),
    cursor: Number(event?.cursor ?? 0),
    revision: Number(event?.project_revision ?? event?.revision ?? 0),
    at: event?.occurred_at ?? event?.at ?? null,
    kind: type,
    type,
    entity: safeEntity(event?.entity),
    causedBy: safeEntity(event?.caused_by ?? event?.causedBy),
    data,
    text: eventText({ ...event, type })
  };
}

export function mapMathCatBoard(remote, binding = {}) {
  const graph = remote?.graph;
  const rawTargetStatement = text(remote.problem?.target_statement ?? remote.problem?.original_problem);
  const targetStatement = extractPrimaryResearchTarget(rawTargetStatement, rawTargetStatement).replace(/\s+/g, " ").trim();
  const graphRootId = graph?.root_id || graph?.nodes?.find((item) => item.kind === "goal" && text(item.label).replace(/\s+/g, " ").trim() === targetStatement)?.id || graph?.nodes?.find((item) => item.kind === "goal")?.id;
  const dependencyGraph = graph ? {
    rootId: graphRootId,
    nodes: list(graph.nodes).map((item) => ({
      ...item,
      label: item.id === graphRootId && item.kind === "goal" ? targetStatement : item.label,
      shortLabel: text(item.attributes?.user_title),
      detail: text(item.attributes?.plain_language_summary ?? item.attributes?.technical_summary ?? item.attributes?.summary),
      routeRole: text(item.attributes?.route_role),
      approachKind: text(item.attributes?.approach_kind)
    })),
    edges: list(graph.edges).map((item) => ({ id: item.id, from: item.source, to: item.target, relation: item.kind }))
  } : undefined;
  // The backend is the durable authority for live research settings. Keep the
  // local binding only as a compatibility fallback for pre-settings servers.
  const remoteSettings = remote?.settings && typeof remote.settings === "object" ? remote.settings : null;
  const budget = mapBudget(remoteSettings?.budget ?? remote?.budget ?? remote?.project?.budget);
  const remoteReviewMode = text(remoteSettings?.review_mode);
  const reviewMode = REVIEW_MODES.has(remoteReviewMode)
    ? remoteReviewMode
    : REVIEW_MODES.has(binding.reviewMode) ? binding.reviewMode : "strict";
  const suggestionSource = Array.isArray(remote?.planning_suggestions) ? remote.planning_suggestions : list(binding.planningSuggestions);
  return {
    ...binding,
    id: binding.id,
    conversationId: binding.conversationId,
    workspaceId: binding.workspaceId ?? null,
    remoteProjectId: remote.project_id,
    agent: "mathcat",
    mode: remote.mode || "human_collaboration",
    integration: { structured: true, source: "mathcat_api", message: "结构化数据由 MathCat API 提供。" },
    status: remote.status || "created",
    revision: Number(remote.revision || 0),
    eventCursor: Number(remote.event_cursor || 0),
    problem: {
      version: Number(remote.problem?.version || 1),
      original: text(remote.problem?.original_problem),
      statement: targetStatement,
      goal: targetStatement,
      assumptions: list(remote.problem?.assumptions),
      successCriteria: text(remote.problem?.success_criteria)
    },
    summary: remote.summary || {},
    routes: [...list(remote.routes).map((route) => ({
      ...route,
      isProposal: false,
      id: route.route_id,
      technicalTitle: text(route.title),
      title: text(route.user_title, route.title),
      summary: text(route.method_summary),
      approachKind: text(route.approach_kind),
      routeRole: text(route.route_role),
      plainLanguageSummary: text(route.plain_language_summary),
      whyThisRoute: text(route.why_this_route),
      expectedOutput: text(route.expected_output),
      relationToGoal: text(route.relation_to_goal),
      steps: list(route.steps).map((item) => text(item)),
      targetGoalIds: list(route.target_goal_ids).map((item) => text(item)),
      humanStatus: text(route.human_review, "pending")
    })), ...list(remote.route_proposals).filter((proposal) => !proposal.route_id).map((proposal) => ({
      ...proposal,
      isProposal: true,
      id: proposal.proposal_id,
      technicalTitle: text(proposal.title),
      title: text(proposal.user_title, proposal.title),
      summary: text(proposal.method_summary),
      approachKind: text(proposal.approach_kind),
      routeRole: text(proposal.route_role),
      plainLanguageSummary: text(proposal.plain_language_summary),
      whyThisRoute: text(proposal.why_this_route),
      expectedOutput: text(proposal.expected_output),
      relationToGoal: text(proposal.relation_to_goal),
      steps: list(proposal.steps).map((item) => text(item)),
      targetGoalIds: list(proposal.target_goal_ids).map((item) => text(item)),
      status: "pending",
      humanStatus: "proposed"
    }))],
    planningSuggestions: suggestionSource.map((suggestion) => {
      const decision = readableSuggestionDecision(suggestion.decision);
      return {
        ...suggestion,
        id: text(suggestion.suggestion_id ?? suggestion.id ?? suggestion.command_id),
        commandId: text(suggestion.command_id ?? suggestion.commandId),
        content: text(suggestion.content),
        targetRouteId: text(suggestion.target_route_id ?? suggestion.targetRouteId),
        status: text(suggestion.status, decision.disposition || "pending"),
        decision: decision.rationale,
        decisionDisposition: decision.disposition,
        createdRound: Number(suggestion.created_in_round ?? suggestion.createdRound ?? 0),
        effectiveRound: Number(suggestion.effective_round ?? suggestion.effectiveRound ?? 0),
        submittedAt: suggestion.submittedAt ?? suggestion.created_at ?? null
      };
    }),
    goals: list(remote.goals).map((goal) => ({ ...goal, id: goal.goal_id })),
    claims: list(remote.claims).map((claim) => ({ ...claim, id: claim.claim_id, routeId: claim.origin_route_id })),
    failedRoutes: list(remote.failed_routes).map((route) => ({ ...route, id: route.route_id, summary: text(route.method_summary), humanStatus: text(route.human_review) })),
    decisions: [...list(remote.human_questions).map((question) => {
      const answer = question.answer && typeof question.answer === "object" ? question.answer.value : question.answer;
      const note = question.answer && typeof question.answer === "object" ? question.answer.note : "";
      return {
        id: question.question_id,
        source: text(question.asked_by, "mathcat"),
        question: text(question.question),
        context: text(question.context ?? question.reason),
        options: list(question.options).map(optionValue),
        blockingEntityIds: list(question.blocking_entity_ids),
        status: question.status === "open" ? "pending" : question.status,
        answer: answer == null ? null : text(answer),
        note: text(note),
        createdAt: question.created_at,
        answeredAt: question.answered_at
      };
    }), ...list(binding.decisions).filter((local) => !list(remote.human_questions).some((question) => question.question_id === local.id))],
    uncertainties: list(remote.uncertainties),
    tasks: list(remote.tasks),
    workers: list(remote.workers),
    verificationQueue: list(remote.verification_queue),
    artifacts: list(remote.artifacts),
    experiments: list(remote.experiments),
    events: [...list(binding.events), ...list(remote.timeline).map(mapTimelineEvent)].filter((event, index, events) => events.findIndex((item) => item.id === event.id) === index),
    dependencyGraph,
    capabilities: remote.capabilities || {},
    budget,
    reviewMode,
    runningTaskPolicy: text(remoteSettings?.running_task_policy),
    updatedAt: new Date().toISOString()
  };
}

export class MathCatClient {
  constructor({ baseUrl, actorId, token, timeoutMs = 10_000, fetchImpl = globalThis.fetch }) {
    this.baseUrl = String(baseUrl || "http://127.0.0.1:8787").replace(/\/$/, "");
    this.actorId = String(actorId || "mathcat-lab");
    this.token = String(token || "");
    this.timeoutMs = timeoutMs;
    this.fetch = fetchImpl;
    this.artifactExperiments = new Map();
  }

  headers(write = false) {
    const headers = { "content-type": "application/json", "x-actor-id": this.actorId, authorization: `Bearer ${this.token}` };
    if (write) headers["idempotency-key"] = makeKey();
    return headers;
  }

  async request(pathname, { method = "GET", body, write = false, publicEndpoint = false } = {}) {
    const headers = publicEndpoint ? { "content-type": "application/json" } : this.headers(write);
    const encodedBody = body === undefined ? undefined : JSON.stringify(body);
    let response;
    let payload;
    for (let attempt = 0; attempt < (write ? 2 : 1); attempt += 1) {
      try {
        response = await this.fetch(`${this.baseUrl}${pathname}`, { method, headers, body: encodedBody, signal: AbortSignal.timeout(this.timeoutMs) });
      } catch (error) {
        if (write && attempt === 0) continue;
        throw error;
      }
      if (response.status === 204) { payload = null; break; }
      try {
        payload = await response.json();
      } catch (error) {
        // A successful status with a truncated body is ambiguous: the server
        // may already have committed. Replay once with the same key.
        if (response.ok && write && attempt === 0) continue;
        if (response.ok) throw new Error("MathCat API returned an unreadable success response.", { cause: error });
        payload = null;
      }
      break;
    }
    if (!response.ok) {
      const error = new Error(payload?.error?.message || payload?.message || `MathCat API returned ${response.status}.`);
      error.status = response.status;
      error.details = payload?.error?.details;
      throw error;
    }
    const data = payload?.data ?? payload;
    return payload?.data && data && typeof data === "object" && !Array.isArray(data)
      ? { ...data, _responseMeta: payload.meta || null }
      : data;
  }

  async requestText(pathname) {
    const response = await this.fetch(`${this.baseUrl}${pathname}`, { headers: this.headers(), signal: AbortSignal.timeout(this.timeoutMs) });
    if (!response.ok) throw new Error(`MathCat API returned ${response.status}.`);
    return response.text();
  }

  health() { return this.request("/health", { publicEndpoint: true }); }
  bootstrap(displayName = "MathCat Lab") { return this.request("/api/v1/actors/bootstrap", { method: "POST", body: { actor_id: this.actorId, display_name: displayName, token: this.token }, publicEndpoint: true }); }
  createProject({ name, problem, targetStatement = problem, reviewMode = "strict" }) { return this.request("/api/v1/projects", { method: "POST", body: { name, problem, target_statement: targetStatement, assumptions: [], success_criteria: "形成可审计的证明路线，并由验证流程确认结论", review_mode: REVIEW_MODES.has(reviewMode) ? reviewMode : "strict", human_route_approval: reviewMode === "strict" } }); }
  projectCommand(projectId, board, command) {
    if (!["start", "resume", "stop"].includes(command)) throw new Error("Unknown project command.");
    return this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/commands/${command}`, {
      method: "POST",
      write: true,
      body: {
        expected_revision: board.revision,
        reason: `MathCat Lab 请求${{ start: "启动", resume: "继续", stop: "中止" }[command]}研究项目`,
        payload: {}
      }
    });
  }
  startProject(projectId, board) { return this.projectCommand(projectId, board, "start"); }
  resumeProject(projectId, board) { return this.projectCommand(projectId, board, "resume"); }
  stopProject(projectId, board) { return this.projectCommand(projectId, board, "stop"); }
  async board(projectId) {
    const [board, proposals, project] = await Promise.all([
      this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/board?include=tasks,workers,verification,artifacts,graph&timeline_limit=100`),
      this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/route-proposals`),
      this.request(`/api/v1/projects/${encodeURIComponent(projectId)}`)
    ]);
    const resultArtifacts = list(board.artifacts).filter((item) => item.kind === "worker_result_envelope_payload").slice(0, 4);
    const experimentGroups = await Promise.all(resultArtifacts.map(async (artifact) => {
      if (this.artifactExperiments.has(artifact.artifact_id)) return this.artifactExperiments.get(artifact.artifact_id);
      try {
        const raw = await this.requestText(`/api/v1/projects/${encodeURIComponent(projectId)}/artifacts/${encodeURIComponent(artifact.artifact_id)}/content`);
        const experiments = list(JSON.parse(raw)?.experiments).map((experiment) => ({ ...experiment, sourceArtifactId: artifact.artifact_id }));
        this.artifactExperiments.set(artifact.artifact_id, experiments);
        return experiments;
      } catch { return []; }
    }));
    return { ...board, project, budget: project?.budget ?? board?.budget, route_proposals: proposals, experiments: experimentGroups.flat() };
  }
  commandStatus(projectId, commandId) { return this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/commands/${encodeURIComponent(commandId)}`); }
  reviseProblem(projectId, board, input) { return this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/problem-revisions`, { method: "POST", write: true, body: { expected_revision: board.revision, target_statement: text(input.statement ?? input.goal), assumptions: list(input.assumptions ?? board.problem?.assumptions), success_criteria: text(input.successCriteria ?? board.problem?.successCriteria, "形成可审计的证明路线，并由验证流程确认结论"), change_reason: text(input.changeReason, "研究者在 MathCat Lab 白板中更新问题"), replan: true } }); }
  proposeRoute(projectId, board, input) {
    const proposal = { expected_revision: board.revision, title: text(input.title, "新路线"), method_summary: text(input.summary), target_goal_ids: list(input.targetGoalIds), required_fact_ids: [], known_risks: list(input.knownRisks), reason: text(input.reason, "研究者通过 MathCat Lab 白板提出路线") };
    return this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/route-proposals`, { method: "POST", write: true, body: proposal });
  }
  async createAndExecuteRoute(projectId, board, input) {
    const body = {
      expected_revision: board.revision,
      title: text(input.title, "新路线"),
      method_summary: text(input.summary),
      objective: text(input.objective, input.summary),
      completion_contract: text(input.completionContract),
      target_goal_ids: list(input.targetGoalIds),
      required_fact_ids: list(input.requiredFactIds),
      known_risks: list(input.knownRisks),
      reason: text(input.reason, "研究者通过 MathCat Lab 白板编写并立即执行路线")
    };
    let result;
    try {
      result = await this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/routes`, { method: "POST", write: true, body });
    } catch (error) {
      throw featureError(error, "编写并执行路线");
    }
    if (result?.execution_started !== true || !result?.route_id || !result?.task_id) {
      throw new Error("MathCat 未确认路线已经启动。为避免误报，白板不会把它显示为已执行；请更新本地 MathCat 后重试。");
    }
    return result;
  }
  async addPlanningSuggestion(projectId, board, input) {
    const content = text(input.content).trim();
    if (!content) throw new Error("规划建议不能为空。");
    const body = {
      expected_revision: board.revision,
      content,
      reason: text(input.reason, "研究者通过 MathCat Lab 白板补充规划建议")
    };
    if (text(input.targetRouteId).trim()) body.target_route_id = text(input.targetRouteId).trim();
    try {
      return await this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/suggestions`, { method: "POST", write: true, body });
    } catch (error) {
      throw featureError(error, "给规划建议");
    }
  }
  async goalReview(projectId, board, input) {
    const focus = text(input.focus).trim();
    if (!focus) throw new Error("请说明这次目标梳理希望重点讨论什么。");
    try {
      return await this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/commands/goal-review`, {
        method: "POST",
        write: true,
        body: { expected_revision: board.revision, reason: "研究者要求立即重新梳理目标并讨论", payload: { focus } }
      });
    } catch (error) {
      throw featureError(error, "重新梳理目标与讨论");
    }
  }
  async adjustBudget(projectId, board, input) {
    const current = board?.budget;
    if (!current) throw new Error("MathCat 未返回当前研究预算，无法安全覆盖设置。请先更新后端并刷新白板。");
    const limits = {
      max_rounds: positiveInteger(current.maxRounds, "max_rounds"),
      max_parallel_workers: positiveInteger(input.maxParallelWorkers ?? current.maxParallelWorkers, "max_parallel_workers"),
      max_minutes_per_task: positiveInteger(input.maxMinutesPerTask ?? current.maxMinutesPerTask, "max_minutes_per_task"),
      max_model_calls_per_task: positiveInteger(current.maxModelCallsPerTask, "max_model_calls_per_task"),
      max_total_model_calls: positiveInteger(current.maxTotalModelCalls, "max_total_model_calls")
    };
    if (limits.max_parallel_workers > 16) throw new Error("最大任务并行数必须在 1–16 之间。");
    if (limits.max_minutes_per_task > 1_440) throw new Error("单任务最长运行时间必须在 1–1440 分钟之间。");
    try {
      return await this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/commands/adjust-budget`, {
        method: "POST",
        write: true,
        body: { expected_revision: board.revision, reason: "研究者通过 MathCat Lab 白板调整当前研究设置", payload: { scope_kind: "project", scope_id: projectId, limits } }
      });
    } catch (error) {
      throw featureError(error, "调整研究预算");
    }
  }
  async updateReviewPolicy(projectId, board, reviewMode) {
    if (!REVIEW_MODES.has(reviewMode)) throw new Error("未知的人工参与程度。");
    try {
      return await this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/commands/review-policy`, {
        method: "POST",
        write: true,
        body: {
          expected_revision: board.revision,
          reason: "研究者通过 MathCat Lab 白板调整人工参与程度",
          payload: { review_mode: reviewMode, human_route_approval: reviewMode === "strict" }
        }
      });
    } catch (error) {
      throw featureError(error, "调整人工参与程度");
    }
  }
  async updateResearchSettings(projectId, board, input) {
    const current = board?.budget;
    if (!current) throw new Error("MathCat 未返回当前研究预算，无法安全保存整组设置。请先更新后端并刷新白板。");
    const reviewMode = text(input.reviewMode, board.reviewMode);
    if (!REVIEW_MODES.has(reviewMode)) throw new Error("未知的人工参与程度。");
    const limits = {
      max_rounds: positiveInteger(current.maxRounds, "max_rounds"),
      max_parallel_workers: positiveInteger(input.maxParallelWorkers ?? current.maxParallelWorkers, "max_parallel_workers"),
      max_minutes_per_task: positiveInteger(input.maxMinutesPerTask ?? current.maxMinutesPerTask, "max_minutes_per_task"),
      max_model_calls_per_task: positiveInteger(current.maxModelCallsPerTask, "max_model_calls_per_task"),
      max_total_model_calls: positiveInteger(current.maxTotalModelCalls, "max_total_model_calls")
    };
    if (limits.max_parallel_workers > 16) throw new Error("最大任务并行数必须在 1–16 之间。");
    if (limits.max_minutes_per_task > 1_440) throw new Error("单任务最长运行时间必须在 1–1440 分钟之间。");
    try {
      return await this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/commands/settings`, {
        method: "POST",
        write: true,
        body: {
          expected_revision: board.revision,
          reason: "研究者通过 MathCat Lab 白板调整当前研究设置",
          payload: { limits, review_mode: reviewMode }
        }
      });
    } catch (error) {
      throw featureError(error, "保存当前研究设置");
    }
  }
  routeCommand(projectId, routeId, board, command) {
    const endpoint = { approve: "approve", pause: "pause", reject: "prune", resume: "resume" }[command];
    if (!endpoint) throw new Error("Unknown route command.");
    return this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/routes/${encodeURIComponent(routeId)}/commands/${endpoint}`, { method: "POST", write: true, body: { expected_revision: board.revision, reason: `研究者在 MathCat Lab 中执行 ${command}`, payload: {} } });
  }
  answerQuestion(projectId, questionId, board, answer, note) {
    return this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/questions/${encodeURIComponent(questionId)}/commands/answer`, { method: "POST", write: true, body: { expected_revision: board.revision, reason: "研究者通过猫猫提问卡回答", payload: { answer: { value: answer, note: text(note) } } } });
  }
}
