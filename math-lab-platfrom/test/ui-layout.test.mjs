import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";

const index = await fs.readFile(new URL("../public/index.html", import.meta.url), "utf8");
const app = await fs.readFile(new URL("../public/app.js", import.meta.url), "utf8");
const boardCss = await fs.readFile(new URL("../public/research-board.css", import.meta.url), "utf8");
const statusLabels = await fs.readFile(new URL("../public/status-labels.js", import.meta.url), "utf8");
const routePresentation = await fs.readFile(new URL("../public/route-presentation.js", import.meta.url), "utf8");
const startScript = await fs.readFile(new URL("../../scripts/start-version.ps1", import.meta.url), "utf8");
const server = await fs.readFile(new URL("../src/server.mjs", import.meta.url), "utf8");

test("sidebar exposes a normal settings button and controls live in its dialog", () => {
  const sidebar = index.match(/<aside\b[^>]*class="sidebar"[^>]*>[\s\S]*?<\/aside>/)?.[0] || "";
  const settings = index.match(/<dialog id="settingsDialog"[\s\S]*?<\/dialog>/)?.[0] || "";
  const composer = index.match(/<section class="composer">[\s\S]*?<\/section>/)?.[0] || "";
  assert.match(sidebar, /id="settingsButton"/);
  assert.doesNotMatch(sidebar, /id="executor"|id="permission"/);
  assert.match(settings, /id="executor"/);
  assert.match(settings, /id="permission"/);
  assert.match(settings, /id="codexSettingsPanel"/);
  assert.doesNotMatch(composer, /id="executor"|id="permission"/);
  assert.match(app, /\$\("settingsButton"\)\.onclick = \(\) => openCodexSettings\(\)/);
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

test('research start validates explicit per-run collaboration options',async()=>{
  const {researchStartOptions}=await import('../public/research-start.js');
  assert.equal(researchStartOptions({mode:'collaborative'}).mode,'collaborative');
  assert.equal(researchStartOptions({mode:'delegated'}).mode,'delegated');
  assert.throws(()=>researchStartOptions({mode:'invalid'}));
  assert.match(app,/mathcatReviewMode:startOptions.mode==='collaborative'/);
});

test("MathCat whiteboard has a persistent human collaboration console with real forms", () => {
  assert.match(app, /人工协作台/);
  assert.match(app, /编写并执行路线/);
  assert.match(app, /给规划建议/);
  assert.match(app, /重新梳理目标与讨论/);
  assert.match(app, /白板研究设置/);
  assert.match(app, /id="humanRouteForm"/);
  assert.match(app, /id="planningSuggestionForm"/);
  assert.match(app, /id="goalReviewForm"/);
  assert.match(app, /id="boardSettingsForm"/);
  assert.match(app, /max="16"/);
  assert.match(app, /max="1440"/);
  assert.match(app, /路线通过依赖、重复与预算检查后立即进入执行队列/);
  assert.match(app, /未完成任务会收到中止信号；已确认的事实与研究成果会保留/);
  assert.doesNotMatch(app, /prompt\("研究路线名称"/);
  assert.match(app, /dataset\.submitting/);
  assert.match(app, /goalReviewSnapshot/);
  assert.match(app, /goal-review-receipt/);
  assert.match(boardCss, /\.human-collaboration-console\{/);
  assert.match(boardCss, /\.collaboration-action-grid\{/);
  assert.match(boardCss, /\.board-action-dialog\{/);
});

test("a MathCat outage does not prevent the conversation from rendering", () => {
  assert.match(app, /const boardRequest = isClassicResearch\(originalConversation\)[\s\S]*:shouldRefreshBoard/);
  assert.match(app, /classicError=error;const saved=state\.classicCache/);
  assert.match(app, /\{ board: null, error \}/);
  assert.match(app, /研究白板暂时无法连接/);
  assert.match(app, /对话内容仍可正常查看/);
});

test("user-generated active files cannot execute with platform origin privileges", () => {
  assert.match(server, /ACTIVE_CONTENT_EXTENSIONS/);
  assert.match(server, /"content-type":active\?"application\/octet-stream":mime\(file\)/);
  assert.match(server, /"x-content-type-options":"nosniff"/);
  assert.match(server, /"content-security-policy":"sandbox;/);
  assert.doesNotMatch(app, /target="_blank"(?! rel="noopener")/);
});

test("human collaboration actions call dedicated platform APIs and refresh watchers", () => {
  assert.match(app, /\/suggestions/);
  assert.match(app, /\/goal-review/);
  assert.match(app, /\/settings/);
  assert.match(app, /executeImmediately:true/);
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
  assert.match(app, /visibleClaims\.map/);
  assert.match(app, /board\.verificationQueue\.slice/);
  assert.match(app, /board-run-status/);
  assert.match(boardCss, /\.board-summary\{/);
  assert.match(boardCss, /\.research-item\.trusted/);
  assert.match(boardCss, /\.verification-grid\{/);
  assert.match(app, /function experimentPanel\(board\)/);
  assert.match(app, /计算证据/);
  assert.match(app, /不等于一般证明/);
  assert.match(boardCss, /\.experiment-card\{/);
  assert.match(app, /renderMessageText\(statement,state\.conversationId\)/);
  assert.match(app, /class="problem-context"/);
  assert.match(app, /function visibleBoardGoals\(board\)/);
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
  assert.match(app, /id="graphReadingGuide"/);
  assert.match(app, /data-graph-jump/);
  assert.match(boardCss, /\.graph-node-status/);
  assert.match(boardCss, /\.graph-node\.kind-strategy>rect/);
  assert.match(boardCss, /\.graph-node\.status-unplanned>rect/);
  assert.match(boardCss, /\.research-graph-dialog \.graph-inspector\{display:block/);
});

test("graph overview separates cross-references and fits a truly focused branch", () => {
  assert.match(app, /import \{ routeGraphEdges \} from "\.\/graph-edges\.js"/);
  assert.match(app, /graphReferencesButton\.dataset\.graphAction = "references"/);
  assert.match(app, /focusGraphBranch\(visible,graphUi\.selectedId\)/);
  assert.match(app, /graphUi\.focus=!graphUi\.focus;fitCurrentGraph\(\)/);
  assert.match(app, /item\.showLabel\?/);
  assert.match(app, /hiddenCrossLinkCount/);
  assert.match(boardCss, /\.research-graph-svg\{min-width:0;min-height:0\}/);
  assert.match(boardCss, /\.graph-edge\.cross-reference path/);
});

test("MathCat whiteboards show auditable live stages and proof-map activity markers", () => {
  assert.match(app, /function researchActivityPanel\(board\)/);
  assert.match(app, /MathCat 当前在做什么/);
  assert.match(app, /class="research-stage-bar"/);
  assert.match(app, /不包含模型私有思维链/);
  assert.match(app, /class="route-live-marker/);
  assert.match(app, /id="graphLiveStatus"/);
  assert.match(app, /graphActivityKey\(board\)/);
  assert.match(app, /graphUi\.activityKey !== graphActivityKey\(state\.currentBoard\)/);
  assert.match(app, /if\(boardOpen\)\{renderResearchBoard\(\);if\(refresh&&state\.conversationId\)void renderConversation\(\)/);
  assert.match(app, /class="graph-activity-ring"/);
  assert.match(app, /当前运行标志/);
  assert.match(app, /class="legend-live"><\/i>当前正在处理/);
  assert.match(boardCss, /\.research-live-card\{/);
  assert.match(boardCss, /\.research-stage-bar\{/);
  assert.match(boardCss, /\.route-live-marker\{/);
  assert.match(boardCss, /\.graph-live-status\{/);
  assert.match(boardCss, /\.graph-node\.kind-task>rect/);
  assert.match(boardCss, /\.graph-node \.graph-activity-ring/);
  assert.match(boardCss, /\.research-stage-bar li\{gap:6px;color:#68766f;font-size:12px\}/);
  assert.match(boardCss, /\.graph-live-status\{font-size:12px\}/);
  assert.match(boardCss, /\.graph-current-activity p\{font-size:12px/);
  assert.match(boardCss, /\.legend-live\{/);
});

test("research routes separate readable decisions from raw planner prose", () => {
  assert.match(app, /function routeReadableParts\(route\)/);
  assert.match(app, /function routePlainExplanation\(route\)/);
  assert.match(app, /groupResearchRoutes\(board\.routes,board,\{includeExpected:true\}\)/);
  assert.match(routePresentation, /export function routePresentation/);
  assert.match(routePresentation, /plainLanguageSummary/);
  assert.match(routePresentation, /fallbackSteps/);
  assert.match(app, /这条路线具体在做什么/);
  assert.match(app, /为什么现在做/);
  assert.match(app, /完成时会得到/);
  assert.match(app, /离最终证明还有多远/);
  assert.match(app, /function humanizeRouteStep\(value\)/);
  assert.match(app, /明确量词与对象范围/);
  assert.match(app, /执行路径/);
  assert.match(app, /完成后还缺什么/);
  assert.match(app, /不能算成功/);
  assert.match(app, /查看智能体原始技术说明/);
  assert.match(app, /批准并启动 Worker/);
  assert.match(boardCss, /\.route-readable-notes/);
  assert.match(boardCss, /\.route-plain/);
  assert.match(boardCss, /\.route-raw/);
  assert.match(boardCss, /\.route-family\.unplanned/);
  assert.match(boardCss, /\.route-role-badge/);
});

test("research directions and route details have stable accessible disclosures", () => {
  assert.match(app, /createResearchDisclosureStore/);
  assert.match(app, /data-route-family-toggle/);
  assert.match(app, /data-route-detail-toggle/);
  assert.match(app, /data-graph-detail-toggle/);
  assert.match(app, /aria-expanded=/);
  assert.match(app, /researchDisclosure\.toggle\(board\.id,"family"/);
  assert.match(app, /researchDisclosure\.toggle\(board\.id,"route"/);
  assert.match(app, /researchDisclosure\.toggle\(graphUi\.boardId,"node"/);
  assert.match(boardCss, /\.route-family-content\[hidden\]/);
  assert.match(boardCss, /\.route-card-body\[hidden\]/);
  assert.match(boardCss, /\.graph-node-detail-content\[hidden\]/);
  assert.match(boardCss, /\.route-detail-toggle:focus-visible/);
});

test("internal route constraints are not repeated as research conclusions", () => {
  assert.match(app, /board\.claims\.filter\(item=>item\.kind!=="hypothesis"\)/);
  assert.match(app, /路线内部约束已整理到上方路线卡/);
  assert.match(app, /子目标与研究结论/);
  assert.match(boardCss, /\.internal-constraint-note/);
});

test("machine board statuses have readable Chinese labels", () => {
  assert.match(statusLabels, /needs_human_review: "等待人工决定"/);
  assert.match(statusLabels, /partial_success: "部分完成"/);
});

test("verbose internal planner logs are collapsed behind a readable summary", () => {
  assert.match(app, /function readableResearchEvent\(value\)/);
  assert.match(app, /智能体完成一轮策略生成、路线反思和监督筛选/);
  assert.match(app, /规划器运行成功/);
  assert.match(boardCss, /\.timeline-raw/);
});

test("MathCat route approval follows backend human-review state", () => {
  assert.match(app, /\["pending","proposed"\]\.includes\(route\.humanStatus\)/);
  assert.match(app, /等待你的决定/);
  assert.match(app, /批准并启动 Worker/);
  assert.match(app, /不必等待其它路线/);
});

test("local startup launches and health-checks the MathCat backend", () => {
  assert.match(startScript, /mathcat-v2\.exe/);
  assert.match(startScript, /Get-Health/);
  assert.match(startScript, /platformHealth\.version/);
  assert.match(startScript, /127\.0\.0\.1:4335/);
  assert.match(startScript, /127\.0\.0\.1:8900/);
});
