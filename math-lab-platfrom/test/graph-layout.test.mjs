import test from "node:test";
import assert from "node:assert/strict";
import { layoutGraph, visibleGraph } from "../public/research-graph.js";

function graph(ids, links, kind = "proof") {
  return {
    kind,
    rootId: "root",
    nodes: ids.map((id) => ({ id, kind: id === "root" ? "theorem" : "goal", status: "active" })),
    edges: links.map(([from, to]) => ({ id: `${from}->${to}`, from, to, relation: "requires" }))
  };
}

function assertCompleteNonOverlapping(model, layout) {
  assert.equal(layout.positions.size, model.nodes.length, "every node remains visible exactly once");
  assert.ok(Number.isFinite(layout.width) && Number.isFinite(layout.height));
  const positioned = model.nodes.map(({ id }) => [id, layout.positions.get(id)]);
  for (const [id, box] of positioned) {
    assert.ok(box, `missing position: ${id}`);
    for (const value of [box.x, box.y, box.width, box.height]) assert.ok(Number.isFinite(value), `${id} has finite bounds`);
    assert.ok(box.x >= 0 && box.y >= 0 && box.width > 0 && box.height > 0);
    assert.ok(box.x + box.width <= layout.width, `${id} fits horizontally`);
    assert.ok(box.y + box.height <= layout.height, `${id} fits vertically`);
  }
  for (let i = 0; i < positioned.length; i += 1) {
    for (let j = i + 1; j < positioned.length; j += 1) {
      const [aId, a] = positioned[i];
      const [bId, b] = positioned[j];
      const overlap = a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
      assert.equal(overlap, false, `${aId} and ${bId} overlap`);
    }
  }
}

function descendants(model, rootId) {
  const children = new Map();
  for (const { from, to } of model.edges) {
    if (!children.has(from)) children.set(from, []);
    children.get(from).push(to);
  }
  const result = new Set([rootId]);
  const queue = [rootId];
  for (let i = 0; i < queue.length; i += 1) {
    for (const id of children.get(queue[i]) || []) {
      if (result.has(id)) continue;
      result.add(id);
      queue.push(id);
    }
  }
  return [...result];
}

function subtreeBounds(model, layout, id) {
  const boxes = descendants(model, id).map((child) => layout.positions.get(child));
  return { left: Math.min(...boxes.map((box) => box.x)), right: Math.max(...boxes.map((box) => box.x + box.width)) };
}

function assertSeparatedSubtrees(model, layout, roots) {
  const spans = roots.map((id) => ({ id, ...subtreeBounds(model, layout, id) })).sort((a, b) => a.left - b.left);
  for (let i = 1; i < spans.length; i += 1) {
    assert.ok(spans[i - 1].right < spans[i].left, `${spans[i - 1].id} and ${spans[i].id} have interwoven descendants`);
  }
}

function crossingsAtSameDepth(model, layout, edges = model.edges) {
  let count = 0;
  for (let i = 0; i < edges.length; i += 1) {
    const a = edges[i];
    const af = layout.positions.get(a.from);
    const at = layout.positions.get(a.to);
    for (let j = i + 1; j < edges.length; j += 1) {
      const b = edges[j];
      if (a.from === b.from || a.to === b.to) continue;
      const bf = layout.positions.get(b.from);
      const bt = layout.positions.get(b.to);
      if (af.y === bf.y && at.y === bt.y && (af.x - bf.x) * (at.x - bt.x) < 0) count += 1;
    }
  }
  return count;
}

const interleaved = graph(
  ["root", "a", "b", "c", "b1", "c1", "a1", "c2", "a2", "b2"],
  [["root", "a"], ["root", "b"], ["root", "c"], ["a", "a1"], ["a", "a2"], ["b", "b1"], ["b", "b2"], ["c", "c1"], ["c", "c2"]]
);

test("interleaved input order does not weave sibling research branches together", () => {
  const layout = layoutGraph(interleaved);
  assert.equal(crossingsAtSameDepth(interleaved, layout), 0, "independent research branches must not cross");
  assertSeparatedSubtrees(interleaved, layout, ["a", "b", "c"]);
  assertCompleteNonOverlapping(interleaved, layout);
});

test("unequal and nested branches reserve space for their entire descendant trees", () => {
  const model = graph(
    ["root", "short", "wide", "deep", "s1", "w1", "d1", "w2", "d2", "w3", "d3", "w11", "w12", "w13"],
    [["root", "short"], ["root", "wide"], ["root", "deep"], ["short", "s1"], ["wide", "w1"], ["wide", "w2"], ["wide", "w3"], ["w1", "w11"], ["w1", "w12"], ["w1", "w13"], ["deep", "d1"], ["d1", "d2"], ["d2", "d3"]]
  );
  const layout = layoutGraph(model);
  assertSeparatedSubtrees(model, layout, ["short", "wide", "deep"]);
  assertSeparatedSubtrees(model, layout, ["w1", "w2", "w3"]);
  for (const id of ["root", "wide", "w1", "deep"]) {
    const box = layout.positions.get(id);
    const span = subtreeBounds(model, layout, id);
    assert.ok(Math.abs(box.x + box.width / 2 - (span.left + span.right) / 2) < 0.001, `${id} is centered above its subtree`);
  }
  assertCompleteNonOverlapping(model, layout);
});

test("refreshes that reorder nodes or edges preserve automatic tree positions", () => {
  const expected = layoutGraph(interleaved);
  const reordered = layoutGraph({ ...interleaved, nodes: [...interleaved.nodes].reverse(), edges: [...interleaved.edges].reverse() });
  for (const { id } of interleaved.nodes) assert.deepEqual(reordered.positions.get(id), expected.positions.get(id), `${id} jumps when response ordering changes`);
  assert.equal(reordered.width, expected.width);
  assert.equal(reordered.height, expected.height);
});

test("a 400-level proof chain remains a narrow readable vertical tree", () => {
  const ids = ["root", ...Array.from({ length: 400 }, (_, i) => `step-${i}`)];
  const model = graph([...ids].reverse(), ids.slice(1).map((id, i) => [ids[i], id]));
  const layout = layoutGraph(model);
  const rootCenter = layout.positions.get("root").x;
  for (const { from, to } of model.edges) {
    assert.ok(layout.positions.get(from).y + layout.positions.get(from).height < layout.positions.get(to).y);
    assert.equal(layout.positions.get(to).x, rootCenter, "a linear proof must not drift across the canvas");
  }
  assertCompleteNonOverlapping(model, layout);
});

test("large research trees keep each direction and each route in its own column of descendants", () => {
  const ids = ["root"];
  const links = [];
  const directions = [];
  const routeGroups = [];
  for (let d = 0; d < 5; d += 1) {
    const direction = `direction-${d}`;
    directions.push(direction);
    ids.push(direction);
    links.push(["root", direction]);
    const routes = [];
    for (let r = 0; r < 12; r += 1) {
      const route = `${direction}:route-${r}`;
      routes.push(route);
      ids.push(route);
      links.push([direction, route]);
      for (let c = 0; c < 9; c += 1) {
        const claim = `${route}:claim-${c}`;
        ids.push(claim);
        links.push([route, claim]);
      }
    }
    routeGroups.push(routes);
  }
  const model = graph([...ids].reverse(), [...links].reverse());
  const layout = layoutGraph(model);
  assert.equal(model.nodes.length, 606);
  assertSeparatedSubtrees(model, layout, directions);
  for (const routes of routeGroups) assertSeparatedSubtrees(model, layout, routes);
  assert.equal(crossingsAtSameDepth(model, layout), 0);
  assertCompleteNonOverlapping(model, layout);
});

test("shared goals, back references and disconnected fragments form a forest without losing nodes", () => {
  const model = graph(
    ["root", "a", "b", "shared", "leaf", "fragment", "fragment-leaf", "unattached"],
    [["root", "a"], ["root", "b"], ["a", "shared"], ["b", "shared"], ["shared", "leaf"], ["leaf", "a"], ["fragment", "fragment-leaf"]]
  );
  const snapshot = structuredClone(model);
  const layout = layoutGraph(model);
  assert.ok(layout.treeEdgeIds instanceof Set);
  assert.ok(layout.parentById instanceof Map);
  const primaryEdges = model.edges.filter(({ id }) => layout.treeEdgeIds.has(id));
  assert.equal(primaryEdges.length, 5, "three forest roots leave exactly five primary edges");
  assert.equal(primaryEdges.filter(({ to }) => to === "shared").length, 1, "a shared goal has only one visual parent");
  assert.equal(layout.treeEdgeIds.has("leaf->a"), false, "a back reference is not a primary branch");
  for (const { from, to } of primaryEdges) {
    assert.equal(layout.parentById.get(to), from);
    assert.ok(layout.positions.get(from).y + layout.positions.get(from).height < layout.positions.get(to).y, `${from}->${to} descends`);
  }
  assert.deepEqual(model, snapshot, "layout must retain all original graph relationships");
  assertCompleteNonOverlapping(model, layout);
  const reordered = layoutGraph({ ...model, nodes: [...model.nodes].reverse(), edges: [...model.edges].reverse() });
  for (const { id } of model.nodes) assert.deepEqual(reordered.positions.get(id), layout.positions.get(id));
  assert.deepEqual([...reordered.treeEdgeIds].sort(), [...layout.treeEdgeIds].sort());
});

test("collapsing one research branch preserves the other branches and their readable layout", () => {
  const collapsed = visibleGraph(interleaved, { collapsed: new Set(["b"]) });
  assert.deepEqual(collapsed.nodes.map(({ id }) => id).sort(), ["root", "a", "b", "c", "c1", "a1", "c2", "a2"].sort());
  const layout = layoutGraph(collapsed);
  assertSeparatedSubtrees(collapsed, layout, ["a", "b", "c"]);
  assert.equal(crossingsAtSameDepth(collapsed, layout), 0);
  assertCompleteNonOverlapping(collapsed, layout);
});

test("dependencies between successive facts get separate layers instead of same-column arrows", () => {
  const model = graph(["root", "source", "fact-a", "fact-b", "fact-c"], [["source", "fact-a"], ["fact-a", "fact-b"], ["fact-b", "fact-c"], ["fact-c", "root"]], "dependency");
  model.nodes.forEach((item) => { item.kind = item.id === "root" ? "theorem" : item.id === "source" ? "source" : "fact"; });
  const layout = layoutGraph(model);
  for (const { from, to } of model.edges) assert.ok(layout.positions.get(from).x + layout.positions.get(from).width < layout.positions.get(to).x, `${from}->${to} follows the evidence direction`);
  assertCompleteNonOverlapping(model, layout);
});

test("cyclic dependency components and multiple source types stay complete and non-overlapping", () => {
  const model = graph(["root", "source", "artifact", "fact-a", "fact-b", "candidate"], [["source", "fact-a"], ["artifact", "fact-b"], ["fact-a", "fact-b"], ["fact-b", "fact-a"], ["fact-b", "candidate"], ["candidate", "root"]], "dependency");
  model.nodes.forEach((item) => { item.kind = item.id === "root" ? "theorem" : item.id.startsWith("fact") ? "fact" : item.id; });
  const layout = layoutGraph(model);
  assertCompleteNonOverlapping(model, layout);
  assert.ok(layout.positions.get("source").x < layout.positions.get("fact-a").x);
  assert.ok(layout.positions.get("artifact").x < layout.positions.get("fact-b").x);
  assert.ok(layout.positions.get("fact-b").x < layout.positions.get("candidate").x);
  assert.ok(layout.positions.get("candidate").x < layout.positions.get("root").x);
});
