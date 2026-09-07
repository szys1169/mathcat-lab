import fs from "node:fs/promises";
import path from "node:path";
import { writeJsonAtomic } from "./fs-utils.mjs";

const DEFINITIONS = {
  mathcat: { id: "mathcat", name: "MathCat 2.4.3", description: "持续研究 · 项目白板 · 按需伙伴与审查", env: "MATHCAT_ROOT", repo: null },
  rethlas: { id: "rethlas", name: "Rethlas", description: "生成—验证闭环数学研究智能体", env: "RETHLAS_ROOT", repo: "https://github.com/frenzymath/Rethlas" },
  danus: { id: "danus", name: "Danus", description: "带事实图与角色门控的研究智能体", env: "DANUS_ROOT", repo: "https://github.com/frenzymath/Danus" }
};

async function isDirectory(candidate) { try { return (await fs.stat(candidate)).isDirectory(); } catch { return false; } }

export class ResearchAgents {
  constructor(config) { this.config = config; this.file = path.join(config.runtimeRoot, "research-agents.json"); this.saved = {}; }
  async load() { try { this.saved = JSON.parse(await fs.readFile(this.file, "utf8")); } catch (error) { if (error.code !== "ENOENT") throw error; } return this; }
  candidates(id) {
    const root = path.dirname(this.config.appRoot);
    const definition = DEFINITIONS[id];
    const values = [process.env[definition.env], this.saved[id]?.path];
    if (id === "mathcat") values.push(path.join(root, "math-research-mvp"));
    if (id === "rethlas") values.push(path.join(root, "agents", "Rethlas"), "F:\\Rethlas-deepseek", "F:\\Rethlas");
    if (id === "danus") values.push(path.join(root, "agents", "Danus"), "F:\\Danus");
    return [...new Set(values.filter(Boolean).map((value) => path.resolve(value)))];
  }
  async discover(id) { for (const candidate of this.candidates(id)) if (await isDirectory(candidate)) return candidate; return null; }
  async list() {
    return Promise.all(Object.values(DEFINITIONS).map(async (item) => { const agentPath=item.id === "mathcat" ? await this.discover(item.id) : this.saved[item.id]?.path || null; return { ...item, configured: Boolean(agentPath), path: agentPath }; }));
  }
  async configure(id, manualPath = null) {
    const definition = DEFINITIONS[id]; if (!definition) throw new Error("Unknown research agent.");
    const found = manualPath ? path.resolve(manualPath) : await this.discover(id);
    if (found && !await isDirectory(found)) throw new Error("Research agent path is not a directory.");
    if (found) { this.saved[id] = { path: found, configuredAt: new Date().toISOString() }; await writeJsonAtomic(this.file, this.saved); }
    return { ...definition, found: Boolean(found), path: found, configured: Boolean(found) };
  }
  async get(id) { return (await this.list()).find((item) => item.id === id) || null; }
}
