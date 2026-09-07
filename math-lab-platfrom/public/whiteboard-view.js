import {escapeHtml as h,latestRun,label,readable} from './research-v2-state.js';
import {proofModel,rows,short,timeLabel,selectionReference,currentStatus,reviewStatus,quoteReference,controlReference} from './whiteboard-model.js';
import {visibleGraph,focusGraphBranch,fitGraphScale} from './research-graph.js';
import {layoutProofGraph,routeProofGraphEdges,directProofRelations,proofTextLines} from './proof-graph.js';
import {headerHtml,overviewHtml,recordsHtml,feedbackHtml,empty,badge,action,dataDetails} from './whiteboard-render.js';
import {laboratoryHtml,planningHtml,questionsHtml,doubtsHtml,failuresHtml,timelineHtml,reframeHistoryHtml,sessionHtml,statementsHtml,mathematicalStatements,mathKind,replacePreservingView} from './whiteboard-lab.js';
import {explicitProblemEdit} from './problem-edit-intent.js';
import {attentionHtml,deliveryHtml,projectModelsPaused} from './whiteboard-delivery.js';
import {selectionSummary} from './codex-status.js';

const storageKey=id=>'mathcat.2.4.board.'+id;
const refKey=ref=>JSON.stringify(ref);
const validTabs=new Set(['overview','records','doubts','failures','timeline']);
export class ResearchWhiteboard {
  constructor(element,{api,classic,refresh,renderText,storage=globalThis.localStorage,openModelSettings=null,getCodexStatus=()=>null}) {
    Object.assign(this,{element,api,classic,refresh,renderText,storage,openModelSettings,getCodexStatus});
    this.ui={tab:'timeline',side:'discussion',selected:null,scale:1,filter:'all',query:'',collapsed:new Set(),positions:new Map(),focus:false,recordsLimit:30};
    this.readCache=new Map();this.readTicket=0;this.busy=false;this.error='';this.notice='';
  }
  updateCodexStatus(status,projectId){
    if(this.project?.id!==projectId)return;
    const summary=this.element.querySelector('[data-wb-codex-current]');if(summary)summary.textContent='项目模型：'+selectionSummary(status);
  }
  persist(){
    if(!this.project)return;
    const input=this.element.querySelector('[data-wb-input]');
    if(!input)return;
    if(this.restoringView){try{const saved=this.restoreView||JSON.parse(this.storage?.getItem(storageKey(this.project.id))||'{}');this.storage?.setItem(storageKey(this.project.id),JSON.stringify({...saved,draft:input.value,modalDrafts:this.modalDrafts||{}}));}catch{}return;}
    try{this.storage?.setItem(storageKey(this.project.id),JSON.stringify({
      tab:this.ui.tab,side:this.ui.side,selected:this.ui.selected,scale:this.ui.scale,graphViewVersion:242,
      modalDrafts:this.modalDrafts||{},boardScroll:this.element.scrollTop,graphScroll:{top:this.element.querySelector('[data-wb-graph]')?.scrollTop||0,left:this.element.querySelector('[data-wb-graph]')?.scrollLeft||0},readerScroll:this.element.querySelector('[data-wb-reader]')?.scrollTop||0,proofPage:this.detail?.proofPage||0,proofOpen:Boolean(this.element.querySelector('[data-wb-proof-dialog]')?.open),proofSidebar:this.ui.proofSidebar,readerWide:this.element.querySelector('.wb-proof-workspace')?.classList.contains('reader-wide'),direct:this.ui.direct,focus:this.ui.focus,
      collapsed:[...this.ui.collapsed],positions:[...this.ui.positions],query:this.ui.query,filter:this.ui.filter,
      draft:input?.value||'',intent:this.element.querySelector('[data-wb-intent]')?.value||'discussion',
      selectionRef:this.selection,quote:this.quote||'',quoteArtifactId:this.quoteArtifactId,referenceCleared:Boolean(this.referenceCleared)
    }));}catch{}
  }
  mount(project){
    this.persist();this.dismiss();this.project=project;this.readTicket++;
    let saved={};try{saved=JSON.parse(this.storage?.getItem(storageKey(project.id))||'{}');}catch{}
    this.restoringView=true;this.restoreView=saved;this.modalDrafts=saved.modalDrafts||{};
    this.ui={tab:validTabs.has(saved.tab)?saved.tab:'timeline',side:saved.side==='feedback'?'feedback':'discussion',selected:saved.selected||null,scale:saved.graphViewVersion===242?Math.max(.15,Math.min(2,saved.scale||1)):1,filter:saved.filter||'all',query:saved.query||'',focus:Boolean(saved.focus),direct:Boolean(saved.direct),proofSidebar:['none','reader','discussion'].includes(saved.proofSidebar)?saved.proofSidebar:'none',collapsed:new Set(saved.collapsed||[]),positions:new Map(saved.graphViewVersion===242?saved.positions||[]:[]),recordsLimit:30};
    this.selection=null;this.detail=null;this.error='';this.notice='';this.graphMarkup=null;this.centerPending=true;
    this.restoreSelection={ref:saved.selectionRef,quote:saved.quote||'',quoteArtifactId:saved.quoteArtifactId,referenceCleared:Boolean(saved.referenceCleared)};
    this.element.classList.add('wb23','wb24');
    this.element.innerHTML='<div data-wb-header></div><div data-wb-connection class="wb-connection"></div>'+
      '<div data-wb-attention></div><div class="wb-workspace"><section class="wb-main"><div data-wb-delivery></div><div data-wb-laboratory></div><div data-wb-planning></div><div data-wb-questions></div><div data-wb-reframe-history></div><div data-wb-proof-card></div><nav class="wb-tabs wb-content-tabs" aria-label="白板页面">'+action('tab','时间线','data-tab="timeline"')+action('tab','开放疑点','data-tab="doubts"')+action('tab','失败路线','data-tab="failures"')+action('tab','更多记录','data-tab="records"')+'</nav>'+
      '<section data-wb-overview hidden></section><section data-wb-records hidden></section><section data-wb-doubts hidden></section><section data-wb-failures hidden></section><section data-wb-timeline hidden></section></section>'+
      '<div data-wb-collaboration-home hidden></div><aside class="wb-collaboration"><nav class="wb-tabs" aria-label="人机协作">'+action('side','讨论','data-side="discussion"')+action('side','我的意见','data-side="feedback"')+'</nav>'+
      '<div data-wb-side-content class="wb-side-history"></div><section class="wb-composer"><div data-wb-reference class="wb-reference">未引用对象</div>'+
      '<label>输入方式<select data-wb-intent><option value="discussion">旁观讨论 · 不改变研究</option><option value="normal">给领研猫建议</option><option value="urgent">紧急建议 · 请求中断并续接</option><option value="annotation">批注所选版本 · 仅记录</option><option value="edit-math">修改数学问题</option><option value="edit-description">修改研究说明</option></select></label>'+
      '<label class="wb-annotation-send" hidden><input data-wb-annotation-send type="checkbox">批注保存后，同时交给领研猫</label>'+
      '<textarea data-wb-input rows="6" placeholder="围绕原文提问，或选择建议模式…"></textarea>'+
      '<p data-wb-intent-help class="wb-muted"></p><div data-wb-send-status aria-live="polite"></div><div class="wb-actions">'+action('clear-ref','移除引用')+action('send','发送')+'</div>'+
      '<div data-wb-pending></div></section></aside></div>'+
      '<dialog class="wb-proof-dialog" data-wb-proof-dialog aria-labelledby="wb-proof-title"><div class="wb-proof-shell"><header class="wb-proof-heading"><div><span class="wb-eyebrow">数学成果与前后依赖</span><h2 id="wb-proof-title">证明树</h2><p data-wb-proof-context></p></div><div class="wb-actions">'+action('proof-reader','成果详情','aria-expanded="false" aria-controls="wb-proof-inspector"')+action('proof-discussion','讨论与建议','aria-expanded="false" aria-controls="wb-proof-inspector"')+action('close-proof','返回白板','autofocus')+'</div></header><div class="wb-proof-page-body"><section data-wb-proof><div class="wb-tree-toolbar"><label>搜索成果<input data-wb-search type="search" placeholder="陈述、引理或目标"></label><select data-wb-filter aria-label="筛选成果"><option value="all">全部成果</option><option value="unverified">待审成果</option><option value="problems">争议与缺口</option></select><div class="wb-zoom-controls">'+action('zoom-out','−','aria-label="缩小证明树"')+action('zoom-reset','100%','aria-label="恢复可读比例" data-wb-scale')+action('zoom-in','＋','aria-label="放大证明树"')+action('fit','概览全树')+'</div>'+action('arrange','自动整理')+action('direct','仅看直接关系','aria-pressed="false"')+action('focus','完整上下游','aria-pressed="false"')+action('collapse','折叠／展开分支')+'</div>'+
      '<p class="wb-tree-legend"><span class="wb-legend-dependency">箭头：前提 → 成果</span><span class="wb-legend-planned">虚线箭头：拟依赖</span><span class="wb-legend-ownership">灰线无箭头：目标归属</span><span>点击成果突出直接关系；拖动空白平移，滚轮浏览，Ctrl + 滚轮缩放。</span></p><div data-wb-tree-warning></div><div class="wb-proof-workspace"><div class="wb-tree-column"><div data-wb-graph class="wb-graph" tabindex="0" aria-label="数学证明树"></div><div data-wb-graph-summary class="wb-graph-summary" aria-live="polite"></div><details class="wb-object-list"><summary>成果列表（与树相同的对象）</summary><div data-wb-list></div></details></div><aside class="wb-proof-inspector" id="wb-proof-inspector" hidden><header class="wb-inspector-toolbar"><div class="wb-actions">'+action('proof-reader','成果详情')+action('proof-discussion','讨论与建议')+'</div>'+action('close-inspector','收起','aria-label="收起侧栏"')+'</header><div class="wb-reader-tools" data-wb-reader-tools>'+action('reader-toggle','展开阅读')+'</div><article data-wb-reader class="wb-reader" tabindex="0">'+empty('在证明树中选择数学成果，读取固定版本。')+'</article><div data-wb-proof-discussion hidden></div></aside></div></section></div></div></dialog>'+
      '<dialog class="wb-dialog" data-wb-dialog><form data-wb-modal-form><div data-wb-modal-content></div><p data-wb-modal-error class="wb-error" aria-live="assertive"></p><div class="wb-actions">'+action('close-dialog','取消')+'<button class="primary" type="submit" data-wb-modal-submit>确认提交</button></div></form></dialog>';
    this.element.querySelector('[data-wb-input]').value=saved.draft||'';
    this.element.querySelector('[data-wb-intent]').value=['discussion','normal','urgent','annotation','edit-math','edit-description'].includes(saved.intent)?saved.intent:'discussion';
    this.element.querySelector('[data-wb-search]').value=this.ui.query;this.element.querySelector('[data-wb-filter]').value=this.ui.filter;
    this.element.onclick=event=>this.click(event);
    if(!this.scrollSaver){this.scrollSaver=()=>{clearTimeout(this.saveScrollTimer);this.saveScrollTimer=setTimeout(()=>this.persist(),160);};this.element.addEventListener('scroll',this.scrollSaver,{capture:true,passive:true});}
    // Keep the user's highlighted source text when the quote button takes a pointer click.
    this.element.onpointerdown=event=>{if(event.target.closest('[data-wb-action="quote"]'))event.preventDefault();};
    this.element.oninput=event=>{if(event.target.matches('[data-wb-search]')){this.ui.query=event.target.value;this.renderGraph();this.persist();}if(event.target.matches('[data-wb-input]'))this.persist();if(event.target.closest('[data-wb-modal-form]')&&event.target.name){this.modalDrafts[this.modalTitle]??={};this.modalDrafts[this.modalTitle][event.target.name]=event.target.value;this.persist();}};
    this.element.onchange=event=>{if(event.target.matches('[data-wb-filter]')){this.ui.filter=event.target.value;this.renderGraph();}if(event.target.matches('[data-wb-intent]')){this.intentHelp();this.persist();}};
    this.element.querySelector('[data-wb-modal-form]').onsubmit=event=>{event.preventDefault();void this.submitModal();};
    const dialog=this.element.querySelector('[data-wb-dialog]');dialog.oncancel=e=>{if(this.modalBusy)e.preventDefault();};
    const proofDialog=this.element.querySelector('[data-wb-proof-dialog]');
    proofDialog.oncancel=event=>{event.preventDefault();this.closeProof();};
    proofDialog.onclose=()=>{if(this.element.querySelector('[data-wb-proof-dialog]')===proofDialog&&!proofDialog.open)this.restoreCollaboration();};
    this.intentHelp();
  }
  openProof(){
    const dialog=this.element.querySelector('[data-wb-proof-dialog]');if(!dialog||dialog.open)return;
    this.proofReturnFocus=this.element.contains(document.activeElement)?document.activeElement:null;
    this.proofReturnScroll=this.element.scrollTop;
    this.element.querySelector('[data-wb-proof-discussion]').append(this.element.querySelector('.wb-collaboration'));
    dialog.showModal();
    this.showProofSidebar(this.ui.proofSidebar);this.renderGraph();
  }
  restoreCollaboration(){
    const home=this.element.querySelector('[data-wb-collaboration-home]'),aside=this.element.querySelector('.wb-collaboration');
    if(!home||!aside||aside.parentElement===home.parentElement)return;
    home.after(aside);this.element.scrollTop=this.proofReturnScroll??this.element.scrollTop;
    const focus=this.proofReturnFocus?.isConnected?this.proofReturnFocus:this.element.querySelector('[data-wb-action="open-proof-page"]');this.proofReturnFocus=null;
    if(focus?.isConnected&&!focus.closest('[hidden]'))focus.focus({preventScroll:true});
  }
  closeProof(){
    const dialog=this.element.querySelector('[data-wb-proof-dialog]');if(!dialog?.open)return;
    dialog.close();this.restoreCollaboration();this.persist();
  }
  dismiss(){
    this.element.querySelector('[data-wb-dialog]')?.close();
    this.closeProof();
  }
  renderProofCard(){
    const nodes=rows(this.model.nodes),results=nodes.filter(n=>n.id!==this.model.rootId&&n.object?.math_kind!=='problem'&&n.object?.kind!=='problem');
    const accepted=results.filter(n=>n.trust==='verified').length,dependencies=rows(this.model.edges).filter(e=>e.mathematical).length;
    replacePreservingView(this.element.querySelector('[data-wb-proof-card]'),'<button type="button" class="wb-proof-card" data-wb-action="open-proof-page" aria-haspopup="dialog"><span class="wb-proof-card-art" aria-hidden="true"><i></i><i></i><i></i><i></i></span><span class="wb-proof-card-copy"><strong>证明树</strong><span>'+h(results.length+' 项数学成果 · '+dependencies+' 条前后依赖 · '+accepted+' 项已准入且当前有效')+'</span><small>'+(results.length?'查看定理、引理、命题与猜想，以及它们的完整陈述。':'已有研究目标；后续数学成果及依赖会保存在这里。')+'</small><em>打开完整证明树 →</em></span></button>');
    this.element.querySelector('[data-wb-proof-context]').textContent=(this.project.title||'当前数学研究')+' · 问题 v'+this.project.problem_version;
  }
  update(project,conversation,error=null){
    if(this.project?.id!==project.id||!this.element.querySelector('[data-wb-header]'))this.mount(project);
    const reading=this.element.querySelector('[data-wb-proof-dialog]')?.open&&this.detail?.statement?this.element.querySelector('[data-wb-reader]'):null;
    const readingTop=reading?.getBoundingClientRect?.().top,containerTop=this.element.getBoundingClientRect?.().top;
    const anchorReading=Number.isFinite(readingTop)&&readingTop<globalThis.innerHeight&&readingTop+(reading?.clientHeight||0)>(containerTop||0);
    this.project=project;this.conversation=conversation;this.offline=error;
    this.model=proofModel(project);
    this.renderProofCard();
    replacePreservingView(this.element.querySelector('[data-wb-attention]'),attentionHtml(project));
    replacePreservingView(this.element.querySelector('[data-wb-delivery]'),deliveryHtml(project));
    replacePreservingView(this.element.querySelector('[data-wb-header]'),headerHtml(project,this.renderText,this.getCodexStatus(project.id)));
    replacePreservingView(this.element.querySelector('[data-wb-laboratory]'),laboratoryHtml(project,this.renderText));
    replacePreservingView(this.element.querySelector('[data-wb-planning]'),planningHtml(project,this.renderText));
    replacePreservingView(this.element.querySelector('[data-wb-questions]'),questionsHtml(project,this.renderText));
    replacePreservingView(this.element.querySelector('[data-wb-reframe-history]'),reframeHistoryHtml(project,this.renderText));
    if(this.detailSession&&this.element.querySelector('[data-wb-dialog]').open){
      const session=rows(project.sessions).find(s=>s.id===this.detailSession);
      if(session)replacePreservingView(this.element.querySelector('[data-wb-session-live]'),sessionHtml(project,session,this.renderText));
    }
    this.element.querySelector('[data-wb-connection]').textContent=error?'连接暂时中断；显示最后保存的记录：'+error:'';
    this.element.querySelectorAll('[data-wb-action="tab"]').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.tab===this.ui.tab)));
    this.element.querySelectorAll('[data-wb-action="side"]').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.side===this.ui.side)));
    this.renderPanels();this.renderSide();this.renderPending();this.intentHelp();this.refreshReaderEvidence();
    if(!this.selection&&this.model.nodes.length){
      const selected=this.model.objects.find(o=>o.key===this.ui.selected)||this.model.objects.find(o=>o.key===this.model.rootId);
      if(selected){const restore=this.restoreSelection||{};this.restoreSelection=null;void this.select(selected.key,restore.ref,{...restore,preserveTab:true}).then(()=>{if(this.project.id===project.id)this.restoreReadingView();});}
    }else if(this.selection){
      const object=this.model.objects.find(o=>o.ref?.id===this.selection.id&&o.ref?.kind===this.selection.kind||o.id===this.selection.id&&o.kind===this.selection.kind);
      const status=this.element.querySelector('[data-wb-current-status]');
      if(status&&object&&object.revision===this.selection.revision)status.innerHTML=this.objectStatus(object);
      const newer=this.element.querySelector('[data-wb-new-version]');
      if(newer)newer.innerHTML=object&&object.revision!==this.selection.revision?'当前已有 v'+h(object.revision)+'；你仍在阅读 v'+h(this.selection.revision)+'。'+action('latest-version','打开新版'):'';
    }
    if(!this.model.nodes.length)this.restoreReadingView();
    if(anchorReading){const delta=reading.getBoundingClientRect().top-readingTop;if(Math.abs(delta)>1&&this.element.scrollHeight>this.element.clientHeight)this.element.scrollTop+=delta;}
  }
  restoreReadingView(){
    const saved=this.restoreView;if(!saved)return;this.restoreView=null;
    if(saved.proofOpen)this.openProof();
    this.showProofSidebar(this.ui.proofSidebar);
    this.element.querySelector('.wb-proof-workspace').classList.toggle('reader-wide',Boolean(saved.readerWide&&this.ui.proofSidebar==='reader'));
    if(this.detail&&saved.proofPage){this.detail.proofPage=saved.proofPage;this.renderReader();}
    this.centerPending=false;
    requestAnimationFrame(()=>{const graph=this.element.querySelector('[data-wb-graph]');graph.scrollTop=saved.graphScroll?.top||0;graph.scrollLeft=saved.graphScroll?.left||0;this.element.scrollTop=saved.boardScroll||0;this.element.querySelector('[data-wb-reader]').scrollTop=saved.readerScroll||0;this.restoringView=false;});
  }
  renderPanels(){
    this.element.querySelectorAll('[data-wb-action="tab"]').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.tab===this.ui.tab)));
    for(const tab of validTabs)this.element.querySelector('[data-wb-'+tab+']').hidden=this.ui.tab!==tab;
    if(this.element.querySelector('[data-wb-proof-dialog]')?.open)this.renderGraph();
    if(this.ui.tab==='overview')replacePreservingView(this.element.querySelector('[data-wb-overview]'),overviewHtml(this.project,this.model,this.renderText));
    if(this.ui.tab==='records')replacePreservingView(this.element.querySelector('[data-wb-records]'),recordsHtml(this.project,this.renderText,this.ui.recordsLimit));
    if(this.ui.tab==='doubts')replacePreservingView(this.element.querySelector('[data-wb-doubts]'),doubtsHtml(this.project,this.renderText));
    if(this.ui.tab==='failures')replacePreservingView(this.element.querySelector('[data-wb-failures]'),failuresHtml(this.project,this.renderText));
    if(this.ui.tab==='timeline')replacePreservingView(this.element.querySelector('[data-wb-timeline]'),timelineHtml(this.project,this.renderText,this.ui.recordsLimit));
  }
  renderGraph(){
    if(this.dragging||!this.model)return;
    // Filtering follows the tree hierarchy; drawing keeps the actual premise → result direction.
    let placement={...this.model,edges:this.model.edges.map(e=>e.mathematical?{...e,from:e.to,to:e.from}:e)};
    placement=visibleGraph(placement,{filter:this.ui.filter,query:this.ui.query,collapsed:this.ui.collapsed});
    if(this.ui.focus)placement=focusGraphBranch(placement,this.ui.selected);
    const relations=directProofRelations(this.model,this.ui.selected);
    const visibleRelated=new Set([...relations.relatedIds,...relations.ownership.flatMap(e=>[e.from,e.to])]);
    if(this.ui.direct){const ids=relations.relatedIds;placement={...placement,nodes:placement.nodes.filter(n=>ids.has(n.id)),edges:placement.edges.filter(e=>e.mathematical&&ids.has(e.from)&&ids.has(e.to))};}
    const graphModel={...placement,edges:placement.edges.map(e=>e.mathematical?{...e,from:e.to,to:e.from}:e)};
    const layout=layoutProofGraph(graphModel,{manualPositions:this.ui.positions});
    const routed=routeProofGraphEdges(graphModel,layout,{selectedId:this.ui.selected,showCrossLinks:true});
    this.layout={...layout,width:routed.width,height:routed.height};
    const scale=this.ui.scale,hasSelection=Boolean(this.ui.selected)&&Boolean(this.ui.graphInteracted);
    const lines=routed.edges.map(e=>{
      const relation=h(e.relationLabel),muted=hasSelection&&!e.selected;
      const direction=e.mathematical?'前提 → 成果':'目标归属（不是数学前提）';
      return '<g class="wb-connection-line'+(muted?' is-muted':'')+'" data-wb-edge="'+h(e.id)+'" data-from="'+h(e.from)+'" data-to="'+h(e.to)+'"><path class="wb-edge '+h(e.relation)+(e.selected?' selected':'')+'" d="'+e.path+'" '+(e.mathematical?'marker-end="url(#wb-arrow-'+(e.relation==='planned'?'planned':'math')+')"':'')+'><title>'+relation+' · '+h(direction)+(e.versionBound?' · 绑定版本':' · 版本信息见原记录')+'</title></path>'+(e.selected?'<text class="wb-edge-label" x="'+e.labelX+'" y="'+e.labelY+'" text-anchor="middle">'+relation+'</text>':'')+'</g>';
    }).join('');
    const nodes=graphModel.nodes.map(n=>{
      const p=layout.positions.get(n.id),o=n.object;
      const reviewCaption={changes_requested:'需进一步处理',unreviewed:'未审',accepted:'审查通过',submitted:'待审'}[o.review_state]||'审核见详情';
      const statements=mathematicalStatements(o),kinds=[...new Set(statements.map(s=>mathKind(s.math_kind||s.kind||o.math_kind)))];
      const selected=this.ui.selected===n.id,prerequisite=relations.prerequisiteIds.has(n.id),consequence=relations.consequenceIds.has(n.id);
      const related=visibleRelated.has(n.id),role=selected?'当前所选':prerequisite&&consequence?'前提与后续':prerequisite?'直接前提':consequence?'后续成果':'';
      const titleLines=proofTextLines(o.title,{maxUnits:32,maxLines:2}),previewLines=proofTextLines(o.statement,{maxUnits:40,maxLines:2});
      return '<g class="wb-node '+h(n.status)+(selected?' selected':'')+(prerequisite?' is-prerequisite':'')+(consequence?' is-consequence':'')+(hasSelection&&!related?' is-muted':'')+'" data-wb-node="'+h(n.id)+'" transform="translate('+p.x+' '+p.y+')" tabindex="0" role="button" aria-pressed="'+selected+'" aria-label="'+h(o.title)+'"><title>'+h(o.title+'\n'+o.statement)+'</title><rect width="'+p.width+'" height="'+p.height+'" rx="12"/><text x="16" y="25" class="wb-node-kind">'+h(kinds.join(' / '))+(statements.length>1?' · '+statements.length+' 条':'')+' · v'+h(o.revision)+'</text><text x="'+(p.width-16)+'" y="25" text-anchor="end" class="wb-node-role">'+h(role)+'</text>'+titleLines.map((t,i)=>'<text x="16" y="'+(51+i*22)+'" class="wb-node-title">'+h(t)+'</text>').join('')+previewLines.map((t,i)=>'<text x="16" y="'+(96+i*17)+'" class="wb-node-preview">'+h(t)+'</text>').join('')+'<text x="16" y="136" class="wb-node-state">'+h(o.admission_state==='accepted'||o.admission_state==='admitted'?'已准入 · '+label(o.validity):o.kind==='problem'||o.math_kind==='problem'?'目标，完成须有覆盖证据':'已保存 · '+reviewCaption)+'</text></g>';
    }).join('');
    const graph=this.element.querySelector('[data-wb-graph]'),oldScroll={top:graph.scrollTop,left:graph.scrollLeft};
    const markup=graphModel.nodes.length?'<svg width="'+Math.ceil(routed.width*scale)+'" height="'+Math.ceil(routed.height*scale)+'" viewBox="0 0 '+routed.width+' '+routed.height+'" xmlns="http://www.w3.org/2000/svg"><defs>'+[['math','#35694f'],['planned','#956927']].map(([id,color])=>'<marker id="wb-arrow-'+id+'" markerWidth="12" markerHeight="12" refX="10" refY="5" orient="auto" markerUnits="userSpaceOnUse"><path d="M0,0 L10,5 L0,10 Z" fill="'+color+'"/></marker>').join('')+'</defs>'+lines+nodes+'</svg>':empty('没有匹配的成果。清除搜索或筛选可恢复。');
    // Unchanged snapshots must not detach nodes while the user is clicking, dragging or reading.
    if(markup!==this.graphMarkup){
      const focused=graph.contains(document.activeElement)?document.activeElement?.dataset?.wbNode:null;
      graph.innerHTML=markup;this.graphMarkup=markup;
      graph.querySelectorAll('[data-wb-node]').forEach(node=>{
        node.onkeydown=e=>{if(['Enter',' '].includes(e.key)){e.preventDefault();void this.select(node.dataset.wbNode);}};
        node.onpointerdown=e=>this.dragStart(e,node);
        node.ondblclick=()=>{this.ui.positions.delete(node.dataset.wbNode);this.persist();this.renderGraph();};
        if(node.dataset.wbNode===focused)node.focus({preventScroll:true});
      });
    }
    graph.scrollTop=oldScroll.top;graph.scrollLeft=oldScroll.left;
    graph.onpointerdown=e=>{if(e.button!==0||e.target.closest('[data-wb-node]'))return;const x=e.clientX,y=e.clientY,left=graph.scrollLeft,top=graph.scrollTop;this.dragging=true;graph.setPointerCapture(e.pointerId);graph.classList.add('is-panning');const move=event=>{graph.scrollLeft=left+x-event.clientX;graph.scrollTop=top+y-event.clientY;};const end=()=>{graph.removeEventListener('pointermove',move);graph.removeEventListener('pointerup',end);graph.removeEventListener('pointercancel',end);graph.classList.remove('is-panning');this.dragging=false;};graph.addEventListener('pointermove',move);graph.addEventListener('pointerup',end);graph.addEventListener('pointercancel',end);};
    graph.onwheel=e=>{if(!e.ctrlKey&&!e.metaKey)return;e.preventDefault();this.zoomGraph(this.ui.scale*(e.deltaY<0?1.12:1/1.12),{x:e.clientX-graph.getBoundingClientRect().left,y:e.clientY-graph.getBoundingClientRect().top});};
    this.element.querySelector('[data-wb-scale]').textContent=Math.round(scale*100)+'%';
    this.element.querySelector('[data-wb-action="direct"]').setAttribute('aria-pressed',String(this.ui.direct));
    this.element.querySelector('[data-wb-action="focus"]').setAttribute('aria-pressed',String(this.ui.focus));
    const selectedObject=this.model.objects.find(o=>o.key===this.ui.selected);
    replacePreservingView(this.element.querySelector('[data-wb-graph-summary]'),'<span>'+h(graphModel.nodes.length+' / '+this.model.nodes.length+' 个数学对象')+'</span><span>'+(selectedObject?h(short(selectedObject.title,36))+' · '+relations.incoming.length+' 个直接前提 · '+relations.outgoing.length+' 个后续成果':'选择成果，查看直接关系')+'</span>');
    if(this.centerPending&&graph.clientWidth){this.centerGraphSelection();this.centerPending=false;}
    const warnings=[...rows(this.model.warnings).map(readable)];
    if(this.project.board_errors?.proof_tree)warnings.unshift('权威证明树读取失败：'+this.project.board_errors.proof_tree+'；当前仅显示已有记录，请勿据此推断完整关系。');
    if(this.model.cycleIds.size)warnings.push('存在循环依赖，不能视为有效证明链；详情保留原关系。');
    if(this.model.missing.length)warnings.push(this.model.missing.length+' 条关系的目标或版本未能完整定位。');
    replacePreservingView(this.element.querySelector('[data-wb-tree-warning]'),warnings.length?'<div class="wb-warning">'+warnings.map(w=>'<p>'+h(w)+'</p>').join('')+'</div>':'');
    replacePreservingView(this.element.querySelector('[data-wb-list]'),graphModel.nodes.map(n=>'<button data-wb-select="'+h(n.id)+'"><strong>'+h(n.object.title)+' · v'+h(n.object.revision)+'</strong><span>'+h(short(n.object.statement,180))+'</span></button>').join(''));
  }
  showProofSidebar(mode){
    this.ui.proofSidebar=mode;
    const workspace=this.element.querySelector('.wb-proof-workspace'),inspector=this.element.querySelector('.wb-proof-inspector');
    if(!inspector)return;
    inspector.hidden=mode==='none';workspace.classList.toggle('has-inspector',mode!=='none');
    if(mode!=='reader')workspace.classList.remove('reader-wide');
    this.element.querySelector('[data-wb-reader]').hidden=mode!=='reader';
    this.element.querySelector('[data-wb-reader-tools]').hidden=mode!=='reader';
    this.element.querySelector('[data-wb-proof-discussion]').hidden=mode!=='discussion';
    this.element.querySelectorAll('[data-wb-action="proof-reader"]').forEach(b=>b.setAttribute('aria-expanded',String(mode==='reader')));
    this.element.querySelectorAll('[data-wb-action="proof-discussion"]').forEach(b=>b.setAttribute('aria-expanded',String(mode==='discussion')));
    this.element.querySelector('[data-wb-action="reader-toggle"]').textContent=workspace.classList.contains('reader-wide')?'返回图与原文':'展开阅读';
  }
  centerGraphSelection(){
    const graph=this.element.querySelector('[data-wb-graph]'),p=this.layout?.positions.get(this.ui.selected)||this.layout?.positions.values().next().value;
    if(!p||!graph?.clientWidth)return;
    graph.scrollLeft=Math.max(0,(p.x+p.width/2)*this.ui.scale-graph.clientWidth/2);
    graph.scrollTop=Math.max(0,(p.y+p.height/2)*this.ui.scale-graph.clientHeight/2);
  }
  zoomGraph(value,anchor=null){
    const graph=this.element.querySelector('[data-wb-graph]'),old=this.ui.scale;
    const x=anchor?.x??graph.clientWidth/2,y=anchor?.y??graph.clientHeight/2;
    const offset=Math.max(0,(graph.clientWidth-this.layout.width*old)/2);
    const left=(graph.scrollLeft+x-offset)/old,top=(graph.scrollTop+y)/old;
    this.ui.scale=Math.max(.15,Math.min(2,value));this.renderGraph();
    graph.scrollLeft=left*this.ui.scale+Math.max(0,(graph.clientWidth-this.layout.width*this.ui.scale)/2)-x;graph.scrollTop=top*this.ui.scale-y;this.persist();
  }
  dragStart(event,node){
    if(event.button!==0)return;
    const key=node.dataset.wbNode,start=this.layout.positions.get(key),x=event.clientX,y=event.clientY;
    let moved=false;this.dragging=true;node.setPointerCapture?.(event.pointerId);
    const move=e=>{const dx=(e.clientX-x)/this.ui.scale,dy=(e.clientY-y)/this.ui.scale;if(Math.abs(dx)+Math.abs(dy)>5)moved=true;if(!moved)return;const position={x:Math.max(18,start.x+dx),y:Math.max(18,start.y+dy)};this.ui.positions.set(key,position);node.setAttribute('transform','translate('+position.x+' '+position.y+')');};
    const end=()=>{node.removeEventListener('pointermove',move);node.removeEventListener('pointerup',end);node.removeEventListener('pointercancel',end);this.dragging=false;this.persist();if(!moved)void this.select(key);else this.renderGraph();};
    node.addEventListener('pointermove',move);node.addEventListener('pointerup',end);node.addEventListener('pointercancel',end);
  }
  objectStatus(object){
    const status=reviewStatus(this.project,object);
    const opinion=status.opinion||object.review_state;
    const opinionText={correct:'审核认为正确',accepted:'模型独立审查通过',incorrect:'审核发现错误',inconclusive:'未形成确定结论',changes_requested:'要求进一步处理',unreviewed:'尚未独立审核',submitted:'待审'}[opinion]||opinion||'尚无可确认意见';
    const admission={accepted:'已准入',admitted:'已准入',not_submitted:'尚未提交审核',not_admitted:'尚未准入'}[object.admission_state]||status.admission;
    return badge(label(object.work_state||object.status||'open'))+badge('审核意见：'+opinionText)+
      badge('程序准入：'+admission,['accepted','admitted'].includes(object.admission_state)?'admitted':'')+
      badge('有效性：'+label(object.validity||'current'))+
      '<p class="wb-muted">已保存不等于已准入；模型独立审核不等于形式化认证。以上为该对象当前状态，原文固定于所选版本。</p>';
  }
  async select(key,ref=null,{quote='',quoteArtifactId=null,referenceCleared=false,preserveTab=false}={}){
    const object=this.model.objects.find(o=>o.key===key);if(!object)return;
    this.ui.selected=key;this.selection=ref||object.ref||selectionReference(object);this.detail={object,reference:this.selection};
    this.quote=quote||'';this.quoteArtifactId=quoteArtifactId;this.referenceCleared=referenceCleared;if(!preserveTab){this.ui.graphInteracted=true;this.openProof();this.showProofSidebar('reader');this.centerPending=true;}this.persist();this.renderPanels();this.renderReference();
    const ticket=++this.readTicket,project=this.project,reference=this.selection;
    const reader=this.element.querySelector('[data-wb-reader]');reader.innerHTML='<h3>'+h(object.title)+'</h3><div data-wb-current-status>'+this.objectStatus(object)+'</div><div data-wb-new-version></div><p>正在读取固定原文…</p>';
    try{
      const key=project.id+' '+refKey(reference);let content=this.readCache.get(key);
      if(!content){
        const statement=object.synthetic_view_root?{body:project.problem}:await this.api.detail(project,object,reference);
        if(ticket!==this.readTicket||this.project.id!==project.id)return;
        const sameVersion=reference.kind===object.ref?.kind&&reference.id===object.ref?.id&&reference.revision===object.ref?.revision;
        const boundProof=rows(this.model.proofs).find(proof=>proof.conclusion_ref?.kind===reference.kind&&proof.conclusion_ref?.id===reference.id&&proof.conclusion_ref?.revision===reference.revision);
        const drafts=rows(statement.draft_artifact_refs).length?statement.draft_artifact_refs:sameVersion?rows(object.draft_artifact_refs):[];
        const lastDraft=drafts.at(-1);
        const proofId=statement.proof_artifact_id||boundProof?.proof_artifact_id||
          (sameVersion?object.proof_artifact_id:null)||reference.artifact_id||lastDraft?.artifact_id||lastDraft?.id||statement.body_artifact_id||
          (sameVersion?object.body_artifact_id:null);
        const proof=proofId?await this.api.artifact(project,proofId):null;
        content={statement,proof,proofId};this.readCache.set(key,content);
      }
      if(ticket!==this.readTicket||this.project.id!==project.id)return;
      let selectedObject=object;
      if(reference.revision&&object.revision!==reference.revision){
        const proof=rows(this.model.proofs).find(proof=>proof.conclusion_ref?.kind===reference.kind&&proof.conclusion_ref?.id===reference.id&&proof.conclusion_ref?.revision===reference.revision);
        const projected=this.model.objects.find(o=>o.ref?.kind===reference.kind&&o.ref?.id===reference.id&&o.ref?.revision===reference.revision);
        selectedObject={...object,...content.statement,key:projected?.key||'unprojected:'+refKey(reference),ref:reference,revision:reference.revision,
          admission_state:proof?.admission_state||'not_admitted',review_state:proof?.review_state||'unreviewed',candidate_id:proof?.candidate_id};
      }
      this.detail={object:selectedObject,reference,...content,proofPage:0};this.renderReader();
    }catch(error){if(ticket===this.readTicket)reader.innerHTML='<h3>'+h(object.title)+'</h3><div data-wb-current-status>'+this.objectStatus(object)+'</div><p class="wb-error">精确原文读取失败：'+h(error.message)+'</p>'+action('retry-detail','重试原文')+'<p>不以摘要替代缺失原文。</p>';}
  }
  renderReader(){
    const d=this.detail;if(!d?.statement)return;
    const o=d.object,ref=d.reference;
    const exact=d.statement.exact_statement||d.statement.statement||d.statement.text||readable(d.statement.body);
    const rawProof=d.proof?.text||'',pageSize=18000,pages=Math.max(1,Math.ceil(rawProof.length/pageSize));
    const page=Math.min(d.proofPage||0,pages-1),proofSlice=rawProof.slice(page*pageSize,(page+1)*pageSize);
    this.element.querySelector('[data-wb-reader]').innerHTML=
      '<div class="wb-reader-heading"><h3 title="'+h(o.title)+'">'+h(short(o.title,80))+'</h3>'+badge(ref.revision?'固定 v'+ref.revision:'固定原始制品')+'</div><div data-wb-current-status>'+this.objectStatus(o)+'</div><div data-wb-new-version></div><div data-wb-direct-relations></div>'+
      '<h4>精确陈述</h4><div class="wb-math-text" data-wb-exact>'+statementsHtml({...d.statement,title:o.title,math_kind:d.statement.math_kind||o.math_kind,review_state:o.review_state,assurance:o.assurance,exact_statement:exact,mathematical_statements:d.statement.mathematical_statements||(ref.revision===o.ref?.revision?o.mathematical_statements:[])},this.renderText)+'</div>'+
      '<div class="wb-actions">'+action('quote','引用所选原文')+action('discuss','围绕此版本讨论')+action('annotate','批注此版本')+
      (o.kind==='candidate'?action('review','请求独立审核'):'')+
      (['node','fact'].includes(ref.kind)&&!o.synthetic_view_root?action('challenge','正式标记争议'):'')+'</div>'+
      '<h4>证明或原始材料</h4>'+
      (pages>1?'<p class="wb-muted">长原文分段阅读，第 '+(page+1)+' / '+pages+' 段；边界可能切开公式，可打开原始文件连续阅读。</p><div class="wb-actions">'+action('proof-prev','上一段')+action('proof-next','下一段')+'</div>':'')+
      (d.proof?.text!==null&&d.proof?.text!==undefined?'<div class="wb-math-text" data-wb-proof-text>'+this.renderText(proofSlice)+'</div>':
        d.proof?'<a href="'+h(d.proof.url)+'" target="_blank" rel="noopener">打开原始文件 · '+h(d.proof.media_type)+'</a>':empty('此对象未关联可读取的证明制品；陈述本身不是完整证明。'))+
      (d.proof?.url?'<p><a href="'+h(d.proof.url)+'" target="_blank" rel="noopener">查看／保存原文</a></p>':'')+
      '<div data-wb-evidence></div>'+
      dataDetails('来源与固定对象引用',{reference:ref,statement:d.statement});
    this.refreshReaderEvidence({force:true});
    this.renderReference();
  }
  refreshReaderEvidence({force=false}={}){
    const d=this.detail,region=this.element.querySelector('[data-wb-evidence]');if(!d?.statement||!region)return;
    const ref=d.reference;
    const same=(a,b)=>a?.id===b?.id&&a?.kind===b?.kind&&a?.revision===b?.revision;
    const projected=this.model.objects.find(o=>same(o.ref,ref));
    const ownProof=rows(this.model.proofs).find(p=>ref.kind==='candidate'&&p.candidate_id===ref.id||ref.kind==='fact'&&p.fact_ref?.id===ref.id);
    const o=projected||{...d.object,...(ownProof?{admission_state:ownProof.admission_state,review_state:ownProof.review_state,validity:ownProof.validity}:{} )};
    const related=this.model.edges.filter(e=>e.from===o.key||e.to===o.key);
    const proofs=rows(this.model.proofs).filter(p=>same(p.conclusion_ref,ref));
    const ids=new Set([...proofs.map(p=>p.candidate_id),o.kind==='candidate'?ref.id:o.candidate_id,ownProof?.candidate_id].filter(Boolean));
    const reviews=rows(this.project.reviews).filter(r=>ids.has(r.candidate_id));
    const annotations=rows(this.project.annotations).filter(a=>a.selection_ref?same({...a.selection_ref,revision:a.selection_ref.revision??1},{...ref,revision:ref.revision??1}):a.node_id===ref.id&&a.anchor?.node_revision===ref.revision);
    const signature=JSON.stringify([ref,o,related,proofs,reviews,annotations]);
    if(!force&&signature===this.readerEvidenceSignature)return;
    this.readerEvidenceSignature=signature;d.object=o;
    const links=this.element.querySelector('[data-wb-direct-relations]');
    if(links){
      const rel=directProofRelations(this.model,o.key);
      const group=(title,edges,incoming)=>'<section><h4>'+title+' <span>'+edges.length+'</span></h4>'+(edges.length?edges.map(e=>{const key=e.mathematical?(incoming?e.from:e.to):(e.from===o.key?e.to:e.from),other=this.model.objects.find(x=>x.key===key);return '<button class="wb-direct-link" data-wb-select="'+h(key)+'"><strong>'+h(short(other?.title||'对象未定位',58))+'</strong><small>'+h(e.relationLabel)+' · v'+h(other?.revision||'?')+'</small></button>';}).join(''):'<p class="wb-muted">未记录'+title+'。</p>')+'</section>';
      replacePreservingView(links,'<div class="wb-direct-relations">'+group('直接前提',rel.incoming,true)+group('直接后续',rel.outgoing,false)+(rel.ownership.length?group('目标归属',rel.ownership,false):'')+'</div>');
    }
    const reader=this.element.querySelector('[data-wb-reader]'),scrollTop=reader.scrollTop;
    const expanded=[...region.querySelectorAll('details[open]')].map(e=>e.querySelector('summary')?.textContent);
    region.innerHTML=
      '<h4>前提与后续成果</h4><p class="wb-muted">同一证明组的前提需同时满足；不同证明分别核对。已读来源不自动成为逻辑前提。</p>'+
      (related.length?related.map(e=>{const other=this.model.objects.find(x=>x.key===(e.to===o.key?e.from:e.to));return '<article class="wb-entry"><span>'+h(e.relationLabel)+' · '+(e.mathematical?(e.to===o.key?'前置依据':'后续成果'):'目标关系')+'</span> <button data-wb-select="'+h(other?.key||'')+'">'+h(short(other?.title||'对象未定位',70))+' · v'+h(other?.revision)+'</button>'+dataDetails('绑定版本、前提组与原始依据',e.record)+'</article>';}).join(''):empty('尚无明确保存的前后置关系。'))+
      dataDetails('假设、符号和适用范围',{assumptions:o.assumptions,symbols:o.symbols,scope:o.scope})+
      dataDetails('声明前提与适用条件',d.statement.declared_premises||o.declared_premises||[])+
      '<h4>该成果的不同证明</h4>'+proofs.map((proof,i)=>'<article class="wb-entry"><strong>证明 '+(i+1)+' · '+h(proof.admission_state==='accepted'?'已准入':'尚未准入')+'</strong><p>各证明的前提分别成组，同组需同时满足。</p>'+action('open-proof','打开这份冻结证明','data-candidate="'+h(proof.candidate_id)+'"')+dataDetails('证明版本与共同前提组',proof)+'</article>').join('')+
      (rows(o.draft_artifact_refs).length?'<h4>保存的研究草稿</h4>'+rows(o.draft_artifact_refs).map(r=>'<p>'+action('open-draft','读取草稿原文','data-artifact="'+h(r.id||r.artifact_id)+'"')+' · '+h(r.filename||r.id||r.artifact_id)+'</p>').join(''):'')+
      '<h4>独立审核与准入</h4>'+ (reviews.length?reviews.map(r=>'<article class="wb-entry"><strong>独立审核结论：'+h(r.verdict==='accepted'?'通过':r.verdict==='inconclusive'?'未形成确定结论':r.verdict||r.state)+'</strong><p>审核报告校验：'+h(r.report_validated===true?'通过':r.report_validated===false?'未通过':'未记录')+'</p>'+dataDetails('原始意见、问题与准入原因',r)+'</article>').join(''):empty('尚无关联审核记录。'))+
      '<h4>版本与原文批注</h4>'+rows(o.historical_versions).map((v,i)=>action('version','打开历史 v'+(v.revision||v.ref?.revision||'?'),'data-index="'+i+'"')).join('')+
      annotations.map(a=>'<article class="wb-entry"><small>'+h(a.author||a.author_kind||'人类')+' · v'+h(a.selection_ref?.revision||a.anchor?.node_revision)+' · '+h(timeLabel(a.created_at))+'</small>'+
        (a.selection_ref?.quote?'<blockquote>'+h(a.selection_ref.quote)+'</blockquote>':'')+'<div>'+this.renderText(a.body)+'</div>'+action('annotation-feedback','转为给领研猫的建议','data-id="'+h(a.id)+'"')+'</article>').join('');
    region.querySelectorAll('details').forEach(e=>{if(expanded.includes(e.querySelector('summary')?.textContent))e.open=true;});
    reader.scrollTop=scrollTop;
    const status=this.element.querySelector('[data-wb-current-status]');if(status)status.innerHTML=this.objectStatus(o);
  }
  renderReference(){
    const ref=this.selection;
    this.element.querySelector('[data-wb-reference]').innerHTML=ref&&!this.referenceCleared?'<strong>'+h(short(this.detail?.object.title||ref.id,65))+' · '+(ref.revision?'固定 v'+h(ref.revision):'固定原始制品')+'</strong>'+(this.quote?'<blockquote>'+h(this.quote)+'</blockquote>':''):'未引用对象；讨论围绕当前项目';
  }
  intentHelp(){
    const intent=this.element.querySelector('[data-wb-intent]').value,run=latestRun(this.project);
    this.element.querySelector('.wb-annotation-send').hidden=intent!=='annotation';
    const text={discussion:!run||['ended','paused'].includes(run.state)?'仅启动本次独立解释，最长 5 分钟；不恢复原研究。':'独立解释可读取所选原文，不进入领研猫待办。',normal:'等待领研猫下一次可接收输入的整体安排。保存不代表已读或采纳。',urgent:'请求中断并续接；终止未确认时不保证送达。紧急不等于自动采纳。',annotation:'批注保留在所选版本；默认不改变研究。','edit-math':'填写修改后的完整数学问题。发送后先核对新旧内容，再保存问题版本。','edit-description':'填写修改后的完整研究说明。数学假设和量词应保留在数学问题里。'};
    this.element.querySelector('[data-wb-intent-help]').textContent=text[intent];
  }
  renderSide(){
    const content=this.element.querySelector('[data-wb-side-content]');
    if(this.ui.side==='feedback')content.innerHTML=feedbackHtml(this.project,this.renderText);
    else {
      const messages=rows(this.project.discussions).flatMap(d=>rows(d.messages).map(m=>({...m,discussion_id:d.id})));
      content.innerHTML=(messages.length?messages.slice(-40).map(m=>'<article class="wb-discussion '+(['user','local-owner'].includes(m.author)?'human':'')+'"><small>'+h(['user','local-owner'].includes(m.author)?'你':'独立解释')+' · '+h(timeLabel(m.created_at))+'</small><div>'+this.renderText(m.text||m.content||m.error?.message||'等待执行结果')+'</div>'+(!['user','local-owner'].includes(m.author)?action('discussion-feedback','转为建议','data-id="'+h(m.id)+'"'):'')+'</article>').join(''):empty('可边读成果边提问。讨论内容不会自动进入主研究。'))+
        rows(this.project.interactions).filter(i=>i.state!=='ended').map(i=>'<p>独立解释 '+h(label(i.state))+action('cancel-explanation','停止这次解释','data-id="'+h(i.id)+'"')+'</p>').join('');
    }
    this.element.querySelector('[data-wb-send-status]').innerHTML=this.error?'<p class="wb-error">'+h(this.error)+'</p>':this.notice?'<p>'+h(this.notice)+'</p>':'';
  }
  renderPending(){
    const pending=[...this.api.pending.values()].filter(p=>p.project_id===this.project.id);
    this.element.querySelector('[data-wb-pending]').innerHTML=pending.length?'<div class="wb-warning">有请求结果待确认；不会自动重新提交。'+pending.map(p=>'<p>'+h(timeLabel(p.created_at))+' '+action('retry-write','使用原请求核对／重试','data-signature="'+h(p.signature)+'"')+'</p>').join('')+'</div>':'';
  }
  clearGraphFilters(){
    this.ui.query='';this.ui.filter='all';this.ui.collapsed.clear();this.ui.focus=false;this.ui.direct=false;
    const search=this.element.querySelector('[data-wb-search]'),filter=this.element.querySelector('[data-wb-filter]');
    if(search)search.value='';if(filter)filter.value='all';
  }
  async openLinkedReference(ref){
    if(!ref.kind){
      const review=rows(this.project.reviews).find(r=>r.id===ref.id);
      if(review){if(review.report_artifact_id)return this.openLinkedReference({kind:'artifact',id:review.report_artifact_id});this.openModal('审核记录','<p>'+h(label(review.verdict||review.state))+'</p>'+dataDetails('具体审核依据',review),'关闭',async()=>{});return;}
      if(rows(this.project.candidates).some(r=>r.id===ref.id))ref={...ref,kind:'candidate',revision:1};
    }
    if(ref.kind==='artifact'){
      const project=this.project,ticket=++this.readTicket,artifact=await this.api.artifact(project,ref.id);
      if(this.project.id!==project.id||ticket!==this.readTicket)return;
      this.openModal('原始研究材料',(artifact.text!=null?'<div class="wb-session-output">'+this.renderText(artifact.text)+'</div>':empty('该材料为附件，可打开原文件阅读。'))+'<p><a href="'+h(artifact.url)+'" target="_blank" rel="noopener">打开原文件</a></p>','关闭',async()=>{});return;
    }
    const object=this.model.objects.find(o=>o.ref?.id===ref.id&&(!ref.kind||o.ref.kind===ref.kind)&&(!ref.revision||o.ref.revision===ref.revision))||this.model.byRef.get(ref.id);
    if(object){this.clearGraphFilters();await this.select(object.key,ref.kind&&ref.revision?ref:null);this.element.querySelector('[data-wb-proof]')?.scrollIntoView({block:'nearest'});return;}
    const proof=rows(this.model.proofs).find(x=>x.candidate_id===ref.id||x.fact_ref?.id===ref.id);
    if(proof?.conclusion_ref){await this.openLinkedReference(proof.conclusion_ref);return;}
    if(['node','candidate','fact','artifact'].includes(ref.kind)){
      const project=this.project,ticket=++this.readTicket,reference={...ref,revision:ref.revision||1},statement=await this.api.detail(project,ref,reference);
      if(this.project.id!==project.id||ticket!==this.readTicket)return;
      this.openModal('关联原文',statementsHtml({...statement,title:statement.title||'关联数学材料'},this.renderText)+dataDetails('固定来源',reference),'关闭',async()=>{});return;
    }
    throw new Error('当前快照没有对应的固定成果版本，请从原始记录核对来源。');
  }
  openProblemEditor(field,{text,source='direct_edit',afterSave}={}){
    const p=this.project,spec=p.problem_spec||{},mathematical=field==='math_statement',current=spec[field]||'';
    this.openModal(mathematical?'编辑数学问题':'编辑研究说明','<p>'+h(mathematical?'完整保留数学对象、假设、量词和目标结论。数学问题修改会生成新问题版本。':'时间、工具限制、输出要求和偏好写在这里。修改说明不自动改变数学问题。')+'</p><details><summary>当前内容</summary><div class="wb-original">'+this.renderText(current||'尚未填写')+'</div></details><label>'+h(mathematical?'新的完整数学问题':'新的完整研究说明')+'<textarea name="body" rows="10" '+(mathematical?'required':'')+'>'+h(text??current)+'</textarea></label>','保存修改',async form=>{
      const value=form.elements.body.value.trim();if(value===current)throw new Error('内容尚未改变。');
      await this.api.problemSpec(p,{[field]:value},source);if(this.project.id!==p.id)return;afterSave?.();this.notice=mathematical?'数学问题已保存为新版本。':'研究说明已更新。';
    });
  }
  openModal(title,body,submitText,handler){
    this.modalTitle=title;this.modalHandler=handler;this.modalProject=this.project.id;this.modalBusy=false;this.detailSession=null;
    this.element.querySelector('[data-wb-modal-content]').innerHTML='<h3>'+h(title)+'</h3>'+body;
    for(const [name,value] of Object.entries(this.modalDrafts?.[title]||{})){const field=this.element.querySelector('[data-wb-modal-form]').elements.namedItem(name);if(field&&typeof field.value==='string')field.value=value;}
    this.element.querySelector('[data-wb-modal-error]').textContent='';
    this.element.querySelector('[data-wb-modal-submit]').textContent=submitText;
    const dialog=this.element.querySelector('[data-wb-dialog]');if(!dialog.open)dialog.showModal();
    dialog.querySelector('textarea,input,button')?.focus();
  }
  async submitModal(){
    if(this.modalBusy)return;if(this.modalProject!==this.project.id)return;
    const button=this.element.querySelector('[data-wb-modal-submit]'),projectId=this.project.id,handler=this.modalHandler;this.modalBusy=true;button.disabled=true;
    try{const result=await handler(this.element.querySelector('[data-wb-modal-form]'));if(this.project.id!==projectId)return;if(result!==false){delete this.modalDrafts[this.modalTitle];this.element.querySelector('[data-wb-dialog]').close();this.persist();}await this.refresh();}
    catch(error){if(this.project.id===projectId)this.element.querySelector('[data-wb-modal-error]').textContent=error.message;}
    finally{this.modalBusy=false;button.disabled=false;if(this.project.id===projectId)this.renderPending();}
  }
  async send(){
    if(this.busy)return;
    const input=this.element.querySelector('[data-wb-input]'),body=input.value.trim(),intent=this.element.querySelector('[data-wb-intent]').value;
    if(!body)return;const project=this.project,selection=this.selection&&!this.referenceCleared?quoteReference(this.selection,this.quote,this.quoteArtifactId):null;
    if(this.offline){this.error='当前断线，内容保留为未发送草稿。';this.renderSide();return;}
    const directEdit=intent==='discussion'?explicitProblemEdit(body):null;
    if(directEdit){const field=Object.keys(directEdit)[0];this.openProblemEditor(field,{text:directEdit[field],source:'conversation_edit',afterSave:()=>{if(input.value.trim()===body)input.value='';this.persist();}});return;}
    if(intent==='discussion'&&projectModelsPaused(project)){this.error='本项目已暂停模型任务。讨论草稿已保存；仍可编辑题面、记录建议和批注。';this.persist();this.renderSide();return;}
    if(intent==='edit-math'||intent==='edit-description'){
      this.openProblemEditor(intent==='edit-math'?'math_statement':'research_description',{text:body,source:'conversation_edit',afterSave:()=>{if(input.value.trim()===body)input.value='';this.persist();}});return;
    }
    this.busy=true;this.error='';this.element.querySelector('[data-wb-action="send"]').disabled=true;
    try{
      if(intent==='discussion'){
        const sent=await this.classic.discuss(this.conversation,body,{selectionRef:selection,authorize:async()=>true});
        if(!sent)return;if(this.project.id===project.id){this.notice='独立解释已提交；不会改变主研究。';this.ui.side='discussion';}
      }else if(intent==='annotation'){
        if(!selection)throw new Error('先选择需要批注的固定成果版本。');
        if(this.element.querySelector('[data-wb-annotation-send]').checked)await this.api.annotateAndFeedback(project,selection,body);
        else await this.api.annotate(project,selection,body);
        if(this.project.id===project.id)this.notice='批注已保存到固定版本。';
      }else{
        await this.api.feedback(project,{body,priority:intent,selection});
        if(this.project.id===project.id){this.notice='意见已保存；投递、领研猫处置和实际变化请查看“我的意见”。';this.ui.side='feedback';}
      }
      if(this.project.id===project.id&&input.value.trim()===body)input.value='';
      if(this.project.id===project.id){this.persist();await this.refresh();}
    }catch(error){if(this.project.id===project.id)this.error=error.message;}
    finally{this.busy=false;const send=this.element.querySelector('[data-wb-action="send"]');if(send)send.disabled=false;if(this.project.id===project.id){this.renderSide();this.renderPending();}}
  }
  async click(event){
    const select=event.target.closest('[data-wb-select]');if(select){this.clearGraphFilters();void this.select(select.dataset.wbSelect);return;}
    const linked=event.target.closest('[data-wb-linked-ref]');if(linked){try{await this.openLinkedReference(JSON.parse(linked.dataset.wbLinkedRef));}catch(error){this.error=error.message;this.renderSide();}return;}
    const result=event.target.closest('[data-wb-result-ref]');if(result){
      const id=result.dataset.wbResultRef,collection=result.dataset.collection,o=this.model.byRef.get(id);
      if(o&&['nodes','candidates','facts'].includes(collection))void this.select(o.key);
      else {const record=rows(this.project[collection]).find(r=>r.id===id),trace=rows(this.project.feedback_traces).flatMap(t=>rows(t.result_refs)).find(r=>r.id===id&&r.collection===collection);
        this.openModal('关联执行对象',dataDetails('动作原始回执与当前状态',trace)+
          (record?dataDetails('当前'+({tasks:'任务',pending_assignments:'改派安排',sessions:'会话',routes:'路线'}[collection]||'对象')+'记录',record):empty('当前快照未包含该对象；仅可确认已有的关联回执。')),'去研究记录',async()=>{this.ui.tab='records';this.renderPanels();});}return;}
    const button=event.target.closest('[data-wb-action]');if(!button)return;
    const a=button.dataset.wbAction,p=this.project;
    if(a==='model-settings'){this.openModelSettings?.(p.id);return;}
    if(a==='attention-jump'){
      const target=button.dataset.target;
      if(target==='records'){this.ui.tab='records';this.renderPanels();}
      const section=this.element.querySelector(target==='control'?'[data-wb-header]':'[data-wb-'+target+']');
      if(section){section.scrollIntoView({block:'start',behavior:'smooth'});section.setAttribute('tabindex','-1');section.focus({preventScroll:true});}return;
    }
    if(a==='project-control'){
      const type=button.dataset.type;
      const text=type==='pause'?'暂停本项目全部猫猫与论文整理任务。已保存内容仍可浏览和编辑，截止时间继续计算。':type==='resume'?'恢复本项目允许继续的任务；仍需遵守人工审批、路线禁令和截止时间。':'停止本项目本轮研究与论文整理。已保存成果保留。';
      this.openModal(button.textContent,'<p>'+h(text)+'</p>',button.textContent,async()=>{const previous=p.project_control;if(type==='pause'||type==='stop'){p.project_control={...previous,state:type==='pause'?'pausing':'stopping'};replacePreservingView(this.element.querySelector('[data-wb-header]'),headerHtml(p,this.renderText,this.getCodexStatus(p.id)));}try{return await this.api.control(p,type);}catch(error){p.project_control=previous;replacePreservingView(this.element.querySelector('[data-wb-header]'),headerHtml(p,this.renderText,this.getCodexStatus(p.id)));throw error;}});return;
    }
    if(a==='delivery-start'){this.openModal('整理当前成果','<p>以当前已保存的数学成果建立一份新的交付记录。完整成果整理为中英文稿；未完成的问题保留在阶段报告中。既有研究不会重新运行。</p>','开始整理',()=>this.api.deliveryStart(p));return;}
    if(a==='delivery-retry'){const regenerate=button.dataset.state==='completed';this.openModal(regenerate?'重新生成英文稿':'局部重试','<p>'+h(regenerate?'使用本次冻结的中文主稿，重新生成英文稿并更新英文后续检查；中文稿与已完成的研究保留。':'使用本次交付冻结的来源，只重试选中步骤；已完成的研究和其他成功步骤不会重新运行。')+'</p>',regenerate?'重新生成英文稿':'重试这一步',()=>this.api.deliveryRetry(p,button.dataset.step));return;}
    if(a==='reveal-folder'){try{await this.api.reveal(p,button.dataset.folder);}catch(error){this.error=error.message;this.renderSide();}return;}
    if(a==='open-proof-page'){this.openProof();return;}
    if(a==='close-proof'){this.closeProof();return;}
    if(a==='proof-reader'||a==='proof-discussion'){const mode=a==='proof-reader'?'reader':'discussion';this.showProofSidebar(this.ui.proofSidebar===mode&&button.closest('.wb-proof-heading')?'none':mode);this.centerGraphSelection();return;}
    if(a==='close-inspector'){this.showProofSidebar('none');this.element.querySelector('[data-wb-action="proof-reader"]').focus();return;}
    if(a==='edit-problem'||a==='edit-description'){this.openProblemEditor(a==='edit-problem'?'math_statement':'research_description');return;}
    if(a==='session-detail'){
      const session=rows(p.sessions).find(s=>s.id===button.dataset.id);if(!session)return;
      this.openModal('猫猫研究详情','<div data-wb-session-live>'+sessionHtml(p,session,this.renderText)+'</div>','关闭',async()=>{});this.detailSession=session.id;return;
    }
    if(a==='candidate-jump'){await this.openLinkedReference({kind:'candidate',id:button.dataset.id,revision:1});return;}
    if(a==='target-advice'||a==='reframe'){
      const target=a==='reframe'?{kind:'project',id:p.id}:{kind:button.dataset.kind,id:button.dataset.id};
      const record=target.kind==='session'?rows(p.sessions).find(s=>s.id===target.id):target.kind==='route'?rows(p.routes).find(s=>s.id===target.id):target.kind==='open_question'?rows(p.open_questions).find(s=>s.id===target.id):null;
      const title=a==='reframe'?'请求重新梳理目标':target.kind==='session'?'针对这只猫提出建议':target.kind==='route'?'针对研究路线提出建议':target.kind==='open_question'?'针对开放疑点提出建议':'建议整体研究路线';
      this.openModal(title,'<p>'+h(record?.title||record?.focus||record?.statement||'由领研猫审视并处理，普通建议在下一次整体安排时接收。')+'</p>'+
        (a==='reframe'?'<p>写明希望重新检查的重点。此请求不自动修改数学命题；人工模式下新分工仍需审核。</p>':'')+
        '<label>建议内容<textarea name="body" required rows="6" placeholder="例如：请检查当前特殊情形是否有助于推进一般结论…"></textarea></label><label>处理时机<select name="priority"><option value="normal">下一次整体安排</option><option value="urgent">紧急处理</option></select></label>','提交给领研猫',async form=>{await this.api.feedback(p,{body:form.elements.body.value.trim(),priority:form.elements.priority.value,target,...(a==='reframe'?{kind:'reframe_goal'}:{})});this.ui.side='feedback';this.notice='请求已保存，可在“我的意见”查看领研猫回应和实际变化。';});return;
    }
    if(a==='mode'){
      const run=latestRun(p);if(!run){this.notice='研究开始时会请你选择本轮分工方式。';this.renderSide();return;}
      this.openModal('研究参与方式','<label>参与方式<select name="mode"><option value="delegated" '+(run.mode!=='collaborative'?'selected':'')+'>自动安排分工</option><option value="collaborative" '+(run.mode==='collaborative'?'selected':'')+'>每轮分工由我批准</option></select></label><p>人工参与：领研猫形成下一轮分工后，先等你审核，再执行新的安排。已有任务不会因模式切换被打断，等待时间仍计入本轮总时长。</p><p>已接收的分工按原模式处理，新分工使用新模式。切换为自动安排分工后，旧待批方案失效，领研猫重新安排。</p>','保存参与方式',form=>this.api.mode(p,form.elements.mode.value));return;
    }
    if(a==='plan-decision'){
      const plan=rows(p.planning_proposals).find(x=>x.id===button.dataset.id);if(!plan)return;
      const decision=button.dataset.decision,titles={approve:'批准本轮分工',reject:'退回领研猫重新安排',defer:'暂缓这份安排'};
      this.openModal(titles[decision],'<p>'+h(plan.reason||'本轮安排')+'</p><p>'+h(decision==='approve'?'批准的是这份固定版本的分工，数学成果仍按独立审核流程处理。':decision==='reject'?'附上需要调整的地方，领研猫重新形成分工。':'新的安排暂不执行，保留已有任务和研究记录。')+'</p><label>意见<textarea name="reason" rows="4" '+(decision==='reject'?'required':'')+'></textarea></label>',titles[decision],form=>this.api.planningDecision(p,plan,decision,form.elements.reason.value));return;
    }
    if(a==='tab'){this.ui.tab=button.dataset.tab;this.update(p,this.conversation,this.offline);this.persist();return;}
    if(a==='side'){this.ui.side=button.dataset.side;this.update(p,this.conversation,this.offline);this.persist();return;}
    if(a==='send'){void this.send();return;}
    if(a==='clear-ref'){this.referenceCleared=true;this.quote='';this.quoteArtifactId=null;this.persist();this.renderReference();return;}
    if(a==='close-dialog'){if(!this.modalBusy)this.element.querySelector('[data-wb-dialog]').close();return;}
    if(a==='zoom-in'||a==='zoom-out'){this.zoomGraph(this.ui.scale*(a==='zoom-in'?1.2:1/1.2));return;}
    if(a==='zoom-reset'){this.zoomGraph(1);this.centerGraphSelection();return;}
    if(a==='fit'){const box=this.element.querySelector('[data-wb-graph]');this.ui.scale=fitGraphScale(this.layout,box.clientWidth,box.clientHeight,{min:.15,max:1});this.renderGraph();box.scrollLeft=0;box.scrollTop=0;this.persist();return;}
    if(a==='arrange'){this.ui.positions.clear();this.centerPending=true;this.renderGraph();this.persist();return;}
    if(a==='focus'||a==='direct'){const enabled=!this.ui[a];if(enabled)this.clearGraphFilters();this.ui[a]=enabled;this.ui[a==='focus'?'direct':'focus']=false;this.ui.graphInteracted=true;this.ui.scale=1;this.centerPending=true;this.renderGraph();this.persist();return;}
    if(a==='collapse'){if(this.ui.collapsed.has(this.ui.selected))this.ui.collapsed.delete(this.ui.selected);else this.ui.collapsed.add(this.ui.selected);this.renderGraph();this.persist();return;}
    if(a==='reader-toggle'){this.element.querySelector('.wb-proof-workspace').classList.toggle('reader-wide');this.showProofSidebar('reader');this.centerGraphSelection();return;}
    if(a==='quote'){
      const selection=globalThis.getSelection();
      const region=[...this.element.querySelectorAll('[data-wb-exact],[data-wb-proof-text]')].find(r=>r.contains(selection?.anchorNode)&&r.contains(selection?.focusNode));
      if(!region||!selection.toString().trim()){this.notice='请先在同一段精确陈述或证明中选中原文。';this.renderSide();return;}
      this.quote=selection.toString().trim();this.quoteArtifactId=region.matches('[data-wb-proof-text]')?this.detail.proofId:null;this.referenceCleared=false;this.persist();this.renderReference();this.showProofSidebar('discussion');return;
    }
    if(a==='discuss'||a==='annotate'){this.element.querySelector('[data-wb-intent]').value=a==='discuss'?'discussion':'annotation';this.showProofSidebar('discussion');this.intentHelp();this.element.querySelector('[data-wb-input]').focus();return;}
    if(a==='retry-detail'){void this.select(this.ui.selected,this.selection);return;}
    if(a==='proof-prev'||a==='proof-next'){if(this.detail?.proof){this.detail.proofPage=Math.max(0,(this.detail.proofPage||0)+(a==='proof-next'?1:-1));this.renderReader();}return;}
    if(a==='open-proof'){
      const candidate=rows(p.candidates).find(c=>c.id===button.dataset.candidate);if(!candidate)return;
      const object=this.detail.object,reference={kind:'candidate',id:candidate.id,revision:1,problem_version:candidate.problem_version};
      const ticket=++this.readTicket;this.selection=reference;this.quote='';this.quoteArtifactId=null;this.referenceCleared=false;this.persist();this.renderReference();
      try{const statement=await this.api.detail(p,candidate,reference),proof=await this.api.artifact(p,candidate.proof_artifact_id);if(ticket!==this.readTicket||this.project.id!==p.id)return;this.detail={object:{...object,...candidate,key:object.key,kind:'candidate'},reference,statement,proof,proofId:candidate.proof_artifact_id,proofPage:0};this.renderReader();}
      catch(error){this.error=error.message;this.renderSide();}return;
    }
    if(a==='open-draft'){
      const id=button.dataset.artifact,ticket=++this.readTicket,object=this.detail.object,reference={kind:'artifact',id,problem_version:p.problem_version};
      try{const statement=await this.api.detail(p,object,reference),proof=await this.api.artifact(p,id);if(ticket!==this.readTicket||this.project.id!==p.id)return;this.selection=reference;this.quote='';this.quoteArtifactId=null;this.referenceCleared=false;this.detail={object:{...object,kind:'artifact',admission_state:'not_admitted',review_state:'未审草稿'},reference,statement,proof,proofId:id,proofPage:0};this.persist();this.renderReader();}
      catch(error){this.error=error.message;this.renderSide();}return;
    }
    if(a==='latest-version'){void this.select(this.ui.selected);return;}
    if(a==='version'){const o=this.model.objects.find(o=>o.key===this.ui.selected),v=o?.historical_versions[Number(button.dataset.index)];if(v)void this.select(o.key,v.ref||{...o.ref,...v});return;}
    if(a==='more-records'){this.ui.recordsLimit+=50;this.renderPanels();return;}
    if(a==='discussion-feedback'||a==='annotation-feedback'||a==='summary-feedback'){
      const item=a==='discussion-feedback'?rows(p.discussions).flatMap(d=>rows(d.messages)).find(m=>m.id===button.dataset.id):
        a==='annotation-feedback'?rows(p.annotations).find(m=>m.id===button.dataset.id):rows(button.dataset.kind==='memory'?p.memory_entries:p.display_summaries).find(m=>m.id===button.dataset.id);
      this.openModal('确认交给领研猫的文字','<p>只发送你确认的文字与引用，不自动转发整段对话。</p><textarea name="body" required rows="7">'+h(readable(item?.body||item?.text||item?.content||item?.summary))+'</textarea>','提交普通建议',async form=>this.api.feedback(p,{body:form.elements.body.value,selection:item?.selection_ref||this.selection,annotation:a==='annotation-feedback'?item:undefined}));return;
    }
    if(a==='answer'){
      const q=rows(p.human_questions).find(q=>q.id===button.dataset.id);if(!q)return;
      this.openModal('明确答复研究员','<div>'+this.renderText(readable(q.body||q.question||q.text))+'</div><textarea name="body" rows="5" required></textarea><p>关联本条待答问题及其版本。只有明确答复才可能恢复允许的研究。</p>','提交答复',form=>this.api.answer(p,q,form.elements.body.value));return;
    }
    if(a==='withdraw'||a==='escalate'){
      const trace=rows(p.feedback_traces).find(t=>t.command_id===button.dataset.id);if(!trace)return;
      this.openModal(a==='withdraw'?'撤回意见':'升级为紧急','<p>'+h(readable(trace.body))+'</p><p>'+(a==='withdraw'?'未投递时原子撤回；已提供给调用的内容保留历史，并发送撤回通知。':'原意见与紧急通知由后端关联处理；不会复制一条无关联建议。')+'</p><label>原因<textarea name="reason" rows="3"></textarea></label>','确认'+(a==='withdraw'?'撤回':'升级'),form=>this.api.feedbackAction(p,trace,a,form.elements.reason.value));return;
    }
    if(a==='retry-write'){this.openModal('核对原请求','<p>使用原路径、原内容及同一个请求标识；不会改成新请求。</p>','核对／重试',()=>this.api.retry(button.dataset.signature));return;}
    if(a==='cancel-explanation'){this.openModal('停止本次独立解释','<p>只停止选中的解释执行；原研究状态不变。</p>','停止解释',()=>this.classic.cancelInteraction(p,button.dataset.id));return;}
    if(a==='start'){try{const conversation=await this.classic.request('/api/conversations/'+encodeURIComponent(this.conversation.id));if(await this.classic.start(conversation)!==false)await this.refresh();}catch(error){this.error=error.message;this.renderSide();}return;}
    if(a==='route'){
      const route=rows(p.routes).find(r=>r.id===button.dataset.id);if(!route)return;
      const reopening=route.status==='blocked',affected=rows(p.sessions).filter(s=>s.route_id===route.id);
      this.openModal(reopening?'解除路线禁令':'禁止研究路线','<p>'+h(route.title||route.objective||route.id)+'</p><p>'+h(reopening?'只允许后续重新安排，不复活旧任务。':'禁止后续派发并取消相关执行；不会自动撤回此前数学成果。')+'</p>'+dataDetails('当前关联会话',affected)+'<label>原因<textarea name="reason" required rows="3"></textarea></label>',reopening?'解除禁令':'禁止此路线',form=>this.api.command(p,reopening?'reopen_route':'prohibit_route',{target:{kind:'route',id:route.id,revision:route.revision},payload:{reason:form.elements.reason.value},applyAt:'immediate'}));return;
    }
    if(a==='review'){
      if(projectModelsPaused(p)){this.error='本项目已暂停审核等模型任务，请先明确恢复研究。';this.renderSide();return;}
      const o=this.detail?.object;if(o?.kind!=='candidate')return;
      this.openModal('对冻结候选请求独立审核','<p>'+h(o.title)+'</p><p>审核绑定该候选的冻结版本；计入允许的研究预算。模型意见不等于形式化认证。</p>','请求审核',()=>this.api.command(p,'request_review',{target:controlReference(p,{kind:'candidate',id:o.id}),payload:{snapshot_hash:o.snapshot_hash,review_kind:'mathematical',reason:'人类请求审核所选冻结候选'},applyAt:'immediate'}));return;
    }
    if(a==='challenge'){
      const ref=this.selection;
      this.openModal('正式标记成果争议','<p>对象：'+h(this.detail?.object.title)+' · v'+h(ref.revision)+'</p><p>这会更新该成果的当前有效性及相关依赖检查，不等于已经证明结论错误，也不自动启动付费审核。</p><textarea name="reason" required rows="5" placeholder="请填写具体争议依据"></textarea>','标记为有争议',form=>this.api.command(p,'challenge_evidence',{target:controlReference(p,ref),payload:{reason:form.elements.reason.value,request_paid_review:false},applyAt:'immediate'}));return;
    }
    if(a==='revise'){
      this.openModal('提出问题修订','<label>当前问题</label><div class="wb-original">'+this.renderText(p.problem)+'</div><label>修订后的完整问题<textarea name="body" rows="9" required>'+h(p.problem)+'</textarea></label>','生成影响预览',async form=>{
        const text=form.elements.body.value;if(text===p.problem)throw new Error('问题尚未改变。');
        const value=await this.api.revisePreview(p,text),preview=value.preview||value;if(!preview.proposal)throw new Error('服务未返回可确认的修订预览。');
        this.openModal('确认新旧问题和影响','<h4>当前 v'+h(p.problem_version)+'</h4><div class="wb-original">'+this.renderText(p.problem)+'</div><h4>修订后</h4><div class="wb-original">'+this.renderText(text)+'</div>'+dataDetails('后台影响预览',preview.impact||preview.summary||preview),'确认创建新版本',()=>this.api.reviseCommit(p,preview));return false;
      });return;
    }
    if(a==='limits'){
      const run=latestRun(p),limits=run?.limits||{};
      this.openModal('本轮时间与预算','<p>修改当前轮次的后续控制边界；实际生效以服务器回执为准。时长留空表示不设时长上限。</p><div class="wb-setting-grid">'+
        '<label>本轮总时长（分钟）<input name="duration" type="number" min="0.0166666667" step="any" max="525600" value="'+h(limits.duration_seconds?limits.duration_seconds/60:'')+'"></label>'+
        '<label>受管伙伴猫上限<input name="partners" type="number" min="0" max="16" value="'+h(limits.max_partners??5)+'"></label>'+
        '<label>顾问周期（分钟）<input name="advisor" type="number" min="1" value="'+h((limits.advisor_interval_seconds??2400)/60)+'"></label>'+
        '<label>总调用上限<input name="calls" type="number" min="1" value="'+h(limits.max_invocations??1000)+'"></label></div>','保存设置',form=>this.api.limits(p,{duration_seconds:form.elements.duration.value?Number(form.elements.duration.value)*60:null,max_partners:Number(form.elements.partners.value),advisor_interval_seconds:Number(form.elements.advisor.value)*60,max_invocations:Number(form.elements.calls.value)}));return;
    }
  }
}
