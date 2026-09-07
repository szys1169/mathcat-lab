import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { citationKeysFromTex, parseBibtexEntries } from "./bibtex-utils.mjs";

const requiredResearch = [
  "contribution-profile.json", "reused-source-manifest.json", "source-ledger.json", "targeted-search-log.jsonl",
  "literature-comparison-matrix.json", "gap-positioning-ledger.json", "related-work-claim-ledger.json", "evidence-gaps.md"
];
const requiredPaper = ["related-work.tex", "related-work-inline.tex", "related-work-preview.pdf", "related-work.bib"];
const requiredResults = ["related-work-summary.md", "related-work-task-report.md"];

async function exists(file) { try { return (await fs.stat(file)).isFile(); } catch { return false; } }

export async function validateDelivery(projectRoot, taskId) {
  const errors = [];
  const researchRoot = path.join(projectRoot, "调研", "相关工作", taskId);
  const paperRoot = path.join(projectRoot, "论文", "相关工作", taskId);
  const resultRoot = path.join(projectRoot, "成果", "相关工作", taskId);
  for (const [root, names] of [[researchRoot, requiredResearch], [paperRoot, requiredPaper], [resultRoot, requiredResults]]) {
    for (const name of names) if (!(await exists(path.join(root, name)))) errors.push(`missing required file: ${path.join(root, name)}`);
  }
  const pdf = path.join(paperRoot, "related-work-preview.pdf");
  if (await exists(pdf)) {
    const header = Buffer.alloc(5);
    const handle = await fs.open(pdf, "r");
    try { await handle.read(header, 0, 5, 0); } finally { await handle.close(); }
    if (header.toString("ascii") !== "%PDF-") errors.push("invalid PDF header: related-work-preview.pdf");
  }
  const bibFile = path.join(paperRoot, "related-work.bib");
  if (await exists(bibFile)) {
    const bibKeys = new Set(parseBibtexEntries(await fs.readFile(bibFile, "utf8")).map((entry) => entry.key));
    for (const name of ["related-work.tex", "related-work-inline.tex"]) {
      const file = path.join(paperRoot, name);
      if (!(await exists(file))) continue;
      for (const key of citationKeysFromTex(await fs.readFile(file, "utf8"))) if (!bibKeys.has(key)) errors.push(`${name} cites missing BibTeX key ${key}.`);
    }
  }
  return errors;
}

async function main() {
  const [projectRoot, taskId] = process.argv.slice(2);
  if (!projectRoot || !taskId) throw new Error("Usage: node validate-delivery.mjs <project-root> <task-id>");
  const errors = await validateDelivery(projectRoot, taskId);
  if (errors.length) { for (const error of errors) console.error(error); process.exitCode = 1; }
  else console.log("Related-work delivery is valid.");
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
