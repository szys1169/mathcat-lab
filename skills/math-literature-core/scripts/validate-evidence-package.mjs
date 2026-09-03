#!/usr/bin/env node
import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseBibtexEntries } from "./bibtex.mjs";
import { validateSourceLedger } from "./source-ledger.mjs";

export function validateEvidencePackage(sourceLedger, bibtex = null, options = {}) {
  const errors = validateSourceLedger(sourceLedger);
  if (bibtex == null) return errors;
  const bibtexKeys = new Set(parseBibtexEntries(bibtex).map((entry) => entry.key));
  const requireAllIncluded = options.requireAllIncluded !== false;
  for (const [index, source] of (sourceLedger?.sources || []).entries()) {
    if (requireAllIncluded && source.screeningStatus === "included" && !bibtexKeys.has(source.bibtexKey)) {
      errors.push(`sources[${index}].bibtexKey ${source.bibtexKey} is missing from BibTeX`);
    }
  }
  return errors;
}

async function main() {
  const [sourceFile, bibFile] = process.argv.slice(2);
  if (!sourceFile) throw new Error("Usage: node validate-evidence-package.mjs <source-ledger.json> [bibliography.bib]");
  const sourceLedger = JSON.parse(await fs.readFile(sourceFile, "utf8"));
  const bibtex = bibFile ? await fs.readFile(bibFile, "utf8") : null;
  const errors = validateEvidencePackage(sourceLedger, bibtex);
  if (errors.length) {
    for (const error of errors) console.error(error);
    process.exitCode = 1;
  } else {
    console.log("literature evidence package is valid");
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
