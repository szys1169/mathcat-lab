import {normalizeSnapshot,latestRun,commandFor,uniqueEvents,publicEventText,researchReplies,label,readable,displayResearchPath,researchResultCaption,isTrusted} from './research-v2-state.js';
import {chooseResearchStart} from './research-start.js';

const BASE='/api/v2/research/projects';
const encode=value=>encodeURIComponent(value);
export const isClassicResearch=conversation=>Boolean(conversation?.researchProjectId);
export const needsResearchPolling=project=>Boolean(latestRun(project)&&latestRun(project).state!=='ended'||project?.interactions?.some(row=>row.state!=='ended'));
const roles={main:'领研猫',partner:'伙伴猫',reviewer:'独立审查',discussion:'旁观讨论'};

export function classicProjection(conversation,project,events=[]){
  const run=latestRun(project);const active=run&&run.state!=='ended';const independent=project.interactions.some(row=>row.state!=='ended');
  const details=events.filter(e=>/^(activity|tool|message\.delta)/.test(e.type||'')&&!e.payload?.discussion_id).slice(-80).map(e=>publicEventText(e)||e.type);
  const messages=[...(conversation.messages||[])];
  if(!messages.length)messages.push({id:'v2-problem-'+project.id,role:'user',content:project.problem,createdAt:project.created_at,capabilityId:'rethlas-research'});
  for(const note of researchReplies(project))messages.push({id:'v2-note-'+note.id,role:'assistant',executor:'Codex',researchAgent:roles[note.role],content:note.body,createdAt:note.created_at,activityDetails:['问题版本 '+note.problem_version+'；'+label(note.assurance)+'；'+label(note.validity)]});
  if(run?.state==='waiting_human'&&run.question)messages.push({id:'v2-question-'+run.id+'-'+run.revision,role:'assistant',executor:'MathCat',researchAgent:'等待你的明确答复',content:'研究员需要你判断：\n'+readable(run.question)+'\n请到研究白板点击“答复并继续研究”；普通发送仍是旁观讨论。',createdAt:run.updated_at||project.updated_at||new Date().toISOString()});
  for(const discussion of project.discussions)for(const message of discussion.messages||[])messages.push({id:'v2-discussion-'+message.id,role:['user','local-owner'].includes(message.author)?'user':'assistant',executor:'Codex',researchAgent:'旁观讨论（不改变主研究）',content:message.text||message.content||message.error?.message||'等待执行回执',createdAt:message.created_at,activityDetails:message.author==='assistant'?[label(message.status),...(message.stale?['这是旧问题版本的讨论']:[])]:[]});
  const ids=new Set();const deduped=messages.filter(m=>{if(ids.has(m.id))return false;ids.add(m.id);return true;}).sort((a,b)=>String(a.createdAt||'').localeCompare(String(b.createdAt||'')));
  const status=active||independent?'running':run?.stop_reason==='environment_error'?'failed':run?'completed':'idle';
  const activity={running:Boolean(active||independent),current:independent?'独立旁观解释 · '+label(project.interactions.find(i=>i.state!=='ended')?.state):'MathCat · '+label(run?.state||'created'),lines:[],details,elapsedSeconds:run?.started_at?Math.max(0,Math.floor((Date.now()-Date.parse(run.started_at))/1000)):0};
  if(independent)activity.details.push(...events.filter(e=>e.type==='discussion.activity').slice(-12).map(e=>publicEventText(e)||e.type));
  const graphNodes=project.nodes.map(n=>({id:n.id,kind:n.node_type==='problem'?'theorem':n.node_type==='source'?'source':'claim',label:n.title||n.body,detail:n.body,trust:isTrusted(n)?'verified':'unverified',status:n.validity!=='current'?'blocked':n.work_state==='working'?'active':isTrusted(n)?'verified':'pending',meta:{nodeRevision:n.revision,assurance:n.assurance,validity:n.validity,work_state:n.work_state}}));
  for(const fact of project.facts)if(!graphNodes.some(n=>n.id===fact.id))graphNodes.push({id:fact.id,kind:isTrusted(fact)?'fact':'claim',label:fact.claim,detail:fact.claim,trust:isTrusted(fact)?'fact':'unverified',status:isTrusted(fact)?'verified':'blocked',meta:{assurance:fact.assurance,validity:fact.validity}});
  const graphIds=new Set(graphNodes.map(n=>n.id));const graphEdges=project.edges.map(e=>({id:e.id,from:e.from||e.from_id||e.from_node_id,to:e.to||e.to_id||e.to_node_id,relation:e.edge_type||e.type||'references'})).filter(e=>graphIds.has(e.from)&&graphIds.has(e.to));
  const root=project.nodes.find(n=>n.node_type==='problem')?.id||graphNodes[0]?.id;const graph={rootId:root,nodes:graphNodes,edges:graphEdges};
  const board={id:project.id,conversationId:conversation.id,agent:'mathcat',classicV2:true,project,events,eventCursor:project.event_cursor,revision:project.revision,status,mode:'持续研究',problem:{version:project.problem_version,statement:project.problem,original:project.problem},integration:{structured:true},proofTree:graph,dependencyGraph:graph,routes:[],goals:[],claims:[],tasks:[],workers:[],decisions:[],failedRoutes:[],uncertainties:[],verificationQueue:[],artifacts:[],experiments:[],summary:{},planningSuggestions:[]};
  return {conversation:{...conversation,messages:deduped,status,classicProject:project},board,activity};
}

export class ClassicResearchClient{
  constructor({fetchImpl=fetch,storage,chooseStart=chooseResearchStart}={}){this.chooseStart=chooseStart;this.fetch=(...args)=>fetchImpl.call(globalThis,...args);this.keys=new Map();this.histories=new Map();this.projects=new Map();this.discussionIds=new Map();this.pendingMessages=new Map();try{this.storage=storage||globalThis.sessionStorage;this.pendingMessages=new Map(JSON.parse(this.storage?.getItem('mathcat.2.0.1.classic.pending')||'[]'));}catch{this.storage=null;}}
  savePending(){try{this.storage?.setItem('mathcat.2.0.1.classic.pending',JSON.stringify([...this.pendingMessages]));}catch{}}
  async editProblem(conversation,text,expectedRevision){
    if(!this.pendingEdits){try{this.pendingEdits=new Map(JSON.parse(this.storage?.getItem('mathcat.2.4.classic.problem-edits')||'[]'));}catch{this.pendingEdits=new Map();}}
    const save=()=>{try{this.storage?.setItem('mathcat.2.4.classic.problem-edits',JSON.stringify([...this.pendingEdits]));}catch{}};
    let pending=this.pendingEdits.get(conversation.id);
    if(pending&&(pending.projectId!==conversation.researchProjectId||pending.body.text!==text))throw new Error('上次改题的结果尚待确认，请先用原文重试，避免覆盖其他修改。');
    if(!pending){pending={projectId:conversation.researchProjectId,body:{text,expected_revision:expectedRevision},key:crypto.randomUUID()};this.pendingEdits.set(conversation.id,pending);save();}
    try{
      const result=await this.request('/api/conversations/'+encode(conversation.id)+'/problem-edit',{method:'POST',body:pending.body,idempotencyKey:pending.key});
      this.pendingEdits.delete(conversation.id);save();return result;
    }catch(error){if(error.status>=400&&error.status<500&&error.status!==408){this.pendingEdits.delete(conversation.id);save();}throw error;}
  }
  async request(path,{method='GET',body,idempotencyKey}={}){
    const key=method+' '+path+' '+JSON.stringify(body);if(method!=='GET'&&!this.keys.has(key))this.keys.set(key,crypto.randomUUID());
    const response=await this.fetch(path,{method,headers:{'content-type':'application/json',...(method!=='GET'?{'Idempotency-Key':idempotencyKey||this.keys.get(key)}:{})},body:body===undefined?undefined:JSON.stringify(body)});
    const value=await response.json();if(!response.ok)throw Object.assign(new Error(value.error?.message||value.message||value.error||'操作失败'),{status:response.status});
    if(value.contract&&value.contract!=='mathcat-research/v2')throw new Error('研究服务协议不匹配，未显示虚假回执。');this.keys.delete(key);return value;
  }
  path(project,suffix=''){return BASE+'/'+encode(project.id)+suffix;}
  async load(conversation){
    const project=normalizeSnapshot(await this.request(BASE+'/'+encode(conversation.researchProjectId)+'/snapshot'));let history=this.histories.get(project.id)||{events:[],cursor:Math.max(0,project.event_cursor-1000)};
    if(project.event_cursor>history.cursor){const response=await this.request(this.path(project,'/events?format=json&after='+history.cursor));history={events:uniqueEvents([...history.events,...(response.events||[])]).slice(-1000),cursor:response.event_cursor??history.cursor};this.histories.set(project.id,history);}
    this.projects.set(conversation.id,project);return classicProjection(conversation,project,history.events);
  }
  async command(project,type,{text='',node,payload}={}){return this.request(this.path(project,'/commands'),{method:'POST',body:commandFor(project,type,{node,text,payload,applyAt:['request_review'].includes(type)?'immediate':undefined})});}
  async start(conversation,options){
    if(conversation.researchProjectId&&!conversation.researchRunId&&conversation.researchStartKey){
      if(!conversation.researchStartOptions)throw new Error('未读取上次启动的实际参数，请刷新会话后重试。');
      options??=await this.chooseStart({projectId:conversation.researchProjectId,initial:conversation.researchStartOptions,title:'重试 MathCat 启动',retry:true,locked:true});
      if(!options)return false;
      return this.request('/api/conversations/'+encode(conversation.id)+'/research-start',{method:'POST',body:{start_authorized:true}});
    }
    const project=this.projects.get(conversation.id);if(!project)throw new Error('请先刷新研究记录。');
    options??=await this.chooseStart({projectId:project.id,title:'开始新的研究轮次',initial:{mode:latestRun(project)?.mode||'delegated',maxPartners:latestRun(project)?.limits?.max_partners??5}});
    if(!options)return false;
    return this.request(this.path(project,'/runs'),{method:'POST',body:{start_authorized:true,duration_seconds:options.durationSeconds,mode:options.mode,auto_deliver:options.autoDeliver,wait_policy:'critical_only',limits:{enforcement:'best_effort',max_partners:options.maxPartners}}});
  }
  async discuss(conversation,text,{authorize=async()=>false,selectionRef=null}={}){
    const projection=await this.load(conversation);const project=projection.board.project;const run=latestRun(project);const active=run&&['running','waiting_human'].includes(run.state);
    const delivery=await this.request('/api/research-projects/'+encode(project.id)+'/delivery');
    if(['pausing','paused','stopping','stopped'].includes(delivery.control?.state))throw new Error('本项目已暂停或停止模型任务，讨论草稿已保留。请明确恢复研究后再请求解释。');
    const key=JSON.stringify(selectionRef?[conversation.id,text,selectionRef]:[conversation.id,text]);let pending=this.pendingMessages.get(key);
    if(!pending){
      if(!active&&!['paused','ended',undefined].includes(run?.state))throw new Error('当前正在切换运行状态，请等待回执后再讨论。');
      if(!active&&!await authorize())return false;
      pending={projectId:project.id,text,selectionRef,discussionId:this.discussionIds.get(project.id)||null,owner:active?{kind:'run',id:run.id}:null,expectedState:run?.state||'none',discussionKey:crypto.randomUUID(),interactionKey:crypto.randomUUID(),messageKey:crypto.randomUUID()};
      this.pendingMessages.set(key,pending);this.savePending();
    }
    if(pending.projectId!==project.id)throw new Error('待重试讨论属于另一项目，未启动新调用。');
    if(!pending.discussionId){const value=await this.request(this.path(project,'/discussions'),{method:'POST',body:{},idempotencyKey:pending.discussionKey});pending.discussionId=(value.discussion||value).id;this.discussionIds.set(project.id,pending.discussionId);this.savePending();}
    if(!pending.owner){const value=await this.request(this.path(project,'/interaction-executions'),{method:'POST',body:{discussion_id:pending.discussionId,explicit_authorization:true,expected_run_state:pending.expectedState,limits:{duration_seconds:300}},idempotencyKey:pending.interactionKey});pending.owner={kind:'interaction',id:(value.interaction||value.execution||value).id};this.savePending();}
    await this.request(this.path(project,'/discussions/'+encode(pending.discussionId)+'/messages'),{method:'POST',body:{text:pending.text,reply_mode:'explain',execution_owner:pending.owner,...(pending.selectionRef?{selection_ref:pending.selectionRef}:{})},idempotencyKey:pending.messageKey});this.pendingMessages.delete(key);this.savePending();return true;
  }
  async cancelInteraction(project,id){const row=project.interactions.find(i=>i.id===id);if(!row)throw new Error('解释状态已变化，请刷新。');const result=await this.request(this.path(project,'/interaction-executions/'+encode(id)+'/cancel'),{method:'POST',body:{expected_revision:row.revision}});for(const [key,pending]of this.pendingMessages)if(pending.projectId===project.id&&(pending.owner?.id===id||pending.interactionKey===row.explicit_authorization_ref))this.pendingMessages.delete(key);this.savePending();return result;}
  async revise(project,text,confirmImpact){const value=await this.request(this.path(project,'/problem-revision-previews'),{method:'POST',body:{new_problem:text,change_reason:'用户在原版白板修订',base_problem_version:project.problem_version}});const preview=value.preview||value;if(!preview.proposal)throw new Error('缺少改题影响预览，未修改原题。');if(!await confirmImpact(preview))return false;return this.request(this.path(project,'/commands'),{method:'POST',body:commandFor(project,'replace_problem',{payload:{...preview.proposal,confirmed_impact_preview_id:preview.id},applyAt:'immediate'})});}
}

export function renderClassicResearchBoard(board,{escapeHtml:h,renderText,graphCards}){
  const p=board.project;const run=latestRun(p);const button=(action,title)=>'<button type="button" data-classic-action="'+action+'">'+title+'</button>';
  const stop=run&&run.state!=='ended'?button(run.state==='paused'?'resume':'pause',run.state==='paused'?'恢复':'暂停')+button('stop','中止研究'):button('start','开始研究');
  const artifact=(id,title)=>/^[A-Za-z0-9_-]+$/.test(String(id))?'<a target="_blank" rel="noopener" href="'+BASE+'/'+encode(p.id)+'/artifacts/'+encode(id)+'/content">'+h(title)+'</a>':h(title);
  const head='<div class="board-top"><div class="board-title"><h2>研究白板</h2><p>题目 v'+h(p.problem_version)+' · '+h(label(run?.state||'created'))+'</p></div><span class="board-agent">MathCat</span></div>';
  const status='<section class="board-card wide"><h3>当前研究 <span class="board-actions">'+stop+'</span></h3><p>'+h(researchResultCaption(p,run))+'</p><p>成果位置：'+h(displayResearchPath(p.workspace_path))+'</p>'+(run?.deadline_at?'<p>截止：'+h(new Date(run.deadline_at).toLocaleString())+'；暂停不延长。</p>':'')+(run?.outstanding_cancellation?'<p class="truth-warning">外部调用停止状态待确认，可能有在途费用。</p>':'')+p.interactions.filter(i=>i.state!=='ended').map(i=>'<p>独立解释：'+h(label(i.state))+' <button data-classic-interaction="'+h(i.id)+'">停止这次解释</button></p>').join('')+'</section>';
  const problem='<section class="board-card wide"><h3>问题与约束 '+button('problem','编辑问题')+'</h3><div class="problem-statement">'+renderText(p.problem)+'</div></section>';
  const question=run?.state==='waiting_human'&&run.question?'<section class="board-card question-panel"><div class="question-panel-head"><h3>猫猫提问卡</h3><span class="question-waiting">等你回答</span></div><div class="question-context">'+renderText(readable(run.question))+'</div><div class="board-actions">'+button('answer','答复并继续研究')+'</div><p>答复是给领研猫的明确指令，不是旁观讨论。</p></section>':'';
  const collaboration='<section class="board-card wide human-collaboration-console"><div class="collaboration-console-head"><div><span>Human in the loop</span><h3>人工协作台</h3><p>普通发送默认为旁观讨论，不改变主研究。明确建议和方向调整使用下面的按钮。</p></div></div><div class="board-actions">'+button('suggest','给研究建议')+button('steer','调整研究重点')+'</div><p>领研猫直接研究整题；此内核不使用旧版强制规划与逐路线审批。</p></section>';
  const objects='<section class="board-card wide"><h3>数学对象与版本</h3>'+p.nodes.map(n=>'<article class="research-item"><strong>'+h(n.title||label(n.node_type))+'</strong><p>'+h(label(n.work_state))+' · '+h(label(n.assurance))+' · '+h(label(n.validity))+' · v'+h(n.revision)+'</p><details><summary>查看内容与版本批注</summary><div class="problem-statement">'+renderText(readable(n.body))+'</div>'+p.annotations.filter(a=>a.node_id===n.id).map(a=>'<p>v'+h(a.anchor?.node_revision)+' 批注：'+h(a.body)+'</p>').join('')+'</details><div class="board-actions"><button data-classic-comment="'+h(n.id)+'">批注</button><button data-classic-challenge="'+h(n.id)+'">标记争议</button></div></article>').join('')+'</section>';
  const evidence='<section class="board-card"><h3>候选与审查</h3>'+p.candidates.map(c=>'<article class="research-item"><strong>'+h(c.claim||c.title||c.id)+'</strong><p>'+h(label(c.status))+'</p><button data-classic-review="'+h(c.id)+'">请求独立审查</button></article>').join('')+p.reviews.map(r=>'<details><summary>审查：'+h(label(r.verdict||r.state))+'</summary><pre>'+h(JSON.stringify(r,null,2))+'</pre></details>').join('')+'<p>模型独立审查不等于形式证明。</p></section>';
  const reports='<section class="board-card"><h3>成果与报告</h3>'+p.reports.map(r=>'<p>'+artifact(r.artifact_id,'打开研究报告')+(r.validity&&r.validity!=='current'?' · '+h(label(r.validity))+'（历史快照）':'')+'</p>').join('')+p.artifacts.map(a=>'<p>'+artifact(a.id,a.filename||a.name||'证据文件')+'</p>').join('')+'</section>';
  const receipts='<section class="board-card wide"><h3>意见与控制回执</h3>'+p.commands.slice(-12).reverse().map(c=>'<details><summary>'+h(c.type)+' · '+h(label(c.status))+(c.disposition?' · '+h(label(c.disposition)):'')+'</summary><pre>'+h(JSON.stringify(c,null,2))+'</pre></details>').join('')+'</section>';
  const memory='<section class="board-card wide"><h3>研究记忆</h3>'+p.memories.slice(-15).reverse().map(m=>'<details><summary>'+h(m.title||label(m.kind)||'研究记录')+'</summary><div class="problem-statement">'+renderText(readable(m.text||m.body||m.content))+'</div><pre>'+h(JSON.stringify(m,null,2))+'</pre></details>').join('')+'</section>';
  return head+'<div class="board-grid">'+status+question+problem+collaboration+graphCards+objects+evidence+reports+receipts+memory+'</div>';
}
