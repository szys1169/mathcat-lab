export const CONTRACT = "mathcat-research/v2";
export const LABELS = Object.freeze({
  created:"待启动",preflighting:"预检中",running:"研究中",waiting_human:"等待你判断",pausing:"正在暂停",paused:"已暂停",stopping:"正在停止",ended:"本次研究已结束",
  open:"待研究",working:"正在研究",waiting:"等待后续安排",waiting_review:"等待审查",blocked:"存在卡点",done:"本项工作结束",
  unreviewed:"尚未独立审查",model_reviewed:"模型独立审查通过",formally_checked:"形式检查通过",current:"当前有效",challenged:"存在争议",revoked:"已撤回",superseded:"已替代",
  accepted:"已保存",queued:"已排队",delivered:"已送达",acknowledged:"已回应",completed:"已处理",failed:"未能落实",cancelled:"已取消",
  adopted:"已采纳",partially_adopted:"部分采纳",declined:"未采纳",needs_clarification:"需要澄清",
  problem:"主问题",assumption:"假设",focus:"研究方向",claim:"数学结论",proof_gap:"证明缺口",counterexample:"反例",experiment:"计算证据",source:"文献",failed_attempt:"失败尝试",question:"待解问题",decision:"研究决策",note:"研究笔记",
  unresolved:"尚未解决",candidate_available:"有证明候选",reviewed_solution:"原题证明通过内部审查",reviewed_refutation:"反例通过内部审查",formally_certified_solution:"原题形式认证通过",formally_certified_refutation:"反例形式认证通过",disputed:"结论有争议",
  goal_satisfied:"目标已满足",time_limit:"时间上限",budget_limit:"预算上限",user_stop:"用户停止",environment_error:"运行环境故障",succeeded:"已完成",unknown:"状态待确认",idle:"等待下一轮",closed:"已关闭",active:"执行中",lost:"执行状态待确认"
});
export function label(value){return LABELS[value]||value||"未知";}
export function conversationCaption(conversation){return conversation.researchProjectId?"研究记录 · 查看实时状态":String(conversation.messageCount??0)+" 条 · "+String(conversation.status||"idle");}
export function problemVersionCaption(project){return "题目v"+String(project.problem_version||project.current_problem_version||1);}
export function displayResearchPath(value){const original=String(value??'');if(original.startsWith('\\\\?\\UNC\\'))return '\\\\'+original.slice(8);return original.startsWith('\\\\?\\')?original.slice(4):original;}
export function researchResultCaption(project,run){const projectState=project?.result_state;const runState=run?.result_state;if(projectState&&runState&&projectState!==runState)return '项目状态：'+label(projectState)+'；本次运行：'+label(runState)+'（状态不一致，待核对）';return label(projectState||runState||'unresolved');}
export function escapeHtml(value){return String(value??"").replace(/[&<>"']/g,char=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[char]));}
export function readable(value){if(typeof value==="string")return value;if(value==null)return"";if(Array.isArray(value))return value.map(readable).filter(Boolean).join("\n");return value.text||value.content||value.statement||value.summary||value.message||JSON.stringify(value,null,2);}
export function normalizeSnapshot(raw){
  if(raw?.contract && raw.contract!==CONTRACT)throw new Error("研究服务协议不匹配，已停止更新当前白板。");
  const p=raw?.project||raw;if(!p?.id)throw new Error("白板缺少项目身份，未用空数据代替。");
  const result={...p,contract:CONTRACT,event_cursor:raw.event_cursor??p.event_cursor??0};
  for(const field of ["runs","sessions","tasks","nodes","edges","candidates","reviews","facts","commands","discussions","annotations","memories","reports","previews","usage","events","messages","public_messages","artifacts","interactions"])result[field]=Array.isArray(p[field])?p[field]:(Array.isArray(raw[field])?raw[field]:[]);
  for(const field of ["cycles","routes","advisories","pending_assignments","proof_checkpoints","display_summaries","memory_entries","background_jobs","human_questions","feedback_traces"])if(Array.isArray(p[field]??raw[field]))result[field]=p[field]??raw[field];
  return result;
}
export function latestRun(project){return project?.runs?.findLast(run=>run.state!=="ended")||project?.runs?.at(-1)||null;}
export function isTrusted(node){return ["model_reviewed","formally_checked"].includes(node.assurance)&&node.validity==="current";}
export function expectedVersions(project,run){return {problem_version:project.problem_version??project.current_problem_version,...(run?{control_epoch:run.control_epoch}: {})};}
export function commandFor(project,type,{node,run=latestRun(project),text="",payload,applyAt}={}){
  const runAction=["pause_run","resume_run","stop_run"].includes(type);
  if(runAction&&!run)throw new Error("当前项目尚无可控制的研究。");
  const target=runAction?{kind:"run",id:run.id}:node?{kind:"node",id:node.id||node.node_id,revision:node.revision}:{kind:"project",id:project.id};
  return {type,target,...(run?{run_id:run.id}:{}),expected_versions:expectedVersions(project,run),apply_at:applyAt||(["challenge_evidence","pause_run","resume_run","stop_run"].includes(type)?"immediate":"next_safe_boundary"),payload:payload||(runAction?{reason:text||"用户通过白板操作"}:type==="challenge_evidence"?{reason:text,request_paid_review:false}:{text})};
}
export function uniqueEvents(items){const ids=new Set();return items.filter(event=>{const id=event.event_id??(event.project_id+":"+event.seq);if(ids.has(id))return false;ids.add(id);return true;}).sort((a,b)=>(a.seq||0)-(b.seq||0));}
export function publicEventText(event){const p=event.payload||event.data||{};const a=p.activity||p;return readable(a.text??a.content??a.delta??a.summary??a.message);}
export function researchReplies(project){const sessions=new Map(project.sessions.map(s=>[s.id,s]));return project.nodes.filter(n=>n.node_type==="note"&&n.author&&["main","partner","reviewer"].includes(sessions.get(n.author)?.role)).map(n=>({...n,role:sessions.get(n.author).role,run_id:sessions.get(n.author).run_id}));}
export function remainingTime(run,now=Date.now()){if(!run?.deadline_at)return null;const deadline=Date.parse(run.deadline_at);return Number.isFinite(deadline)?Math.max(0,Math.ceil((deadline-now)/1000)):null;}
