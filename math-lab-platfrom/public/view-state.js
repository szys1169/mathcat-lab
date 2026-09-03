export function resolveResearchView(requestedView, boardEnabled) {
  return requestedView === "board" && boardEnabled ? "board" : "chat";
}

export function isCurrentConversationLoad(activeConversationId, requestedConversationId, activeLoadId, requestedLoadId) {
  return Boolean(requestedConversationId) && activeConversationId === requestedConversationId && activeLoadId === requestedLoadId;
}

export function normalizeResearchBoard(value) {
  if (!value || typeof value !== "object") return null;
  const board = {
    ...value,
    problem: { version: 1, original: "", statement: "", assumptions: [], goal: "", ...(value.problem || {}) },
    decisions: Array.isArray(value.decisions) ? value.decisions : [],
    events: Array.isArray(value.events) ? value.events : [],
    uncertainties: Array.isArray(value.uncertainties) ? value.uncertainties : [],
    tasks: Array.isArray(value.tasks) ? value.tasks : [],
    workers: Array.isArray(value.workers) ? value.workers : [],
    verificationQueue: Array.isArray(value.verificationQueue) ? value.verificationQueue : [],
    artifacts: Array.isArray(value.artifacts) ? value.artifacts : [],
    experiments: Array.isArray(value.experiments) ? value.experiments : [],
    summary: value.summary && typeof value.summary === "object" ? value.summary : {}
  };
  if (board.agent === "rethlas") return { ...board, iterations: Array.isArray(value.iterations) ? value.iterations : [], blueprint: Array.isArray(value.blueprint) ? value.blueprint : [], verification: { verdict: "pending", criticalErrors: [], gaps: [], repairHints: [], ...(value.verification || {}) } };
  if (board.agent === "danus") return { ...board, strategy: { elaboration: "等待首次综合", masterGuidance: "等待确定研究方向", ...(value.strategy || {}) }, workers: Array.isArray(value.workers) ? value.workers : [], memories: { local: [], global: [], ...(value.memories || {}) }, facts: Array.isArray(value.facts) ? value.facts : [], verificationQueue: Array.isArray(value.verificationQueue) ? value.verificationQueue : [] };
  return { ...board, agent: "mathcat", routes: Array.isArray(value.routes) ? value.routes : [], goals: Array.isArray(value.goals) ? value.goals : [], claims: Array.isArray(value.claims) ? value.claims : [], failedRoutes: Array.isArray(value.failedRoutes) ? value.failedRoutes : [] };
}
