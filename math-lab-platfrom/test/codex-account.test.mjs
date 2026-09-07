import test from "node:test";
import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { PassThrough, Writable } from "node:stream";
import { createCodexAccount } from "../src/codex-account.mjs";

const catalog = (id = "model-a", extra = {}) => ({ id, model: id, displayName: "测试模型", isDefault: true, hidden: false, defaultReasoningEffort: "high", supportedReasoningEfforts: [{ reasoningEffort: "high", description: "深入推理" }, { reasoningEffort: "low", description: "简短推理" }], ignored: "do-not-display-secret", ...extra });
const quota = (used = 25) => ({ rateLimits: { limitId: "legacy", primary: { usedPercent: 99 } }, rateLimitsByLimitId: { codex: { limitId: "codex", limitName: "Codex", primary: { usedPercent: used, windowDurationMins: 300, resetsAt: 1900000000 }, secondary: { usedPercent: 0, windowDurationMins: null, resetsAt: null } } } });
const privateValue = "do-not-display-secret";
function connection(handler, receipt) {
  return {
    async request(method, params) {
      receipt.methods.push({ method, params });
      if (method === "initialize") return {};
      return handler(method, params);
    },
    notify(method) { receipt.methods.push({ method }); },
    close() { receipt.closed += 1; },
  };
}
const reply = (method) => {
  if (method === "account/rateLimits/read") return quota();
  if (method === "model/list") return { data: [catalog()], nextCursor: null };
  if (method === "config/read") return { config: { model: "model-a", model_reasoning_effort: "low", access_token: privateValue, providers: { private: privateValue } }, origins: { secret: privateValue } };
  throw new Error("Unexpected method");
};

test("read-only protocol projects safe fields, paginates and preserves unknown quota", async () => {
  const receipt = { methods: [], closed: 0 };
  const client = createCodexAccount({ connect: () => connection((method, params) => {
    if (method === "model/list") return params.cursor ? { data: [catalog("model-b", { isDefault: false })], nextCursor: null } : { data: [catalog()], nextCursor: "next" };
    if (method === "account/rateLimits/read") return { ...quota(), rateLimitsByLimitId: { ...quota().rateLimitsByLimitId, fast: { primary: { usedPercent: 110 }, secondary: { usedPercent: null, windowDurationMins: 60 } } } };
    if (method === "config/read") return { config: { model: null, model_reasoning_effort: null, token: privateValue } };
    return reply(method);
  }, receipt) });
  const result = await client.read();
  assert.equal(result.status, "available");
  assert.equal(result.models.length, 2);
  assert.deepEqual(Object.keys(result.models[0]), ["id", "model", "display_name", "default_reasoning_effort", "supported_reasoning_efforts"]);
  assert.deepEqual(result.configured, { model: "model-a", reasoning_effort: "high" });
  assert.equal(result.rate_limits[0].id, "codex");
  assert.equal(result.rate_limits[0].windows[0].remaining_percent, 75);
  assert.equal(result.rate_limits[0].windows[1].remaining_percent, 100);
  assert.equal(result.rate_limits[1].windows[0].remaining_percent, 0);
  assert.equal(result.rate_limits[1].windows[1].remaining_percent, null);
  assert.equal(result.rate_limits[1].windows[1].used_percent, null);
  assert.equal(result.rate_limits[1].windows[1].resets_at, null);
  assert.ok(!JSON.stringify(result).includes(privateValue));
  assert.deepEqual(receipt.methods.slice(0, 2).map((item) => item.method), ["initialize", "initialized"]);
  assert.ok(receipt.methods.every((item) => ["initialize", "initialized", "account/rateLimits/read", "model/list", "config/read"].includes(item.method)));
  assert.equal(receipt.methods.find((item) => item.method === "account/rateLimits/read").params, null);
  assert.equal(receipt.methods.find((item) => item.method === "config/read").params.includeLayers, false);
  assert.equal(receipt.closed, 1);
});

test("cache coalesces refreshes and endpoint failures retain successful data timestamps", async () => {
  let time = 1900000000000, calls = 0;
  const receipt = { methods: [], closed: 0 };
  const client = createCodexAccount({ now: () => time, connect: () => {
    const turn = ++calls;
    return connection((method) => {
      if (turn === 2 && method === "account/rateLimits/read") throw new Error(privateValue);
      if (turn === 3 && method === "model/list") throw new Error(privateValue);
      return reply(method);
    }, receipt);
  } });
  const [first, same] = await Promise.all([client.read(), client.read({ force: true })]);
  assert.equal(calls, 1); assert.deepEqual(first, same);
  first.models.length = 0;
  assert.equal((await client.read()).models.length, 1);
  assert.equal(calls, 1);
  time += 50_000;
  const partial = await client.read();
  assert.equal(calls, 2); assert.equal(partial.status, "partial");
  assert.equal(partial.models_status, "available"); assert.equal(partial.models_stale, false);
  assert.equal(partial.rate_limits_status, "partial"); assert.equal(partial.rate_limits_stale, true);
  assert.equal(partial.rate_limits_updated_at, same.rate_limits_updated_at);
  assert.equal(partial.updated_at, same.updated_at);
  assert.notEqual(partial.models_updated_at, same.models_updated_at);
  assert.ok(!JSON.stringify(partial).includes(privateValue));
  time += 1_000;
  const quotaFresh = await client.read({ force: true });
  assert.equal(calls, 3); assert.equal(quotaFresh.rate_limits_status, "available");
  assert.equal(quotaFresh.models_status, "partial"); assert.equal(quotaFresh.models_stale, true);
  assert.equal(quotaFresh.models_updated_at, partial.models_updated_at);
  assert.equal(quotaFresh.models.length, 1);
});

test("absent account quota is not invented and models remain usable", async () => {
  const receipt = { methods: [], closed: 0 };
  const client = createCodexAccount({ connect: () => connection((method) => method === "account/rateLimits/read" ? { rateLimits: { primary: null, secondary: null, credits: { unlimited: true } } } : reply(method), receipt) });
  const result = await client.read();
  assert.equal(result.status, "partial"); assert.equal(result.rate_limits_status, "unavailable");
  assert.equal(result.models_status, "available");
  assert.deepEqual(result.rate_limits[0].windows, []);
  assert.match(result.error, /未提供百分比额度/);
});

test("initialization failures and timeouts never expose raw errors or throw to UI", async () => {
  const failed = createCodexAccount({ connect: () => ({ request: async () => { throw new Error(privateValue); }, close() {} }) });
  const absent = await failed.read();
  assert.equal(absent.status, "unavailable");
  assert.deepEqual(absent.configured, { model: null, reasoning_effort: null });
  assert.ok(!JSON.stringify(absent).includes(privateValue));
  let closed = false;
  const timeout = createCodexAccount({ timeoutMs: 25, connect: () => ({
    request: async (method) => method === "initialize" ? {} : method === "account/rateLimits/read" ? new Promise(() => {}) : reply(method),
    notify() {}, close() { closed = true; },
  }) });
  const partial = await timeout.read();
  assert.equal(partial.status, "partial"); assert.equal(partial.models_status, "available");
  assert.equal(partial.rate_limits_status, "unavailable"); assert.equal(closed, true);
});

test("interrupted pagination retains the prior complete model list", async () => {
  let calls = 0;
  const receipt = { methods: [], closed: 0 };
  const client = createCodexAccount({ connect: () => {
    const turn = ++calls;
    return connection((method, params) => {
      if (method !== "model/list") return reply(method);
      if (turn === 1) return { data: [catalog(), catalog("model-b", { isDefault: false })] };
      if (params.cursor) throw new Error(privateValue);
      return { data: [catalog()], nextCursor: "second" };
    }, receipt);
  } });
  await client.read();
  const partial = await client.read({ force: true });
  assert.equal(partial.models.length, 2); assert.equal(partial.models_status, "partial");
  assert.equal(partial.models_stale, true);
});

test("JSONL transport uses hidden owned process, rejects token requests, and discards diagnostics", async () => {
  const writes = [], launches = [], killed = [];
  const child = new EventEmitter();
  child.pid = 12345; child.exitCode = null; child.signalCode = null;
  child.stdout = new PassThrough(); child.stderr = new PassThrough();
  child.stdin = new Writable({ write(chunk, _encoding, callback) {
    const message = JSON.parse(String(chunk)); writes.push(message); callback();
    if (!message.method || !Object.hasOwn(message, "id")) return;
    const result = message.method === "initialize" ? {} : reply(message.method);
    setImmediate(() => {
      const line = JSON.stringify({ id: message.id, result }) + "\n";
      child.stdout.write(line.slice(0, 11)); child.stdout.write(line.slice(11));
      if (message.method === "initialize") child.stdout.write(JSON.stringify({ method: "account/chatgptAuthTokens/refresh", id: 900, params: { privateValue } }) + "\n");
      child.stderr.write(privateValue);
    });
  } });
  const client = createCodexAccount({ config: { codexBin: "fixture-codex.exe" }, spawnProcess: (...args) => { launches.push(args); return child; }, terminateProcess: async (owned) => { killed.push(owned.pid); owned.exitCode = 0; owned.emit("close", 0); } });
  const result = await client.read();
  assert.equal(result.status, "available");
  assert.equal(launches.length, 1); assert.equal(launches[0][2].windowsHide, true);
  assert.equal(launches[0][2].shell, false); assert.deepEqual(launches[0][1], ["app-server", "--stdio"]);
  assert.deepEqual(killed, [12345]);
  assert.equal(writes.find((message) => message.id === 900).error.code, -32601);
  assert.ok(!JSON.stringify(result).includes(privateValue));
  assert.ok(!writes.some((message) => /thread\/|turn\/|config\/.*write|consume/i.test(message.method || "")));
});
