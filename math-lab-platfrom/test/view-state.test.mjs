import test from "node:test";
import assert from "node:assert/strict";
import { resolveResearchView, isCurrentConversationLoad, normalizeResearchBoard } from "../public/view-state.js";

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
  const danus = normalizeResearchBoard({ id: "b3", agent: "danus", memories: null });
  assert.deepEqual(danus.workers, []);
  assert.deepEqual(danus.memories.local, []);
});

test("legacy MathCat questions remain renderable without optional question-card fields", () => {
  const board = normalizeResearchBoard({ id: "b1", agent: "mathcat", decisions: [{ id: "d1", question: "继续吗？", status: "pending" }] });
  assert.equal(board.decisions[0].question, "继续吗？");
  assert.equal(board.decisions[0].options, undefined);
});
