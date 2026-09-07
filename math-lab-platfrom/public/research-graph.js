import { STATUS_LABELS } from "./status-labels.js";
import { groupResearchRoutes } from "./route-presentation.js";
import { deriveResearchActivity } from "./research-activity.js";

const text = (value, fallback = "") => String(value ?? fallback);
const list = (value) => Array.isArray(value) ? value : [];

function statusOf(value, fallback = "pending") {
  const normalized = text(value, fallback).toLowerCase();
  const aliases = { created: "pending", proposed: "pending", active: "active", running: "active", complete: "verified", completed: "verified", accepted: "verified", approved: "verified", rejected: "failed", pruned: "failed", error: "failed", waiting: "blocked" };
  return aliases[normalized] || normalized;
}

function node(id, kind, label, status, detail = "", extra = {}) {
  const fullLabel = text(label, "未命名节点");
  return { id: text(id), kind, label: fullLabel, shortLabel: text(extra.shortLabel, conciseGraphLabel(fullLabel, kind)), status: statusOf(status), detail: text(detail), trust: extra.trust || "unverified", logic: extra.logic || null, sourceId: extra.sourceId || null, meta: extra.meta || {} };
}

export function conciseGraphLabel(value, kind = "") {
  let normalized = text(value, "未命名节点").replace(/\s+/g, " ").trim();
  if (kind === "route") {
    normalized = normalized
      .replace(/量词规格化/g, "明确量词与对象范围")
      .replace(/定义依赖闭包(?:恢复)?/g, "补齐命题所需定义")
      .replace(/可实例化性反压力审计/g, "检查能否用于具体对象与边界反例")
      .replace(/权威来源定位/g, "找到权威原文")
      .replace(/完整命题与定义闭包恢复/g, "还原完整命题和所需定义")
      .replace(/[—–]/g, " → ");
  }
  if (normalized.length <= (kind === "route" ? 50 : 34)) return normalized;
  const clause = normalized.split(/(?<=[。！？；])|(?=[，：])/u).map((item) => item.replace(/^[，：]\s*/, "").trim()).find((item) => item.length >= 8 && item.length <= 34);
  if (clause) return clause.replace(/[。！？；]$/, "");
  const prefix = { hypothesis: "路线约束", candidate: "候选结论", route: "研究路线", goal: "子目标", theorem: "主目标" }[kind];
  return prefix ? `${prefix} · ${normalized.slice(0, 22)}…` : `${normalized.slice(0, 30)}…`;
}

function edge(from, to, relation = "requires") {
  return { id: `${from}:${relation}:${to}`, from: text(from), to: text(to), relation };
}

export function researchGoalHeadline(value) {
  const raw = text(value, "研究目标");
  const headings = raw.split(/\r?\n/).map((line) => line.match(/^#{1,4}\s+(.+)$/)?.[1]?.trim()).filter(Boolean);
  const meaningful = headings.find((heading) => !/^(?:工作区中的完整数学问题|用户本轮要求|Statement|Notes for the solver|Known results|本轮白板验收要求)$/i.test(heading));
  if (meaningful) return meaningful;
  const first = raw.split(/\r?\n/).map((line) => line.replace(/^[-#>*\s]+/, "").trim()).find(Boolean);
  return first || "研究目标";
}

function normalizeExplicitGraph(value, kind, board) {
  if (!value || !Array.isArray(value.nodes)) return null;
  const contractWarnings = [];
  const allRoutes = [...list(board?.routes), ...list(board?.failedRoutes || board?.failed_routes)];
  const routeIdFor = (item) => {
    if (item.kind !== "route") return text(item.id);
    if (allRoutes.some((route) => text(route.id || route.route_id) === text(item.id))) return text(item.id);
    const routeClaim = list(board?.claims).find((claim) => text(claim.id || claim.claim_id) === text(item.id));
    return text(routeClaim?.routeId || routeClaim?.origin_route_id || item.id);
  };
  const nodes = value.nodes.map((item, index) => {
    const nodeKind = item.kind || "claim";
    const canonicalId = routeIdFor(item);
    if (text(item.id) && canonicalId !== text(item.id)) contractWarnings.push(`路线节点 ${text(item.id)} 已映射为 ${canonicalId}`);
    const linkedRoute = nodeKind === "route" ? allRoutes.find((route) => text(route.id || route.route_id) === canonicalId) : null;
    const historical = nodeKind === "route" && ["pruned", "failed", "rejected", "human_stopped", "cancelled"].includes(text(linkedRoute?.status || item.status));
    const nodeStatus = historical ? "archived" : linkedRoute ? effectiveRouteStatus(linkedRoute) : item.status;
    const trust = item.trust || (nodeKind === "fact" ? "fact" : ["verified", "accepted"].includes(String(nodeStatus).toLowerCase()) ? "verified" : undefined);
    const shortLabel = item.shortLabel || linkedRoute?.title;
    return node(canonicalId || `${kind}_${index}`, nodeKind, item.label || item.statement || item.title, nodeStatus, item.detail || item.summary, { ...item, sourceId: text(item.id), shortLabel, trust, meta: { ...item.meta, historical } });
  });
  const ids = new Set(nodes.map((item) => item.id));
  const canonicalIds = new Map(nodes.map((item) => [item.sourceId, item.id]));
  const edges = list(value.edges).map((item) => ({ id: item.id || `${item.from}:${item.relation || "requires"}:${item.to}`, from: canonicalIds.get(text(item.from)) || text(item.from), to: canonicalIds.get(text(item.to)) || text(item.to), relation: item.relation || "requires" })).filter((item) => ids.has(item.from) && ids.has(item.to));
  return { kind, source: "backend", contractWarnings, rootId: text(value.rootId || value.root_id || nodes[0]?.id), nodes, edges };
}

export function effectiveRouteStatus(route) {
  const humanStatus = text(route?.humanStatus || route?.human_review).toLowerCase();
  if (["pending", "proposed"].includes(humanStatus)) return "pending";
  return statusOf(route?.status);
}

export function buildProofTree(board) {
  const explicit = normalizeExplicitGraph(board?.proofTree || board?.proof_tree, "proof", board);
  if (explicit) return explicit;
  const currentRoutes = list(board?.routes);
  const routeKey = (route) => text(route.id || route.route_id);
  const historicalRoutes = list(board?.failedRoutes || board?.failed_routes).filter((route) => !currentRoutes.some((current) => routeKey(current) === routeKey(route)));
  const routes = [...currentRoutes, ...historicalRoutes];
  const historicalIds = new Set(routes.filter((route) => historicalRoutes.includes(route) || ["failed", "pruned", "rejected", "human_stopped", "cancelled"].includes(text(route.status))).map(routeKey));
  const goals = list(board?.goals);
  const researchActivity = board?.agent === "mathcat" ? deriveResearchActivity(board) : null;
  const backendRootId = text(board?.dependencyGraph?.rootId || board?.dependency_graph?.rootId || board?.dependency_graph?.root_id);
  const goalId = (goal) => text(goal?.id || goal?.goal_id);
  const goalParents = (goal) => list(goal?.parentGoalIds || goal?.parent_goal_ids).map((item) => text(item)).filter(Boolean);
  const topGoals = goals.filter((goal) => !goalParents(goal).length);
  const rootCandidates = topGoals.filter((goal) => !text(goal?.routeId || goal?.route_id));
  const mainGoal = rootCandidates.length === 1 ? rootCandidates[0] : null;
  const syntheticRoot = !mainGoal || topGoals.length > 1;
  const rootId = syntheticRoot ? `problem:${text(board?.id, "board")}` : goalId(mainGoal);
  const rootStatement = syntheticRoot
    ? (board?.problem?.statement || board?.problem?.goal || board?.problem?.original || "研究问题")
    : mainGoal.statement;
  const routeGroups = groupResearchRoutes(routes, board, { includeExpected: true });
  const plannedDirections = routeGroups.filter((group) => group.items.some(({ route }) => !["failed", "pruned", "rejected", "human_stopped"].includes(effectiveRouteStatus(route)))).length;
  const nodes = [node(rootId, "theorem", syntheticRoot ? `研究问题：${researchGoalHeadline(rootStatement)}` : researchGoalHeadline(rootStatement), mainGoal?.status || board?.status, text(board?.problem?.statement || board?.problem?.original || rootStatement), { logic: plannedDirections > 1 ? "PORTFOLIO" : null, trust: "target", meta: { activity: researchActivity ? { phase: researchActivity.phase, label: researchActivity.label, tone: researchActivity.tone, active: researchActivity.active } : null } })];
  const edges = [];

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
    goals.forEach((goal, index) => {
      const id = goalId(goal) || `${rootId}:goal:${index}`;
      if (id === rootId) return;
      const isTopTarget = !goalParents(goal).length;
      nodes.push(node(id, isTopTarget ? "theorem" : "goal", goal.statement || goal.title || goal.label || `子目标 ${index + 1}`, goal.status, goal.summary, { logic: goal.logic || (isTopTarget ? null : "AND"), trust: isTopTarget ? "target" : "unverified" }));
    });
    const goalNodeIds = new Set(nodes.filter((item) => ["theorem", "goal"].includes(item.kind)).map((item) => item.id));
    goals.forEach((goal) => {
      const id = goalId(goal);
      if (!id || id === rootId || !goalNodeIds.has(id)) return;
      if (text(goal.routeId || goal.route_id)) return;
      const parents = goalParents(goal).filter((parent) => goalNodeIds.has(parent) && parent !== id);
      (parents.length ? parents : [rootId]).forEach((parent) => edges.push(edge(parent, id, parents.length ? "requires" : "targets")));
    });

    const primaryGoalId = goalNodeIds.has(backendRootId)
      ? backendRootId
      : (mainGoal ? rootId : goalId(rootCandidates[0]) || rootId);
    routeGroups.forEach((group) => {
      const buckets = new Map();
      if (!group.items.length) buckets.set(primaryGoalId, []);
      group.items.forEach((item) => {
        const targets = list(item.route.targetGoalIds || item.route.target_goal_ids).map((value) => text(value)).filter((value) => goalNodeIds.has(value));
        const owner = targets[0] || primaryGoalId;
        if (!buckets.has(owner)) buckets.set(owner, []);
        buckets.get(owner).push(item);
      });
      buckets.forEach((items, owner) => {
        const groupId = `${owner}:lane:${group.kind}`;
        const statuses = items.map(({ route }) => historicalIds.has(routeKey(route)) ? "archived" : effectiveRouteStatus(route));
        const groupStatus = !items.length ? "unplanned" : statuses.includes("active") ? "active" : statuses.includes("blocked") ? "blocked" : statuses[0] || "pending";
        const laneActivities = items.filter(({ route }) => !historicalIds.has(routeKey(route))).map(({ route }) => researchActivity?.routeById?.[text(route.id || route.route_id)]).filter(Boolean);
        const laneActivity = laneActivities.find((item) => item.active) || laneActivities.find((item) => ["review", "waiting_others", "blocked", "paused"].includes(item.phase)) || laneActivities[0] || null;
        nodes.push(node(groupId, "strategy", group.label, groupStatus, items.length ? group.description : `${group.description} 当前尚无具体执行路线。`, { meta: { planned: statuses.some((status) => status !== "archived"), historical: statuses.length > 0 && statuses.every((status) => status === "archived"), approachKind: group.kind, targetGoalId: owner, activity: laneActivity ? { phase: laneActivity.phase, label: laneActivity.label, tone: laneActivity.tone, active: laneActivity.active } : null } }));
        edges.push(edge(goalNodeIds.has(owner) ? owner : rootId, groupId, "explores"));
        items.forEach(({ route, presentation }, index) => {
          const routeId = text(route.id || route.route_id || `${groupId}:route:${index}`);
          const historical = historicalIds.has(routeId);
          const routeActivity = historical ? null : researchActivity?.routeById?.[routeId] || null;
          nodes.push(node(routeId, "route", presentation.title, historical ? "archived" : effectiveRouteStatus(route), presentation.what, {
            shortLabel: presentation.title,
            meta: {
              historical,
              originalStatus: route.status,
              humanStatus: route.humanStatus,
              role: presentation.role,
              roleLabel: presentation.roleLabel,
              approachKind: presentation.kind,
              why: presentation.why,
              deliverable: presentation.deliverable,
              relation: presentation.relation,
              steps: presentation.steps,
              technicalTitle: presentation.technicalTitle,
              rawSummary: text(route.summary || route.method_summary),
              targetGoalIds: list(route.targetGoalIds || route.target_goal_ids),
              activity: routeActivity
            }
          }));
          const relation = { prerequisite: "prepares", auxiliary: "assists", adversarial: "tests", primary: "implements" }[presentation.role] || "implements";
          edges.push(edge(groupId, routeId, historical ? "historical_route" : relation));
          list(routeActivity?.tasks).filter((task) => task.active || ["blocked", "paused"].includes(task.phase)).slice(-4).forEach((task) => {
            const taskId = text(task.id || `${routeId}:task`);
            const taskStatus = task.active ? "active" : task.phase === "blocked" ? "blocked" : "pending";
            nodes.push(node(taskId, "task", task.objective || `${task.workerRole}任务`, taskStatus, `${task.workerRole}任务 · ${task.label}`, { shortLabel: conciseGraphLabel(task.objective || `${task.workerRole}任务`, "task"), meta: { activity: task, workerRole: task.workerRole, completionContract: task.completionContract } }));
            edges.push(edge(routeId, taskId, "runs"));
          });
          list(routeActivity?.verifications).filter((verification) => verification.active).slice(-3).forEach((verification) => {
            const verificationId = text(verification.id || `${routeId}:verification`);
            nodes.push(node(verificationId, "verification", verification.label, "active", `候选 ${verification.candidateId || "结论"} · ${verification.label}`, { shortLabel: verification.label, meta: { activity: verification, candidateId: verification.candidateId, caseId: verification.caseId } }));
            edges.push(edge(routeId, verificationId, "checks"));
          });
        });
      });
    });
    goals.forEach((goal) => {
      const id = goalId(goal);
      const routeId = text(goal.routeId || goal.route_id);
      if (id && routeId && nodes.some((item) => item.id === routeId)) edges.push(edge(routeId, id));
    });
    appendProofProvenance(board, nodes, edges, rootId);
  }
  return { kind: "proof", source: "route_projection", contractWarnings: [], rootId, nodes, edges };
}

function appendProofProvenance(board, nodes, edges, rootId) {
  const records = list(board?.claims).filter((claim) => claim.kind !== "hypothesis").map((claim, index) => ({ ...claim, id: text(claim.id || claim.claim_id || claim.fact_id || `${rootId}:claim:${index}`) }));
  const idOf = (claim) => text(claim.id || claim.claim_id || claim.fact_id);
  const isFact = (claim) => claim.kind === "fact" || Boolean(claim.fact_id);
  const byId = new Map(nodes.map((item) => [item.id, item]));
  const add = (item) => { if (!byId.has(item.id)) { nodes.push(item); byId.set(item.id, item); } return byId.get(item.id); };
  const connect = (parent, child, relation) => {
    edges.push(edge(parent, child, relation));
    byId.get(child).meta.layoutParentId = parent;
  };
  const unassignedId = `${rootId}:unassigned-results`;
  const unassigned = () => {
    if (!byId.has(unassignedId)) {
      add(node(unassignedId, "provenance", "来源未明确的研究记录", "pending", "仅收纳未能定位来源的记录，不表示它们是主目标的前提或证明步骤。"));
      edges.push(edge(rootId, unassignedId, "records"));
    }
    return unassignedId;
  };
  const routeParent = (claim) => {
    const routeId = text(claim.routeId || claim.origin_route_id);
    if (!routeId) return unassigned();
    if (!byId.has(routeId)) {
      add(node(routeId, "route", "来源路线（记录缺失）", "archived", `原记录指定来源路线 ${routeId}，但当前快照不含该路线详情。`, { meta: { historical: true, missingSource: true } }));
      edges.push(edge(rootId, routeId, "records"));
    }
    return routeId;
  };
  records.forEach((claim, index) => {
    const id = idOf(claim) || `${rootId}:claim:${index}`;
    add(node(id, isFact(claim) ? "fact" : claim.kind === "counterexample" ? "counterexample" : "candidate", claim.statement || claim.title || claim.label, claim.status, claim.summary, {
      trust: isFact(claim) ? "fact" : "unverified",
      meta: { originRouteId: text(claim.routeId || claim.origin_route_id), provenanceNote: "成果来源关系，不等同于证明依赖。" }
    }));
    if (!isFact(claim)) connect(routeParent(claim), id, "produces");
  });
  const verificationById = new Map();
  for (const verification of list(board?.verificationQueue)) {
    const id = text(verification.verification_id || verification.id);
    const candidateId = text(verification.candidate_id || verification.candidateId);
    if (!id || id === candidateId || !byId.has(candidateId) || !["candidate", "counterexample"].includes(byId.get(candidateId).kind)) continue;
    const item = add(node(id, "verification", `验证：${byId.get(candidateId).shortLabel}`, verification.status, "独立验证记录；通过验证不自动意味着主目标已解决。", { meta: { candidateId } }));
    item.meta.candidateId = candidateId;
    connect(candidateId, id, "submitted_for");
    verificationById.set(id, candidateId);
  }
  for (const fact of records.filter(isFact)) {
    const id = idOf(fact);
    if (!byId.has(id)) continue;
    const verificationId = text(fact.verification_id || fact.verificationId);
    const candidates = records.filter((claim) => !isFact(claim) && verificationId && text(claim.verification_id || claim.verificationId) === verificationId);
    // Only exact identifiers establish provenance; never infer it from similar prose.
    const candidateId = verificationById.get(verificationId) || (candidates.length === 1 ? idOf(candidates[0]) : "");
    if (candidateId && byId.has(candidateId)) {
      if (!verificationById.has(verificationId)) {
        add(node(verificationId, "verification", "验证记录（详情未提供）", "pending", `候选与 Fact 明确引用同一验证编号：${verificationId}`, { meta: { candidateId } }));
        connect(candidateId, verificationId, "submitted_for");
        verificationById.set(verificationId, candidateId);
      }
      connect(verificationId, id, "accepted_as");
      byId.get(id).meta.sourceCandidateId = candidateId;
      byId.get(id).meta.verificationId = verificationId;
      byId.get(id).meta.provenanceNote = `由候选「${byId.get(candidateId).shortLabel}」经对应的独立验证形成；这不表示主目标已获证明。`;
    } else connect(routeParent(fact), id, "produces");
  }
}

export function buildDependencyGraph(board) {
  const explicit = normalizeExplicitGraph(board?.dependencyGraph || board?.dependency_graph, "dependency", board);
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
  return { kind: "dependency", source: "derived", contractWarnings: [], rootId, nodes, edges };
}

export function graphSummary(model) {
  const counts = { total: model.nodes.length, active: 0, blocked: 0, verified: 0, unverified: 0, unplanned: 0, failed: 0 };
  model.nodes.forEach((item) => {
    if (item.status === "active") counts.active += 1;
    if (item.status === "blocked") counts.blocked += 1;
    if (item.status === "unplanned") counts.unplanned += 1;
    if (item.status === "failed") counts.failed += 1;
    if (item.status === "verified" || item.trust === "fact" || item.trust === "verified") counts.verified += 1;
    else if (item.status !== "unplanned" && item.kind !== "theorem" && item.trust !== "target") counts.unverified += 1;
  });
  return counts;
}

export function visibleGraph(model, { filter = "all", query = "", collapsed = new Set() } = {}) {
  const hidden = new Set();
  if (model.kind === "proof" && collapsed.size) {
    const graph = layoutAdjacency(model);
    const { roots } = proofForest(graph, model.rootId);
    const reachable = new Set(roots);
    const queue = [...roots];
    for (let index = 0; index < queue.length; index += 1) {
      const id = queue[index];
      if (collapsed.has(id)) continue;
      for (const item of graph.outgoing.get(id)) {
        if (graph.preferredParents.has(item.to) && graph.preferredParents.get(item.to) !== id) continue;
        if (reachable.has(item.to)) continue;
        reachable.add(item.to);
        queue.push(item.to);
      }
    }
    for (const id of graph.ids) if (!reachable.has(id)) hidden.add(id);
  }
  const normalizedQuery = text(query).trim().toLocaleLowerCase("zh-CN");
  const matches = (item) => {
    if (hidden.has(item.id)) return false;
    if (normalizedQuery && !`${item.label} ${item.detail}`.toLocaleLowerCase("zh-CN").includes(normalizedQuery)) return false;
    if (filter === "active" && !["active", "blocked"].includes(item.status) && item.id !== model.rootId) return false;
    if (filter === "unverified" && (item.status === "unplanned" || (item.trust !== "unverified" && !["pending", "active", "blocked"].includes(item.status))) && item.id !== model.rootId) return false;
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
  return { ...model, nodes, edges: model.edges.filter((item) => ids.has(item.from) && ids.has(item.to) && !(model.kind === "proof" && collapsed.has(item.from))) };
}

export function layoutGraph(model) {
  const nodeWidth = 252;
  const nodeHeight = 108;
  const gapY = 72;
  const graph = layoutAdjacency(model);
  return model.kind === "proof"
    ? layoutProofForest(graph, model.rootId, { nodeWidth, nodeHeight, gapX: 52, gapY })
    : layoutDependencyLayers(graph, { nodeWidth, nodeHeight, gapX: 96, gapY });
}

function layoutAdjacency(model) {
  const byId = new Map(model.nodes.map((item) => [item.id, item]));
  const kindOrder = { theorem: 0, goal: 1, strategy: 2, route: 3, lemma: 4, hypothesis: 5, task: 6, verification: 7, candidate: 8, counterexample: 9, fact: 10, source: 11, artifact: 12 };
  const directionOrder = { direct_proof: 0, counterexample: 1, computation: 2, reduction: 3 };
  const compareText = (left, right) => left < right ? -1 : left > right ? 1 : 0;
  const compareIds = (left, right) => {
    const a = byId.get(left);
    const b = byId.get(right);
    return ((kindOrder[a.kind] ?? 20) - (kindOrder[b.kind] ?? 20))
      || ((directionOrder[a.meta?.approachKind] ?? 4) - (directionOrder[b.meta?.approachKind] ?? 4))
      || compareText(left, right);
  };
  const ids = [...byId.keys()].sort(compareIds);
  const outgoing = new Map(ids.map((id) => [id, []]));
  const incoming = new Map(ids.map((id) => [id, []]));
  for (const item of model.edges) {
    if (!byId.has(item.from) || !byId.has(item.to)) continue;
    const normalized = { ...item, id: item.id || `${item.from}:${item.relation || "requires"}:${item.to}` };
    outgoing.get(item.from).push(normalized);
    incoming.get(item.to).push(normalized);
  }
  for (const edges of outgoing.values()) edges.sort((a, b) => compareIds(a.to, b.to) || compareText(a.id, b.id));
  for (const edges of incoming.values()) edges.sort((a, b) => compareIds(a.from, b.from) || compareText(a.id, b.id));
  const preferredParents = new Map([...byId].filter(([, item]) => byId.has(item.meta?.layoutParentId)).map(([id, item]) => [id, item.meta.layoutParentId]));
  return { ids, outgoing, incoming, compareIds, preferredParents };
}

function proofForest({ ids, outgoing, incoming, preferredParents }, rootId) {
  const children = new Map(ids.map((id) => [id, []]));
  const parentById = new Map();
  const treeEdgeIds = new Set();
  const rank = new Map();
  const roots = [];
  const traversal = [];
  // Each node has one layout parent. Additional dependencies and back edges stay
  // in the model and are drawn separately rather than folding branches together.
  const starts = [rootId, ...ids.filter((id) => !incoming.get(id).length), ...ids];
  for (const start of starts) {
    if (!outgoing.has(start) || rank.has(start)) continue;
    roots.push(start);
    rank.set(start, 0);
    const queue = [start];
    for (let index = 0; index < queue.length; index += 1) {
      const parent = queue[index];
      traversal.push(parent);
      for (const item of outgoing.get(parent)) {
        if (preferredParents.has(item.to) && preferredParents.get(item.to) !== parent) continue;
        if (rank.has(item.to)) continue;
        rank.set(item.to, rank.get(parent) + 1);
        parentById.set(item.to, parent);
        treeEdgeIds.add(item.id);
        children.get(parent).push(item.to);
        queue.push(item.to);
      }
    }
  }
  return { children, parentById, treeEdgeIds, rank, roots, traversal };
}

function layoutProofForest(graph, rootId, { nodeWidth, nodeHeight, gapX, gapY }) {
  const { children, parentById, treeEdgeIds, rank, roots, traversal } = proofForest(graph, rootId);
  const spans = new Map();
  for (let index = traversal.length - 1; index >= 0; index -= 1) {
    const id = traversal[index];
    const descendants = children.get(id);
    spans.set(id, descendants.length
      ? descendants.reduce((sum, child) => sum + spans.get(child), 0) + (descendants.length - 1) * gapX
      : nodeWidth);
  }
  const forestGap = gapX * 2;
  const contentWidth = roots.reduce((sum, id) => sum + spans.get(id), 0) + Math.max(0, roots.length - 1) * forestGap;
  const width = Math.max(760, contentWidth + 140);
  const maxRank = traversal.reduce((value, id) => Math.max(value, rank.get(id)), 0);
  const height = Math.max(430, 88 + nodeHeight + maxRank * (nodeHeight + gapY));
  const positions = new Map();
  const leftById = new Map();
  let forestLeft = (width - contentWidth) / 2;
  for (const id of roots) {
    leftById.set(id, forestLeft);
    forestLeft += spans.get(id) + forestGap;
  }
  // Reserve a contiguous horizontal lane for every complete subtree. Layout is
  // independent of backend array order and uses no recursion for deep proofs.
  for (const id of traversal) {
    let left = leftById.get(id);
    positions.set(id, { x: left + (spans.get(id) - nodeWidth) / 2, y: 44 + rank.get(id) * (nodeHeight + gapY), width: nodeWidth, height: nodeHeight });
    for (const child of children.get(id)) {
      leftById.set(child, left);
      left += spans.get(child) + gapX;
    }
  }
  return { width, height, positions, parentById, treeEdgeIds };
}

function layoutDependencyLayers(graph, { nodeWidth, nodeHeight, gapX, gapY }) {
  const { ids, outgoing, incoming, compareIds } = graph;
  const seen = new Set();
  const finishOrder = [];
  // Collapse strongly connected components before ranking. Cyclic references
  // remain visible without an unbounded rank walk or invented dependency order.
  for (const start of ids) {
    if (seen.has(start)) continue;
    seen.add(start);
    const stack = [{ id: start, index: 0 }];
    while (stack.length) {
      const current = stack[stack.length - 1];
      const edges = outgoing.get(current.id);
      if (current.index < edges.length) {
        const child = edges[current.index++].to;
        if (!seen.has(child)) { seen.add(child); stack.push({ id: child, index: 0 }); }
      } else {
        finishOrder.push(current.id);
        stack.pop();
      }
    }
  }
  const componentById = new Map();
  const components = [];
  for (let index = finishOrder.length - 1; index >= 0; index -= 1) {
    const start = finishOrder[index];
    if (componentById.has(start)) continue;
    const component = { members: [], incoming: new Set(), outgoing: new Set(), rank: 0 };
    const componentId = components.length;
    components.push(component);
    componentById.set(start, componentId);
    const stack = [start];
    while (stack.length) {
      const id = stack.pop();
      component.members.push(id);
      for (const item of incoming.get(id)) {
        if (componentById.has(item.from)) continue;
        componentById.set(item.from, componentId);
        stack.push(item.from);
      }
    }
    component.members.sort(compareIds);
  }
  for (const id of ids) {
    const from = componentById.get(id);
    for (const item of outgoing.get(id)) {
      const to = componentById.get(item.to);
      if (from === to) continue;
      components[from].outgoing.add(to);
      components[to].incoming.add(from);
    }
  }
  const compareComponents = (a, b) => compareIds(components[a].members[0], components[b].members[0]);
  const degrees = components.map((item) => item.incoming.size);
  const queue = components.map((_, index) => index).filter((index) => !degrees[index]).sort(compareComponents);
  for (let index = 0; index < queue.length; index += 1) {
    const current = components[queue[index]];
    for (const target of current.outgoing) {
      components[target].rank = Math.max(components[target].rank, current.rank + 1);
      degrees[target] -= 1;
      if (!degrees[target]) queue.push(target);
    }
  }
  const layers = [];
  components.forEach((item, index) => { (layers[item.rank] ||= []).push(index); });
  layers.forEach((layer) => layer.sort(compareComponents));
  const centers = new Map();
  const updateCenters = (layer) => {
    const count = layer.reduce((sum, index) => sum + components[index].members.length, 0);
    let offset = 0;
    for (const index of layer) {
      const size = components[index].members.length;
      centers.set(index, (offset + size / 2) / count);
      offset += size;
    }
  };
  layers.forEach(updateCenters);
  // Neighbor ordering reduces crossings without reinterpreting an edge based
  // on node kind. Same-kind claims can occupy different dependency levels.
  for (let sweep = 0; sweep < 4; sweep += 1) {
    for (const forward of [true, false]) {
      const order = forward ? layers : [...layers].reverse();
      for (const layer of order) {
        const scores = new Map(layer.map((index) => {
          const neighbors = components[index][forward ? "incoming" : "outgoing"];
          const score = neighbors.size ? [...neighbors].reduce((sum, other) => sum + centers.get(other), 0) / neighbors.size : centers.get(index);
          return [index, score];
        }));
        layer.sort((a, b) => scores.get(a) - scores.get(b) || compareComponents(a, b));
        updateCenters(layer);
      }
    }
  }
  const rows = layers.map((layer) => layer.flatMap((index) => components[index].members));
  const maxRows = rows.reduce((value, layer) => Math.max(value, layer.length), 0);
  const width = Math.max(760, 140 + layers.length * nodeWidth + Math.max(0, layers.length - 1) * gapX);
  const height = Math.max(430, 88 + maxRows * nodeHeight + Math.max(0, maxRows - 1) * gapY);
  const positions = new Map();
  rows.forEach((items, column) => {
    const columnHeight = items.length * nodeHeight + Math.max(0, items.length - 1) * gapY;
    const top = (height - columnHeight) / 2;
    items.forEach((id, row) => positions.set(id, { x: 70 + column * (nodeWidth + gapX), y: top + row * (nodeHeight + gapY), width: nodeWidth, height: nodeHeight }));
  });
  return { width, height, positions, componentById };
}

export function overrideGraphPositions(layout, manualPositions = new Map()) {
  const positions = new Map();
  let width = layout.width;
  let height = layout.height;
  layout.positions.forEach((position, id) => {
    const manual = manualPositions.get(id);
    const next = manual ? { ...position, x: Math.max(18, Number(manual.x) || 18), y: Math.max(18, Number(manual.y) || 18) } : { ...position };
    positions.set(id, next);
    width = Math.max(width, next.x + next.width + 36);
    height = Math.max(height, next.y + next.height + 36);
  });
  return { ...layout, width, height, positions };
}

export function relatedGraphIds(model, selectedId) {
  const related = new Set();
  if (!selectedId || !model.nodes.some((item) => item.id === selectedId)) return related;
  related.add(selectedId);
  const outgoing = new Map();
  const incoming = new Map();
  model.edges.forEach((item) => {
    if (!outgoing.has(item.from)) outgoing.set(item.from, []);
    if (!incoming.has(item.to)) incoming.set(item.to, []);
    outgoing.get(item.from).push(item.to);
    incoming.get(item.to).push(item.from);
  });
  const expand = (start, adjacency) => {
    const queue = [start];
    const visited = new Set(queue);
    for (let index = 0; index < queue.length; index += 1) {
      for (const id of adjacency.get(queue[index]) || []) {
        if (visited.has(id)) continue;
        visited.add(id);
        related.add(id);
        queue.push(id);
      }
    }
  };
  expand(selectedId, outgoing);
  expand(selectedId, incoming);
  return related;
}

export function focusGraphBranch(model, selectedId) {
  const related = relatedGraphIds(model, selectedId);
  if (!related.size) return model;
  const nodes = model.nodes.filter((item) => related.has(item.id));
  const ids = new Set(nodes.map((item) => item.id));
  return { ...model, nodes, edges: model.edges.filter((item) => ids.has(item.from) && ids.has(item.to)) };
}

export function fitGraphScale(layout, viewportWidth, viewportHeight, { min = 0.35, max = 1.25, padding = 28 } = {}) {
  if (!layout?.width || !layout?.height || viewportWidth <= 0 || viewportHeight <= 0) return 1;
  const availableWidth = Math.max(1, viewportWidth - padding * 2);
  const availableHeight = Math.max(1, viewportHeight - padding * 2);
  return Math.max(min, Math.min(max, Math.min(availableWidth / layout.width, availableHeight / layout.height)));
}

export const GRAPH_STATUS_LABELS = { ...STATUS_LABELS, archived: "已归档", promoted_to_fact: "已转为 Fact" };
export const GRAPH_KIND_LABELS = { theorem: "主目标", strategy: "研究方向", goal: "子目标", route: "具体路线", lemma: "引理", hypothesis: "路线约束", candidate: "候选结论", fact: "Fact", counterexample: "反例", source: "来源", artifact: "研究产物", task: "任务", verification: "验证", provenance: "记录分组" };
export const GRAPH_RELATION_LABELS = { requires: "需要", supports: "支持", derived_from: "推导自", verified_by: "验证", contradicts: "冲突", produced_by: "生成", targets: "指向", explores: "研究", implements: "采用", tests: "检验", assists: "辅助", prepares: "准备", runs: "执行任务", checks: "独立验证", produces: "产出记录", submitted_for: "送交验证", accepted_as: "形成 Fact", historical_route: "历史路线", records: "收录记录" };
