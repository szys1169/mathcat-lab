import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function loadEnv(file) {
  if (!fs.existsSync(file)) return;
  for (const raw of fs.readFileSync(file, "utf8").split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const i = line.indexOf("=");
    if (i < 1) continue;
    const key = line.slice(0, i).trim();
    let value = line.slice(i + 1).trim();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) value = value.slice(1, -1);
    if (process.env[key] == null) process.env[key] = value;
  }
}

loadEnv(path.join(appRoot, ".env.local"));

function defaultCodexBin() {
  if (process.platform !== "win32" || !process.env.APPDATA) return "codex";
  const candidate = path.join(process.env.APPDATA, "npm", "node_modules", "@openai", "codex", "node_modules", "@openai", "codex-win32-x64", "vendor", "x86_64-pc-windows-msvc", "bin", "codex.exe");
  return fs.existsSync(candidate) ? candidate : "codex";
}

export const config = Object.freeze({
  host: process.env.MATH_LAB_HOST || "127.0.0.1",
  port: Number.parseInt(process.env.MATH_LAB_PORT || "4335", 10),
  version: "2.5.3",
  appRoot,
  publicRoot: path.join(appRoot, "public"),
  runtimeRoot: path.resolve(process.env.MATH_LAB_RUNTIME_ROOT || path.join(appRoot, "runtime-data")),
  capabilitiesRoot: path.resolve(process.env.MATH_LAB_CAPABILITIES_ROOT || path.join(appRoot, "..", "capabilities")),
  codexBin: process.env.CODEX_BIN || defaultCodexBin(),
  workspacesRoot: path.resolve(process.env.MATHCAT_V2_WORKSPACES_ROOT || path.join(appRoot,"..","workspaces")),
  researchV2: {
    baseUrl: (process.env.MATHCAT_V2_API_URL || "http://127.0.0.1:8900").replace(/\/$/, ""),
    tokenFile: path.resolve(process.env.MATHCAT_V2_TOKEN_FILE || path.join(appRoot,"..","runtime","api-token")),
    timeoutMs: 15000
  },
  mathcat: {
    baseUrl: (process.env.MATHCAT_API_URL || "http://127.0.0.1:8900").replace(/\/$/, ""),
    actorId: process.env.MATHCAT_ACTOR_ID || "mathcat-lab",
    token: process.env.MATHCAT_API_TOKEN || "",
    timeoutMs: Number.parseInt(process.env.MATHCAT_API_TIMEOUT_MS || "10000", 10)
  }
});
