import {escapeHtml as h, label, readable, latestRun, publicEventText} from './research-v2-state.js';
import {rows, short, timeLabel, activityCards, roles, feedbackTrace, canAnswerQuestion} from './whiteboard-model.js';
import {patchLiveContent} from './stable-view.js';

const empty=text=>'<p class="wb-empty">'+h(text)+'</p>';
const action=(id,text,attrs='')=>'<button type="button" data-wb-action="'+h(id)+'" '+attrs+'>'+h(text)+'</button>';
const badge=(text,tone='')=>'<span class="wb-badge '+tone+'">'+h(text)+'</span>';
const raw=(title,value)=>'<details class="wb-raw"><summary>'+h(title)+'</summary><pre>'+h(JSON.stringify(value,null,2))+'</pre></details>';
export const mathKind=value=>({problem:'问题',theorem:'定理',lemma:'引理',proposition:'命题',claim:'命题',conjecture:'猜想',corollary:'推论',assumption:'假设',counterexample:'反例',proof_gap:'证明缺口',question:'问题',definition:'定义'}[value]||'数学陈述');
export function mathematicalStatements(object){
  const statements=rows(object.mathematical_statements||object.statements);
  if(statements.length)return statements.map((s,i)=>typeof s==='string'?{exact_statement:s,math_kind:object.math_kind,index:i}:{...s,index:i});
  return [{title:object.title,math_kind:object.math_kind||object.node_type,exact_statement:object.exact_statement||object.statement||object.claim||'',revision:object.revision,review_state:object.review_state,assurance:object.assurance}];
}
export function statementsHtml(object,renderText){
  return mathematicalStatements(object).map(s=>'<section class="wb-statement">'+
    '<div class="wb-statement-heading">'+badge(mathKind(s.math_kind||s.kind||s.type))+(s.title?'<strong>'+h(s.title)+'</strong>':'')+
    badge(s.review_state?({accepted:'审查通过',submitted:'待审',unreviewed:'未审',changes_requested:'需补充论证',rejected:'审查未通过'}[s.review_state]||s.review_state):s.assurance==='model_reviewed'?'模型审查通过':s.assurance==='formally_checked'?'形式检查通过':'本条陈述尚未独立审查')+'</div>'+
    '<div class="wb-math-text">'+renderText(s.exact_statement||s.statement||s.text||'尚未提供精确陈述')+'</div>'+
    (s.assumptions?'<details><summary>假设与适用条件</summary>'+renderText(readable(s.assumptions))+'</details>':'')+'</section>').join('');
}
export function problemHtml(p,renderText){
  const spec=p.problem_spec||{},state=spec.normalization_state;
  // The launch prompt is a source, never guessed to be a pure mathematics statement.
  const pending=['pending','unparsed'].includes(state),exact=pending?'':spec.math_statement||'',description=spec.research_description||'';
  return '<section class="wb-problem wb-card" data-wb-stable="problem-card"><div class="wb-section-heading"><div><span class="wb-eyebrow">我们正在研究</span><h3>数学问题 '+badge('v'+(spec.problem_version||p.problem_version||1))+'</h3></div>'+action('edit-problem','编辑问题')+'</div>'+
    (exact?'<div class="wb-math-text wb-problem-math">'+renderText(exact)+'</div>':empty('数学题面尚待整理。可打开研究说明查看原始输入，或直接填写准确的数学问题。'))+
    (state==='model_structured'?'<p class="wb-muted">领研猫整理的题面，尚未独立核对与原始输入的数学等价性。</p>':state&&['pending','needs_review','unparsed'].includes(state)?'<p class="wb-muted">领研猫尚在整理数学题面；请确认对象、假设、量词和结论齐全。</p>':'')+
    '<details class="wb-description" data-wb-stable="problem-description"><summary>研究说明与限制</summary><div>'+renderText(description||'尚未单独记录研究说明。')+'</div>'+action('edit-description','编辑说明')+
    '<details data-wb-stable="original-input"><summary>查看启动时的原始输入</summary><div class="wb-original">'+renderText(spec.original_input||p.problem||'未记录')+'</div></details></details></section>';
}
export function sessionActivity(p,session){
  const events=rows(p.events).filter(e=>{
    const a=e.payload||e.data||e;return a.session_id===session.id&&!a.discussion_id&&/^(activity\.|tool\.|message\.)/.test(e.type||a.type||'');
  });
  return events.map(e=>{const a=e.payload||e.data||e;return {...e,at:e.created_at||e.occurred_at||e.at||a.created_at,type:e.type||a.type,text:publicEventText(e),activity:a};});
}
export function sessionHtml(p,session,renderText){
  const events=sessionActivity(p,session),notes=rows(p.nodes).filter(n=>n.author===session.id&&n.node_type==='note');
  const labels={'activity.delta':'公开进展','activity.error':'执行异常','message.completed':'返回内容','message.delta':'返回内容','tool.started':'工具开始','tool.completed':'工具完成'};
  return '<div class="wb-session-output"><p class="wb-muted">会话 '+h(session.id)+' · '+h(roles[session.role]||session.role)+'</p><p>以下是 CLI 已公开返回的进展与执行记录。没有新输出时，保留最后返回时间。</p>'+
    (events.length?events.map(e=>'<article class="wb-entry"><small>'+h(timeLabel(e.at))+' · '+h(labels[e.type]||'公开活动')+(e.activity.stale?' · 历史调用':'')+'</small>'+
      (e.text?'<div>'+renderText(e.text)+'</div>':'<p>'+h(({command_execution:'命令执行',mcp_tool_call:'工具调用',web_search:'检索',file_change:'文件变更'}[e.activity.tool]||e.activity.tool||'活动'))+' · '+h(label(e.activity.status||'started'))+'</p>')+'</article>').join(''):empty('此会话尚无可读取的公开 CLI 输出。'))+
    (notes.length?'<details><summary>此前保存的数学草稿</summary>'+notes.map(n=>'<article class="wb-entry"><small>'+h(timeLabel(n.created_at))+'</small>'+renderText(readable(n.body))+'</article>').join('')+'</details>':'')+'</div>';
}
function sessionName(p,session){const peers=rows(p.sessions).filter(s=>s.role==='partner'&&s.run_id===session.run_id);return session.role==='partner'?'伙伴猫 '+(peers.findIndex(s=>s.id===session.id)+1):roles[session.role]||session.role;}
export function laboratoryHtml(p,renderText){
  const run=latestRun(p),cards=activityCards(p),major=cards.filter(c=>['main','partner','reviewer'].includes(c.session.role)),minor=cards.filter(c=>!['main','partner','reviewer'].includes(c.session.role));
  const card=c=>{
    const s=c.session,route=rows(p.routes).find(r=>r.id===s.route_id),candidate=rows(p.candidates).find(r=>r.id===s.candidate_id),events=sessionActivity(p,s),last=events.at(-1);
    const goal=s.role==='reviewer'?(candidate?.exact_statement||candidate?.claim||s.candidate_snapshot?.claim):c.currentGoal||c.task?.focus||s.focus;
    return '<article class="wb-cat-card '+(c.active?'is-working':'')+'"><div class="wb-cat-heading"><span class="wb-cat-icon" aria-hidden="true">'+(s.role==='reviewer'?'◎':'♧')+'</span><div><h4>'+h(sessionName(p,s))+'</h4><small>'+h(c.active?'正在工作':label(s.state))+' · '+h(c.phase)+'</small></div></div>'+
      '<div class="wb-cat-goal">'+renderText(goal||'等待登记当前研究目标')+'</div>'+
      (s.role==='reviewer'&&s.candidate_id?'<small>冻结候选 '+h(short(s.candidate_id,16))+(candidate?.snapshot_hash?' · 快照 '+h(short(candidate.snapshot_hash,12)):'')+'</small>':'')+
      (s.focus&&goal!==s.focus?'<p class="wb-muted">研究焦点：'+h(s.focus)+'</p>':'')+
      (route?'<p class="wb-muted">方向：'+h(route.title||route.objective||route.id)+'</p>':'')+
      (last?'<small>最近公开输出 '+h(timeLabel(last.at))+'</small>':'<small>尚无公开输出</small>')+
      (c.pending.length?'<p class="wb-next-task">下一项任务有 '+c.pending.length+' 项安排，当前任务完成后生效。</p>':'')+
      '<div class="wb-actions">'+action('session-detail','查看详情','data-id="'+h(s.id)+'"')+
      (['main','partner'].includes(s.role)?action('target-advice','提出建议','data-kind="session" data-id="'+h(s.id)+'"'):'')+'</div></article>';
  };
  const liveStates=['queued','pending','running','reviewing','active','in_progress'],queueByCandidate=new Map();
  const candidateById=id=>rows(p.candidates).find(c=>c.id===id);
  const add=(id,extra={})=>{if(id)queueByCandidate.set(id,{...queueByCandidate.get(id),id,candidate:candidateById(id),...extra});};
  for(const review of rows(p.reviews)){
    const session=rows(p.sessions).find(s=>s.id===review.reviewer_session_id),candidate=candidateById(review.candidate_id);
    if(liveStates.includes(review.state||review.status)&&(review.run_id===run?.id||session?.run_id===run?.id||candidate?.run_id===run?.id))add(review.candidate_id,{review,session});
  }
  for(const c of cards.filter(c=>c.session.role==='reviewer'&&(c.active||c.session.state==='active')))add(c.session.candidate_id,{session:c.session,running:true});
  for(const job of rows(p.background_jobs))if(['review','reviewer'].includes(job.kind)&&liveStates.includes(job.status||job.state)&&(!job.run_id||job.run_id===run?.id))add(job.candidate_id,{job});
  for(const candidate of rows(p.candidates))if(candidate.run_id===run?.id&&['submitted','queued','reviewing'].includes(candidate.status)&&!rows(p.reviews).some(r=>r.candidate_id===candidate.id&&!liveStates.includes(r.state||r.status)&&r.verdict))add(candidate.id);
  const candidates=[...queueByCandidate.values()];
  const queueHtml=candidates.length?candidates.map(({id,job,candidate,session,review,running})=>'<article class="wb-queue-item"><span>'+h(running||['running','reviewing','active'].includes(review?.state||job?.status)?'正在审核':'等待审核')+'</span><div>'+renderText(candidate?.exact_statement||candidate?.claim||session?.candidate_snapshot?.claim||job?.objective||'审核对象尚未关联')+'<small>冻结候选 '+h(short(id,18))+(candidate?.snapshot_hash?' · 快照 '+h(short(candidate.snapshot_hash,12)):'')+(candidate?.problem_version?' · 题目 v'+h(candidate.problem_version):'')+'</small></div>'+action('candidate-jump','阅读成果','data-id="'+h(id)+'"')+'</article>').join(''):empty('当前没有登记的待审成果。');
  return '<section class="wb-card wb-laboratory"><div class="wb-section-heading"><div><span class="wb-eyebrow">猫猫实验室</span><h3>现在在做什么</h3></div><div class="wb-actions">'+action('target-advice','建议整体路线','data-kind="project" data-id="'+h(p.id)+'"')+action('reframe','重新梳理目标')+'</div></div>'+
    '<div class="wb-cat-grid">'+(major.length?major.map(card).join(''):empty('研究开始后，领研猫、伙伴猫与审核猫会在这里显示当前工作。'))+'</div>'+
    '<details class="wb-review-queue" data-wb-stable="review-queue"><summary>审核列表 · '+candidates.length+' 项</summary>'+queueHtml+'</details>'+
    (minor.length?'<details class="wb-support-cats" data-wb-stable="support-cats"><summary>顾问、记忆与展示工作 · '+minor.length+'</summary><div class="wb-cat-grid">'+minor.map(card).join('')+'</div></details>':'')+'</section>';
}
export function planningAction(actionValue,p){
  const a=actionValue,type=a.type||a.action||a.kind||'',session=rows(p.sessions).find(s=>s.id===(a.session_id||a.partner_session_id)),target=session?sessionName(p,session):'';
  const names={request_partner:'新增伙伴猫',register_route:'登记研究路线',set_route:'调整研究路线',withdraw_assignment:'撤回伙伴猫后续安排',spawn_partner:'新增伙伴猫',assign_partner:'分配伙伴猫任务',reassign_partner:'调整伙伴猫后续任务',steer_partner:'调整伙伴猫后续任务',close_partner:'停止伙伴猫后续任务',stop_partner:'停止伙伴猫后续任务',continue_research:'继续研究',request_review:'提交审核',submit_candidate:'提交证明候选',set_focus:'调整研究焦点',steer_focus:'调整研究焦点',update_route:'调整研究路线',create_route:'新增研究路线',transition_phase:'下一阶段',end_run:'结束本轮'};
  return {title:(names[type]||'研究安排')+(target?' · '+target:''),body:a.focus||a.objective||a.task?.objective||a.reason||a.summary||a.description||a.text||'',raw:a};
}
export function planningHtml(p,renderText){
  const run=latestRun(p),plans=rows(p.planning_proposals).filter(x=>x.run_id===run?.id&&['pending','deferred'].includes(x.state));
  if(run?.mode!=='collaborative'&&!plans.length)return '';
  return '<section class="wb-card wb-planning"><div class="wb-section-heading"><div><span class="wb-eyebrow">人工参与</span><h3>本轮分工审核</h3></div>'+badge(plans.length?'等待你处理':'尚无待批安排')+'</div>'+
    '<p class="wb-muted">批准后执行新的安排。等待期间，未调整的伙伴猫可在原任务范围内继续；等待时间计入总时长。</p>'+
    (plans.length?plans.map(plan=>'<article class="wb-plan"><div class="wb-section-heading"><strong>'+h(plan.reason||'领研猫提交了下一轮研究安排')+'</strong><small>'+h(timeLabel(plan.created_at))+'</small></div>'+
      '<ol class="wb-plan-actions">'+rows(plan.actions).map(a=>{const item=planningAction(a,p);return '<li><strong>'+h(item.title)+'</strong><div>'+renderText(readable(item.body)||'具体内容见安排依据')+'</div>'+raw('安排依据',item.raw)+'</li>';}).join('')+'</ol>'+
      '<p>领研猫下一步：'+h(plan.main_next_phase==='coordination'?'继续安排':plan.main_next_phase==='research'?'开展研究':plan.main_next_phase||'见本轮安排')+'</p>'+
      (run?.mode==='collaborative'&&run?.state==='running'?'<div class="wb-actions">'+action('plan-decision','通过并执行','class="primary" data-decision="approve" data-id="'+h(plan.id)+'"')+action('plan-decision','附意见退回','data-decision="reject" data-id="'+h(plan.id)+'"')+action('plan-decision','暂缓','data-decision="defer" data-id="'+h(plan.id)+'"')+'</div>':empty('当前研究状态不允许执行安排，保留该分工记录。'))+'</article>').join(''):empty('领研猫形成新分工后，会在这里等待你的审核。'))+'</section>';
}
export function questionsHtml(p,renderText){
  const run=latestRun(p),questions=rows(p.human_questions).filter(q=>q.run_id===run?.id&&['pending','open','waiting'].includes(q.status||q.state));
  if(run?.mode!=='collaborative'&&!questions.length&&run?.state!=='waiting_human')return '';
  return '<section class="wb-card"><h3>猫猫提问箱 '+badge(questions.length+' 项')+'</h3>'+(questions.length?questions.map(q=>'<article class="wb-entry">'+renderText(readable(q.body||q.question||q.text))+(q.context||q.reason?'<p class="wb-muted">'+h(readable(q.context||q.reason))+'</p>':'')+(canAnswerQuestion(p,q)?action('answer','回答这条问题','data-id="'+h(q.id)+'"'):empty('这条问题保留为记录，当前研究状态不允许通过答复恢复。'))+'</article>').join(''):empty('目前没有需要你回答的问题。'))+'</section>';
}
function objectButton(ref,p,caption='查看关联成果'){
  if(!ref)return '';const r=typeof ref==='string'?{id:ref}:ref;
  return '<button type="button" data-wb-linked-ref="'+h(JSON.stringify(r))+'">'+h(caption)+'</button>';
}
export function doubtsHtml(p,renderText){
  const doubts=rows(p.open_questions),fallback=doubts.length?[]:rows(p.nodes).filter(n=>['proof_gap','question'].includes(n.node_type)).map(n=>({...n,question:n.exact_statement||readable(n.body),node_ref:{kind:'node',id:n.id,revision:n.revision},state:n.work_state}));
  const all=[...doubts,...fallback];
  return '<section class="wb-card"><div class="wb-section-heading"><h3>开放疑点</h3>'+badge(all.filter(q=>!['resolved','closed'].includes(q.state||q.status)).length+' 项待解决')+'</div><p class="wb-muted">保存具体数学障碍及其处理记录。尚待核对并不代表已证明错误。</p>'+
    (all.length?all.map(q=>'<article class="wb-doubt wb-entry"><div class="wb-section-heading"><h4>'+h(q.title||'待解决的问题')+'</h4>'+badge({resolved:'已解决',closed:'已关闭',open:'待解决',working:'正在处理',blocked:'暂时受阻'}[q.state||q.status]||label(q.state||q.status||'open'))+'</div>'+renderText(q.question||q.exact_statement||q.statement||q.description||readable(q.body))+
      (q.conditions?'<p>适用条件：'+h(readable(q.conditions))+'</p>':'')+'<p class="wb-muted">影响：'+h(readable(q.impact||q.affected_scope)||'尚未单独说明')+' · 负责：'+h(q.owner_session_id?sessionName(p,rows(p.sessions).find(s=>s.id===q.owner_session_id)||{role:'研究猫'}):'未分配')+'</p>'+
      (rows(q.issues).length?'<ul class="wb-issues">'+q.issues.map(issue=>'<li>'+renderText(typeof issue==='string'?issue:issue.issue||issue.description||issue.detail||issue.message||readable(issue))+'</li>').join('')+'</ul>':'')+
      '<div class="wb-actions">'+objectButton(q.node_ref||q.source_ref||(q.candidate_id?{kind:'candidate',id:q.candidate_id,revision:1}:null),p)+rows(q.affected_refs||q.evidence_refs).map(r=>objectButton(r,p,'关联依据')).join('')+(q.source_artifact_id?objectButton({kind:'artifact',id:q.source_artifact_id},p,'阅读原稿或审核报告'):'')+action('target-advice','针对疑点建议','data-kind="open_question" data-id="'+h(q.id)+'"')+'</div>'+(q.resolution?'<div>'+renderText(readable(q.resolution))+'</div>':'')+raw('疑点来源与处理沿革',q)+'</article>').join(''):empty('尚未单独登记开放疑点。'))+'</section>';
}
export const outcomeLabel=kind=>({mathematically_refuted:'数学路线被否定',refuted:'数学路线被否定',mathematical_failure:'数学路线被否定',blocked:'暂时受阻',stalled:'暂时受阻',inconclusive:'暂时受阻',abandoned:'暂时放弃',human_stopped:'人工停止',user_stop:'人工停止',runtime_error:'运行故障',runtime_failure:'运行故障',environment_error:'运行故障',time_limit:'时间耗尽',budget_limit:'预算耗尽',interrupted:'研究中断'}[kind]||'结束原因待核对');
export function failuresHtml(p,renderText){
  const results=rows(p.route_outcomes);
  return '<section class="wb-card"><h3>失败路线与未完成尝试</h3><p class="wb-muted">区分数学方法被否定、暂时受阻与执行中断。结束路线后，已有成果和依据继续保留。</p>'+
    (results.length?results.map(r=>'<article class="wb-entry wb-route-outcome"><div class="wb-section-heading"><h4>'+h(r.title||r.objective||'已记录的尝试')+'</h4>'+badge(outcomeLabel(r.outcome||r.kind||r.reason_kind))+'</div>'+renderText(r.statement||r.objective||r.goal||r.description||'')+
      (r.method?'<p><strong>尝试方法</strong> '+h(readable(r.method))+'</p>':'')+(r.conditions?'<p><strong>当时条件</strong> '+h(readable(r.conditions))+'</p>':'')+
      '<div><strong>没有继续的原因</strong>'+renderText(readable(r.reason||r.conclusion)||'尚未记录具体原因')+'</div>'+
      (r.scope?'<p><strong>结论适用范围</strong> '+h(readable(r.scope))+'</p>':'')+
      '<div class="wb-actions">'+rows(r.evidence_refs).map(ref=>objectButton(ref,p,'查看依据')).join('')+(r.source_artifact_id?objectButton({kind:'artifact',id:r.source_artifact_id},p,'阅读原稿'):'')+rows(r.result_refs||r.useful_result_refs).map(ref=>objectButton(ref,p,'留下的成果')).join('')+action('target-advice','建议重新尝试','data-kind="'+(r.route_id?'route':'route_outcome')+'" data-id="'+h(r.route_id||r.id)+'"')+'</div>'+raw('尝试条件、原稿与历史',r)+'</article>').join(''):empty('尚未单独保存失败或中断的路线。不会根据任务耗时推断数学失败。'))+'</section>';
}
export function timelineItems(p){
  const names={'planning.proposed':'领研猫提出分工','planning.decided':'人工处理分工','problem.edit_requested':'提交问题或说明修改','feedback.queued':'收到人类建议','feedback.changed':'人类建议处理更新','turn.applied':'猫猫保存本轮研究进展','run.created':'研究开始','run.started':'研究开始','run.ended':'本轮结束','run.paused':'研究暂停','run.resumed':'恢复研究','run.stopped':'停止研究','run.mode_changed':'参与模式变更','problem_spec.updated':'问题或说明已修改','problem.updated':'数学问题已修改','planning_proposal.created':'领研猫提出分工','planning_proposal.approved':'人工批准分工','planning_proposal.rejected':'人工退回分工','planning_proposal.deferred':'人工暂缓分工','candidate.submitted':'提交数学成果审核','review.completed':'审核完成','fact.admitted':'成果通过审核并保存','session.created':'猫猫会话建立','task.created':'研究任务建立','task.completed':'本项研究结束','feedback.created':'收到人类建议','feedback.responded':'领研猫回应建议','node.created':'保存数学成果','node.updated':'数学成果更新','human_question.created':'猫猫提出问题','human_question.answered':'收到人工答复','route.created':'新增研究路线','route.prohibited':'禁止研究路线','cycle.transitioned':'领研猫进入下一阶段'};
  const decisionTitles={approved:'人工批准分工',rejected:'人工退回分工',deferred:'人工暂缓分工',stale:'原分工已失效'};
  const events=rows(p.events).filter(e=>e.type!=='planning.output_received'&&!/^(activity\.|tool\.|message\.|usage\.|session.bound|execution\.)/.test(e.type||'')).map(e=>{
    const a=e.payload||e.data||{},proposal=a.proposal||a.planning_proposal||a;
    const created=['planning.proposed','planning_proposal.created'].includes(e.type);
    const decided=e.type==='planning.decided'||/^planning_proposal\.(approved|rejected|deferred)$/.test(e.type||'');
    const decision=decided?(proposal.state||({approve:'approved',reject:'rejected',defer:'deferred'}[proposal.decision])||(e.type||'').split('.')[1]):null;
    return {id:e.event_id||e.id||'event:'+e.seq,at:e.created_at||e.occurred_at||e.at,
      title:decisionTitles[decision]||names[e.type]||({candidate:'数学成果',review:'成果审核',feedback:'人类意见',route:'研究路线',run:'运行状态',planning:'分工安排',problem:'研究问题',memory:'记忆保存',advisor:'顾问检查',display:'展示更新'}[(e.type||'').split('.')[0]])||'研究记录更新',
      body:(decided?proposal.decision_reason:null)||a.summary||a.reason||a.objective||a.title||'',
      refs:[a.node_ref,a.statement_ref,a.candidate_id?{kind:'candidate',id:a.candidate_id,revision:1}:null].filter(Boolean),session_id:a.session_id,
      planId:created||decided?proposal.proposal_id||proposal.planning_proposal_id||proposal.id:null,planEvent:created?'created':decided?'decided':null,planDecision:decision,raw:e};
  });
  // Creation is immutable history. A later decision adds a separate milestone;
  // it must never replace the earlier proposal, even after old events age out.
  for(const plan of rows(p.planning_proposals)){
    if(!events.some(e=>e.planId===plan.id&&e.planEvent==='created'))events.push({id:'plan-created:'+plan.id,at:plan.created_at,title:'领研猫提出分工',body:plan.reason||'',planId:plan.id,planEvent:'created',session_id:plan.session_id,raw:plan});
    if(decisionTitles[plan.state]&&plan.decided_at&&!events.some(e=>e.planId===plan.id&&e.planEvent==='decided'&&e.planDecision===plan.state))events.push({id:'plan-decision:'+plan.id+':'+plan.state,at:plan.decided_at,title:decisionTitles[plan.state],body:plan.decision_reason||'',planId:plan.id,planEvent:'decided',planDecision:plan.state,raw:plan});
  }
  return events.sort((a,b)=>(Date.parse(b.at)||0)-(Date.parse(a.at)||0));
}
export function timelineHtml(p,renderText,limit=30){
  const events=timelineItems(p);
  return '<section class="wb-card"><h3>实验室时间线</h3><p class="wb-muted">记录研究、成果、审核和人工参与。连续 CLI 输出可在对应猫猫详情中阅读。</p><ol class="wb-timeline">'+
    (events.length?events.slice(0,limit).map(e=>'<li><small>'+h(timeLabel(e.at))+'</small><h4>'+h(e.title)+'</h4>'+(e.body?'<div>'+renderText(readable(e.body))+'</div>':'')+'<div class="wb-actions">'+rows(e.refs).map(r=>objectButton(r,p)).join('')+(e.session_id?action('session-detail','查看猫猫','data-id="'+h(e.session_id)+'"'):'')+'</div>'+raw('事件原始记录',e.raw)+'</li>').join(''):empty('研究开始后，这里记录有意义的进展。'))+'</ol>'+(events.length>limit?action('more-records','显示更早记录'):'')+'</section>';
}
export function reframeHistoryHtml(p,renderText){
  const traces=rows(p.feedback_traces).map(t=>feedbackTrace(p,t)).filter(t=>(t.kind||t.command?.payload?.feedback?.kind||t.command?.payload?.kind)==='reframe_goal');
  return traces.length?'<details class="wb-card" data-wb-stable="reframe-history"><summary>重新梳理目标 · 请求与回应 '+traces.length+'</summary>'+traces.slice().reverse().map(t=>'<article class="wb-entry"><small>'+h(timeLabel(t.created_at))+'</small>'+renderText(t.body)+'<p>领研猫回应：'+h(t.dispositionText)+'</p>'+(t.response?renderText(readable(t.response)):'')+'<p>'+h(t.results.length?'已关联 '+t.results.length+' 项安排变化':'尚未关联安排变化')+'</p></article>').join('')+'</details>':'';
}

// Keep opened descriptions and the reader's position when live snapshots change.
export function replacePreservingView(element,html){
  if(!element||element._wbHtml===html)return;
  if(element.ownerDocument?.createElement){patchLiveContent(element,html);return;}
  const scrollTop=element.scrollTop,scrollLeft=element.scrollLeft;
  const expanded=[...element.querySelectorAll('details[open]')].map(d=>d.dataset.wbStable||d.querySelector('summary')?.textContent);
  element.innerHTML=html;element._wbHtml=html;
  element.querySelectorAll('details').forEach(d=>{if(expanded.includes(d.dataset.wbStable||d.querySelector('summary')?.textContent))d.open=true;});
  element.scrollTop=scrollTop;element.scrollLeft=scrollLeft;
}
