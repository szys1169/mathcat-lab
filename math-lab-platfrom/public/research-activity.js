const list = (value) => Array.isArray(value) ? value : [];
const text = (value, fallback = "") => String(value ?? fallback);

export const RESEARCH_STAGES = [
  { id: "strategy", label: "梳理目标" },
  { id: "routes", label: "形成路线" },
  { id: "review", label: "审查路线" },
  { id: "dispatch", label: "安排任务" },
  { id: "execute", label: "执行研究" },
  { id: "verify", label: "独立验证" }
];

const PLANNING_STAGES = {
  strategy_director: { index: 0, label: "正在梳理总目标与关键卡点", short: "梳理目标" },
  strategy_director_fallback: { index: 0, label: "正在保守整理研究目标", short: "整理目标" },
  route_generator: { index: 1, label: "正在提出可执行的研究路线", short: "生成路线" },
  reflection: { index: 2, label: "正在检查路线漏洞、重复与条件", short: "审查路线" },
  proximity: { index: 2, label: "正在比较并合并相近路线", short: "比较路线" },
  ranking: { index: 2, label: "正在比较路线优先级", short: "路线排序" },
  supervisor: { index: 3, label: "正在拆分任务并安排执行者", short: "安排任务" },
  deterministic_continuity: { index: 3, label: "主规划暂不可用，正在生成保守续跑方案", short: "保守续跑" }
};

const TASK_TERMINAL_FAILURE = new Set(["rejected", "failed", "dead_lettered"]);
const ROUTE_TERMINAL = new Set(["completed", "refuted", "failed", "merged", "pruned", "human_stopped", "superseded"]);
const PROJECT_TERMINAL = new Set(["success", "partial_success", "refuted", "environment_failed", "stopped_by_human", "error"]);
const VERIFICATION_ACTIVE = new Set(["submitted", "prechecking", "verifying"]);

export function researchEventType(event) {
  return text(event?.kind || event?.type || event?.event_type).trim();
}

function eventData(event) {
  return event?.data && typeof event.data === "object" && !Array.isArray(event.data) ? event.data : {};
}

function entityOf(event) {
  const value = event?.entity;
  return value && typeof value === "object" ? value : {};
}

function causedByOf(event) {
  const value = event?.causedBy || event?.caused_by;
  return value && typeof value === "object" ? value : {};
}

function eventTime(event) {
  const value = Date.parse(text(event?.at || event?.occurred_at));
  return Number.isFinite(value) ? value : 0;
}

function normalizeEvents(board) {
  return list(board?.events).map((event, index) => ({
    ...event,
    _index: index,
    _type: researchEventType(event),
    _data: eventData(event),
    _entity: entityOf(event),
    _causedBy: causedByOf(event),
    _time: eventTime(event),
    _cursor: Number(event?.cursor ?? index)
  })).sort((left, right) => left._cursor - right._cursor || left._time - right._time || left._index - right._index);
}

function idOf(value, ...keys) {
  for (const key of keys) {
    const result = text(value?.[key]).trim();
    if (result) return result;
  }
  return "";
}

function latestRelatedEvent(events, kind, id) {
  if (!id) return null;
  return [...events].reverse().find((event) =>
    (event._entity.kind === kind && text(event._entity.id) === id)
    || (event._causedBy.kind === kind && text(event._causedBy.id) === id)
    || text(event._data[`${kind}_id`]) === id
  ) || null;
}

function workerRoleLabel(role) {
  return {
    prover: "证明",
    explorer: "数学探索",
    counterexample_hunter: "反例搜索",
    literature_researcher: "文献核验"
  }[text(role).toLowerCase()] || text(role, "研究");
}

function taskPhase(task, worker) {
  const status = text(task?.status).toLowerCase();
  const workerStatus = text(worker?.status).toLowerCase();
  if (TASK_TERMINAL_FAILURE.has(status)) return { phase: "failed", label: status === "dead_lettered" ? "多次尝试失败" : "任务失败", tone: "error", active: false };
  if (["cancelled", "human_stopped"].includes(status)) return { phase: "stopped", label: "任务已停止", tone: "muted", active: false };
  if (status === "completed") return { phase: "completed", label: "任务已完成", tone: "success", active: false };
  if (status === "blocked") return { phase: "blocked", label: "任务遇到阻碍", tone: "warning", active: false };
  if (status === "paused") return { phase: "paused", label: "任务已暂停", tone: "waiting", active: false };
  if (["startup_failed", "unhealthy", "quarantined", "failed", "offline"].includes(workerStatus)) return { phase: "failed", label: "执行环境异常", tone: "error", active: false };
  if (status === "ingesting" || status === "result_submitted" || ["result_submitted", "draining"].includes(workerStatus)) return { phase: "ingesting", label: "正在接收并整理结果", tone: "active", active: true };
  if (status === "checkpointed" || workerStatus === "checkpointing") return { phase: "executing", label: "已保存进度，继续研究", tone: "active", active: true };
  if (status === "running" || workerStatus === "running") return { phase: "executing", label: "正在执行研究任务", tone: "active", active: true };
  if (["offered", "leased"].includes(status) || ["spawn_requested", "starting", "handshaking", "ready", "lease_accepted"].includes(workerStatus)) return { phase: "starting", label: "正在启动并加载任务", tone: "active", active: true };
  if (["open", "queued", "assigned"].includes(status)) return { phase: "queued", label: "任务已排队", tone: "waiting", active: true };
  return { phase: "idle", label: status ? `任务状态：${status}` : "等待任务状态", tone: "muted", active: false };
}

function verificationStageLabel(stage) {
  return {
    intake: "正在接收候选结论",
    snapshotting: "正在冻结验证所需输入",
    precheck: "正在做确定性预检查",
    review: "独立审阅者正在检查论证",
    formalization: "正在转换为形式化陈述",
    proof_search: "正在进行形式证明搜索",
    adjudication: "正在综合检查结果",
    packaging: "正在整理验证证据包",
    replay: "正在独立重放验证",
    commit_ready: "验证完成，正在进入 Fact Gate",
    committed: "已通过 Fact Gate",
    rejected: "未通过独立验证",
    unknown: "验证结果尚不确定",
    failed: "验证过程失败",
    cancelled: "验证已取消"
  }[text(stage).toLowerCase()] || "正在独立验证";
}

function verificationActivity(verification, claims, events) {
  const id = idOf(verification, "verification_id", "id");
  const candidateId = idOf(verification, "candidate_id", "candidateId");
  const claim = claims.find((item) => idOf(item, "id", "claim_id") === candidateId);
  const routeId = idOf(claim, "routeId", "origin_route_id", "route_id");
  const caseCreated = [...events].reverse().find((event) => event._type === "verification.case.created" && (text(event._data.verification_id) === id || text(event._data.candidate_id) === candidateId));
  const caseId = text(caseCreated?._entity?.id || caseCreated?._data?.case_id);
  const caseEvents = caseId ? events.filter((event) => text(event._data.case_id) === caseId || (event._entity.kind === "verification_case" && text(event._entity.id) === caseId)) : [];
  const transition = [...caseEvents].reverse().find((event) => event._type === "verification.case.transitioned");
  const terminalCase = [...caseEvents].reverse().find((event) => event._type === "verification.case.completed");
  const status = text(verification?.status).toLowerCase();
  const stage = text(terminalCase?._data?.stage || transition?._data?.to || (status === "verifying" ? "review" : status));
  const active = VERIFICATION_ACTIVE.has(status);
  const latest = [...caseEvents, latestRelatedEvent(events, "verification", id)].filter(Boolean).sort((left, right) => left._cursor - right._cursor).at(-1) || null;
  const tone = ["accepted", "promoted_to_fact", "committed"].includes(status) || stage === "committed" ? "success" : ["rejected", "failed"].includes(status) ? "error" : status === "unknown" ? "warning" : active ? "active" : "muted";
  return { id, candidateId, routeId, caseId, status, stage, label: verificationStageLabel(stage), tone, active, at: latest?.at || latest?.occurred_at || null };
}

function currentPlanning(events, currentRound, projectStatus, now) {
  const starts = events.filter((event) => event._type === "round.started" && (!currentRound || Number(event._data.number) === currentRound));
  const roundStart = starts.at(-1) || events.filter((event) => event._type === "round.started").at(-1) || null;
  const roundId = text(roundStart?._entity?.id);
  const roundEvents = roundStart ? events.slice(events.indexOf(roundStart)) : events;
  const completedRound = [...roundEvents].reverse().find((event) => event._type === "round.completed" && (!roundId || text(event._entity.id) === roundId));
  const committed = [...roundEvents].reverse().find((event) => event._type === "planning.revision.committed");
  const attempts = roundEvents.filter((event) => event._type === "planning.stage.attempt.started");
  const openAttempt = [...attempts].reverse().find((attempt) => {
    const stage = text(attempt._data.stage);
    const later = roundEvents.slice(roundEvents.indexOf(attempt) + 1);
    return !later.some((event) =>
      (event._type === "planning.stage.completed" && text(event._data.stage) === stage)
      || event._type === "planning.revision.committed"
      || event._type === "round.completed"
      || (event._type === "planning.stage.attempt.started" && text(event._data.stage) !== stage)
    );
  });
  if (openAttempt && projectStatus === "running") {
    const stageId = text(openAttempt._data.stage);
    const definition = PLANNING_STAGES[stageId] || { index: 1, label: "正在生成研究计划", short: "研究规划" };
    const hardSeconds = Number(openAttempt._data.hard_timeout_seconds || 0);
    const elapsedSeconds = openAttempt._time ? Math.max(0, Math.floor((now - openAttempt._time) / 1000)) : 0;
    const softExceeded = roundEvents.some((event) => event._type === "planning.stage.soft_budget_exceeded" && text(event._data.stage) === stageId && event._cursor >= openAttempt._cursor);
    const stale = Boolean(hardSeconds && elapsedSeconds > hardSeconds + 60);
    return { active: !stale, stale, stageId, stageIndex: definition.index, label: stale ? `${definition.short}状态长时间未更新` : definition.label, short: stale ? "阶段未更新" : definition.short, detail: stale ? "后端没有在预期时间内报告该阶段完成，请检查 MathCat 运行状态。" : softExceeded ? "这一阶段比预期更久，但仍在允许的硬时限内运行。" : "MathCat 正在执行可审计的规划阶段。", sinceAt: openAttempt.at || openAttempt.occurred_at || null, elapsedSeconds, roundId, roundStart, completedRound, committed };
  }
  if (!completedRound && !committed && projectStatus === "running" && roundStart) {
    const lastCompleted = [...roundEvents].reverse().find((event) => event._type === "planning.stage.completed");
    if (lastCompleted) {
      const stageId = text(lastCompleted._data.stage);
      const index = Math.min(3, (PLANNING_STAGES[stageId]?.index ?? 0) + 1);
      const next = RESEARCH_STAGES[index];
      return { active: true, stale: false, stageId: next.id, stageIndex: index, label: `正在准备${next.label}`, short: next.label, detail: "上一规划阶段已经完成，等待下一阶段开始。", sinceAt: lastCompleted.at || lastCompleted.occurred_at || null, elapsedSeconds: 0, roundId, roundStart, completedRound, committed };
    }
    return { active: true, stale: false, stageId: "intake", stageIndex: 0, label: "正在汇总本轮新信息", short: "汇总变化", detail: "MathCat 正在收集新事实、失败记录和人工操作。", sinceAt: roundStart.at || roundStart.occurred_at || null, elapsedSeconds: 0, roundId, roundStart, completedRound, committed };
  }
  return { active: false, stale: false, stageId: "", stageIndex: committed ? 3 : -1, label: "", short: "", detail: "", sinceAt: null, elapsedSeconds: 0, roundId, roundStart, completedRound, committed };
}

function stageBarState({ phase, planning, projectStatus, pendingRoutes = 0, pendingQuestions = 0 }) {
  let activeIndex = -1;
  let completedThrough = -1;
  let waitingIndex = -1;
  if (phase === "planning") { activeIndex = planning.stageIndex; completedThrough = activeIndex - 1; }
  else if (phase === "review") { completedThrough = 1; waitingIndex = 2; }
  else if (phase === "dispatch") { completedThrough = 2; waitingIndex = 3; }
  else if (["queued", "starting", "executing", "ingesting"].includes(phase)) { completedThrough = 3; activeIndex = 4; }
  else if (phase === "verifying") { completedThrough = 4; activeIndex = 5; }
  else if (phase === "terminal" && ["success", "refuted"].includes(projectStatus)) completedThrough = 5;
  else if (phase === "idle" && projectStatus === "running") completedThrough = 3;
  const stages = RESEARCH_STAGES.map((step, index) => ({ ...step, status: index === activeIndex ? "active" : index === waitingIndex ? "waiting" : index <= completedThrough ? "complete" : "upcoming" }));
  if ((pendingRoutes || pendingQuestions) && activeIndex >= 4) stages[2].status = "waiting";
  return stages;
}

export function deriveResearchActivity(board, { now = Date.now() } = {}) {
  const events = normalizeEvents(board);
  const routes = list(board?.routes);
  const tasks = list(board?.tasks);
  const workers = list(board?.workers);
  const claims = list(board?.claims);
  const verifications = list(board?.verificationQueue || board?.verification_queue).map((item) => verificationActivity(item, claims, events));
  const projectStatus = text(board?.status, "created").toLowerCase();
  const currentRound = Number(board?.summary?.current_round ?? board?.summary?.currentRound ?? 0);
  const planning = currentPlanning(events, currentRound, projectStatus, Number(now));
  const pendingRoutes = routes.filter((route) => !route?.isProposal && ["pending", "proposed"].includes(text(route?.humanStatus || route?.human_review).toLowerCase()));
  const pendingQuestions = list(board?.decisions).filter((item) => item?.status === "pending");

  const taskActivities = tasks.map((task) => {
    const taskId = idOf(task, "task_id", "id");
    const routeId = idOf(task, "route_id", "routeId");
    const workerId = idOf(task, "worker_id", "workerId");
    const worker = workers.find((item) => idOf(item, "worker_id", "id") === workerId || idOf(item, "current_task_id", "currentTaskId") === taskId);
    const phase = taskPhase(task, worker);
    const latest = latestRelatedEvent(events, "task", taskId);
    return { ...phase, id: taskId, routeId, workerId, workerRole: workerRoleLabel(task?.worker_role || worker?.role), objective: text(task?.objective, "未命名研究任务"), completionContract: text(task?.completion_contract), resultSummary: text(task?.result_summary), status: text(task?.status).toLowerCase(), at: latest?.at || latest?.occurred_at || null };
  });

  const activeTasks = taskActivities.filter((item) => item.active);
  const activeVerifications = verifications.filter((item) => item.active);
  const routeActivities = routes.map((route) => {
    const id = idOf(route, "id", "route_id");
    const status = text(route?.status).toLowerCase();
    const humanStatus = text(route?.humanStatus || route?.human_review).toLowerCase();
    const routeTasks = taskActivities.filter((item) => item.routeId === id);
    const routeVerifications = verifications.filter((item) => item.routeId === id);
    const currentTasks = routeTasks.filter((item) => item.active);
    const currentVerifications = routeVerifications.filter((item) => item.active);
    let phase = "ready", label = "等待任务分配", tone = "waiting", active = false;
    if (ROUTE_TERMINAL.has(status)) {
      phase = status === "completed" || status === "refuted" ? "completed" : "failed";
      label = status === "completed" ? "路线已完成" : status === "refuted" ? "路线已得到反驳结果" : status === "merged" ? "已并入其它路线" : status === "pruned" ? "路线已淘汰" : "路线已停止";
      tone = phase === "completed" ? "success" : "muted";
    } else if (route.isProposal) {
      phase = "planning"; label = "提案等待规划器处理，无需人工批准"; tone = "waiting";
    } else if (["pending", "proposed"].includes(humanStatus)) {
      phase = "review"; label = "等待你的批准"; tone = "waiting";
    } else if (status === "paused") {
      phase = "paused"; label = "路线已暂停"; tone = "waiting";
    } else if (status === "blocked") {
      phase = "blocked"; label = "路线遇到阻碍"; tone = "warning";
    } else if (currentVerifications.length) {
      phase = "verifying"; label = `独立验证中 · ${currentVerifications.length}`; tone = "active"; active = true;
    } else if (currentTasks.length) {
      const priority = ["ingesting", "executing", "starting", "queued"];
      const primary = priority.map((name) => currentTasks.find((item) => item.phase === name)).find(Boolean) || currentTasks[0];
      phase = primary.phase; label = currentTasks.length > 1 ? `${primary.label} · ${currentTasks.length} 个任务` : primary.label; tone = primary.tone; active = primary.active;
    } else if (pendingRoutes.length && ["approved", "not_required"].includes(humanStatus)) {
      const completed = routeTasks.some((item) => item.phase === "completed");
      phase = completed ? "idle" : "ready"; label = completed ? "已批准 · 当前任务已完成" : routeTasks.length ? "已批准 · 等待 Worker" : "已批准 · 当前没有待执行任务"; tone = completed ? "success" : "waiting";
    } else if (planning.active) {
      phase = "planning"; label = "正在参与下一轮规划"; tone = planning.stale ? "warning" : "active"; active = !planning.stale;
    } else if (routeTasks.some((item) => item.phase === "completed")) {
      phase = "idle"; label = "本轮任务已完成"; tone = "success";
    } else if (["approved", "not_required"].includes(humanStatus)) {
      phase = "ready"; label = "已批准 · 尚未分配任务"; tone = "waiting";
    } else if (status) {
      phase = status; label = `路线状态：${status}`; tone = "muted";
    }
    return { id, status, humanStatus, phase, label, tone, active, tasks: routeTasks, verifications: routeVerifications };
  });

  let phase = "idle", label = "等待 MathCat 开始研究", detail = "当前没有可审计的运行活动。", tone = "muted", active = false;
  if (PROJECT_TERMINAL.has(projectStatus)) {
    phase = "terminal";
    label = { success: "研究目标已完成", partial_success: "研究取得部分结果", refuted: "已找到并确认反例", environment_failed: "运行环境失败", stopped_by_human: "研究已由人工停止", error: "研究运行出错" }[projectStatus] || "研究已结束";
    detail = ["success", "refuted"].includes(projectStatus) ? "最终状态来自 MathCat 的验证与 Fact Gate。" : "可以查看研究日志了解最后一次状态变化。";
    tone = ["success", "refuted"].includes(projectStatus) ? "success" : ["error", "environment_failed"].includes(projectStatus) ? "error" : "muted";
  } else if (projectStatus === "paused") {
    phase = "paused"; label = "研究已暂停"; detail = "恢复后 MathCat 会从持久化状态继续。"; tone = "waiting";
  } else if (activeVerifications.length && activeTasks.length) {
    phase = "verifying"; active = true; tone = "active"; label = `正在研究，同时独立验证候选结果${pendingRoutes.length ? ` · ${pendingRoutes.length} 条路线待审核` : ""}`; detail = `${activeTasks.length} 个任务运行中，${activeVerifications.length} 个候选正在验证。${pendingRoutes.length ? "待审路线保持暂停，不会阻塞已批准路线。" : ""}`;
  } else if (activeVerifications.length) {
    phase = "verifying"; active = true; tone = "active"; label = `${activeVerifications.length === 1 ? activeVerifications[0].label : `正在独立验证 ${activeVerifications.length} 个候选结果`}${pendingRoutes.length ? ` · ${pendingRoutes.length} 条路线待审核` : ""}`; detail = `验证状态来自独立验证队列；候选通过 Fact Gate 前不会显示为可信 Fact。${pendingRoutes.length ? "待审路线保持暂停。" : ""}`;
  } else if (activeTasks.length) {
    const priority = ["ingesting", "executing", "starting", "queued"];
    const primary = priority.map((name) => activeTasks.find((item) => item.phase === name)).find(Boolean) || activeTasks[0];
    phase = primary.phase; active = primary.active; tone = primary.tone; label = `${activeTasks.length === 1 ? primary.label : `${primary.label} · 共 ${activeTasks.length} 个任务`}${pendingRoutes.length ? ` · ${pendingRoutes.length} 条路线待审核` : ""}`; detail = `${activeTasks.length === 1 ? `${primary.workerRole}任务：${primary.objective}` : "多个研究任务正在并行推进；可在证明路线图中查看它们分别属于哪条路线。"}${pendingRoutes.length ? " 待审路线保持暂停，不会阻塞这些已批准任务。" : ""}`;
  } else if (pendingRoutes.length || pendingQuestions.length) {
    phase = "review"; tone = "waiting";
    label = pendingRoutes.length ? `等待你处理 ${pendingRoutes.length} 条研究路线` : `等待你回答 ${pendingQuestions.length || 1} 个研究问题`;
    const approved = routeActivities.filter((item) => ["approved", "not_required"].includes(item.humanStatus)).length;
    detail = approved ? `${approved} 条路线已经逐条放行；它们有可执行任务时会直接进入 Worker，未审核路线继续保持暂停。` : "这些操作只决定是否投入研究资源，不代表认可数学结论。";
  } else if (projectStatus === "needs_human_review") {
    phase = "blocked"; tone = "warning"; label = "等待你检查研究状态"; detail = "MathCat 报告需要人工审核，但当前快照里没有待处理的路线或问题；这可能是运行状态未及时刷新，请查看研究日志。";
  } else if (planning.active || planning.stale) {
    phase = "planning"; active = planning.active; tone = planning.stale ? "warning" : "active"; label = planning.label; detail = planning.detail;
  } else if (taskActivities.some((item) => ["blocked", "failed"].includes(item.phase))) {
    phase = "blocked"; tone = "warning"; label = "当前没有任务在运行"; detail = "至少一个任务遇到阻碍或执行失败，请展开任务列表和研究日志。";
  } else if (projectStatus === "running") {
    phase = "idle"; tone = "waiting"; label = "正在准备下一轮研究"; detail = "当前快照中没有正在执行的任务或验证；MathCat 可能正在轮次切换，若长时间不变请检查研究日志。";
  } else if (projectStatus === "created") {
    phase = "idle"; label = "等待启动研究"; detail = "项目已经建立，尚未进入第一轮规划。";
  }

  const latestEvent = events.at(-1) || null;
  const result = {
    phase, label, detail, tone, active, projectStatus, currentRound, planning,
    pendingRoutes: pendingRoutes.length,
    pendingQuestions: pendingQuestions.length,
    activeTasks: activeTasks.length,
    activeWorkers: workers.filter((item) => ["spawn_requested", "starting", "handshaking", "ready", "lease_accepted", "running", "checkpointing", "result_submitted", "draining"].includes(text(item?.status).toLowerCase())).length,
    activeVerifications: activeVerifications.length,
    tasks: taskActivities,
    verifications,
    routes: routeActivities,
    routeById: Object.fromEntries(routeActivities.map((item) => [item.id, item])),
    lastUpdatedAt: latestEvent?.at || latestEvent?.occurred_at || board?.updatedAt || null
  };
  result.stages = stageBarState(result);
  return result;
}
