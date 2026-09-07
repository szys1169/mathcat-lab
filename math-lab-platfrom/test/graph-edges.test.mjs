import test from "node:test";
import assert from "node:assert/strict";
import { layoutGraph } from "../public/research-graph.js";
import { graphEdgeId, routeGraphEdges } from "../public/graph-edges.js";

function graph(ids, links, kind = "proof") {
  return {
    kind,
    rootId: "root",
    nodes: ids.map((id) => ({ id, kind: id === "root" ? "theorem" : "goal" })),
    edges: links.map(([from, to, relation = "requires"], i) => ({ id: `edge-${i}`, from, to, relation }))
  };
}

function segmentEntersBox([x1, y1], [x2, y2], box) {
  const epsilon = 0.001;
  if (Math.abs(y1 - y2) < epsilon) {
    return y1 > box.y + epsilon && y1 < box.y + box.height - epsilon
      && Math.max(x1, x2) > box.x + epsilon && Math.min(x1, x2) < box.x + box.width - epsilon;
  }
  if (Math.abs(x1 - x2) < epsilon) {
    return x1 > box.x + epsilon && x1 < box.x + box.width - epsilon
      && Math.max(y1, y2) > box.y + epsilon && Math.min(y1, y2) < box.y + box.height - epsilon;
  }
  assert.fail("graph edges should use orthogonal segments");
}

function assertSafeEdgeGeometry(result, layout) {
  for (const edge of result.edges) {
    assert.match(edge.path, /^M/);
    assert.doesNotMatch(edge.path, /NaN|Infinity|C/);
    assert.ok(edge.points.length >= 2);
    for (const [x, y] of edge.points) {
      assert.ok(Number.isFinite(x) && Number.isFinite(y));
      assert.ok(x >= 0 && x <= result.width, `${graphEdgeId(edge)} exceeds canvas horizontally`);
      assert.ok(y >= 0 && y <= result.height, `${graphEdgeId(edge)} exceeds canvas vertically`);
    }
    for (let i = 1; i < edge.points.length; i += 1) {
      for (const [id, box] of layout.positions) {
        assert.equal(segmentEntersBox(edge.points[i - 1], edge.points[i], box), false, `${graphEdgeId(edge)} cuts through node ${id}`);
      }
    }
  }
}

const mixed = graph(
  ["root", "a", "b", "shared"],
  [["root", "a"], ["root", "b"], ["a", "shared"], ["b", "shared"], ["shared", "root"], ["b", "b"], ["a", "b", "contradicts"], ["root", "missing"]]
);

test("proof branches use orthogonal lines in row gaps without entering any cards", () => {
  const model = graph(
    ["root", "a", "b", "c", "c1", "a1", "b1", "a2", "b2", "c2", "a11"],
    [["root", "a"], ["root", "b"], ["root", "c"], ["a", "a1"], ["a", "a2"], ["b", "b1"], ["b", "b2"], ["c", "c1"], ["c", "c2"], ["a1", "a11"]]
  );
  const layout = layoutGraph(model);
  const result = routeGraphEdges(model, layout);
  assert.equal(result.edges.length, model.edges.length);
  assert.equal(result.crossLinkCount, 0);
  assert.equal(result.hiddenCrossLinkCount, 0);
  assert.ok(result.edges.every((edge) => edge.points.length === 4 && !edge.crossLink && !edge.showLabel));
  assertSafeEdgeGeometry(result, layout);
});

test("proof overview hides ordinary cross references while retaining contradictions and accurate per-node counts", () => {
  const layout = layoutGraph(mixed);
  const result = routeGraphEdges(mixed, layout);
  assert.equal(result.crossLinkCount, 4);
  assert.equal(result.hiddenCrossLinkCount, 3);
  assert.equal(result.edges.length, 4);
  assert.deepEqual(result.edges.filter((edge) => edge.crossLink).map((edge) => edge.relation), ["contradicts"]);
  assert.deepEqual(Object.fromEntries(result.crossCounts), { a: 1, b: 3, shared: 2, root: 1 });
  assertSafeEdgeGeometry(result, layout);
});

test("selecting a node shows its incident cross references without revealing unrelated lines", () => {
  const layout = layoutGraph(mixed);
  const result = routeGraphEdges(mixed, layout, { selectedId: "shared" });
  assert.equal(result.crossLinkCount, 4);
  assert.equal(result.hiddenCrossLinkCount, 1);
  assert.equal(result.edges.length, 6);
  const extra = result.edges.filter((edge) => edge.crossLink);
  assert.ok(extra.some((edge) => edge.from === "b" && edge.to === "shared"));
  assert.ok(extra.some((edge) => edge.from === "shared" && edge.to === "root"));
  assert.equal(extra.some((edge) => edge.from === "b" && edge.to === "b"), false);
  assert.ok(result.edges.find((edge) => edge.from === "a" && edge.to === "shared").showLabel);
  assertSafeEdgeGeometry(result, layout);
});

test("show all cross references preserves every valid edge including self loops and back references", () => {
  const layout = layoutGraph(mixed);
  const result = routeGraphEdges(mixed, layout, { showCrossLinks: true });
  assert.equal(result.hiddenCrossLinkCount, 0);
  assert.equal(result.edges.length, mixed.edges.length - 1);
  assert.deepEqual(result.edges.map(({ id }) => id).sort(), mixed.edges.filter(({ to }) => to !== "missing").map(({ id }) => id).sort());
  const rightmostNode = Math.max(...[...layout.positions.values()].map((box) => box.x + box.width));
  for (const edge of result.edges.filter((item) => item.crossLink)) {
    assert.ok(edge.points.some(([x]) => x > rightmostNode), `${edge.id} uses an outside reference lane`);
  }
  assertSafeEdgeGeometry(result, layout);
});

test("proof edges with omitted optional ids and relations remain visible as primary branches", () => {
  const model = graph(["root", "child"], [["root", "child"]]);
  model.edges = [{ from: "root", to: "child" }];
  const layout = layoutGraph(model);
  const result = routeGraphEdges(model, layout);
  assert.equal(graphEdgeId(model.edges[0]), "root:requires:child");
  assert.equal(result.edges.length, 1);
  assert.equal(result.crossLinkCount, 0);
  assertSafeEdgeGeometry(result, layout);
});

test("dependency links that skip columns pass below intervening fact cards", () => {
  const model = graph(
    ["root", "source", "fact-a", "fact-b", "fact-c", "alternate"],
    [["source", "fact-a"], ["fact-a", "fact-b"], ["fact-b", "fact-c"], ["fact-c", "root"], ["source", "root"], ["alternate", "fact-b"]],
    "dependency"
  );
  const layout = layoutGraph(model);
  const result = routeGraphEdges(model, layout);
  assert.equal(result.edges.length, model.edges.length);
  assert.equal(result.crossLinkCount, 0);
  assert.equal(result.hiddenCrossLinkCount, 0);
  const longEdge = result.edges.find((edge) => edge.from === "source" && edge.to === "root");
  const bottommostNode = Math.max(...[...layout.positions.values()].map((box) => box.y + box.height));
  assert.ok(longEdge.points.some(([, y]) => y > bottommostNode), "skip-level dependency takes an exterior lane");
  assertSafeEdgeGeometry(result, layout);
});

test("strongly connected dependency nodes and self references route beside cards without entering them", () => {
  const model = graph(
    ["root", "source", "fact-a", "fact-b", "candidate"],
    [["source", "fact-a"], ["fact-a", "fact-b"], ["fact-b", "fact-a"], ["fact-a", "fact-a"], ["fact-b", "candidate"], ["candidate", "root"]],
    "dependency"
  );
  const layout = layoutGraph(model);
  const result = routeGraphEdges(model, layout);
  assert.equal(result.edges.length, model.edges.length);
  assert.equal(layout.positions.get("fact-a").x, layout.positions.get("fact-b").x);
  const self = result.edges.find((edge) => edge.from === edge.to);
  assert.notDeepEqual(self.points[0], self.points.at(-1), "self reference uses separate start and end ports");
  assertSafeEdgeGeometry(result, layout);
});

test("routing expands graph bounds for many visible references and does not mutate input", () => {
  const ids = ["root", ...Array.from({ length: 18 }, (_, i) => `child-${String(i).padStart(2, "0")}`)];
  const model = graph(ids, ids.slice(1).flatMap((id) => [["root", id], [id, "root"], [id, id]]));
  const layout = layoutGraph(model);
  const beforeModel = structuredClone(model);
  const beforeLayout = structuredClone(layout);
  const result = routeGraphEdges(model, layout, { showCrossLinks: true });
  assert.equal(result.crossLinkCount, 36);
  assert.equal(result.edges.length, 54);
  assert.ok(result.width > layout.width, "outer reference lanes enlarge the canvas");
  assertSafeEdgeGeometry(result, layout);
  assert.deepEqual(model, beforeModel);
  assert.deepEqual(layout, beforeLayout);
  const reversed = routeGraphEdges({ ...model, nodes: [...model.nodes].reverse(), edges: [...model.edges].reverse() }, layout, { showCrossLinks: true });
  assert.deepEqual(reversed, result, "response array order does not shuffle reference lanes");
});
