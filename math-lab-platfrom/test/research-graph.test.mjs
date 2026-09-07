import test from "node:test";
import assert from "node:assert/strict";
import { buildProofTree, buildDependencyGraph, graphSummary, visibleGraph, layoutGraph, conciseGraphLabel, overrideGraphPositions, relatedGraphIds, fitGraphScale } from "../public/research-graph.js";

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

test("MathCat proof map separates the target, research directions, routes, and claims", () => {
  const model = buildProofTree(board);
  assert.equal(model.nodes.find((item) => item.id === model.rootId).logic, "PORTFOLIO");
  const routeLane = model.edges.find((item) => item.to === "route_a")?.from;
  assert.equal(model.nodes.find((item) => item.id === routeLane)?.kind, "strategy");
  assert.ok(model.edges.some((item) => item.from === model.rootId && item.to === routeLane));
  assert.ok(model.edges.some((item) => item.from === "route_a" && item.to === "candidate_a"));
  const layout = layoutGraph(model);
  assert.ok(layout.positions.get(model.rootId).y < layout.positions.get(routeLane).y);
  assert.ok(layout.positions.get(routeLane).y < layout.positions.get("route_a").y);
});

test("proof routes expose the current task and a live activity marker", () => {
  const model = buildProofTree({
    id: "live-task",
    agent: "mathcat",
    status: "running",
    problem: { statement: "证明命题 P" },
    routes: [{ id: "route_live", title: "直接证明", status: "active", humanStatus: "approved" }],
    tasks: [{ task_id: "task_live", route_id: "route_live", worker_id: "worker_live", worker_role: "prover", objective: "证明关键局部引理", completion_contract: "给出完整证明或明确缺口", status: "running" }],
    workers: [{ worker_id: "worker_live", current_task_id: "task_live", role: "prover", status: "running" }],
    claims: [],
    verificationQueue: []
  });
  const route = model.nodes.find((item) => item.id === "route_live");
  const task = model.nodes.find((item) => item.id === "task_live");
  assert.equal(route.meta.activity.phase, "executing");
  assert.equal(route.meta.activity.active, true);
  assert.equal(task.kind, "task");
  assert.equal(task.meta.activity.objective, "证明关键局部引理");
  assert.equal(task.meta.completionContract, "给出完整证明或明确缺口");
  assert.ok(model.edges.some((item) => item.from === "route_live" && item.to === "task_live" && item.relation === "runs"));
});

test("proof routes expose an active independent-verification node", () => {
  const model = buildProofTree({
    id: "live-verification",
    agent: "mathcat",
    status: "running",
    problem: { statement: "证明命题 Q" },
    routes: [{ id: "route_verify", title: "归约证明", status: "active", humanStatus: "approved" }],
    claims: [{ id: "candidate_verify", kind: "candidate", routeId: "route_verify", statement: "候选归约引理", status: "verifying" }],
    verificationQueue: [{ verification_id: "verification_live", candidate_id: "candidate_verify", status: "verifying" }],
    events: [
      { kind: "verification.case.created", cursor: 1, entity: { kind: "verification_case", id: "case_live" }, data: { verification_id: "verification_live", candidate_id: "candidate_verify" } },
      { kind: "verification.case.transitioned", cursor: 2, entity: { kind: "verification_case", id: "case_live" }, data: { case_id: "case_live", from: "review", to: "proof_search" } }
    ]
  });
  const route = model.nodes.find((item) => item.id === "route_verify");
  const verification = model.nodes.find((item) => item.id === "verification_live");
  assert.equal(route.meta.activity.phase, "verifying");
  assert.equal(verification.kind, "verification");
  assert.match(verification.label, /形式证明搜索/);
  assert.ok(model.edges.some((item) => item.from === "route_verify" && item.to === "verification_live" && item.relation === "checks"));
});

test("dependency graph points evidence toward claims and the target", () => {
  const model = buildDependencyGraph(board);
  assert.equal(model.source, "derived");
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

test("explicit MathCat graphs reconcile hypothesis route ids with route edges", () => {
  const model = buildDependencyGraph({
    routes: [{ id: "route_1", title: "直接证明" }],
    claims: [{ id: "hyp_1", kind: "hypothesis", routeId: "route_1" }],
    dependencyGraph: {
      rootId: "goal_1",
      nodes: [
        { id: "goal_1", kind: "goal", label: "目标", status: "open" },
        { id: "hyp_1", kind: "route", label: "路线说明", status: "active" }
      ],
      edges: [{ id: "edge_1", from: "route_1", to: "goal_1", relation: "targets" }]
    }
  });
  const route = model.nodes.find((item) => item.kind === "route");
  assert.equal(route.id, "route_1");
  assert.equal(route.sourceId, "hyp_1");
  assert.equal(route.shortLabel, "直接证明");
  assert.equal(model.source, "backend");
  assert.equal(model.contractWarnings.length, 1);
  assert.deepEqual(model.edges, [{ id: "edge_1", from: "route_1", to: "goal_1", relation: "targets" }]);
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

test("long planner prose is shortened for graph nodes but preserved as the full label", () => {
  const long = "即使完全成功，原目标仍剩下证明或反驳已经形式化的 S，并且还需建立明确推出 S 的命名引理 L；本路线不能被包装为主目标路线。";
  const model = buildDependencyGraph({ id: "long", problem: { statement: "S" }, claims: [{ id: "c", kind: "candidate", statement: long }] });
  const candidate = model.nodes.find((item) => item.id === "c");
  assert.equal(candidate.label, long);
  assert.ok(candidate.shortLabel.length <= 34);
  assert.ok(conciseGraphLabel(long, "candidate").length <= 34);
});

test("route graph labels shorten generic planner jargon without problem-specific rewrites", () => {
  assert.equal(
    conciseGraphLabel("量词规格化—定义依赖闭包—可实例化性反压力审计", "route"),
    "明确量词与对象范围 → 补齐命题所需定义 → 检查能否用于具体对象与边界反例"
  );
  assert.equal(conciseGraphLabel("这是一个没有预设模板的新数学技巧", "route"), "这是一个没有预设模板的新数学技巧");
});

test("planner route hypotheses are not duplicated as proof-tree conclusions", () => {
  const model = buildProofTree({ id: "clean", agent: "mathcat", problem: { statement: "S" }, routes: [{ id: "r", title: "路线", status: "active" }], goals: [], claims: [{ id: "h", kind: "hypothesis", statement: "这是一段内部路线约束", routeId: "r" }] });
  assert.equal(model.nodes.some((item) => item.id === "h"), false);
});

test("manual graph positions override auto layout and expand the canvas safely", () => {
  const model = buildProofTree(board);
  const automatic = layoutGraph(model);
  const manual = overrideGraphPositions(automatic, new Map([["route_a", { x: 1450, y: 620 }]]));
  assert.equal(manual.positions.get("route_a").x, 1450);
  assert.equal(manual.positions.get("route_a").y, 620);
  assert.ok(manual.width > 1700);
  assert.ok(manual.height > 740);
  assert.notEqual(manual.positions.get("route_b"), automatic.positions.get("route_b"));
});

test("relationship focus includes the complete selected branch", () => {
  const model = buildProofTree(board);
  const related = relatedGraphIds(model, "route_a");
  const lane = model.edges.find((item) => item.to === "route_a").from;
  assert.deepEqual([...related].sort(), [model.rootId, lane, "candidate_a", "fact_a", "goal_a", "route_a"].sort());
  assert.equal(related.has("route_b"), false);
});

test("fit scale keeps the whole graph in view and respects zoom bounds", () => {
  assert.equal(fitGraphScale({ width: 1000, height: 500 }, 556, 306), 0.5);
  assert.equal(fitGraphScale({ width: 100, height: 100 }, 1200, 800), 1.25);
  assert.equal(fitGraphScale({ width: 3000, height: 2000 }, 300, 200), 0.35);
});

test("independent top-level goals are siblings and parent edges do not depend on array order", () => {
  const model = buildProofTree({
    id: "multi",
    agent: "mathcat",
    problem: { statement: "研究主命题及其加强版" },
    routes: [],
    goals: [
      { id: "leaf", statement: "叶子引理", parent_goal_ids: ["middle"], status: "open" },
      { id: "main", statement: "主命题", parent_goal_ids: [], status: "open" },
      { id: "strong", statement: "更强但独立的版本", parent_goal_ids: [], status: "open" },
      { id: "middle", statement: "中间引理", parent_goal_ids: ["main"], status: "open" }
    ]
  });
  assert.match(model.rootId, /^problem:/);
  assert.ok(model.edges.some((item) => item.from === model.rootId && item.to === "main"));
  assert.ok(model.edges.some((item) => item.from === model.rootId && item.to === "strong"));
  assert.ok(model.edges.some((item) => item.from === "main" && item.to === "middle"));
  assert.ok(model.edges.some((item) => item.from === "middle" && item.to === "leaf"));
  assert.equal(model.edges.some((item) => item.from === "main" && item.to === "strong"), false);
});

test("pending review overrides active route status and unplanned directions are not unverified facts", () => {
  const model = buildProofTree({
    id: "review",
    agent: "mathcat",
    problem: { statement: "证明或反驳猜想 C" },
    routes: [{ id: "r", title: "直接证明", status: "active", humanStatus: "pending" }],
    goals: [{ id: "g", statement: "猜想 C", parent_goal_ids: [], status: "open" }]
  });
  assert.equal(model.nodes.find((item) => item.id === "r").status, "pending");
  const summary = graphSummary(model);
  assert.ok(summary.unplanned >= 1);
  const unverified = visibleGraph(model, { filter: "unverified" });
  assert.equal(unverified.nodes.some((item) => item.status === "unplanned"), false);
});

test("Conjecture 3.6 legacy routes remain readable while missing research directions stay explicit", () => {
  const model = buildProofTree({
    id: "hns-3-6",
    agent: "mathcat",
    status: "running",
    problem: { statement: "Open problem (Conjecture 3.6): prove or disprove that $J_f:f \\not\\subset I_f$." },
    routes: [
      { id: "artinian", title: "任意维乘法核—Artinian 对偶中央桥梁", status: "active", humanStatus: "approved", summary: "即使完全成功，仍需核验原目标；禁止把没有反例当成证明。" },
      { id: "rees", title: "Rees valuation—m-adic 退化的加强版辅助路线", status: "active", humanStatus: "approved", summary: "本路线只处理加强版；有限反例搜索不能当作证明。" }
    ],
    goals: [{ id: "main", statement: "$J_f:f \\not\\subset I_f$", parent_goal_ids: [], status: "open" }],
    claims: []
  });
  const direct = model.nodes.find((item) => item.id === "main:lane:direct_proof");
  assert.equal(direct.status, "active");
  assert.deepEqual(model.nodes.filter((item) => item.kind === "route").map((item) => item.shortLabel), [
    "直接证明：用 Artinian 对偶研究乘法核",
    "加强版：用 Rees 赋值检验积分闭包"
  ]);
  for (const kind of ["counterexample", "computation", "reduction"]) {
    assert.equal(model.nodes.find((item) => item.id === `main:lane:${kind}`).status, "unplanned");
  }
  const visible = model.nodes.filter((item) => ["strategy", "route"].includes(item.kind)).map((item) => `${item.shortLabel} ${item.detail}`).join(" ");
  assert.doesNotMatch(visible, /central missing bridge|interface debt|L⇒S|可实例化性反压力审计/i);
});
