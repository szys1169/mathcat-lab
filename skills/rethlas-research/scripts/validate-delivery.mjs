#!/usr/bin/env node
import fs from "node:fs/promises";
import path from "node:path";

const resultRoot = path.resolve(process.argv[2] || "");
if (!process.argv[2]) throw new Error("Usage: validate-delivery.mjs <result-root>");
const report = path.join(resultRoot, "research-task-report.md");
const blueprint = path.join(resultRoot, "blueprint.md");
const verified = path.join(resultRoot, "blueprint_verified.md");
const exists = async (file) => fs.stat(file).then((stat) => stat.isFile()).catch(() => false);
const output = {
  valid: await exists(report),
  report: await exists(report),
  blueprint: await exists(blueprint),
  verifiedBlueprint: await exists(verified)
};
console.log(JSON.stringify(output, null, 2));
if (!output.valid) process.exitCode = 1;
