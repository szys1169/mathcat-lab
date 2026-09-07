import test from "node:test";
import assert from "node:assert/strict";
import { createResearchDisclosureStore } from "../public/research-disclosure.js";

test("research disclosures start expanded and survive repeated board renders", () => {
  const disclosures = createResearchDisclosureStore();

  assert.equal(disclosures.isCollapsed("board-a", "family", "direct_proof"), false);
  assert.equal(disclosures.toggle("board-a", "family", "direct_proof"), true);

  // A render reads the same key again instead of deriving state from refreshed board data.
  assert.equal(disclosures.isCollapsed("board-a", "family", "direct_proof"), true);
  assert.equal(disclosures.toggle("board-a", "family", "direct_proof"), false);
  assert.equal(disclosures.isCollapsed("board-a", "family", "direct_proof"), false);
});

test("family, route, and proof-node disclosures are independent per board", () => {
  const disclosures = createResearchDisclosureStore();

  disclosures.toggle("board-a", "family", "counterexample");
  disclosures.toggle("board-a", "route", "route-1");
  disclosures.toggle("board-a", "node", "route-1");

  assert.equal(disclosures.isCollapsed("board-a", "family", "counterexample"), true);
  assert.equal(disclosures.isCollapsed("board-a", "route", "route-1"), true);
  assert.equal(disclosures.isCollapsed("board-a", "node", "route-1"), true);
  assert.equal(disclosures.isCollapsed("board-a", "family", "route-1"), false);
  assert.equal(disclosures.isCollapsed("board-b", "family", "counterexample"), false);
  assert.equal(disclosures.isCollapsed("board-b", "route", "route-1"), false);
  assert.equal(disclosures.isCollapsed("board-b", "node", "route-1"), false);
});

test("unknown research disclosure scopes fail loudly", () => {
  const disclosures = createResearchDisclosureStore();
  assert.throws(() => disclosures.toggle("board-a", "unknown", "item"), /Unknown research disclosure scope/);
});
