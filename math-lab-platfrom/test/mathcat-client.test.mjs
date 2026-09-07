import test from "node:test";
import assert from "node:assert/strict";
import { MathCatClient, mapMathCatBoard } from "../src/mathcat-client.mjs";

test("MathCat board contract maps to the platform whiteboard and graph models", () => {
  const board = mapMathCatBoard({
    project_id: "project_1", mode: "human_collaboration", status: "created", revision: 7,
    problem: { version: 2, original_problem: "证明 P", target_statement: "证明 P 在有限情形成立", assumptions: ["A"], success_criteria: "verified" },
    routes: [{ route_id: "route_1", title: "归纳", method_summary: "按规模归纳", approach_kind: "reduction", route_role: "primary", user_title: "归约与推广：按规模归纳", plain_language_summary: "从小规模推到大规模", why_this_route: "对象有自然规模", expected_output: "归纳引理", relation_to_goal: "闭合归纳后得到主结论", steps: ["验证基例", "证明归纳步"], target_goal_ids: ["goal_1"], status: "active", human_review: "approved" }],
    route_proposals: [{ proposal_id: "proposal_1", title: "极小反例", method_summary: "选择极小反例", status: "queued", route_id: null }],
    goals: [{ goal_id: "goal_1", statement: "归纳步", status: "open" }],
    claims: [{ claim_id: "fact_1", kind: "fact", statement: "基例成立", status: "accepted", fact_id: "fact_1" }],
    failed_routes: [],
    human_questions: [{ question_id: "q1", question: "采用更强归纳假设吗？", options: [{ value: "yes", label: "采用", reason: "闭合归纳步" }], blocking_entity_ids: ["goal_1"], status: "open", answer: null, asked_by: "mathcat", created_at: "2026-09-03T00:00:00Z" }],
    event_cursor: 18,
    timeline: [
      {
        event_id: "e1",
        type: "planning.stage.attempt.started",
        cursor: 17,
        project_revision: 7,
        entity: { kind: "planning_attempt", id: "attempt_1", ignored: "not exposed" },
        caused_by: { kind: "round", id: "round_2" },
        data: { stage: "route_generator", hard_timeout_seconds: 600, secret_prompt: "must not reach the browser" },
        occurred_at: "2026-09-03T00:00:00Z"
      },
      {
        event_id: "e2",
        event_type: "route_created",
        cursor: 18,
        entity: { kind: "route", id: "route_1" },
        data: { title: "归纳" },
        occurred_at: "2026-09-03T00:00:01Z"
      }
    ],
    graph: { nodes: [{ id: "goal_1", kind: "goal", label: "归纳步", status: "open", attributes: {} }, { id: "fact_1", kind: "fact", label: "基例成立", status: "active", attributes: {} }], edges: [{ id: "edge_1", source: "fact_1", target: "goal_1", kind: "supports" }] },
    summary: {}, uncertainties: [], tasks: [], workers: [], verification_queue: [], artifacts: [], experiments: [{ language: "Python", exit_code: 0 }], capabilities: {}
  }, { id: "rb_1", conversationId: "c1" });
  assert.equal(board.remoteProjectId, "project_1");
  assert.equal(board.problem.original, "证明 P");
  assert.equal(board.routes[0].id, "route_1");
  assert.equal(board.routes[0].plainLanguageSummary, "从小规模推到大规模");
  assert.equal(board.routes[0].expectedOutput, "归纳引理");
  assert.equal(board.routes[0].approachKind, "reduction");
  assert.equal(board.routes[0].routeRole, "primary");
  assert.equal(board.routes[0].title, "归约与推广：按规模归纳");
  assert.deepEqual(board.routes[0].steps, ["验证基例", "证明归纳步"]);
  assert.deepEqual(board.routes[0].targetGoalIds, ["goal_1"]);
  assert.equal(board.routes[1].status, "pending");
  assert.equal(board.decisions[0].status, "pending");
  assert.equal(board.decisions[0].options[0].description, "闭合归纳步");
  assert.equal(board.experiments[0].language, "Python");
  assert.deepEqual(board.dependencyGraph.edges[0], { id: "edge_1", from: "fact_1", to: "goal_1", relation: "supports" });
  assert.equal(board.eventCursor, 18);
  assert.deepEqual(board.events[0], {
    id: "e1",
    cursor: 17,
    revision: 7,
    at: "2026-09-03T00:00:00Z",
    kind: "planning.stage.attempt.started",
    type: "planning.stage.attempt.started",
    entity: { kind: "planning_attempt", id: "attempt_1" },
    causedBy: { kind: "round", id: "round_2" },
    data: { stage: "route_generator", hard_timeout_seconds: 600 },
    text: "planning.stage.attempt.started · planning_attempt"
  });
  assert.equal(board.events[1].kind, "route_created");
  assert.equal("secret_prompt" in board.events[0].data, false);
});

test("legacy MathCat boards recover a concise primary target from a full problem document", () => {
  const fullProblem = `# Background

Definitions and assumptions.

**Open problem (Conjecture 3.6):**
prove or disprove that $J_f:f \\not\\subset I_f$.

**Stronger variant:** prove a statement about $\\overline{J_f}$.`;
  const board = mapMathCatBoard({
    project_id: "legacy_project",
    problem: { original_problem: fullProblem, target_statement: fullProblem },
    graph: { nodes: [{ id: "main", kind: "goal", label: fullProblem, attributes: {} }], edges: [] }
  }, { id: "legacy_board", conversationId: "legacy_conversation" });
  assert.equal(board.problem.statement, "prove or disprove that $J_f:f \\not\\subset I_f$.");
  assert.equal(board.problem.original, fullProblem);
  assert.equal(board.dependencyGraph.nodes[0].label, board.problem.statement);
});

test("MathCat project lifecycle commands use the board revision", async () => {
  const client = new MathCatClient({ baseUrl: "http://127.0.0.1:8787", actorId: "test", token: "token" });
  const calls = [];
  client.request = async (pathname, options) => { calls.push({ pathname, options }); return { command_id: "command_1" }; };
  const board = { revision: 12 };
  await client.startProject("project A", board);
  await client.resumeProject("project A", board);
  await client.stopProject("project A", board);
  assert.deepEqual(calls.map((item) => item.pathname), [
    "/api/v1/projects/project%20A/commands/start",
    "/api/v1/projects/project%20A/commands/resume",
    "/api/v1/projects/project%20A/commands/stop"
  ]);
  assert.ok(calls.every((item) => item.options.write && item.options.body.expected_revision === 12));
});

test("MathCat project creation maps three review levels to backend route approval", async () => {
  const client = new MathCatClient({ baseUrl: "http://127.0.0.1:8787", actorId: "test", token: "token" });
  const calls = [];
  client.request = async (_pathname, options) => { calls.push(options.body); return { project_id: `p${calls.length}` }; };
  await client.createProject({ name: "自动", problem: "P", reviewMode: "automatic" });
  await client.createProject({ name: "中等", problem: "P", reviewMode: "balanced" });
  await client.createProject({ name: "严格", problem: "完整上下文", targetStatement: "主目标 P", reviewMode: "strict" });
  assert.deepEqual(calls.map((item) => item.human_route_approval), [false, false, true]);
  assert.deepEqual(calls.map((item) => item.review_mode), ["automatic", "balanced", "strict"]);
  assert.equal(calls[2].problem, "完整上下文");
  assert.equal(calls[2].target_statement, "主目标 P");
});

test("MathCat board projects the complete current project budget", () => {
  const board = mapMathCatBoard({
    project_id: "budgeted",
    revision: 2,
    problem: { original_problem: "P", target_statement: "P" },
    project: { budget: { max_rounds: 20, max_parallel_workers: 6, max_minutes_per_task: 90, max_model_calls_per_task: 8, max_total_model_calls: 240 } }
  }, { id: "rb-budget", conversationId: "c-budget", reviewMode: "balanced" });
  assert.deepEqual(board.budget, {
    maxRounds: 20,
    maxParallelWorkers: 6,
    maxMinutesPerTask: 90,
    maxModelCallsPerTask: 8,
    maxTotalModelCalls: 240
  });
  assert.equal(board.reviewMode, "balanced");
});

test("MathCat board treats backend research settings as authoritative", () => {
  const board = mapMathCatBoard({
    project_id: "settings-authority",
    revision: 3,
    problem: { original_problem: "P", target_statement: "P" },
    settings: {
      review_mode: "automatic",
      running_task_policy: "new_tasks_only",
      budget: { max_rounds: 18, max_parallel_workers: 8, max_minutes_per_task: 150, max_model_calls_per_task: 7, max_total_model_calls: 300 }
    },
    planning_suggestions: [{ suggestion_id: "suggestion_1", command_id: "command_1", content: "多用局部化", target_route_id: "route_1", status: "applied", created_in_round: 2, effective_round: 3 }],
    project: { budget: { max_rounds: 1, max_parallel_workers: 1, max_minutes_per_task: 1, max_model_calls_per_task: 1, max_total_model_calls: 1 } }
  }, { id: "rb-settings", conversationId: "c-settings", reviewMode: "strict" });
  assert.equal(board.reviewMode, "automatic");
  assert.equal(board.budget.maxParallelWorkers, 8);
  assert.equal(board.budget.maxMinutesPerTask, 150);
  assert.equal(board.runningTaskPolicy, "new_tasks_only");
  assert.deepEqual(board.planningSuggestions[0], {
    suggestion_id: "suggestion_1", command_id: "command_1", content: "多用局部化", target_route_id: "route_1", status: "applied", created_in_round: 2, effective_round: 3,
    id: "suggestion_1", commandId: "command_1", targetRouteId: "route_1", decision: "", decisionDisposition: "", createdRound: 2, effectiveRound: 3, submittedAt: null
  });
});

test("MathCat planning decisions expose readable rationale instead of raw JSON", () => {
  const board = mapMathCatBoard({
    project_id: "suggestion-decision",
    problem: { original_problem: "P", target_statement: "P" },
    planning_suggestions: [{
      suggestion_id: "suggestion_2",
      content: "多从交换代数角度规划",
      status: "applied",
      decision: JSON.stringify({ disposition: "applied", rationale: "局部化可以直接利用当前假设。", internal_score: 0.9 })
    }]
  }, { id: "rb-decision", conversationId: "c-decision" });
  assert.equal(board.planningSuggestions[0].decision, "局部化可以直接利用当前假设。");
  assert.equal(board.planningSuggestions[0].decisionDisposition, "applied");
  assert.doesNotMatch(board.planningSuggestions[0].decision, /internal_score/);
});

test("MathCat board snapshot reads project settings alongside the live board", async () => {
  const client = new MathCatClient({ baseUrl: "http://127.0.0.1:8787", actorId: "test", token: "token" });
  const paths=[];
  client.request=async(pathname)=>{paths.push(pathname);if(pathname.endsWith("/board?include=tasks,workers,verification,artifacts,graph&timeline_limit=100"))return {project_id:"p1",artifacts:[]};if(pathname.endsWith("/route-proposals"))return [];return {project_id:"p1",budget:{max_rounds:12,max_parallel_workers:5,max_minutes_per_task:60,max_model_calls_per_task:4,max_total_model_calls:120}};};
  const remote=await client.board("p1");
  assert.ok(paths.includes("/api/v1/projects/p1"));
  assert.equal(remote.budget.max_parallel_workers,5);
});

test("MathCat command status uses the dedicated read endpoint", async () => {
  const client = new MathCatClient({ baseUrl: "http://127.0.0.1:8787", actorId: "test", token: "token" });
  let request;
  client.request = async (pathname, options) => { request = { pathname, options }; return { command_id: "cmd 1", status: "applied" }; };
  const command = await client.commandStatus("project A", "cmd 1");
  assert.equal(request.pathname, "/api/v1/projects/project%20A/commands/cmd%201");
  assert.equal(request.options, undefined);
  assert.equal(command.status, "applied");
});

test("human collaboration commands carry the current revision and full budget", async () => {
  const client = new MathCatClient({ baseUrl: "http://127.0.0.1:8787", actorId: "test", token: "token" });
  const calls = [];
  client.request = async (pathname, options) => {
    calls.push({ pathname, options });
    if (pathname.endsWith("/routes")) return { execution_started: true, route_id: "route_human", task_id: "task_human" };
    return { command_id: `command_${calls.length}` };
  };
  const board = { revision: 17, budget: { maxRounds: 12, maxParallelWorkers: 3, maxMinutesPerTask: 45, maxModelCallsPerTask: 4, maxTotalModelCalls: 120 } };
  await client.createAndExecuteRoute("project A", board, { title: "局部化", objective: "证明局部引理", summary: "逐素理想局部化", completionContract: "完整证明或最小缺口", knownRisks: ["拼接可能失败"] });
  await client.addPlanningSuggestion("project A", board, { content: "更多采用交换代数", targetRouteId: "route_1", reason: "利用局部结构" });
  await client.goalReview("project A", board, { focus: "重新区分主目标与窄化命题" });
  await client.adjustBudget("project A", board, { maxParallelWorkers: 7, maxMinutesPerTask: 120 });
  await client.updateReviewPolicy("project A", board, "balanced");
  await client.updateResearchSettings("project A", { ...board, reviewMode: "strict" }, { maxParallelWorkers: 6, maxMinutesPerTask: 100, reviewMode: "automatic" });
  assert.deepEqual(calls.map((item) => item.pathname), [
    "/api/v1/projects/project%20A/routes",
    "/api/v1/projects/project%20A/suggestions",
    "/api/v1/projects/project%20A/commands/goal-review",
    "/api/v1/projects/project%20A/commands/adjust-budget",
    "/api/v1/projects/project%20A/commands/review-policy",
    "/api/v1/projects/project%20A/commands/settings"
  ]);
  assert.ok(calls.every((item) => item.options.write && item.options.body.expected_revision === 17));
  assert.equal(calls[0].options.body.completion_contract, "完整证明或最小缺口");
  assert.equal(calls[1].options.body.target_route_id, "route_1");
  assert.equal(calls[2].options.body.payload.focus, "重新区分主目标与窄化命题");
  assert.deepEqual(calls[3].options.body.payload.limits, { max_rounds: 12, max_parallel_workers: 7, max_minutes_per_task: 120, max_model_calls_per_task: 4, max_total_model_calls: 120 });
  assert.deepEqual(calls[4].options.body.payload, { review_mode: "balanced", human_route_approval: false });
  assert.deepEqual(calls[5].options.body.payload, {
    limits: { max_rounds: 12, max_parallel_workers: 6, max_minutes_per_task: 100, max_model_calls_per_task: 4, max_total_model_calls: 120 },
    review_mode: "automatic"
  });
});

test("immediate route creation never reports success without backend execution confirmation", async () => {
  const client = new MathCatClient({ baseUrl: "http://127.0.0.1:8787", actorId: "test", token: "token" });
  client.request = async () => ({ proposal_id: "legacy_proposal" });
  await assert.rejects(
    client.createAndExecuteRoute("legacy", { revision: 3 }, { title: "路线", objective: "目标", summary: "步骤", completionContract: "完成标准" }),
    /未确认路线已经启动/
  );
});

test("unsupported collaboration endpoints produce an actionable update message", async () => {
  const client = new MathCatClient({ baseUrl: "http://127.0.0.1:8787", actorId: "test", token: "token" });
  client.request = async () => { const error = new Error("not found"); error.status = 404; throw error; };
  await assert.rejects(client.goalReview("legacy", { revision: 3 }, { focus: "重做目标" }), /请先更新本地 MathCat/);
});

test("a write retry reuses the same idempotency key after a lost response", async () => {
  const keys = [];
  let attempts = 0;
  const client = new MathCatClient({
    baseUrl: "http://127.0.0.1:8787",
    actorId: "test",
    token: "token",
    fetchImpl: async (_url, options) => {
      attempts += 1;
      keys.push(options.headers["idempotency-key"]);
      if (attempts === 1) throw new TypeError("connection closed after commit");
      return new Response(JSON.stringify({ data: { command_id: "replayed_command", after_revision: 4 } }), { status: 200, headers: { "content-type": "application/json" } });
    }
  });
  const result = await client.goalReview("project", { revision: 3 }, { focus: "重新讨论目标" });
  assert.equal(result.command_id, "replayed_command");
  assert.equal(attempts, 2);
  assert.ok(keys[0]);
  assert.equal(keys[0], keys[1]);
});

test("a truncated successful write response replays with the same idempotency key", async () => {
  const keys = [];
  let attempts = 0;
  const client = new MathCatClient({
    baseUrl: "http://127.0.0.1:8787",
    actorId: "test",
    token: "token",
    fetchImpl: async (_url, options) => {
      attempts += 1;
      keys.push(options.headers["idempotency-key"]);
      if (attempts === 1) return { status: 202, ok: true, async json() { throw new TypeError("truncated body"); } };
      return new Response(JSON.stringify({ data: { execution_started: true, route_id: "route_once", task_id: "task_once" } }), { status: 202, headers: { "content-type": "application/json" } });
    }
  });
  const result = await client.createAndExecuteRoute("project", { revision: 2 }, { title: "路线", objective: "目标", summary: "方法", completionContract: "完成" });
  assert.equal(result.route_id, "route_once");
  assert.equal(attempts, 2);
  assert.ok(keys[0]);
  assert.equal(keys[0], keys[1]);
});
