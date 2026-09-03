import { resolveResearchView, isCurrentConversationLoad, normalizeResearchBoard } from "./view-state.js";
import { renderLatexText } from "./math-renderer.js";
import { buildProofTree, buildDependencyGraph, graphSummary, visibleGraph, layoutGraph, GRAPH_STATUS_LABELS, GRAPH_KIND_LABELS, GRAPH_RELATION_LABELS } from "./research-graph.js";
import katex from "./vendor/katex/katex.esm.js";

const state = { workspaces: [], conversations: [], capabilities: [], researchAgents: [], workspaceId: null, conversationId: null, conversationLoadId: 0, executor: "codex", capabilityId: "", researchAgentId: "mathcat", permission: "workspace-write", activeView: "chat", currentBoard: null, projectlessExpanded: true, expanded: new Set(), expandedThinking: new Set(), poll: null, renderedConversationId: null, renderedMessagesKey: null };
try { const savedPermission = localStorage.getItem("mathcat.permission"); if (["workspace-write", "read-only"].includes(savedPermission)) state.permission = savedPermission; } catch {}
const $ = (id) => document.getElementById(id);
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
graphDialog.innerHTML = `<div class="graph-dialog-shell"><header class="graph-dialog-head"><div><span class="graph-dialog-kicker">研究结构</span><h2 id="graphDialogTitle">证明树</h2><p id="graphDialogSubtitle">查看主目标、证明义务与研究路线</p></div><button type="button" class="graph-dialog-close" data-graph-close aria-label="关闭图谱">×</button></header><div class="graph-toolbar"><div class="graph-tabs" role="tablist"><button type="button" role="tab" data-graph-view="proof">证明树</button><button type="button" role="tab" data-graph-view="dependency">依赖图</button></div><label class="graph-search"><span aria-hidden="true">⌕</span><input id="graphSearch" type="search" placeholder="搜索节点或数学陈述"></label><select id="graphFilter" aria-label="筛选图谱节点"><option value="all">全部节点</option><option value="active">进行中与阻塞</option><option value="unverified">未验证依赖</option><option value="problems">阻塞、失败与反例</option></select><div class="graph-zoom" aria-label="缩放图谱"><button type="button" data-graph-zoom="out" aria-label="缩小">−</button><button type="button" data-graph-zoom="reset" id="graphZoomLabel">100%</button><button type="button" data-graph-zoom="in" aria-label="放大">＋</button></div></div><div class="graph-workspace"><div id="graphStage" class="graph-stage" tabindex="0"></div><aside id="graphInspector" class="graph-inspector"></aside></div><footer class="graph-legend"><span><i class="legend-shape theorem"></i>目标</span><span><i class="legend-shape route"></i>路线/引理</span><span><i class="legend-shape candidate"></i>候选</span><span><i class="legend-shape fact"></i>已验证 Fact</span><span><i class="legend-line contradiction"></i>冲突</span><small>进度状态与数学可信度分开显示</small></footer></div>`;
document.body.append(graphDialog);
const graphUi = { view: "proof", scale: 1, filter: "all", query: "", selectedId: null, collapsed: new Set() };
const ICON_PATHS = {
  "cat-chat": ["M5 9 4.5 4.5 8.5 6.7a8.5 8.5 0 0 1 5 0l4-2.2L17 9c.8 1 1.2 2.2 1.2 3.5 0 3.5-2.9 5.7-6.2 5.7s-6.2-2.2-6.2-5.7C5.8 11.2 5.5 10 5 9Z", "M8.5 12h.1m6.8 0h.1M9.5 15c1.5 1 3.5 1 5 0", "M17.5 15.5h3v3l-1.2-1h-1.8Z"],
  "cat-library": ["M5 9 4.5 4.5 8.4 6.6a9 9 0 0 1 5.2 0l3.9-2.1L17 9c.8 1 1.2 2.2 1.2 3.4 0 3.3-2.8 5.3-6.2 5.3s-6.2-2-6.2-5.3C5.8 11.2 5.5 10 5 9Z", "M7.7 11.8h3v2h-3Zm5.6 0h3v2h-3Zm-2.6 1h2.6M10 15.3c1.3.7 2.7.7 4 0", "M4 19h7l1 1 1-1h7v3h-7l-1 1-1-1H4Z"],
  "cat-research": ["M4.8 9.5 4.3 5 8.2 7.1a8.5 8.5 0 0 1 5 0L17.2 5l-.6 4.4a6.3 6.3 0 0 1 .9 3.2c0 3.4-2.9 5.5-6.5 5.5s-6.5-2.1-6.5-5.5c0-1.1.1-2.1.3-3.1Z", "M7.7 12h.1m5.7 0h.1M9 15c1.2.8 2.8.8 4 0", "M15.2 16.2a3 3 0 1 0 4.2 4.2 3 3 0 0 0-4.2-4.2Zm4.1 4.1 2.2 2.2"],
  "cat-question": ["M5.1 9.4 4.7 5.1 8.5 7a8 8 0 0 1 4.8-.1l3.9-2.3-.5 4.7a6 6 0 0 1 .9 3.1c0 3.2-2.7 5.2-6 5.2s-6-2-6-5.2c0-1.1.2-2.1-.5-3Z", "M8.4 12h.1m6.1 0h.1M9.7 15c1.2.7 2.6.7 3.8 0", "M17.2 7.8h3.3c1 0 1.8.8 1.8 1.8v3.1c0 1-.8 1.8-1.8 1.8h-.8l-1.7 1.7v-1.7h-.8M19.7 10.2h.1m-.1 2h.1"],
  "cat-citations": ["M5.2 9 4.7 4.5 8.6 6.7a8.5 8.5 0 0 1 5 0l3.8-2.2L17 9c.7.9 1 2 1 3.2 0 3.1-2.7 5.1-6 5.1s-6-2-6-5.1c0-1.2.4-2.3 1.2-3.2Z", "M8.2 12h.1m7 0h.1M10 14.8c1.3.8 2.7.8 4 0", "M3 18.5h6v4H3Zm12 0h6v4h-6M7 18.5l2-2m6 2-2-2M9 20.5h6"],
  "cat-writer": ["M5 9 4.5 4.5 8.5 6.7a8.5 8.5 0 0 1 5 0l4-2.2L17 9c.8 1 1.2 2.2 1.2 3.4 0 3.3-2.9 5.4-6.2 5.4s-6.2-2.1-6.2-5.4C5.8 11.2 5.5 10 5 9Z", "M8.5 12h.1m6.8 0h.1M9.8 15c1.4.8 3 .8 4.4 0", "M4 19.5h9M15 21l5.8-5.8-2-2L13 19l-.5 2.5Z"],
  "cat-revision": ["M5 9 4.5 4.5 8.4 6.7a8.5 8.5 0 0 1 5.2 0l3.9-2.2L17 9c.8 1 1.2 2.2 1.2 3.4 0 3.3-2.8 5.4-6.2 5.4s-6.2-2.1-6.2-5.4C5.8 11.2 5.5 10 5 9Z", "M8.2 12h.1m7.4 0h.1M9.8 15.2c1.4.7 3 .7 4.4 0", "M3 20l2 2 4-5M14 20h7M14 17h5"],
  "cat-presenter": ["M4 5h16v12H4Z", "M7 9 6.7 6.5 9 7.8a7 7 0 0 1 4 0l2.3-1.3L15 9c.5.6.8 1.4.8 2.2 0 2.2-1.8 3.6-3.8 3.6s-3.8-1.4-3.8-3.6c0-.8.3-1.6.8-2.2ZM9.3 11h.1m5.2 0h.1M10.5 13c1 .6 2 .6 3 0", "M20 10l2-2M12 17v4m-3 0h6"],
  "cat-autonomous": ["M6 10 5.5 5.5 9 7.4a7.5 7.5 0 0 1 4.6 0l3.5-1.9-.5 4.3a5.7 5.7 0 0 1 .8 2.9c0 3-2.5 4.9-5.7 4.9S6 15.7 6 12.7c0-1 .3-1.9 0-2.7Z", "M9 12h.1m5.3 0h.1M10 15c1 .6 2.4.6 3.4 0", "M3 8 1.5 6.5 3 5l1.5 1.5ZM20 4l.7 1.5L22 6l-1.3.7L20 8l-.7-1.3L18 6l1.3-.5ZM18 18l3 3M4 20c5 2 11 2 16-1"],
  "cat-project": ["M5.2 9 4.7 4.5 8.5 6.6a8.5 8.5 0 0 1 5 0l3.8-2.1L17 9c.8 1 1.2 2.1 1.2 3.3 0 3.2-2.8 5.3-6.2 5.3s-6.2-2.1-6.2-5.3C5.8 11.1 5.5 10 5.2 9Z", "M8.5 12h.1m6.8 0h.1M10 15c1.3.7 2.7.7 4 0", "M3 18h7l1.2 1.5H21v3H3Z"],
  "cat-unbound": ["M6 9 5.5 4.5 9 6.5a8 8 0 0 1 4.5 0L17 4.5 16.5 9c.7 1 1 2 1 3.2 0 3.2-2.6 5.2-5.7 5.2S6 15.4 6 12.2c0-1.2.3-2.2 0-3.2Z", "M9 12h.1m5.4 0h.1M10 15c1 .7 2.3.7 3.3 0", "M7 18c-3 0-4 1-4 3m9-3c4 0 7 1 8 4m-1-4 2-1"],
  "cat-sidebar": ["M5.5 10 5 5.5 8.7 7.5a8 8 0 0 1 4.6 0L17 5.5l-.5 4.3c.7.9 1 1.9 1 3 0 3-2.6 4.9-5.7 4.9s-5.7-1.9-5.7-4.9c0-1.1.1-2 .4-2.8Z", "M8.8 12.5c.7-.7 1.4-.7 2.1 0m2.2 0c.7-.7 1.4-.7 2.1 0M10 15c1.1.6 2.3.6 3.4 0", "M7 18.5c-2.5 0-4 1-4 2.5h12c4 0 6-1 6-3 0-1.2-.8-2-2-2"],
  "cat-brand": ["M5.2 9.1 4.4 4.2 8.7 6.5A9.8 9.8 0 0 1 12 5.9c1.1 0 2.2.2 3.3.6l4.3-2.3-.8 4.9c.8 1.1 1.2 2.4 1.2 3.8 0 4.2-3.6 7-8 7s-8-2.8-8-7c0-1.4.4-2.7 1.2-3.8Z", "M8.1 12.2c.7-.6 1.5-.6 2.2 0m3.4 0c.7-.6 1.5-.6 2.2 0M12 13.8l-.8.7.8.6.8-.6-.8-.7ZM9.6 16.2c1.5 1 3.3 1 4.8 0"],
  cat: ["M5.2 9.2 4.5 4l4.4 2.5a10 10 0 0 1 6.2 0L19.5 4l-.7 5.2A7.7 7.7 0 0 1 20 13.3c0 4.2-3.6 6.7-8 6.7s-8-2.5-8-6.7c0-1.5.4-2.8 1.2-4.1Z", "M8.3 12.3h.1m7.2 0h.1M9.5 16c1.5 1 3.5 1 5 0M12 14v1"]
};
function catIcon(name = "cat") { return `<svg class="cat-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${(ICON_PATHS[name] || ICON_PATHS.cat).map((d) => `<path d="${d}"></path>`).join("")}</svg>`; }
const CAPABILITY_ICONS = { "": "cat-chat", "math-literature-research": "cat-library", "rethlas-research": "cat-research", "math-related-work": "cat-citations", "math-paper-writing": "cat-writer", "math-paper-revision": "cat-revision", "latex-beamer-ppt": "cat-presenter", "autonomous-math-research": "cat-autonomous" };
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
function esc(value) { return String(value).replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[char])); }
function normalizeLocalPath(filePath) { return String(filePath).trim().replace(/^\/(?=[A-Za-z]:[\\/])/, "").replace(/\\_/g, "_"); }
function fileUrl(conversationId, filePath) { return `/api/workspace-file?conversationId=${encodeURIComponent(conversationId)}&path=${encodeURIComponent(normalizeLocalPath(filePath))}`; }
function revealUrl(conversationId, filePath) { return `/api/reveal-file?conversationId=${encodeURIComponent(conversationId)}&path=${encodeURIComponent(normalizeLocalPath(filePath))}`; }
function fileLinks(conversationId, filePath, label = "") {
  const normalized = normalizeLocalPath(filePath);
  const visible = label || normalized.replaceAll("\\", "/");
  return `<span class="file-actions"><a class="file-link" target="_blank" href="${fileUrl(conversationId, normalized)}" title="${esc(normalized)}">${esc(visible)}</a><button type="button" class="reveal-link" data-reveal-path="${esc(normalized)}" title="${esc(normalized)}">在资源管理器中显示</button></span>`;
}
async function revealFile(button, conversationId) {
  const original = button.textContent;
  button.disabled = true;
  button.textContent = "正在打开…";
  try { await api(revealUrl(conversationId, button.dataset.revealPath)); button.textContent = "已打开"; }
  catch (error) { button.textContent = original; alert(`无法打开资源管理器：${error.message}`); }
  finally { setTimeout(() => { button.disabled = false; button.textContent = original; }, 1200); }
}
function renderMessageText(value, conversationId) {
  const tokens = [];
  const tokenFor = (html) => { const token = `@@MATHLAB_LINK_${tokens.length}@@`; tokens.push(html); return token; };
  let text = String(value).replace(/:codex-file-citation\{path="([^"]+)"(?:\s+purpose="[^"]*")?\}/g, (_, filePath) => tokenFor(fileLinks(conversationId, filePath, "打开文件")));
  text = text.replace(/:codex-file-citation\{path='([^']+)'(?:\s+purpose='[^']*')?\}/g, (_, filePath) => tokenFor(fileLinks(conversationId, filePath, "打开文件")));
  text = text.replace(/\[([^\]]+)\]\(\s*<?(\/?[A-Za-z]:[\\/][^>)\r\n]+)>?\s*\)/g, (_, label, filePath) => {
    const token = `@@MATHLAB_LINK_${tokens.length}@@`;
    tokens.push(fileLinks(conversationId, filePath, label));
    return token;
  });
  text = text.replace(/(^|[\s：:<])([/]?[A-Za-z]:[\\/][^\r\n<>"|?*]+?\.(?:pdf|tex|md|json|pptx|docx|zip|bib|log|png|jpe?g))(?=$|[\s，。；、>)）])/g, (_, prefix, filePath) => `${prefix}${tokenFor(fileLinks(conversationId, filePath))}`);
  text = renderLatexText(text, { escapeHtml: esc, renderFormula: (source, displayMode) => {
    return `<span class="rendered-math${displayMode ? " display" : ""}">${katex.renderToString(source, { displayMode, throwOnError: true, strict: "warn", trust: false, output: "htmlAndMathml" })}</span>`;
  } });
  return tokens.reduce((result, html, index) => result.replace(`@@MATHLAB_LINK_${index}@@`, html), text);
}

function renderWorkspaceTree() {
  const tree = $("workspaceTree");
  const visibleConversations = state.conversations.filter((item) => Number(item.messageCount) > 0);
  const projectless = visibleConversations.filter((item) => item.workspaceId == null);
  const projectlessGroup = `<section class="workspace-group ${state.conversationId && state.workspaceId == null ? "active" : ""}"><button class="workspace-row" data-projectless aria-expanded="${state.projectlessExpanded}"><span class="chevron">${state.projectlessExpanded ? "⌄" : "›"}</span><span class="folder">💬</span><span class="workspace-text"><strong>无工作区对话</strong><small>${projectless.length} 个对话</small></span><span class="row-action add-chat" data-new-projectless title="新建无工作区对话">＋</span></button><div class="workspace-conversations" ${state.projectlessExpanded ? "" : "hidden"}>${projectless.length ? projectless.map((conversation) => `<button class="conversation ${conversation.id === state.conversationId ? "active" : ""}" data-conversation="${conversation.id}"><span class="conversation-text"><span>${esc(conversation.title)}</span><small>${conversation.messageCount} 条 · ${conversation.status}</small></span><span class="delete-conversation" data-delete-conversation="${conversation.id}" role="button" aria-label="删除对话" title="删除这条对话记录">×</span></button>`).join("") : `<div class="empty-conversations">暂无对话</div>`}</div></section>`;
  tree.innerHTML = projectlessGroup + state.workspaces.map((workspace) => {
    const rows = visibleConversations.filter((item) => item.workspaceId === workspace.id);
    const expanded = state.expanded.has(workspace.id);
    return `<section class="workspace-group ${workspace.id === state.workspaceId ? "active" : ""}">
      <button class="workspace-row" data-workspace="${workspace.id}" title="${esc(workspace.path)}"><span class="chevron">${expanded ? "⌄" : "›"}</span><span class="folder">📁</span><span class="workspace-text"><strong>${esc(workspace.name)}</strong><small>${rows.length} 个对话</small></span><span class="row-action add-chat" data-new-chat="${workspace.id}" title="在此工作区新建对话">＋</span><span class="row-action delete-workspace" data-delete-workspace="${workspace.id}" role="button" aria-label="移除工作区" title="从平台移除此工作区">×</span></button>
      <div class="workspace-conversations" ${expanded ? "" : "hidden"}>${rows.length ? rows.map((conversation) => `<button class="conversation ${conversation.id === state.conversationId ? "active" : ""}" data-conversation="${conversation.id}"><span class="conversation-text"><span>${esc(conversation.title)}</span><small>${conversation.messageCount} 条 · ${conversation.status}</small></span><span class="delete-conversation" data-delete-conversation="${conversation.id}" role="button" aria-label="删除对话" title="删除这条对话记录">×</span></button>`).join("") : `<div class="empty-conversations">暂无对话</div>`}</div>
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
    state.workspaceId = id;
    state.conversationId = null;
    state.currentBoard = null;
    setResearchView("chat");
    renderAll();
  });
  tree.querySelectorAll("[data-new-chat]").forEach((button) => button.onclick = async (event) => { event.stopPropagation(); await createChat(button.dataset.newChat); });
  tree.querySelectorAll("[data-new-projectless]").forEach((button) => button.onclick = async (event) => { event.stopPropagation(); await createChat(null); });
  tree.querySelectorAll("[data-delete-workspace]").forEach((button) => button.onclick = async (event) => { event.stopPropagation(); const workspace = state.workspaces.find((item) => item.id === button.dataset.deleteWorkspace); const count = state.conversations.filter((item) => item.workspaceId === workspace.id).length; if (!confirm(`确认从平台移除工作区“${workspace.name}”吗？\n\n将删除平台中的工作区条目和 ${count} 条对话记录。磁盘上的原始文件、论文和任务成果不会被删除。\n\n是否继续？`)) return; await api(`/api/workspaces/${workspace.id}`, { method: "DELETE" }); if (state.workspaceId === workspace.id) { clearTimeout(state.poll); state.conversationLoadId += 1; state.workspaceId = null; state.conversationId = null; state.currentBoard = null; setResearchView("chat"); } state.expanded.delete(workspace.id); await reloadLists(); });
  tree.querySelectorAll("[data-conversation]").forEach((button) => button.onclick = async (event) => { if (event.target.closest("[data-delete-conversation]")) return; clearTimeout(state.poll); state.conversationLoadId += 1; state.conversationId = button.dataset.conversation; state.currentBoard = null; setResearchView("chat"); renderWorkspaceTree(); await renderConversation(); });
  tree.querySelectorAll("[data-delete-conversation]").forEach((button) => button.onclick = async (event) => { event.stopPropagation(); const conversation = state.conversations.find((item) => item.id === button.dataset.deleteConversation); const hasRecords = Number(conversation.messageCount) > 0; if (hasRecords && !confirm(`确认删除对话“${conversation.title}”吗？\n\n这会永久删除平台保存的 ${conversation.messageCount} 条对话记录，但不会删除工作区文件或任务成果。\n\n是否继续？`)) return; await api(`/api/conversations/${conversation.id}`, { method: "DELETE" }); if (state.conversationId === conversation.id) { clearTimeout(state.poll); state.conversationLoadId += 1; state.conversationId = null; state.currentBoard = null; setResearchView("chat"); } await reloadLists(); });
}

function boardStatus(value) { const labels={created:"已创建",running:"研究中",proposed:"待确认",active:"进行中",paused:"已暂停",pruned:"已否决",pending:"待规划器接纳",queued:"排队中",approved:"已批准",rejected:"已否决",accepted:"已接受",open:"待处理",solved:"已解决",blocked:"被阻塞"}; return labels[value]||value||"未知"; }
function emptyBoardList(text) { return `<div class="board-empty-list">${esc(text)}</div>`; }
function boardProblem(board) { return `<section class="board-card wide"><h3><span>问题与约束</span><button type="button" data-edit-problem>编辑问题</button></h3><div class="problem-statement">${renderMessageText(board.problem.statement,state.conversationId)}</div><div class="problem-meta">版本 v${board.problem.version} · 目标：${renderMessageText(board.problem.goal,state.conversationId)}</div></section>`; }
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
function graphModel(board, view) { return view === "dependency" ? buildDependencyGraph(board) : buildProofTree(board); }
function graphStatusLabel(status) { return GRAPH_STATUS_LABELS[status] || boardStatus(status); }
function svgLabel(value) {
  const chars=[...String(value||"未命名节点")];
  const lines=[];
  while(chars.length&&lines.length<2)lines.push(chars.splice(0,15).join(""));
  if(chars.length&&lines.length)lines[lines.length-1]=`${lines[lines.length-1].slice(0,14)}…`;
  return lines.map((line,index)=>`<tspan x="14" dy="${index?18:0}">${esc(line)}</tspan>`).join("");
}
function renderGraphSvg(model) {
  const layout=layoutGraph(model);
  const marker=`<defs><marker id="graph-arrow" markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto"><path d="M0 0 8 4 0 8Z"></path></marker><marker id="graph-arrow-red" markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto"><path d="M0 0 8 4 0 8Z"></path></marker></defs>`;
  const edges=model.edges.map(item=>{const from=layout.positions.get(item.from);const to=layout.positions.get(item.to);if(!from||!to)return"";const contradiction=item.relation==="contradicts";let path,labelX,labelY;if(model.kind==="proof"){const x1=from.x+from.width/2,y1=from.y+from.height,x2=to.x+to.width/2,y2=to.y;const middle=(y1+y2)/2;path=`M${x1} ${y1} C${x1} ${middle},${x2} ${middle},${x2} ${y2}`;labelX=(x1+x2)/2;labelY=middle-4;}else{const x1=from.x+from.width,y1=from.y+from.height/2,x2=to.x,y2=to.y+to.height/2;const middle=(x1+x2)/2;path=`M${x1} ${y1} C${middle} ${y1},${middle} ${y2},${x2} ${y2}`;labelX=middle;labelY=(y1+y2)/2-5;}return `<g class="graph-edge ${contradiction?"contradiction":""}"><path d="${path}" marker-end="url(#graph-arrow${contradiction?"-red":""})"></path><text x="${labelX}" y="${labelY}">${esc(GRAPH_RELATION_LABELS[item.relation]||item.relation)}</text></g>`;}).join("");
  const nodes=model.nodes.map(item=>{const position=layout.positions.get(item.id);if(!position)return"";const selected=item.id===graphUi.selectedId;const collapsed=graphUi.collapsed.has(item.id);return `<g class="graph-node kind-${esc(item.kind)} status-${esc(item.status)} ${selected?"selected":""}" transform="translate(${position.x} ${position.y})" data-graph-node="${esc(item.id)}" role="button" tabindex="0" aria-label="${esc(`${GRAPH_KIND_LABELS[item.kind]||item.kind}：${item.label}，${graphStatusLabel(item.status)}`)}"><rect width="${position.width}" height="${position.height}" rx="13"></rect><circle class="graph-status-dot" cx="15" cy="15" r="4"></circle><text class="graph-kind" x="25" y="18">${esc(GRAPH_KIND_LABELS[item.kind]||item.kind)}</text>${item.logic?`<text class="graph-logic" x="180" y="18" text-anchor="end">${esc(item.logic)}</text>`:""}<text class="graph-node-label" x="14" y="42">${svgLabel(item.label)}</text>${collapsed?`<g class="graph-collapsed"><circle cx="184" cy="64" r="8"></circle><text x="184" y="67" text-anchor="middle">＋</text></g>`:""}</g>`;}).join("");
  return `<svg class="research-graph-svg" viewBox="0 0 ${layout.width} ${layout.height}" width="${Math.round(layout.width*graphUi.scale)}" height="${Math.round(layout.height*graphUi.scale)}" aria-label="${model.kind==="proof"?"证明树":"依赖图"}">${marker}${edges}${nodes}</svg>`;
}
function renderGraphInspector(model) {
  const selected=model.nodes.find(item=>item.id===graphUi.selectedId);
  if(!selected)return `<div class="graph-inspector-empty">${catIcon(graphUi.view==="proof"?"cat-research":"cat-citations")}<strong>选择一个节点</strong><p>查看完整数学陈述、可信状态和连接关系。</p></div>`;
  const incoming=model.edges.filter(item=>item.to===selected.id);
  const outgoing=model.edges.filter(item=>item.from===selected.id);
  const canCollapse=model.kind==="proof"&&outgoing.length>0;
  return `<div class="graph-inspector-content"><span class="graph-inspector-kind">${esc(GRAPH_KIND_LABELS[selected.kind]||selected.kind)}</span><h3>${esc(selected.label)}</h3><div class="graph-inspector-badges"><span class="badge ${esc(selected.status)}">${esc(graphStatusLabel(selected.status))}</span><span class="trust-badge ${selected.trust==="fact"||selected.trust==="verified"?"trusted":""}">${selected.trust==="fact"?"Fact Gate 已接受":selected.trust==="verified"?"已核验":"尚未成为 Fact"}</span></div>${selected.detail?`<p class="graph-inspector-detail">${esc(selected.detail)}</p>`:""}<dl><div><dt>进入关系</dt><dd>${incoming.length}</dd></div><div><dt>后续关系</dt><dd>${outgoing.length}</dd></div><div><dt>节点 ID</dt><dd title="${esc(selected.id)}">${esc(selected.id)}</dd></div></dl>${canCollapse?`<button type="button" class="graph-collapse-button" data-collapse-node="${esc(selected.id)}">${graphUi.collapsed.has(selected.id)?"展开这个分支":"折叠这个分支"}</button>`:""}<div class="graph-trust-note">猫猫提示：任务完成不等于证明成立。只有通过独立验证和 Fact Gate 的结论才显示为可信 Fact。</div></div>`;
}
function renderGraphDialog() {
  const board=normalizeResearchBoard(state.currentBoard);
  if(!board)return;
  const fullModel=graphModel(board,graphUi.view);
  const model=visibleGraph(fullModel,{filter:graphUi.filter,query:graphUi.query,collapsed:graphUi.collapsed});
  if(graphUi.selectedId&&!fullModel.nodes.some(item=>item.id===graphUi.selectedId))graphUi.selectedId=null;
  $("graphDialogTitle").textContent=graphUi.view==="proof"?"证明树":"依赖图";
  $("graphDialogSubtitle").textContent=graphUi.view==="proof"?"主目标在顶部，AND 必须全部完成，OR 表示可选证明路线":"箭头从依据指向使用它的结论；虚线红边表示冲突";
  graphDialog.querySelectorAll("[data-graph-view]").forEach(button=>{const active=button.dataset.graphView===graphUi.view;button.classList.toggle("active",active);button.setAttribute("aria-selected",String(active));});
  $("graphFilter").value=graphUi.filter;
  $("graphSearch").value=graphUi.query;
  $("graphZoomLabel").textContent=`${Math.round(graphUi.scale*100)}%`;
  $("graphStage").innerHTML=model.nodes.length?renderGraphSvg(model):`<div class="graph-no-results">没有符合当前筛选条件的节点</div>`;
  $("graphInspector").innerHTML=renderGraphInspector(fullModel);
  $("graphStage").querySelectorAll("[data-graph-node]").forEach(element=>{const select=()=>{graphUi.selectedId=element.dataset.graphNode;renderGraphDialog();};element.onclick=select;element.onkeydown=event=>{if(event.key==="Enter"||event.key===" "){event.preventDefault();select();}};});
  $("graphInspector").querySelector("[data-collapse-node]")?.addEventListener("click",event=>{const id=event.currentTarget.dataset.collapseNode;if(graphUi.collapsed.has(id))graphUi.collapsed.delete(id);else graphUi.collapsed.add(id);renderGraphDialog();});
}
function openResearchGraph(view) { graphUi.view=view;graphUi.scale=1;graphUi.filter="all";graphUi.query="";graphUi.selectedId=null;graphUi.collapsed=new Set();renderGraphDialog();graphDialog.showModal(); }
function boardGraphCards(board) {
  const proof=graphSummary(buildProofTree(board));
  const dependency=graphSummary(buildDependencyGraph(board));
  return `<section class="board-card wide graph-overview"><div class="graph-overview-head"><div><h3>证明结构</h3><p>从路线到可信依赖，快速找到研究卡点</p></div><span>实时投影</span></div><div class="graph-preview-grid"><button type="button" class="graph-preview-card proof" data-open-graph="proof"><span class="graph-preview-art" aria-hidden="true"><i></i><i></i><i></i><i></i></span><span class="graph-preview-copy"><strong>证明树</strong><small>${proof.total} 个节点 · ${proof.active} 个进行中 · ${proof.blocked} 个阻塞</small><em>展开查看主目标与证明分支 →</em></span></button><button type="button" class="graph-preview-card dependency" data-open-graph="dependency"><span class="graph-preview-art" aria-hidden="true"><i></i><i></i><i></i><i></i></span><span class="graph-preview-copy"><strong>依赖图</strong><small>${dependency.verified} 个可信节点 · ${dependency.unverified} 个未验证依赖</small><em>展开检查事实、来源与冲突 →</em></span></button></div></section>`;
}
function boardTimeline(board) { return `<section class="board-card wide"><h3>研究日志</h3><div class="timeline">${[...board.events].reverse().slice(0,12).map(item=>`<div class="timeline-item"><time>${new Date(item.at).toLocaleTimeString("zh-CN",{hour:"2-digit",minute:"2-digit"})}</time><i></i><span>${esc(item.text)}</span></div>`).join("")}</div></section>`; }
function shortText(value,limit=220){const text=String(value||"").trim();return text.length>limit?`${text.slice(0,limit)}…`:text;}
function boardSummary(board){const summary=board.summary||{};const items=[["轮次",summary.current_round??0],["进行路线",summary.active_routes??board.routes.length],["开放目标",summary.open_goals??board.goals.length],["可信 Fact",summary.accepted_facts??board.claims.filter(item=>item.kind==="fact"||item.status==="accepted").length],["待验证",summary.pending_candidates??board.verificationQueue.filter(item=>item.status==="pending").length],["阻塞疑点",summary.blocking_uncertainties??board.uncertainties.filter(item=>item.status==="open").length]];return `<section class="board-summary wide" aria-label="研究状态摘要">${items.map(([label,value])=>`<div><strong>${esc(value)}</strong><span>${label}</span></div>`).join("")}</section>`;}
function goalClaimList(board){const goals=board.goals.map(item=>`<article class="research-item goal-item"><div><span class="item-kind">目标</span><span class="badge ${esc(item.status)}">${boardStatus(item.status)}</span></div><strong>${esc(shortText(item.statement,320))}</strong>${item.solved_by_fact_id?`<small>由 Fact ${esc(item.solved_by_fact_id)} 解决</small>`:""}</article>`);const claims=board.claims.map(item=>`<article class="research-item claim-item ${item.kind==="fact"||item.status==="accepted"?"trusted":""}"><div><span class="item-kind">${esc(item.kind==="fact"?"Fact":item.kind==="hypothesis"?"假设":"候选")}</span><span class="badge ${esc(item.status)}">${boardStatus(item.status)}</span></div><strong>${esc(shortText(item.statement,280))}</strong>${item.verification_id?`<small>验证记录：${esc(item.verification_id)}</small>`:""}</article>`);return goals.length||claims.length?`<div class="research-item-list">${[...goals,...claims].join("")}</div>`:emptyBoardList("智能体尚未提交子目标或候选结论");}
function failedRouteList(board){return board.failedRoutes.length?`<div class="research-item-list">${board.failedRoutes.map(item=>`<article class="research-item failed-item"><div><span class="item-kind">失败路线</span><span class="badge ${esc(item.status)}">${boardStatus(item.status)}</span></div><strong>${esc(item.title||"未命名路线")}</strong><p>${esc(shortText(item.summary||item.reason,260))}</p></article>`).join("")}</div>`:emptyBoardList("尚未记录失败路线");}
function executionPanel(board){const tasks=board.tasks.slice().reverse().slice(0,6);const workers=board.workers;return `<section class="board-card"><h3>任务与 Worker <span class="section-count">${board.tasks.length} / ${workers.length}</span></h3>${tasks.length?`<div class="research-item-list">${tasks.map(item=>`<article class="research-item"><div><span class="item-kind">${esc(item.worker_role||"task")}</span><span class="badge ${esc(item.status)}">${boardStatus(item.status)}</span></div><strong>${esc(shortText(item.objective,180))}</strong>${item.result_summary?`<p>${esc(shortText(item.result_summary,220))}</p>`:""}</article>`).join("")}</div>`:emptyBoardList("尚未派发研究任务")}${workers.length?`<div class="worker-strip">${workers.map(item=>`<span><i class="${esc(item.status)}"></i>${esc(item.role||"worker")} · ${esc(item.backend||"")} · ${boardStatus(item.status)}</span>`).join("")}</div>`:""}</section>`;}
function uncertaintyPanel(board){const open=board.uncertainties.filter(item=>item.status!=="resolved");return `<section class="board-card"><h3>开放疑点 <span class="section-count">${open.length}</span></h3>${open.length?`<div class="research-item-list">${open.slice(0,8).map(item=>`<article class="research-item uncertainty-item"><div><span class="item-kind">${esc(item.type||"疑点")}</span><span class="severity ${esc(item.severity||"medium")}">${esc(item.severity||"medium")}</span></div><strong>${esc(shortText(item.description,250))}</strong></article>`).join("")}</div>`:emptyBoardList("当前没有开放疑点")}</section>`;}
function verificationPanel(board){const queue=board.verificationQueue.slice().reverse();const accepted=queue.filter(item=>item.status==="accepted").length;const rejected=queue.filter(item=>item.status==="rejected").length;return `<section class="board-card wide"><h3>独立验证与研究产物 <span class="section-count">${accepted} 接受 · ${rejected} 拒绝 · ${board.artifacts.length} 个产物</span></h3>${queue.length?`<div class="verification-grid">${queue.slice(0,6).map(item=>`<details class="verification-item"><summary><span class="badge ${esc(item.status)}">${boardStatus(item.status)}</span><strong>${esc(item.candidate_id||item.verification_id)}</strong></summary><p>${esc(shortText(item.report?.summary,520))}</p>${item.report?.repair_actions?.length?`<ul>${item.report.repair_actions.slice(0,3).map(action=>`<li>${esc(shortText(action,180))}</li>`).join("")}</ul>`:""}</details>`).join("")}</div>`:emptyBoardList("当前没有验证记录")}${board.artifacts.length?`<div class="artifact-strip">${board.artifacts.slice(0,8).map(item=>`<span title="${esc(item.artifact_id||"")}">${esc(item.filename||item.kind||"研究产物")}</span>`).join("")}</div>`:""}</section>`;}
function entryValue(value){return Array.isArray(value?.entries)?Object.fromEntries(value.entries.map(item=>[item.name,item.value])):{};}
function experimentPanel(board){if(!board.experiments.length)return"";return `<section class="board-card wide computation-evidence"><h3>计算证据 <span class="section-count">${board.experiments.length} 次可重放实验 · 不等于一般证明</span></h3><div class="experiment-list">${board.experiments.map(item=>{const input=entryValue(item.input);const mapping=entryValue(item.conclusion_mapping);const artifacts=Array.isArray(item.artifacts)?item.artifacts:[];return `<article class="experiment-card"><div class="experiment-head"><strong>${esc(item.language||"计算实验")}</strong><span class="badge ${Number(item.exit_code)===0?"accepted":"rejected"}">exit ${esc(item.exit_code??"?")}</span></div><dl><div><dt>输入范围</dt><dd>${esc(input.lower_bound&&input.upper_bound?`${input.lower_bound}–${input.upper_bound}`:"见实验输入")}</dd></div><div><dt>重放命令</dt><dd><code>${esc((item.replay_command||[]).join(" ")||"未提供")}</code></dd></div><div><dt>逻辑边界</dt><dd>${esc(mapping.logical_limitation||"计算结果仅作为有限证据")}</dd></div></dl>${item.stdout?`<pre class="experiment-output">${esc(shortText(item.stdout,900))}</pre>`:""}${artifacts.length?`<div class="artifact-strip">${artifacts.map(file=>`<span title="SHA-256 ${esc(file.sha256||"")}">${esc(file.path||"实验产物")}</span>`).join("")}</div>`:""}${item.program_text?`<details class="experiment-code"><summary>查看可重放代码</summary><pre>${esc(item.program_text)}</pre></details>`:""}</article>`;}).join("")}</div></section>`;}
function renderMathCatBoard(board) { return `${boardSummary(board)}${boardProblem(board)}${boardGraphCards(board)}<section class="board-card"><h3><span>研究路线</span><button type="button" data-add-route>＋ 新路线</button></h3><div class="board-list">${board.routes.map(route=>`<div class="route-card"><div class="route-head"><strong>${esc(route.title)}</strong><span class="badge ${route.status}">${boardStatus(route.status)}</span>${route.humanStatus==="pending"?`<span class="badge pending">待人工批准</span>`:""}</div><p>${esc(route.summary)}</p><div class="route-actions">${route.humanStatus==="pending"?`<button data-route="${route.id}" data-command="approve">批准</button><button data-route="${route.id}" data-command="reject">否决</button>`:route.status==="active"?`<button data-route="${route.id}" data-command="pause">暂停</button>`:route.status==="paused"?`<button data-route="${route.id}" data-command="resume">恢复</button>`:""}</div></div>`).join("")}</div></section>${boardDecisions(board)}<section class="board-card wide"><h3>子目标与候选结论 <span class="section-count">${board.goals.length+board.claims.length}</span></h3>${goalClaimList(board)}</section>${executionPanel(board)}${uncertaintyPanel(board)}${experimentPanel(board)}${verificationPanel(board)}<section class="board-card"><h3>失败路线 <span class="section-count">${board.failedRoutes.length}</span></h3>${failedRouteList(board)}</section>${boardTimeline(board)}`; }
function renderRethlasBoard(board) { return `${boardProblem(board)}${boardGraphCards(board)}<section class="board-card wide"><h3>生成—验证闭环</h3><div class="loop-flow"><div class="loop-node"><strong>生成 Agent</strong><small>证明蓝图</small></div><span>→</span><div class="loop-node"><strong>验证 Agent</strong><small>独立检查</small></div><span>→</span><div class="loop-node"><strong>${board.verification.verdict==="pending"?"等待运行":esc(board.verification.verdict)}</strong><small>修复或完成</small></div></div></section><section class="board-card"><h3>证明蓝图</h3>${board.blueprint.length?"":emptyBoardList("尚未生成 blueprint.md")}</section><section class="board-card"><h3>验证报告</h3><div class="status-row"><strong>当前判定</strong><span class="badge pending">${esc(board.verification.verdict)}</span></div>${emptyBoardList("等待 Rethlas 适配器同步验证报告")}</section><section class="board-card wide"><h3>迭代历史</h3>${board.iterations.length?"":emptyBoardList("尚无生成—验证迭代")}</section>${boardTimeline(board)}`; }
function renderDanusBoard(board) { return `${boardProblem(board)}${boardGraphCards(board)}<section class="board-card"><h3>战略与调度</h3><div class="status-row"><strong>Elaboration</strong><span>${esc(board.strategy.elaboration)}</span></div><div class="status-row"><strong>Master guidance</strong><span>${esc(board.strategy.masterGuidance)}</span></div></section><section class="board-card"><h3>Worker 状态</h3>${board.workers.length?"":emptyBoardList("尚未启动 Worker")}</section><section class="board-card wide"><h3>三层记忆与事实边界</h3><div class="truth-warning">Global Memory 只是共享发现；只有通过验证器写入 Fact Graph 的内容才是事实。</div><div class="memory-columns"><div class="memory-tier"><strong>Local Memory</strong><small>Worker 私有草稿 · ${board.memories.local.length} 条</small></div><div class="memory-tier"><strong>Global Memory</strong><small>共享发现 · ${board.memories.global.length} 条</small></div><div class="memory-tier"><strong>Fact Graph</strong><small>已验证事实 · ${board.facts.length} 条</small></div></div></section><section class="board-card"><h3>验证队列</h3>${board.verificationQueue.length?"":emptyBoardList("当前没有待验证候选")}</section>${boardDecisions(board)}${boardTimeline(board)}`; }
function renderResearchBoard() {
  const board=normalizeResearchBoard(state.currentBoard);
  if(!board){boardElement.innerHTML=`<div class="board-empty">${catIcon("cat-research")}<div><strong>研究白板将在提交研究问题后创建</strong><p>问题、路线、人工决策与研究日志会保存在这里。</p></div></div>`;return;}
  state.currentBoard=board;
  const body=board.agent==="rethlas"?renderRethlasBoard(board):board.agent==="danus"?renderDanusBoard(board):renderMathCatBoard(board);
  boardElement.innerHTML=`<div class="board-top"><div class="board-title"><h2>研究白板</h2><p>${esc(board.mode)} · Revision ${board.revision}</p></div><span class="board-run-status ${esc(board.status)}">${boardStatus(board.status)}</span><span class="board-agent">${esc(board.agent==="mathcat"?"MathCat":board.agent==="rethlas"?"Rethlas":"Danus")}</span></div><div class="board-grid">${body}</div>`;
  const applyBoardResult = async (request) => { const conversationId=state.conversationId; try { const updated=normalizeResearchBoard(await request); if(state.conversationId!==conversationId||state.currentBoard?.id!==board.id)return false; state.currentBoard=updated; if(state.activeView==="board")renderResearchBoard(); return true; } catch(error) { if(state.conversationId===conversationId)alert(`白板操作失败：${error.message}`); return false; } };
  boardElement.querySelectorAll("[data-open-graph]").forEach(button=>button.onclick=()=>openResearchGraph(button.dataset.openGraph));
  boardElement.querySelector("[data-edit-problem]").onclick=async()=>{const statement=prompt("修改数学问题（将生成新版本）",board.problem.statement);if(!statement||statement===board.problem.statement)return;await applyBoardResult(api(`/api/research-boards/${board.id}`,{method:"PATCH",body:JSON.stringify({statement,goal:statement})}));};
  boardElement.querySelector("[data-add-route]")?.addEventListener("click",async()=>{const title=prompt("研究路线名称");if(!title)return;const summary=prompt("简要说明这条路线","")||"";await applyBoardResult(api(`/api/research-boards/${board.id}/routes`,{method:"POST",body:JSON.stringify({title,summary})}));});
  boardElement.querySelectorAll("[data-route]").forEach(button=>button.onclick=async()=>{await applyBoardResult(api(`/api/research-boards/${board.id}/routes/${button.dataset.route}/commands`,{method:"POST",body:JSON.stringify({command:button.dataset.command})}));});
  boardElement.querySelectorAll("[data-decision]").forEach(button=>button.onclick=async()=>{const card=button.closest("[data-question-card]");const note=card?.querySelector(`[data-decision-note="${button.dataset.decision}"]`)?.value||"";const controls=card?.querySelectorAll("button, textarea")||[];controls.forEach(control=>control.disabled=true);const succeeded=await applyBoardResult(api(`/api/research-boards/${board.id}/decisions/${button.dataset.decision}`,{method:"POST",body:JSON.stringify({answer:button.dataset.answer,note})}));if(!succeeded)controls.forEach(control=>control.disabled=false);});
}
graphDialog.querySelector("[data-graph-close]").onclick=()=>graphDialog.close();
graphDialog.addEventListener("click",event=>{if(event.target===graphDialog)graphDialog.close();});
graphDialog.querySelectorAll("[data-graph-view]").forEach(button=>button.onclick=()=>{graphUi.view=button.dataset.graphView;graphUi.selectedId=null;graphUi.collapsed=new Set();renderGraphDialog();});
$("graphFilter").onchange=event=>{graphUi.filter=event.target.value;renderGraphDialog();};
$("graphSearch").oninput=event=>{graphUi.query=event.target.value;renderGraphDialog();$("graphSearch").focus();};
graphDialog.querySelectorAll("[data-graph-zoom]").forEach(button=>button.onclick=()=>{if(button.dataset.graphZoom==="in")graphUi.scale=Math.min(1.8,graphUi.scale+.15);else if(button.dataset.graphZoom==="out")graphUi.scale=Math.max(.55,graphUi.scale-.15);else graphUi.scale=1;renderGraphDialog();});
function setResearchView(requestedView) { const enabled=Boolean(state.currentBoard)||state.capabilityId==="rethlas-research"; const view=resolveResearchView(requestedView,enabled); state.activeView=view; const boardOpen=view==="board"; $("messages").hidden=boardOpen; boardElement.hidden=!boardOpen; document.querySelector(".composer").hidden=boardOpen; viewSwitch.querySelectorAll("button").forEach(button=>{const active=button.dataset.view===view;button.classList.toggle("active",active);button.setAttribute("aria-pressed",String(active));}); if(boardOpen)renderResearchBoard(); }
function updateResearchViewVisibility(enabled) { viewSwitch.classList.toggle("visible",enabled); if(!enabled&&state.activeView==="board")setResearchView("chat"); }

function renderAll() {
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
  const loadId = ++state.conversationLoadId;
  const [conversation, currentBoard] = await Promise.all([
    api(`/api/conversations/${conversationId}`),
    api(`/api/research-boards/by-conversation/${conversationId}`).catch((error) => { if(error.status===404)return null; throw error; })
  ]);
  if (!isCurrentConversationLoad(state.conversationId, conversationId, state.conversationLoadId, loadId)) return;
  state.currentBoard = normalizeResearchBoard(currentBoard);
  updateResearchViewVisibility(Boolean(state.currentBoard) || conversation.messages.some((message) => message.capabilityId === "rethlas-research"));
  if (state.activeView === "board") renderResearchBoard();
  state.workspaceId = conversation.workspaceId;
  if (conversation.workspaceId != null) state.expanded.add(conversation.workspaceId);
  $("title").textContent = conversation.title;
  $("workspaceName").textContent = "";
  $("workspaceName").hidden = true;
  contextPicker.hidden = true;
  $("status").textContent = conversation.status === "running" ? "运行中" : conversation.status === "failed" ? "上次失败" : conversation.status === "cancelled" ? "已中止" : "就绪";
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
      const legacyLog = stdoutLog ? `<a class="thinking-log-link" target="_blank" href="/api/artifact?conversationId=${encodeURIComponent(conversation.id)}&path=${encodeURIComponent(stdoutLog)}">打开完整运行日志</a>` : "";
      const activityDetails = message.role === "assistant" && (savedDetails.length || stdoutLog) ? `<details class="thinking-details saved-thinking"><summary>查看本次思考/执行详情${savedDetails.length ? ` <span>${savedDetails.length} 条</span>` : ""}</summary><div class="thinking-note">以下是 Codex 提供的可见推理摘要与执行记录，不包含私有思维链。</div>${savedDetails.map((line) => `<div class="thinking-detail">${esc(line)}</div>`).join("")}${legacyLog}</details>` : "";
      return `<article class="message ${message.role}"><div class="meta">${message.role === "user" ? "你" : esc(message.executor || "智能体")} ${message.capabilityId ? `· ${esc(message.capabilityId)}` : ""}${message.researchAgent ? ` · ${esc(message.researchAgent)}` : ""}</div>${renderMessageText(message.content, conversation.id)}${choice}${activityDetails}${(message.artifacts || []).length ? `<div class="artifacts">${message.artifacts.map((artifact) => `<a target="_blank" href="/api/artifact?conversationId=${encodeURIComponent(conversation.id)}&path=${encodeURIComponent(artifact)}">${esc(normalizeLocalPath(artifact).replaceAll("\\", "/"))}</a>`).join("")}</div>` : ""}</article>`;
    }).join("");
    state.renderedConversationId = conversation.id;
    state.renderedMessagesKey = messagesKey;
    messagesElement.querySelectorAll("[data-choice-message]").forEach((button) => button.onclick = async () => { const buttons = messagesElement.querySelectorAll("[data-choice-message]"); buttons.forEach((item) => item.disabled = true); try { await api(`/api/conversations/${conversation.id}/choices`, { method: "POST", body: JSON.stringify({ messageId: button.dataset.choiceMessage, optionIndex: Number(button.dataset.choiceIndex) }) }); await reloadLists(); await renderConversation(); } catch (error) { buttons.forEach((item) => item.disabled = false); alert(error.message); } });
    messagesElement.querySelectorAll("[data-reveal-path]").forEach((button) => button.onclick = () => revealFile(button, conversation.id));
  }
  let activityElement = $("activityBox");
  if (conversation.status === "running") {
    const activity = await api(`/api/conversations/${conversation.id}/activity`).catch(() => ({ running: true, current: "任务正在启动…", lines: [], elapsedSeconds: 0 }));
    if (!isCurrentConversationLoad(state.conversationId, conversationId, state.conversationLoadId, loadId)) return;
    if (activity.running) {
      const thinkingOpen = state.expandedThinking.has(conversation.id);
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
  if (conversation.status === "running") { clearTimeout(state.poll); state.poll = setTimeout(() => { if (state.conversationId === conversationId) renderConversation(); }, 1500); }
}

async function reloadLists() {
  [state.workspaces, state.conversations, state.capabilities, state.researchAgents] = await Promise.all([api("/api/workspaces"), api("/api/conversations"), api("/api/capabilities"), api("/api/research-agents")]);
  state.capabilities.sort((a, b) => Number(a.order || 999) - Number(b.order || 999));
  $("capability").innerHTML = `<option value="">普通对话喵</option>` + state.capabilities.map((item) => `<option value="${item.id}">${esc(item.label || item.name)}</option>`).join("");
  if (!state.capabilities.some((item) => item.id === state.capabilityId)) state.capabilityId = "";
  $("capability").value = state.capabilityId;
  $("executor").value = state.executor;
  $("permission").value = state.permission;
  $("settingsSummary").textContent = `Codex · ${state.permission === "read-only" ? "只读" : "可覆写"}`;
  renderCapabilityPicker();
  renderResearchAgentPicker();
  renderAll();
}

function renderCapabilityPicker() {
  const options = [{ id: "", label: "普通对话喵", description: "直接与 Codex 对话", icon: "cat" }, ...state.capabilities];
  const selected = options.find((item) => item.id === state.capabilityId) || options[0];
  $("capabilityIcon").innerHTML = catIcon(CAPABILITY_ICONS[selected.id] || selected.icon);
  $("capabilityLabel").textContent = selected.label || selected.name;
  $("hint").textContent = selected.label || selected.name;
  $("capabilityMenu").innerHTML = options.map((item) => `<button type="button" class="capability-option ${item.id === state.capabilityId ? "selected" : ""}" role="option" aria-selected="${item.id === state.capabilityId}" data-capability-id="${esc(item.id)}"><span class="capability-option-icon">${catIcon(CAPABILITY_ICONS[item.id] || item.icon)}</span><span class="capability-option-copy"><strong>${esc(item.label || item.name)}</strong><small>${esc(item.description || "")}</small></span><span class="capability-check">${item.id === state.capabilityId ? "✓" : ""}</span></button>`).join("");
  $("capabilityMenu").querySelectorAll("[data-capability-id]").forEach((button) => button.onclick = () => {
    state.capabilityId = button.dataset.capabilityId;
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
$("settingsButton").onclick = () => $("settingsDialog").showModal();
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
$("send").onclick = async () => { const text = $("prompt").value.trim(); if (!text) return; let createdId = null; try { if (!state.conversationId) { const conversation = await api("/api/conversations", { method: "POST", body: JSON.stringify({ workspaceId: state.workspaceId, title: text.slice(0, 40) }) }); state.conversationId = conversation.id; createdId = conversation.id; } await api(`/api/conversations/${state.conversationId}/messages`, { method: "POST", body: JSON.stringify({ text, executor: state.executor, capabilityId: state.capabilityId || null, researchAgent: state.capabilityId === "rethlas-research" ? state.researchAgentId : null, permission: state.permission }) }); $("prompt").value = ""; await reloadLists(); await renderConversation(); } catch (error) { if (createdId) { await api(`/api/conversations/${createdId}`, { method: "DELETE" }).catch(() => {}); state.conversationId = null; } alert(`无法发送：${error.message}`); } };
$("prompt").onkeydown = (event) => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); $("send").click(); } };
$("capabilityTrigger").onclick = (event) => { event.stopPropagation(); const opening = $("capabilityMenu").hidden; $("capabilityMenu").hidden = !opening; $("capabilityTrigger").setAttribute("aria-expanded", String(opening)); };
$("researchAgentTrigger").onclick = (event) => { event.stopPropagation(); const opening = $("researchAgentMenu").hidden; closeCapabilityMenu(); $("researchAgentMenu").hidden = !opening; $("researchAgentTrigger").setAttribute("aria-expanded", String(opening)); };
viewSwitch.querySelectorAll("[data-view]").forEach((button) => button.onclick = () => setResearchView(button.dataset.view));
$("contextTrigger").onclick = (event) => { event.stopPropagation(); const opening = $("contextMenu").hidden; closeCapabilityMenu(); $("contextMenu").hidden = !opening; $("contextTrigger").setAttribute("aria-expanded", String(opening)); };
document.addEventListener("click", (event) => { if (!event.target.closest(".capability-picker")) closeCapabilityMenu(); if (!event.target.closest(".research-picker")) closeResearchAgentMenu(); if (!event.target.closest(".context-picker")) closeContextMenu(); });
$("permission").onchange = () => { state.permission = $("permission").value; localStorage.setItem("mathcat.permission", state.permission); $("settingsSummary").textContent = `Codex · ${state.permission === "read-only" ? "只读" : "可覆写"}`; };
$("executor").onchange = () => { state.executor = $("executor").value; };
async function cancelCurrentTask(event) { const button = event?.currentTarget; if (!state.conversationId || !confirm("确定中止当前任务吗？已生成的工作区文件会保留。")) return; if (button) button.disabled = true; try { await api(`/api/conversations/${state.conversationId}/cancel`, { method: "POST", body: "{}" }); } catch (error) { alert(error.message); } finally { if (button) button.disabled = false; } }

try { await refreshHealth(); await reloadLists(); } catch (error) { $("health").textContent = `平台连接失败：${error.message}`; }
