#!/usr/bin/env node
import fs from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

async function exists(file) { try { return (await fs.stat(file)).isFile(); } catch { return false; } }

export async function validateDeliverables(root) {
  const required = ["paper.pdf", "revision.diff", "revision-log.json", "writing-audit.md"];
  const missing = [];
  for (const name of required) if (!(await exists(path.join(root, name)))) missing.push(name);
  const source = path.join(root, "source");
  const tex = (await fs.readdir(source, { recursive: true }).catch(() => [])).filter((file) => String(file).toLowerCase().endsWith(".tex"));
  if (!tex.length) missing.push("source/**/*.tex");
  return missing;
}

async function main() {
  const root = process.argv[2];
  if (!root) throw new Error("Usage: validate-deliverables.mjs <revision-root>");
  const missing = await validateDeliverables(path.resolve(root));
  if (missing.length) {
    console.error(`missing deliverables: ${missing.join(", ")}`);
    process.exitCode = 1;
  } else {
    console.log("revision deliverables are complete");
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] || "").href) await main();
