import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const coverageFields = ["researchObject", "closestResults", "methods", "currentStatus"];
function nonempty(value) { return typeof value === "string" && value.trim().length > 0; }

export function assessRoute({ contributionProfile, sourceLedger, reuseManifest }) {
  const missingContribution = [];
  if (!nonempty(contributionProfile?.researchObject)) missingContribution.push("researchObject");
  if (!nonempty(contributionProfile?.problem)) missingContribution.push("problem");
  if (!Array.isArray(contributionProfile?.mainResults) || contributionProfile.mainResults.length === 0) missingContribution.push("mainResults");
  if (missingContribution.length) {
    return { route: "needs_clarification", readyToWrite: false, missingContribution, missingCoverage: coverageFields };
  }

  if (!Array.isArray(sourceLedger?.sources) || sourceLedger.sources.length === 0) {
    return { route: "cold_start", readyToWrite: false, missingContribution: [], missingCoverage: coverageFields };
  }

  const missingCoverage = coverageFields.filter((field) => reuseManifest?.coverage?.[field] !== true);
  const conflicts = Array.isArray(reuseManifest?.conflicts) ? reuseManifest.conflicts.filter(Boolean) : [];
  if (missingCoverage.length || conflicts.length) {
    return { route: "supplement", readyToWrite: false, missingContribution: [], missingCoverage, conflicts };
  }
  return { route: "fast", readyToWrite: true, missingContribution: [], missingCoverage: [], conflicts: [] };
}

async function main() {
  const [profileFile, sourceFile, manifestFile] = process.argv.slice(2);
  if (!profileFile || !sourceFile || !manifestFile) throw new Error("Usage: node assess-route.mjs <contribution-profile.json> <source-ledger.json> <reused-source-manifest.json>");
  const [contributionProfile, sourceLedger, reuseManifest] = await Promise.all([profileFile, sourceFile, manifestFile].map(async (file) => JSON.parse(await fs.readFile(file, "utf8"))));
  console.log(JSON.stringify(assessRoute({ contributionProfile, sourceLedger, reuseManifest }), null, 2));
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
