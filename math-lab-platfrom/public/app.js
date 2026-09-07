import {WhiteboardClient} from './whiteboard-client.js';
import {chooseResearchStart} from './research-start.js';
import {normalizeLocalPath,workspaceFileUrl,workspaceRevealUrl,prepareMessageLinks} from './workspace-links.js';
import {CodexStatusController,selectionSummary} from './codex-status.js';
import {createWorkspaceMemory} from './workspace-memory.js';
import {explicitProblemEdit} from './problem-edit-intent.js';
import {bindSidebarNavigation} from './sidebar-navigation.js';
import {ResearchWhiteboard} from './whiteboard-view.js';
import {ClassicResearchClient,isClassicResearch,needsResearchPolling,renderClassicResearchBoard} from "./research-v2-classic.js";
import {label as researchLabel,commandFor,latestRun,conversationCaption} from "./research-v2-state.js";
import { resolveResearchView, isCurrentConversationLoad, normalizeResearchBoard } from "./view-state.js";
import { renderLatexText } from "./math-renderer.js";
import { buildProofTree, buildDependencyGraph, graphSummary, visibleGraph, focusGraphBranch, layoutGraph, overrideGraphPositions, relatedGraphIds, fitGraphScale, GRAPH_STATUS_LABELS, GRAPH_KIND_LABELS, GRAPH_RELATION_LABELS } from "./research-graph.js";
import { routeGraphEdges } from "./graph-edges.js";
import { statusLabel as boardStatus, boardReviewMode } from "./status-labels.js";
import { catIcon, CAPABILITY_ICONS } from "./icons.js";
import { groupResearchRoutes, routePresentation } from "./route-presentation.js";
import { deriveResearchActivity } from "./research-activity.js";
import { createResearchDisclosureStore } from "./research-disclosure.js";
import katex from "./vendor/katex/katex.esm.js";

const state = { workspaces: [], conversations: [], capabilities: [], researchAgents: [], workspaceId: null, conversationId: null, conversationLoadId: 0, executor: "codex", capabilityId: "", researchAgentId: "mathcat", permission: "workspace-write", activeView: "chat", currentBoard: null, boardError: null, projectlessExpanded: true, expanded: new Set(), expandedThinking: new Set(), poll: null, renderedConversationId: null, renderedMessagesKey: null };
try { const savedPermission = localStorage.getItem("mathcat.permission"); if (["workspace-write", "read-only"].includes(savedPermission)) state.permission = savedPermission; } catch {}
const workspaceMemory=createWorkspaceMemory(),savedWorkspace=workspaceMemory.session();
state.capabilityId=savedWorkspace.capabilityId||'';state.capabilityNeedsChoice=Boolean(savedWorkspace.capabilityNeedsChoice);state.workspaceId=savedWorkspace.workspaceId||null;
state.expanded=new Set(savedWorkspace.expanded||[]);
const $ = (id) => document.getElementById(id);
const sidebarNavigation=bindSidebarNavigation({sidebar:$("conversationSidebar"),main:document.querySelector("main"),trigger:$("sidebarToggle"),closeButton:$("sidebarClose"),backdrop:$("sidebarBackdrop")});
const classicResearch = new ClassicResearchClient({chooseStart:chooseStartWithModel});
const whiteboardClient = new WhiteboardClient(classicResearch);
let researchWhiteboard = null;
const codexStatus=new CodexStatusController({summaryElement:$('settingsSummary'),panelElement:$('codexSettingsPanel'),onChange:(status,projectId)=>researchWhiteboard?.updateCodexStatus(status,projectId)});
function currentCodexProject(){
  if(!state.conversationId)return null;
  if(state.codexConversationScope?.conversationId===state.conversationId)return state.codexConversationScope.projectId;
  return state.conversations.find(c=>c.id===state.conversationId)?.researchProjectId||null;
}
async function chooseStartWithModel({projectId=null,...options}={}){
  const conversationId=state.conversationId;
  const status=await codexStatus.refresh({projectId});
  if(conversationId!==state.conversationId)return null;
  const preview=options.locked?{...status,status:'available',selection:{model:options.initial?.model,reasoning_effort:options.initial?.reasoning_effort}}:status;
  return chooseResearchStart({...options,modelSummary:options.locked&&!options.initial?.model?'原启动回执未记录模型':selectionSummary(preview)});
}
function openCodexSettings(projectId=currentCodexProject()){
  codexStatus.setContext(projectId);codexStatus.resetDraft();$('settingsDialog').showModal();void codexStatus.refresh();
}
setInterval(()=>{if(!document.hidden)void codexStatus.refresh();},60000);
document.title = "MathCat Lab 2.5.0";
let draftContext=null;
const draftKey=()=>state.conversationId?'conversation:'+state.conversationId:'new:'+(state.workspaceId||'none');
function persistWorkspace(){
  if(draftContext)workspaceMemory.saveDraft(draftContext,$('prompt').value);
  workspaceMemory.saveSession({conversationId:state.conversationId,workspaceId:state.workspaceId,capabilityId:state.capabilityId,capabilityNeedsChoice:Boolean(state.capabilityNeedsChoice),activeView:state.activeView,expanded:[...state.expanded]});
  if(state.conversationId)workspaceMemory.saveView(state.conversationId,{activeView:state.activeView,chatScroll:$('messages').scrollTop});
  researchWhiteboard?.persist();
}
function syncPromptDraft(){const key=draftKey();if(draftContext===key)return;if(draftContext)workspaceMemory.saveDraft(draftContext,$('prompt').value);draftContext=key;$('prompt').value=workspaceMemory.draft(key);}
globalThis.addEventListener('pagehide',persistWorkspace);
document.addEventListener('visibilitychange',()=>{if(document.hidden)persistWorkspace();});
const thinkingStyles = document.createElement("link");
thinkingStyles.rel = "stylesheet";
thinkingStyles.href = "/thinking.css";
document.head.append(thinkingStyles);
const contextPicker = document.createElement("div");
contextPicker.className = "context-picker";
contextPicker.innerHTML = `<button id="contextTrigger" class="context-trigger" type="button" aria-haspopup="listbox" aria-expanded="false"><svg class="context-cat" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5.2 9.2 4.5 4l4.4 2.5a10 10 0 0 1 6.2 0L19.5 4l-.7 5.2A7.7 7.7 0 0 1 20 13.3c0 4.2-3.6 6.7-8 6.7s-8-2.5-8-6.7c0-1.5.4-2.8 1.2-4.1Z"></path><path d="M8.3 12.3h.1m7.2 0h.1M9.5 16c1.5 1 3.5 1 5 0M12 14v1"></path></svg><span class="context-copy"><strong id="contextTitle">项目：无工作区</strong><small id="contextPath">当前对话不绑定本地项目</small></span><span class="context-chevron">⌄</span></button><div id="contextMenu" class="context-menu" role="listbox" hidden></div>`;
$("prompt").closest(".composer").querySelector(".toolbar").prepend(contextPicker);
const researchPicker = document.createElement("div");
researchPicker.className = "research-picker";
researchPicker.hidden = true;
researchPicker.innerHTML = `<button id="researchAgentTrigger" class="research-agent-trigger" type="button" aria-haspopup="listbox" aria-expanded="false"><span id="researchAgentIcon"></span><span class="research-agent-copy"><small>具体智能体</small><strong id="researchAgentLabel">MathCat</strong></span><span class="picker-chevron">⌄</span></button><div id="researchAgentMenu" class="research-agent-menu" role="listbox" hidden></div>`;
document.querySelector(".capability-picker").after(researchPicker);
const viewSwitch = document.createElement("div");
viewSwitch.className = "view-switch";
viewSwitch.setAttribute("aria-label", "对话与研究白板视图切换");
viewSwitch.innerHTML = `<span class="view-switch-label">当前视图</span><button type="button" data-view="chat" class="active" aria-pressed="true"><span aria-hidden="true">💬</span> 对话</button><button type="button" data-view="board" aria-pressed="false"><span aria-hidden="true">▦</span> 研究白板</button>`;
document.querySelector(".header-actions").prepend(viewSwitch);
const boardElement = document.createElement("section");
boardElement.id = "researchBoard";
boardElement.className = "research-board";
boardElement.hidden = true;
$("messages").after(boardElement);
const graphDialog = document.createElement("dialog");
graphDialog.id = "researchGraphDialog";
graphDialog.className = "research-graph-dialog";
graphDialog.innerHTML = `<div class="graph-dialog-shell"><header class="graph-dialog-head"><div><span class="graph-dialog-kicker">研究结构</span><h2 id="graphDialogTitle">证明路线图</h2><p id="graphDialogSubtitle">先看研究方向，再展开具体技巧与数学结论</p></div><div class="graph-dialog-head-actions"><span id="graphLiveStatus" class="graph-live-status" aria-live="polite"></span><button type="button" class="graph-dialog-close" data-graph-close aria-label="关闭图谱">×</button></div></header><div class="graph-toolbar"><div class="graph-tabs" role="tablist"><button type="button" role="tab" data-graph-view="proof">证明路线</button><button type="button" role="tab" data-graph-view="dependency">依赖图</button></div><label class="graph-search"><span aria-hidden="true">⌕</span><input id="graphSearch" type="search" placeholder="搜索节点或数学陈述"></label><select id="graphFilter" aria-label="筛选图谱节点"><option value="all">全部节点</option><option value="active">进行中与阻塞</option><option value="unverified">未验证依赖</option><option value="problems">阻塞、失败与反例</option></select><div class="graph-actions" aria-label="图谱布局工具"><button type="button" data-graph-action="focus" aria-pressed="false" title="突出当前节点所在的完整分支">聚焦分支</button><button type="button" data-graph-action="fit" title="缩放到完整图谱概览">概览全图</button><button type="button" data-graph-action="arrange" title="清除手动位置并恢复自动布局">自动整理</button></div><div class="graph-zoom" aria-label="缩放图谱"><button type="button" data-graph-zoom="out" aria-label="缩小">−</button><button type="button" data-graph-zoom="reset" id="graphZoomLabel">100%</button><button type="button" data-graph-zoom="in" aria-label="放大">＋</button></div></div><div id="graphReadingGuide" class="graph-reading-guide"></div><div class="graph-workspace"><div id="graphStage" class="graph-stage" tabindex="0"></div><aside id="graphInspector" class="graph-inspector"></aside></div><footer class="graph-legend"><span><i class="legend-shape theorem"></i>目标</span><span><i class="legend-shape strategy"></i>研究方向</span><span><i class="legend-shape route"></i>具体路线</span><span><i class="legend-shape candidate"></i>候选</span><span><i class="legend-shape fact"></i>已验证 Fact</span><span><i class="legend-live"></i>当前正在处理</span><span><i class="legend-line contradiction"></i>冲突</span><small>拖动节点可固定位置；双击节点恢复自动位置</small></footer></div>`;
document.body.append(graphDialog);
const graphReferencesButton = document.createElement("button");
graphReferencesButton.type = "button";
graphReferencesButton.dataset.graphAction = "references";
graphReferencesButton.setAttribute("aria-pressed", "false");
graphReferencesButton.title = "显示全部跨路线引用；关闭时仍显示所选节点的引用和冲突";
graphReferencesButton.textContent = "全部引用";
graphDialog.querySelector(".graph-actions").prepend(graphReferencesButton);
const collaborationDialogs = document.createElement("div");
collaborationDialogs.innerHTML = `
  <dialog id="humanRouteDialog" class="board-action-dialog" aria-labelledby="humanRouteDialogTitle">
    <form id="humanRouteForm">
      <div class="board-dialog-head"><div><span>人工路线</span><h2 id="humanRouteDialogTitle">编写并执行路线</h2><p>把研究者的想法直接变成一条可执行路线。</p></div><button type="button" class="dialog-close" data-close-board-dialog aria-label="关闭">×</button></div>
      <div class="board-dialog-notice"><strong>提交后立即排队</strong><span>路线通过依赖、重复与预算检查后立即进入执行队列；它仍是研究方案，不会被当作数学结论。</span></div>
      <label><span>路线名称</span><small>用一句话说清采用什么角度或技巧</small><input id="humanRouteTitle" maxlength="120" required placeholder="例如：从局部化与素理想链直接证明"></label>
      <label><span>针对的研究目标</span><small>可指定一个当前目标；留空时由 MathCat 关联到主目标</small><select id="humanRouteGoal"><option value="">主目标（自动关联）</option></select></label>
      <label><span>Worker 目标</span><small>这次执行需要具体完成什么</small><textarea id="humanRouteObjective" maxlength="1200" required placeholder="例如：把命题局部化到每个素理想，并尝试构造可拼接的局部引理。"></textarea></label>
      <label><span>执行思路</span><small>给出可以实际开始的步骤，而非只写研究方向</small><textarea id="humanRouteSummary" maxlength="2400" required placeholder="写下拟采用的工具、切入点和关键步骤"></textarea></label>
      <label><span>完成标准</span><small>什么产出才算这条路线执行完毕</small><textarea id="humanRouteContract" maxlength="1200" required placeholder="例如：给出完整证明；若失败，定位最小缺口并给出可核验的反例条件。"></textarea></label>
      <label><span>已知风险（可选）</span><small>每行一项，MathCat 会保留这些边界</small><textarea id="humanRouteRisks" maxlength="1200" placeholder="例如：局部结论可能无法全局拼接"></textarea></label>
      <p class="board-dialog-error" data-dialog-error aria-live="assertive"></p>
      <div class="dialog-actions"><button type="button" data-close-board-dialog>取消</button><button type="submit" class="primary">编写并立即执行</button></div>
    </form>
  </dialog>
  <dialog id="planningSuggestionDialog" class="board-action-dialog" aria-labelledby="planningSuggestionDialogTitle">
    <form id="planningSuggestionForm">
      <div class="board-dialog-head"><div><span>影响后续规划</span><h2 id="planningSuggestionDialogTitle">给规划建议</h2><p>建议会进入下一轮 Planner 上下文，并在白板中留痕。</p></div><button type="button" class="dialog-close" data-close-board-dialog aria-label="关闭">×</button></div>
      <label><span>建议内容</span><small>可以指定更多采用某个数学领域、工具或论证风格</small><textarea id="planningSuggestionContent" maxlength="2400" required placeholder="例如：后续希望更多从交换代数的局部化、深度和关联素理想角度规划。"></textarea></label>
      <label><span>作用范围</span><small>留空表示影响整个项目的后续规划</small><select id="planningSuggestionRoute"><option value="">整个研究规划</option></select></label>
      <label><span>原因（可选）</span><small>帮助 Planner 理解为什么现在需要改变侧重点</small><textarea id="planningSuggestionReason" maxlength="1000" placeholder="例如：当前路线过度依赖计算，没有利用环的局部结构。"></textarea></label>
      <p class="board-dialog-error" data-dialog-error aria-live="assertive"></p>
      <div class="dialog-actions"><button type="button" data-close-board-dialog>取消</button><button type="submit" class="primary">提交规划建议</button></div>
    </form>
  </dialog>
  <dialog id="goalReviewDialog" class="board-action-dialog" aria-labelledby="goalReviewDialogTitle">
    <form id="goalReviewForm">
      <div class="board-dialog-head"><div><span>强制规划动作</span><h2 id="goalReviewDialogTitle">重新梳理目标与讨论</h2><p>立即发起一次新的目标确认，重新讨论“要证明什么、先做什么”。</p></div><button type="button" class="dialog-close" data-close-board-dialog aria-label="关闭">×</button></div>
      <div class="board-dialog-notice warning"><strong>会开启新一轮</strong><span>未完成任务会收到中止信号；已确认的事实与研究成果会保留，并成为新一轮讨论的依据。</span></div>
      <label><span>本轮讨论重点</span><small>指出希望重新审视的目标、假设、路线边界或数学视角</small><textarea id="goalReviewFocus" maxlength="2400" required placeholder="例如：重新区分主猜想与可先证明的窄化结论，并讨论交换代数路线是否应成为主路线。"></textarea></label>
      <p class="board-dialog-error" data-dialog-error aria-live="assertive"></p>
      <div class="dialog-actions"><button type="button" data-close-board-dialog>取消</button><button type="submit" class="primary">立即重新梳理</button></div>
    </form>
  </dialog>
  <dialog id="boardSettingsDialog" class="board-action-dialog" aria-labelledby="boardSettingsDialogTitle">
    <form id="boardSettingsForm">
      <div class="board-dialog-head"><div><span>当前研究</span><h2 id="boardSettingsDialogTitle">白板研究设置</h2><p>只调整当前 MathCat 项目；新研究的分工方式在启动时选择。</p></div><button type="button" class="dialog-close" data-close-board-dialog aria-label="关闭">×</button></div>
      <div class="board-settings-grid">
        <label><span>最大任务并行数</span><small>同时运行的 Worker 上限（1–16）</small><input id="boardMaxParallelWorkers" type="number" min="1" max="16" step="1" required></label>
        <label><span>单任务最长运行时间</span><small>每个 Worker 的上限（1–1440 分钟）</small><div class="number-with-unit"><input id="boardMaxMinutesPerTask" type="number" min="1" max="1440" step="1" required><span>分钟</span></div></label>
      </div>
      <label><span>人工参与程度</span><small id="boardReviewModeHelp">控制路线和关键数学歧义在何处等待研究者</small><select id="boardReviewMode" required><option value="automatic">自动运行 · 不等待常规审核</option><option value="balanced">关键节点审核 · 只询问重要歧义</option><option value="strict">全程审核 · 每条新路线都确认</option></select></label>
      <p class="board-dialog-hint">并行数与时限会写入后续新任务的执行契约；已经运行的 Worker 不会被强行改写。</p>
      <p id="boardBudgetUnavailable" class="board-dialog-hint" hidden>当前 MathCat 后端没有返回完整预算。你仍可修改人工参与程度，预算字段将在后端更新后开放。</p>
      <p class="board-dialog-error" data-dialog-error aria-live="assertive"></p>
      <div class="dialog-actions"><button type="button" data-close-board-dialog>取消</button><button type="submit" class="primary">保存当前研究设置</button></div>
    </form>
  </dialog>`;
while (collaborationDialogs.firstElementChild) document.body.append(collaborationDialogs.firstElementChild);
document.querySelectorAll("[data-close-board-dialog]").forEach((button) => { button.onclick = () => button.closest("dialog")?.close(); });
document.querySelectorAll(".board-action-dialog").forEach((dialog) => { dialog.addEventListener("click", (event) => { if (event.target === dialog && dialog.dataset.submitting !== "true") dialog.close(); }); dialog.addEventListener("cancel",(event)=>{if(dialog.dataset.submitting==="true")event.preventDefault();}); });
const graphUi = { boardId: null, view: "proof", scale: 1, filter: "all", query: "", selectedId: null, collapsed: new Set(), focus: false, showCrossLinks: false, activityKey: "", positions: { proof: new Map(), dependency: new Map() } };
const researchDisclosure = createResearchDisclosureStore();
document.querySelector(".brand-mark").innerHTML = catIcon("cat-brand");
function renderContextPicker() {
  const workspace = state.workspaces.find((item) => item.id === state.workspaceId);
  $("contextTitle").textContent = `项目：${workspace?.name || "无工作区"}`;
  $("contextPath").textContent = workspace?.path || "当前对话不绑定本地项目";
  $("contextTrigger").title = workspace ? `当前工作区：${workspace.path}` : "无工作区：当前对话不绑定用户项目";
  $("contextTrigger").querySelector(".context-cat").outerHTML = catIcon("cat-project").replace('class="cat-icon"', 'class="context-cat"');
  $("contextMenu").innerHTML = `<button type="button" class="context-option ${workspace ? "" : "selected"}" role="option" data-context-id=""><span class="context-option-icon">${catIcon("cat-unbound")}</span><span><strong>无工作区</strong><small>不读取已添加的本地项目</small></span><em>${workspace ? "" : "✓"}</em></button>` + state.workspaces.map((item) => `<button type="button" class="context-option ${item.id === state.workspaceId ? "selected" : ""}" role="option" data-context-id="${esc(item.id)}"><span class="context-option-icon">📁</span><span><strong>${esc(item.name)}</strong><small>${esc(item.path)}</small></span><em>${item.id === state.workspaceId ? "✓" : ""}</em></button>`).join("") + `<button type="button" class="context-option context-add" data-context-add><span class="context-option-icon">＋</span><span><strong>添加新工作区</strong><small>选择一个本地文件夹</small></span></button>`;
  $("contextMenu").querySelectorAll("[data-context-id]").forEach((button) => button.onclick = async () => { closeContextMenu(); await createChat(button.dataset.contextId || null); });
  $("contextMenu").querySelector("[data-context-add]").onclick = () => { closeContextMenu(); $("workspaceDialog").showModal(); };
}
async function api(url, options = {}) { const response = await fetch(url, { headers: { "content-type": "application/json", ...(options.headers || {}) }, ...options }); const value = await response.json(); if (!response.ok) { const error=new Error(value.error || `HTTP ${response.status}`); error.status=response.status; throw error; } return value; }
function readableBoardError(error) {
  const message=String(error?.message||"未知错误");
  if(/revision|stale|expected_revision|conflict/i.test(message))return"白板在提交期间已被 MathCat 更新。请刷新白板后重试；本次操作没有被标记为成功。";
  return message;
}
function setBoardDialogBusy(dialog,busy) {
  dialog.dataset.submitting=String(Boolean(busy));
  dialog.setAttribute("aria-busy",String(Boolean(busy)));
  dialog.querySelectorAll("button,input,select,textarea").forEach(control=>{control.disabled=Boolean(busy)||control.dataset.unavailable==="true";});
  const submit=dialog.querySelector("button[type='submit']");
  if(submit){submit.classList.toggle("is-loading",Boolean(busy));if(busy){submit.dataset.idleLabel=submit.textContent;submit.textContent="正在提交，不可撤回…";}else if(submit.dataset.idleLabel){submit.textContent=submit.dataset.idleLabel;delete submit.dataset.idleLabel;}}
  if(busy)dialog.querySelector("[data-dialog-error]").textContent="正在提交到 MathCat，请等待完成。";
}
function openBoardDialog(dialog,focusTarget) {
  dialog.querySelector("[data-dialog-error]").textContent="";
  setBoardDialogBusy(dialog,false);
  dialog.showModal();
  requestAnimationFrame(()=>dialog.querySelector(focusTarget)?.focus());
}
function esc(value) { return String(value).replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[char])); }
const fileUrl=workspaceFileUrl,revealUrl=workspaceRevealUrl;
function fileLinks(conversationId, filePath, label = "", fragment = "") {
  const normalized = normalizeLocalPath(filePath);
  const visible = label || normalized.replaceAll("\\", "/");
  return `<span class="file-actions"><a class="file-link" target="_blank" rel="noopener" href="${esc(fileUrl(conversationId, normalized)+fragment)}" title="${esc(normalized)}">${esc(visible)}</a><button type="button" class="reveal-link" data-reveal-path="${esc(normalized)}" data-reveal-conversation="${esc(conversationId)}" title="${esc(normalized)}">在资源管理器中显示</button></span>`;
}
async function revealFile(button, conversationId) {
  const original = button.textContent;
  button.disabled = true;
  button.textContent = "正在打开…";
  try { await api(revealUrl(button.dataset.revealConversation||conversationId, button.dataset.revealPath)); button.textContent = "已打开"; }
  catch (error) { button.textContent = original; alert(`无法打开资源管理器：${error.message}`); }
  finally { setTimeout(() => { button.disabled = false; button.textContent = original; }, 1200); }
}
function renderMessageText(value, conversationId) {
  const links=prepareMessageLinks(value,conversationId,fileLinks);
  const text = renderLatexText(links.text, { escapeHtml: esc, renderFormula: (source, displayMode) => {
    return `<span class="rendered-math${displayMode ? " display" : ""}">${katex.renderToString(source, { displayMode, throwOnError: true, strict: "warn", trust: false, output: "htmlAndMathml" })}</span>`;
  } });
  return links.restore(text);
}

function renderWorkspaceTree() {
  const tree = $("workspaceTree");
  const visibleConversations = state.conversations.filter((item) => Number(item.messageCount) > 0 || isClassicResearch(item));
  const projectless = visibleConversations.filter((item) => item.workspaceId == null);
  const projectlessGroup = `<section class="workspace-group ${state.conversationId && state.workspaceId == null ? "active" : ""}"><button class="workspace-row" data-projectless aria-expanded="${state.projectlessExpanded}"><span class="chevron">${state.projectlessExpanded ? "⌄" : "›"}</span><span class="folder">💬</span><span class="workspace-text"><strong>无工作区对话</strong><small>${projectless.length} 个对话</small></span><span class="row-action add-chat" data-new-projectless title="新建无工作区对话">＋</span></button><div class="workspace-conversations" ${state.projectlessExpanded ? "" : "hidden"}>${projectless.length ? projectless.map((conversation) => `<button class="conversation ${conversation.id === state.conversationId ? "active" : ""}" data-conversation="${conversation.id}"><span class="conversation-text"><span>${esc(conversation.title)}</span><small>${esc(conversationCaption(conversation))}</small></span><span class="delete-conversation" data-delete-conversation="${conversation.id}" role="button" aria-label="删除对话" title="删除这条对话记录">×</span></button>`).join("") : `<div class="empty-conversations">暂无对话</div>`}</div></section>`;
  tree.innerHTML = projectlessGroup + state.workspaces.map((workspace) => {
    const rows = visibleConversations.filter((item) => item.workspaceId === workspace.id);
    const expanded = state.expanded.has(workspace.id);
    return `<section class="workspace-group ${workspace.id === state.workspaceId ? "active" : ""}">
      <button class="workspace-row" data-workspace="${workspace.id}" title="${esc(workspace.path)}"><span class="chevron">${expanded ? "⌄" : "›"}</span><span class="folder">📁</span><span class="workspace-text"><strong>${esc(workspace.name)}</strong><small>${rows.length} 个对话</small></span><span class="row-action add-chat" data-new-chat="${workspace.id}" title="在此工作区新建对话">＋</span><span class="row-action delete-workspace" data-delete-workspace="${workspace.id}" role="button" aria-label="移除工作区" title="从平台移除此工作区">×</span></button>
      <div class="workspace-conversations" ${expanded ? "" : "hidden"}>${rows.length ? rows.map((conversation) => `<button class="conversation ${conversation.id === state.conversationId ? "active" : ""}" data-conversation="${conversation.id}"><span class="conversation-text"><span>${esc(conversation.title)}</span><small>${esc(conversationCaption(conversation))}</small></span><span class="delete-conversation" data-delete-conversation="${conversation.id}" role="button" aria-label="删除对话" title="删除这条对话记录">×</span></button>`).join("") : `<div class="empty-conversations">暂无对话</div>`}</div>
    </section>`;
  }).join("");
  const projectlessIcon = tree.querySelector("[data-projectless] .folder");
  if (projectlessIcon) { projectlessIcon.classList.add("projectless-cat"); projectlessIcon.innerHTML = catIcon("cat-sidebar"); }

  tree.querySelector("[data-projectless]").onclick = (event) => {
    if (event.target.closest("[data-new-projectless]")) return;
    state.projectlessExpanded = !state.projectlessExpanded;
    renderWorkspaceTree();
  };

  tree.querySelectorAll("[data-workspace]").forEach((button) => button.onclick = (event) => {
    if (event.target.closest("[data-new-chat], [data-delete-workspace]")) return;
    const id = button.dataset.workspace;
    if (state.expanded.has(id)) state.expanded.delete(id); else state.expanded.add(id);
    clearTimeout(state.poll);
    state.conversationLoadId += 1;
    persistWorkspace();
    state.workspaceId = id;
    state.conversationId = null;
    state.currentBoard = null;
    setResearchView("chat");
    renderAll();
  });
  tree.querySelectorAll("[data-new-chat]").forEach((button) => button.onclick = async (event) => { event.stopPropagation(); await createChat(button.dataset.newChat); });
  tree.querySelectorAll("[data-new-projectless]").forEach((button) => button.onclick = async (event) => { event.stopPropagation(); await createChat(null); });
  tree.querySelectorAll("[data-delete-workspace]").forEach((button) => button.onclick = async (event) => { event.stopPropagation(); try{ const workspace = state.workspaces.find((item) => item.id === button.dataset.deleteWorkspace); const count = state.conversations.filter((item) => item.workspaceId === workspace.id).length; if (!confirm(`确认从平台移除工作区“${workspace.name}”吗？\n\n将删除平台中的工作区条目和 ${count} 条对话记录。磁盘上的原始文件、论文和任务成果不会被删除。\n\n是否继续？`)) return; await api(`/api/workspaces/${workspace.id}`, { method: "DELETE" }); if (state.workspaceId === workspace.id) { clearTimeout(state.poll); state.conversationLoadId += 1; state.workspaceId = null; state.conversationId = null; state.currentBoard = null; setResearchView("chat"); } state.expanded.delete(workspace.id); await reloadLists(); }catch(error){alert("无法移除工作区："+error.message);} });
  tree.querySelectorAll("[data-conversation]").forEach((button) => button.onclick = async (event) => { if (event.target.closest("[data-delete-conversation]")) return; sidebarNavigation.close(); clearTimeout(state.poll); state.conversationLoadId += 1; const selectedId=button.dataset.conversation; persistWorkspace(); state.conversationId = selectedId; state.currentBoard = null; setResearchView("chat"); renderWorkspaceTree(); await renderConversation(); if(state.conversationId===selectedId&&state.currentBoard?.classicV2)setResearchView(workspaceMemory.view(selectedId).activeView||"board",{refresh:false});persistWorkspace(); });
  tree.querySelectorAll("[data-delete-conversation]").forEach((button) => button.onclick = async (event) => { event.stopPropagation(); try{ const conversation = state.conversations.find((item) => item.id === button.dataset.deleteConversation); const hasRecords = Number(conversation.messageCount) > 0 || isClassicResearch(conversation); const deletePrompt=isClassicResearch(conversation)?"确认移除此研究对话的索引吗？项目、研究记录与成果文件仍保留；活动研究或独立解释未结束时不能删除。":"确认删除对话“"+conversation.title+"”？平台对话记录将删除，工作区文件和成果保留。"; if(hasRecords&&!confirm(deletePrompt))return; await api(`/api/conversations/${conversation.id}`, { method: "DELETE" }); if (state.conversationId === conversation.id) { clearTimeout(state.poll); state.conversationLoadId += 1; state.conversationId = null; state.currentBoard = null; setResearchView("chat"); } await reloadLists(); }catch(error){alert("无法删除对话："+error.message);} });
}

function emptyBoardList(text) { return `<div class="board-empty-list">${esc(text)}</div>`; }
function boardProblem(board) { const statement=String(board.problem.statement||board.problem.goal||"").trim();const goal=String(board.problem.goal||"").trim();const original=String(board.problem.original||"").trim();const distinctGoal=goal&&goal.replace(/\s+/g," ")!==statement.replace(/\s+/g," ");const distinctContext=original&&original.replace(/\s+/g," ")!==statement.replace(/\s+/g," ");return `<section class="board-card wide"><h3><span>问题与约束</span><button type="button" data-edit-problem>编辑问题</button></h3><div class="problem-statement">${renderMessageText(statement,state.conversationId)}</div><div class="problem-meta">版本 v${board.problem.version}${distinctGoal?` · 规范目标：${renderMessageText(goal,state.conversationId)}`:""}</div>${distinctContext?`<details class="problem-context"><summary>查看完整工作区题目、已知结果与备注</summary><div>${renderMessageText(original,state.conversationId)}</div></details>`:""}</section>`; }
function decisionOptions(item) {
  if (Array.isArray(item.options) && item.options.length) return item.options.map((option, index) => ({ value: String(option.value || `option_${index + 1}`), label: String(option.label || option.value || `选项 ${index + 1}`), description: String(option.description || option.reason || "") }));
  return [{ value: "approve", label: "同意并继续", description: "按猫猫建议的方向继续研究" }, { value: "reject", label: "否决并换路", description: "停止这个方向，请智能体重新规划" }];
}
function decisionAnswerLabel(item) { const option=decisionOptions(item).find(choice=>choice.value===item.answer); return option?.label||({approve:"同意并继续",reject:"否决并换路"}[item.answer])||item.answer||"已回答"; }
function boardDecisions(board) {
  const pending=board.decisions.filter(item=>item.status==="pending");
  const answered=board.decisions.filter(item=>item.status==="answered").slice().reverse();
  const pendingCards=pending.map(item=>{const asker=({mathcat:"MathCat",rethlas:"Rethlas",danus:"Danus"})[item.source||board.agent]||"研究猫猫";return `<article class="decision-card cat-question-card" data-question-card="${esc(item.id)}"><div class="question-card-top"><span class="question-cat-avatar">${catIcon("cat-question")}</span><div class="question-card-heading"><span class="question-eyebrow">${esc(asker)} 想问你</span><strong>${esc(item.question)}</strong></div><span class="question-waiting">等你回答</span></div>${item.context?`<div class="question-context"><span>为什么需要你</span><p>${esc(item.context)}</p></div>`:""}<div class="question-options">${decisionOptions(item).map(option=>`<button type="button" class="question-option" data-decision="${esc(item.id)}" data-answer="${esc(option.value)}"><span>${esc(option.label)}</span>${option.description?`<small>${esc(option.description)}</small>`:""}</button>`).join("")}</div><label class="question-note"><span>补充给猫猫的话 <small>可选</small></span><textarea rows="2" data-decision-note="${esc(item.id)}" placeholder="例如：可以先加强假设，但要在结论中明确标注"></textarea></label><button type="button" class="question-defer" data-decision="${esc(item.id)}" data-answer="defer">我再想想，先不要阻塞其它路线</button></article>`;}).join("");
  const empty=`<div class="question-empty"><span class="question-empty-cat">${catIcon("cat-question")}</span><div><strong>猫猫暂时没有问题</strong><p>研究会继续进行；需要你的数学判断时，问题会出现在这里。</p></div></div>`;
  const history=answered.length?`<details class="question-history"><summary>查看已回答的问题 <span>${answered.length}</span></summary><div class="question-history-list">${answered.map(item=>`<div class="question-history-item"><div><strong>${esc(item.question)}</strong><small>${item.answeredAt?new Date(item.answeredAt).toLocaleString("zh-CN"):"已回答"}</small></div><span>${esc(decisionAnswerLabel(item))}</span>${item.note?`<p>${esc(item.note)}</p>`:""}</div>`).join("")}</div></details>`:"";
  return `<section class="board-card question-panel"><div class="question-panel-head"><div class="question-panel-title"><span class="question-panel-icon">${catIcon("cat-question")}</span><div><h3>猫猫提问卡</h3><p>需要你的经验、偏好或数学判断</p></div></div><span class="question-count ${pending.length?"has-pending":""}">${pending.length?`${pending.length} 个待回答`:"暂无待回答"}</span></div><div class="board-list question-list">${pending.length?pendingCards:empty}</div>${history}</section>`;
}
function boardIntegrationNotice(board) { return board.integration?.structured ? "" : `<section class="board-card wide truth-warning"><strong>结构化白板尚未接入</strong><p>${esc(board.integration?.message || "当前只保留对话执行结果，不会伪造智能体内部结构。")}</p></section>`; }
function researchActivityPanel(board) {
  const activity=deriveResearchActivity(board);
  const updated=activity.lastUpdatedAt?new Date(activity.lastUpdatedAt).toLocaleTimeString("zh-CN",{hour:"2-digit",minute:"2-digit",second:"2-digit"}):"等待首次更新";
  const metrics=[["执行任务",activity.activeTasks],["活动 Worker",activity.activeWorkers],["独立验证",activity.activeVerifications],["待你处理",activity.pendingRoutes+activity.pendingQuestions]];
  return `<section class="board-card wide research-live-card tone-${esc(activity.tone)}" aria-live="polite"><div class="research-live-head"><span class="research-live-signal ${activity.active?"is-active":""}" aria-hidden="true"><i></i></span><div><span>MathCat 当前在做什么</span><strong>${esc(activity.label)}</strong><p>${esc(activity.detail)}</p></div><em>第 ${esc(activity.currentRound||0)} 轮 · ${esc(updated)} 更新</em></div><ol class="research-stage-bar" aria-label="本轮研究阶段">${activity.stages.map((stage,index)=>`<li class="${esc(stage.status)}"><span>${stage.status==="complete"?"✓":index+1}</span><strong>${esc(stage.label)}</strong></li>`).join("")}</ol><div class="research-live-metrics">${metrics.map(([label,value])=>`<span><b>${esc(value)}</b>${esc(label)}</span>`).join("")}<small>这里只展示可审计的运行阶段、任务和验证状态，不包含模型私有思维链。</small></div></section>`;
}
function graphModel(board, view) { return view === "dependency" ? buildDependencyGraph(board) : buildProofTree(board); }
function graphActivityKey(board) { const latest=board?.events?.at?.(-1);return JSON.stringify([board?.revision,board?.eventCursor,latest?.id,board?.status,board?.routes?.map(item=>[item.id,item.status,item.humanStatus]),board?.tasks?.map(item=>[item.task_id||item.id,item.status,item.revision]),board?.workers?.map(item=>[item.worker_id||item.id,item.status,item.current_task_id]),board?.verificationQueue?.map(item=>[item.verification_id||item.id,item.status])]); }
function graphStatusLabel(status) { return GRAPH_STATUS_LABELS[status] || boardStatus(status); }
function graphLogicLabel(value) { return { OR:"替代路线", AND:"全部需要", PORTFOLIO:"并行方向" }[value] || value; }
function svgLabel(value) {
  const chars=[...String(value||"未命名节点")];
  const lines=[];
  while(chars.length&&lines.length<3)lines.push(chars.splice(0,18).join(""));
  if(chars.length&&lines.length)lines[lines.length-1]=`${lines[lines.length-1].slice(0,17)}…`;
  return lines.map((line,index)=>`<tspan x="16" dy="${index?17:0}">${esc(line)}</tspan>`).join("");
}
function currentGraphLayout(model) {
  const layout=overrideGraphPositions(layoutGraph(model),graphUi.positions[graphUi.view]);
  const connections=routeGraphEdges(model,layout,graphUi);
  return {...layout,width:connections.width,height:connections.height,connections};
}
function displayedGraph(model) {
  const visible=visibleGraph(model,{filter:graphUi.filter,query:graphUi.query,collapsed:graphUi.collapsed});
  return graphUi.focus?focusGraphBranch(visible,graphUi.selectedId):visible;
}
function renderGraphSvg(model,layout=currentGraphLayout(model)) {
  const focusIds=graphUi.focus&&graphUi.selectedId?relatedGraphIds(model,graphUi.selectedId):new Set();
  const marker=`<defs><marker id="graph-arrow" markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto"><path d="M0 0 8 4 0 8Z"></path></marker><marker id="graph-arrow-red" markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto"><path d="M0 0 8 4 0 8Z"></path></marker></defs>`;
  const edges=layout.connections.edges.map(item=>{
    const contradiction=item.relation==="contradicts";
    const dimmed=focusIds.size&&!(focusIds.has(item.from)&&focusIds.has(item.to));
    const label=GRAPH_RELATION_LABELS[item.relation]||item.relation;
    return `<g class="graph-edge ${contradiction?"contradiction":""} ${item.crossLink?"cross-reference":""} ${item.selected?"selected":""} ${dimmed?"dimmed":""}" data-graph-edge="${esc(item.id||`${item.from}:${item.relation}:${item.to}`)}"><title>${esc(`${label}${item.crossLink?" · 跨路线引用":""}`)}</title><path d="${item.path}" marker-end="url(#graph-arrow${contradiction?"-red":""})"></path>${item.showLabel?`<text x="${item.labelX}" y="${item.labelY}">${esc(label)}</text>`:""}</g>`;
  }).join("");
  const nodes=model.nodes.map(item=>{
    const position=layout.positions.get(item.id);if(!position)return"";
    const selected=item.id===graphUi.selectedId;const collapsed=graphUi.collapsed.has(item.id);const pinned=graphUi.positions[graphUi.view].has(item.id);const dimmed=focusIds.size&&!focusIds.has(item.id);
    const activity=item.meta?.activity;const activityClass=activity?`activity-${esc(activity.phase)} tone-${esc(activity.tone)} ${activity.active?"has-live-activity":""}`:"";
    const statusText=activity?.label||graphStatusLabel(item.status);const verified=item.trust==="fact"||item.trust==="verified";
    return `<g class="graph-node kind-${esc(item.kind)} status-${esc(item.status)} ${activityClass} ${selected?"selected":""} ${pinned?"pinned":""} ${dimmed?"dimmed":""}" transform="translate(${position.x} ${position.y})" data-graph-node="${esc(item.id)}" role="button" tabindex="0" aria-label="${esc(`${GRAPH_KIND_LABELS[item.kind]||item.kind}：${item.label}，${statusText}`)}"><rect width="${position.width}" height="${position.height}" rx="14"></rect>${activity?.active?`<circle class="graph-activity-ring" cx="17" cy="17" r="9"></circle>`:""}<circle class="graph-status-dot" cx="17" cy="17" r="5"></circle><text class="graph-kind" x="29" y="21">${esc(GRAPH_KIND_LABELS[item.kind]||item.kind)}</text>${item.logic?`<text class="graph-logic" x="${position.width-16}" y="21" text-anchor="end">${esc(graphLogicLabel(item.logic))}</text>`:""}<text class="graph-node-label" x="16" y="47">${svgLabel(item.shortLabel||item.label)}</text><text class="graph-node-status" x="16" y="${position.height-12}">${esc(shortText(`${statusText}${verified?" · 已核验":""}`,pinned?14:21))}</text>${pinned?`<text class="graph-pin" x="${position.width-16}" y="${position.height-13}" text-anchor="end">⌖ 已固定</text>`:""}${collapsed?`<g class="graph-collapsed"><circle cx="${position.width-17}" cy="${position.height-16}" r="9"></circle><text x="${position.width-17}" y="${position.height-13}" text-anchor="middle">＋</text></g>`:""}</g>`;
  }).join("");
  return `<svg class="research-graph-svg" viewBox="0 0 ${layout.width} ${layout.height}" width="${Math.round(layout.width*graphUi.scale)}" height="${Math.round(layout.height*graphUi.scale)}" aria-label="${model.kind==="proof"?"证明路线图":"依赖图"}">${marker}${edges}${nodes}</svg>`;
}
function bindGraphNodes(model,layout) {
  $("graphStage").querySelectorAll("[data-graph-node]").forEach(element=>{
    const id=element.dataset.graphNode;
    const original=layout.positions.get(id);
    element.onpointerdown=event=>{
      if(event.button!==0||!original)return;
      const startX=event.clientX,startY=event.clientY;
      let dragged=false,lastX=original.x,lastY=original.y;
      try { element.setPointerCapture?.(event.pointerId); } catch {}
      element.classList.add("dragging");
      const move=moveEvent=>{const dx=(moveEvent.clientX-startX)/graphUi.scale,dy=(moveEvent.clientY-startY)/graphUi.scale;if(Math.hypot(dx,dy)>4)dragged=true;if(!dragged)return;lastX=Math.max(18,original.x+dx);lastY=Math.max(18,original.y+dy);element.setAttribute("transform",`translate(${lastX} ${lastY})`);};
      const end=endEvent=>{element.removeEventListener("pointermove",move);element.removeEventListener("pointerup",end);element.removeEventListener("pointercancel",end);element.classList.remove("dragging");try{if(element.hasPointerCapture?.(endEvent.pointerId))element.releasePointerCapture(endEvent.pointerId);}catch{}const selectionChanged=graphUi.selectedId!==id;graphUi.selectedId=id;if(dragged){graphUi.positions[graphUi.view].set(id,{x:lastX,y:lastY});renderGraphDialog();}else if(selectionChanged){if(graphUi.focus)fitCurrentGraph();else renderGraphDialog();}};
      element.addEventListener("pointermove",move);element.addEventListener("pointerup",end);element.addEventListener("pointercancel",end);
    };
    element.ondblclick=event=>{event.preventDefault();graphUi.positions[graphUi.view].delete(id);graphUi.selectedId=id;renderGraphDialog();};
    element.onkeydown=event=>{if(event.key==="Enter"||event.key===" "){event.preventDefault();graphUi.selectedId=id;if(graphUi.focus)fitCurrentGraph();else renderGraphDialog();}};
  });
}
function renderGraphInspector(model) {
  const selected=model.nodes.find(item=>item.id===graphUi.selectedId);
  if(!selected)return `<div class="graph-inspector-empty">${catIcon(graphUi.view==="proof"?"cat-research":"cat-citations")}<strong>选择一个节点</strong><p>查看完整数学陈述、可信状态和连接关系。</p></div>`;
  const incoming=model.edges.filter(item=>item.to===selected.id);
  const outgoing=model.edges.filter(item=>item.from===selected.id);
  const canCollapse=model.kind==="proof"&&outgoing.length>0;
  const related=(edges,direction)=>edges.length?`<div class="graph-connection-list">${edges.map(edge=>{const other=model.nodes.find(item=>item.id===(direction==="in"?edge.from:edge.to));return other?`<button type="button" data-graph-jump="${esc(other.id)}"><span>${esc(GRAPH_RELATION_LABELS[edge.relation]||edge.relation)}</span><strong>${esc(other.shortLabel||other.label)}</strong></button>`:"";}).join("")}</div>`:`<p class="graph-no-connections">无</p>`;
  const trustLabel=selected.kind==="theorem"?"待解决的研究目标":selected.kind==="strategy"?(selected.meta?.historical?"历史路线均已归档":selected.meta?.planned?"已有具体路线":"尚未规划具体路线"):selected.kind==="route"?(selected.meta?.historical?"历史路线：已停止，成果归属保留":"研究方案，不是数学结论"):selected.trust==="fact"?"Fact Gate 已接受":selected.trust==="verified"?"已核验":"尚未成为 Fact";
  const activity=selected.meta?.activity;
  const activityTasks=activity?.tasks?.filter(item=>item.active)||[];
  const activityDetail=activity?.objective?`${activity.workerRole||"研究"}任务：${activity.objective}`:activityTasks.length?activityTasks.map(item=>`${item.workerRole}：${shortText(item.objective,90)}`).join("；"):"";
  const activityCard=activity?`<section class="graph-current-activity tone-${esc(activity.tone||"muted")}"><span><i class="${activity.active?"is-active":""}"></i>当前运行标志</span><strong>${esc(activity.label||"等待状态更新")}</strong>${activityDetail?`<p>${esc(activityDetail)}</p>`:""}<small>显示可审计阶段，不显示模型私有思维链。</small></section>`:"";
  const routeSummary=selected.kind==="route"?`<section class="graph-route-summary"><h4>研究者视角</h4>${selected.detail?`<p>${renderMessageText(selected.detail,state.conversationId)}</p>`:""}<dl>${selected.meta?.why?`<div><dt>为什么尝试</dt><dd>${renderMessageText(selected.meta.why,state.conversationId)}</dd></div>`:""}${selected.meta?.deliverable?`<div><dt>预期产出</dt><dd>${renderMessageText(selected.meta.deliverable,state.conversationId)}</dd></div>`:""}${selected.meta?.relation?`<div><dt>与总目标关系</dt><dd>${renderMessageText(selected.meta.relation,state.conversationId)}</dd></div>`:""}</dl>${selected.meta?.steps?.length?`<ol>${selected.meta.steps.map(step=>`<li>${renderMessageText(step,state.conversationId)}</li>`).join("")}</ol>`:""}</section>`:(selected.detail?`<div class="graph-inspector-detail">${renderMessageText(selected.detail,state.conversationId)}</div>`:"");
  const detailKind=["strategy","route"].includes(selected.kind)?(selected.kind==="strategy"?"研究方向说明":"具体路线内容"):"";
  const detailCollapsed=detailKind&&researchDisclosure.isCollapsed(graphUi.boardId,"node",selected.id);
  const detailId=`graph-node-detail-${Math.max(0,model.nodes.indexOf(selected))}`;
  const detailControl=detailKind&&routeSummary?`<button type="button" class="graph-detail-toggle" data-graph-detail-toggle="${esc(selected.id)}" aria-expanded="${String(!detailCollapsed)}" aria-controls="${detailId}"><span>${detailCollapsed?`展开${detailKind}`:`收起${detailKind}`}</span><i aria-hidden="true">⌄</i></button>`:"";
  const detailContent=routeSummary?`<div id="${detailId}" class="graph-node-detail-content" ${detailCollapsed?"hidden":""}>${routeSummary}</div>`:"";
  const incomingLabel=model.kind==="proof"?"上一级":"它依赖什么";
  const outgoingLabel=model.kind==="proof"?"下一步":"它通向哪里";
  const technical=[selected.meta?.technicalTitle&&selected.meta.technicalTitle!==selected.label?`内部标题：${selected.meta.technicalTitle}`:"",selected.meta?.rawSummary||""].filter(Boolean);
  return `<div class="graph-inspector-content"><span class="graph-inspector-kind">${esc(GRAPH_KIND_LABELS[selected.kind]||selected.kind)}</span><h3>${renderMessageText(selected.shortLabel||selected.label,state.conversationId)}</h3><div class="graph-inspector-badges"><span class="badge ${esc(selected.status)}">${esc(graphStatusLabel(selected.status))}</span>${selected.meta?.roleLabel?`<span class="route-role-badge ${esc(selected.meta.role||"")}">${esc(selected.meta.roleLabel)}</span>`:""}<span class="trust-badge ${selected.trust==="fact"||selected.trust==="verified"?"trusted":""}">${esc(trustLabel)}</span></div>${activityCard}${selected.meta?.historical?`<div class="graph-provenance-note">这条路线已归档；归档不等于其全部成果被否定。下方保留它产生的记录。</div>`:""}${selected.meta?.provenanceNote?`<div class="graph-provenance-note">${esc(selected.meta.provenanceNote)}</div>`:""}${detailControl}${detailContent}<section class="graph-connections"><h4>${incomingLabel}</h4>${related(incoming,"in")}<h4>${outgoingLabel}</h4>${related(outgoing,"out")}</section>${graphUi.positions[graphUi.view].has(selected.id)?`<button type="button" class="graph-collapse-button" data-reset-node="${esc(selected.id)}">恢复这个节点的自动位置</button>`:""}${canCollapse?`<button type="button" class="graph-collapse-button" data-collapse-node="${esc(selected.id)}">${graphUi.collapsed.has(selected.id)?"展开这个分支":"折叠这个分支"}</button>`:""}<details class="graph-technical"><summary>内部技术信息</summary>${technical.map(item=>`<p>${esc(item)}</p>`).join("")}<code>${esc(selected.id)}</code></details><div class="graph-trust-note">研究方向和路线表示“正在尝试什么”；只有通过独立验证和 Fact Gate 的结论才是可信 Fact。</div></div>`;
}
function renderGraphDialog() {
  const board=normalizeResearchBoard(state.currentBoard);
  if(!board)return;
  graphUi.activityKey=graphActivityKey(board);
  const activity=board.agent==="mathcat"&&!board.classicV2?deriveResearchActivity(board):null;
  const fullModel=graphModel(board,graphUi.view);
  const model=displayedGraph(fullModel);
  if(graphUi.selectedId&&!fullModel.nodes.some(item=>item.id===graphUi.selectedId))graphUi.selectedId=null;
  $("graphDialogTitle").textContent=graphUi.view==="proof"?"证明路线图":"依赖图";
  $("graphDialogSubtitle").textContent=graphUi.view==="proof"?"主目标 → 人类可读的研究方向 → 具体数学技巧与结果":"箭头从依据指向使用它的结论；虚线红边表示冲突";
  const liveStatus=$("graphLiveStatus");
  liveStatus.className=`graph-live-status tone-${activity?.tone||"muted"} ${activity?.active?"is-active":""}`;
  liveStatus.innerHTML=activity?`<i aria-hidden="true"></i><span>${esc(activity.label)}</span>`:"";
  const provenance={backend:"后端结构图",route_projection:"路线语义投影",derived:"兼容推导图"}[fullModel.source]||"结构投影";const warning=fullModel.contractWarnings?.length?` · 已记录 ${fullModel.contractWarnings.length} 项接口映射警告`:"";
  $("graphReadingGuide").innerHTML=(graphUi.view==="proof"?`<strong>怎么读</strong><span>从上到下：主目标 → 研究方向 → 具体路线 → 当前任务或验证。发光圆点表示此刻正在运行；灰色虚线方向表示尚未规划。</span>`:`<strong>怎么读</strong><span>从左到右：来源或产物 → 候选／Fact → 目标。点击查看详情，拖动可固定节点。</span>`)+`<em>${provenance}${warning}</em>`;
  graphDialog.querySelectorAll("[data-graph-view]").forEach(button=>{const active=button.dataset.graphView===graphUi.view;button.classList.toggle("active",active);button.setAttribute("aria-selected",String(active));});
  $("graphFilter").value=graphUi.filter;
  $("graphSearch").value=graphUi.query;
  $("graphZoomLabel").textContent=`${Math.round(graphUi.scale*100)}%`;
  const focusButton=graphDialog.querySelector('[data-graph-action="focus"]');focusButton.classList.toggle("active",graphUi.focus);focusButton.setAttribute("aria-pressed",String(graphUi.focus));focusButton.disabled=!graphUi.selectedId;focusButton.textContent=graphUi.focus?"退出聚焦":"聚焦分支";focusButton.title="选中一条路线后，仅显示它的上下游并自动放大";
  const layout=currentGraphLayout(model);
  graphReferencesButton.hidden=graphUi.view!=="proof";
  graphReferencesButton.disabled=!layout.connections.crossLinkCount;
  graphReferencesButton.textContent=`全部引用${layout.connections.crossLinkCount?` · ${layout.connections.crossLinkCount}`:""}`;
  graphReferencesButton.classList.toggle("active",graphUi.showCrossLinks);
  graphReferencesButton.setAttribute("aria-pressed",String(graphUi.showCrossLinks));
  if(graphUi.view==="proof"){
    const note=document.createElement("span");
    note.className="graph-reference-note";
    note.textContent=(graphUi.focus?`已聚焦 ${model.nodes.length}/${fullModel.nodes.length} 个节点。`:"选中路线后点“聚焦分支”可放大阅读。")+(layout.connections.hiddenCrossLinkCount?`${layout.connections.hiddenCrossLinkCount} 条跨路线引用暂时收起；点击节点或打开“全部引用”查看。冲突始终显示。`:"连线文字随节点选择显示，完整关系保留在右侧。");
    note.textContent += "“产出记录 → 送交验证 → 形成 Fact”表示成果沿革，不是证明依赖；已归档路线仍保留来源。";
    $("graphReadingGuide").append(note);
  }
  $("graphStage").innerHTML=model.nodes.length?renderGraphSvg(model,layout):`<div class="graph-no-results">没有符合当前筛选条件的节点</div>`;
  $("graphInspector").innerHTML=renderGraphInspector(fullModel);
  bindGraphNodes(model,layout);
  $("graphInspector").querySelector("[data-graph-detail-toggle]")?.addEventListener("click",event=>{researchDisclosure.toggle(graphUi.boardId,"node",event.currentTarget.dataset.graphDetailToggle);renderGraphDialog();});
  $("graphInspector").querySelector("[data-collapse-node]")?.addEventListener("click",event=>{const id=event.currentTarget.dataset.collapseNode;if(graphUi.collapsed.has(id))graphUi.collapsed.delete(id);else graphUi.collapsed.add(id);fitCurrentGraph();});
  $("graphInspector").querySelector("[data-reset-node]")?.addEventListener("click",event=>{graphUi.positions[graphUi.view].delete(event.currentTarget.dataset.resetNode);renderGraphDialog();});
  $("graphInspector").querySelectorAll("[data-graph-jump]").forEach(button=>button.onclick=()=>{graphUi.selectedId=button.dataset.graphJump;if(graphUi.focus)fitCurrentGraph();else renderGraphDialog();});
}
function fitCurrentGraph() { const board=normalizeResearchBoard(state.currentBoard);if(!board)return;const model=displayedGraph(graphModel(board,graphUi.view));const stage=$("graphStage");const layout=currentGraphLayout(model);graphUi.scale=fitGraphScale(layout,stage.clientWidth,stage.clientHeight,{min:0.1});renderGraphDialog();requestAnimationFrame(()=>{const root=layout.positions.get(model.rootId);stage.scrollLeft=graphUi.view==="proof"&&root?Math.max(0,(root.x+root.width/2)*graphUi.scale-stage.clientWidth/2):0;stage.scrollTop=0;}); }
function openResearchGraph(view) { const board=normalizeResearchBoard(state.currentBoard);if(!board)return;if(graphUi.boardId!==board.id){graphUi.boardId=board.id;graphUi.positions={proof:new Map(),dependency:new Map()};graphUi.collapsed=new Set();graphUi.activityKey="";}graphUi.view=view;graphUi.scale=1;graphUi.filter="all";graphUi.query="";graphUi.focus=false;graphUi.selectedId=graphModel(board,view).rootId;renderGraphDialog();graphDialog.showModal();requestAnimationFrame(fitCurrentGraph); }
function boardGraphCards(board) {
  const proofModel=buildProofTree(board);
  const proof=graphSummary(proofModel);
  const dependency=graphSummary(buildDependencyGraph(board));
  const directions=proofModel.nodes.filter(item=>item.kind==="strategy");
  const planned=directions.filter(item=>item.meta?.planned).length;
  return `<section class="board-card wide graph-overview"><div class="graph-overview-head"><div><h3>研究结构</h3><p>先看在尝试哪些方向，再检查结论依赖是否可信</p></div><span>实时投影</span></div><div class="graph-preview-grid"><button type="button" class="graph-preview-card proof" data-open-graph="proof"><span class="graph-preview-art" aria-hidden="true"><i></i><i></i><i></i><i></i></span><span class="graph-preview-copy"><strong>证明路线图</strong><small>${planned}/${directions.length||planned} 个方向已有路线 · ${proof.active} 个节点进行中</small><em>展开查看方向、技巧与子目标 →</em></span></button><button type="button" class="graph-preview-card dependency" data-open-graph="dependency"><span class="graph-preview-art" aria-hidden="true"><i></i><i></i><i></i><i></i></span><span class="graph-preview-copy"><strong>依赖图</strong><small>${dependency.verified} 个可信节点 · ${dependency.unverified} 个未验证依赖</small><em>展开检查事实、来源与冲突 →</em></span></button></div></section>`;
}
function readableResearchEvent(value) {
  const raw=String(value||"").trim();
  const labels={"planner call succeeded":"规划器运行成功","filled an available bottleneck route slot":"已补充一条瓶颈研究路线"};
  if(labels[raw])return {summary:labels[raw],raw:""};
  if(/Strategy:|Generator:|Reflection:|Supervisor:/.test(raw))return {summary:"智能体完成一轮策略生成、路线反思和监督筛选",raw};
  if(raw.length>180)return {summary:`${raw.slice(0,72)}…`,raw};
  return {summary:raw,raw:""};
}
function boardTimeline(board) { return `<section class="board-card wide"><h3>研究日志</h3><div class="timeline">${[...board.events].reverse().slice(0,12).map(item=>{const event=readableResearchEvent(item.text);return `<div class="timeline-item"><time>${new Date(item.at).toLocaleTimeString("zh-CN",{hour:"2-digit",minute:"2-digit"})}</time><i></i>${event.raw?`<details class="timeline-raw"><summary>${esc(event.summary)}</summary><p>${esc(event.raw)}</p></details>`:`<span>${esc(event.summary)}</span>`}</div>`;}).join("")}</div></section>`; }
function shortText(value,limit=220){const text=String(value||"").trim();return text.length>limit?`${text.slice(0,limit)}…`:text;}
function routeReadableParts(route) {
  const title=String(route.title||"未命名路线").trim();
  const steps=title.split(/[—–→]/).map(item=>item.trim()).filter(Boolean);
  const sentences=(String(route.summary||"").match(/[^。！？；]+[。！？；]?/g)||[]).map(item=>item.trim()).filter(Boolean);
  const pick=(pattern)=>sentences.find(item=>pattern.test(item));
  const method=pick(/方法是|通过|采用|依次|逐级/);
  const remaining=pick(/仍剩下|还需|尚需|不能被包装|不是主目标/);
  const guard=pick(/禁止|不得|不能把|不可把/);
  const next=pick(/^若|之后|后续/);
  return { title, steps, method, remaining, guard, next, raw:String(route.summary||"").trim() };
}
function humanizeRouteStep(value) {
  const text=String(value||"").trim();
  const replacements=[
    [/量词规格化/g,"明确量词与对象范围"],
    [/定义依赖闭包(?:恢复)?/g,"补齐命题所需定义"],
    [/可实例化性反压力审计/g,"检查能否用于具体对象，并寻找边界反例"],
    [/权威来源定位/g,"找到权威原文"],
    [/完整命题与定义闭包恢复/g,"还原完整命题和所需定义"]
  ];
  return replacements.reduce((result,[pattern,label])=>result.replace(pattern,label),text);
}
function routePlainExplanation(route) {
  return routePresentation(route,state.currentBoard||{});
}
function renderResearchRoute(route,index,boardId) {
  const info=routeReadableParts(route);
  const plain=routePlainExplanation(route);
  const activity=deriveResearchActivity(state.currentBoard||{}).routeById?.[route.id||route.route_id];
  const waiting=!route.isProposal&&["pending","proposed"].includes(route.humanStatus);
  const routeId=String(route.id||route.route_id||`route-${index}`);
  const collapsed=researchDisclosure.isCollapsed(boardId,"route",routeId);
  const detailId=`route-detail-${index}`;
  const routeSteps=plain.steps?.length?plain.steps:info.steps;
  const stepText=routeSteps.length>1?routeSteps.map((step,i)=>`<span><b>${i+1}</b>${renderMessageText(humanizeRouteStep(step),state.conversationId)}</span>`).join(`<i>→</i>`):`<span><b>1</b>${renderMessageText(humanizeRouteStep(routeSteps[0]||info.method?.replace(/^(它)?(方法是|通过|采用)/,"")||info.title),state.conversationId)}</span>`;
  const notes=[info.method&&!info.steps.some(step=>info.method.includes(step))?["怎么做",info.method]:null,info.remaining?["完成后还缺什么",info.remaining]:null,info.guard?["不能算成功",info.guard]:null,info.next?["下一步条件",info.next]:null].filter(Boolean);
  return `<article class="route-card readable-route ${waiting?"awaiting-review":""}" data-route-card="${esc(routeId)}"><div class="route-index"><span>路线 ${String(index+1).padStart(2,"0")}</span><span class="route-role-badge ${esc(plain.role)}">${esc(plain.roleLabel)}</span></div><div class="route-head"><strong>${renderMessageText(plain.title||info.title,state.conversationId)}</strong><div class="route-head-actions"><span class="badge ${waiting?"pending":esc(route.status)}">${waiting?"等待你的决定":boardStatus(route.status)}</span><button type="button" class="route-detail-toggle" data-route-detail-toggle="${esc(routeId)}" aria-expanded="${String(!collapsed)}" aria-controls="${detailId}"><span>${collapsed?"展开详情":"收起详情"}</span><i aria-hidden="true">⌄</i></button></div></div>${activity?`<div class="route-live-marker tone-${esc(activity.tone)} ${activity.active?"is-active":""}"><i aria-hidden="true"></i><strong>${esc(activity.label)}</strong>${activity.tasks.filter(item=>item.active).length?`<span>${activity.tasks.filter(item=>item.active).map(item=>`${item.workerRole}：${shortText(item.objective,52)}`).join("；")}</span>`:""}</div>`:""}<div id="${detailId}" class="route-card-body" ${collapsed?"hidden":""}><section class="route-plain"><span>这条路线具体在做什么</span><strong>${renderMessageText(plain.what,state.conversationId)}</strong><dl><div><dt>为什么现在做</dt><dd>${renderMessageText(plain.why,state.conversationId)}</dd></div><div><dt>完成时会得到</dt><dd>${renderMessageText(plain.deliverable,state.conversationId)}</dd></div><div><dt>离最终证明还有多远</dt><dd>${renderMessageText(plain.relation,state.conversationId)}</dd></div></dl></section><div class="route-purpose"><span>执行路径</span><div class="route-steps">${stepText}</div></div>${notes.length?`<details class="route-boundaries"><summary>查看成功标准与限制</summary><dl class="route-readable-notes">${notes.map(([label,value])=>`<div><dt>${esc(label)}</dt><dd>${renderMessageText(value,state.conversationId)}</dd></div>`).join("")}</dl></details>`:""}${info.raw?`<details class="route-raw"><summary>查看智能体原始技术说明</summary>${plain.technicalTitle&&plain.technicalTitle!==plain.title?`<strong>内部标题：${renderMessageText(plain.technicalTitle,state.conversationId)}</strong>`:""}<p>${renderMessageText(info.raw,state.conversationId)}</p></details>`:""}</div><div class="route-actions ${waiting?"decision-required":""}">${waiting?`<span>批准后将立即放行这条路线，不必等待其它路线</span><button class="route-approve" data-route="${route.id}" data-command="approve">批准并启动 Worker</button><button data-route="${route.id}" data-command="reject">否决路线</button>`:route.status==="active"?`<button data-route="${route.id}" data-command="pause">暂停</button>`:route.status==="paused"?`<button data-route="${route.id}" data-command="resume">恢复</button>`:""}</div></article>`;
}
function renderRoutePortfolio(board) {
  const groups=groupResearchRoutes(board.routes,board,{includeExpected:true});
  const hasMathematicalRoute=groups.some(group=>group.planned&&["direct_proof","counterexample","computation","reduction"].includes(group.kind)&&group.items.some(({presentation})=>presentation.role!=="prerequisite"));
  const context=board.contextSources?.length?`<span class="route-context-source">已读取 ${esc(board.contextSources.map(item=>item.name).join("、"))}</span>`:"";
  const warning=!hasMathematicalRoute&&board.routes.length?`<div class="route-portfolio-warning"><strong>当前还没有进入数学攻关</strong><span>现有路线都在澄清问题或查找资料；它们完成后仍需规划直接证明、反例或计算路线。</span></div>`:"";
  return `${context}${warning}<div class="route-family-list">${groups.map((group,groupIndex)=>{const collapsed=researchDisclosure.isCollapsed(board.id,"family",group.kind);const contentId=`route-family-content-${groupIndex}`;return `<section class="route-family ${group.planned?"planned":"unplanned"} ${collapsed?"is-collapsed":""}" data-route-family="${esc(group.kind)}"><header><button type="button" class="route-family-toggle" data-route-family-toggle="${esc(group.kind)}" aria-expanded="${String(!collapsed)}" aria-controls="${contentId}"><span class="route-family-heading"><strong>${esc(group.label)}</strong><em>${group.planned?`${group.items.length} 条具体路线`:"尚未规划"}</em></span><span class="route-disclosure-action">${collapsed?"展开方向":"收起方向"}<i aria-hidden="true">⌄</i></span></button></header><div id="${contentId}" class="route-family-content" ${collapsed?"hidden":""}><p class="route-family-description">${esc(group.description)}</p>${group.planned?`<div class="board-list">${group.items.map(({route})=>renderResearchRoute(route,board.routes.indexOf(route),board.id)).join("")}</div>`:`<div class="route-family-empty">MathCat 尚未为这个方向提出可执行方法。</div>`}</div></section>`;}).join("")}</div>`;
}
function goalReviewSnapshot(board) {
  const local=board.lastGoalReview;
  if(!local)return null;
  const commandId=String(local.commandId||"");
  const events=[...(board.events||[])].filter(item=>{const kind=String(item?.kind||item?.type||"").toLowerCase();const direct=/^goal_review[._]/.test(kind)&&(!commandId||String(item?.causedBy?.id||"")===commandId);if(direct)return true;const revision=Number(item.revision||0);if(revision&&local.afterRevision)return revision>local.afterRevision;const cursor=Number(item.cursor||0);if(cursor&&local.submittedEventCursor)return cursor>local.submittedEventCursor;const at=Date.parse(item.at||"");return !Number.isFinite(at)||at>=Date.parse(local.submittedAt||"");});
  const classify=item=>{const kind=String(item?.kind||item?.type||"").toLowerCase();if(/^goal_review[._]/.test(kind)){if(/completed|finished|committed|succeeded/.test(kind))return"completed";if(/failed|error|cancelled/.test(kind))return"failed";if(/started|running/.test(kind))return"running";return"queued";}if(/^(round\.(committed|completed)|plan_revision\.committed|planning\.(completed|succeeded))$/.test(kind))return"completed";if(/^(round|planning|project)\.(failed|error)$/.test(kind))return"failed";if(/^(round|planning)\.(started|running)$/.test(kind)||/^planning\.stage\..*\.started$/.test(kind))return"running";return null;};
  let status=local.status||"queued";let transition=null;
  for(const event of events){const next=classify(event);if(!next||status==="completed")continue;status=next;transition=event;}
  return {focus:String(events.find(item=>item?.data?.focus)?.data?.focus||local.focus||"重新检查研究目标与路线优先级"),status,at:transition?.at||local.submittedAt||null};
}
function planningSuggestionReceipts(board) {
  const suggestions=(board.planningSuggestions||[]).slice(-3).reverse();
  if(!suggestions.length)return"";
  const statusLabel={pending:"等待后续规划",validated:"已登记",applied:"已纳入规划",deferred:"暂缓采用",rejected:"未采用"};
  return `<div class="planning-suggestion-receipts"><div class="planning-suggestion-title"><strong>已提交的规划建议</strong><span>最近 ${suggestions.length} 条</span></div>${suggestions.map(item=>{const route=board.routes.find(candidate=>candidate.id===item.targetRouteId);const scope=route?`路线：${route.title||"未命名路线"}`:"整个研究规划";const round=Number(item.effectiveRound)>0?` · 最早第 ${esc(item.effectiveRound)} 轮生效`:"";return `<article><div><span>${esc(statusLabel[item.status]||"已提交")}</span><small>${esc(scope)}${round}</small></div><p>${renderMessageText(shortText(item.content,320),state.conversationId)}</p>${item.decision?`<small class="planning-suggestion-decision">Planner 记录：${esc(shortText(item.decision,220))}</small>`:""}</article>`;}).join("")}</div>`;
}
function humanCollaborationConsole(board) {
  const budget=board.budget;
  const budgetLabel=budget?`${esc(budget.maxParallelWorkers)} 个 Worker · 单任务 ${esc(budget.maxMinutesPerTask)} 分钟`:"研究预算等待同步";
  const review=goalReviewSnapshot(board);
  const reviewLabel={queued:"已提交，等待新一轮",running:"正在重新梳理",completed:"目标梳理已完成",failed:"目标梳理未完成"}[review?.status]||"已提交";
  return `<section class="board-card wide human-collaboration-console"><div class="collaboration-console-head"><div><span>Human in the loop</span><h3>人工协作台</h3><p>直接改变路线、规划重点和当前研究的运行方式。</p></div><div class="collaboration-settings-summary"><strong>${esc(boardReviewMode(board.reviewMode))}</strong><span>${budgetLabel}</span></div></div><div class="collaboration-action-grid"><button type="button" data-board-action="route"><i aria-hidden="true">↗</i><span><strong>编写并执行路线</strong><small>提交你的具体方法，检查后立即进入执行队列</small></span></button><button type="button" data-board-action="suggestion"><i aria-hidden="true">✎</i><span><strong>给规划建议</strong><small>例如要求更多采用交换代数视角</small></span></button><button type="button" data-board-action="goal-review"><i aria-hidden="true">◎</i><span><strong>重新梳理目标</strong><small>立即开启一次新目标梳理与规划讨论</small></span></button><button type="button" data-board-action="settings"><i aria-hidden="true">⚙</i><span><strong>当前研究设置</strong><small>并行数、运行时间和人工参与程度</small></span></button></div>${planningSuggestionReceipts(board)}${review?`<div class="goal-review-receipt ${esc(review.status)}"><span><i aria-hidden="true"></i><strong>${esc(reviewLabel)}</strong>${review.at?`<time>${esc(new Date(review.at).toLocaleString("zh-CN",{hour12:false}))}</time>`:""}</span><p>${esc(shortText(review.focus,360))}</p></div>`:""}</section>`;
}
function boardSummary(board){const summary=board.summary||{};const items=[["轮次",summary.current_round??0],["进行路线",summary.active_routes??board.routes.length],["开放目标",summary.open_goals??board.goals.length],["可信 Fact",summary.accepted_facts??board.claims.filter(item=>item.kind==="fact"||item.status==="accepted").length],["待验证",summary.pending_candidates??board.verificationQueue.filter(item=>item.status==="pending").length],["阻塞疑点",summary.blocking_uncertainties??board.uncertainties.filter(item=>item.status==="open").length]];return `<section class="board-summary wide" aria-label="研究状态摘要">${items.map(([label,value])=>`<div><strong>${esc(value)}</strong><span>${label}</span></div>`).join("")}</section>`;}
function visibleBoardGoals(board){const rootId=buildProofTree(board).rootId;return board.goals.filter(item=>String(item.id||item.goal_id)!==rootId);}
function visibleGoalClaimCount(board){return visibleBoardGoals(board).length+board.claims.filter(item=>item.kind!=="hypothesis").length;}
function goalClaimList(board){const goals=visibleBoardGoals(board).map(item=>`<article class="research-item goal-item"><div><span class="item-kind">子目标</span><span class="badge ${esc(item.status)}">${boardStatus(item.status)}</span></div><strong>${renderMessageText(shortText(item.statement,320),state.conversationId)}</strong>${item.solved_by_fact_id?`<small>由 Fact ${esc(item.solved_by_fact_id)} 解决</small>`:""}</article>`);const visibleClaims=board.claims.filter(item=>item.kind!=="hypothesis");const claims=visibleClaims.map(item=>`<article class="research-item claim-item ${item.kind==="fact"||item.status==="accepted"?"trusted":""}"><div><span class="item-kind">${esc(item.kind==="fact"?"Fact":"候选")}</span><span class="badge ${esc(item.status)}">${boardStatus(item.status)}</span></div><strong>${renderMessageText(shortText(item.statement,280),state.conversationId)}</strong>${item.verification_id?`<small>验证记录：${esc(item.verification_id)}</small>`:""}</article>`);const hidden=board.claims.length-visibleClaims.length;const note=hidden?`<p class="internal-constraint-note">${hidden} 条路线内部约束已整理到上方路线卡，不再作为研究结论重复展示。</p>`:"";return goals.length||claims.length?`<div class="research-item-list">${[...goals,...claims].join("")}${note}</div>`:note||emptyBoardList("智能体尚未提交子目标或候选结论");}
function failedRouteList(board){return board.failedRoutes.length?`<div class="research-item-list">${board.failedRoutes.map(item=>`<article class="research-item failed-item"><div><span class="item-kind">失败路线</span><span class="badge ${esc(item.status)}">${boardStatus(item.status)}</span></div><strong>${esc(item.title||"未命名路线")}</strong><p>${esc(shortText(item.summary||item.reason,260))}</p></article>`).join("")}</div>`:emptyBoardList("尚未记录失败路线");}
function executionPanel(board){const tasks=board.tasks.slice().reverse().slice(0,6);const workers=board.workers;return `<section class="board-card"><h3>任务与 Worker <span class="section-count">${board.tasks.length} / ${workers.length}</span></h3>${tasks.length?`<div class="research-item-list">${tasks.map(item=>`<article class="research-item"><div><span class="item-kind">${esc(item.worker_role||"task")}</span><span class="badge ${esc(item.status)}">${boardStatus(item.status)}</span></div><strong>${esc(shortText(item.objective,180))}</strong>${item.result_summary?`<p>${esc(shortText(item.result_summary,220))}</p>`:""}</article>`).join("")}</div>`:emptyBoardList("尚未派发研究任务")}${workers.length?`<div class="worker-strip">${workers.map(item=>`<span><i class="${esc(item.status)}"></i>${esc(item.role||"worker")} · ${esc(item.backend||"")} · ${boardStatus(item.status)}</span>`).join("")}</div>`:""}</section>`;}
function uncertaintyPanel(board){const open=board.uncertainties.filter(item=>item.status!=="resolved");return `<section class="board-card"><h3>开放疑点 <span class="section-count">${open.length}</span></h3>${open.length?`<div class="research-item-list">${open.slice(0,8).map(item=>`<article class="research-item uncertainty-item"><div><span class="item-kind">${esc(item.type||"疑点")}</span><span class="severity ${esc(item.severity||"medium")}">${esc(item.severity||"medium")}</span></div><strong>${esc(shortText(item.description,250))}</strong></article>`).join("")}</div>`:emptyBoardList("当前没有开放疑点")}</section>`;}
function verificationPanel(board){const queue=board.verificationQueue.slice().reverse();const accepted=queue.filter(item=>item.status==="accepted").length;const rejected=queue.filter(item=>item.status==="rejected").length;return `<section class="board-card wide"><h3>独立验证与研究产物 <span class="section-count">${accepted} 接受 · ${rejected} 拒绝 · ${board.artifacts.length} 个产物</span></h3>${queue.length?`<div class="verification-grid">${queue.slice(0,6).map(item=>`<details class="verification-item"><summary><span class="badge ${esc(item.status)}">${boardStatus(item.status)}</span><strong>${esc(item.candidate_id||item.verification_id)}</strong></summary><p>${esc(shortText(item.report?.summary,520))}</p>${item.report?.repair_actions?.length?`<ul>${item.report.repair_actions.slice(0,3).map(action=>`<li>${esc(shortText(action,180))}</li>`).join("")}</ul>`:""}</details>`).join("")}</div>`:emptyBoardList("当前没有验证记录")}${board.artifacts.length?`<div class="artifact-strip">${board.artifacts.slice(0,8).map(item=>`<span title="${esc(item.artifact_id||"")}">${esc(item.filename||item.kind||"研究产物")}</span>`).join("")}</div>`:""}</section>`;}
function entryValue(value){return Array.isArray(value?.entries)?Object.fromEntries(value.entries.map(item=>[item.name,item.value])):{};}
function experimentPanel(board){if(!board.experiments.length)return"";return `<section class="board-card wide computation-evidence"><h3>计算证据 <span class="section-count">${board.experiments.length} 次可重放实验 · 不等于一般证明</span></h3><div class="experiment-list">${board.experiments.map(item=>{const input=entryValue(item.input);const mapping=entryValue(item.conclusion_mapping);const artifacts=Array.isArray(item.artifacts)?item.artifacts:[];return `<article class="experiment-card"><div class="experiment-head"><strong>${esc(item.language||"计算实验")}</strong><span class="badge ${Number(item.exit_code)===0?"accepted":"rejected"}">exit ${esc(item.exit_code??"?")}</span></div><dl><div><dt>输入范围</dt><dd>${esc(input.lower_bound&&input.upper_bound?`${input.lower_bound}–${input.upper_bound}`:"见实验输入")}</dd></div><div><dt>重放命令</dt><dd><code>${esc((item.replay_command||[]).join(" ")||"未提供")}</code></dd></div><div><dt>逻辑边界</dt><dd>${esc(mapping.logical_limitation||"计算结果仅作为有限证据")}</dd></div></dl>${item.stdout?`<pre class="experiment-output">${esc(shortText(item.stdout,900))}</pre>`:""}${artifacts.length?`<div class="artifact-strip">${artifacts.map(file=>`<span title="SHA-256 ${esc(file.sha256||"")}">${esc(file.path||"实验产物")}</span>`).join("")}</div>`:""}${item.program_text?`<details class="experiment-code"><summary>查看可重放代码</summary><pre>${esc(item.program_text)}</pre></details>`:""}</article>`;}).join("")}</div></section>`;}
function renderMathCatBoard(board) { return `${researchActivityPanel(board)}${humanCollaborationConsole(board)}${boardSummary(board)}${boardProblem(board)}${boardGraphCards(board)}<section class="board-card route-panel"><h3><span>研究方向与具体路线</span><button type="button" data-add-route>＋ 编写并执行路线</button></h3><p class="route-panel-help">先看“直接证明、寻找反例、计算探索、归约推广”等研究方向，再展开其中的具体技巧。内部治理与审计文字默认收起。</p>${renderRoutePortfolio(board)}</section>${boardDecisions(board)}<section class="board-card wide"><h3>子目标与研究结论 <span class="section-count">${visibleGoalClaimCount(board)}</span></h3>${goalClaimList(board)}</section>${executionPanel(board)}${uncertaintyPanel(board)}${experimentPanel(board)}${verificationPanel(board)}<section class="board-card"><h3>失败路线 <span class="section-count">${board.failedRoutes.length}</span></h3>${failedRouteList(board)}</section>${boardTimeline(board)}`; }
function renderRethlasBoard(board) { return `${boardProblem(board)}${boardGraphCards(board)}<section class="board-card wide"><h3>生成—验证闭环</h3><div class="loop-flow"><div class="loop-node"><strong>生成 Agent</strong><small>证明蓝图</small></div><span>→</span><div class="loop-node"><strong>验证 Agent</strong><small>独立检查</small></div><span>→</span><div class="loop-node"><strong>${board.verification.verdict==="pending"?"等待运行":esc(board.verification.verdict)}</strong><small>修复或完成</small></div></div></section><section class="board-card"><h3>证明蓝图</h3>${board.blueprint.length?"":emptyBoardList("尚未生成 blueprint.md")}</section><section class="board-card"><h3>验证报告</h3><div class="status-row"><strong>当前判定</strong><span class="badge pending">${esc(board.verification.verdict)}</span></div>${emptyBoardList("等待 Rethlas 适配器同步验证报告")}</section><section class="board-card wide"><h3>迭代历史</h3>${board.iterations.length?"":emptyBoardList("尚无生成—验证迭代")}</section>${boardTimeline(board)}`; }
function renderDanusBoard(board) { return `${boardProblem(board)}${boardGraphCards(board)}<section class="board-card"><h3>战略与调度</h3><div class="status-row"><strong>Elaboration</strong><span>${esc(board.strategy.elaboration)}</span></div><div class="status-row"><strong>Master guidance</strong><span>${esc(board.strategy.masterGuidance)}</span></div></section><section class="board-card"><h3>Worker 状态</h3>${board.workers.length?"":emptyBoardList("尚未启动 Worker")}</section><section class="board-card wide"><h3>三层记忆与事实边界</h3><div class="truth-warning">Global Memory 只是共享发现；只有通过验证器写入 Fact Graph 的内容才是事实。</div><div class="memory-columns"><div class="memory-tier"><strong>Local Memory</strong><small>Worker 私有草稿 · ${board.memories.local.length} 条</small></div><div class="memory-tier"><strong>Global Memory</strong><small>共享发现 · ${board.memories.global.length} 条</small></div><div class="memory-tier"><strong>Fact Graph</strong><small>已验证事实 · ${board.facts.length} 条</small></div></div></section><section class="board-card"><h3>验证队列</h3>${board.verificationQueue.length?"":emptyBoardList("当前没有待验证候选")}</section>${boardDecisions(board)}${boardTimeline(board)}`; }
function renderClassicBoard(board){
  researchWhiteboard ??= new ResearchWhiteboard(boardElement, {
    api:whiteboardClient, classic:classicResearch,
    refresh:async()=>{await renderConversation();await reloadLists();},
    renderText:text=>renderMessageText(text,state.conversationId),
    openModelSettings:openCodexSettings,getCodexStatus:projectId=>codexStatus.getStatus(projectId)
  });
  researchWhiteboard.update(board.project,{id:board.conversationId,researchProjectId:board.project.id},state.boardError);
}

function renderResearchBoard() {
  const board=normalizeResearchBoard(state.currentBoard);
  if(board?.classicV2){renderClassicBoard(board);return;}researchWhiteboard?.dismiss();
  const offline=state.boardError?`<section class="board-card wide truth-warning board-offline"><strong>研究白板暂时无法连接</strong><p>${esc(state.boardError)}。对话内容仍可正常查看；恢复 MathCat 后刷新即可继续，已有白板快照不会被清空。</p></section>`:"";
  if(!board){boardElement.innerHTML=`${offline}<div class="board-empty">${catIcon("cat-research")}<div><strong>${state.boardError?"暂时无法读取研究白板":"研究白板将在提交研究问题后创建"}</strong><p>${state.boardError?"MathCat 恢复后会重新载入路线、任务与研究记录。":"问题、路线、人工决策与研究日志会保存在这里。"}</p></div></div>`;return;}
  state.currentBoard=board;
  boardElement.classList.remove("wb23");
  const body=`${offline}${boardIntegrationNotice(board)}${board.agent==="rethlas"?renderRethlasBoard(board):board.agent==="danus"?renderDanusBoard(board):renderMathCatBoard(board)}`;
  boardElement.innerHTML=`<div class="board-top"><div class="board-title"><h2>研究白板</h2><p>${esc(board.mode)} · Revision ${board.revision}</p></div><span class="board-run-status ${esc(board.status)}">${boardStatus(board.status)}</span>${board.agent==="mathcat"?`<span class="board-review-mode">${boardReviewMode(board.reviewMode)}</span>`:""}<span class="board-agent">${esc(board.agent==="mathcat"?"MathCat":board.agent==="rethlas"?"Rethlas":"Danus")}</span></div><div class="board-grid">${body}</div>`;
  const applyBoardResult = async (request,errorTarget=null) => { const conversationId=state.conversationId; try { const updated=normalizeResearchBoard(await request); if(state.conversationId!==conversationId||state.currentBoard?.id!==board.id){if(errorTarget)errorTarget.textContent="当前对话已经切换，请在目标白板中重新操作。";return false;} state.currentBoard=updated;state.boardError=updated?.syncWarning||null; if(state.activeView==="board")renderResearchBoard(); if(graphDialog.open&&graphUi.boardId===updated.id)renderGraphDialog(); return true; } catch(error) { if(state.conversationId===conversationId){const message=`白板操作失败：${readableBoardError(error)}`;state.boardError=readableBoardError(error);if(errorTarget)errorTarget.textContent=message;else alert(message);} return false; } };
  const submitDialog=async(dialog,requestFactory,onSuccess)=>{if(dialog.dataset.submitting==="true")return;const errorTarget=dialog.querySelector("[data-dialog-error]");errorTarget.textContent="";setBoardDialogBusy(dialog,true);try{const succeeded=await applyBoardResult(Promise.resolve().then(requestFactory),errorTarget);if(succeeded){onSuccess?.();dialog.close();}}finally{setBoardDialogBusy(dialog,false);}};
  const openRouteDialog=()=>{const form=$("humanRouteForm");form.reset();$("humanRouteGoal").innerHTML=`<option value="">主目标（自动关联）</option>${board.goals.map(goal=>`<option value="${esc(goal.id||goal.goal_id)}">${esc(shortText(goal.statement||goal.title||"未命名目标",120))}</option>`).join("")}`;openBoardDialog($("humanRouteDialog"),"#humanRouteTitle");};
  const openSuggestionDialog=()=>{const form=$("planningSuggestionForm");form.reset();$("planningSuggestionRoute").innerHTML=`<option value="">整个研究规划</option>${board.routes.filter(route=>!route.isProposal).map(route=>`<option value="${esc(route.id)}">路线：${esc(route.title||"未命名路线")}</option>`).join("")}`;openBoardDialog($("planningSuggestionDialog"),"#planningSuggestionContent");};
  const openGoalReviewDialog=()=>{const form=$("goalReviewForm");form.reset();openBoardDialog($("goalReviewDialog"),"#goalReviewFocus");};
  const reviewHelp={automatic:"路线和常规提问自动推进；安全故障仍会停止。",balanced:"路线自动推进，只在关键数学歧义或风险处询问。",strict:"每条新路线和关键数学判断都等待研究者确认。"};
  const syncBoardReviewHelp=()=>{$("boardReviewModeHelp").textContent=reviewHelp[$("boardReviewMode").value]||reviewHelp.strict;};
  const openSettingsDialog=()=>{const budget=board.budget;const fields=[[$("boardMaxParallelWorkers"),budget?.maxParallelWorkers],[$("boardMaxMinutesPerTask"),budget?.maxMinutesPerTask]];fields.forEach(([input,value])=>{if(Number.isFinite(value)){input.value=String(value);input.required=true;delete input.dataset.unavailable;}else{input.value="";input.required=false;input.dataset.unavailable="true";}});$("boardBudgetUnavailable").hidden=Boolean(budget);$("boardReviewMode").value=board.reviewMode||"strict";syncBoardReviewHelp();openBoardDialog($("boardSettingsDialog"),"#boardMaxParallelWorkers");if(!budget)$("boardReviewMode").focus();};
  boardElement.querySelectorAll("[data-open-graph]").forEach(button=>button.onclick=()=>openResearchGraph(button.dataset.openGraph));
  boardElement.querySelector("[data-edit-problem]").onclick=async()=>{const statement=prompt("修改数学问题（将生成新版本）",board.problem.statement);if(!statement||statement===board.problem.statement)return;await applyBoardResult(api(`/api/research-boards/${board.id}`,{method:"PATCH",body:JSON.stringify({statement,goal:statement})}));};
  boardElement.querySelectorAll("[data-add-route],[data-board-action='route']").forEach(button=>button.onclick=openRouteDialog);
  boardElement.querySelector("[data-board-action='suggestion']")?.addEventListener("click",openSuggestionDialog);
  boardElement.querySelector("[data-board-action='goal-review']")?.addEventListener("click",openGoalReviewDialog);
  boardElement.querySelector("[data-board-action='settings']")?.addEventListener("click",openSettingsDialog);
  $("humanRouteForm").onsubmit=event=>{event.preventDefault();const dialog=$("humanRouteDialog");const targetGoal=$("humanRouteGoal").value;void submitDialog(dialog,()=>api(`/api/research-boards/${board.id}/routes`,{method:"POST",body:JSON.stringify({title:$("humanRouteTitle").value.trim(),objective:$("humanRouteObjective").value.trim(),summary:$("humanRouteSummary").value.trim(),completionContract:$("humanRouteContract").value.trim(),targetGoalIds:targetGoal?[targetGoal]:[],knownRisks:$("humanRouteRisks").value.split(/\r?\n/).map(item=>item.trim()).filter(Boolean),executeImmediately:true})}));};
  $("planningSuggestionForm").onsubmit=event=>{event.preventDefault();const dialog=$("planningSuggestionDialog");void submitDialog(dialog,()=>api(`/api/research-boards/${board.id}/suggestions`,{method:"POST",body:JSON.stringify({content:$("planningSuggestionContent").value.trim(),targetRouteId:$("planningSuggestionRoute").value,reason:$("planningSuggestionReason").value.trim()})}));};
  $("goalReviewForm").onsubmit=event=>{event.preventDefault();const dialog=$("goalReviewDialog");void submitDialog(dialog,()=>api(`/api/research-boards/${board.id}/goal-review`,{method:"POST",body:JSON.stringify({focus:$("goalReviewFocus").value.trim()})}));};
  $("boardReviewMode").onchange=syncBoardReviewHelp;
  $("boardSettingsForm").onsubmit=event=>{event.preventDefault();const dialog=$("boardSettingsDialog");const payload={reviewMode:$("boardReviewMode").value};if($("boardMaxParallelWorkers").dataset.unavailable!=="true")payload.maxParallelWorkers=Number($("boardMaxParallelWorkers").value);if($("boardMaxMinutesPerTask").dataset.unavailable!=="true")payload.maxMinutesPerTask=Number($("boardMaxMinutesPerTask").value);void submitDialog(dialog,()=>api(`/api/research-boards/${board.id}/settings`,{method:"PATCH",body:JSON.stringify(payload)}),()=>{});};
  boardElement.querySelectorAll("[data-route-family-toggle]").forEach(button=>button.onclick=()=>{researchDisclosure.toggle(board.id,"family",button.dataset.routeFamilyToggle);renderResearchBoard();});
  boardElement.querySelectorAll("[data-route-detail-toggle]").forEach(button=>button.onclick=()=>{researchDisclosure.toggle(board.id,"route",button.dataset.routeDetailToggle);renderResearchBoard();});
  boardElement.querySelectorAll("[data-route]").forEach(button=>button.onclick=async()=>{await applyBoardResult(api(`/api/research-boards/${board.id}/routes/${button.dataset.route}/commands`,{method:"POST",body:JSON.stringify({command:button.dataset.command})}));});
  boardElement.querySelectorAll("[data-decision]").forEach(button=>button.onclick=async()=>{const card=button.closest("[data-question-card]");const note=card?.querySelector(`[data-decision-note="${button.dataset.decision}"]`)?.value||"";const controls=card?.querySelectorAll("button, textarea")||[];controls.forEach(control=>control.disabled=true);const succeeded=await applyBoardResult(api(`/api/research-boards/${board.id}/decisions/${button.dataset.decision}`,{method:"POST",body:JSON.stringify({answer:button.dataset.answer,note})}));if(!succeeded)controls.forEach(control=>control.disabled=false);});
}
graphDialog.querySelector("[data-graph-close]").onclick=()=>graphDialog.close();
graphDialog.addEventListener("click",event=>{if(event.target===graphDialog)graphDialog.close();});
graphDialog.querySelectorAll("[data-graph-view]").forEach(button=>button.onclick=()=>{graphUi.view=button.dataset.graphView;graphUi.selectedId=null;graphUi.focus=false;fitCurrentGraph();});
$("graphFilter").onchange=event=>{graphUi.filter=event.target.value;renderGraphDialog();};
$("graphSearch").oninput=event=>{graphUi.query=event.target.value;renderGraphDialog();$("graphSearch").focus();};
graphDialog.querySelectorAll("[data-graph-zoom]").forEach(button=>button.onclick=()=>{if(button.dataset.graphZoom==="in")graphUi.scale=Math.min(1.8,graphUi.scale+.15);else if(button.dataset.graphZoom==="out")graphUi.scale=Math.max(.1,graphUi.scale-.15);else graphUi.scale=1;renderGraphDialog();});
graphDialog.querySelectorAll("[data-graph-action]").forEach(button=>button.onclick=()=>{const action=button.dataset.graphAction;if(action==="focus"){graphUi.focus=!graphUi.focus;fitCurrentGraph();}else if(action==="references"){graphUi.showCrossLinks=!graphUi.showCrossLinks;renderGraphDialog();}else if(action==="arrange"){graphUi.positions[graphUi.view].clear();fitCurrentGraph();}else if(action==="fit")fitCurrentGraph();});
function setResearchView(requestedView,{refresh=true}={}) { const enabled=Boolean(state.currentBoard)||state.capabilityId==="rethlas-research"; const view=resolveResearchView(requestedView,enabled); state.activeView=view; const boardOpen=view==="board"; $("messages").hidden=boardOpen; if(!boardOpen)researchWhiteboard?.dismiss();boardElement.hidden=!boardOpen; document.querySelector(".composer").hidden=boardOpen; viewSwitch.querySelectorAll("button").forEach(button=>{const active=button.dataset.view===view;button.classList.toggle("active",active);button.setAttribute("aria-pressed",String(active));}); if(boardOpen){renderResearchBoard();if(refresh&&state.conversationId)void renderConversation().catch(error=>console.error("刷新研究白板失败",error));} }
function updateResearchViewVisibility(enabled) { viewSwitch.classList.toggle("visible",enabled); if(!enabled&&state.activeView==="board")setResearchView("chat"); }

function renderAll() {
  syncPromptDraft();
  updateSettingsSummary();
  renderWorkspaceTree();
  const workspace = state.workspaces.find((item) => item.id === state.workspaceId);
  $("workspaceName").textContent = workspace ? workspace.name : "无工作区对话";
  $("workspaceName").hidden = Boolean(state.conversationId);
  contextPicker.hidden = Boolean(state.conversationId);
  renderContextPicker();
  updateResearchViewVisibility(state.capabilityId === "rethlas-research" || Boolean(state.currentBoard));
  if (!state.conversationId) {
    $("title").textContent = "新对话";
    $("messages").innerHTML = `<div class="welcome"><div class="hero">${catIcon("cat-brand")}</div><h1>数学科研，从一个问题开始喵</h1><p>选择工作区，直接对话，或调用一个专业能力。</p></div>`;
  }
}

async function renderConversation() {
  const conversationId = state.conversationId;
  if (!conversationId) return;
  updateSettingsSummary();
  const loadId = ++state.conversationLoadId;
  const originalConversation = await api('/api/conversations/'+conversationId);
  if (!isCurrentConversationLoad(state.conversationId, conversationId, state.conversationLoadId, loadId)) return;
  state.codexConversationScope={conversationId,projectId:originalConversation.researchProjectId||null};
  updateSettingsSummary();
  let classic = null, classicError = null;
  if(isClassicResearch(originalConversation)){
    try{classic=await classicResearch.load(originalConversation);classic.board.project=await whiteboardClient.supplement(classic.board.project);classic.board.project.events=classic.board.events;}
    catch(error){classicError=error;const saved=state.classicCache?.get(conversationId);if(saved)classic=saved;}
    if(classic){state.classicCache??=new Map();state.classicCache.set(conversationId,classic);}
  }
  const shouldRefreshBoard = !state.currentBoard || state.activeView === "board";
  const boardRequest = isClassicResearch(originalConversation)?Promise.resolve({board:classic?.board||null,error:classicError}):shouldRefreshBoard
    ? api(`/api/research-boards/by-conversation/${conversationId}`).then((board) => ({ board, error: null })).catch((error) => error.status === 404 ? ({ board: null, error: null }) : ({ board: null, error }))
    : Promise.resolve({ board: state.currentBoard, error: null });
  const [conversation, boardResult] = await Promise.all([
    Promise.resolve(classic?.conversation||originalConversation),
    boardRequest
  ]);
  if (!isCurrentConversationLoad(state.conversationId, conversationId, state.conversationLoadId, loadId)) return;
  if (boardResult.error) state.boardError = readableBoardError(boardResult.error);
  else { state.currentBoard = normalizeResearchBoard(boardResult.board); state.boardError = boardResult.board?.syncWarning || null; }
  state.openedResearchProjects??=new Set();
  if(classic?.board&& !state.openedResearchProjects.has(classic.board.project.id)){state.openedResearchProjects.add(classic.board.project.id);setResearchView(workspaceMemory.view(conversationId).activeView||"board",{refresh:false});}
  updateSettingsSummary();
  updateResearchViewVisibility(Boolean(state.currentBoard) || conversation.messages.some((message) => message.capabilityId === "rethlas-research"));
  if (state.activeView === "board") renderResearchBoard();
  if (graphDialog.open && graphUi.boardId === state.currentBoard?.id && graphUi.activityKey !== graphActivityKey(state.currentBoard)) renderGraphDialog();
  state.workspaceId = conversation.workspaceId;
  syncPromptDraft();
  if (conversation.workspaceId != null) state.expanded.add(conversation.workspaceId);
  $("title").textContent = conversation.title;
  $("workspaceName").textContent = "";
  $("workspaceName").hidden = true;
  contextPicker.hidden = true;
  $("status").textContent = conversation.status === "running" ? "运行中" : conversation.status === "failed" ? "上次失败" : conversation.status === "cancelled" ? "已中止" : "就绪";
  if(isClassicResearch(originalConversation))$("status").textContent=classicError?"连接暂时中断":researchLabel(latestRun(classic?.board.project)?.state||"created");
  const messagesElement = $("messages");
  const wasNearBottom = messagesElement.scrollHeight - messagesElement.scrollTop - messagesElement.clientHeight < 90;
  const messagesKey = JSON.stringify(conversation.messages.map((message) => [message.id, message.content, message.artifacts, message.choice?.selectedIndex, message.activityDetails]));
  const messagesChanged = state.renderedConversationId !== conversation.id || state.renderedMessagesKey !== messagesKey;
  if (messagesChanged) {
    messagesElement.innerHTML = conversation.messages.map((message) => {
      const hasRecommendation = message.choice?.recommendedIndex != null && Boolean(message.choice.recommendationReason);
      const choice = message.choice ? `<section class="choice-card"><strong>${esc(message.choice.question)}</strong>${hasRecommendation ? `<p class="recommendation-reason"><b>推荐理由：</b>${esc(message.choice.recommendationReason)}</p>` : ""}<div class="choice-options">${message.choice.options.map((option, index) => `<button class="choice-option ${message.choice.selectedIndex === index ? "selected" : ""}" data-choice-message="${message.id}" data-choice-index="${index}" ${message.choice.selectedIndex != null ? "disabled" : ""}><span class="choice-main"><span>${esc(option.label)}</span>${hasRecommendation && message.choice.recommendedIndex === index ? `<em>推荐</em>` : ""}</span><small>${esc(option.reason)}</small><code>${esc(option.value)}</code></button>`).join("")}</div>${message.choice.selectedIndex != null ? `<p class="choice-selected">已选择：${esc(message.choice.options[message.choice.selectedIndex].label)}</p>` : ""}</section>` : "";
      const savedDetails = message.activityDetails || [];
      const stdoutLog = (message.artifacts || []).find((artifact) => /codex\.stdout\.log$/i.test(artifact));
      const legacyLog = stdoutLog ? `<a class="thinking-log-link" target="_blank" rel="noopener" href="/api/artifact?conversationId=${encodeURIComponent(conversation.id)}&path=${encodeURIComponent(stdoutLog)}">打开完整运行日志</a>` : "";
      const activityDetails = message.role === "assistant" && (savedDetails.length || stdoutLog) ? `<details class="thinking-details saved-thinking"><summary>查看本次思考/执行详情${savedDetails.length ? ` <span>${savedDetails.length} 条</span>` : ""}</summary><div class="thinking-note">以下是 Codex 提供的可见推理摘要与执行记录，不包含私有思维链。</div>${savedDetails.map((line) => `<div class="thinking-detail">${esc(line)}</div>`).join("")}${legacyLog}</details>` : "";
      return `<article class="message ${message.role}"><div class="meta">${message.role === "user" ? "你" : esc(message.executor || "智能体")} ${message.capabilityId ? `· ${esc(message.capabilityId)}` : ""}${message.researchAgent ? ` · ${esc(message.researchAgent)}` : ""}</div>${renderMessageText(message.content, conversation.id)}${choice}${activityDetails}${(message.artifacts || []).length ? `<div class="artifacts">${message.artifacts.map((artifact) => `<a target="_blank" rel="noopener" href="/api/artifact?conversationId=${encodeURIComponent(conversation.id)}&path=${encodeURIComponent(artifact)}">${esc(normalizeLocalPath(artifact).replaceAll("\\", "/"))}</a>`).join("")}</div>` : ""}</article>`;
    }).join("");
    state.renderedConversationId = conversation.id;
    state.renderedMessagesKey = messagesKey;
    messagesElement.querySelectorAll("[data-choice-message]").forEach((button) => button.onclick = async () => { const buttons = messagesElement.querySelectorAll("[data-choice-message]"); buttons.forEach((item) => item.disabled = true); try { await api(`/api/conversations/${conversation.id}/choices`, { method: "POST", body: JSON.stringify({ messageId: button.dataset.choiceMessage, optionIndex: Number(button.dataset.choiceIndex) }) }); await reloadLists(); await renderConversation(); } catch (error) { buttons.forEach((item) => item.disabled = false); alert(error.message); } });
    messagesElement.querySelectorAll("[data-reveal-path]").forEach((button) => button.onclick = () => revealFile(button, conversation.id));
  }
  let activityElement = $("activityBox");
  if (conversation.status === "running") {
    const activity = classic?.activity || await api(`/api/conversations/${conversation.id}/activity`).catch(() => ({ running: true, current: "任务正在启动…", lines: [], elapsedSeconds: 0 }));
    if (!isCurrentConversationLoad(state.conversationId, conversationId, state.conversationLoadId, loadId)) return;
    if (activity.running) {
      const thinkingOpen = isClassicResearch(originalConversation)||state.expandedThinking.has(conversation.id);
      const details = `<details class="thinking-details live-thinking" ${thinkingOpen ? "open" : ""}><summary>${thinkingOpen ? "收起" : "展开"}思考/执行详情 <span>${(activity.details || []).length} 条</span></summary><div class="thinking-note">以下是 Codex 提供的可见推理摘要与执行记录，不包含私有思维链。</div>${(activity.details || []).slice(-8).map((line) => `<div class="thinking-detail">${esc(line)}</div>`).join("") || `<div class="thinking-detail">等待详细事件…</div>`}</details>`;
      const body = `<div class="activity-head"><span class="activity-pulse"></span><strong>智能体正在工作</strong><span class="activity-elapsed">${Number(activity.elapsedSeconds || 0)} 秒</span><button type="button" class="cancel-task activity-cancel">■ 中止</button></div><div class="activity-current">${esc(activity.current || "正在处理任务")}</div>${(activity.lines || []).slice(-2).map((line) => `<div class="activity-line">${esc(line)}</div>`).join("")}${details}`;
      if (!activityElement) { messagesElement.insertAdjacentHTML("beforeend", `<aside id="activityBox" class="activity-box">${body}</aside>`); activityElement = $("activityBox"); }
      else if (activityElement.innerHTML !== body) activityElement.innerHTML = body;
      const cancelButton = activityElement.querySelector(".activity-cancel"); if (cancelButton) cancelButton.onclick = cancelCurrentTask;
      const liveThinking = activityElement.querySelector(".live-thinking"); if (liveThinking) liveThinking.ontoggle = () => { if (liveThinking.open) state.expandedThinking.add(conversation.id); else state.expandedThinking.delete(conversation.id); };
    }
  } else if (activityElement) {
    activityElement.remove();
  }
  if (messagesChanged && wasNearBottom) messagesElement.scrollTop = messagesElement.scrollHeight;
  const summary = state.conversations.find((item) => item.id === conversation.id);
  if (summary && (summary.status !== conversation.status || summary.messageCount !== conversation.messages.length)) { summary.status = conversation.status; summary.messageCount = conversation.messages.length; renderWorkspaceTree(); }
  if (conversation.status === "running" || (state.activeView === "board" && state.currentBoard) || (isClassicResearch(originalConversation) && (classicError || needsResearchPolling(classic?.board.project)))) {
    clearTimeout(state.poll);
    state.poll = setTimeout(() => { if (state.conversationId === conversationId) renderConversation(); }, 1500);
  }
}

async function reloadLists() {
  [state.workspaces, state.conversations, state.capabilities, state.researchAgents] = await Promise.all([api("/api/workspaces"), api("/api/conversations"), api("/api/capabilities"), api("/api/research-agents")]);
  state.capabilities.sort((a, b) => Number(a.order || 999) - Number(b.order || 999));
  $("capability").innerHTML = `<option value="">普通对话喵</option>` + state.capabilities.map((item) => `<option value="${item.id}">${esc(item.label || item.name)}</option>`).join("");
  if (state.capabilityId&&!state.capabilities.some((item) => item.id === state.capabilityId)) {state.capabilityId="";state.capabilityNeedsChoice=true;}
  $("capability").value = state.capabilityId;
  $("executor").value = state.executor;
  $("permission").value = state.permission;
  updateSettingsSummary();
  renderCapabilityPicker();
  renderResearchAgentPicker();
  renderAll();
}
function updateSettingsSummary(){
  codexStatus.setContext(currentCodexProject());
}

function renderCapabilityPicker() {
  const options = [{ id: "", label: "普通对话喵", description: "直接与 Codex 对话", icon: "cat" }, ...state.capabilities];
  const selected = options.find((item) => item.id === state.capabilityId) || options[0];
  $("capabilityIcon").innerHTML = catIcon(CAPABILITY_ICONS[selected.id] || selected.icon);
  $("capabilityLabel").textContent = selected.label || selected.name;
  $("hint").textContent = state.capabilityNeedsChoice?"原能力已移除，请重新选择后发送":selected.label || selected.name;
  $("capabilityMenu").innerHTML = options.map((item) => `<button type="button" class="capability-option ${item.id === state.capabilityId ? "selected" : ""}" role="option" aria-selected="${item.id === state.capabilityId}" data-capability-id="${esc(item.id)}"><span class="capability-option-icon">${catIcon(CAPABILITY_ICONS[item.id] || item.icon)}</span><span class="capability-option-copy"><strong>${esc(item.label || item.name)}</strong><small>${esc(item.description || "")}</small></span><span class="capability-check">${item.id === state.capabilityId ? "✓" : ""}</span></button>`).join("");
  $("capabilityMenu").querySelectorAll("[data-capability-id]").forEach((button) => button.onclick = () => {
    state.capabilityId = button.dataset.capabilityId;
    state.capabilityNeedsChoice=false;persistWorkspace();
    $("capability").value = state.capabilityId;
    closeCapabilityMenu();
    renderCapabilityPicker();
    renderResearchAgentPicker();
  });
}

function renderResearchAgentPicker() {
  const enabled = state.capabilityId === "rethlas-research";
  researchPicker.hidden = !enabled;
  updateResearchViewVisibility(enabled || Boolean(state.currentBoard));
  if (!enabled) { closeResearchAgentMenu(); return; }
  const selected = state.researchAgents.find((item) => item.id === state.researchAgentId) || state.researchAgents[0];
  if (!selected) return;
  state.researchAgentId = selected.id;
  $("researchAgentIcon").innerHTML = catIcon(selected.id === "mathcat" ? "cat-brand" : selected.id === "rethlas" ? "cat-research" : "cat-autonomous");
  $("researchAgentLabel").textContent = selected.name;
  $("hint").textContent = `研究喵 · ${selected.name}`;
  $("researchAgentMenu").innerHTML = state.researchAgents.map((item) => `<button type="button" class="research-agent-option ${item.id === state.researchAgentId ? "selected" : ""}" role="option" aria-selected="${item.id === state.researchAgentId}" data-research-agent="${esc(item.id)}"><span>${catIcon(item.id === "mathcat" ? "cat-brand" : item.id === "rethlas" ? "cat-research" : "cat-autonomous")}</span><span><strong>${esc(item.name)}</strong><small>${esc(item.description)}</small></span><em>${item.configured ? (item.id === state.researchAgentId ? "✓" : "已配置") : "首次配置"}</em></button>`).join("");
  $("researchAgentMenu").querySelectorAll("[data-research-agent]").forEach((button) => button.onclick = async () => {
    const item = state.researchAgents.find((row) => row.id === button.dataset.researchAgent);
    state.researchAgentId = item.id;
    closeResearchAgentMenu();
    if (!item.configured && item.id !== "mathcat") {
      button.disabled = true;
      try {
        const result = await api(`/api/research-agents/${item.id}/configure`, { method: "POST", body: "{}" });
        clearTimeout(state.poll);
        state.conversationLoadId += 1;
        state.workspaceId = null;
        state.conversationId = result.conversationId;
        state.currentBoard = null;
        setResearchView("chat");
        state.renderedConversationId = null;
        await reloadLists();
        await renderConversation();
      } catch (error) { alert(`无法配置 ${item.name}：${error.message}`); }
      return;
    }
    renderResearchAgentPicker();
    $("hint").textContent = `研究喵 · ${item.name}`;
  });
}

function closeCapabilityMenu() { $("capabilityMenu").hidden = true; $("capabilityTrigger").setAttribute("aria-expanded", "false"); }
function closeResearchAgentMenu() { $("researchAgentMenu").hidden = true; $("researchAgentTrigger").setAttribute("aria-expanded", "false"); }
function closeContextMenu() { $("contextMenu").hidden = true; $("contextTrigger").setAttribute("aria-expanded", "false"); }

async function createChat(workspaceId = null) {
  persistWorkspace();
  sidebarNavigation.close();
  state.conversationLoadId += 1;
  state.workspaceId = workspaceId;
  if (workspaceId != null) state.expanded.add(workspaceId);
  else state.projectlessExpanded = true;
  state.conversationId = null;
  state.currentBoard = null;
  state.activeView = "chat";
  state.renderedConversationId = null;
  state.renderedMessagesKey = null;
  clearTimeout(state.poll);
  renderAll();
  $("prompt").focus();
}

async function refreshHealth() {
  const health = await api("/api/health");
  $("health").textContent = `Codex ${health.codexConfigured ? "已配置" : "未配置"} · ${health.capabilities} 个能力`;
}

$("addWorkspace").onclick = () => $("workspaceDialog").showModal();
$("settingsButton").onclick = () => openCodexSettings();
$("settingsDialog").addEventListener('close',()=>codexStatus.resetDraft());
$("browse").onclick = async () => {
  const button = $("browse"); const original = button.textContent; button.disabled = true; button.textContent = "正在打开资源管理器…"; $("dialogError").textContent = "";
  try {
    const result = await api("/api/folder-picker", { method: "POST", body: "{}" });
    if (result.path) { $("workspacePath").value = result.path; if (!$("workspaceLabel").value) $("workspaceLabel").value = result.path.replace(/[\\/]$/, "").split(/[\\/]/).pop() || result.path; }
  } catch (error) { $("dialogError").textContent = `无法打开 Windows 文件夹选择窗口：${error.message}`; }
  finally { button.disabled = false; button.textContent = original; }
};
$("saveWorkspace").onclick = async () => { try { const workspace = await api("/api/workspaces", { method: "POST", body: JSON.stringify({ workspacePath: $("workspacePath").value, name: $("workspaceLabel").value }) }); state.workspaceId = workspace.id; state.expanded.add(workspace.id); $("workspaceDialog").close(); await reloadLists(); } catch (error) { $("dialogError").textContent = error.message; } };
$("newChat").onclick = () => createChat(null);
$("refresh").onclick = reloadLists;
$("send").onclick = async () => {
  const text=$("prompt").value.trim();if(!text||$("send").disabled)return;if(state.capabilityNeedsChoice){alert("之前选择的能力已移除，请重新选择数学研究或普通对话后再发送。");return;}const targetBefore=state.conversationId,workspaceBefore=state.workspaceId;const checkSendTarget=()=>{if(state.conversationId!==targetBefore||state.workspaceId!==workspaceBefore)throw new Error("当前对话已切换，未向另一项目提交内容；请回到原对话继续。");};$("send").disabled=true;
  try{
    const existing=targetBefore?await api('/api/conversations/'+targetBefore):null;checkSendTarget();
    if(isClassicResearch(existing)){
      const loaded=await classicResearch.load(existing);
      if(explicitProblemEdit(text)){
        const spec=loaded.board.project.problem_spec;
        if(!spec)throw new Error('请在新版白板刷新题面后再修改。');
        await classicResearch.editProblem(existing,text,spec.revision);
      }else if(!existing.researchRunId&&existing.researchStartKey&&!loaded.board.project.runs.length){
        if(await classicResearch.start(existing)===false)return;
      }else{
        const sent=await classicResearch.discuss(existing,text,{authorize:()=>confirm('研究已暂停或结束。本次提问将单独调用只读模型解释，最长 5 分钟，独立计量，不恢复原研究。继续？')});
        if(sent===false)return;
      }
    }else{
      const research=state.capabilityId==='rethlas-research'&&state.researchAgentId==='mathcat';
      if(research&&state.permission==='read-only')throw new Error('研究需要保存独立项目文件，请将工作权限切换为可覆写。');
      const startOptions=research?await chooseStartWithModel():null;
      if(research&&!startOptions)return;checkSendTarget();
      if(!state.conversationId){const created=await api('/api/conversations',{method:'POST',body:JSON.stringify({workspaceId:state.workspaceId,title:text.slice(0,40)})});checkSendTarget();state.conversationId=created.id;draftContext='conversation:'+created.id;workspaceMemory.saveDraft(draftContext,text);persistWorkspace();}
      await api('/api/conversations/'+state.conversationId+'/messages',{method:'POST',body:JSON.stringify({text,executor:state.executor,capabilityId:state.capabilityId||null,researchAgent:state.capabilityId==='rethlas-research'?state.researchAgentId:null,permission:state.permission,...(startOptions?{mathcatReviewMode:startOptions.mode==='collaborative'?'strict':'automatic',durationSeconds:startOptions.durationSeconds,maxPartners:startOptions.maxPartners,autoDeliver:startOptions.autoDeliver}:{})})});
    }
    if((!targetBefore||state.conversationId===targetBefore)&&$("prompt").value.trim()===text)$("prompt").value='';
    persistWorkspace();await reloadLists();await renderConversation();
  }catch(error){await reloadLists().catch(()=>{});if(state.conversationId)await renderConversation().catch(()=>{});alert('无法发送：'+error.message+'\n输入和已创建的研究记录已保留，请确认状态后重试。');}
  finally{$("send").disabled=false;}
};
$("prompt").oninput=persistWorkspace;
$("prompt").onkeydown = (event) => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); $("send").click(); } };
$("capabilityTrigger").onclick = (event) => { event.stopPropagation(); const opening = $("capabilityMenu").hidden; $("capabilityMenu").hidden = !opening; $("capabilityTrigger").setAttribute("aria-expanded", String(opening)); };
$("researchAgentTrigger").onclick = (event) => { event.stopPropagation(); const opening = $("researchAgentMenu").hidden; closeCapabilityMenu(); $("researchAgentMenu").hidden = !opening; $("researchAgentTrigger").setAttribute("aria-expanded", String(opening)); };
viewSwitch.querySelectorAll("[data-view]").forEach((button) => button.onclick = () => setResearchView(button.dataset.view));
$("contextTrigger").onclick = (event) => { event.stopPropagation(); const opening = $("contextMenu").hidden; closeCapabilityMenu(); $("contextMenu").hidden = !opening; $("contextTrigger").setAttribute("aria-expanded", String(opening)); };
document.addEventListener("click", (event) => { if (!event.target.closest(".capability-picker")) closeCapabilityMenu(); if (!event.target.closest(".research-picker")) closeResearchAgentMenu(); if (!event.target.closest(".context-picker")) closeContextMenu(); });
$("permission").onchange = () => { state.permission = $("permission").value; localStorage.setItem("mathcat.permission", state.permission); updateSettingsSummary(); };
$("executor").onchange = () => { state.executor = $("executor").value; };
async function cancelCurrentTask(event) { const button=event?.currentTarget;const conversationId=state.conversationId;if(!conversationId||!confirm("确定中止当前任务吗？已生成的工作区文件会保留。"))return;if(button)button.disabled=true;try{const project=classicResearch.projects.get(conversationId);const interactions=project?.interactions?.filter(i=>i.state!=='ended')||[];if(interactions.length){for(const row of interactions)await classicResearch.cancelInteraction(project,row.id);}else await api('/api/conversations/'+conversationId+'/cancel',{method:'POST',body:'{}'});if(state.conversationId===conversationId)await renderConversation();}catch(error){alert(error.message);}finally{if(button)button.disabled=false;} }

try { await refreshHealth(); await reloadLists();
  const reopened=state.conversations.find(c=>c.id===savedWorkspace.conversationId);
  if(reopened){state.conversationId=reopened.id;state.workspaceId=reopened.workspaceId;syncPromptDraft();await renderConversation();const savedView=workspaceMemory.view(reopened.id);if(savedView.activeView)setResearchView(savedView.activeView,{refresh:false});requestAnimationFrame(()=>{$('messages').scrollTop=savedView.chatScroll||0;});}
  persistWorkspace();
} catch (error) { $("health").textContent = `平台连接失败：${error.message}`; }
