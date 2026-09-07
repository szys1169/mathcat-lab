#!/usr/bin/env node
import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  EVIDENCE_LEVELS,
  SCREENING_STATUSES,
  SOURCE_TYPES,
  validateSourceLedger
} from "../../math-literature-core/scripts/source-ledger.mjs";

export { EVIDENCE_LEVELS, SCREENING_STATUSES, SOURCE_TYPES, validateSourceLedger };

async function main() {
  const file = process.argv[2];
  if (!file) throw new Error("Usage: validate-source-ledger.mjs <source-ledger.json>");
  const ledger = JSON.parse(await fs.readFile(file, "utf8"));
  const errors = validateSourceLedger(ledger);
  if (errors.length) {
    for (const error of errors) console.error(error);
    process.exitCode = 1;
  } else {
    console.log("source ledger is valid");
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
