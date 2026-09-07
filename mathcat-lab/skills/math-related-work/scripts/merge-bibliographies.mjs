export { mergeBibliographies, parseBibtexEntries } from "./bibtex-utils.mjs";

import fs from "node:fs/promises";
import path from "node:path";
import { mergeBibliographies } from "./bibtex-utils.mjs";

const [output, ...inputs] = process.argv.slice(2);
if (!output || inputs.length < 1) {
  console.error("Usage: node merge-bibliographies.mjs <output.bib> <input-a.bib> [input-b.bib ...]");
  process.exitCode = 1;
} else {
  try {
    const result = mergeBibliographies(await Promise.all(inputs.map((file) => fs.readFile(file, "utf8"))));
    if (result.conflicts.length) throw new Error(`BibTeX key conflicts: ${result.conflicts.map((item) => item.key).join(", ")}`);
    await fs.mkdir(path.dirname(path.resolve(output)), { recursive: true });
    await fs.writeFile(output, result.text, "utf8");
    console.log(JSON.stringify({ entries: result.entries.length, duplicates: result.duplicates.length, output: path.resolve(output) }));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
