import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { QueuedJsonWriter } from "./fs-utils.mjs";

const now = () => new Date().toISOString();
const makeId = (prefix) => `${prefix}_${crypto.randomUUID()}`;
const REVIEW_MODES = new Set(["automatic", "balanced", "strict"]);
const COMMAND_SETTLE_ATTEMPTS = 5;
const COMMAND_SETTLE_DELAY_MS = 25;

const wait = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

function commandEventState(board, commandId) {
  if (!commandId) return null;
  for (const event of board?.events || []) {
    const type = String(event?.type || event?.kind || "").toLowerCase();
    const entityId = String(event?.entity?.id || "");
    const causedById = String(event?.causedBy?.id || event?.caused_by?.id || "");
    if (entityId !== commandId && causedById !== commandId) continue;
    if (type === "human_command.failed") return "failed";
    if (type === "human_command.applied" || type === "human_command.validated") return "applied";
  }
  return null;
}

function initialBoard({ conversationId, workspaceId, agent, problem, targetStatement = problem, contextSources = [], reviewMode = "strict" }) {
  const common = { id: makeId("rb"), conversationId, workspaceId: workspaceId ?? null, agent, reviewMode, contextSources, problem: { version: 1, original: problem, statement: targetStatement, assumptions: [], goal: targetStatement }, status: "created", revision: 1, createdAt: now(), updatedAt: now(), decisions: [], events: [{ id: makeId("evt"), at: now(), kind: "board_created", text: `已为 ${agent} 创建研究白板` }] };
  if (agent === "rethlas") return { ...common, mode: "generation_verification", integration: { structured: false, source: "codex_cli", message: "Rethlas 当前通过 Codex CLI 运行；结构化蓝图与验证记录尚未接入白板协议。" }, iterations: [], blueprint: [], verification: { verdict: "pending", criticalErrors: [], gaps: [], repairHints: [] } };
  if (agent === "danus") return { ...common, mode: "orchestration_truth", integration: { structured: false, source: "codex_cli", message: "Danus 当前通过 Codex CLI 运行；Worker、记忆与 Fact 数据尚未接入白板协议。" }, strategy: { elaboration: "等待首次综合", masterGuidance: "等待确定研究方向" }, workers: [], memories: { local: [], global: [] }, facts: [], verificationQueue: [] };
  return { ...common, agent: "mathcat", mode: "human_collaboration", integration: { structured: true, source: "mathcat_api", message: "结构化数据由 MathCat API 提供。" }, routes: [{ id: makeId("route"), title: "直接证明", summary: "从定义与已知条件出发构造证明", status: "proposed", humanStatus: "pending" }, { id: makeId("route"), title: "反例与边界检查", summary: "检查结论边界、必要条件及潜在反例", status: "proposed", humanStatus: "pending" }], goals: [], claims: [], failedRoutes: [] };
}

export class ResearchBoards {
  constructor(runtimeRoot, mathcatClient = null, { cacheTtlMs = 1_500 } = {}) { this.file = path.join(runtimeRoot, "research-boards.json"); this.rows = []; this.mathcat = mathcatClient; this.writer = new QueuedJsonWriter(this.file); this.cacheTtlMs = cacheTtlMs; this.cache = new Map(); this.refreshing = new Map(); }
  async load() { try { this.rows = JSON.parse(await fs.readFile(this.file, "utf8")); } catch (error) { if (error.code !== "ENOENT") throw error; } return this; }
  async save() { return this.writer.save(this.rows); }
  get(boardId) { return this.rows.find((row) => row.id === boardId) || null; }
  byConversation(conversationId) { return this.rows.find((row) => row.conversationId === conversationId) || null; }
  async create(input) { const existing = this.byConversation(input.conversationId); if (existing) return this.refresh(existing); const row = initialBoard(input); if (row.agent === "mathcat" && this.mathcat) { const remote = await this.mathcat.createProject({ name: input.title || `MathCat Lab · ${String(input.targetStatement || input.problem).slice(0, 36)}`, problem: String(input.problem), targetStatement: String(input.targetStatement || input.problem), reviewMode: row.reviewMode }); row.remoteProjectId = remote.project_id; } this.rows.push(row); await this.save(); return this.refresh(row); }
  invalidate(row) { if (row?.remoteProjectId) this.cache.delete(row.remoteProjectId); }
  async refreshAfterMutation(row, previousBoard, receipt, { assumeRunning = false, waitForCommand = false } = {}) {
    this.invalidate(row);
    const commandId = String(receipt?.command_id || "");
    let lastBoard = null;
    let lastError = null;
    let commandFailure = null;
    const attempts = waitForCommand && commandId ? COMMAND_SETTLE_ATTEMPTS : 1;
    let useBoardEventFallback = !waitForCommand || !commandId || typeof this.mathcat?.commandStatus !== "function";
    if (!useBoardEventFallback) {
      for (let attempt = 0; attempt < attempts; attempt += 1) {
        try {
          const command = await this.mathcat.commandStatus(row.remoteProjectId, commandId);
          const status = String(command?.status || "").toLowerCase();
          if (status === "failed") {
            commandFailure = new Error(String(command?.error || "MathCat 已拒绝这次白板操作，请刷新后检查命令记录。"));
            break;
          }
          if (["applied", "validated"].includes(status)) {
            try { return await this.refresh(row, { fresh: true }); }
            catch (error) { lastError = error; break; }
          }
        } catch (error) {
          if ([404, 405].includes(error?.status)) {
            useBoardEventFallback = true;
            break;
          }
          lastError = error;
        }
        if (attempt + 1 < attempts) await wait(COMMAND_SETTLE_DELAY_MS);
      }
      if (commandFailure) throw commandFailure;
    }
    if (!useBoardEventFallback) {
      const meta = receipt?._responseMeta || {};
      return {
        ...previousBoard,
        planningSuggestions: Array.isArray(row.planningSuggestions) ? row.planningSuggestions : previousBoard.planningSuggestions,
        lastGoalReview: row.lastGoalReview ?? previousBoard.lastGoalReview,
        reviewMode: REVIEW_MODES.has(row.reviewMode) ? row.reviewMode : previousBoard.reviewMode,
        revision: Number(receipt?.after_revision ?? meta.project_revision ?? previousBoard.revision ?? 0),
        eventCursor: Number(meta.event_cursor ?? previousBoard.eventCursor ?? 0),
        status: assumeRunning ? "running" : previousBoard.status,
        syncWarning: lastError
          ? `操作已由 MathCat 接受，但命令状态暂时无法确认：${lastError.message}`
          : "操作已由 MathCat 接受，后台正在同步最新白板状态。",
        updatedAt: now()
      };
    }
    for (let attempt = 0; attempt < attempts; attempt += 1) {
      try {
        lastBoard = await this.refresh(row, { fresh: true });
        const commandState = commandEventState(lastBoard, commandId);
        if (commandState === "failed") {
          commandFailure = new Error("MathCat 已拒绝这次白板操作，请刷新后检查命令记录。");
          break;
        }
        if (!waitForCommand || !commandId || commandState === "applied") return lastBoard;
      } catch (error) {
        lastError = error;
      }
      if (attempt + 1 < attempts) {
        this.invalidate(row);
        await wait(COMMAND_SETTLE_DELAY_MS);
      }
    }
    if (commandFailure) throw commandFailure;
    const meta = receipt?._responseMeta || {};
    return {
      ...previousBoard,
      planningSuggestions: Array.isArray(row.planningSuggestions) ? row.planningSuggestions : previousBoard.planningSuggestions,
      lastGoalReview: row.lastGoalReview ?? previousBoard.lastGoalReview,
      reviewMode: REVIEW_MODES.has(row.reviewMode) ? row.reviewMode : previousBoard.reviewMode,
      revision: Number(receipt?.after_revision ?? meta.project_revision ?? lastBoard?.revision ?? previousBoard.revision ?? 0),
      eventCursor: Number(meta.event_cursor ?? lastBoard?.eventCursor ?? previousBoard.eventCursor ?? 0),
      status: assumeRunning ? "running" : previousBoard.status,
      syncWarning: lastError
        ? `操作已由 MathCat 接受，但最新白板暂时读取失败：${lastError.message}`
        : "操作已由 MathCat 接受，后台正在同步最新白板状态。",
      updatedAt: now()
    };
  }
  async refresh(row, { fresh = false } = {}) {
    if (!row) return null;
    if (row.agent !== "mathcat" || !this.mathcat) return row;
    if (!row.remoteProjectId) { const remote = await this.mathcat.createProject({ name: `MathCat Lab · ${String(row.problem?.statement || row.problem?.original).slice(0, 36)}`, problem: String(row.problem?.original || row.problem?.statement), targetStatement: String(row.problem?.statement || row.problem?.goal || row.problem?.original), reviewMode: row.reviewMode || "strict" }); row.remoteProjectId = remote.project_id; await this.save(); }
    const key = row.remoteProjectId;
    const cached = this.cache.get(key);
    if (!fresh && cached && Date.now() - cached.at < this.cacheTtlMs) return cached.board;
    if (this.refreshing.has(key)) { const board = await this.refreshing.get(key); if (!fresh) return board; }
    let request;
    request = (async () => {
      const { mapMathCatBoard } = await import("./mathcat-client.mjs");
      const board = mapMathCatBoard(await this.mathcat.board(key), row);
      // Mirror the backend-owned policy for new-project defaults and legacy
      // fallback. A local persistence problem must not hide the live setting.
      if (REVIEW_MODES.has(board.reviewMode) && row.reviewMode !== board.reviewMode) {
        row.reviewMode = board.reviewMode;
        row.updatedAt = now();
        await this.save().catch((error) => console.error("Failed to mirror MathCat review mode locally:", error));
      }
      this.cache.set(key, { at: Date.now(), board });
      return board;
    })().finally(() => { if (this.refreshing.get(key) === request) this.refreshing.delete(key); });
    this.refreshing.set(key, request);
    return request;
  }
  async view(boardId) { return this.refresh(this.get(boardId)); }
  async viewByConversation(conversationId) { return this.refresh(this.byConversation(conversationId)); }
  async startByConversation(conversationId) {
    const row = this.byConversation(conversationId);
    if (!row) throw new Error("Research board not found.");
    const board = await this.refresh(row);
    if (row.agent !== "mathcat" || !this.mathcat) return board;
    if (board.status === "created") await this.mathcat.startProject(row.remoteProjectId, board);
    else if (board.status === "paused") await this.mathcat.resumeProject(row.remoteProjectId, board);
    else if (board.status !== "running") throw new Error(`MathCat project cannot start from status ${board.status}.`);
    this.invalidate(row); return this.refresh(row, { fresh: true });
  }
  async stopByConversation(conversationId) {
    const row = this.byConversation(conversationId);
    if (!row || row.agent !== "mathcat" || !this.mathcat) return null;
    const board = await this.refresh(row);
    if (["created", "running", "paused"].includes(board.status)) await this.mathcat.stopProject(row.remoteProjectId, board);
    this.invalidate(row); return this.refresh(row, { fresh: true });
  }
  async retire(row) { if (row?.agent === "mathcat" && row.remoteProjectId && this.mathcat) { const board = await this.refresh(row, { fresh: true }); if (["created", "running", "paused"].includes(board.status)) await this.mathcat.stopProject(row.remoteProjectId, board); this.invalidate(row); } }
  async deleteByConversation(conversationId) { const row = this.byConversation(conversationId); if (!row) return; await this.retire(row); this.rows = this.rows.filter((item) => item.conversationId !== conversationId); await this.save(); }
  async deleteByConversations(conversationIds) { const ids = new Set(conversationIds); const rows = this.rows.filter((row) => ids.has(row.conversationId)); for (const row of rows) await this.retire(row); if (!rows.length) return; this.rows = this.rows.filter((row) => !ids.has(row.conversationId)); await this.save(); }
  async mutate(boardId, updater, eventText) { const row = this.get(boardId); if (!row) throw new Error("Research board not found."); updater(row); row.revision += 1; row.updatedAt = now(); if (eventText) row.events.push({ id: makeId("evt"), at: now(), kind: "human_action", text: eventText }); await this.save(); return row; }
  async updateProblem(boardId, input) { const row = this.get(boardId); if (row?.agent === "mathcat" && this.mathcat) { const board = await this.refresh(row, { fresh: true }); await this.mathcat.reviseProblem(row.remoteProjectId, board, input); this.invalidate(row); return this.refresh(row, { fresh: true }); } return this.mutate(boardId, (item) => { item.problem = { ...item.problem, ...input, version: item.problem.version + 1 }; }, "研究者更新了问题陈述"); }
  async addRoute(boardId, input) {
    const row = this.get(boardId);
    if (row?.agent === "mathcat" && this.mathcat) {
      const board = await this.refresh(row, { fresh: true });
      const receipt = input.executeImmediately
        ? await this.mathcat.createAndExecuteRoute(row.remoteProjectId, board, input)
        : await this.mathcat.proposeRoute(row.remoteProjectId, board, input);
      const optimisticBoard = input.executeImmediately && receipt?.route_id
        ? {
            ...board,
            routes: [...(board.routes || []), {
              id: String(receipt.route_id),
              title: String(input.title || "人工路线"),
              summary: String(input.summary || input.objective || ""),
              plainLanguageSummary: String(input.objective || input.summary || ""),
              targetGoalIds: Array.isArray(input.targetGoalIds) ? input.targetGoalIds.map(String) : [],
              status: "active",
              humanStatus: "approved",
              isProposal: false,
              taskId: String(receipt.task_id || ""),
              optimistic: true
            }]
          }
        : board;
      return this.refreshAfterMutation(row, optimisticBoard, receipt, { assumeRunning: Boolean(input.executeImmediately) });
    }
    if (input.executeImmediately) throw new Error("编写并执行路线需要可用的 MathCat 后端。");
    return this.mutate(boardId, (item) => { if (item.agent !== "mathcat") throw new Error("Routes are managed by MathCat only."); item.routes.push({ id: makeId("route"), title: String(input.title || "新路线"), summary: String(input.summary || ""), status: "proposed", humanStatus: "pending" }); }, `研究者添加路线：${input.title || "新路线"}`);
  }
  async addPlanningSuggestion(boardId, input) {
    const row = this.get(boardId);
    if (!row || row.agent !== "mathcat" || !this.mathcat) throw new Error("规划建议需要可用的 MathCat 后端。");
    const board = await this.refresh(row, { fresh: true });
    const command = await this.mathcat.addPlanningSuggestion(row.remoteProjectId, board, input);
    row.planningSuggestions = Array.isArray(row.planningSuggestions) ? row.planningSuggestions : [];
    row.planningSuggestions.push({
      id: String(command?.command_id || makeId("suggestion")),
      commandId: String(command?.command_id || ""),
      content: String(input.content || "").trim(),
      targetRouteId: String(input.targetRouteId || "").trim(),
      reason: String(input.reason || "").trim(),
      status: String(command?.status || "validated"),
      createdRound: Number(board.summary?.current_round ?? board.currentRound ?? 0),
      effectiveRound: Number(board.summary?.current_round ?? board.currentRound ?? 0) + 1,
      submittedAt: now()
    });
    row.updatedAt = now();
    await this.save().catch((error) => console.error("Failed to persist the planning suggestion receipt locally:", error));
    return this.refreshAfterMutation(row, { ...board, planningSuggestions: row.planningSuggestions }, command, { waitForCommand: true });
  }
  async forceGoalReview(boardId, input) {
    const row = this.get(boardId);
    if (!row || row.agent !== "mathcat" || !this.mathcat) throw new Error("目标梳理需要可用的 MathCat 后端。");
    const focus = String(input.focus || "").trim();
    const board = await this.refresh(row, { fresh: true });
    const command = await this.mathcat.goalReview(row.remoteProjectId, board, { focus });
    row.lastGoalReview = {
      focus,
      status: "queued",
      commandId: String(command?.command_id || ""),
      afterRevision: Number(command?.after_revision || board.revision || 0),
      submittedAt: now(),
      submittedEventCursor: Number(board.eventCursor || 0)
    };
    row.updatedAt = now();
    await this.save().catch((error) => console.error("Failed to persist the goal-review receipt locally:", error));
    return this.refreshAfterMutation(row, { ...board, lastGoalReview: row.lastGoalReview }, command, { assumeRunning: true, waitForCommand: true });
  }
  async updateSettings(boardId, input) {
    const row = this.get(boardId);
    if (!row || row.agent !== "mathcat" || !this.mathcat) throw new Error("研究设置需要可用的 MathCat 后端。");
    const board = await this.refresh(row, { fresh: true });
    const reviewMode = input.reviewMode == null ? board.reviewMode : String(input.reviewMode);
    if (!REVIEW_MODES.has(reviewMode)) throw new Error("未知的人工参与程度。");
    const currentBudget = board.budget;
    const requestedWorkers = input.maxParallelWorkers == null ? currentBudget?.maxParallelWorkers : Number(input.maxParallelWorkers);
    const requestedMinutes = input.maxMinutesPerTask == null ? currentBudget?.maxMinutesPerTask : Number(input.maxMinutesPerTask);
    const budgetChanged = (input.maxParallelWorkers != null && requestedWorkers !== currentBudget?.maxParallelWorkers)
      || (input.maxMinutesPerTask != null && requestedMinutes !== currentBudget?.maxMinutesPerTask);
    const reviewChanged = reviewMode !== board.reviewMode;
    if (!budgetChanged && !reviewChanged) return board;
    const command = await this.mathcat.updateResearchSettings(row.remoteProjectId, board, {
      maxParallelWorkers: requestedWorkers,
      maxMinutesPerTask: requestedMinutes,
      reviewMode
    });
    // Keep the value for old-server fallback and new-project defaults, but only
    // after the atomic backend mutation has succeeded.
    row.reviewMode = reviewMode;
    row.updatedAt = now();
    await this.save().catch((error) => console.error("Failed to persist the MathCat review mode locally:", error));
    const optimisticBoard = {
      ...board,
      reviewMode,
      budget: currentBudget ? {
        ...currentBudget,
        maxParallelWorkers: requestedWorkers,
        maxMinutesPerTask: requestedMinutes
      } : currentBudget
    };
    const releasesRouteReview = board.status === "needs_human_review"
      && reviewMode !== "strict"
      && !(board.decisions || []).some((decision) => decision.status === "pending");
    return this.refreshAfterMutation(row, optimisticBoard, command, { assumeRunning: releasesRouteReview, waitForCommand: true });
  }
  async routeCommand(boardId, routeId, command) {
    const row = this.get(boardId);
    if (row?.agent === "mathcat" && this.mathcat) {
      const board = await this.refresh(row, { fresh: true });
      const route = board.routes?.find((entry) => entry.id === routeId);
      if (!route) throw new Error("Route not found.");
      const map = { approve: ["active", "approved"], pause: ["paused", route.humanStatus], reject: ["pruned", "rejected"], resume: ["active", route.humanStatus] };
      if (!map[command]) throw new Error("Unknown route command.");
      const receipt = await this.mathcat.routeCommand(row.remoteProjectId, routeId, board, command);
      const optimisticBoard = {
        ...board,
        routes: board.routes.map((entry) => entry.id === routeId
          ? { ...entry, status: map[command][0], humanStatus: map[command][1] }
          : entry)
      };
      return this.refreshAfterMutation(row, optimisticBoard, receipt, { assumeRunning: ["approve", "resume"].includes(command) });
    }
    return this.mutate(boardId, (item) => { const route = item.routes?.find((entry) => entry.id === routeId); if (!route) throw new Error("Route not found."); const map = { approve: ["active", "approved"], pause: ["paused", route.humanStatus], reject: ["pruned", "rejected"], resume: ["active", route.humanStatus] }; if (!map[command]) throw new Error("Unknown route command."); [route.status, route.humanStatus] = map[command]; }, `研究路线已${{ approve: "批准", pause: "暂停", reject: "否决", resume: "恢复" }[command]}`);
  }
  async addDecision(boardId, input) {
    const question = String(input.question || "").trim();
    if (!question) throw new Error("Question is required.");
    const options = Array.isArray(input.options) ? input.options.slice(0, 5).map((option, index) => ({
      value: String(option?.value || `option_${index + 1}`),
      label: String(option?.label || option?.value || `选项 ${index + 1}`),
      description: String(option?.description || option?.reason || "")
    })) : [];
    return this.mutate(boardId, (row) => row.decisions.push({
      id: makeId("decision"),
      source: String(input.source || row.agent || "mathcat"),
      question,
      context: String(input.context || ""),
      options,
      blockingEntityIds: Array.isArray(input.blockingEntityIds) ? input.blockingEntityIds.map(String) : [],
      status: "pending",
      answer: null,
      note: "",
      createdAt: now()
    }), `智能体向研究者提问：${question}`);
  }
  async answerDecision(boardId, decisionId, answer, note = "") {
    const remoteRow = this.get(boardId);
    if (remoteRow?.agent === "mathcat" && this.mathcat) {
      if (remoteRow.decisions?.some((item) => item.id === decisionId)) {
        const board = await this.refresh(remoteRow, { fresh: true });
        await this.answerLocalDecision(boardId, decisionId, answer, note);
        const optimisticBoard = {
          ...board,
          decisions: board.decisions.map((item) => item.id === decisionId
            ? { ...item, status: answer === "defer" ? "pending" : "answered", answer: answer === "defer" ? item.answer : answer, note: String(note || "") }
            : item)
        };
        return this.refreshAfterMutation(remoteRow, optimisticBoard, null, { assumeRunning: answer !== "defer" });
      }
      if (answer === "defer") return this.refresh(remoteRow);
      const board = await this.refresh(remoteRow, { fresh: true });
      const receipt = await this.mathcat.answerQuestion(remoteRow.remoteProjectId, decisionId, board, answer, note);
      const optimisticBoard = {
        ...board,
        decisions: board.decisions.map((item) => item.id === decisionId
          ? { ...item, status: "answered", answer, note: String(note || "") }
          : item)
      };
      return this.refreshAfterMutation(remoteRow, optimisticBoard, receipt, { assumeRunning: true });
    }
    return this.answerLocalDecision(boardId, decisionId, answer, note);
  }
  async resolveAutomaticReview(conversationId) {
    const row = this.byConversation(conversationId);
    if (!row) throw new Error("Research board not found.");
    let board = await this.refresh(row);
    if (board.reviewMode !== "automatic") return board;
    for (const decision of (board.decisions || []).filter((item) => item.status === "pending").slice(0, 10)) {
      const options = Array.isArray(decision.options) ? decision.options : [];
      const conservative = options.find((option) => /reject|拒绝|保持|不修改|no_change|abort/i.test(`${option.value} ${option.label}`)) || options[0];
      const answer = conservative?.value || "automatic_continue";
      board = await this.answerDecision(row.id, decision.id, answer, "自动运行模式：采用可用选项中的保守选择，不等待人工审核。");
    }
    return board;
  }
  async answerLocalDecision(boardId, decisionId, answer, note = "") {
    const normalizedAnswer = String(answer || "").trim();
    if (!normalizedAnswer) throw new Error("Answer is required.");
    return this.mutate(boardId, (row) => {
      const decision = row.decisions.find((item) => item.id === decisionId);
      if (!decision) throw new Error("Decision not found.");
      decision.note = String(note || "").trim();
      if (normalizedAnswer === "defer") {
        decision.status = "pending";
        decision.deferredAt = now();
        return;
      }
      decision.status = "answered";
      decision.answer = normalizedAnswer;
      decision.answeredAt = now();
    }, `研究者${{ approve: "同意", reject: "否决", defer: "暂缓" }[normalizedAnswer] || "回答"}了一项提问`);
  }
}
