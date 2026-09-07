import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import { runMathCatResearch, TaskManager, hasReleasedMathCatWork, mathCatWatcherComplete } from "../src/task-manager.mjs";

const serverSource = await fs.readFile(new URL("../src/server.mjs", import.meta.url), "utf8");

test("MathCat runner starts the backend project and waits for a terminal state", async () => {
  const states = [
    { status: "created", revision: 2, routes: [], goals: [], claims: [], events: [] },
    { status: "running", revision: 3, routes: [{ id: "r1" }], goals: [{ id: "g1" }], claims: [], tasks: [], workers: [], summary: { current_round: 1 }, events: [{ text: "规划研究路线" }] },
    { status: "success", revision: 4, routes: [{ id: "r1" }], goals: [{ id: "g1" }], claims: [{ id: "c1" }], events: [{ text: "证明完成" }] }
  ];
  let refresh = 0;
  const researchBoards = {
    async startByConversation(id) { assert.equal(id, "conversation_1"); return states[0]; },
    async viewByConversation() { refresh += 1; return states[Math.min(refresh, states.length - 1)]; }
  };
  const updates = [];
  const result = await runMathCatResearch({ researchBoards, conversationId: "conversation_1", signal: new AbortController().signal, onProgress: (update) => updates.push(update), pollIntervalMs: 1 });
  assert.equal(result.executor, "mathcat");
  assert.match(result.content, /研究完成/);
  assert.match(result.content, /1 条结论/);
  assert.ok(updates.some((item) => item.summary.includes("正在研究")));
});

test("MathCat runner keeps a human-review result visible instead of reporting a short-process failure", async () => {
  const board = { status: "needs_human_review", revision: 6, routes: [{ id: "r1" }], goals: [], claims: [], decisions: [{ status: "pending" }], events: [{ text: "请选择研究路线" }] };
  const result = await runMathCatResearch({ researchBoards: { startByConversation: async () => board }, conversationId: "conversation_2", signal: new AbortController().signal, pollIntervalMs: 1 });
  assert.match(result.content, /等待人工协作/);
  assert.match(result.content, /1 项需要你确认/);
});

test("MathCat runner keeps watching approved route work while other routes remain under review", async () => {
  const routeStates = [
    {
      status: "needs_human_review", revision: 7,
      routes: [{ id: "approved", humanStatus: "approved" }, { id: "pending", humanStatus: "pending" }],
      tasks: [{ task_id: "task_1", route_id: "approved", status: "running" }],
      workers: [{ worker_id: "worker_1", current_task_id: "task_1", current_route_id: "approved", status: "running" }],
      goals: [], claims: [], decisions: [], summary: { current_round: 1 }, events: [{ text: "已批准路线的 Worker 正在运行" }]
    },
    {
      status: "needs_human_review", revision: 8,
      routes: [{ id: "approved", humanStatus: "approved" }, { id: "pending", humanStatus: "pending" }],
      tasks: [{ task_id: "task_1", route_id: "approved", status: "completed" }],
      workers: [{ worker_id: "worker_1", current_task_id: null, current_route_id: null, status: "exited" }],
      goals: [], claims: [], decisions: [{ status: "pending" }], summary: { current_round: 1 }, events: [{ text: "已批准路线的当前任务完成" }]
    }
  ];
  let reads = 0;
  const updates = [];
  const result = await runMathCatResearch({
    researchBoards: {
      async startByConversation() { return { ...routeStates[0], tasks: [{ ...routeStates[0].tasks[0], status: "queued" }] }; },
      async viewByConversation() { return routeStates[Math.min(reads++, routeStates.length - 1)]; }
    },
    conversationId: "partial-release",
    signal: new AbortController().signal,
    onProgress: (update) => updates.push(update),
    pollIntervalMs: 1
  });
  assert.equal(reads, 2);
  assert.match(result.content, /等待人工协作/);
  assert.ok(updates.some((item) => item.summary.includes("正在执行已批准路线")));
});

test("only work belonging to an approved route keeps the MathCat watcher alive", () => {
  const base = {
    status: "needs_human_review",
    routes: [{ id: "approved", humanStatus: "approved" }, { id: "pending", humanStatus: "pending" }],
    workers: []
  };
  const released = { ...base, tasks: [{ task_id: "approved_task", route_id: "approved", status: "queued" }] };
  const blocked = { ...base, tasks: [{ task_id: "pending_task", route_id: "pending", status: "queued" }] };
  assert.equal(hasReleasedMathCatWork(released), true);
  assert.equal(mathCatWatcherComplete(released), false);
  assert.equal(hasReleasedMathCatWork(blocked), false);
  assert.equal(mathCatWatcherComplete(blocked), true);
});

test("verification for an approved route keeps the MathCat watcher alive while sibling routes wait", () => {
  const base = {
    status: "needs_human_review",
    routes: [{ id: "approved", humanStatus: "approved" }, { id: "pending", humanStatus: "pending" }],
    tasks: [],
    workers: [],
    claims: [
      { id: "approved_candidate", routeId: "approved" },
      { id: "pending_candidate", routeId: "pending" }
    ]
  };
  const released = { ...base, verificationQueue: [{ candidate_id: "approved_candidate", status: "verifying" }] };
  const blocked = { ...base, verificationQueue: [{ candidate_id: "pending_candidate", status: "submitted" }] };
  assert.equal(hasReleasedMathCatWork(released), true);
  assert.equal(mathCatWatcherComplete(released), false);
  assert.equal(hasReleasedMathCatWork(blocked), false);
  assert.equal(mathCatWatcherComplete(blocked), true);
});

test("automatic review mode resolves pending questions and continues", async () => {
  const review = { status: "needs_human_review", reviewMode: "automatic", revision: 6, routes: [], goals: [], claims: [], decisions: [{ id: "q1", status: "pending", options: [{ value: "reject", label: "拒绝有歧义的形式化" }] }], events: [] };
  const success = { status: "success", reviewMode: "automatic", revision: 7, routes: [], goals: [], claims: [], decisions: [], events: [{ text: "继续后完成" }] };
  let resolved = 0;
  const researchBoards = { async startByConversation(){return review;}, async resolveAutomaticReview(){resolved += 1; return success;} };
  const result = await runMathCatResearch({ researchBoards, conversationId: "auto", signal: new AbortController().signal, pollIntervalMs: 1 });
  assert.equal(resolved, 1);
  assert.match(result.content, /研究完成/);
});

test("environment failure is a terminal MathCat result", async () => {
  const board = { status: "environment_failed", revision: 9, routes: [], goals: [], claims: [], decisions: [], events: [{ text: "Codex 环境不可用" }] };
  assert.equal(mathCatWatcherComplete(board), true);
  const result = await runMathCatResearch({ researchBoards: { startByConversation: async () => board }, conversationId: "environment-failed", signal: new AbortController().signal, pollIntervalMs: 1 });
  assert.match(result.content, /研究环境失败/);
});

test("TaskManager resumes an automatic pending decision even when no route work is released", async () => {
  const conversation = { id: "automatic_resume", title: "自动恢复", status: "idle", error: null, messages: [{ id: "m1", role: "user", content: "继续证明" }] };
  const review = { status: "needs_human_review", reviewMode: "automatic", revision: 8, routes: [], goals: [], claims: [], decisions: [{ id: "q1", status: "pending", options: [{ value: "reject", label: "保守拒绝" }] }], events: [] };
  const success = { status: "success", reviewMode: "automatic", revision: 9, routes: [], goals: [], claims: [], decisions: [], events: [{ text: "自动处理后继续完成" }] };
  let resolved = 0;
  const researchBoards = {
    byConversation: () => ({ agent: "mathcat", remoteProjectId: "project_auto" }),
    async viewByConversation() { return review; },
    async resolveAutomaticReview() { resolved += 1; return success; },
    async stopByConversation() {}
  };
  const store = {
    getConversation: () => conversation,
    workspaceFor: () => ({ path: "F:\\virtual-mathcat-workspace" }),
    async updateConversation(_id, update) { update(conversation); }
  };
  const manager = new TaskManager({ store, config: {}, capabilities: [{ id: "rethlas-research", name: "研究喵" }], researchBoards });
  const response = await manager.resumeMathCatWatcher(conversation.id, review);
  assert.equal(response.accepted, true);
  await manager.running.get(conversation.id).promise;
  assert.equal(resolved, 1);
  assert.equal(conversation.status, "idle");
  assert.match(conversation.messages.at(-1).content, /研究完成/);
});

test("a resumed MathCat watcher observes the project without issuing another start command", async () => {
  let starts = 0;
  let reads = 0;
  const board = { status: "success", revision: 9, routes: [], goals: [], claims: [], events: [{ text: "人工操作后研究完成" }] };
  const result = await runMathCatResearch({
    researchBoards: {
      async startByConversation() { starts += 1; return board; },
      async viewByConversation() { reads += 1; return board; }
    },
    conversationId: "resumed",
    signal: new AbortController().signal,
    pollIntervalMs: 1,
    startProject: false
  });
  assert.equal(starts, 0);
  assert.equal(reads, 1);
  assert.match(result.content, /研究完成/);
});

test("TaskManager restores a visible watcher after human approval makes MathCat runnable", async () => {
  const conversation = {
    id: "conversation_resume",
    title: "继续研究",
    workspaceId: null,
    status: "idle",
    error: null,
    messages: [
      { id: "user_1", role: "user", content: "证明 P" },
      { id: "assistant_1", role: "assistant", content: "等待批准", permission: "read-only" }
    ]
  };
  let releaseRead;
  const readGate = new Promise((resolve) => { releaseRead = resolve; });
  const completed = { status: "success", revision: 12, routes: [{ id: "route_1" }], goals: [], claims: [], events: [{ text: "任务完成" }] };
  const researchBoards = {
    byConversation(id) { assert.equal(id, conversation.id); return { agent: "mathcat", remoteProjectId: "project_1" }; },
    async viewByConversation(id) { assert.equal(id, conversation.id); await readGate; return completed; },
    async startByConversation() { throw new Error("a watcher must not start the project again"); },
    async stopByConversation() {}
  };
  const store = {
    getConversation(id) { return id === conversation.id ? conversation : null; },
    workspaceFor() { return { path: "F:\\virtual-mathcat-workspace" }; },
    async updateConversation(id, update) { assert.equal(id, conversation.id); update(conversation); }
  };
  const manager = new TaskManager({
    store,
    config: {},
    capabilities: [{ id: "rethlas-research", name: "研究喵" }],
    researchBoards
  });

  const response = await manager.resumeMathCatWatcher(conversation.id, { status: "running", revision: 11 });
  assert.equal(response.accepted, true);
  assert.equal(conversation.status, "running");
  assert.equal(manager.getActivity(conversation.id).running, true);
  const runningTask = manager.running.get(conversation.id);
  releaseRead();
  await runningTask.promise;

  assert.equal(conversation.status, "idle");
  assert.equal(conversation.messages.at(-1).executor, "mathcat");
  assert.equal(conversation.messages.at(-1).permission, "read-only");
  assert.match(conversation.messages.at(-1).content, /研究完成/);
  assert.equal(manager.getActivity(conversation.id).running, false);
});

test("a resumed watcher survives repeated reads failing immediately after a committed mutation", async () => {
  const conversation = { id: "post_commit_outage", title: "批准后恢复", status: "idle", error: null, messages: [{ id: "m1", role: "user", content: "执行已批准路线" }] };
  const accepted = { status: "running", revision: 12, routes: [{ id: "route_1", humanStatus: "approved" }], goals: [], claims: [], decisions: [], events: [{ text: "路线已批准" }] };
  const terminal = { ...accepted, status: "success", revision: 13, claims: [{ id: "fact_1" }], events: [{ text: "研究完成" }] };
  let reads = 0;
  const researchBoards = {
    byConversation: () => ({ agent: "mathcat", remoteProjectId: "project_post_commit" }),
    async viewByConversation() {
      reads += 1;
      if (reads <= 2) throw new Error("temporary read outage");
      return terminal;
    },
    async stopByConversation() {}
  };
  const store = {
    getConversation: () => conversation,
    workspaceFor: () => ({ path: "F:\\virtual-mathcat-workspace" }),
    async updateConversation(_id, update) { update(conversation); }
  };
  const manager = new TaskManager({ store, config: {}, capabilities: [{ id: "rethlas-research", name: "研究喵" }], researchBoards });
  const response = await manager.resumeMathCatWatcher(conversation.id, accepted);
  assert.equal(response.accepted, true);
  const task = manager.running.get(conversation.id);
  await task.promise;
  assert.equal(reads, 3);
  assert.equal(conversation.status, "idle");
  assert.equal(conversation.error, null);
  assert.match(conversation.messages.at(-1).content, /研究完成/);
});

test("TaskManager does not create a watcher while another human decision still blocks MathCat", async () => {
  const manager = new TaskManager({
    store: {},
    config: {},
    capabilities: [],
    researchBoards: { byConversation: () => ({ agent: "mathcat", remoteProjectId: "project_1" }) }
  });
  const response = await manager.resumeMathCatWatcher("conversation_waiting", { status: "needs_human_review" });
  assert.equal(response.accepted, false);
  assert.equal(response.reason, "status_needs_human_review");
  assert.equal(manager.getActivity("conversation_waiting").running, false);
});

test("a concurrent whiteboard mutation restarts the watcher after the old watcher finalizes", async () => {
  const conversation = { id: "watcher_race", title: "竞态回归", status: "running", error: null, messages: [{ id: "m1", role: "user", content: "证明 P" }] };
  let releaseFinalize;
  let finalizeStartedResolve;
  const finalizeGate = new Promise((resolve) => { releaseFinalize = resolve; });
  const finalizeStarted = new Promise((resolve) => { finalizeStartedResolve = resolve; });
  let releaseSecondRead;
  const secondReadGate = new Promise((resolve) => { releaseSecondRead = resolve; });
  let boardReads = 0;
  let conversationWrites = 0;
  const researchBoards = {
    byConversation: () => ({ agent: "mathcat", remoteProjectId: "project_race" }),
    async viewByConversation() {
      boardReads += 1;
      if (boardReads === 1) return { status: "running", revision: 10, routes: [], goals: [], claims: [], decisions: [], events: [] };
      await secondReadGate;
      return { status: "environment_failed", revision: 11, routes: [], goals: [], claims: [], decisions: [], events: [{ text: "测试结束" }] };
    },
    async stopByConversation() {}
  };
  const store = {
    getConversation: () => conversation,
    workspaceFor: () => ({ path: "F:\\virtual-mathcat-workspace" }),
    async updateConversation(_id, update) {
      conversationWrites += 1;
      if (conversationWrites === 1) { finalizeStartedResolve(); await finalizeGate; }
      update(conversation);
    }
  };
  const manager = new TaskManager({ store, config: {}, capabilities: [{ id: "rethlas-research", name: "研究喵" }], researchBoards });
  const first = manager.launch({ conversationId: conversation.id, workspace: store.workspaceFor(), capability: { id: "rethlas-research", name: "研究喵" }, researchAgent: { id: "mathcat", name: "MathCat" }, conversation, text: "证明 P", permission: "workspace-write", taskDir: "F:\\virtual-mathcat-workspace\\task", runner: async () => ({ executor: "mathcat", content: "旧监视已到终态", artifacts: [] }), isMathCat: true, capabilityId: "rethlas-research" });
  const firstTask = manager.running.get(conversation.id);
  await finalizeStarted;
  const concurrent = await manager.resumeMathCatWatcher(conversation.id, { status: "running", revision: 10 });
  assert.equal(concurrent.alreadyRunning, true);
  releaseFinalize();
  await firstTask.promise;
  const secondTask = manager.running.get(conversation.id);
  assert.ok(secondTask, "a fresh watcher should replace the finalized watcher");
  releaseSecondRead();
  await secondTask.promise;
  assert.equal(manager.running.has(conversation.id), false);
  assert.match(conversation.messages.at(-1).content, /研究环境失败/);
  assert.equal(first.accepted, true);
});

test("startup recovery does not mark an idle MathCat conversation failed when the backend is offline", async () => {
  const conversation = { id: "idle_offline", title: "历史研究", status: "idle", error: null, messages: [] };
  let writes = 0;
  const manager = new TaskManager({
    store: { state: { conversations: [conversation] }, async updateConversation() { writes += 1; } },
    config: {},
    capabilities: [],
    researchBoards: { rows: [{ conversationId: conversation.id, agent: "mathcat", remoteProjectId: "project_offline" }], byConversation: () => ({ agent: "mathcat", remoteProjectId: "project_offline" }), async viewByConversation() { throw new Error("offline"); } }
  });
  const originalError = console.error;
  console.error = () => {};
  try { await manager.recoverInterrupted(); } finally { console.error = originalError; }
  assert.equal(writes, 0);
  assert.equal(conversation.status, "idle");
});

test("startup recovery never starts an idle project that was only created", async () => {
  const conversation = { id: "idle_created", title: "尚未启动", status: "idle", error: null, messages: [] };
  let starts = 0;
  const researchBoards = {
    rows: [{ conversationId: conversation.id, agent: "mathcat", remoteProjectId: "project_created" }],
    mathcat: { async health() { return { status: "ok" }; } },
    byConversation: () => ({ agent: "mathcat", remoteProjectId: "project_created" }),
    async viewByConversation() { return { status: "created", revision: 1, routes: [], goals: [], claims: [], decisions: [], events: [] }; },
    async startByConversation() { starts += 1; }
  };
  const manager = new TaskManager({ store: { state: { conversations: [conversation] } }, config: {}, capabilities: [], researchBoards });
  await manager.recoverInterrupted();
  assert.equal(starts, 0);
  assert.equal(manager.running.size, 0);
  assert.equal(conversation.status, "idle");
});

test("startup recovery restores an idle watcher when the remote project is still running", async () => {
  const conversation = { id: "idle_running", title: "仍在研究", status: "idle", error: null, messages: [{ id: "m1", role: "user", content: "证明 P", permission: "workspace-write" }] };
  let reads = 0;
  const researchBoards = {
    rows: [{ conversationId: conversation.id, agent: "mathcat", remoteProjectId: "project_running" }],
    mathcat: { async health() { return { status: "ok" }; } },
    byConversation: () => ({ agent: "mathcat", remoteProjectId: "project_running" }),
    async viewByConversation() {
      reads += 1;
      return reads === 1
        ? { status: "running", revision: 5, routes: [], goals: [], claims: [], decisions: [], events: [] }
        : { status: "environment_failed", revision: 6, routes: [], goals: [], claims: [], decisions: [], events: [{ text: "恢复监视后结束" }] };
    },
    async stopByConversation() {}
  };
  const store = {
    state: { conversations: [conversation] },
    getConversation: () => conversation,
    workspaceFor: () => ({ path: "F:\\virtual-mathcat-workspace" }),
    async updateConversation(_id, update) { update(conversation); }
  };
  const manager = new TaskManager({ store, config: {}, capabilities: [{ id: "rethlas-research", name: "研究喵" }], researchBoards });
  await manager.recoverInterrupted();
  const task = manager.running.get(conversation.id);
  assert.ok(task);
  await task.promise;
  assert.match(conversation.messages.at(-1).content, /研究环境失败/);
});

test("startup recovery reconciles a locally failed watcher with the remote terminal result", async () => {
  const conversation = { id: "failed_then_success", title: "网络中断", status: "failed", error: null, messages: [{ id: "m1", role: "assistant", content: "任务失败：连续无法读取 MathCat 研究状态" }] };
  const researchBoards = {
    rows: [{ conversationId: conversation.id, agent: "mathcat", remoteProjectId: "project_success" }],
    mathcat: { async health() { return { status: "ok" }; } },
    byConversation: () => ({ agent: "mathcat", remoteProjectId: "project_success" }),
    async viewByConversation() { return { status: "success", revision: 22, routes: [], goals: [], claims: [{ id: "fact_1" }], decisions: [], events: [{ text: "远端研究完成" }] }; }
  };
  const store = { state: { conversations: [conversation] }, async updateConversation(_id, update) { update(conversation); } };
  const manager = new TaskManager({ store, config: {}, capabilities: [], researchBoards });
  await manager.recoverInterrupted();
  assert.equal(conversation.status, "idle");
  assert.equal(conversation.messages.at(-1).recovered, true);
  assert.match(conversation.messages.at(-1).content, /研究完成/);
});

test("route commands and human-question answers both reconnect the MathCat watcher", () => {
  assert.match(serverSource, /async function resumeBoardWatcher[\s\S]*tasks\.resumeMathCatWatcher\(conversationId,board\)/);
  assert.match(serverSource, /researchBoards\.routeCommand[\s\S]*resumeBoardWatcher/);
  assert.match(serverSource, /researchBoards\.answerDecision[\s\S]*resumeBoardWatcher/);
});
