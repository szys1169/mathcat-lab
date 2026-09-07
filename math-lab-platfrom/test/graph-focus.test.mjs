import test from "node:test";
import assert from "node:assert/strict";
import { focusGraphBranch, layoutGraph, relatedGraphIds, visibleGraph } from "../public/research-graph.js";

function graph(ids, links, extra = {}) {
  return {
    kind: "proof",
    rootId: "root",
    source: "backend",
    ...extra,
    nodes: ids.map((id) => ({ id, kind: id === "root" ? "theorem" : "route", label: id, status: "active" })),
    edges: links.map(([from, to, relation = "requires"], index) => ({ id: `edge-${index}`, from, to, relation }))
  };
}

const idsOf = (model) => model.nodes.map((item) => item.id).sort();

test("branch focus keeps its complete ancestors and descendants while shrinking the layout", () => {
  const model = graph(["root", "direction", "a", "a-task", "a-fact", "b", "b-task"], [
    ["root", "direction"], ["direction", "a"], ["a", "a-task", "runs"], ["a", "a-fact", "supports"],
    ["root", "b"], ["b", "b-task", "runs"]
  ]);
  const before = structuredClone(model);
  const focused = focusGraphBranch(model, "a");
  assert.deepEqual(idsOf(focused), ["a", "a-fact", "a-task", "direction", "root"]);
  assert.deepEqual(focused.edges, model.edges.slice(0, 4));
  assert.equal(focused.source, "backend");
  assert.ok(layoutGraph(focused).width < layoutGraph(model).width);
  assert.deepEqual(model, before);
});

test("focusing the root retains the connected whole proof tree", () => {
  const model = graph(["root", "a", "b", "leaf"], [["root", "a"], ["root", "b"], ["a", "leaf"]]);
  assert.deepEqual(focusGraphBranch(model, "root"), model);
});

test("an empty or stale selection leaves the graph visible", () => {
  const model = graph(["root", "a"], [["root", "a"]]);
  assert.equal(focusGraphBranch(model, "missing"), model);
  assert.equal(focusGraphBranch(model, null), model);
});

test("cyclic branch focus does not lose upstream sources already encountered downstream", () => {
  const model = graph(["root", "a", "cycle", "fact", "source", "unrelated"], [
    ["root", "a"], ["a", "cycle"], ["cycle", "a"], ["fact", "cycle", "supports"],
    ["source", "fact", "supports"], ["root", "unrelated"], ["source", "a", "cites"]
  ]);
  assert.deepEqual([...relatedGraphIds(model, "a")].sort(), ["a", "cycle", "fact", "root", "source"]);
  const focused = focusGraphBranch(model, "a");
  assert.deepEqual(idsOf(focused), ["a", "cycle", "fact", "root", "source"]);
  assert.deepEqual(focused.edges, model.edges.filter((item) => item.to !== "unrelated"));
});

test("dependency focus keeps supporting evidence and downstream targets with every internal edge", () => {
  const model = graph(["root", "source", "fact", "claim", "other"], [
    ["source", "fact", "supports"], ["fact", "claim", "supports"], ["source", "claim", "cites"],
    ["claim", "root", "supports"], ["other", "root", "contradicts"]
  ], { kind: "dependency" });
  const focused = focusGraphBranch(model, "claim");
  assert.deepEqual(idsOf(focused), ["claim", "fact", "root", "source"]);
  assert.deepEqual(focused.edges, model.edges.slice(0, 4));
});

test("collapsing a cyclic route retains itself, the root and other live routes", () => {
  const model = graph(["root", "a", "a-task", "b", "b-task"], [
    ["root", "a"], ["a", "a-task"], ["a-task", "root"], ["root", "b"], ["b", "b-task"]
  ]);
  const collapsed = visibleGraph(model, { collapsed: new Set(["a"]) });
  assert.deepEqual(idsOf(collapsed), ["a", "b", "b-task", "root"]);
  assert.equal(collapsed.edges.some((item) => item.from === "a"), false);
});

test("collapsing one route keeps a shared goal reachable through an open route", () => {
  const model = graph(["root", "a", "b", "shared", "leaf"], [
    ["root", "a"], ["root", "b"], ["a", "shared"], ["b", "shared"], ["shared", "leaf"]
  ]);
  const collapsed = visibleGraph(model, { collapsed: new Set(["a"]) });
  assert.deepEqual(idsOf(collapsed), idsOf(model));
  assert.equal(collapsed.edges.some((item) => item.from === "a"), false);
  assert.ok(collapsed.edges.some((item) => item.from === "b" && item.to === "shared"));
  assert.deepEqual(idsOf(visibleGraph(model, { collapsed: new Set(["a", "b"]) })), ["a", "b", "root"]);
});

test("collapsing a deep proof chain is iterative and keeps its root", () => {
  const count = 10_000;
  const ids = Array.from({ length: count }, (_, index) => index ? `n${index}` : "root");
  const model = graph(ids, ids.slice(1).map((id, index) => [ids[index], id]));
  assert.deepEqual(idsOf(visibleGraph(model, { collapsed: new Set(["root"]) })), ["root"]);
});
