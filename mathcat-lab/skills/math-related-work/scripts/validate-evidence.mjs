import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseBibtexEntries } from "./bibtex-utils.mjs";
import { validateSourceLedger } from "../../math-literature-core/scripts/source-ledger.mjs";

const dimensions = new Set(["object", "assumptions", "conclusion", "method", "scope", "limitation"]);
const relations = new Set(["same", "extends", "specializes", "improves", "weaker_assumptions", "stronger_assumptions", "complementary", "contrasts", "incomparable", "unknown"]);
const gapStatuses = new Set(["verified_gap", "supported_but_incomplete", "author_claim_only", "unknown", "contradicted"]);
const attributionTypes = new Set(["cite_as_existing", "attributed", "open_problem", "present_work", "transition"]);

function nonempty(value) { return typeof value === "string" && value.trim().length > 0; }

export function validateEvidence({ sourceLedger, contributionProfile = null, comparisonMatrix, gapLedger, claimLedger, bibtex }) {
  const errors = validateSourceLedger(sourceLedger).map((error) => `sourceLedger: ${error}`);
  const sourceIds = new Set((sourceLedger?.sources || []).map((source) => source.sourceId));
  const presentWorkIds = new Set((contributionProfile?.mainResults || []).map((result) => result.resultId));
  const bibKeys = new Set(parseBibtexEntries(bibtex || "").map((entry) => entry.key));
  const comparisonIds = new Set();

  function validatePresentWorkIds(ids, label, required = false) {
    if (required && (!Array.isArray(ids) || ids.length === 0)) errors.push(`${label}.presentWorkResultIds must not be empty.`);
    for (const resultId of ids || []) if (!presentWorkIds.has(resultId)) errors.push(`${label} references missing present-work result ${resultId}.`);
  }

  for (const [index, item] of (comparisonMatrix?.comparisons || []).entries()) {
    const label = `comparisons[${index}]`;
    if (!nonempty(item.comparisonId)) errors.push(`${label}.comparisonId is required.`);
    else if (comparisonIds.has(item.comparisonId)) errors.push(`${label}.comparisonId is duplicated.`);
    else comparisonIds.add(item.comparisonId);
    if (!nonempty(item.priorWork) || !nonempty(item.currentWork)) errors.push(`${label} requires priorWork and currentWork.`);
    if (!dimensions.has(item.dimension)) errors.push(`${label}.dimension is invalid.`);
    if (!relations.has(item.relation)) errors.push(`${label}.relation is invalid.`);
    if (!Array.isArray(item.sourceIds) || item.sourceIds.length === 0) errors.push(`${label}.sourceIds must not be empty.`);
    for (const sourceId of item.sourceIds || []) if (!sourceIds.has(sourceId)) errors.push(`${label} references missing source ${sourceId}.`);
    validatePresentWorkIds(item.presentWorkResultIds, label, contributionProfile != null);
    if (item.relation !== "unknown" && !nonempty(item.evidenceLevel)) errors.push(`${label}.evidenceLevel is required for a definite relation.`);
  }

  for (const [index, gap] of (gapLedger?.gaps || []).entries()) {
    const label = `gaps[${index}]`;
    if (!nonempty(gap.gapId) || !nonempty(gap.statement)) errors.push(`${label} requires gapId and statement.`);
    if (!gapStatuses.has(gap.status)) errors.push(`${label}.status is invalid.`);
    for (const sourceId of gap.evidenceSourceIds || []) if (!sourceIds.has(sourceId)) errors.push(`${label} references missing source ${sourceId}.`);
    validatePresentWorkIds(gap.presentWorkResultIds, label, contributionProfile != null && gap.status === "author_claim_only");
    if (gap.status === "verified_gap" && (!gap.evidenceSourceIds?.length || !nonempty(gap.searchCoverage))) errors.push(`${label} verified_gap requires evidenceSourceIds and searchCoverage.`);
    if (gap.status !== "verified_gap" && !nonempty(gap.cautiousWording)) errors.push(`${label} requires cautiousWording unless verified.`);
  }

  for (const [index, claim] of (claimLedger?.claims || []).entries()) {
    const label = `claims[${index}]`;
    if (!nonempty(claim.claimId) || !nonempty(claim.text)) errors.push(`${label} requires claimId and text.`);
    if (!attributionTypes.has(claim.attributionType)) errors.push(`${label}.attributionType is invalid.`);
    const requiresCitation = ["cite_as_existing", "attributed", "open_problem"].includes(claim.attributionType);
    if (requiresCitation && !claim.citationKeys?.length) errors.push(`${label} requires at least one citation key.`);
    for (const key of claim.citationKeys || []) if (!bibKeys.has(key)) errors.push(`${label} references missing BibTeX key ${key}.`);
    for (const sourceId of claim.sourceIds || []) if (!sourceIds.has(sourceId)) errors.push(`${label} references missing source ${sourceId}.`);
    for (const comparisonId of claim.comparisonIds || []) if (!comparisonIds.has(comparisonId)) errors.push(`${label} references missing comparison ${comparisonId}.`);
    validatePresentWorkIds(claim.presentWorkResultIds, label, contributionProfile != null && claim.attributionType === "present_work");
  }
  return errors;
}

async function readJson(file) { return JSON.parse(await fs.readFile(file, "utf8")); }

async function main() {
  const [researchRoot, bibFile, sourceFile] = process.argv.slice(2);
  if (!researchRoot || !bibFile) throw new Error("Usage: node validate-evidence.mjs <research-root> <related-work.bib> [source-ledger.json]");
  const sourceLedgerPath = sourceFile || path.join(researchRoot, "source-ledger.json");
  const errors = validateEvidence({
    sourceLedger: await readJson(sourceLedgerPath),
    contributionProfile: await readJson(path.join(researchRoot, "contribution-profile.json")),
    comparisonMatrix: await readJson(path.join(researchRoot, "literature-comparison-matrix.json")),
    gapLedger: await readJson(path.join(researchRoot, "gap-positioning-ledger.json")),
    claimLedger: await readJson(path.join(researchRoot, "related-work-claim-ledger.json")),
    bibtex: await fs.readFile(bibFile, "utf8")
  });
  if (errors.length) { for (const error of errors) console.error(error); process.exitCode = 1; }
  else console.log("Related-work evidence is valid.");
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
