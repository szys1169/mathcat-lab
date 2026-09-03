import fs from "node:fs/promises";
import path from "node:path";
import { runProcess } from "./process-runner.mjs";

function capabilityPrompt(capability, executor) {
  const interaction = `When a task cannot continue because the workspace contains multiple plausible input files, cases, bundles, source packets, manuscripts, or output targets, inspect the candidates instead of asking the user to type a path. Present 2-5 best candidates and end your response with exactly one fenced block in this form:\n\n\`\`\`math-lab-choice\n{"question":"请选择本次任务使用的输入","options":[{"label":"简短名称","value":"完整路径或明确选择值","reason":"为什么它适合"}],"recommendedIndex":0,"recommendationReason":"它明显优于其他候选的具体理由"}\n\`\`\`\n\nOnly include recommendedIndex and recommendationReason when one option has a clear, material, evidence-based advantage over all others. If candidates are close or evidence is insufficient, omit both fields and recommend nothing. Never invent or display numeric scores. Do not continue the task until the user chooses. Do not use this protocol when there is only one safe unambiguous candidate.`;
  if (!capability) return `You are a local mathematical research assistant. Respect the selected workspace and never invent completed tool results. ${interaction}`;
  return `Use the ${capability.id} capability. Read its authoritative Skill at ${capability.skillFile}. Executor is ${executor}. Follow its contracts and adapters. Keep all outputs inside the selected workspace and finish only after its delivery gate passes. ${interaction}`;
}

function shorten(value, max = 110) { const text = String(value || "").replace(/\s+/g, " ").trim(); return text.length > max ? `${text.slice(0, max - 1)}…` : text; }
function commandStage(command) {
  const value = String(command || "");
  if (/\b(?:xelatex|latexmk)\b/i.test(value)) return "正在编译 LaTeX 文档";
  if (/\bpdftoppm\b|render/i.test(value)) return "正在渲染并检查输出页面";
  if (/\b(?:validate|preflight)\b/i.test(value)) return "正在运行交付校验";
  if (/\bprepare\b/i.test(value)) return "正在准备任务输入和版本目录";
  if (/\bfinalize\b/i.test(value)) return "正在汇总结果并执行完成门禁";
  return `正在执行命令：${shorten(value, 85)}`;
}
async function readCodexProgress(stdoutFile) {
  let handle; let raw = "";
  try {
    handle = await fs.open(stdoutFile, "r");
    const stat = await handle.stat(); const length = Math.min(stat.size, 96 * 1024); const buffer = Buffer.alloc(length);
    await handle.read(buffer, 0, length, stat.size - length); raw = buffer.toString("utf8");
    if (stat.size > length) raw = raw.slice(raw.indexOf("\n") + 1);
  } catch {} finally { await handle?.close().catch(() => {}); }
  const rows = raw.trim().split(/\r?\n/).slice(-40).reverse();
  for (const row of rows) {
    let event; try { event = JSON.parse(row); } catch { continue; }
    if (event.type === "error") return { summary: `连接状态：${shorten(event.message)}`, detail: `错误：${shorten(event.message, 360)}` };
    const item = event.item; if (!item) continue;
    if (item.type === "agent_message" && item.text) return { summary: `阶段进展：${shorten(item.text)}`, detail: `Codex 进展摘要：${shorten(item.text, 500)}` };
    if (item.type === "command_execution") return { summary: commandStage(item.command), detail: `命令：${shorten(item.command, 500)}` };
    if (item.type === "file_change") { const changes = item.changes || []; const first = changes[0]?.path; return { summary: first ? `正在写入 ${changes.length} 个文件，当前：${shorten(first, 72)}` : "正在写入任务文件", detail: changes.length ? `文件变更：${changes.map((change) => change.path || change).slice(0, 8).join("；")}` : "文件变更：正在写入任务文件" }; }
    if (item.type === "mcp_tool_call") { const tool = item.tool || item.name || "外部工具"; return { summary: `正在调用工具：${shorten(tool)}`, detail: `工具调用：${shorten(tool, 240)}` }; }
    if (item.type === "web_search") { const query = item.query || "相关资料"; return { summary: `正在检索：${shorten(query)}`, detail: `检索：${shorten(query, 400)}` }; }
    if (item.type === "reasoning") return { summary: "正在分析已有材料并规划下一步", detail: item.text ? `Codex 提供的推理摘要：${shorten(item.text, 500)}` : "Codex 正在推理（未提供可公开的文字摘要）" };
  }
  return { summary: "正在分析工作区和任务要求", detail: "等待 Codex 输出下一个可见执行事件" };
}

export async function runCodex({ config, workspace, capability, researchAgent, conversation, text, permission, taskDir, signal, onProgress = () => {} }) {
  onProgress("正在构造 Codex CLI 任务上下文");
  await fs.mkdir(taskDir, { recursive: true }); const output = path.join(taskDir, "assistant-last-message.md");
  const history = conversation.messages.slice(-8).map((item) => `${item.role}: ${item.content}`).join("\n\n");
  const agentPrompt = researchAgent ? `\nSelected research agent: ${researchAgent.name} (${researchAgent.id}). Local root: ${researchAgent.path}. Use this agent through Codex CLI and follow its local documentation; do not silently substitute another research agent.` : "";
  const prompt = `${capabilityPrompt(capability, "codex")}${agentPrompt}\nPermission: ${permission}.\nConversation context:\n${history}\n\nCurrent user request:\n${text}`;
  const args = ["exec", "--json", "--output-last-message", output, "--cd", workspace.path, "--sandbox", permission === "workspace-write" ? "workspace-write" : "read-only", "--skip-git-repo-check", prompt];
  onProgress("Codex CLI 已启动，正在分析工作区并执行任务");
  let readingProgress = false;
  const heartbeat = setInterval(async () => { if (readingProgress) return; readingProgress = true; try { onProgress(await readCodexProgress(path.join(taskDir, "codex.stdout.log"))); } finally { readingProgress = false; } }, 2500);
  let result;
  try { result = await runProcess({ command: config.codexBin, args, cwd: workspace.path, stdoutFile: path.join(taskDir, "codex.stdout.log"), stderrFile: path.join(taskDir, "codex.stderr.log"), signal }); }
  finally { clearInterval(heartbeat); }
  onProgress("Codex CLI 已结束，正在读取最终输出");
  if (result.aborted) throw new DOMException("Task cancelled by user.", "AbortError");
  const content = await fs.readFile(output, "utf8").catch(() => "");
  if (result.code !== 0) throw new Error(`Codex CLI exited with code ${result.code}. See ${path.join(taskDir, "codex.stderr.log")}`);
  return { content: content.trim() || "Codex completed without a final text response.", executor: "codex", artifacts: [output, path.join(taskDir, "codex.stdout.log"), path.join(taskDir, "codex.stderr.log")] };
}
