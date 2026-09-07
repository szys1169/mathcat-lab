import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { extractPrimaryResearchTarget, needsWorkspaceProblem, resolveResearchProblem } from "../src/research-context.mjs";

test("ambiguous numbered requests are resolved from the workspace problem file", async () => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "mathcat-context-"));
  await fs.writeFile(path.join(root, "problem.md"), "# 猜想 3.6\n证明或反驳完整命题 S。", "utf8");
  const resolved = await resolveResearchProblem({
    message: "尝试推进猜想3.6",
    workspace: { path: root, projectless: false }
  });
  assert.match(resolved.problem, /完整命题 S/);
  assert.match(resolved.problem, /用户本轮要求\n尝试推进猜想3\.6/);
  assert.equal(resolved.targetStatement, "尝试推进猜想3.6");
  assert.equal(resolved.sources[0].name, "problem.md");
});

test("a self-contained research question is not replaced by an unrelated workspace problem", async () => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "mathcat-context-specific-"));
  await fs.writeFile(path.join(root, "problem.md"), "旧问题", "utf8");
  const message = "证明：对任意整数 n，若 n 为奇数，则 n 的平方也是奇数。";
  assert.equal(needsWorkspaceProblem(message), false);
  const resolved = await resolveResearchProblem({ message, workspace: { path: root, projectless: false } });
  assert.equal(resolved.problem, message);
  assert.equal(resolved.targetStatement, message);
  assert.deepEqual(resolved.sources, []);
});

test("projectless research remains scoped to the user's message", async () => {
  const resolved = await resolveResearchProblem({
    message: "研究猜想 3.6",
    workspace: { path: "ignored", projectless: true }
  });
  assert.equal(resolved.problem, "研究猜想 3.6");
  assert.equal(resolved.targetStatement, "研究猜想 3.6");
  assert.deepEqual(resolved.sources, []);
});

test("the primary open problem is separated from a stronger variant", () => {
  const content = `# Background

**Open problem (Conjecture 3.6, arXiv:2509.16933v1):**
prove or disprove that $A \\not\\subset B$.

**Stronger variant:** prove that $A \\not\\subset \\overline{C}$.`;
  const target = extractPrimaryResearchTarget(content, "研究猜想3.6");
  assert.match(target, /A \\not\\subset B/);
  assert.doesNotMatch(target, /overline/);
  assert.doesNotMatch(target, /2509\.16933/);
});
