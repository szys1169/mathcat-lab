import fs from "node:fs/promises";
import path from "node:path";
import crypto from "node:crypto";
import { writeJsonAtomic, safeId } from "./fs-utils.mjs";

export class Store {
  constructor(runtimeRoot) { this.runtimeRoot = path.resolve(runtimeRoot); this.projectlessRoot = path.join(this.runtimeRoot, "projectless"); this.file = path.join(this.runtimeRoot, "state.json"); this.state = { schemaVersion: 1, workspaces: [], conversations: [] }; this.queue = Promise.resolve(); }
  async load() {
    try { this.state = JSON.parse(await fs.readFile(this.file, "utf8")); }
    catch (error) { if (error.code !== "ENOENT") throw error; await this.save(); }
    let changed = false;
    for (const conversation of this.state.conversations ?? []) {
      if (conversation.status === "running") {
        conversation.status = "failed";
        conversation.error = "平台重启中断了上一次任务，请重新发送。";
        changed = true;
      }
    }
    if (changed) await this.save();
    return this;
  }
  async save() { this.queue = this.queue.then(() => writeJsonAtomic(this.file, this.state)); return this.queue; }
  listWorkspaces() { return this.state.workspaces; }
  async addWorkspace({ name, workspacePath }) {
    const resolved = path.resolve(workspacePath); const stat = await fs.stat(resolved); if (!stat.isDirectory()) throw new Error("Workspace must be a directory.");
    let row = this.state.workspaces.find((item) => item.path.toLowerCase() === resolved.toLowerCase());
    if (!row) { row = { id: crypto.randomUUID(), name: String(name || path.basename(resolved)), path: resolved, createdAt: new Date().toISOString() }; this.state.workspaces.push(row); await this.save(); }
    return row;
  }
  async deleteWorkspace(id) {
    const workspace = this.state.workspaces.find((item) => item.id === id);
    if (!workspace) throw new Error("Unknown workspace.");
    const conversationIds = this.state.conversations.filter((item) => item.workspaceId === id).map((item) => item.id);
    this.state.workspaces = this.state.workspaces.filter((item) => item.id !== id);
    this.state.conversations = this.state.conversations.filter((item) => item.workspaceId !== id);
    await this.save();
    return { workspace, conversationIds };
  }
  listConversations() { return [...this.state.conversations].sort((a,b) => b.updatedAt.localeCompare(a.updatedAt)).map(({ messages, ...row }) => ({ ...row, messageCount: messages.length })); }
  getConversation(id) { return this.state.conversations.find((item) => item.id === id) || null; }
  async createConversation({ workspaceId, title = "新对话" }) {
    const projectless = workspaceId == null || workspaceId === "";
    if (!projectless && !this.state.workspaces.some((item) => item.id === workspaceId)) throw new Error("Unknown workspace.");
    const now = new Date().toISOString(); const row = { id: crypto.randomUUID(), workspaceId: projectless ? null : workspaceId, title: String(title).slice(0, 80), createdAt: now, updatedAt: now, status: "idle", messages: [] };
    if (projectless) await fs.mkdir(path.join(this.projectlessRoot, safeId(row.id)), { recursive: true });
    this.state.conversations.push(row); await this.save(); return row;
  }
  async deleteConversation(id) {
    const conversation = this.getConversation(id);
    if (!conversation) throw new Error("Unknown conversation.");
    this.state.conversations = this.state.conversations.filter((item) => item.id !== id);
    await this.save();
    return conversation;
  }
  async updateConversation(id, mutate) { const row = this.getConversation(id); if (!row) throw new Error("Unknown conversation."); mutate(row); row.updatedAt = new Date().toISOString(); await this.save(); return row; }
  workspaceFor(conversation) {
    if (conversation?.workspaceId == null) return { id: null, name: "无工作区对话", path: path.join(this.projectlessRoot, safeId(conversation.id)), projectless: true };
    return this.state.workspaces.find((item) => item.id === conversation.workspaceId) || null;
  }
  taskDir(workspace, conversationId) { return path.join(workspace.path, ".platform", "tasks", safeId(conversationId)); }
}
