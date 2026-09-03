import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { writeJsonAtomic } from "./fs-utils.mjs";

const now = () => new Date().toISOString();
const makeId = (prefix) => `${prefix}_${crypto.randomUUID()}`;

function initialBoard({ conversationId, workspaceId, agent, problem }) {
  const common = { id: makeId("rb"), conversationId, workspaceId: workspaceId ?? null, agent, problem: { version: 1, original: problem, statement: problem, assumptions: [], goal: problem }, status: "created", revision: 1, createdAt: now(), updatedAt: now(), decisions: [], events: [{ id: makeId("evt"), at: now(), kind: "board_created", text: `已为 ${agent} 创建研究白板` }] };
  if (agent === "rethlas") return { ...common, mode: "generation_verification", iterations: [], blueprint: [], verification: { verdict: "pending", criticalErrors: [], gaps: [], repairHints: [] } };
  if (agent === "danus") return { ...common, mode: "orchestration_truth", strategy: { elaboration: "等待首次综合", masterGuidance: "等待确定研究方向" }, workers: [], memories: { local: [], global: [] }, facts: [], verificationQueue: [] };
  return { ...common, agent: "mathcat", mode: "human_collaboration", routes: [{ id: makeId("route"), title: "直接证明", summary: "从定义与已知条件出发构造证明", status: "proposed", humanStatus: "pending" }, { id: makeId("route"), title: "反例与边界检查", summary: "检查结论边界、必要条件及潜在反例", status: "proposed", humanStatus: "pending" }], goals: [], claims: [], failedRoutes: [] };
}

export class ResearchBoards {
  constructor(runtimeRoot, mathcatClient = null) { this.file = path.join(runtimeRoot, "research-boards.json"); this.rows = []; this.mathcat = mathcatClient; }
  async load() { try { this.rows = JSON.parse(await fs.readFile(this.file, "utf8")); } catch (error) { if (error.code !== "ENOENT") throw error; } return this; }
  async save() { await fs.mkdir(path.dirname(this.file), { recursive: true }); await writeJsonAtomic(this.file, this.rows); }
  get(boardId) { return this.rows.find((row) => row.id === boardId) || null; }
  byConversation(conversationId) { return this.rows.find((row) => row.conversationId === conversationId) || null; }
  async create(input) { const existing = this.byConversation(input.conversationId); if (existing) return this.refresh(existing); const row = initialBoard(input); if (row.agent === "mathcat" && this.mathcat) { const remote = await this.mathcat.createProject({ name: input.title || `MathCat Lab · ${String(input.problem).slice(0, 36)}`, problem: String(input.problem) }); row.remoteProjectId = remote.project_id; } this.rows.push(row); await this.save(); return this.refresh(row); }
  async refresh(row) { if (!row) return null; if (row.agent !== "mathcat" || !this.mathcat) return row; if (!row.remoteProjectId) { const remote = await this.mathcat.createProject({ name: `MathCat Lab · ${String(row.problem?.statement || row.problem?.original).slice(0, 36)}`, problem: String(row.problem?.original || row.problem?.statement) }); row.remoteProjectId = remote.project_id; await this.save(); } const { mapMathCatBoard } = await import("./mathcat-client.mjs"); return mapMathCatBoard(await this.mathcat.board(row.remoteProjectId), row); }
  async view(boardId) { return this.refresh(this.get(boardId)); }
  async viewByConversation(conversationId) { return this.refresh(this.byConversation(conversationId)); }
  async deleteByConversation(conversationId) { const before = this.rows.length; this.rows = this.rows.filter((row) => row.conversationId !== conversationId); if (this.rows.length !== before) await this.save(); }
  async deleteByConversations(conversationIds) { const ids = new Set(conversationIds); const before = this.rows.length; this.rows = this.rows.filter((row) => !ids.has(row.conversationId)); if (this.rows.length !== before) await this.save(); }
  async mutate(boardId, updater, eventText) { const row = this.get(boardId); if (!row) throw new Error("Research board not found."); updater(row); row.revision += 1; row.updatedAt = now(); if (eventText) row.events.push({ id: makeId("evt"), at: now(), kind: "human_action", text: eventText }); await this.save(); return row; }
  async updateProblem(boardId, input) { const row = this.get(boardId); if (row?.agent === "mathcat" && this.mathcat) { const board = await this.refresh(row); await this.mathcat.reviseProblem(row.remoteProjectId, board, input); return this.refresh(row); } return this.mutate(boardId, (item) => { item.problem = { ...item.problem, ...input, version: item.problem.version + 1 }; }, "研究者更新了问题陈述"); }
  async addRoute(boardId, input) { const row = this.get(boardId); if (row?.agent === "mathcat" && this.mathcat) { const board = await this.refresh(row); await this.mathcat.proposeRoute(row.remoteProjectId, board, input); return this.refresh(row); } return this.mutate(boardId, (item) => { if (item.agent !== "mathcat") throw new Error("Routes are managed by MathCat only."); item.routes.push({ id: makeId("route"), title: String(input.title || "新路线"), summary: String(input.summary || ""), status: "proposed", humanStatus: "pending" }); }, `研究者添加路线：${input.title || "新路线"}`); }
  async routeCommand(boardId, routeId, command) { const row = this.get(boardId); if (row?.agent === "mathcat" && this.mathcat) { const board = await this.refresh(row); await this.mathcat.routeCommand(row.remoteProjectId, routeId, board, command); return this.refresh(row); } return this.mutate(boardId, (item) => { const route = item.routes?.find((entry) => entry.id === routeId); if (!route) throw new Error("Route not found."); const map = { approve: ["active", "approved"], pause: ["paused", route.humanStatus], reject: ["pruned", "rejected"], resume: ["active", route.humanStatus] }; if (!map[command]) throw new Error("Unknown route command."); [route.status, route.humanStatus] = map[command]; }, `研究路线已${{ approve: "批准", pause: "暂停", reject: "否决", resume: "恢复" }[command]}`); }
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
      if (remoteRow.decisions?.some((item) => item.id === decisionId)) { await this.answerLocalDecision(boardId, decisionId, answer, note); return this.refresh(remoteRow); }
      if (answer === "defer") return this.refresh(remoteRow);
      const board = await this.refresh(remoteRow);
      await this.mathcat.answerQuestion(remoteRow.remoteProjectId, decisionId, board, answer, note);
      return this.refresh(remoteRow);
    }
    return this.answerLocalDecision(boardId, decisionId, answer, note);
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
