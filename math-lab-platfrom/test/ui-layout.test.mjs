import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";

const index = await fs.readFile(new URL("../public/index.html", import.meta.url), "utf8");
const app = await fs.readFile(new URL("../public/app.js", import.meta.url), "utf8");
const boardCss = await fs.readFile(new URL("../public/research-board.css", import.meta.url), "utf8");
const startScript = await fs.readFile(new URL("../start-platform.ps1", import.meta.url), "utf8");

test("sidebar exposes a normal settings button and controls live in its dialog", () => {
  const sidebar = index.match(/<aside class="sidebar">[\s\S]*?<\/aside>/)?.[0] || "";
  const settings = index.match(/<dialog id="settingsDialog"[\s\S]*?<\/dialog>/)?.[0] || "";
  const composer = index.match(/<section class="composer">[\s\S]*?<\/section>/)?.[0] || "";
  assert.match(sidebar, /id="settingsButton"/);
  assert.doesNotMatch(sidebar, /id="executor"|id="permission"/);
  assert.match(settings, /id="executor"/);
  assert.match(settings, /id="permission"/);
  assert.doesNotMatch(composer, /id="executor"|id="permission"/);
  assert.match(app, /\$\("settingsButton"\)\.onclick = \(\) => \$\("settingsDialog"\)\.showModal\(\)/);
});

test("workspace ownership is hidden after a conversation is established", () => {
  assert.match(app, /\$\("workspaceName"\)\.hidden = Boolean\(state\.conversationId\)/);
  assert.match(app, /\$\("workspaceName"\)\.hidden = true/);
  assert.match(app, /contextPicker\.hidden = Boolean\(state\.conversationId\)/);
  assert.match(app, /contextPicker\.hidden = true/);
});

test("permission choice is stored for later messages", () => {
  assert.match(app, /localStorage\.getItem\("mathcat\.permission"\)/);
  assert.match(app, /localStorage\.setItem\("mathcat\.permission", state\.permission\)/);
});

test("research view switch has an explicit label and high-contrast active state", () => {
  assert.match(app, /当前视图/);
  assert.match(app, /研究白板/);
  assert.match(boardCss, /border:2px solid/);
  assert.match(boardCss, /button\.active\{background:var\(--green\);color:#fff/);
  assert.match(app, /isCurrentConversationLoad/);
  assert.doesNotMatch(app, /setTimeout\(renderConversation,/);
});

test("research board cards and inner controls use visible borders", () => {
  assert.match(boardCss, /\.board-card\{border:2px solid var\(--board-border\)/);
  assert.match(boardCss, /\.route-card,\.decision-card,\.status-row\{border:1px solid var\(--board-inner-border\)/);
  assert.match(boardCss, /\.board-empty-list\{border:2px dashed/);
  assert.match(boardCss, /\.route-actions button:hover/);
});

test("MathCat whiteboard renders live research data instead of hiding non-empty sections", () => {
  assert.match(app, /function boardSummary\(board\)/);
  assert.match(app, /function goalClaimList\(board\)/);
  assert.match(app, /function executionPanel\(board\)/);
  assert.match(app, /function uncertaintyPanel\(board\)/);
  assert.match(app, /function verificationPanel\(board\)/);
  assert.match(app, /board\.claims\.map/);
  assert.match(app, /board\.verificationQueue\.slice/);
  assert.match(app, /board-run-status/);
  assert.match(boardCss, /\.board-summary\{/);
  assert.match(boardCss, /\.research-item\.trusted/);
  assert.match(boardCss, /\.verification-grid\{/);
  assert.match(app, /function experimentPanel\(board\)/);
  assert.match(app, /计算证据/);
  assert.match(app, /不等于一般证明/);
  assert.match(boardCss, /\.experiment-card\{/);
  assert.match(app, /renderMessageText\(board\.problem\.statement,state\.conversationId\)/);
  assert.match(app, /renderMessageText\(board\.problem\.goal,state\.conversationId\)/);
});

test("MathCat questions use a dedicated cat card with context, notes, defer, and history", () => {
  assert.match(app, /猫猫提问卡/);
  assert.match(app, /mathcat:"MathCat",rethlas:"Rethlas",danus:"Danus"/);
  assert.match(app, /想问你/);
  assert.match(app, /为什么需要你/);
  assert.match(app, /补充给猫猫的话/);
  assert.match(app, /我再想想，先不要阻塞其它路线/);
  assert.match(app, /查看已回答的问题/);
  assert.doesNotMatch(app, /data-add-decision/);
  assert.match(app, /item\.source\|\|board\.agent/);
  assert.match(app, /if\(!succeeded\)controls\.forEach\(control=>control\.disabled=false\)/);
  assert.match(boardCss, /\.cat-question-card\{padding:12px;border:1px solid/);
  assert.match(boardCss, /\.question-option:hover:not\(:disabled\)/);
  assert.match(boardCss, /\.question-history-item/);
});

test("research boards expose expandable proof-tree and dependency-graph workspaces", () => {
  assert.match(app, /data-open-graph="proof"/);
  assert.match(app, /data-open-graph="dependency"/);
  assert.match(app, /graphDialog\.id = "researchGraphDialog"/);
  assert.match(app, /data-graph-zoom="in"/);
  assert.match(app, /data-collapse-node/);
  assert.match(boardCss, /\.graph-preview-grid\{display:grid/);
  assert.match(boardCss, /\.research-graph-dialog\{/);
  assert.match(boardCss, /\.graph-node\.kind-fact>rect/);
  assert.match(boardCss, /\.graph-edge\.contradiction path/);
});

test("MathCat route approval follows backend human-review state", () => {
  assert.match(app, /route\.humanStatus==="pending"/);
  assert.match(app, /待人工批准/);
});

test("local startup launches and health-checks the MathCat backend", () => {
  assert.match(startScript, /math-research-agent\.exe/);
  assert.match(startScript, /Test-MathCat/);
  assert.match(startScript, /platformHealth\.mathcat\.status/);
});
