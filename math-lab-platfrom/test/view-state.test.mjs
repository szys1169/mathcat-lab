import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import { resolveResearchView, isCurrentConversationLoad, normalizeResearchBoard } from "../public/view-state.js";

const appSource = await fs.readFile(new URL("../public/app.js", import.meta.url), "utf8");

test("board view is only allowed when research context is available", () => {
  assert.equal(resolveResearchView("board", true), "board");
  assert.equal(resolveResearchView("board", false), "chat");
  assert.equal(resolveResearchView("invalid", true), "chat");
});

test("stale conversation responses cannot replace the active conversation", () => {
  assert.equal(isCurrentConversationLoad("new", "old", 2, 1), false);
  assert.equal(isCurrentConversationLoad("same", "same", 2, 1), false);
  assert.equal(isCurrentConversationLoad("same", "same", 2, 2), true);
  assert.equal(isCurrentConversationLoad(null, null, 2, 2), false);
});

test("legacy or incomplete research boards are normalized before rendering", () => {
  const mathcat = normalizeResearchBoard({ id: "b1", agent: "mathcat", problem: { statement: "P" } });
  assert.deepEqual(mathcat.routes, []);
  assert.deepEqual(mathcat.decisions, []);
  assert.deepEqual(mathcat.events, []);
  const rethlas = normalizeResearchBoard({ id: "b2", agent: "rethlas" });
  assert.deepEqual(rethlas.blueprint, []);
  assert.equal(rethlas.verification.verdict, "pending");
  assert.equal(rethlas.integration.structured, false);
  const danus = normalizeResearchBoard({ id: "b3", agent: "danus", memories: null });
  assert.deepEqual(danus.workers, []);
  assert.deepEqual(danus.memories.local, []);
});

test("unknown whiteboard agents are not silently rendered as MathCat", () => {
  const board = normalizeResearchBoard({ id: "unknown", agent: "future-agent" });
  assert.equal(board.agent,"future-agent");
  assert.equal(board.integration.source,"unknown");
  assert.match(board.integration.message,/future-agent/);
});

test("legacy MathCat questions remain renderable without optional question-card fields", () => {
  const board = normalizeResearchBoard({ id: "b1", agent: "mathcat", decisions: [{ id: "d1", question: "继续吗？", status: "pending" }] });
  assert.equal(board.decisions[0].question, "继续吗？");
  assert.equal(board.decisions[0].options, undefined);
});

test("an open research board keeps polling after chat activity becomes idle", () => {
  assert.match(appSource, /conversation\.status === "running" \|\| \(state\.activeView === "board" && state\.currentBoard\)/);
});
