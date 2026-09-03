#!/usr/bin/env node
import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

function arg(name, fallback = null) {
  const index = process.argv.indexOf(`--${name}`);
  return index >= 0 ? process.argv[index + 1] : fallback;
}

async function isFile(file) {
  return fs.stat(file).then((stat) => stat.isFile()).catch(() => false);
}

export async function preflightRethlas({ executor = "codex", rethlasRoot = process.env.RETHLAS_ROOT || "", env = process.env } = {}) {
  if (executor !== "codex") throw new Error("executor must be codex");
  const missing = [];
  const resolvedRoot = rethlasRoot ? path.resolve(rethlasRoot) : null;
  const model = env.RETHLAS_CODEX_MODEL || "codex-cli-default";
  if (!resolvedRoot) missing.push("environment:RETHLAS_ROOT");
  const requiredFiles = resolvedRoot ? [
    path.join(resolvedRoot, "agents", "generation", "AGENTS.md"),
    path.join(resolvedRoot, "agents", "verification", "AGENTS.md")
  ] : [];
  for (const file of requiredFiles) if (!(await isFile(file))) missing.push(file);
  return {
    ready: missing.length === 0,
    executor,
    provider: "openai",
    model,
    rethlasRoot: resolvedRoot,
    verificationPort: 8091,
    missing
  };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const report = await preflightRethlas({
    executor: arg("executor", "codex"),
    rethlasRoot: arg("rethlas-root", process.env.RETHLAS_ROOT || "")
  });
  console.log(JSON.stringify(report, null, 2));
  if (!report.ready) process.exitCode = 1;
}
