import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import net from "node:net";
import { spawn } from "node:child_process";

async function freePort() {
  const server = net.createServer();
  await new Promise((resolve, reject) => server.once("error", reject).listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return port;
}

async function waitForServer(child, port) {
  let output = "";
  child.stdout.on("data", (chunk) => { output += chunk; });
  child.stderr.on("data", (chunk) => { output += chunk; });
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (child.exitCode != null) throw new Error(`Stress server exited early: ${output}`);
    try { const response = await fetch(`http://127.0.0.1:${port}/api/conversations`); if (response.ok) return; } catch {}
    await new Promise((resolve) => setTimeout(resolve, 40));
  }
  throw new Error(`Stress server did not start: ${output}`);
}

test("isolated HTTP server survives concurrent conversations and board updates", { timeout: 30_000 }, async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "math-lab-http-stress-"));
  const port = await freePort();
  const appRoot = path.resolve(import.meta.dirname, "..");
  const child = spawn(process.execPath, ["src/server.mjs"], {
    cwd: appRoot,
    env: { ...process.env, MATH_LAB_PORT: String(port), MATH_LAB_RUNTIME_ROOT: root, MATHCAT_API_URL: "http://127.0.0.1:1", MATHCAT_API_TIMEOUT_MS: "100" },
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true
  });
  t.after(() => { if (child.exitCode == null) child.kill(); });
  await waitForServer(child, port);
  const request = async (pathname, options = {}) => {
    const response = await fetch(`http://127.0.0.1:${port}${pathname}`, { ...options, headers: { "content-type": "application/json", ...(options.headers || {}) } });
    const body = await response.json();
    if (!response.ok) throw new Error(`${response.status} ${JSON.stringify(body)}`);
    return body;
  };

  const conversations = await Promise.all(Array.from({ length: 100 }, (_, index) => request("/api/conversations", { method: "POST", body: JSON.stringify({ workspaceId: null, title: `HTTP 并发 ${index}` }) })));
  assert.equal(new Set(conversations.map((item) => item.id)).size, 100);
  const listed = await request("/api/conversations");
  assert.equal(listed.length, 100);

  const boards = await Promise.all(conversations.slice(0, 40).map((conversation, index) => request("/api/research-boards", { method: "POST", body: JSON.stringify({ conversationId: conversation.id, agent: "rethlas", problem: `命题 ${index}` }) })));
  assert.equal(boards.length, 40);
  const target = boards[0];
  await Promise.all(Array.from({ length: 100 }, (_, index) => request(`/api/research-boards/${target.id}/decisions`, { method: "POST", body: JSON.stringify({ question: `并发问题 ${index}`, options: [{ value: "yes", label: "是" }, { value: "no", label: "否" }] }) })));
  const finalBoard = await request(`/api/research-boards/${target.id}`);
  assert.equal(finalBoard.decisions.length, 100);
  assert.equal(finalBoard.revision, 101);

  const workspacePath = path.join(root, "untrusted-workspace");
  await fs.mkdir(workspacePath, { recursive: true });
  const activeFile = path.join(workspacePath, "agent-output.html");
  await fs.writeFile(activeFile, "<script>document.body.textContent='unsafe'</script>", "utf8");
  const workspace = await request("/api/workspaces", { method: "POST", body: JSON.stringify({ workspacePath, name: "Untrusted output" }) });
  const fileConversation = await request("/api/conversations", { method: "POST", body: JSON.stringify({ workspaceId: workspace.id, title: "Active content" }) });
  const activeResponse = await fetch(`http://127.0.0.1:${port}/api/workspace-file?conversationId=${encodeURIComponent(fileConversation.id)}&path=${encodeURIComponent(activeFile)}`);
  assert.equal(activeResponse.status, 200);
  assert.equal(activeResponse.headers.get("content-type"), "application/octet-stream");
  assert.match(activeResponse.headers.get("content-disposition"), /^attachment;/);
  assert.equal(activeResponse.headers.get("x-content-type-options"), "nosniff");
  assert.match(activeResponse.headers.get("content-security-policy"), /sandbox/);
  assert.equal(child.exitCode, null);
});
