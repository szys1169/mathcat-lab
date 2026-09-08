import {escapeHtml as h,label,readable,latestRun} from './research-v2-state.js';
import {rows,short,timeLabel,currentStatus,activityCards,phaseLabel,feedbackTrace,transportLabel,dispositionLabel,canAnswerQuestion} from './whiteboard-model.js';
import {problemHtml} from './whiteboard-lab.js';
import {deliveryStateLabel} from './whiteboard-delivery.js';
import {selectionSummary} from './codex-status.js';

export const empty=text=>'<p class="wb-empty">'+h(text)+'</p>';
export const badge=(text,tone='')=>'<span class="wb-badge '+h(tone)+'">'+h(text)+'</span>';
export const action=(id,text,extra='')=>'<button type="button" data-wb-action="'+h(id)+'" '+extra+'>'+h(text)+'</button>';
export const dataDetails=(title,value)=>'<details class="wb-raw"><summary>'+h(title)+'</summary><pre>'+h(JSON.stringify(value,null,2))+'</pre></details>';
const unavailable=(p,field)=>p.board_errors?.[field]?'<p class="wb-warning">暂时不能读取：'+h(p.board_errors[field])+'。不代表没有记录。</p>':'';
export function headerHtml(p,renderText=h,codexStatus=null){
  const s=currentStatus(p),run=s.run,active=run&&run.state!=='ended',limits=run?.limits||{};
  const projectControl=p.project_control,controlState=projectControl?.state;
  const controlBusy=['pausing','stopping'].includes(controlState);
  const disabled=controlBusy||!projectControl?'disabled':'';
  const control=action('project-control',controlBusy?deliveryStateLabel(controlState):controlState==='paused'?'恢复研究':'暂停全部研究','data-type="'+(controlState==='paused'?'resume':'pause')+'" '+disabled)+
    (active?action('project-control','停止本轮','data-type="stop" '+disabled):action('start','开始新一轮',controlBusy||controlState==='paused'?'disabled':''));
  const remaining=run?.deadline_at?Math.max(0,Math.ceil((Date.parse(run.deadline_at)-Date.now())/60000)):null;
  return '<div data-wb-stable="live-run-heading"><div class="wb-heading"><div><small>MathCat Lab 2.5.1 · 研究白板</small><h2>'+h(short(p.title||p.name||'数学研究',54))+'</h2><p>'+h(s.state)+' · '+h(s.result)+'</p></div><div class="wb-controls">'+action('mode',run?.mode==='collaborative'?'分工由我批准':'自动安排分工')+action('limits','研究设置')+control+'</div></div>'+
    '<div class="wb-run-line"><span>'+h(controlState==='running'?'项目允许研究':controlState?deliveryStateLabel(controlState):'项目控制状态待读取')+'</span><span>'+(run?.deadline_at?'剩余 '+h(remaining)+' 分钟 · 截止 '+h(timeLabel(run.deadline_at)):'未设置截止时间')+'</span><span>伙伴猫上限 '+h(limits.max_partners??'未设置')+'</span><span>顾问每 '+(limits.advisor_interval_seconds===undefined?'未记录':h(limits.advisor_interval_seconds/60)+' 分钟')+'</span><span>更新 '+h(timeLabel(p.board_read_at||p.updated_at))+'</span><span class="wb-codex-selection"><span data-wb-codex-current>项目模型：'+h(selectionSummary(codexStatus))+'</span>'+action('model-settings','模型设置')+'</span></div>'+
    (['pausing','paused','stopping','stopped'].includes(controlState)?'<div class="wb-pause-banner" data-wb-control-status><strong>'+h(controlState==='paused'&&!projectControl.outstanding_cancellation?'本项目全部研究已暂停':projectControl.outstanding_cancellation?'正在确认调用停止':deliveryStateLabel(controlState))+'</strong><p>已保存的成果、草稿和待处理事项保留；可以浏览、下载和编辑。总截止时间继续计算。</p>'+(projectControl.outstanding_cancellation?'<p class="wb-warning">仍有调用等待停止确认，尚不能确认全部已暂停。</p>':'')+'</div>':'')+
    (run?.state==='waiting_human'?'<div class="wb-warning"><strong>研究员正在等待你的明确答复</strong><p>普通讨论与建议不会解除此等待。</p>'+action('attention-jump','查看待答问题','data-target="questions"')+'</div>':'')+
    (s.ended?'<div class="wb-terminal"><strong>本轮已结束 · '+h(s.stop)+'</strong><p>'+h(s.result)+'。成果与历史原文仍可阅读；独立解释不会恢复原研究。</p></div>':'')+
    (s.cancellationUnknown?'<div class="wb-warning"><strong>仍有执行或用量待确认</strong><p>已登记未知交付 '+s.unknown.length+' 次；结束不等于全部外部调用已确认停止，未知用量不按零计算。</p></div>':'')+'</div>'+problemHtml(p,renderText);
}

export function overviewHtml(p,model,renderText) {
  const run=latestRun(p),questions=rows(p.human_questions).filter(q=>q.run_id===run?.id&&['pending','open','waiting'].includes(q.status||q.state));
  const goals=rows(model.goals);
  return '<section class="wb-card"><h3>当前研究目标</h3><div>'+renderText(p.problem||'尚无问题正文')+'</div>'+action('revise','提出改题')+
    '<p class="wb-muted">局部成果入库不代表整题或其他阶段完成。阶段覆盖只按明确记录展示。</p>'+
    (goals.length?goals.map(g=>'<article class="wb-entry"><strong>'+h(g.title||g.exact_statement||g.statement||g.id)+'</strong><p>覆盖判断：'+h(g.status==='reviewed_coverage'?'该目标已有有效审查覆盖':g.status==='historical'?'历史问题版本':'尚无有效覆盖记录')+'</p>'+dataDetails('覆盖证据与问题版本',g)+'</article>').join(''):empty('尚无独立的阶段覆盖记录；不从成果数量推断。'))+'</section>'+
    '<section class="wb-card"><h3>最近保存的成果</h3>'+model.objects.filter(o=>o.math_kind!=='problem'&&o.kind!=='problem').slice(-5).reverse().map(o=>'<button class="wb-result-link" data-wb-select="'+h(o.key)+'"><strong>'+h(short(o.title,110))+'</strong><small>'+h(o.admission_state||label(o.assurance||'unreviewed'))+' · v'+h(o.revision)+' · '+h(timeLabel(o.created_at))+'</small></button>').join('')+'</section>'+
    '<section class="wb-card"><h3>需要你回答</h3>'+unavailable(p,'human_questions')+
    (questions.length?questions.map(q=>'<article class="wb-entry"><div>'+renderText(readable(q.body||q.question||q.text))+'</div><p>'+h(readable(q.context||q.reason))+'</p>'+(canAnswerQuestion(p,q)?action('answer','明确答复','data-id="'+h(q.id)+'"'):empty(run?.state==='paused'?'本輪已暂停；须先明确恢复到待答状态。':'这是保留的问题记录，当前不允许通过答复恢复研究。'))+'</article>').join(''):
      run?.state==='waiting_human'?empty('研究正在等待人类；问题关联暂未就绪，请刷新。普通建议不会解除等待。'):empty('当前没有登记的待答问题。'))+'</section>'+
    '<section class="wb-card"><h3>研究现场</h3>'+activityHtml(p,renderText)+'</section>'+
    '<section class="wb-card"><h3>最近展示摘要</h3>'+unavailable(p,'display_summaries')+
    (rows(p.display_summaries).length?rows(p.display_summaries).slice(-1).map(d=>summaryHtml(d,p,renderText)).join(''):empty('尚无展示摘要。确定状态仍由上方运行记录提供。'))+'</section>';
}
export function activityHtml(p,renderText){
  const cards=activityCards(p),run=latestRun(p),partners=cards.filter(c=>c.session.role==='partner');
  return '<p>受管伙伴猫会话 '+partners.filter(c=>!['closed','ended'].includes(c.session.state)).length+' / '+h(run?.limits?.max_partners??'上限未记录')+'；累计登记 '+partners.length+'。原生内部助手覆盖情况未知，不能计作零。</p>'+
    (cards.length?cards.map(c=>'<details class="wb-entry"><summary>'+h(c.role)+' · '+h(c.active?'调用中':label(c.session.state))+' · '+h(c.phase)+'</summary><p>当前任务：'+h(c.task?.objective||'未记录结构化任务目标')+'</p><p>本次调用目标：'+h(c.currentGoal||'未记录；不从旧交接推断')+'</p><small>目标／调用时间 '+h(timeLabel(c.goalAt))+'</small>'+
      (c.call?'<p>实际调用：'+h(label(c.call.state))+' · 开始 '+h(timeLabel(c.call.started_at))+' · 结束 '+h(timeLabel(c.call.ended_at))+'</p>':'')+
      (c.checkpoint?'<details><summary>上次保存的证明进展 · '+h(timeLabel(c.checkpoint.created_at))+'</summary>'+renderText(readable(c.checkpoint.progress||c.checkpoint.summary||c.checkpoint.body||c.checkpoint))+dataDetails('交接及精确来源',c.checkpoint)+'</details>':empty('尚无公开证明交接。'))+
      (c.pending.length?'<div class="wb-warning"><strong>下次安排，尚未生效</strong>'+c.pending.map(a=>'<p>'+h(readable(a.objective||a.task||a.assignment||a))+'</p>'+dataDetails('安排回执',a)).join('')+'</div>':'')+
      dataDetails('会话与当前任务记录',{session:c.session,task:c.task})+'</details>').join(''):empty('本轮尚无登记会话。')) ;
}
export function summaryHtml(d,p,renderText){
  return '<article class="wb-entry"><small>历史摘要生成于 '+h(timeLabel(d.created_at))+' · 源快照 '+h(d.snapshot_revision??'未记录')+'</small><div>'+renderText(readable(d.text||d.summary||d.body||d.content))+'</div><p class="wb-muted">当前确定状态：'+h(currentStatus(p).state)+'；此摘要中的下一步描述仅代表当时记录。</p>'+dataDetails('摘要来源与当前有效性',d)+'</article>';
}
export function recordsHtml(p,renderText,limit=30){
  const list=(field,title,render)=>'<section class="wb-card"><h3>'+h(title)+'</h3>'+unavailable(p,field)+(rows(p[field]).length?rows(p[field]).slice(-limit).reverse().map(render).join(''):empty('暂无已读取记录。'))+'</section>';
  return list('cycles','领研猫研究与安排',c=>'<article class="wb-entry"><small>'+h(timeLabel(c.created_at||c.at||c.started_at))+'</small><strong>'+h(phaseLabel(c.from_phase||c.from))+' → '+h(phaseLabel(c.to_phase||c.phase||c.to))+'</strong><p>'+h(readable(c.reason))+'</p>'+dataDetails('安排与输入截止记录',c)+'</article>')+
    list('routes','研究路线（组织关系）',r=>'<article class="wb-entry"><strong>'+h(r.title||r.objective||r.id)+'</strong><p>'+h(label(r.status))+' · '+h(readable(r.restriction||r.description))+'</p>'+action('route',r.status==='blocked'?'解除禁令':'禁止此路线','data-id="'+h(r.id)+'"')+dataDetails('路线及影响范围',r)+'</article>')+
    list('pending_assignments','伙伴猫安排与实际应用',a=>'<article class="wb-entry"><strong>'+h(a.applied_at?'已应用于后续任务':'未见应用回执')+'</strong><p>'+h(readable(a.objective||a.task||a.assignment))+'</p><small>'+h(timeLabel(a.applied_at||a.created_at))+'</small>'+dataDetails('完整安排记录',a)+'</article>')+
    list('commands','控制与修改回执',c=>'<article class="wb-entry"><strong>'+h(label(c.type||c.command_type))+' · '+h(transportLabel(c.status))+'</strong><small>'+h(timeLabel(c.created_at))+'</small><p>'+h(readable(c.summary||c.reason))+'</p>'+dataDetails('目标、控制版本与实际回执',c)+'</article>')+
    list('advisories','顾问意见',a=>'<article class="wb-entry"><small>'+h(timeLabel(a.created_at))+' · '+h(a.trigger||a.reason||'触发方式见来源')+'</small><div>'+renderText(readable(a.text||a.body||a.summary||a.content))+'</div>'+dataDetails('来源、范围与回应',a)+'</article>')+
    list('proof_checkpoints','证明交接',c=>'<article class="wb-entry"><strong>'+h(short(c.goal||c.title||'证明交接'))+'</strong><small>'+h(timeLabel(c.created_at))+'</small>'+dataDetails('目标、假设、符号、未完成步骤与原稿',c)+'</article>')+
    list('memory_entries','记忆与尝试',m=>'<article class="wb-entry"><strong>'+h(m.title||m.kind||'研究记忆')+'</strong><small>'+h(timeLabel(m.created_at))+'</small><div>'+renderText(readable(m.text||m.body||m.content||m.summary))+'</div><p class="wb-muted">摘要不替代精确原文；是否可复用须核对适用条件和当前来源状态。</p>'+dataDetails('精确来源、条件与原稿',m)+action('summary-feedback','指出记录问题','data-kind="memory" data-id="'+h(m.id)+'"')+'</article>')+
    list('display_summaries','展示摘要历史',d=>summaryHtml(d,p,renderText)+action('summary-feedback','指出摘要问题','data-kind="display" data-id="'+h(d.id)+'"'))+
    list('background_jobs','后台工作',j=>'<article class="wb-entry"><strong>'+h(j.kind)+' · '+h(label(j.status))+'</strong><p>'+h(readable(j.error||j.summary))+'</p>'+dataDetails('后台真实回执',j)+'</article>')+
    '<section class="wb-card"><h3>公开活动</h3>'+rows(p.events).slice(-limit).reverse().map(e=>'<details class="wb-entry"><summary>'+h(timeLabel(e.created_at||e.at||e.occurred_at))+' · '+h(e.type)+'</summary><pre>'+h(JSON.stringify(e.payload||e.data,null,2))+'</pre></details>').join('')+action('more-records','显示更早记录')+'</section>';
}
export function feedbackHtml(p,renderText){
  const traces=rows(p.feedback_traces).map(t=>feedbackTrace(p,t));
  return unavailable(p,'feedback_traces')+(traces.length?traces.slice().reverse().map(t=>{
    const hasResponse=t.hasExplicitDisposition;
    const waiting=['queued','prepared'].includes(t.delivery_state||'queued')&&['accepted','queued','prepared'].includes(t.status);
    const waited=Number.isFinite(Date.parse(t.created_at))?Math.max(0,Math.floor((Date.now()-Date.parse(t.created_at))/60000)):null;
    const kind=t.kind||t.command?.payload?.feedback?.kind||t.command?.payload?.kind,target=t.target||t.command?.payload?.feedback?.target;
    const targetRecord=target?.kind==='session'?rows(p.sessions).find(s=>s.id===target.id):target?.kind==='route'?rows(p.routes).find(r=>r.id===target.id):null;
    return '<article class="wb-entry wb-feedback"><div>'+badge(kind==='reframe_goal'?'重新梳理目标':t.priority==='urgent'?'紧急建议':'普通建议')+badge(transportLabel(t.status))+'</div><small>'+h(timeLabel(t.created_at))+'</small>'+(target?'<p class="wb-muted">针对：'+h(targetRecord?.title||targetRecord?.focus||{project:'整体研究路线',session:'猫猫研究任务',open_question:'开放疑点',route:'研究路线',route_outcome:'已结束的尝试'}[target.kind]||'所选对象')+'</p>':'')+'<div>'+renderText(t.body)+'</div>'+
      '<p>提供给调用：'+h(transportLabel(t.delivery_state||'queued'))+(waiting&&waited!==null?' · 已等待 '+waited+' 分钟':'')+'</p><p>领研猫处置：'+h(t.dispositionText)+'</p>'+
      (t.response?'<div>'+renderText(readable(t.response))+'</div>':empty(hasResponse?'未记录处置理由。':'尚无明确处置和理由。'))+
      '<p>实际变化：'+(t.results.length?t.results.map(r=>'<button data-wb-result-ref="'+h(r.id)+'" data-collection="'+h(r.collection||'')+'">'+h({tasks:'任务',sessions:'会话',pending_assignments:'改派安排',routes:'路线',nodes:'数学节点',facts:'准入事实',candidates:'证明候选'}[r.collection]||r.kind||'关联对象')+' '+h(r.id)+'</button>').join(''): '尚未关联到执行变化')+'</p>'+
      dataDetails('意见、消息、投递与动作回执',t)+
      (!['cancelled','withdrawn','superseded'].includes(t.status)?'<div class="wb-actions">'+action('withdraw','撤回／发送撤回通知','data-id="'+h(t.command_id)+'"')+
        (t.priority!=='urgent'?action('escalate','升级为紧急','data-id="'+h(t.command_id)+'"'):'')+'</div>':'')+'</article>';
  }).join(''):empty('尚无已读取的意见记录。普通讨论不会自动成为研究建议。'));
}
