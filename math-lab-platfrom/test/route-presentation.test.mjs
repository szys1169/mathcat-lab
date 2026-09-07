import test from "node:test";
import assert from "node:assert/strict";
import { groupResearchRoutes, routeApproachKind, routePresentation, routeRole } from "../public/route-presentation.js";

test("legacy proof routes are not misclassified by caveats that mention counterexamples", () => {
  const route = {
    title: "任意维乘法核—Artinian 对偶中央桥梁",
    summary: "即使完全成功，仍要检查原目标。禁止把有限测试或没有反例当成证明。"
  };
  assert.equal(routeApproachKind(route), "direct_proof");
  assert.equal(routeRole(route), "primary");
  const presentation = routePresentation(route);
  assert.equal(presentation.title, "直接证明：用 Artinian 对偶研究乘法核");
  assert.match(presentation.what, /有限维 Artinian 商代数/);
  assert.equal(presentation.steps.length, 3);
  assert.doesNotMatch(presentation.title, /^即使完全成功/);
});

test("a Rees valuation route remains a direct auxiliary route", () => {
  const route = {
    title: "Rees valuation—m-adic 退化的加强版辅助路线",
    summary: "若没有找到反例，也不能把有限计算当成原命题的证明。"
  };
  assert.equal(routeApproachKind(route), "direct_proof");
  assert.equal(routeRole(route), "auxiliary");
  const presentation = routePresentation(route);
  assert.equal(presentation.title, "加强版：用 Rees 赋值检验积分闭包");
  assert.match(presentation.what, /Rees 赋值/);
});

test("mapped legacy routes fall back to their preserved technical title", () => {
  const presentation = routePresentation({
    title: "",
    technicalTitle: "任意维乘法核—Artinian 对偶中央桥梁",
    plainLanguageSummary: "",
    whyThisRoute: "",
    expectedOutput: "",
    relationToGoal: "",
    summary: "即使完全成功，仍需检查它是否推出原目标。"
  });
  assert.equal(presentation.kind, "direct_proof");
  assert.equal(presentation.title, "直接证明：用 Artinian 对偶研究乘法核");
  assert.match(presentation.what, /有限维 Artinian 商代数/);
  assert.match(presentation.relation, /证明链真正到达原命题/);
});

test("structured researcher-facing fields always take precedence over legacy inference", () => {
  const presentation = routePresentation({
    title: "内部标题提到反例和审计",
    approachKind: "computation",
    routeRole: "primary",
    userTitle: "计算探索：枚举低维对象",
    plainLanguageSummary: "用可重放程序枚举低维对象。",
    whyThisRoute: "先观察可能的结构模式。",
    expectedOutput: "脚本、输入范围和结果表。",
    relationToGoal: "只提供候选，不替代一般证明。",
    steps: ["确定范围", "运行计算"]
  });
  assert.equal(presentation.kind, "computation");
  assert.equal(presentation.title, "计算探索：枚举低维对象");
  assert.deepEqual(presentation.steps, ["确定范围", "运行计算"]);
});

test("open conjectures expose missing proof, counterexample, computation, and reduction directions", () => {
  const groups = groupResearchRoutes([], { problem: { statement: "Open problem: prove or disprove conjecture C" } }, { includeExpected: true });
  assert.deepEqual(groups.map((item) => item.kind), ["direct_proof", "counterexample", "computation", "reduction"]);
  assert.ok(groups.every((item) => item.planned === false));
});
