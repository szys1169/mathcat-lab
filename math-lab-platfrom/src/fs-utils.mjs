import fs from "node:fs/promises";
import path from "node:path";
import crypto from "node:crypto";

export function ensureInside(root, candidate) {
  const base = path.resolve(root);
  const value = path.resolve(candidate);
  if (value !== base && !value.startsWith(base + path.sep)) throw new Error("Path is outside the selected workspace.");
  return value;
}

export async function writeJsonAtomic(file, value) {
  await fs.mkdir(path.dirname(file), { recursive: true });
  const temp = `${file}.${process.pid}.${Date.now()}.${crypto.randomUUID()}.tmp`;
  await fs.writeFile(temp, JSON.stringify(value, null, 2) + "\n", "utf8");
  await fs.rename(temp, file);
}

export class QueuedJsonWriter {
  constructor(file) { this.file = file; this.pending = Promise.resolve(); }
  save(value) {
    const snapshot = structuredClone(value);
    const write = this.pending.catch(() => {}).then(() => writeJsonAtomic(this.file, snapshot));
    this.pending = write;
    return write;
  }
}

export function safeId(value, fallback = "item") {
  const id = String(value || "").normalize("NFKC").replace(/[^\p{L}\p{N}._-]+/gu, "-").replace(/^-+|-+$/g, "").slice(0, 80);
  return id || fallback;
}
