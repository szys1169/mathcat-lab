import test from "node:test";
import assert from "node:assert/strict";
import { buildProofTree, buildDependencyGraph, graphSummary, visibleGraph, layoutGraph } from "../public/research-graph.js";

const board = {
  id: "board_1",
  agent: "mathcat",
  status: "running",
  problem: { statement: "证明主定理 T" },
  routes: [
    { id: "route_a", title: "归纳法", status: "active", summary: "对维数归纳" },
    { id: "route_b", title: "极小反例法", status: "blocked", summary: "缺少有限性" }
  ],
  goals: [{ id: "goal_a", routeId: "route_a", statement: "证明归纳步", status: "active" }],
  sources: [{ id: "source_a", title: "参考论文", verified: false }],
  claims: [
    { id: "fact_a", kind: "fact", statement: "基础情形成立", status: "accepted", routeId: "route_a" },
    { id: "candidate_a", kind: "candidate", statement: "归纳步成立", status: "verifying", routeId: "route_a", dependencyFactIds: ["fact_a"], sourceIds: ["source_a"] }
  ]
};

test("MathCat proof tree keeps the target as an OR root and attaches routes and claims", () => {
  const model = buildProofTree(board);
  assert.equal(model.nodes.find((item) => item.id === model.rootId).logic, "OR");
  assert.ok(model.edges.some((item) => item.from === model.rootId && item.to === "route_a"));
  assert.ok(model.edges.some((item) => item.from === "route_a" && item.to === "candidate_a"));
  const layout = layoutGraph(model);
  assert.ok(layout.positions.get(model.rootId).y < layout.positions.get("route_a").y);
});

test("dependency graph points evidence toward claims and the target", () => {
  const model = buildDependencyGraph(board);
  assert.ok(model.edges.some((item) => item.from === "source_a" && item.to === "candidate_a"));
  assert.ok(model.edges.some((item) => item.from === "fact_a" && item.to === "candidate_a"));
  assert.ok(model.edges.some((item) => item.from === "candidate_a" && item.to === model.rootId));
  const layout = layoutGraph(model);
  assert.ok(layout.positions.get("source_a").x < layout.positions.get(model.rootId).x);
});

test("graph filters preserve the path to the root and proof branches can collapse", () => {
  const proof = buildProofTree(board);
  const collapsed = visibleGraph(proof, { collapsed: new Set(["route_a"]) });
  assert.ok(collapsed.nodes.some((item) => item.id === "route_a"));
  assert.ok(!collapsed.nodes.some((item) => item.id === "goal_a"));
  const problems = visibleGraph(proof, { filter: "problems" });
  assert.ok(problems.nodes.some((item) => item.id === proof.rootId));
  assert.ok(problems.nodes.some((item) => item.id === "route_b"));
  assert.ok(!problems.nodes.some((item) => item.id === "goal_a"));
});

test("Fact counts stay separate from unverified dependencies", () => {
  const summary = graphSummary(buildDependencyGraph(board));
  assert.equal(summary.verified, 1);
  assert.equal(summary.unverified, 2);
});

test("explicit backend fact nodes retain trusted Fact semantics", () => {
  const model = buildDependencyGraph({ dependencyGraph: { rootId: "goal", nodes: [{ id: "goal", kind: "goal", label: "G", status: "open" }, { id: "f1", kind: "fact", label: "F", status: "active" }], edges: [{ from: "f1", to: "goal", relation: "supports" }] } });
  const summary = graphSummary(model);
  assert.equal(summary.verified, 1);
  assert.equal(model.nodes.find((item) => item.id === "f1").trust, "fact");
});

test("agent-specific fallbacks and explicit backend projections are supported", () => {
  const rethlas = buildProofTree({ id: "r", agent: "rethlas", problem: { statement: "Q" }, blueprint: [], verification: { verdict: "pending" } });
  assert.ok(rethlas.nodes.some((item) => item.kind === "verification"));
  const rethlasDependencies = buildDependencyGraph({ id: "r", agent: "rethlas", problem: { statement: "Q" }, blueprint: ["步骤一"], verification: { verdict: "pending" } });
  assert.ok(rethlasDependencies.edges.some((item) => item.relation === "verified_by"));
  const danus = buildProofTree({ id: "d", agent: "danus", problem: { statement: "R" }, facts: [{ fact_id: "f1", statement: "已验证引理", status: "accepted" }] });
  assert.equal(danus.nodes.find((item) => item.id === "f1").trust, "fact");
  const explicit = buildProofTree({ proofTree: { rootId: "T", nodes: [{ id: "T", kind: "theorem", statement: "T" }, { id: "L", kind: "lemma", statement: "L" }], edges: [{ from: "T", to: "L", relation: "requires" }] } });
  assert.equal(explicit.rootId, "T");
  assert.equal(explicit.edges.length, 1);
});
