import test from "node:test";
import assert from "node:assert/strict";
import { buildProofTree, layoutGraph, visibleGraph, focusGraphBranch, buildDependencyGraph } from "../public/research-graph.js";
import { routeGraphEdges } from "../public/graph-edges.js";

// Minimal regression fixture from Test03 / continuing Conjecture 3.6, revision 243.
const titles = ["半长度—Jordan 归约", "显式非拟齐次参数族及其纯幂 suspension", "Euler-trivial summand 的 suspension 不变性", "初始次数归约"];
function test03() {
  return {
    id: "test03-revision243", agent: "mathcat", status: "partial_success",
    problem: { statement: "证明或反驳猜想3.6" }, goals: [{ id: "goal", statement: "猜想3.6", parent_goal_ids: [] }], routes: [],
    failedRoutes: [{ id: "R1", title: "结构直接证明", status: "pruned", humanStatus: "approved" }, { id: "R2", title: "认证计算反例", status: "pruned", humanStatus: "approved" }],
    claims: [
      ...titles.map((statement, index) => ({ id: `c${index}`, statement, kind: "candidate", status: index === 2 ? "rejected" : "promoted_to_fact", origin_route_id: "R2", verification_id: `v${index}` })),
      ...[0, 1, 3].map((index) => ({ id: `f${index}`, statement: titles[index], kind: "fact", status: "accepted", origin_route_id: null, verification_id: `v${index}` }))
    ],
    verificationQueue: titles.map((_, index) => ({ verification_id: `v${index}`, candidate_id: `c${index}`, status: index === 2 ? "rejected" : "accepted" }))
  };
}

test("Test03 historical R2 retains all four candidates and three verified Fact lineages", () => {
  const board = test03(), original = structuredClone(board);
  const model = buildProofTree(board), layout = layoutGraph(model);
  assert.equal(model.nodes.find((n) => n.id === "R2").status, "archived");
  assert.equal(model.nodes.find((n) => n.id === "R2").meta.activity, null);
  for (let i = 0; i < 4; i++) {
    assert.equal(layout.parentById.get(`c${i}`), "R2");
    assert.equal(layout.parentById.get(`v${i}`), `c${i}`);
    assert.ok(model.edges.some((e) => e.from === "R2" && e.to === `c${i}` && e.relation === "produces"));
  }
  for (const i of [0, 1, 3]) {
    assert.equal(layout.parentById.get(`f${i}`), `v${i}`);
    assert.equal(model.nodes.find((n) => n.id === `f${i}`).trust, "fact");
  }
  assert.equal(model.nodes.find((n) => n.id === "c2").status, "failed");
  assert.equal(model.nodes.some((n) => n.id === "f2"), false);
  assert.equal(model.edges.some((e) => e.from === "goal" && /^(c|f|v)\d/.test(e.to)), false);
  assert.equal(model.edges.some((e) => e.relation === "requires" && /^(c|f|v)\d/.test(e.to)), false);
  assert.ok(model.nodes.filter((n) => n.kind === "strategy").every((n) => !n.meta.planned));
  assert.deepEqual(board, original);
  assert.equal(routeGraphEdges(model, layout, { showCrossLinks: true }).edges.length, model.edges.length);
});

test("archiving a live route does not reparent its candidates, verifications or Facts", () => {
  const archived = test03(), live = structuredClone(archived);
  live.routes = [{ ...live.failedRoutes.pop(), status: "active" }];
  const a = layoutGraph(buildProofTree(archived)), b = layoutGraph(buildProofTree(live));
  for (const [id, parent] of a.parentById) if (/^(c|v|f)\d/.test(id)) assert.equal(b.parentById.get(id), parent);
});

test("focus and collapse keep Test03 provenance without reparenting surviving records", () => {
  const model = buildProofTree(test03()), before = layoutGraph(model);
  for (const node of model.nodes) for (const view of [focusGraphBranch(model, node.id), visibleGraph(model, { collapsed: new Set([node.id]) })]) {
    for (const [id, parent] of layoutGraph(view).parentById) if (/^(c|v|f)\d/.test(id)) assert.equal(parent, before.parentById.get(id));
  }
  const collapsed = visibleGraph(model, { collapsed: new Set(["R2"]) });
  assert.ok(collapsed.nodes.some((n) => n.id === "R2"));
  assert.equal(collapsed.nodes.some((n) => /^(c|v|f)\d/.test(n.id)), false);
});

test("missing origin routes get an identified placeholder, not a made-up goal dependency", () => {
  const board = test03(); board.failedRoutes = [];
  const model = buildProofTree(board);
  assert.equal(model.nodes.find((n) => n.id === "R2").meta.missingSource, true);
  assert.equal(layoutGraph(model).parentById.get("c0"), "R2");
});

test("unattributed records use a non-logical collection and identical prose never establishes provenance", () => {
  const model = buildProofTree({ id: "unknown", agent: "mathcat", claims: [
    { id: "c", kind: "candidate", statement: "相同陈述", routeId: "r", verification_id: "v" },
    { id: "f", kind: "fact", statement: "相同陈述", status: "accepted" },
    { kind: "fact", statement: "没有编号的记录" }
  ] });
  const collection = model.nodes.find((n) => n.kind === "provenance");
  assert.equal(layoutGraph(model).parentById.get("f"), collection.id);
  assert.ok(model.edges.some((e) => e.to === collection.id && e.relation === "records"));
  assert.ok(model.nodes.filter((n) => n.kind === "fact").every((n) => layoutGraph(model).parentById.get(n.id) === collection.id));
});

test("exact shared verification ids recover lineage when verification details are absent", () => {
  const board = test03(); board.verificationQueue = [];
  const model = buildProofTree(board), layout = layoutGraph(model);
  assert.equal(layout.parentById.get("f0"), "v0");
  assert.equal(layout.parentById.get("v0"), "c0");
  assert.match(model.nodes.find((n) => n.id === "v0").label, /详情未提供/);
});

test("ambiguous shared verification ids do not choose an arbitrary candidate", () => {
  const board = test03(); board.verificationQueue = [];
  board.claims.push({ id: "other", kind: "candidate", statement: "另一个候选", verification_id: "v0" });
  const model = buildProofTree(board), parent = layoutGraph(model).parentById.get("f0");
  assert.equal(model.nodes.find((n) => n.id === parent).kind, "provenance");
});

test("active verification keeps its candidate parent despite the route activity shortcut", () => {
  const board = test03(); board.routes = [{ ...board.failedRoutes.pop(), status: "active" }];
  board.claims.find((c) => c.id === "c0").status = "verifying";
  board.verificationQueue[0].status = "verifying";
  const model = buildProofTree(board);
  assert.ok(model.edges.some((e) => e.from === "R2" && e.to === "v0" && e.relation === "checks"));
  assert.equal(layoutGraph(model).parentById.get("v0"), "c0");
  assert.equal(visibleGraph(model, { collapsed: new Set(["c0"]) }).nodes.some((n) => n.id === "v0"), false);
});

test("explicit historical routes and alias endpoints preserve their original edges", () => {
  const board = { failedRoutes: [{ id: "r", status: "pruned", title: "历史路线" }], claims: [{ id: "hyp", kind: "hypothesis", routeId: "r" }],
    proofTree: { rootId: "g", nodes: [{ id: "g", kind: "theorem" }, { id: "hyp", kind: "route" }, { id: "c", kind: "candidate" }], edges: [{ from: "g", to: "hyp", relation: "explores" }, { from: "hyp", to: "c", relation: "produces" }] } };
  const model = buildProofTree(board);
  assert.equal(model.nodes.find((n) => n.id === "r").status, "archived");
  assert.deepEqual(model.edges.map((e) => [e.from, e.to]), [["g", "r"], ["r", "c"]]);
  // Authoritative dependency graphs are not enriched with unproven logical edges.
  assert.deepEqual(buildDependencyGraph({ ...board, dependencyGraph: { nodes: [], edges: [] } }).edges, []);
});
