import path from "node:path";
import { spawn } from "node:child_process";

// Confirmed against the locally generated codex-cli 0.151.0 app-server schema.
// Never start a thread/turn, refresh credentials, consume a reset, or write config.
const METHODS = new Set(["initialize", "account/rateLimits/read", "model/list", "config/read"]);
const failure = (code) => Object.assign(new Error(code), { code });
const record = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const text = (value, max = 200) => typeof value === "string" && value.trim() ? value.trim().replace(/[\u0000-\u001f\u007f]/g, " ").slice(0, max) : null;
const identifier = (value) => typeof value === "string" && /^[A-Za-z0-9][A-Za-z0-9_./:+-]{0,159}$/.test(value) && !/^sk[-_]/i.test(value) ? value : null;
const number = (value) => typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
const clone = (value) => structuredClone(value);

function bounded(promise, timeoutMs) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(failure("timeout")), Math.max(1, timeoutMs));
    Promise.resolve(promise).then((value) => { clearTimeout(timer); resolve(value); }, (error) => { clearTimeout(timer); reject(error); });
  });
}

async function terminateOwned(child, spawnProcess) {
  if (!child.pid || child.exitCode != null || child.signalCode != null) return;
  if (process.platform !== "win32") { try { process.kill(-child.pid, "SIGTERM"); } catch { child.kill("SIGTERM"); } return; }
  // No name-based killing: this is only the PID created for this read and its tree.
  await new Promise((resolve) => {
    const killer = spawnProcess("taskkill.exe", ["/PID", String(child.pid), "/T", "/F"], { windowsHide: true, shell: false, stdio: "ignore" });
    const timer = setTimeout(resolve, 1000);
    const finish = () => { clearTimeout(timer); resolve(); };
    killer.once("close", finish);
    killer.once("error", () => { try { child.kill(); } catch {} finish(); });
  });
}

function jsonlClient({ config, spawnProcess, terminateProcess }) {
  const child = spawnProcess(config.codexBin || "codex", ["app-server", "--stdio"], {
    cwd: config.appRoot || process.cwd(), windowsHide: true, shell: false,
    detached: process.platform !== "win32", stdio: ["pipe", "pipe", "pipe"],
  });
  let sequence = 0, buffer = "", closed = false, broken = false;
  const pending = new Map();
  const failAll = (code) => { broken = true; for (const promise of pending.values()) promise.reject(failure(code)); pending.clear(); };
  child.stdout.setEncoding("utf8");
  // CLI diagnostics and full config responses can contain private paths/settings.
  // Do not retain stderr, persist raw JSONL, or expose protocol errors to the UI.
  child.stderr?.resume();
  child.stdin.on("error", () => failAll("connection"));
  child.once("error", () => failAll("launch"));
  child.once("close", () => { closed = true; failAll("connection"); });
  child.stdout.on("data", (chunk) => {
    buffer += chunk;
    if (buffer.length > 4 * 1024 * 1024) { buffer = ""; failAll("protocol"); return; }
    let newline;
    while ((newline = buffer.indexOf("\n")) !== -1) {
      const line = buffer.slice(0, newline).trim(); buffer = buffer.slice(newline + 1);
      if (!line) continue;
      let message;
      try { message = JSON.parse(line); } catch { failAll("protocol"); continue; }
      if (!record(message)) continue;
      if (Object.hasOwn(message, "id") && message.method) {
        // A read-only client never fulfills server requests for tools or tokens.
        child.stdin.write(JSON.stringify({ id: message.id, error: { code: -32601, message: "Read-only client does not support server requests" } }) + "\n");
        continue;
      }
      const promise = pending.get(String(message.id));
      if (!promise) continue;
      pending.delete(String(message.id));
      if (message.error) promise.reject(failure("rpc"));
      else if (Object.hasOwn(message, "result")) promise.resolve(message.result);
      else promise.reject(failure("protocol"));
    }
  });
  return {
    request(method, params) {
      if (!METHODS.has(method)) return Promise.reject(failure("method"));
      if (closed || broken) return Promise.reject(failure("connection"));
      const id = ++sequence;
      return new Promise((resolve, reject) => {
        pending.set(String(id), { resolve, reject });
        try { child.stdin.write(JSON.stringify({ id, method, params }) + "\n"); }
        catch { pending.delete(String(id)); reject(failure("connection")); }
      });
    },
    notify(method) {
      if (method !== "initialized" || closed || broken) throw failure("connection");
      child.stdin.write(JSON.stringify({ method }) + "\n");
    },
    async close() {
      failAll("closed"); buffer = "";
      if (closed) return;
      try { child.stdin.end(); } catch {}
      await new Promise((resolve) => {
        const timer = setTimeout(resolve, 150);
        child.once("close", () => { clearTimeout(timer); resolve(); });
      });
      if (!closed) await terminateProcess(child, spawnProcess);
    },
  };
}

function rateLimits(response) {
  if (!record(response)) throw failure("protocol");
  const multiple = record(response.rateLimitsByLimitId) ? Object.entries(response.rateLimitsByLimitId) : [];
  const entries = multiple.length ? multiple : record(response.rateLimits) ? [[response.rateLimits.limitId || "codex", response.rateLimits]] : [];
  if (!entries.length) throw failure("protocol");
  return entries.filter(([, bucket]) => record(bucket)).map(([id, bucket]) => ({
    id: text(bucket.limitId) || text(id) || "codex",
    name: text(bucket.limitName) || text(bucket.limitId) || text(id) || "Codex",
    windows: ["primary", "secondary"].filter((name) => record(bucket[name])).map((name) => {
      const source = bucket[name], used = number(source.usedPercent);
      return { name, used_percent: used, remaining_percent: used === null ? null : Math.min(100, Math.max(0, 100 - used)), window_minutes: number(source.windowDurationMins), resets_at: number(source.resetsAt) };
    }),
  }));
}

function modelRow(row) {
  if (!record(row) || !identifier(row.id) || !identifier(row.model)) return null;
  return {
    id: row.id, model: row.model, display_name: text(row.displayName) || row.model,
    default_reasoning_effort: identifier(row.defaultReasoningEffort),
    supported_reasoning_efforts: Array.isArray(row.supportedReasoningEfforts) ? row.supportedReasoningEfforts.filter((option) => identifier(option?.reasoningEffort)).map((option) => ({ effort: option.reasoningEffort, description: text(option.description, 600) || "" })) : [],
  };
}

/** Reads the host CLI's catalog/config/quota; never reads auth files or sends inference. */
export function createCodexAccount({ config = {}, connect, spawnProcess = spawn, terminateProcess = terminateOwned, now = Date.now, ttlMs = 45_000, timeoutMs = 15_000 } = {}) {
  const cacheMs = Math.max(30_000, Math.min(60_000, Number(ttlMs) || 45_000));
  const readTimeout = Math.max(10, Math.min(30_000, Number(timeoutMs) || 15_000));
  let cached = null, cachedAt = 0, inFlight = null, lastCompleteAt = null;
  const good = { models: null, rate_limits: null, configured: null };

  async function attempt() {
    let client;
    const started = Date.now();
    const limit = (operation) => bounded(Promise.resolve().then(operation), readTimeout - (Date.now() - started));
    const outcomes = {};
    let rawModels = [];
    try {
      client = await limit(() => connect ? connect({ config }) : jsonlClient({ config, spawnProcess, terminateProcess }));
      await limit(() => client.request("initialize", { clientInfo: { name: "mathcat_account_status", title: "MathCat account status", version: String(config.version || "2.5.3") }, capabilities: { experimentalApi: false } }));
      await limit(() => client.notify("initialized"));
      const results = await Promise.allSettled([
        limit(() => client.request("account/rateLimits/read", null)).then(rateLimits),
        (async () => {
          const rows = [], cursors = new Set();
          let cursor = null;
          for (let page = 0; page < 10; page += 1) {
            let response;
            try { response = await limit(() => client.request("model/list", { limit: 100, includeHidden: false, ...(cursor ? { cursor } : {}) })); }
            catch (error) { if (rows.length) return { rows, complete: false }; throw error; }
            if (!Array.isArray(response?.data)) { if (rows.length) return { rows, complete: false }; throw failure("protocol"); }
            rows.push(...response.data.filter((row) => row?.hidden !== true));
            cursor = typeof response.nextCursor === "string" && response.nextCursor ? response.nextCursor : null;
            if (!cursor) return { rows, complete: true };
            if (cursors.has(cursor)) return { rows, complete: false };
            cursors.add(cursor);
          }
          return { rows, complete: false };
        })(),
        limit(() => client.request("config/read", { includeLayers: false, cwd: path.resolve(config.appRoot || process.cwd()) })).then((response) => {
          if (!record(response?.config)) throw failure("protocol");
          // Deliberately discard all other config fields, origins and layers.
          return { model: identifier(response.config.model), reasoning_effort: identifier(response.config.model_reasoning_effort) };
        }),
      ]);
      for (const [index, name] of ["rate_limits", "models", "configured"].entries()) outcomes[name] = results[index];
    } catch {
      for (const name of ["rate_limits", "models", "configured"]) outcomes[name] = { status: "rejected" };
    } finally {
      try { await bounded(client?.close?.(), 1500); } catch {}
    }

    const checkedAt = new Date(now()).toISOString();
    const result = { updated_at: null, checked_at: checkedAt, status: "unavailable", rate_limits: [], models: [], configured: { model: null, reasoning_effort: null } };
    const errors = [];
    function fallback(name, explanation) {
      if (good[name]) {
        result[name] = clone(good[name].data);
        result[`${name}_status`] = "partial";
        result[`${name}_stale`] = true;
        result[`${name}_updated_at`] = good[name].updated_at;
        errors.push(`${explanation}，暂显示上次成功读取的数据。`);
      } else {
        result[`${name}_status`] = "unavailable";
        result[`${name}_stale`] = false;
        result[`${name}_updated_at`] = null;
        errors.push(`${explanation}，请确认本机 Codex 可用后刷新。`);
      }
    }
    function accept(name, data, usable, complete = true) {
      result[name] = data;
      result[`${name}_status`] = usable ? complete ? "available" : "partial" : "unavailable";
      result[`${name}_stale`] = false;
      result[`${name}_updated_at`] = usable ? checkedAt : null;
      if (usable && (complete || !good[name])) good[name] = { data: clone(data), updated_at: checkedAt };
      if (!usable && complete) good[name] = null;
    }
    if (outcomes.rate_limits.status === "fulfilled") {
      const data = outcomes.rate_limits.value;
      const usable = data.some((bucket) => bucket.windows.some((window) => window.used_percent !== null));
      accept("rate_limits", data, usable);
      if (!usable) errors.push("当前账号未提供百分比额度；未知额度不会显示为 0% 或 100%。");
    } else fallback("rate_limits", "暂时无法读取账号额度");

    if (outcomes.models.status === "fulfilled") {
      rawModels = outcomes.models.value.rows;
      const seen = new Set();
      const data = rawModels.map(modelRow).filter((row) => { if (!row || seen.has(row.id)) return false; seen.add(row.id); return true; });
      const complete = outcomes.models.value.complete && rawModels.every((row) => modelRow(row) !== null);
      if (!complete && good.models) fallback("models", "暂时无法完整读取可用模型");
      else accept("models", data, data.length > 0, complete);
      if (!complete && !result.models_stale) errors.push("模型列表只读取到部分内容，可以使用已列出的模型或稍后刷新。");
      else if (!data.length) errors.push("本机 Codex 暂未返回可用模型列表。");
    } else fallback("models", "暂时无法读取可用模型");

    if (outcomes.configured.status === "fulfilled") {
      const configured = outcomes.configured.value;
      // A successful config read with no override uses the observed catalog default.
      // A failed config read must never silently substitute that catalog default.
      const catalogDefault = rawModels.find((row) => row.isDefault === true && identifier(row.model));
      configured.model ||= catalogDefault?.model || null;
      const selected = result.models.find((row) => row.model === configured.model || row.id === configured.model);
      configured.reasoning_effort ||= selected?.default_reasoning_effort || null;
      accept("configured", configured, Boolean(configured.model && configured.reasoning_effort));
      if (!configured.model || !configured.reasoning_effort) errors.push("默认模型或推理强度尚未确定，缺失项保留为空。");
    } else fallback("configured", "暂时无法读取本机默认配置");

    const statuses = [result.models_status, result.rate_limits_status, result.configured_status];
    result.status = statuses.every((state) => state === "available") ? "available" : statuses.some((state) => state !== "unavailable") || result.configured.model ? "partial" : "unavailable";
    if (result.status === "available") lastCompleteAt = checkedAt;
    result.updated_at = lastCompleteAt || [result.models_updated_at, result.rate_limits_updated_at, result.configured_updated_at].filter(Boolean).sort().at(-1) || null;
    if (errors.length) result.error = errors.join(" ");
    return result;
  }

  return {
    async read({ force = false } = {}) {
      if (inFlight) return clone(await inFlight);
      if (!force && cached && now() - cachedAt < cacheMs) return clone(cached);
      inFlight = attempt().catch(() => ({
        updated_at: cached?.updated_at || null, checked_at: new Date(now()).toISOString(),
        status: cached ? "partial" : "unavailable", rate_limits: clone(cached?.rate_limits || []), models: clone(cached?.models || []), configured: clone(cached?.configured || { model: null, reasoning_effort: null }),
        models_status: cached?.models?.length ? "partial" : "unavailable", rate_limits_status: cached?.rate_limits?.some((bucket) => bucket.windows.some((window) => window.used_percent !== null)) ? "partial" : "unavailable",
        models_stale: Boolean(cached?.models?.length), rate_limits_stale: Boolean(cached?.rate_limits?.length),
        models_updated_at: cached?.models_updated_at || null, rate_limits_updated_at: cached?.rate_limits_updated_at || null,
        error: "本机 Codex 状态暂时无法读取，请稍后刷新。",
      }));
      try { cached = await inFlight; cachedAt = now(); return clone(cached); }
      finally { inFlight = null; }
    },
  };
}
