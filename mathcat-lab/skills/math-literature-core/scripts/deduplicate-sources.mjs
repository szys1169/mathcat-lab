#!/usr/bin/env node
import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { deduplicateSources } from "./source-ledger.mjs";

async function main() {
  const [inputFile, outputFile] = process.argv.slice(2);
  if (!inputFile || !outputFile) throw new Error("Usage: deduplicate-sources.mjs <input.json> <output.json>");
  const input = JSON.parse(await fs.readFile(inputFile, "utf8"));
  const isArray = Array.isArray(input);
  const sources = isArray ? input : input.sources;
  if (!Array.isArray(sources)) throw new Error("Input must be an array or an object with a sources array.");
  const deduped = deduplicateSources(sources);
  const output = isArray ? deduped : { ...input, sources: deduped, deduplication: { inputCount: sources.length, outputCount: deduped.length } };
  await fs.writeFile(outputFile, `${JSON.stringify(output, null, 2)}\n`, "utf8");
  console.log(`deduplicated ${sources.length} sources to ${deduped.length}`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
