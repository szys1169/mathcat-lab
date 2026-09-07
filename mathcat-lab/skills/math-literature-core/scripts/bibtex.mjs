#!/usr/bin/env node
import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

export function parseBibtexEntries(text) {
  const entries = [];
  let cursor = 0;
  while (cursor < text.length) {
    const at = text.indexOf("@", cursor);
    if (at < 0) break;
    const openerIndex = text.slice(at).search(/[({]/);
    if (openerIndex < 0) break;
    const open = at + openerIndex;
    const closeChar = text[open] === "{" ? "}" : ")";
    let depth = 1;
    let quoted = false;
    let escaped = false;
    let end = open + 1;
    for (; end < text.length && depth > 0; end += 1) {
      const char = text[end];
      if (escaped) { escaped = false; continue; }
      if (char === "\\") { escaped = true; continue; }
      if (char === '"') { quoted = !quoted; continue; }
      if (quoted) continue;
      if (char === text[open]) depth += 1;
      if (char === closeChar) depth -= 1;
    }
    if (depth !== 0) throw new Error(`Unbalanced BibTeX entry near offset ${at}.`);
    const raw = text.slice(at, end).trim();
    const comma = raw.indexOf(",", openerIndex + 1);
    if (comma < 0) throw new Error(`BibTeX entry near offset ${at} has no citation key.`);
    const key = raw.slice(openerIndex + 1, comma).trim();
    if (!key) throw new Error(`BibTeX entry near offset ${at} has an empty citation key.`);
    const doi = raw.match(/\bdoi\s*=\s*[{"]\s*([^}"\s,]+)[}"]/i)?.[1]?.replace(/^https?:\/\/(?:dx\.)?doi\.org\//i, "").toLowerCase() || null;
    entries.push({ key, doi, raw });
    cursor = end;
  }
  return entries;
}

export function mergeBibliographies(texts) {
  const kept = [];
  const byKey = new Map();
  const byDoi = new Map();
  const duplicates = [];
  const conflicts = [];
  for (const [sourceIndex, text] of texts.entries()) {
    for (const entry of parseBibtexEntries(text)) {
      const normalizedKey = entry.key.toLowerCase();
      const normalizedRaw = entry.raw.replace(/\s+/g, " ").trim().toLowerCase();
      const keyMatch = byKey.get(normalizedKey);
      const doiMatch = entry.doi ? byDoi.get(entry.doi) : null;
      if (keyMatch) {
        if (keyMatch.normalizedRaw === normalizedRaw) duplicates.push({ key: entry.key, reason: "identical_key_and_entry", sourceIndex });
        else conflicts.push({ key: entry.key, reason: "same_key_different_entry", sourceIndex });
        continue;
      }
      if (doiMatch) {
        duplicates.push({ key: entry.key, keptKey: doiMatch.key, reason: "same_doi", sourceIndex });
        continue;
      }
      const record = { ...entry, normalizedRaw };
      kept.push(record);
      byKey.set(normalizedKey, record);
      if (entry.doi) byDoi.set(entry.doi, record);
    }
  }
  return { text: `${kept.map((entry) => entry.raw).join("\n\n")}\n`, entries: kept, duplicates, conflicts };
}

export function citationKeysFromTex(text) {
  const keys = new Set();
  for (const match of text.matchAll(/\\cite\w*\s*(?:\[[^\]]*\]\s*){0,2}\{([^}]+)\}/g)) {
    for (const key of match[1].split(",").map((value) => value.trim()).filter(Boolean)) keys.add(key);
  }
  return keys;
}

async function main() {
  const [output, ...inputs] = process.argv.slice(2);
  if (!output || inputs.length < 1) throw new Error("Usage: node bibtex.mjs <output.bib> <input-a.bib> [input-b.bib ...]");
  const result = mergeBibliographies(await Promise.all(inputs.map((file) => fs.readFile(file, "utf8"))));
  if (result.conflicts.length) throw new Error(`BibTeX key conflicts: ${result.conflicts.map((item) => item.key).join(", ")}`);
  await fs.mkdir(path.dirname(path.resolve(output)), { recursive: true });
  await fs.writeFile(output, result.text, "utf8");
  console.log(JSON.stringify({ entries: result.entries.length, duplicates: result.duplicates.length, output: path.resolve(output) }));
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
