# References and dependency closure

Adapted from Rethlas `check-referenced-statements`, with missing-source handling corrected.

For every essential external reference, read the supplied original or query `search-arxiv-theorems` with the necessary statement when network access is allowed. Native web search may supply missing originals. Expand source definitions, hypotheses, ambient objects, formulas and quantifiers. Check both the theorem match and the exact deduction made from it. A real theorem can still be misapplied.

Log source and location, exact statement, applicable conditions and the downstream inference. False application is an error; an omitted specialization is a gap. A failed search, missing PDF or service outage is unresolved material, not evidence a reference is fictional. Search hits remain untrusted until compared to their originals. Do not send whole private papers to a search service.

For internal dependencies, read the statements/proofs in the packet and track the full closure. An ID or previous review label alone is not sufficient. Record checked dependency IDs; identify circular, challenged, stale and missing premises. Review the supplied prior issues against this version and mark them resolved with reasons or still open.
