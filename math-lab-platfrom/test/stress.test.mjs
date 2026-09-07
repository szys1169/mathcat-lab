import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { Store } from "../src/store.mjs";
import { ResearchBoards } from "../src/research-boards.mjs";
import { layoutGraph, visibleGraph, overrideGraphPositions, relatedGraphIds } from "../public/research-graph.js";

test("large proof graphs keep every node finite and searchable", () => {
  const count = 2500;
  const nodes = Array.from({ length: count }, (_, index) => ({ id: `n${index}`, kind: index ? "lemma" : "theorem", label: `引理 ${index}`, shortLabel: `引理 ${index}`, status: index % 11 ? "pending" : "blocked", trust: "unverified" }));
  const edges = Array.from({ length: count - 1 }, (_, index) => ({ id: `e${index}`, from: `n${Math.floor(index / 4)}`, to: `n${index + 1}`, relation: "requires" }));
  const model = { kind: "proof", rootId: "n0", nodes, edges };
  const startedAt = performance.now();
  const layout = layoutGraph(model);
  assert.equal(layout.positions.size, count);
  for (const position of layout.positions.values()) assert.ok(Number.isFinite(position.x) && Number.isFinite(position.y));
  const filtered = visibleGraph(model, { query: "引理 2499" });
  assert.ok(filtered.nodes.some((item) => item.id === "n2499"));
  const moved = overrideGraphPositions(layout, new Map([["n2499", { x: 9000, y: 7000 }]]));
  assert.ok(moved.width > 9000 && moved.height > 7000);
  assert.ok(relatedGraphIds(model, "n624").size >= 2);
  assert.ok(performance.now() - startedAt < 10_000);
});

test("deep proof chains lay out in linear time regardless of edge order", () => {
  const count = 10_000;
  const nodes = Array.from({ length: count }, (_, index) => ({ id: `d${index}`, kind: index ? "lemma" : "theorem", label: `D${index}`, status: "pending" }));
  const edges = Array.from({ length: count - 1 }, (_, index) => ({ from: `d${index}`, to: `d${index + 1}`, relation: "requires" })).reverse();
  const startedAt = performance.now();
  const layout = layoutGraph({ kind: "proof", rootId: "d0", nodes, edges });
  assert.equal(layout.positions.size, count);
  assert.ok(layout.positions.get("d9999").y > layout.positions.get("d9998").y);
  assert.ok(performance.now() - startedAt < 2_000);
});

test("store preserves 120 concurrent conversations and updates", async () => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "math-lab-store-stress-"));
  const store = await new Store(path.join(root, "runtime")).load();
  const conversations = await Promise.all(Array.from({ length: 120 }, (_, index) => store.createConversation({ workspaceId: null, title: `并发对话 ${index}` })));
  await Promise.all(conversations.map((conversation, index) => store.updateConversation(conversation.id, (row) => row.messages.push({ role: "user", content: `消息 ${index}` }))));
  const restored = await new Store(path.join(root, "runtime")).load();
  assert.equal(restored.listConversations().length, 120);
  assert.ok(conversations.every((conversation) => restored.getConversation(conversation.id)?.messages.length === 1));
});

test("research board persists 100 concurrent decisions without write collisions", async () => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "math-lab-board-stress-"));
  const boards = await new ResearchBoards(root).load();
  const board = await boards.create({ conversationId: "stress", workspaceId: null, agent: "mathcat", problem: "证明大型命题" });
  await Promise.all(Array.from({ length: 100 }, (_, index) => boards.addDecision(board.id, { question: `问题 ${index}`, options: [{ value: "yes", label: "是" }, { value: "no", label: "否" }] })));
  const restored = await new ResearchBoards(root).load();
  assert.equal(restored.get(board.id).decisions.length, 100);
  assert.equal(restored.get(board.id).revision, 101);
});

test("mixed concurrent board mutations remain valid JSON", async () => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "math-lab-board-mixed-stress-"));
  const boards = await new ResearchBoards(root).load();
  const board = await boards.create({ conversationId: "mixed", workspaceId: null, agent: "mathcat", problem: "P" });
  await Promise.all([
    ...Array.from({ length: 40 }, (_, index) => boards.addRoute(board.id, { title: `路线 ${index}`, summary: "并发路线" })),
    ...Array.from({ length: 40 }, (_, index) => boards.addDecision(board.id, { question: `判断 ${index}`, options: [] })),
    ...Array.from({ length: 20 }, (_, index) => boards.updateProblem(board.id, { statement: `P${index}` }))
  ]);
  const raw = await fs.readFile(path.join(root, "research-boards.json"), "utf8");
  assert.doesNotThrow(() => JSON.parse(raw));
  const restored = await new ResearchBoards(root).load();
  assert.equal(restored.get(board.id).routes.length, 42);
  assert.equal(restored.get(board.id).decisions.length, 40);
  assert.equal(restored.get(board.id).revision, 101);
});
