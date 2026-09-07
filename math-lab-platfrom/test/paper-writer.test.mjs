import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createPaperWriter, verifyPaperSources } from "../src/paper-writer.mjs";

const resultsRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../tests/results/paper-writer-2.5");
async function fixture(name) {
  await fs.mkdir(resultsRoot, { recursive: true });
  const workspace = await fs.mkdtemp(path.join(resultsRoot, `${name}-`));
  const source = path.join(workspace, "source.json");
  await fs.writeFile(source, JSON.stringify({ schema_version: "1.0", writing_goal: { deliverable: "paper", language: "zh" }, core_results: [{ id: "R1", statement: "For every real x, x^2 >= 0.", status: "proved", sources: ["proof.md"] }], source_materials: ["proof.md"], citation_whitelist: [], evidence_gaps: [] }));
  await fs.writeFile(path.join(workspace, "proof.md"), "Supplied elementary fixture: a real square is nonnegative by the order axioms.");
  return { workspace, sourcePacket: source, selectedFiles: ["proof.md"] };
}
async function writeFixture(writerDir, language) {
  const zh = language === "zh";
  const tex = `${zh ? "\\documentclass{ctexart}" : "\\documentclass{article}"}\n\\usepackage{amsmath,amsthm}\n\\newtheorem{theorem}{${zh ? "定理" : "Theorem"}}\n\\begin{document}\n\\section{${zh ? "测试材料" : "Fixture"}}\\label{sec:main}\n${zh ? "这份文件仅用于验证写作交付流程。" : "This document is a local delivery fixture."}\n\\begin{theorem}\\label{thm:square}\n${zh ? "每个实数" : "Every real number"} $x$ ${zh ? "满足" : "satisfies"} $x^2\\geq0$.\n\\end{theorem}\n\\begin{proof}${zh ? "结论由实数的序性质成立。" : "The claim follows from the order axioms."}\\end{proof}\n\\end{document}\n`;
  await fs.writeFile(path.join(writerDir, "article_candidate.tex"), tex);
  for (const name of ["article_plan.md", "claim_evidence_ledger.md", "revision_notes.md"]) await fs.writeFile(path.join(writerDir, name), "Local fixture; no real model or mathematical review was run.\n");
  if (!zh) await fs.writeFile(path.join(writerDir, "sync_checklist.md"), "Fixture hypotheses, quantified variables, formula and labels are unchanged.\n");
}

test("prepare retries restore one version, conflicting inputs are rejected", async () => {
  const input = await fixture("prepare");
  const driver = createPaperWriter();
  const prepared = await driver.prepare({ ...input, taskId: "same", language: "zh" });
  const restored = await createPaperWriter().prepare({ ...input, taskId: "same", language: "zh" });
  assert.equal(restored.runDir, prepared.runDir);
  assert.equal((await fs.readdir(path.join(input.workspace, "论文", "versions"))).length, 1);
  await assert.rejects(verifyPaperSources(prepared, { status: "completed", compile_status: "passed", render_status: "passed", version: "different" }), /不属于同一论文版本/);
  await assert.rejects(driver.prepare({ ...input, taskId: "same", language: "en" }), /其他论文输入/);
  const controller = new AbortController(); controller.abort();
  await assert.rejects(driver.prepare({ ...input, taskId: "abort", signal: controller.signal }), { name: "AbortError" });
});

test("failed writing resumes the same prepared version without claiming completion", async () => {
  const input = await fixture("write-retry");
  let attempts = 0;
  const driver = createPaperWriter({ runModel: async ({ workspace, prompt, taskDir }) => {
    assert.equal(workspace, taskDir);
    assert.match(prompt, /不要调用 prepare、finalize/);
    if (++attempts === 1) throw new Error("fixture interrupted model");
    await writeFixture(path.join(workspace, "writer"), "zh");
    return { ok: true };
  } });
  const prepared = await driver.prepare({ ...input, taskId: "retry", language: "zh" });
  await assert.rejects(driver.write({ prepared }), /fixture interrupted/);
  const result = await driver.write({ prepared });
  assert.equal(result.stage, "written");
  assert.equal(attempts, 2);
  assert.equal((await fs.readdir(path.join(input.workspace, "论文", "versions"))).length, 1);
  await assert.rejects(fs.access(path.join(prepared.runDir, "result.json")));
});

test("bilingual real local compile/render; finalize retry never reruns model and primary changes block English", { timeout: 180000 }, async () => {
  const input = await fixture("bilingual-real-compile");
  let modelCalls = 0;
  const driver = createPaperWriter({ runModel: async ({ workspace, prompt }) => {
    modelCalls += 1;
    const language = prompt.includes("用英文同步固定中文主稿") ? "en" : "zh";
    await writeFixture(path.join(workspace, "writer"), language);
    return { ok: true };
  } });
  const zh = await driver.prepare({ ...input, taskId: "pair-zh", language: "zh" });
  await driver.write({ prepared: zh });
  const zhResult = await driver.finalize({ prepared: zh, publishCurrent: false });
  assert.equal(zhResult.status, "completed", JSON.stringify(zhResult.errors));
  assert.equal(zhResult.compile_status, "passed");
  assert.equal(zhResult.render_status, "passed");
  const en = await driver.prepare({ ...input, taskId: "pair-en", language: "en" });
  await driver.write({ prepared: en, primary: zh });
  const first = await driver.finalize({ prepared: en });
  assert.equal(first.status, "completed", JSON.stringify(first.errors));
  const retried = await driver.finalize({ prepared: en });
  assert.equal(retried.status, "completed", JSON.stringify(retried.errors));
  assert.equal(modelCalls, 2);
  assert.equal(retried.version, first.version);
  const checkedSources = await verifyPaperSources(en, retried);
  assert.ok(checkedSources.length >= 5);
  assert.ok(checkedSources.every((file) => Buffer.isBuffer(file.bytes)));
  assert.ok(!checkedSources.some((file) => /primary_snapshot|article_candidate.pdf/.test(file.path)));
  // Preserve the successful pair separately before deliberate corruption tests.
  for (const [language, prepared, result] of [["zh", zh, zhResult], ["en", en, retried]]) {
    const receiptDir = path.join(input.workspace, "passed-pair", language);
    await fs.mkdir(path.join(receiptDir, "writer"), { recursive: true });
    await fs.mkdir(path.join(receiptDir, "logs"), { recursive: true });
    await fs.writeFile(path.join(receiptDir, "result.json"), JSON.stringify(result, null, 2));
    for (const file of ["writer/article_candidate.tex", "writer/article_candidate.pdf", "logs/compile_report.json", "logs/render_report.json"]) await fs.copyFile(path.join(prepared.runDir, file), path.join(receiptDir, file));
    await fs.copyFile(path.join(prepared.runDir, "logs/render-check/page-1.png"), path.join(receiptDir, "page-1.png"));
  }
  await fs.writeFile(path.join(input.workspace, "passed-pair", "README.md"), "Successful local compilation and rendering receipts captured before intentional negative tests. No real model was called; both manuscript bodies are test fixtures.\n");
  await assert.rejects(fs.access(path.join(input.workspace, "论文/current/current.json")));
  const primarySnapshot = path.join(en.runDir, "primary_snapshot", "article_candidate.tex");
  const snapshotBytes = await fs.readFile(primarySnapshot);
  await fs.appendFile(primarySnapshot, "\n% changed snapshot\n");
  const tamperedSnapshot = await driver.finalize({ prepared: en });
  assert.equal(tamperedSnapshot.status, "partial");
  assert.ok(tamperedSnapshot.errors.some((error) => /固定中文快照已被修改/.test(error)));
  await fs.writeFile(primarySnapshot, snapshotBytes);
  const pendingEnglish = await driver.prepare({ ...input, taskId: "pending-en", language: "en" });
  const revisionLog = path.join(zh.writerDir, "human_revision_log.md");
  await fs.writeFile(revisionLog, "| 修改 | 状态 |\n| 统一术语 | applied |\n");
  await assert.rejects(driver.write({ prepared: pendingEnglish, primary: zh }), /pending\/applied/);
  await fs.unlink(revisionLog);
  assert.equal(modelCalls, 2);
  const article = path.join(en.writerDir, "article_candidate.tex");
  await fs.writeFile(article, (await fs.readFile(article, "utf8")).replace("x^2\\geq0", "x^2>0"));
  await assert.rejects(verifyPaperSources(en, retried), /完整论文源文件.*变化/);
  const wrongFormula = await driver.finalize({ prepared: en });
  assert.equal(wrongFormula.status, "partial");
  assert.ok(wrongFormula.errors.some((error) => /formulas/.test(error)));
  await fs.appendFile(path.join(zh.writerDir, "article_candidate.tex"), "\n% manual change\n");
  const changedPrimary = await driver.finalize({ prepared: en });
  assert.equal(changedPrimary.status, "partial");
  assert.ok(changedPrimary.errors.some((error) => /中文主稿.*变化/.test(error)));
  assert.equal(modelCalls, 2);
});
