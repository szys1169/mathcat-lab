import test from "node:test";
import assert from "node:assert/strict";
import { deriveResearchActivity, researchEventType } from "../public/research-activity.js";

const at = (seconds) => new Date(Date.UTC(2026, 8, 4, 0, 0, seconds)).toISOString();
const baseBoard = (overrides = {}) => ({
  id: "board_activity",
  agent: "mathcat",
  status: "running",
  summary: { current_round: 2 },
  problem: { statement: "证明或反驳命题 P" },
  routes: [],
  tasks: [],
  workers: [],
  claims: [],
  verificationQueue: [],
  decisions: [],
  events: [],
  ...overrides
});

test("event types remain compatible with mapped, modern, and legacy board events", () => {
  assert.equal(researchEventType({ kind: "mapped.event" }), "mapped.event");
  assert.equal(researchEventType({ type: "modern.event" }), "modern.event");
  assert.equal(researchEventType({ event_type: "legacy.event" }), "legacy.event");
});

test("queued proposals are planner input, not a human approval gate", () => {
  const activity = deriveResearchActivity(baseBoard({
    reviewMode: "automatic",
    routes: [{ id: "proposal", isProposal: true, status: "pending", humanStatus: "proposed" }]
  }));
  assert.equal(activity.pendingRoutes, 0);
  assert.notEqual(activity.phase, "review");
  assert.match(activity.routeById.proposal.label, /规划器/);
  assert.doesNotMatch(activity.label, /等待你/);
});

test("a proposal does not suppress a real pending route approval", () => {
  const activity = deriveResearchActivity(baseBoard({
    status: "needs_human_review",
    routes: [
      { id: "proposal", isProposal: true, status: "pending", humanStatus: "proposed" },
      { id: "route", isProposal: false, status: "active", humanStatus: "pending" }
    ]
  }));
  assert.equal(activity.pendingRoutes, 1);
  assert.equal(activity.phase, "review");
  assert.equal(activity.routeById.route.label, "等待你的批准");
});

test("approved routes are released independently while another route still needs review", () => {
  const activity = deriveResearchActivity(baseBoard({
    status: "needs_human_review",
    routes: [
      { id: "approved_a", status: "active", humanStatus: "approved" },
      { id: "approved_b", status: "active", humanStatus: "approved" },
      { id: "pending_c", status: "active", humanStatus: "pending" }
    ]
  }));
  assert.equal(activity.phase, "review");
  assert.equal(activity.pendingRoutes, 1);
  assert.match(activity.label, /1 条研究路线/);
  assert.equal(activity.routeById.approved_a.phase, "ready");
  assert.match(activity.routeById.approved_a.label, /当前没有待执行任务/);
  assert.doesNotMatch(activity.routeById.approved_a.label, /等待其他路线/);
  assert.match(activity.detail, /逐条放行/);
  assert.equal(activity.routeById.pending_c.phase, "review");
  assert.equal(activity.stages[2].status, "waiting");
  assert.equal(activity.stages[3].status, "upcoming");
});

test("approved route work stays visibly active while another route awaits review", () => {
  const activity = deriveResearchActivity(baseBoard({
    status: "needs_human_review",
    routes: [
      { id: "approved", status: "active", humanStatus: "approved" },
      { id: "pending", status: "active", humanStatus: "pending" }
    ],
    tasks: [{ task_id: "task_live", route_id: "approved", worker_id: "worker_live", worker_role: "prover", objective: "证明关键引理", status: "running" }],
    workers: [{ worker_id: "worker_live", current_task_id: "task_live", current_route_id: "approved", role: "prover", status: "running" }]
  }));
  assert.equal(activity.phase, "executing");
  assert.equal(activity.activeTasks, 1);
  assert.equal(activity.pendingRoutes, 1);
  assert.match(activity.label, /1 条路线待审核/);
  assert.match(activity.detail, /不会阻塞/);
  assert.equal(activity.routeById.approved.phase, "executing");
  assert.equal(activity.routeById.pending.phase, "review");
  assert.equal(activity.stages[2].status, "waiting");
  assert.equal(activity.stages[4].status, "active");
});

test("a review snapshot without pending items reports a possible state mismatch", () => {
  const activity = deriveResearchActivity(baseBoard({
    status: "needs_human_review",
    routes: [{ id: "approved", status: "active", humanStatus: "approved" }]
  }));
  assert.equal(activity.phase, "blocked");
  assert.equal(activity.tone, "warning");
  assert.match(activity.label, /检查研究状态/);
  assert.doesNotMatch(activity.label, /回答.*问题/);
  assert.match(activity.detail, /研究日志/);
  assert.equal(activity.pendingRoutes, 0);
  assert.equal(activity.pendingQuestions, 0);
});

test("a live planning attempt maps to a readable current stage", () => {
  const activity = deriveResearchActivity(baseBoard({
    events: [
      { type: "round.started", cursor: 10, occurred_at: at(0), entity: { kind: "round", id: "round_2" }, data: { number: 2 } },
      { event_type: "planning.stage.attempt.started", cursor: 11, occurred_at: at(2), entity: { kind: "planning_attempt", id: "attempt_1" }, data: { stage: "route_generator", hard_timeout_seconds: 300 } }
    ]
  }), { now: Date.parse(at(32)) });
  assert.equal(activity.phase, "planning");
  assert.equal(activity.planning.stageId, "route_generator");
  assert.equal(activity.planning.elapsedSeconds, 30);
  assert.match(activity.label, /研究路线/);
  assert.equal(activity.stages[1].status, "active");
});

test("soft planning budgets are warnings without falsely declaring a timeout", () => {
  const activity = deriveResearchActivity(baseBoard({
    events: [
      { kind: "round.started", cursor: 1, at: at(0), entity: { kind: "round", id: "round_2" }, data: { number: 2 } },
      { kind: "planning.stage.attempt.started", cursor: 2, at: at(1), data: { stage: "reflection", hard_timeout_seconds: 180 } },
      { kind: "planning.stage.soft_budget_exceeded", cursor: 3, at: at(20), data: { stage: "reflection", elapsed_ms: 19000 } }
    ]
  }), { now: Date.parse(at(40)) });
  assert.equal(activity.planning.active, true);
  assert.equal(activity.planning.stale, false);
  assert.match(activity.detail, /比预期更久/);
});

test("a planning attempt is marked stale only beyond its hard timeout grace window", () => {
  const activity = deriveResearchActivity(baseBoard({
    events: [
      { kind: "round.started", cursor: 1, at: at(0), entity: { kind: "round", id: "round_2" }, data: { number: 2 } },
      { kind: "planning.stage.attempt.started", cursor: 2, at: at(1), data: { stage: "ranking", hard_timeout_seconds: 10 } }
    ]
  }), { now: Date.parse(at(75)) });
  assert.equal(activity.phase, "planning");
  assert.equal(activity.active, false);
  assert.equal(activity.tone, "warning");
  assert.equal(activity.planning.stale, true);
  assert.match(activity.label, /长时间未更新/);
});

test("running tasks and workers identify the active route and current work", () => {
  const activity = deriveResearchActivity(baseBoard({
    routes: [{ id: "route_direct", status: "active", humanStatus: "approved" }],
    tasks: [{ task_id: "task_lemma", route_id: "route_direct", worker_id: "worker_1", worker_role: "prover", objective: "证明局部化引理", status: "running" }],
    workers: [{ worker_id: "worker_1", current_task_id: "task_lemma", role: "prover", status: "running" }]
  }));
  assert.equal(activity.phase, "executing");
  assert.equal(activity.activeTasks, 1);
  assert.equal(activity.activeWorkers, 1);
  assert.match(activity.detail, /证明局部化引理/);
  assert.equal(activity.routeById.route_direct.phase, "executing");
  assert.equal(activity.routeById.route_direct.tasks[0].objective, "证明局部化引理");
  assert.equal(activity.stages[4].status, "active");
});

test("submitted task results are shown as result ingestion rather than ongoing proof search", () => {
  const activity = deriveResearchActivity(baseBoard({
    routes: [{ id: "route_counter", status: "active", humanStatus: "approved" }],
    tasks: [{ task_id: "task_counter", route_id: "route_counter", worker_id: "worker_2", objective: "整理候选反例", status: "result_submitted" }],
    workers: [{ worker_id: "worker_2", current_task_id: "task_counter", status: "draining" }]
  }));
  assert.equal(activity.phase, "ingesting");
  assert.match(activity.label, /接收并整理结果/);
  assert.equal(activity.routeById.route_counter.phase, "ingesting");
});

test("verification case transitions refine the visible verification stage", () => {
  const activity = deriveResearchActivity(baseBoard({
    routes: [{ id: "route_direct", status: "active", humanStatus: "approved" }],
    claims: [{ id: "candidate_1", kind: "candidate", routeId: "route_direct", statement: "候选引理" }],
    verificationQueue: [{ verification_id: "verification_1", candidate_id: "candidate_1", status: "verifying" }],
    events: [
      { kind: "verification.case.created", cursor: 20, at: at(20), entity: { kind: "verification_case", id: "case_1" }, data: { verification_id: "verification_1", candidate_id: "candidate_1" } },
      { kind: "verification.case.transitioned", cursor: 21, at: at(21), entity: { kind: "verification_case", id: "case_1" }, data: { case_id: "case_1", from: "review", to: "formalization" } }
    ]
  }));
  assert.equal(activity.phase, "verifying");
  assert.equal(activity.verifications[0].stage, "formalization");
  assert.match(activity.verifications[0].label, /形式化陈述/);
  assert.equal(activity.routeById.route_direct.phase, "verifying");
  assert.equal(activity.stages[5].status, "active");
});

test("terminal project snapshots override older running events and tasks", () => {
  const activity = deriveResearchActivity(baseBoard({
    status: "success",
    tasks: [{ task_id: "old_task", status: "running", objective: "旧快照任务" }],
    events: [{ kind: "task.started", cursor: 1, at: at(1), entity: { kind: "task", id: "old_task" }, data: { task_id: "old_task" } }]
  }));
  assert.equal(activity.phase, "terminal");
  assert.equal(activity.label, "研究目标已完成");
  assert.equal(activity.active, false);
  assert.equal(activity.stages[5].status, "complete");
});
