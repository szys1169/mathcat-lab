#!/usr/bin/env node
import fs from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

export function createLineDiff(originalText, revisedText, originalLabel = "original", revisedLabel = "revised") {
  const before = originalText.replaceAll("\r\n", "\n").split("\n");
  const after = revisedText.replaceAll("\r\n", "\n").split("\n");
  const rows = before.length + 1;
  const cols = after.length + 1;
  if (rows * cols > 4_000_000) {
    return [`--- ${originalLabel}`, `+++ ${revisedLabel}`, "@@ full-file change (diff simplified for large manuscript) @@", ...before.map((line) => `-${line}`), ...after.map((line) => `+${line}`), ""].join("\n");
  }
  const lcs = Array.from({ length: rows }, () => new Uint32Array(cols));
  for (let i = before.length - 1; i >= 0; i -= 1) {
    for (let j = after.length - 1; j >= 0; j -= 1) lcs[i][j] = before[i] === after[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
  }
  const body = [];
  let i = 0; let j = 0;
  while (i < before.length || j < after.length) {
    if (i < before.length && j < after.length && before[i] === after[j]) { body.push(` ${before[i]}`); i += 1; j += 1; }
    else if (j < after.length && (i === before.length || lcs[i][j + 1] >= lcs[i + 1][j])) { body.push(`+${after[j]}`); j += 1; }
    else { body.push(`-${before[i]}`); i += 1; }
  }
  return [`--- ${originalLabel}`, `+++ ${revisedLabel}`, "@@", ...body, ""].join("\n");
}

async function main() {
  const [original, revised, output] = process.argv.slice(2);
  if (!original || !revised || !output) throw new Error("Usage: create-revision-diff.mjs <original.tex> <revised.tex> <revision.diff>");
  const diff = createLineDiff(await fs.readFile(original, "utf8"), await fs.readFile(revised, "utf8"), original, revised);
  await fs.mkdir(path.dirname(output), { recursive: true });
  await fs.writeFile(output, diff, "utf8");
  console.log(output);
}

if (import.meta.url === pathToFileURL(process.argv[1] || "").href) await main();
