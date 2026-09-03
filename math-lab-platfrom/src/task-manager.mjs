import fs from "node:fs/promises";
import path from "node:path";
import crypto from "node:crypto";
import { runCodex } from "./executors.mjs";

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
  constructor({ store, config, capabilities }) { this.store = store; this.config = config; this.capabilities = capabilities; this.running = new Map(); }
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
    task.controller.abort();
    return { accepted: true };
  }
  async start({ conversationId, text, executor, capabilityId, researchAgent = null, permission }) {
    if (this.running.has(conversationId)) throw new Error("This conversation already has a running task.");
    const conversation = this.store.getConversation(conversationId); if (!conversation) throw new Error("Unknown conversation."); const workspace = this.store.workspaceFor(conversation); if (!workspace) throw new Error("Workspace is unavailable.");
    const historyConversation = { ...conversation, messages: [...conversation.messages] };
    const capability = capabilityId ? this.capabilities.find((item) => item.id === capabilityId) : null; if (capabilityId && !capability) throw new Error("Unknown capability.");
    await this.store.updateConversation(conversationId, (row) => { row.status = "running"; row.messages.push({ id: crypto.randomUUID(), role: "user", content: String(text), createdAt: new Date().toISOString(), executor, capabilityId: capabilityId || null, researchAgent: researchAgent?.id || null }); if (row.messages.length === 1) row.title = String(text).slice(0, 40); });
    if (executor !== "codex") throw new Error("Only the Codex executor is available.");
    const taskDir = path.join(workspace.path, ".platform", "tasks", `${conversationId}-${Date.now()}`); const runner = runCodex;
    const activity = { startedAt: Date.now(), lines: [], details: [] };
    const report = (update) => { const summary = String(update?.summary || update || "").trim(); const detail = String(update?.detail || summary).trim(); if (summary && activity.lines.at(-1) !== summary) activity.lines.push(summary); if (activity.lines.length > 20) activity.lines.shift(); if (detail && activity.details.at(-1) !== detail) activity.details.push(detail); if (activity.details.length > 40) activity.details.shift(); };
    report(capability ? `正在准备能力：${capability.name}` : "正在准备普通对话");
    const controller = new AbortController();
    const task = { activity, controller, promise: null }; this.running.set(conversationId, task);
    const promise = runner({ config: this.config, workspace, capability, capabilities: this.capabilities, researchAgent, conversation: historyConversation, text, permission, taskDir, signal: controller.signal, onProgress: report }).then(async (result) => {
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
    }).finally(() => this.running.delete(conversationId));
    task.promise = promise; return { accepted: true, conversationId, taskDir };
  }
  async choose({ conversationId, messageId, optionIndex }) {
    if (this.running.has(conversationId)) throw new Error("This conversation already has a running task.");
    const conversation = this.store.getConversation(conversationId); if (!conversation) throw new Error("Unknown conversation.");
    const message = conversation.messages.find((item) => item.id === messageId); if (!message?.choice) throw new Error("Choice request not found.");
    const index = Number(optionIndex); const option = message.choice.options[index]; if (!option) throw new Error("Unknown choice option.");
    if (message.choice.selectedIndex != null) throw new Error("This choice has already been submitted.");
    await this.store.updateConversation(conversationId, (row) => { const target = row.messages.find((item) => item.id === messageId); target.choice.selectedIndex = index; });
    return this.start({ conversationId, text: `我选择“${option.label}”。\n选择值：${option.value}\n请使用这个选择继续并完成刚才暂停的任务。`, executor: "codex", capabilityId: message.capabilityId || null, permission: message.permission || "workspace-write" });
  }
}
