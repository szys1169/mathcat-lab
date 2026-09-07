import {label, readable, latestRun, isTrusted} from './research-v2-state.js';

export const rows = value => Array.isArray(value) ? value : [];
export const roles = {main:'领研猫',partner:'伙伴猫',reviewer:'审核猫',advisor:'研究顾问猫',memory:'记忆猫',display:'展示猫',discussion:'旁观解释'};
export const timeLabel = value => value && Number.isFinite(Date.parse(value)) ? new Date(value).toLocaleString('zh-CN') : '未记录';
export const short = (value, length=64) => { const s=readable(value).replace(/\s+/g,' ').trim(); return s.length>length?s.slice(0,length)+'…':s; };
export const revisionKey = object => [object.kind||object.node_type||'node',object.id,object.revision??object.node_revision??1].join(':');
export const phaseLabel = value => ({research:'研究',coordination:'整体安排',waiting:'等待',waiting_plan:'等待人工审核分工',paused:'暂停'}[value]||value||'阶段未记录');
export const dispositionLabel = value => ({adopted:'已采纳',partially_adopted:'部分采纳',declined:'暂不采纳',needs_clarification:'需要澄清',deferred:'暂缓'}[value]||'尚无明确处置');
export const transportLabel = value => ({confirmed:'已确认提供给调用',unconfirmed:'未确认提供',accepted:'已保存',queued:'等待处理',prepared:'已准备输入，尚未确认提供',delivered:'已提供给调用',acknowledged:'领研猫已回应',handled:'已处理，处置另见回应',completed:'处理结束，效果另查',cancelled:'已撤回',withdrawn:'已撤回',superseded:'已被后续意见替代',failed:'未能处理',unknown:'交付状态未知'}[value]||value||'尚未记录');

export function reviewStatus(project, object) {
  const candidateId=object.kind==='candidate'?object.id:object.candidate_id;
  const reviews=rows(project.reviews).filter(r=>r.candidate_id===candidateId);
  const review=reviews.at(-1);
  const raw=review?.raw_verification||review?.report?.raw_verification||review?.report?.verification||{};
  const admitted=object.kind==='fact'&&object.assurance==='model_reviewed'||object.kind==='fact'&&object.assurance==='formally_checked';
  return {review,reviews,admitted,opinion:raw.verdict||review?.raw_verdict||null,
    admission:admitted?'已准入':review?.report_validated===false?'未准入：审核报告校验未通过':'尚未准入',
    validity:label(object.validity||'current'),
    assurance:label(object.assurance||'unreviewed')};
}

// Layout is a projection. An edge is mathematical only if its stored type says so.
const relationLabels={declared:'声明依赖',planned:'拟依赖',checked:'已核对使用',ownership:'目标归属',related:'相关记录'};

export function proofModel(project) {
  if(project.proof_tree)return authoritativeProofModel(project.proof_tree);
  const objects=[], byRef=new Map(), edges=[], missing=[];
  const add=(record,kind) => {
    const object={...record,kind};
    object.key=revisionKey(object);
    object.title=record.title||record.claim||record.exact_statement||short(record.body)||label(record.node_type)||'未命名成果';
    object.statement=record.exact_statement||record.claim||readable(record.body)||record.statement||'';
    object.problem_version=record.problem_version??project.problem_version;
    if(!objects.some(x=>x.key===object.key))objects.push(object);
    byRef.set(record.id,object);return object;
  };
  for(const n of rows(project.nodes))add(n,n.node_type==='problem'?'problem':'node');
  for(const c of rows(project.candidates))add(c,'candidate');
  for(const f of rows(project.facts))add(f,'fact');
  let root=objects.find(x=>x.kind==='problem'&&x.problem_version===project.problem_version);
  if(!root)root=add({id:'problem:'+project.id+':'+project.problem_version,revision:project.problem_version,body:project.problem,title:'当前研究目标',problem_version:project.problem_version,synthetic_view_root:true},'problem');
  // Missing authoritative projection: preserve readable records but do not infer dependency directions or versions.
  // Connectedness to the project does not assert goal coverage.
  const owned=new Set(edges.filter(e=>e.relation==='ownership').map(e=>e.to));
  for(const object of objects)if(object.key!==root.key&&!owned.has(object.key)){
    edges.push({id:'view-owner:'+object.key,from:root.key,to:object.key,relation:'ownership',relationLabel:'项目收录',mathematical:false,record:{view_only:true}});
  }
  const cycleIds=cyclicDependencies(edges);
  const nodes=objects.map(o=>({id:o.key,label:o.title,shortLabel:short(o.title,30),detail:o.statement,
    kind:o.kind==='problem'?'theorem':o.kind==='fact'?'fact':o.kind==='candidate'?'candidate':o.node_type||'note',
    status:cycleIds.has(o.key)||o.validity&&o.validity!=='current'?'blocked':isTrusted(o)?'verified':o.work_state==='working'?'active':'pending',
    trust:isTrusted(o)?'verified':'unverified',object:o,meta:{layoutParentId:o.key===root.key?null:root.key}}));
  return {kind:'proof',rootId:root.key,nodes,edges,objects,byRef,cycleIds,missing,warnings:['权威证明树尚不可用；这里只列已保存记录，未重建数学依赖。']};
}

export function authoritativeProofModel(tree) {
  const byRef=new Map();
  const objects=rows(tree.nodes).map(n=>{
    const ref=n.ref||{};
    const statements=rows(n.mathematical_statements).map(s=>typeof s==='string'?s:s.exact_statement||s.statement||'').filter(Boolean);
    const object={...n,...ref,ref,title:n.title||n.exact_statement||'未命名成果',statement:[n.exact_statement,...statements].filter(Boolean).join('\n'),
      key:n.id,kind:ref.kind||'node',revision:ref.revision,problem_version:n.problem_version??tree.problem_version};
    byRef.set(ref.id,object);return object;
  });
  const keyFor=ref=>typeof ref==='string'?(objects.find(o=>o.key===ref)?.key||byRef.get(ref)?.key):
    objects.find(o=>o.ref.kind===ref?.kind&&o.ref.id===ref?.id&&(!ref.revision||o.ref.revision===ref.revision))?.key;
  const missing=[];
  const edges=rows(tree.edges).map((e,i)=>{
    const from=keyFor(e.from_node_ref||e.from),to=keyFor(e.to_node_ref||e.to),mathematical=e.relation==='dependency';
    const relation=mathematical?({proposed:'planned',checked:'checked'}[e.status]||'declared'):'ownership';
    if(!from||!to){missing.push(e);return null;}
    return {id:e.id||'edge:'+i,from,to,relation,relationLabel:relationLabels[relation],record:e,
      sourceRevision:e.from?.revision,targetRevision:e.to?.revision,versionBound:Boolean(e.from?.revision),
      mathematical,conditional:e.status==='proposed'};
  }).filter(Boolean);
  const root=objects.find(o=>o.math_kind==='problem'&&o.problem_version===tree.problem_version)||objects[0];
  const cycleIds=cyclicDependencies(edges);
  const nodes=objects.map(o=>{
    const trusted=['accepted','admitted'].includes(o.admission_state)&&o.validity==='current';
    return {id:o.key,label:o.title,shortLabel:short(o.title,30),detail:o.statement,
      kind:o.math_kind||o.kind,status:cycleIds.has(o.key)||o.validity&&o.validity!=='current'?'blocked':trusted?'verified':o.work_state==='working'?'active':'pending',
      trust:trusted?'verified':'unverified',object:o,meta:{}};
  });
  return {kind:'proof',rootId:root?.key,nodes,edges,objects,byRef,cycleIds,missing,
    proofs:rows(tree.proofs),goals:rows(tree.goals),warnings:rows(tree.warnings)};
}

export function cyclicDependencies(edges) {
  const adjacency=new Map(),stack=[],visiting=new Set(),done=new Set(),cycles=new Set();
  for(const e of edges.filter(e=>e.mathematical)){if(!adjacency.has(e.from))adjacency.set(e.from,[]);adjacency.get(e.from).push(e.to);}
  function visit(id){if(visiting.has(id)){stack.slice(stack.indexOf(id)).forEach(x=>cycles.add(x));return;}if(done.has(id))return;visiting.add(id);stack.push(id);for(const to of adjacency.get(id)||[])visit(to);stack.pop();visiting.delete(id);done.add(id);}
  for(const id of adjacency.keys())visit(id);return cycles;
}

export function currentStatus(project) {
  const run=latestRun(project),usage=rows(project.usage).filter(u=>u.run_id===run?.id);
  const unknown=usage.filter(u=>u.state==='unknown'||u.delivery_status==='unknown');
  const current=usage.filter(u=>['running','reserved','dispatching','started'].includes(u.state));
  const stop=run?.stop_reason==='user_stop'?'外部停止命令（具体发起原因见控制记录）':label(run?.stop_reason);
  return {run,unknown,current,ended:run?.state==='ended',state:label(run?.state||'created'),
    result:label(run?.result_state||project.result_state||'unresolved'),stop,
    cancellationUnknown:Boolean(run?.outstanding_cancellation||unknown.length),
    // Summary prose is never consulted for authoritative state.
    coverage:rows(project.goal_coverages||project.goal_coverage),sessions:rows(project.sessions).filter(s=>s.run_id===run?.id)};
}

export function activityCards(project) {
  const {run}=currentStatus(project);
  return rows(project.sessions).filter(s=>s.run_id===run?.id).map(session=>{
    const calls=rows(project.usage).filter(u=>u.session_id===session.id);
    const call=calls.findLast(u=>['running','reserved','dispatching','started'].includes(u.state))||calls.at(-1);
    const task=rows(project.tasks).find(t=>t.id===session.task_id);
    const checkpoint=rows(project.proof_checkpoints).filter(c=>c.session_id===session.id).at(-1);
    return {session,role:roles[session.role]||session.role,phase:phaseLabel(session.phase),call,task,checkpoint,
      currentGoal:call?.objective||call?.goal||call?.task_objective||task?.objective||null,
      goalAt:call?.started_at||task?.created_at||null,
      pending:rows(project.pending_assignments).filter(a=>(a.partner_session_id||a.session_id)===session.id&&!a.applied_at&&!a.cancelled_at&&['pending',undefined].includes(a.state||a.status)),
      active:Boolean(call&&['running','reserved','dispatching','started'].includes(call.state))};
  });
}

export function feedbackTrace(project, trace) {
  const command=rows(project.commands).find(c=>c.id===trace.command_id)||trace;
  const messages=rows(project.messages).filter(m=>m.command_id===command.id);
  const disposition=trace.disposition||command.disposition;
  const rawRefs=rows(trace.result_refs);
  // Only explicit backend action/result references count, never a prose acknowledgement.
  const results=rawRefs.filter(ref=>typeof ref==='object'&&ref.id);
  return {...trace,command,command_id:trace.command_id||command.id,body:trace.body||command.payload?.text||'',
    priority:trace.priority||command.priority||'normal',status:trace.status||command.status,
    messages,disposition,dispositionText:dispositionLabel(disposition),
    response:trace.response||command.response||null,results,
    created_at:trace.created_at||command.created_at,revision:trace.revision??command.revision,
    hasExplicitDisposition:['adopted','partially_adopted','declined','needs_clarification','deferred'].includes(disposition)};
}

export function selectionReference(object, {artifactId,sha256,quote}={}) {
  if(!object)return null;
  return {kind:object.kind==='problem'?'node':object.kind,id:object.id,revision:object.revision??1,
    problem_version:object.problem_version,
    ...(artifactId?{artifact_id:artifactId}:{}),...(sha256?{sha256}:{}),...(quote?{quote}: {})};
}

export function quoteReference(ref,quote,artifactId) {
  return {...ref,...(quote?{quote}:{}),...(quote&&artifactId?{artifact_id:artifactId}:{})};
}

export function controlReference(project,ref) {
  const collection={node:'nodes',candidate:'candidates',fact:'facts'}[ref.kind];
  const record=rows(project[collection]).find(row=>row.id===ref.id);
  if(!record)throw new Error('当前快照未找到可控制的成果，请刷新后重试。');
  if(ref.kind==='node'&&record.revision!==ref.revision)throw new Error('当前正在阅读旧版本，请先打开新版，再确认是否标记其争议。');
  return {kind:ref.kind,id:ref.id,revision:record.state_revision??record.revision};
}

export function canAnswerQuestion(project,question) {
  const run=latestRun(project);
  return Boolean(run?.state==='waiting_human'&&run.human_question_id===question.id&&
    run.id===question.run_id&&question.state==='open'&&question.problem_version===project.problem_version);
}
