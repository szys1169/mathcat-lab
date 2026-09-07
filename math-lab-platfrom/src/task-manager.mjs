import fs from "node:fs/promises";
import path from "node:path";
import crypto from "node:crypto";
import { runCodex } from "./executors.mjs";

const MATHCAT_TERMINAL = new Set(["success", "partial_success", "refuted", "stopped_by_human", "environment_failed", "error"]);
const MATHCAT_ACTIVE_TASK = new Set(["open", "queued", "assigned", "offered", "leased", "running", "checkpointed", "result_submitted", "ingesting"]);
const MATHCAT_ACTIVE_WORKER = new Set(["spawn_requested", "starting", "handshaking", "ready", "lease_accepted", "running", "checkpointing", "result_submitted", "draining"]);
const MATHCAT_ACTIVE_VERIFICATION = new Set(["queued", "submitted", "prechecking", "verifying"]);

function routeId(value) { return String(value?.routeId || value?.route_id || value?.id || "").trim(); }
function routeReview(value) { return String(value?.humanStatus || value?.human_review || "").toLowerCase(); }
function releasedRouteIds(board) {
  return new Set((board?.routes || [])
    .filter((route) => ["approved", "not_required"].includes(routeReview(route)))
    .map(routeId)
    .filter(Boolean));
}

export function hasReleasedMathCatWork(board) {
  const released = releasedRouteIds(board);
  if (!released.size) return false;
  const releasedCandidateIds = new Set((board?.claims || [])
    .filter((claim) => released.has(routeId(claim)))
    .map((claim) => String(claim?.candidateId || claim?.candidate_id || claim?.claimId || claim?.claim_id || claim?.id || "").trim())
    .filter(Boolean));
  const hasActiveVerification = (board?.verificationQueue || []).some((verification) => {
    const candidateId = String(verification?.candidateId || verification?.candidate_id || "").trim();
    return MATHCAT_ACTIVE_VERIFICATION.has(String(verification?.status || "").toLowerCase())
      && (released.has(routeId(verification)) || releasedCandidateIds.has(candidateId));
  });
  if (hasActiveVerification) return true;
  const activeTaskIds = new Set((board?.tasks || [])
    .filter((task) => released.has(routeId(task)) && MATHCAT_ACTIVE_TASK.has(String(task?.status || "").toLowerCase()))
    .map((task) => String(task?.taskId || task?.task_id || task?.id || "").trim())
    .filter(Boolean));
  if (activeTaskIds.size) return true;
  return (board?.workers || []).some((worker) => {
    const workerRoute = routeId(worker);
    const taskId = String(worker?.currentTaskId || worker?.current_task_id || "").trim();
    return MATHCAT_ACTIVE_WORKER.has(String(worker?.status || "").toLowerCase())
      && (released.has(workerRoute) || activeTaskIds.has(taskId));
  });
}

function hasPendingAutomaticReview(board) {
  return String(board?.status || "").toLowerCase() === "needs_human_review"
    && board?.reviewMode === "automatic"
    && (board?.decisions || []).some((item) => item.status === "pending");
}

export function mathCatWatcherComplete(board) {
  const status = String(board?.status || "").toLowerCase();
  return MATHCAT_TERMINAL.has(status)
    || (status === "needs_human_review" && !hasReleasedMathCatWork(board) && !hasPendingAutomaticReview(board));
}
const wait = (milliseconds, signal) => new Promise((resolve, reject) => {
  const timer = setTimeout(resolve, milliseconds);
  signal?.addEventListener("abort", () => { clearTimeout(timer); reject(Object.assign(new Error("Aborted"), { name: "AbortError" })); }, { once: true });
});

export function mathCatResult(board) {
  const labels = { success: "研究完成", partial_success: "得到部分成果", refuted: "命题被反驳", needs_human_review: "等待人工协作", stopped_by_human: "已由用户中止", environment_failed: "研究环境失败", error: "研究出错" };
  const counts = [`${board.routes?.length || 0} 条路线`, `${board.goals?.length || 0} 个目标`, `${board.claims?.length || 0} 条结论`];
  const openQuestions = (board.decisions || []).filter((item) => item.status === "pending").length;
  const next = board.status === "needs_human_review" ? board.reviewMode === "automatic" ? "\n\n自动模式已处理可回答的提问，但 MathCat 仍因无法安全自动恢复的异常停止；请在研究白板检查原因。" : `\n\n研究白板中有 ${openQuestions || "待处理"} 项需要你确认；处理后 MathCat 会继续运行。` : "";
  return { executor: "mathcat", content: `MathCat ${labels[board.status] || `已停止（${board.status}）`}。本轮形成 ${counts.join("、")}。你可以切换到“研究白板”查看证明树、依赖图和完整过程。${next}`, artifacts: [] };
}

export async function runMathCatResearch({ researchBoards, conversationId, signal, onProgress, pollIntervalMs = 2_000, startProject = true, initialBoard = null }) {
  if (!researchBoards) throw new Error("MathCat research board service is unavailable.");
  onProgress?.(startProject
    ? { summary: "正在启动 MathCat 研究循环", detail: "已绕过一次性对话包装，研究将由 MathCat 后端持续调度。" }
    : { summary: "正在恢复 MathCat 进度监视", detail: "人工操作已提交，正在继续读取规划、任务、Worker 与验证状态。" });
  let board = startProject
    ? await researchBoards.startByConversation(conversationId)
    : initialBoard || await researchBoards.viewByConversation(conversationId);
  let lastSignature = "";
  let failures = 0;
  let automaticReviewStalls = 0;
  while (true) {
    if (signal?.aborted) throw Object.assign(new Error("Aborted"), { name: "AbortError" });
    if (board.status === "needs_human_review" && board.reviewMode === "automatic") {
      const pending = (board.decisions || []).filter((item) => item.status === "pending").length;
      if (pending) {
        onProgress?.({ summary: "MathCat 正在自动处理审核节点", detail: `自动运行模式将为 ${pending} 个待处理问题采用保守选项。` });
        const before = `${board.revision}:${pending}`;
        const next = await researchBoards.resolveAutomaticReview(conversationId);
        const remaining = (next.decisions || []).filter((item) => item.status === "pending").length;
        automaticReviewStalls = `${next.revision}:${remaining}` === before ? automaticReviewStalls + 1 : 0;
        board = next;
        if (automaticReviewStalls >= 3) throw new Error("自动审核连续三次没有推进；已停止轮询，请在研究白板检查该提问或切换人工参与程度。");
        if (automaticReviewStalls) await wait(pollIntervalMs, signal);
        continue;
      }
    }
    if (mathCatWatcherComplete(board)) break;
    const signature = [board.status, board.revision, board.routes?.length, board.tasks?.length, board.workers?.length, board.events?.length].join(":");
    if (signature !== lastSignature) {
      const current = `MathCat ${board.status === "running" ? "正在研究" : board.status === "needs_human_review" && hasReleasedMathCatWork(board) ? "正在执行已批准路线，其他路线仍待审核" : `状态：${board.status}`} · 第 ${board.summary?.current_round ?? board.currentRound ?? 0} 轮`;
      const latest = board.events?.at(-1)?.text || `白板修订 ${board.revision}`;
      onProgress?.({ summary: current, detail: latest });
      lastSignature = signature;
    }
    await wait(pollIntervalMs, signal);
    try {
      board = await researchBoards.viewByConversation(conversationId);
      failures = 0;
    } catch (error) {
      failures += 1;
      onProgress?.({ summary: "MathCat 仍在后台研究，正在重新连接白板", detail: error.message });
      if (failures >= 5) throw new Error(`连续无法读取 MathCat 研究状态：${error.message}`);
    }
  }
  onProgress?.({ summary: `MathCat ${board.status}`, detail: board.events?.at(-1)?.text || `研究以 ${board.status} 状态结束。` });
  return mathCatResult(board);
}

export function parseChoiceResponse(raw) {
  const match = String(raw).match(/```math-lab-choice\s*([\s\S]*?)```/i);
  if (!match) return { content: raw, choice: null };
  try {
    const value = JSON.parse(match[1]);
    if (!String(value.question || "").trim() || !Array.isArray(value.options) || value.options.length < 2 || value.options.length > 5) throw new Error("invalid choice request");
    const options = value.options.map((item, index) => ({ label: String(item.label || `选项 ${index + 1}`), value: String(item.value || ""), reason: String(item.reason || "") })).filter((item) => item.value);
    if (options.length < 2) throw new Error("not enough valid options");
    const proposed = Number.isInteger(value.recommendedIndex) && value.recommendedIndex >= 0 && value.recommendedIndex < options.length ? value.recommendedIndex : null;
    const recommendationReason = String(value.recommendationReason || "").trim();
    const recommendedIndex = proposed != null && recommendationReason ? proposed : null;
    return { content: String(raw).replace(match[0], "").trim() || "需要你选择一个输入后才能继续。", choice: { question: String(value.question), options, recommendedIndex, recommendationReason: recommendedIndex == null ? "" : recommendationReason, selectedIndex: null } };
  } catch { return { content: raw, choice: null }; }
}

export class TaskManager {
  constructor({ store, config, capabilities, researchBoards = null }) { this.store = store; this.config = config; this.capabilities = capabilities; this.researchBoards = researchBoards; this.running = new Map(); }
  getActivity(conversationId) {
    const task = this.running.get(conversationId);
    if (!task) return { running: false, current: "", lines: [], elapsedSeconds: 0 };
    const elapsed = Math.max(0, Math.floor((Date.now() - task.activity.startedAt) / 1000));
    const lines = task.activity.lines.slice(-4);
    return { running: true, current: lines.at(-1) || "正在启动任务", lines: lines.slice(0, -1), details: task.activity.details.slice(-12), elapsedSeconds: elapsed };
  }
  async cancel(conversationId) {
    const task = this.running.get(conversationId);
    if (!task) throw new Error("This conversation has no running task.");
    task.activity.lines.push("正在中止当前任务…");
    if (task.cancel) await task.cancel();
    task.controller.abort();
    return { accepted: true };
  }
  async recoverInterrupted() {
    const mathCatConversationIds = new Set((this.researchBoards?.rows || []).filter((row) => row.agent === "mathcat" && row.remoteProjectId).map((row) => row.conversationId));
    const candidates = this.store.state.conversations.filter((conversation) => conversation.status === "running" || (conversation.status === "failed" && mathCatConversationIds.has(conversation.id)) || (conversation.status === "failed" && conversation.error === "平台重启中断了上一次任务，请重新发送。") || (conversation.status === "idle" && mathCatConversationIds.has(conversation.id)));
    let mathCatAvailable = true;
    if (candidates.some((conversation) => mathCatConversationIds.has(conversation.id)) && typeof this.researchBoards?.mathcat?.health === "function") {
      try { await this.researchBoards.mathcat.health(); }
      catch { mathCatAvailable = false; }
    }
    for (const conversation of candidates) {
      // v2 Run recovery belongs to the Rust service; never relaunch it through v1.
      if (conversation.researchProjectId && conversation.researchContract === "mathcat-research/v2") continue;
      const legacyInterrupted = conversation.status === "failed" && conversation.error === "平台重启中断了上一次任务，请重新发送。";
      const needsLocalReconciliation = conversation.status === "running" || conversation.status === "failed";
      const mayStartCreatedProject = conversation.status === "running" || legacyInterrupted;
      const binding = this.researchBoards?.byConversation(conversation.id);
      if (binding?.agent === "mathcat" && binding.remoteProjectId) {
        if (!mathCatAvailable) {
          if (needsLocalReconciliation) await this.store.updateConversation(conversation.id, (row) => { row.status = "failed"; row.error = "无法恢复 MathCat 任务：MathCat 后端当前不可用"; });
          continue;
        }
        try {
          const board = await this.researchBoards.viewByConversation(conversation.id);
          if (mathCatWatcherComplete(board)) {
            if (!needsLocalReconciliation) continue;
            const result = mathCatResult(board);
            await this.store.updateConversation(conversation.id, (row) => {
              row.status = "idle";
              row.error = null;
              row.messages.push({ id: crypto.randomUUID(), role: "assistant", content: result.content, createdAt: new Date().toISOString(), executor: result.executor, capabilityId: "rethlas-research", permission: "workspace-write", artifacts: [], recovered: true });
            });
          } else {
            if (board.status === "created") {
              if (!mayStartCreatedProject) continue;
              await this.store.updateConversation(conversation.id, (row) => { row.status = "running"; row.error = null; });
              const workspace = this.store.workspaceFor(conversation);
              const capability = this.capabilities.find((item) => item.id === "rethlas-research") || null;
              this.launch({ conversationId: conversation.id, workspace, capability, researchAgent: { id: "mathcat", name: "MathCat" }, conversation: { ...conversation, messages: [...conversation.messages] }, text: conversation.messages.findLast((item) => item.role === "user")?.content || conversation.title, permission: "workspace-write", taskDir: path.join(workspace.path, ".platform", "tasks", `${conversation.id}-recovered-${Date.now()}`), runner: runMathCatResearch, isMathCat: true });
            } else await this.resumeMathCatWatcher(conversation.id, board);
          }
          continue;
        } catch (error) {
          if (needsLocalReconciliation) await this.store.updateConversation(conversation.id, (row) => { row.status = "failed"; row.error = `无法恢复 MathCat 任务：${error.message}`; });
          else console.error(`Failed to inspect idle MathCat task ${conversation.id} during recovery:`, error);
          continue;
        }
      }
      await this.store.updateConversation(conversation.id, (row) => { row.status = "failed"; row.error = "平台重启中断了上一次 Codex CLI 任务，请重新发送。"; });
    }
  }
  async resumeMathCatWatcher(conversationId, currentBoard = null) {
    if (this.store.getConversation(conversationId)?.researchProjectId) return { accepted: false, conversationId, reason: "v2_project" };
    if (this.running.has(conversationId)) {
      this.running.get(conversationId).resumeRequested = true;
      return { accepted: true, conversationId, alreadyRunning: true };
    }
    const binding = this.researchBoards?.byConversation(conversationId);
    if (binding?.agent !== "mathcat" || !binding.remoteProjectId) return { accepted: false, conversationId, reason: "not_mathcat" };
    const board = currentBoard || await this.researchBoards.viewByConversation(conversationId);
    if (board?.status !== "running" && !(board?.status === "needs_human_review" && (hasReleasedMathCatWork(board) || hasPendingAutomaticReview(board)))) return { accepted: false, conversationId, reason: `status_${board?.status || "unknown"}` };
    const conversation = this.store.getConversation(conversationId);
    if (!conversation) return { accepted: false, conversationId, reason: "conversation_not_found" };
    const workspace = this.store.workspaceFor(conversation);
    if (!workspace) return { accepted: false, conversationId, reason: "workspace_unavailable" };
    const capability = this.capabilities.find((item) => item.id === "rethlas-research") || null;
    const permission = [...conversation.messages].reverse().find((message) => ["workspace-write", "read-only"].includes(message.permission))?.permission || "workspace-write";
    await this.store.updateConversation(conversationId, (row) => { row.status = "running"; row.error = null; });
    const taskDir = path.join(workspace.path, ".platform", "tasks", `${conversationId}-resumed-${Date.now()}`);
    // A mutation may have committed even when its immediate fresh board read
    // failed. Reuse the accepted/optimistic board for the first watcher tick so
    // that the existing reconnect loop owns subsequent transient read errors.
    const runner = (input) => runMathCatResearch({ ...input, startProject: false, initialBoard: board });
    return this.launch({
      conversationId,
      workspace,
      capability,
      researchAgent: { id: "mathcat", name: "MathCat" },
      conversation: { ...conversation, messages: [...conversation.messages] },
      text: conversation.messages.findLast((item) => item.role === "user")?.content || conversation.title,
      permission,
      taskDir,
      runner,
      isMathCat: true,
      capabilityId: "rethlas-research"
    });
  }
  async start({ conversationId, text, executor, capabilityId, researchAgent = null, permission }) {
    if (this.running.has(conversationId)) throw new Error("This conversation already has a running task.");
    const conversation = this.store.getConversation(conversationId); if (!conversation) throw new Error("Unknown conversation.");
    if (conversation.researchProjectId) throw Object.assign(new Error("此对话已关联 MathCat 研究项目，请从研究白板继续；不会启动旧版对话任务。"), { status: 409 });
    const workspace = this.store.workspaceFor(conversation); if (!workspace) throw new Error("Workspace is unavailable.");
    const historyConversation = { ...conversation, messages: [...conversation.messages] };
    const capability = capabilityId ? this.capabilities.find((item) => item.id === capabilityId) : null; if (capabilityId && !capability) throw new Error("Unknown capability.");
    await this.store.updateConversation(conversationId, (row) => { row.status = "running"; row.messages.push({ id: crypto.randomUUID(), role: "user", content: String(text), createdAt: new Date().toISOString(), executor, capabilityId: capabilityId || null, researchAgent: researchAgent?.id || null }); if (row.messages.length === 1) row.title = String(text).slice(0, 40); });
    if (executor !== "codex") throw new Error("Only the Codex executor is available.");
    const taskDir = path.join(workspace.path, ".platform", "tasks", `${conversationId}-${Date.now()}`);
    const isMathCat = capabilityId === "rethlas-research" && researchAgent?.id === "mathcat";
    const runner = isMathCat ? runMathCatResearch : runCodex;
    return this.launch({ conversationId, workspace, capability, researchAgent, conversation: historyConversation, text, permission, taskDir, runner, isMathCat, capabilityId });
  }
  launch({ conversationId, workspace, capability, researchAgent, conversation, text, permission, taskDir, runner, isMathCat, capabilityId = capability?.id || null }) {
    if (this.store.getConversation(conversationId)?.researchProjectId) throw Object.assign(new Error("此对话已关联 MathCat 研究项目，旧任务不能绕过研究白板启动。"), { status: 409 });
    const activity = { startedAt: Date.now(), lines: [], details: [] };
    const report = (update) => { const summary = String(update?.summary || update || "").trim(); const detail = String(update?.detail || summary).trim(); if (summary && activity.lines.at(-1) !== summary) activity.lines.push(summary); if (activity.lines.length > 20) activity.lines.shift(); if (detail && activity.details.at(-1) !== detail) activity.details.push(detail); if (activity.details.length > 40) activity.details.shift(); };
    report(capability ? `正在准备能力：${capability.name}` : "正在准备普通对话");
    const controller = new AbortController();
    const executor = isMathCat ? "mathcat" : "codex";
    const task = { activity, controller, promise: null, resumeRequested: false, cancel: isMathCat ? () => this.researchBoards.stopByConversation(conversationId) : null }; this.running.set(conversationId, task);
    const promise = runner({ config: this.config, workspace, capability, capabilities: this.capabilities, researchAgent, researchBoards: this.researchBoards, conversationId, conversation, text, permission, taskDir, signal: controller.signal, onProgress: report }).then(async (result) => {
      report("正在整理最终回答和成果文件");
      const parsed = parseChoiceResponse(result.content);
      await this.store.updateConversation(conversationId, (row) => { row.status = "idle"; row.messages.push({ id: crypto.randomUUID(), role: "assistant", content: parsed.content, createdAt: new Date().toISOString(), executor: result.executor, capabilityId: capabilityId || null, permission, artifacts: result.artifacts || [], choice: parsed.choice, activityDetails: [...activity.details] }); });
    }).catch(async (error) => {
      await fs.mkdir(taskDir, { recursive: true });
      if (controller.signal.aborted || error.name === "AbortError") {
        await this.store.updateConversation(conversationId, (row) => { row.status = "cancelled"; row.messages.push({ id: crypto.randomUUID(), role: "assistant", content: "任务已由用户中止。已生成的工作区文件予以保留。", createdAt: new Date().toISOString(), executor, capabilityId: capabilityId || null, activityDetails: [...activity.details] }); });
        return;
      }
      await fs.writeFile(path.join(taskDir, "error.txt"), error.stack || error.message, "utf8"); await this.store.updateConversation(conversationId, (row) => { row.status = "failed"; row.messages.push({ id: crypto.randomUUID(), role: "assistant", content: `任务失败：${error.message}`, createdAt: new Date().toISOString(), executor, capabilityId: capabilityId || null, error: true, artifacts: [path.join(taskDir, "error.txt")], activityDetails: [...activity.details] }); });
    }).finally(async () => {
      if (this.running.get(conversationId) !== task) return;
      this.running.delete(conversationId);
      if (isMathCat && task.resumeRequested) {
        await this.resumeMathCatWatcher(conversationId).catch((error) => console.error(`Failed to resume MathCat watcher after concurrent update for ${conversationId}:`, error));
      }
    });
    task.promise = promise; return { accepted: true, conversationId, taskDir };
  }
  async choose({ conversationId, messageId, optionIndex }) {
    if (this.running.has(conversationId)) throw new Error("This conversation already has a running task.");
    const conversation = this.store.getConversation(conversationId); if (!conversation) throw new Error("Unknown conversation.");
    if (conversation.researchProjectId) throw Object.assign(new Error("此对话已关联 MathCat 研究项目，请从研究白板处理；旧选择卡不会启动任务。"), { status: 409 });
    const message = conversation.messages.find((item) => item.id === messageId); if (!message?.choice) throw new Error("Choice request not found.");
    const index = Number(optionIndex); const option = message.choice.options[index]; if (!option) throw new Error("Unknown choice option.");
    if (message.choice.selectedIndex != null) throw new Error("This choice has already been submitted.");
    await this.store.updateConversation(conversationId, (row) => { const target = row.messages.find((item) => item.id === messageId); target.choice.selectedIndex = index; });
    return this.start({ conversationId, text: `我选择“${option.label}”。\n选择值：${option.value}\n请使用这个选择继续并完成刚才暂停的任务。`, executor: "codex", capabilityId: message.capabilityId || null, permission: message.permission || "workspace-write" });
  }
}
