import fs from "node:fs/promises";
import path from "node:path";

export async function loadCapabilities(root) {
  const indexFile = path.join(root, "capability-index.json");
  const index = JSON.parse(await fs.readFile(indexFile, "utf8"));
  const rows = [];
  for (const item of index.capabilities || []) {
    const capabilityRoot = path.resolve(root, item.root);
    const manifest = JSON.parse(await fs.readFile(path.join(capabilityRoot, "capability.json"), "utf8"));
    rows.push({ ...item, name: manifest.name || item.id, version: manifest.version, contractVersion: manifest.contract_version, root: capabilityRoot, skillFile: path.join(capabilityRoot, item.skill), adapterFile: path.join(capabilityRoot, item.adapter) });
  }
  return rows;
}
