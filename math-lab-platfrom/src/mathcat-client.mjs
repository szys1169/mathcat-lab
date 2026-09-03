import crypto from "node:crypto";

const makeKey = () => crypto.randomUUID();
const text = (value, fallback = "") => String(value ?? fallback);
const list = (value) => Array.isArray(value) ? value : [];

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
  return text(data.message ?? data.summary ?? data.reason ?? data.title, `${text(event?.event_type, "研究状态更新")} · ${text(event?.entity?.kind, "project")}`);
}

export function mapMathCatBoard(remote, binding = {}) {
  const graph = remote?.graph;
  const dependencyGraph = graph ? {
    rootId: graph.nodes?.find((item) => item.kind === "goal")?.id,
    nodes: list(graph.nodes).map((item) => ({ ...item, detail: text(item.attributes?.summary) })),
    edges: list(graph.edges).map((item) => ({ id: item.id, from: item.source, to: item.target, relation: item.kind }))
  } : undefined;
  return {
    ...binding,
    id: binding.id,
    conversationId: binding.conversationId,
    workspaceId: binding.workspaceId ?? null,
    remoteProjectId: remote.project_id,
    agent: "mathcat",
    mode: remote.mode || "human_collaboration",
    status: remote.status || "created",
    revision: Number(remote.revision || 0),
    problem: {
      version: Number(remote.problem?.version || 1),
      original: text(remote.problem?.original_problem),
      statement: text(remote.problem?.target_statement ?? remote.problem?.original_problem),
      goal: text(remote.problem?.target_statement ?? remote.problem?.original_problem),
      assumptions: list(remote.problem?.assumptions),
      successCriteria: text(remote.problem?.success_criteria)
    },
    summary: remote.summary || {},
    routes: [...list(remote.routes).map((route) => ({
      ...route,
      id: route.route_id,
      summary: text(route.method_summary),
      humanStatus: text(route.human_review, "pending")
    })), ...list(remote.route_proposals).filter((proposal) => !proposal.route_id).map((proposal) => ({
      ...proposal,
      id: proposal.proposal_id,
      summary: text(proposal.method_summary),
      status: "pending",
      humanStatus: "proposed"
    }))],
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
    events: [...list(binding.events), ...list(remote.timeline).map((event) => ({ id: event.event_id, at: event.occurred_at, kind: event.event_type, text: eventText(event) }))].filter((event, index, events) => events.findIndex((item) => item.id === event.id) === index),
    dependencyGraph,
    capabilities: remote.capabilities || {},
    updatedAt: new Date().toISOString()
  };
}

export class MathCatClient {
  constructor({ baseUrl, actorId, token, timeoutMs = 10_000 }) {
    this.baseUrl = String(baseUrl || "http://127.0.0.1:8787").replace(/\/$/, "");
    this.actorId = String(actorId || "mathcat-lab");
    this.token = String(token || "");
    this.timeoutMs = timeoutMs;
  }

  headers(write = false) {
    const headers = { "content-type": "application/json", "x-actor-id": this.actorId, authorization: `Bearer ${this.token}` };
    if (write) headers["idempotency-key"] = makeKey();
    return headers;
  }

  async request(pathname, { method = "GET", body, write = false, publicEndpoint = false } = {}) {
    const response = await fetch(`${this.baseUrl}${pathname}`, {
      method,
      headers: publicEndpoint ? { "content-type": "application/json" } : this.headers(write),
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(this.timeoutMs)
    });
    const payload = response.status === 204 ? null : await response.json().catch(() => null);
    if (!response.ok) {
      const error = new Error(payload?.error?.message || payload?.message || `MathCat API returned ${response.status}.`);
      error.status = response.status;
      error.details = payload?.error?.details;
      throw error;
    }
    return payload?.data ?? payload;
  }

  async requestText(pathname) {
    const response = await fetch(`${this.baseUrl}${pathname}`, { headers: this.headers(), signal: AbortSignal.timeout(this.timeoutMs) });
    if (!response.ok) throw new Error(`MathCat API returned ${response.status}.`);
    return response.text();
  }

  health() { return this.request("/health", { publicEndpoint: true }); }
  bootstrap(displayName = "MathCat Lab") { return this.request("/api/v1/actors/bootstrap", { method: "POST", body: { actor_id: this.actorId, display_name: displayName, token: this.token }, publicEndpoint: true }); }
  createProject({ name, problem }) { return this.request("/api/v1/projects", { method: "POST", body: { name, problem, target_statement: problem, assumptions: [], success_criteria: "形成可审计的证明路线，并由验证流程确认结论", human_route_approval: true } }); }
  async board(projectId) {
    const [board, proposals] = await Promise.all([
      this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/board?include=tasks,workers,verification,artifacts,graph&timeline_limit=100`),
      this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/route-proposals`)
    ]);
    const resultArtifacts = list(board.artifacts).filter((item) => item.kind === "worker_result_envelope_payload").slice(0, 4);
    const experimentGroups = await Promise.all(resultArtifacts.map(async (artifact) => {
      try {
        const raw = await this.requestText(`/api/v1/projects/${encodeURIComponent(projectId)}/artifacts/${encodeURIComponent(artifact.artifact_id)}/content`);
        return list(JSON.parse(raw)?.experiments).map((experiment) => ({ ...experiment, sourceArtifactId: artifact.artifact_id }));
      } catch { return []; }
    }));
    return { ...board, route_proposals: proposals, experiments: experimentGroups.flat() };
  }
  reviseProblem(projectId, board, input) { return this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/problem-revisions`, { method: "POST", write: true, body: { expected_revision: board.revision, target_statement: text(input.statement ?? input.goal), assumptions: list(input.assumptions ?? board.problem?.assumptions), success_criteria: text(input.successCriteria ?? board.problem?.successCriteria, "形成可审计的证明路线，并由验证流程确认结论"), change_reason: text(input.changeReason, "研究者在 MathCat Lab 白板中更新问题"), replan: true } }); }
  proposeRoute(projectId, board, input) {
    const proposal = { expected_revision: board.revision, title: text(input.title, "新路线"), method_summary: text(input.summary), target_goal_ids: list(input.targetGoalIds), required_fact_ids: [], known_risks: list(input.knownRisks), reason: text(input.reason, "研究者通过 MathCat Lab 白板提出路线") };
    return this.request(`/api/v1/projects/${encodeURIComponent(projectId)}/route-proposals`, { method: "POST", write: true, body: proposal });
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
