export const EVIDENCE_LEVELS = new Set(["full_text_checked", "primary_metadata_only", "abstract_only", "secondary_source_only", "unverified"]);
export const SCREENING_STATUSES = new Set(["included", "excluded", "pending"]);
export const SOURCE_TYPES = new Set(["original_problem", "classic", "breakthrough", "special_case", "generalization", "counterexample", "method", "survey", "recent", "metadata_only"]);

function normalizedDoi(value) {
  return String(value || "").trim().toLowerCase().replace(/^https?:\/\/(?:dx\.)?doi\.org\//, "").replace(/^doi:\s*/, "");
}

function normalizedArxiv(value) {
  return String(value || "").trim().toLowerCase().replace(/^arxiv:\s*/, "").replace(/v\d+$/, "");
}

function normalizedTitle(value) {
  return String(value || "").normalize("NFKD").toLowerCase().replace(/[^\p{L}\p{N}]+/gu, " ").trim();
}

export function sourceKey(source) {
  const doi = normalizedDoi(source?.doi);
  if (doi) return `doi:${doi}`;
  const arxiv = normalizedArxiv(source?.arxivId);
  if (arxiv) return `arxiv:${arxiv}`;
  return `title:${normalizedTitle(source?.title)}|${Number(source?.year) || ""}|${normalizedTitle(source?.authors?.[0] || "")}`;
}

export function deduplicateSources(sources) {
  const groups = new Map();
  for (const source of sources || []) {
    const key = sourceKey(source);
    if (!groups.has(key)) {
      groups.set(key, { ...source, alternateVersions: [...(source.alternateVersions || [])] });
      continue;
    }
    const primary = groups.get(key);
    primary.alternateVersions.push({
      sourceId: source.sourceId,
      doi: source.doi || null,
      arxivId: source.arxivId || null,
      url: source.url || null,
      localPath: source.localPath || null
    });
    primary.categories = [...new Set([...(primary.categories || []), ...(source.categories || [])])];
    primary.urls = [...new Set([...(primary.urls || []), primary.url, source.url].filter(Boolean))];
  }
  return [...groups.values()];
}

export function validateSourceLedger(ledger) {
  const errors = [];
  if (!ledger || typeof ledger !== "object" || Array.isArray(ledger)) return ["ledger must be an object"];
  if (ledger.schemaVersion !== "0.1") errors.push("schemaVersion must be 0.1");
  if (typeof ledger.taskId !== "string" || !ledger.taskId.trim()) errors.push("taskId is required");
  if (typeof ledger.searchCutoff !== "string" || !ledger.searchCutoff.trim()) errors.push("searchCutoff is required");
  if (!Array.isArray(ledger.sources)) errors.push("sources must be an array");
  const sourceIds = new Set();
  const bibtexKeys = new Set();
  for (const [index, source] of (Array.isArray(ledger.sources) ? ledger.sources : []).entries()) {
    const prefix = `sources[${index}]`;
    for (const field of ["sourceId", "title", "bibtexKey"]) {
      if (typeof source?.[field] !== "string" || !source[field].trim()) errors.push(`${prefix}.${field} is required`);
    }
    if (!Array.isArray(source?.authors) || !source.authors.length || source.authors.some((author) => typeof author !== "string" || !author.trim())) errors.push(`${prefix}.authors must be a non-empty string array`);
    if (!Number.isInteger(source?.year) || source.year < 1400 || source.year > 2200) errors.push(`${prefix}.year is invalid`);
    if (!SOURCE_TYPES.has(source?.sourceType)) errors.push(`${prefix}.sourceType is invalid`);
    if (!SCREENING_STATUSES.has(source?.screeningStatus)) errors.push(`${prefix}.screeningStatus is invalid`);
    if (!EVIDENCE_LEVELS.has(source?.evidenceLevel)) errors.push(`${prefix}.evidenceLevel is invalid`);
    if (!source?.doi && !source?.arxivId && !source?.url) errors.push(`${prefix} requires doi, arxivId, or url`);
    if (source?.sourceId) {
      if (sourceIds.has(source.sourceId)) errors.push(`${prefix}.sourceId is duplicated`);
      sourceIds.add(source.sourceId);
    }
    if (source?.bibtexKey) {
      if (bibtexKeys.has(source.bibtexKey)) errors.push(`${prefix}.bibtexKey is duplicated`);
      bibtexKeys.add(source.bibtexKey);
    }
    if (source?.fullTextStatus === "downloaded") {
      if (typeof source.localPath !== "string" || !source.localPath.trim()) errors.push(`${prefix}.localPath is required for downloaded sources`);
      if (typeof source.sha256 !== "string" || !/^[a-f0-9]{64}$/i.test(source.sha256)) errors.push(`${prefix}.sha256 must be a 64-character hex digest`);
      if (typeof source.retrievedAt !== "string" || !source.retrievedAt.trim()) errors.push(`${prefix}.retrievedAt is required for downloaded sources`);
    }
  }
  return errors;
}
