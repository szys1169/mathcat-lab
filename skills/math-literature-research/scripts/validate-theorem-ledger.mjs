#!/usr/bin/env node
import fs from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { EVIDENCE_LEVELS } from "./validate-source-ledger.mjs";

const KINDS = new Set(["definition", "lemma", "theorem", "proposition", "corollary", "criterion", "bound", "counterexample", "method"]);
const RELEVANCE = new Set(["direct", "conditional", "analogical", "background", "uncertain"]);
const STATUSES = new Set(["verified_from_full_text", "statement_needs_verification", "secondary_only"]);

export function validateTheoremLedger(ledger, sourceLedger) {
  const errors = [];
  if (!ledger || typeof ledger !== "object" || Array.isArray(ledger)) return ["theorem ledger must be an object"];
  if (ledger.schemaVersion !== "0.1") errors.push("schemaVersion must be 0.1");
  if (typeof ledger.taskId !== "string" || !ledger.taskId.trim()) errors.push("taskId is required");
  if (!Array.isArray(ledger.theorems)) errors.push("theorems must be an array");
  if (ledger.unverifiedCandidates != null && !Array.isArray(ledger.unverifiedCandidates)) errors.push("unverifiedCandidates must be an array");
  const sourceIds = new Set((sourceLedger?.sources || []).map((source) => source.sourceId));
  const theoremIds = new Set();
  for (const [index, theorem] of (ledger.theorems || []).entries()) {
    const prefix = `theorems[${index}]`;
    for (const field of ["theoremId", "name", "statement", "assumptions", "conclusion", "sourceId", "bibtexKey", "sourceLocation", "possibleUse", "applicabilityRisk", "nextStep"]) {
      if (typeof theorem?.[field] !== "string" || !theorem[field].trim()) errors.push(`${prefix}.${field} is required`);
    }
    if (!KINDS.has(theorem?.kind)) errors.push(`${prefix}.kind is invalid`);
    if (!RELEVANCE.has(theorem?.relevance)) errors.push(`${prefix}.relevance is invalid`);
    if (!EVIDENCE_LEVELS.has(theorem?.evidenceLevel)) errors.push(`${prefix}.evidenceLevel is invalid`);
    if (!STATUSES.has(theorem?.verificationStatus)) errors.push(`${prefix}.verificationStatus is invalid`);
    if (theorem?.sourceId && !sourceIds.has(theorem.sourceId)) errors.push(`${prefix}.sourceId is not present in source ledger`);
    if (theorem?.theoremId) {
      if (theoremIds.has(theorem.theoremId)) errors.push(`${prefix}.theoremId is duplicated`);
      theoremIds.add(theorem.theoremId);
    }
    if (theorem?.verificationStatus === "verified_from_full_text" && theorem?.evidenceLevel !== "full_text_checked") errors.push(`${prefix} cannot be verified_from_full_text without full_text_checked evidence`);
  }
  return errors;
}

async function main() {
  const [theoremFile, sourceFile] = process.argv.slice(2);
  if (!theoremFile || !sourceFile) throw new Error("Usage: validate-theorem-ledger.mjs <theorem-ledger.json> <source-ledger.json>");
  const [ledger, sources] = await Promise.all([fs.readFile(theoremFile, "utf8").then(JSON.parse), fs.readFile(sourceFile, "utf8").then(JSON.parse)]);
  const errors = validateTheoremLedger(ledger, sources);
  if (errors.length) { errors.forEach((error) => console.error(error)); process.exitCode = 1; }
  else console.log("theorem ledger is valid");
}

if (import.meta.url === pathToFileURL(process.argv[1] || "").href) await main();
