const text = (value, fallback = "") => String(value ?? fallback);
const list = (value) => Array.isArray(value) ? value : [];

function statusOf(value, fallback = "pending") {
  const normalized = text(value, fallback).toLowerCase();
  const aliases = { created: "pending", proposed: "pending", active: "active", running: "active", complete: "verified", completed: "verified", accepted: "verified", approved: "verified", rejected: "failed", pruned: "failed", error: "failed", waiting: "blocked" };
  return aliases[normalized] || normalized;
}

function node(id, kind, label, status, detail = "", extra = {}) {
  return { id: text(id), kind, label: text(label, "未命名节点"), status: statusOf(status), detail: text(detail), trust: extra.trust || "unverified", logic: extra.logic || null, sourceId: extra.sourceId || null, meta: extra.meta || {} };
}

function edge(from, to, relation = "requires") {
  return { id: `${from}:${relation}:${to}`, from: text(from), to: text(to), relation };
}

function normalizeExplicitGraph(value, kind) {
  if (!value || !Array.isArray(value.nodes)) return null;
  const nodes = value.nodes.map((item, index) => {
    const nodeKind = item.kind || "claim";
    const trust = item.trust || (nodeKind === "fact" ? "fact" : ["verified", "accepted"].includes(String(item.status).toLowerCase()) ? "verified" : undefined);
    return node(item.id || `${kind}_${index}`, nodeKind, item.label || item.statement || item.title, item.status, item.detail || item.summary, { ...item, trust });
  });
  const ids = new Set(nodes.map((item) => item.id));
  const edges = list(value.edges).filter((item) => ids.has(text(item.from)) && ids.has(text(item.to))).map((item) => ({ id: item.id || `${item.from}:${item.relation || "requires"}:${item.to}`, from: text(item.from), to: text(item.to), relation: item.relation || "requires" }));
  return { kind, rootId: text(value.rootId || value.root_id || nodes[0]?.id), nodes, edges };
}

export function buildProofTree(board) {
  const explicit = normalizeExplicitGraph(board?.proofTree || board?.proof_tree, "proof");
  if (explicit) return explicit;
  const rootId = `problem:${text(board?.id, "board")}`;
  const nodes = [node(rootId, "theorem", board?.problem?.statement || board?.problem?.goal || "研究目标", board?.status, "主研究问题", { logic: list(board?.routes).length > 1 ? "OR" : "AND", trust: "target" })];
  const edges = [];
  const routes = list(board?.routes);

  if (board?.agent === "rethlas") {
    const generationId = `${rootId}:generation`;
    const verificationId = `${rootId}:verification`;
    nodes.push(node(generationId, "route", "生成证明蓝图", list(board.blueprint).length ? "active" : "pending", "Generation Agent 负责构造证明步骤", { logic: "AND" }));
    nodes.push(node(verificationId, "verification", "独立验证", board?.verification?.verdict, "Verification Agent 独立检查蓝图"));
    edges.push(edge(rootId, generationId), edge(generationId, verificationId, "verified_by"));
    list(board.blueprint).forEach((step, index) => {
      const id = text(step?.id || `${generationId}:step:${index}`);
      nodes.push(node(id, "lemma", step?.statement || step?.title || step, step?.status, step?.summary || "证明蓝图步骤", { logic: step?.logic || "AND" }));
      edges.push(edge(generationId, id));
    });
  } else if (board?.agent === "danus") {
    list(board.facts).forEach((fact, index) => {
      const id = text(fact?.id || fact?.fact_id || `${rootId}:fact:${index}`);
      nodes.push(node(id, "fact", fact?.statement || fact?.title || fact, fact?.status || "verified", fact?.summary, { trust: "fact" }));
      edges.push(edge(rootId, id));
    });
  } else {
    routes.forEach((route, index) => {
      const routeId = text(route.id || `${rootId}:route:${index}`);
      nodes.push(node(routeId, "route", route.title || `研究路线 ${index + 1}`, route.status, route.summary, { logic: route.logic || "AND", meta: { humanStatus: route.humanStatus } }));
      edges.push(edge(rootId, routeId));
    });
    list(board?.goals).forEach((goal, index) => {
      const id = text(goal.id || goal.goal_id || `${rootId}:goal:${index}`);
      const parent = text(goal.routeId || goal.route_id || routes[0]?.id || rootId);
      nodes.push(node(id, "goal", goal.statement || goal.title || goal.label || `子目标 ${index + 1}`, goal.status, goal.summary, { logic: goal.logic || "AND" }));
      edges.push(edge(nodes.some((item) => item.id === parent) ? parent : rootId, id));
    });
    list(board?.claims).forEach((claim, index) => {
      const id = text(claim.id || claim.claim_id || claim.fact_id || `${rootId}:claim:${index}`);
      const kind = claim.kind === "fact" || claim.fact_id ? "fact" : claim.kind === "counterexample" ? "counterexample" : "candidate";
      const parent = text(claim.routeId || claim.origin_route_id || claim.goalId || routes[0]?.id || rootId);
      nodes.push(node(id, kind, claim.statement || claim.title || claim.label || `候选结论 ${index + 1}`, claim.status, claim.summary, { trust: kind === "fact" ? "fact" : "unverified" }));
      edges.push(edge(nodes.some((item) => item.id === parent) ? parent : rootId, id));
    });
  }
  return { kind: "proof", rootId, nodes, edges };
}

export function buildDependencyGraph(board) {
  const explicit = normalizeExplicitGraph(board?.dependencyGraph || board?.dependency_graph, "dependency");
  if (explicit) return explicit;
  const rootId = `problem:${text(board?.id, "board")}`;
  const nodes = [node(rootId, "theorem", board?.problem?.statement || board?.problem?.goal || "研究目标", board?.status, "主研究问题", { trust: "target" })];
  const edges = [];
  const addUnique = (item) => { if (!nodes.some((existing) => existing.id === item.id)) nodes.push(item); };
  const facts = [...list(board?.facts), ...list(board?.claims).filter((item) => item.kind === "fact" || item.fact_id)];
  const candidates = list(board?.claims).filter((item) => item.kind !== "fact" && !item.fact_id);
  const sources = list(board?.sources);
  const artifacts = list(board?.artifacts);

  sources.forEach((source, index) => addUnique(node(source.id || source.source_id || `source:${index}`, "source", source.title || source.citation || source.url || `来源 ${index + 1}`, source.status || "unverified", source.summary, { trust: source.verified ? "verified" : "unverified" })));
  artifacts.forEach((artifact, index) => addUnique(node(artifact.id || artifact.artifact_id || `artifact:${index}`, "artifact", artifact.filename || artifact.title || `产物 ${index + 1}`, artifact.status || "active", artifact.kind || artifact.media_type)));
  facts.forEach((fact, index) => addUnique(node(fact.id || fact.fact_id || `fact:${index}`, "fact", fact.statement || fact.title || `事实 ${index + 1}`, fact.status || "verified", fact.summary, { trust: "fact" })));
  candidates.forEach((claim, index) => addUnique(node(claim.id || claim.claim_id || `candidate:${index}`, claim.kind === "counterexample" ? "counterexample" : "candidate", claim.statement || claim.title || `候选结论 ${index + 1}`, claim.status, claim.summary)));

  const dependentItems = [...facts, ...candidates];
  dependentItems.forEach((item, index) => {
    const id = text(item.id || item.fact_id || item.claim_id || `${item.kind === "fact" ? "fact" : "candidate"}:${index}`);
    const dependencies = [...list(item.dependencyFactIds || item.dependency_fact_ids), ...list(item.sourceIds || item.source_ids), ...list(item.artifactIds || item.artifact_ids)];
    dependencies.forEach((dependencyId) => { if (nodes.some((entry) => entry.id === text(dependencyId))) edges.push(edge(text(dependencyId), id, "supports")); });
    edges.push(edge(id, rootId, item.kind === "counterexample" ? "contradicts" : "supports"));
  });

  if (board?.agent === "rethlas") {
    const blueprintId = `${rootId}:blueprint`;
    const verificationId = `${rootId}:verification`;
    addUnique(node(blueprintId, "artifact", "证明蓝图", list(board?.blueprint).length ? "active" : "pending", "Generation Agent 输出"));
    addUnique(node(verificationId, "verification", "验证报告", board?.verification?.verdict, "Verification Agent 输出"));
    edges.push(edge(blueprintId, verificationId, "verified_by"), edge(verificationId, rootId, "supports"));
  }
  return { kind: "dependency", rootId, nodes, edges };
}

export function graphSummary(model) {
  const counts = { total: model.nodes.length, active: 0, blocked: 0, verified: 0, unverified: 0, failed: 0 };
  model.nodes.forEach((item) => {
    if (item.status === "active") counts.active += 1;
    if (item.status === "blocked") counts.blocked += 1;
    if (item.status === "failed") counts.failed += 1;
    if (item.status === "verified" || item.trust === "fact" || item.trust === "verified") counts.verified += 1;
    else if (item.kind !== "theorem" && item.trust !== "target") counts.unverified += 1;
  });
  return counts;
}

export function visibleGraph(model, { filter = "all", query = "", collapsed = new Set() } = {}) {
  const hidden = new Set();
  if (model.kind === "proof" && collapsed.size) {
    const children = new Map();
    model.edges.forEach((item) => { if (!children.has(item.from)) children.set(item.from, []); children.get(item.from).push(item.to); });
    const hideChildren = (id) => { for (const child of children.get(id) || []) { if (!hidden.has(child)) { hidden.add(child); hideChildren(child); } } };
    collapsed.forEach(hideChildren);
  }
  const normalizedQuery = text(query).trim().toLocaleLowerCase("zh-CN");
  const matches = (item) => {
    if (hidden.has(item.id)) return false;
    if (normalizedQuery && !`${item.label} ${item.detail}`.toLocaleLowerCase("zh-CN").includes(normalizedQuery)) return false;
    if (filter === "active" && !["active", "blocked"].includes(item.status) && item.id !== model.rootId) return false;
    if (filter === "unverified" && item.trust !== "unverified" && !["pending", "active", "blocked"].includes(item.status) && item.id !== model.rootId) return false;
    if (filter === "problems" && !["blocked", "failed"].includes(item.status) && item.kind !== "counterexample" && item.id !== model.rootId) return false;
    return true;
  };
  const direct = new Set(model.nodes.filter(matches).map((item) => item.id));
  if (direct.size && (filter !== "all" || normalizedQuery)) {
    direct.add(model.rootId);
    const nextTowardRoot = new Map();
    model.edges.forEach((item) => {
      const from = model.kind === "proof" ? item.to : item.from;
      const to = model.kind === "proof" ? item.from : item.to;
      if (!nextTowardRoot.has(from)) nextTowardRoot.set(from, []);
      nextTowardRoot.get(from).push(to);
    });
    const queue = [...direct];
    while (queue.length) {
      for (const related of nextTowardRoot.get(queue.shift()) || []) {
        if (!hidden.has(related) && !direct.has(related)) { direct.add(related); queue.push(related); }
      }
    }
  }
  const nodes = model.nodes.filter((item) => !hidden.has(item.id) && (direct.size ? direct.has(item.id) : matches(item)));
  const ids = new Set(nodes.map((item) => item.id));
  return { ...model, nodes, edges: model.edges.filter((item) => ids.has(item.from) && ids.has(item.to)) };
}

export function layoutGraph(model) {
  const nodeWidth = 196;
  const nodeHeight = 76;
  const gapX = model.kind === "proof" ? 38 : 78;
  const gapY = 38;
  const rank = new Map();

  if (model.kind === "proof") {
    rank.set(model.rootId, 0);
    let changed = true;
    while (changed) {
      changed = false;
      model.edges.forEach((item) => {
        if (rank.has(item.from) && !rank.has(item.to)) { rank.set(item.to, rank.get(item.from) + 1); changed = true; }
      });
    }
  } else {
    const kindRank = { source: 0, artifact: 0, task: 0, verification: 1, fact: 2, candidate: 2, counterexample: 2, goal: 3, lemma: 3, theorem: 4 };
    model.nodes.forEach((item) => rank.set(item.id, item.id === model.rootId ? 4 : (kindRank[item.kind] ?? 2)));
  }

  const groups = new Map();
  model.nodes.forEach((item) => { const value = rank.get(item.id) ?? 1; if (!groups.has(value)) groups.set(value, []); groups.get(value).push(item); });
  const ranks = [...groups.keys()].sort((a, b) => a - b);
  const maxRows = Math.max(1, ...[...groups.values()].map((items) => items.length));
  const width = model.kind === "proof" ? Math.max(760, maxRows * (nodeWidth + gapX) + 70) : Math.max(760, ranks.length * (nodeWidth + gapX) + 70);
  const height = model.kind === "proof" ? Math.max(430, ranks.length * (nodeHeight + gapY) + 70) : Math.max(430, maxRows * (nodeHeight + gapY) + 70);
  const positions = new Map();
  ranks.forEach((rankValue, rankIndex) => {
    const items = groups.get(rankValue);
    if (model.kind === "proof") {
      const rowWidth = items.length * nodeWidth + Math.max(0, items.length - 1) * gapX;
      const startX = Math.max(35, (width - rowWidth) / 2);
      items.forEach((item, columnIndex) => positions.set(item.id, { x: startX + columnIndex * (nodeWidth + gapX), y: 35 + rankIndex * (nodeHeight + gapY), width: nodeWidth, height: nodeHeight }));
    } else {
      const columnHeight = items.length * nodeHeight + Math.max(0, items.length - 1) * gapY;
      const startY = Math.max(34, (height - columnHeight) / 2);
      items.forEach((item, rowIndex) => positions.set(item.id, { x: 35 + rankIndex * (nodeWidth + gapX), y: startY + rowIndex * (nodeHeight + gapY), width: nodeWidth, height: nodeHeight }));
    }
  });
  return { width, height, positions };
}

export const GRAPH_STATUS_LABELS = { pending: "待处理", active: "进行中", blocked: "被阻塞", verified: "已验证", failed: "已失败", paused: "已暂停", unverified: "未验证" };
export const GRAPH_KIND_LABELS = { theorem: "主目标", goal: "子目标", route: "研究路线", lemma: "引理", candidate: "候选结论", fact: "Fact", counterexample: "反例", source: "来源", artifact: "研究产物", task: "任务", verification: "验证" };
export const GRAPH_RELATION_LABELS = { requires: "需要", supports: "支持", derived_from: "推导自", verified_by: "验证", contradicts: "冲突", produced_by: "生成" };
