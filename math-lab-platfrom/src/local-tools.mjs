import fs from "node:fs/promises";
import path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { ensureInside } from "./fs-utils.mjs";

const execFileAsync = promisify(execFile);

export const toolDefinitions = [
  { name: "list_files", description: "List files below the selected workspace.", input_schema: { type: "object", properties: { path: { type: "string" }, depth: { type: "integer", minimum: 1, maximum: 4 } } } },
  { name: "read_text_file", description: "Read a UTF-8 text file inside the selected workspace or selected capability.", input_schema: { type: "object", required: ["path"], properties: { path: { type: "string" } } } },
  { name: "write_text_file", description: "Write a UTF-8 file inside the workspace. Requires workspace-write permission.", input_schema: { type: "object", required: ["path", "content"], properties: { path: { type: "string" }, content: { type: "string" } } } },
  { name: "run_capability_adapter", description: "Run a registered capability adapter action.", input_schema: { type: "object", required: ["capability_id", "action"], properties: { capability_id: { type: "string" }, action: { enum: ["health", "preflight", "prepare", "finalize", "terminate"] }, request_path: { type: "string" }, run_dir: { type: "string" }, reason: { type: "string" } } } },
  { name: "run_allowed_process", description: "Run Python, Node or a LaTeX build tool with arguments restricted to the workspace/capability roots.", input_schema: { type: "object", required: ["command", "args"], properties: { command: { enum: ["python", "node", "latexmk", "xelatex", "pdftoppm"] }, args: { type: "array", items: { type: "string" } }, cwd: { type: "string" } } } }
];

function resolveReadable(value, workspace, capabilityRoot) {
  const candidate = path.resolve(value);
  try { return ensureInside(workspace, candidate); } catch {}
  if (capabilityRoot) return ensureInside(capabilityRoot, candidate);
  throw new Error("Path is outside allowed roots.");
}

async function walk(root, depth, prefix = "") {
  if (depth < 0) return [];
  const entries = await fs.readdir(path.join(root, prefix), { withFileTypes: true });
  const rows = [];
  for (const entry of entries.slice(0, 300)) {
    const rel = path.join(prefix, entry.name); rows.push(rel.replaceAll("\\", "/") + (entry.isDirectory() ? "/" : ""));
    if (entry.isDirectory() && depth > 0 && !["node_modules", ".git"].includes(entry.name)) rows.push(...await walk(root, depth - 1, rel));
  }
  return rows;
}

export async function executeTool({ name, input, workspace, capability, capabilities, permission, signal }) {
  if (name === "list_files") { const target = ensureInside(workspace, path.resolve(workspace, input.path || ".")); return (await walk(target, Math.min(Number(input.depth || 2), 4))).join("\n"); }
  if (name === "read_text_file") { const file = resolveReadable(path.resolve(workspace, input.path), workspace, capability?.root); const stat = await fs.stat(file); if (stat.size > 2_000_000) throw new Error("Text file exceeds 2 MB."); return await fs.readFile(file, "utf8"); }
  if (name === "write_text_file") { if (permission !== "workspace-write") throw new Error("Workspace is read-only."); const file = ensureInside(workspace, path.resolve(workspace, input.path)); await fs.mkdir(path.dirname(file), { recursive: true }); await fs.writeFile(file, String(input.content), "utf8"); return `written: ${file}`; }
  if (name === "run_capability_adapter") {
    if (permission !== "workspace-write" && input.action !== "health" && input.action !== "preflight") throw new Error("Workspace is read-only.");
    const selected = capabilities.find((item) => item.id === input.capability_id); if (!selected) throw new Error("Unknown capability.");
    const args = [selected.adapterFile, input.action];
    if (["preflight", "prepare"].includes(input.action)) args.push("--request", ensureInside(workspace, path.resolve(workspace, input.request_path)));
    if (["finalize", "terminate"].includes(input.action)) args.push("--run-dir", ensureInside(workspace, path.resolve(workspace, input.run_dir)));
    if (input.action === "terminate") args.push("--reason", input.reason || "stopped");
    const result = await execFileAsync(process.env.MATH_LAB_PYTHON || "python", args, { cwd: workspace, windowsHide: true, signal, timeout: 120_000, maxBuffer: 8_000_000, encoding: "utf8" }); return result.stdout;
  }
  if (name === "run_allowed_process") {
    if (permission !== "workspace-write") throw new Error("Workspace is read-only.");
    const cwd = ensureInside(workspace, path.resolve(workspace, input.cwd || ".")); const args = (input.args || []).map(String);
    for (const arg of args) { if (/^[A-Za-z]:[\\/]/.test(arg)) resolveReadable(arg, workspace, capability?.root); }
    const result = await execFileAsync(input.command, args, { cwd, windowsHide: true, signal, timeout: 180_000, maxBuffer: 8_000_000, encoding: "utf8" }); return [result.stdout, result.stderr].filter(Boolean).join("\n");
  }
  throw new Error(`Unknown tool: ${name}`);
}
