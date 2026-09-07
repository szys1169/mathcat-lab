import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import { ResearchBoards } from "../src/research-boards.mjs";

const server = await fs.readFile(new URL("../src/server.mjs", import.meta.url), "utf8");
const boards = await fs.readFile(new URL("../src/research-boards.mjs", import.meta.url), "utf8");
const app = await fs.readFile(new URL("../public/app.js", import.meta.url), "utf8");

test("platform exposes every MathCat human collaboration mutation", () => {
  assert.match(server, /\/api\\\/research-boards\\\/\(\[\^\/\]\+\)\\\/suggestions/);
  assert.match(server, /\/api\\\/research-boards\\\/\(\[\^\/\]\+\)\\\/goal-review/);
  assert.match(server, /\/api\\\/research-boards\\\/\(\[\^\/\]\+\)\\\/settings/);
  assert.match(server, /researchBoards\.addPlanningSuggestion/);
  assert.match(server, /researchBoards\.forceGoalReview/);
  assert.match(server, /researchBoards\.updateSettings/);
  assert.match(server, /errorStatus\(error\)/);
  assert.match(server, /explicit>=400&&explicit<600/);
});

test("every collaboration mutation reconnects the MathCat watcher", () => {
  assert.match(server, /async function resumeBoardWatcher/);
  for (const marker of ["addRoute", "addPlanningSuggestion", "forceGoalReview", "updateSettings"]) {
    const start = server.indexOf(`researchBoards.${marker}`);
    assert.notEqual(start, -1, `missing ${marker}`);
    assert.match(server.slice(start, start + 260), /resumeBoardWatcher/);
  }
});

test("goal review receipts and current review mode are persisted locally after backend success", () => {
  assert.match(boards, /const command = await this\.mathcat\.goalReview[\s\S]*commandId: String\(command\?\.command_id/);
  assert.match(boards, /afterRevision: Number\(command\?\.after_revision/);
  assert.match(boards, /await this\.mathcat\.updateResearchSettings[\s\S]*row\.reviewMode = reviewMode/);
  assert.match(boards, /this\.invalidate\(row\)[\s\S]*this\.refresh\(row, \{ fresh: true \}\)/);
});

test("goal review UI correlates the command and never regresses after completion", () => {
  assert.match(app, /String\(item\?\.causedBy\?\.id\|\|""\)===commandId/);
  assert.match(app, /if\(!next\|\|status==="completed"\)continue/);
});

test("planning suggestion selector excludes route proposals that have no live route id", () => {
  assert.match(app, /board\.routes\.filter\(route=>!route\.isProposal\)/);
});

test("planning suggestions remain visible with scope, status, and effective round", () => {
  assert.match(boards, /row\.planningSuggestions\.push/);
  assert.match(app, /function planningSuggestionReceipts/);
  assert.match(app, /已提交的规划建议/);
  assert.match(app, /最早第/);
  assert.match(app, /Planner 记录/);
});

test("collaboration dialogs cannot be dismissed while a command is in flight", () => {
  assert.match(app, /dialog\.setAttribute\("aria-busy",String\(Boolean\(busy\)\)\)/);
  assert.match(app, /dialog\.dataset\.submitting==="true"/);
  assert.match(app, /addEventListener\("cancel",\(event\)=>\{if\(dialog\.dataset\.submitting==="true"\)event\.preventDefault\(\)/);
});

test("a committed manual route remains successful when the follow-up board read fails", async () => {
  let writes = 0;
  const client = {
    async createAndExecuteRoute() {
      writes += 1;
      return { execution_started: true, route_id: "route_manual", task_id: "task_manual", _responseMeta: { project_revision: 12, event_cursor: 19 } };
    }
  };
  const service = new ResearchBoards("F:\\unused-test-runtime", client);
  const row = { id: "board_route", conversationId: "conversation_route", agent: "mathcat", remoteProjectId: "project_route", reviewMode: "strict" };
  const snapshot = { id: row.id, revision: 11, eventCursor: 18, reviewMode: "strict", status: "needs_human_review", routes: [], goals: [], budget: null };
  let reads = 0;
  service.rows = [row];
  service.refresh = async () => {
    reads += 1;
    if (reads === 1) return snapshot;
    throw new Error("temporary read outage");
  };
  service.invalidate = () => {};
  const result = await service.addRoute(row.id, { title: "局部化路线", objective: "逐素理想局部化", summary: "证明局部引理", executeImmediately: true });
  assert.equal(writes, 1);
  assert.equal(result.status, "running");
  assert.equal(result.revision, 12);
  assert.equal(result.eventCursor, 19);
  assert.equal(result.routes[0].id, "route_manual");
  assert.match(result.syncWarning, /操作已由 MathCat 接受/);
});

test("a committed route approval remains runnable when the follow-up board read fails", async () => {
  let writes = 0;
  const client = { async routeCommand() { writes += 1; return { command_id: "approve_1", _responseMeta: { project_revision: 8, event_cursor: 13 } }; } };
  const service = new ResearchBoards("F:\\unused-test-runtime", client);
  const row = { id: "board_approve", conversationId: "conversation_approve", agent: "mathcat", remoteProjectId: "project_approve", reviewMode: "strict" };
  const snapshot = { id: row.id, revision: 7, eventCursor: 12, reviewMode: "strict", status: "needs_human_review", routes: [{ id: "route_1", status: "pending", humanStatus: "pending" }] };
  let reads = 0;
  service.rows = [row];
  service.refresh = async () => ++reads === 1 ? snapshot : Promise.reject(new Error("temporary read outage"));
  service.invalidate = () => {};
  const result = await service.routeCommand(row.id, "route_1", "approve");
  assert.equal(writes, 1);
  assert.equal(result.status, "running");
  assert.equal(result.routes[0].status, "active");
  assert.equal(result.routes[0].humanStatus, "approved");
  assert.equal(result.revision, 8);
});

test("goal review waits through a stale accepted snapshot without submitting twice", async () => {
  let writes = 0;
  let statusReads = 0;
  const client = {
    async goalReview() {
      writes += 1;
      return { command_id: "goal_review_1", status: "queued", before_revision: 7, _responseMeta: { project_revision: 8, event_cursor: 13 } };
    },
    async commandStatus() {
      statusReads += 1;
      return { command_id: "goal_review_1", status: statusReads === 1 ? "queued" : "applied" };
    },
  };
  const service = new ResearchBoards("F:\\unused-test-runtime", client);
  const row = { id: "board_goal_review", conversationId: "conversation_goal_review", agent: "mathcat", remoteProjectId: "project_goal_review", reviewMode: "strict" };
  const stale = { id: row.id, revision: 7, eventCursor: 12, reviewMode: "strict", status: "needs_human_review", routes: [], goals: [], events: [], decisions: [], budget: null };
  const applied = {
    ...stale,
    revision: 9,
    eventCursor: 15,
    status: "running",
    events: [{ type: "goal_review.requested", causedBy: { kind: "command", id: "goal_review_1" } }, { type: "human_command.applied", entity: { kind: "command", id: "goal_review_1" } }]
  };
  let reads = 0;
  service.rows = [row];
  service.refresh = async () => {
    reads += 1;
    return reads === 1 ? stale : applied;
  };
  service.save = async () => {};
  service.invalidate = () => {};
  const result = await service.forceGoalReview(row.id, { focus: "重新检查交换代数主路线" });
  assert.equal(writes, 1);
  assert.equal(statusReads, 2);
  assert.equal(reads, 2);
  assert.equal(result.status, "running");
  assert.equal(result.revision, 9);
  assert.equal(result.syncWarning, undefined);
});

test("queued collaboration commands fall back to runnable optimistic state after stale reads", async () => {
  let writes = 0;
  const client = {
    async goalReview() {
      writes += 1;
      return { command_id: "goal_review_slow", status: "queued", before_revision: 4, _responseMeta: { project_revision: 5, event_cursor: 8 } };
    },
    async commandStatus() { return { command_id: "goal_review_slow", status: "queued" }; }
  };
  const service = new ResearchBoards("F:\\unused-test-runtime", client);
  const row = { id: "board_goal_review_slow", conversationId: "conversation_goal_review_slow", agent: "mathcat", remoteProjectId: "project_goal_review_slow", reviewMode: "strict" };
  const stale = { id: row.id, revision: 4, eventCursor: 7, reviewMode: "strict", status: "needs_human_review", routes: [], goals: [], events: [], decisions: [], budget: null };
  service.rows = [row];
  service.refresh = async () => stale;
  service.save = async () => {};
  service.invalidate = () => {};
  const result = await service.forceGoalReview(row.id, { focus: "强制开始新目标讨论" });
  assert.equal(writes, 1);
  assert.equal(result.status, "running");
  assert.equal(result.revision, 5);
  assert.match(result.syncWarning, /后台正在同步/);
});

test("an asynchronously failed collaboration command is not reported as optimistic success", async () => {
  const client = {
    async goalReview() { return { command_id: "goal_review_failed", status: "queued" }; },
    async commandStatus() { return { command_id: "goal_review_failed", status: "failed", error: "invalid transition" }; }
  };
  const service = new ResearchBoards("F:\\unused-test-runtime", client);
  const row = { id: "board_goal_review_failed", conversationId: "conversation_goal_review_failed", agent: "mathcat", remoteProjectId: "project_goal_review_failed", reviewMode: "strict" };
  const initial = { id: row.id, revision: 3, eventCursor: 5, reviewMode: "strict", status: "needs_human_review", routes: [], goals: [], events: [], decisions: [], budget: null };
  service.rows = [row];
  service.refresh = async () => initial;
  service.save = async () => {};
  service.invalidate = () => {};
  await assert.rejects(service.forceGoalReview(row.id, { focus: "重新梳理目标" }), /invalid transition/);
});

test("automatic review survives a committed answer followed by a read outage", async () => {
  let writes = 0;
  const client = { async answerQuestion() { writes += 1; return { command_id: "answer_1", _responseMeta: { project_revision: 6, event_cursor: 9 } }; } };
  const service = new ResearchBoards("F:\\unused-test-runtime", client);
  const row = { id: "board_auto_outage", conversationId: "conversation_auto_outage", agent: "mathcat", remoteProjectId: "project_auto_outage", reviewMode: "automatic" };
  const snapshot = {
    id: row.id, revision: 5, eventCursor: 8, reviewMode: "automatic", status: "needs_human_review", routes: [],
    decisions: [{ id: "question_1", status: "pending", options: [{ value: "reject", label: "保持原命题" }] }]
  };
  let reads = 0;
  service.rows = [row];
  service.refresh = async () => {
    reads += 1;
    if (reads <= 2) return snapshot;
    throw new Error("temporary read outage");
  };
  service.invalidate = () => {};
  const result = await service.resolveAutomaticReview(row.conversationId);
  assert.equal(writes, 1);
  assert.equal(result.status, "running");
  assert.equal(result.decisions[0].status, "answered");
  assert.equal(result.revision, 6);
  assert.match(result.syncWarning, /操作已由 MathCat 接受/);
});

test("board settings use one atomic backend mutation and persist only after success", async () => {
  const calls = [];
  const client = { async updateResearchSettings(projectId, board, input) { calls.push({ projectId, board, input }); } };
  const service = new ResearchBoards("F:\\unused-test-runtime", client);
  const row = { id: "board_1", conversationId: "conversation_1", agent: "mathcat", remoteProjectId: "project_1", reviewMode: "strict" };
  const snapshot = { revision: 11, reviewMode: "strict", budget: { maxRounds: 12, maxParallelWorkers: 3, maxMinutesPerTask: 45, maxModelCallsPerTask: 4, maxTotalModelCalls: 100 } };
  service.rows = [row];
  service.refresh = async () => snapshot;
  service.save = async () => {};
  service.invalidate = () => {};
  await service.updateSettings(row.id, { maxParallelWorkers: 6, maxMinutesPerTask: 120, reviewMode: "balanced" });
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].input, { maxParallelWorkers: 6, maxMinutesPerTask: 120, reviewMode: "balanced" });
  assert.equal(row.reviewMode, "balanced");

  row.reviewMode = "strict";
  client.updateResearchSettings = async () => { throw new Error("remote rejected"); };
  await assert.rejects(service.updateSettings(row.id, { maxParallelWorkers: 7, reviewMode: "automatic" }), /remote rejected/);
  assert.equal(row.reviewMode, "strict");
});

test("a slow settings command does not resume a deliberately paused project", async () => {
  let writes = 0;
  const client = {
    async updateResearchSettings() {
      writes += 1;
      return { command_id: "settings_paused", status: "queued", _responseMeta: { project_revision: 12 } };
    },
    async commandStatus() { return { command_id: "settings_paused", status: "queued" }; }
  };
  const service = new ResearchBoards("F:\\unused-test-runtime", client);
  const row = { id: "board_paused_settings", conversationId: "conversation_paused_settings", agent: "mathcat", remoteProjectId: "project_paused_settings", reviewMode: "strict" };
  const snapshot = {
    id: row.id,
    revision: 11,
    eventCursor: 20,
    reviewMode: "strict",
    status: "paused",
    routes: [],
    goals: [],
    events: [],
    decisions: [],
    budget: { maxParallelWorkers: 3, maxMinutesPerTask: 45 }
  };
  service.rows = [row];
  service.refresh = async () => snapshot;
  service.save = async () => {};
  service.invalidate = () => {};
  const result = await service.updateSettings(row.id, { maxParallelWorkers: 4, maxMinutesPerTask: 60, reviewMode: "balanced" });
  assert.equal(writes, 1);
  assert.equal(result.status, "paused");
  assert.equal(result.reviewMode, "balanced");
  assert.match(result.syncWarning, /后台正在同步/);
});

test("automatic review follows the backend board mode even when the local mirror is stale", async () => {
  const client = {};
  const service = new ResearchBoards("F:\\unused-test-runtime", client);
  const row = { id: "board_auto", conversationId: "conversation_auto", agent: "mathcat", remoteProjectId: "project_auto", reviewMode: "strict" };
  const automatic = { revision: 3, reviewMode: "automatic", decisions: [{ id: "q1", status: "pending", options: [{ value: "reject", label: "拒绝" }] }] };
  service.rows = [row];
  service.refresh = async () => automatic;
  let answered = 0;
  service.answerDecision = async () => { answered += 1; return { ...automatic, revision: 4, decisions: [] }; };
  const result = await service.resolveAutomaticReview(row.conversationId);
  assert.equal(answered, 1);
  assert.equal(result.revision, 4);
});
