import test from "node:test";
import assert from "node:assert/strict";
import { mapMathCatBoard } from "../src/mathcat-client.mjs";

test("MathCat board contract maps to the platform whiteboard and graph models", () => {
  const board = mapMathCatBoard({
    project_id: "project_1", mode: "human_collaboration", status: "created", revision: 7,
    problem: { version: 2, original_problem: "证明 P", target_statement: "证明 P 在有限情形成立", assumptions: ["A"], success_criteria: "verified" },
    routes: [{ route_id: "route_1", title: "归纳", method_summary: "按规模归纳", status: "active", human_review: "approved" }],
    route_proposals: [{ proposal_id: "proposal_1", title: "极小反例", method_summary: "选择极小反例", status: "queued", route_id: null }],
    goals: [{ goal_id: "goal_1", statement: "归纳步", status: "open" }],
    claims: [{ claim_id: "fact_1", kind: "fact", statement: "基例成立", status: "accepted", fact_id: "fact_1" }],
    failed_routes: [],
    human_questions: [{ question_id: "q1", question: "采用更强归纳假设吗？", options: [{ value: "yes", label: "采用", reason: "闭合归纳步" }], blocking_entity_ids: ["goal_1"], status: "open", answer: null, asked_by: "mathcat", created_at: "2026-09-03T00:00:00Z" }],
    timeline: [{ event_id: "e1", event_type: "route_created", entity: { kind: "route" }, data: { title: "归纳" }, occurred_at: "2026-09-03T00:00:00Z" }],
    graph: { nodes: [{ id: "goal_1", kind: "goal", label: "归纳步", status: "open", attributes: {} }, { id: "fact_1", kind: "fact", label: "基例成立", status: "active", attributes: {} }], edges: [{ id: "edge_1", source: "fact_1", target: "goal_1", kind: "supports" }] },
    summary: {}, uncertainties: [], tasks: [], workers: [], verification_queue: [], artifacts: [], experiments: [{ language: "Python", exit_code: 0 }], capabilities: {}
  }, { id: "rb_1", conversationId: "c1" });
  assert.equal(board.remoteProjectId, "project_1");
  assert.equal(board.problem.original, "证明 P");
  assert.equal(board.routes[0].id, "route_1");
  assert.equal(board.routes[1].status, "pending");
  assert.equal(board.decisions[0].status, "pending");
  assert.equal(board.decisions[0].options[0].description, "闭合归纳步");
  assert.equal(board.experiments[0].language, "Python");
  assert.deepEqual(board.dependencyGraph.edges[0], { id: "edge_1", from: "fact_1", to: "goal_1", relation: "supports" });
});
