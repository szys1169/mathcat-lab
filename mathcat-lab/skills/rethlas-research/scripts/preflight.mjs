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

export async function preflightRethlas({ executor = "deepseek_harness", rethlasRoot = process.env.RETHLAS_ROOT || "F:\\Rethlas-deepseek", env = process.env } = {}) {
  if (!new Set(["codex", "deepseek_harness"]).has(executor)) throw new Error("executor must be codex or deepseek_harness");
  const resolvedRoot = path.resolve(rethlasRoot);
  const model = executor === "codex"
    ? env.RETHLAS_CODEX_MODEL || "gpt-5.6-sol"
    : env.RETHLAS_DEEPSEEK_MODEL || env.RETHLAS_MODEL || "deepseek-v4-flash";
  const modelCatalog = path.join(resolvedRoot, "models.json");
  const requiredFiles = [
    path.join(resolvedRoot, "agents", "generation", "AGENTS.md"),
    path.join(resolvedRoot, "agents", "verification", "AGENTS.md"),
    ...(executor === "deepseek_harness" ? [modelCatalog] : [])
  ];
  const missing = [];
  for (const file of requiredFiles) if (!(await isFile(file))) missing.push(file);
  if (executor === "deepseek_harness" && !env.DEEPSEEK_API_KEY) missing.push("environment:DEEPSEEK_API_KEY");
  let modelCatalogValid = true;
  if (executor === "deepseek_harness" && !missing.includes(modelCatalog)) {
    const catalog = JSON.parse(await fs.readFile(modelCatalog, "utf8"));
    const entry = (catalog.models || []).find((item) => item.slug === model);
    modelCatalogValid = Boolean(entry?.base_instructions);
    if (!modelCatalogValid) missing.push(`model-catalog:${model}`);
  }
  return {
    ready: missing.length === 0,
    executor,
    provider: executor === "codex" ? "openai" : "deepseek",
    model,
    rethlasRoot: resolvedRoot,
    verificationPort: 8091,
    modelCatalogValid,
    missing
  };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const report = await preflightRethlas({
    executor: arg("executor", "deepseek_harness"),
    rethlasRoot: arg("rethlas-root", process.env.RETHLAS_ROOT || "F:\\Rethlas-deepseek")
  });
  console.log(JSON.stringify(report, null, 2));
  if (!report.ready) process.exitCode = 1;
}
