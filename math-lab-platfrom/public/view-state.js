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
    planningSuggestions: Array.isArray(value.planningSuggestions) ? value.planningSuggestions : [],
    decisions: Array.isArray(value.decisions) ? value.decisions : [],
    events: Array.isArray(value.events) ? value.events : [],
    uncertainties: Array.isArray(value.uncertainties) ? value.uncertainties : [],
    tasks: Array.isArray(value.tasks) ? value.tasks : [],
    workers: Array.isArray(value.workers) ? value.workers : [],
    verificationQueue: Array.isArray(value.verificationQueue) ? value.verificationQueue : [],
    artifacts: Array.isArray(value.artifacts) ? value.artifacts : [],
    experiments: Array.isArray(value.experiments) ? value.experiments : [],
    summary: value.summary && typeof value.summary === "object" ? value.summary : {},
    integration: value.integration && typeof value.integration === "object" ? value.integration : { structured: value.agent === "mathcat", source: value.agent === "mathcat" ? "mathcat_api" : "legacy", message: value.agent === "mathcat" ? "结构化数据由 MathCat API 提供。" : "这是旧版白板记录，尚未声明结构化数据来源。" }
  };
  if (board.agent === "rethlas") return { ...board, iterations: Array.isArray(value.iterations) ? value.iterations : [], blueprint: Array.isArray(value.blueprint) ? value.blueprint : [], verification: { verdict: "pending", criticalErrors: [], gaps: [], repairHints: [], ...(value.verification || {}) } };
  if (board.agent === "danus") return { ...board, strategy: { elaboration: "等待首次综合", masterGuidance: "等待确定研究方向", ...(value.strategy || {}) }, workers: Array.isArray(value.workers) ? value.workers : [], memories: { local: [], global: [], ...(value.memories || {}) }, facts: Array.isArray(value.facts) ? value.facts : [], verificationQueue: Array.isArray(value.verificationQueue) ? value.verificationQueue : [] };
  if (!board.agent || board.agent === "mathcat") return { ...board, agent: "mathcat", routes: Array.isArray(value.routes) ? value.routes : [], goals: Array.isArray(value.goals) ? value.goals : [], claims: Array.isArray(value.claims) ? value.claims : [], failedRoutes: Array.isArray(value.failedRoutes) ? value.failedRoutes : [] };
  return { ...board, integration: { structured: false, source: "unknown", message: `无法识别白板智能体：${board.agent}` }, routes: [], goals: [], claims: [], failedRoutes: [] };
}
