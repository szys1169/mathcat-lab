#!/usr/bin/env node
import fs from "node:fs/promises";
import { pathToFileURL } from "node:url";

export const GATES = new Set(["PASS", "PASS_WITH_WARNINGS", "RESEARCH_REQUIRED", "HUMAN_REQUIRED", "FAIL"]);
export const ISSUE_TYPES = new Set([
  "false_or_counterexample_risk", "missing_assumption", "proof_gap", "circular_dependency",
  "citation_mismatch", "novelty_overlap_risk", "definition_or_notation",
  "unsupported_computation", "unresolved"
]);
export const SEVERITIES = new Set(["blocking_math", "high", "medium", "low", "info"]);
export const ISSUE_STATUSES = new Set(["open", "resolved", "research_required", "human_required", "not_applicable"]);
export const VERIFICATION_LEVELS = new Set(["rethlas_natural_language", "formal_lean", "human_reviewed", "unverified"]);

export function validateGapLedger(ledger) {
  const errors = [];
  if (!ledger || typeof ledger !== "object" || Array.isArray(ledger)) return ["ledger must be an object"];
  if (ledger.schemaVersion !== "0.1") errors.push("schemaVersion must be 0.1");
  if (typeof ledger.revisionId !== "string" || !ledger.revisionId.trim()) errors.push("revisionId is required");
  if (!GATES.has(ledger.gate)) errors.push("gate is invalid");
  if (!VERIFICATION_LEVELS.has(ledger.verificationLevel)) errors.push("verificationLevel is invalid");
  if (!Array.isArray(ledger.issues)) errors.push("issues must be an array");
  for (const [index, issue] of (Array.isArray(ledger.issues) ? ledger.issues : []).entries()) {
    const prefix = `issues[${index}]`;
    for (const field of ["id", "paperLocation", "claim", "evidence", "reasoningSummary", "recommendedAction"]) {
      if (typeof issue?.[field] !== "string" || !issue[field].trim()) errors.push(`${prefix}.${field} is required`);
    }
    if (!ISSUE_TYPES.has(issue?.type)) errors.push(`${prefix}.type is invalid`);
    if (!SEVERITIES.has(issue?.severity)) errors.push(`${prefix}.severity is invalid`);
    if (!ISSUE_STATUSES.has(issue?.status)) errors.push(`${prefix}.status is invalid`);
    if (typeof issue?.confidence !== "number" || issue.confidence < 0 || issue.confidence > 1) errors.push(`${prefix}.confidence must be between 0 and 1`);
  }
  const unresolvedBlocking = (ledger.issues || []).some((issue) => ["blocking_math", "high"].includes(issue.severity) && issue.status !== "resolved" && issue.status !== "not_applicable");
  if (ledger.gate === "PASS" && unresolvedBlocking) errors.push("PASS cannot contain unresolved blocking_math/high issues");
  return errors;
}

async function main() {
  const file = process.argv[2];
  if (!file) throw new Error("Usage: validate-gap-ledger.mjs <gap-ledger.json>");
  const ledger = JSON.parse(await fs.readFile(file, "utf8"));
  const errors = validateGapLedger(ledger);
  if (errors.length) {
    for (const error of errors) console.error(error);
    process.exitCode = 1;
  } else {
    console.log("gap ledger is valid");
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] || "").href) await main();
