#!/usr/bin/env node
import fs from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

async function isFile(file) { try { return (await fs.stat(file)).isFile(); } catch { return false; } }
async function validPdf(file) { if (!(await isFile(file))) return false; const handle = await fs.open(file, "r"); try { const buffer = Buffer.alloc(5); await handle.read(buffer, 0, 5, 0); return buffer.toString("ascii") === "%PDF-"; } finally { await handle.close(); } }

export async function validateDelivery(projectRoot, taskId) {
  const researchRoot = path.join(projectRoot, "调研", "文献调研", taskId);
  const resultRoot = path.join(projectRoot, "成果", "文献调研", taskId);
  const requiredResearch = ["research-scope.json", "search-strategy.md", "search-log.jsonl", "screening-log.json", "source-ledger.json", "claim-source-ledger.json", "theorem-ledger.json", "theorem-dependency-map.json", "historical-timeline.json", "candidate-gaps.md", "evidence-gaps.md"];
  const requiredResults = ["literature-review.tex", "literature-review.pdf", "theorem-toolbox.tex", "theorem-toolbox.pdf", "selected-bibliography.bib", "literature-research-task-report.md"];
  const missing = [];
  for (const name of requiredResearch) if (!(await isFile(path.join(researchRoot, name)))) missing.push(`调研/文献调研/${taskId}/${name}`);
  for (const name of requiredResults) if (!(await isFile(path.join(resultRoot, name)))) missing.push(`成果/文献调研/${taskId}/${name}`);
  for (const name of ["literature-review.pdf", "theorem-toolbox.pdf"]) {
    const target = path.join(resultRoot, name);
    if ((await isFile(target)) && !(await validPdf(target))) missing.push(`invalid PDF: 成果/文献调研/${taskId}/${name}`);
  }
  return missing;
}

async function main() {
  const [projectRoot, taskId] = process.argv.slice(2);
  if (!projectRoot || !taskId) throw new Error("Usage: validate-delivery.mjs <project-root> <task-id>");
  const missing = await validateDelivery(path.resolve(projectRoot), taskId);
  if (missing.length) { missing.forEach((item) => console.error(item)); process.exitCode = 1; }
  else console.log("literature research delivery is complete");
}

if (import.meta.url === pathToFileURL(process.argv[1] || "").href) await main();
