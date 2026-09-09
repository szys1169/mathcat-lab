import test from 'node:test';
import assert from 'node:assert/strict';
import {escapeHtml,normalizeSnapshot} from '../research-v2-state.js';
import {authoritativeProofModel} from '../whiteboard-model.js';
import {problemHtml,laboratoryHtml,sessionActivity,sessionHtml,statementsHtml,planningHtml,planningAction,doubtsHtml,failuresHtml,timelineHtml,timelineItems,reframeHistoryHtml,replacePreservingView} from '../whiteboard-lab.js';
import {WhiteboardClient} from '../whiteboard-client.js';
import {ClassicResearchClient} from '../research-v2-classic.js';
import {ResearchWhiteboard} from '../whiteboard-view.js';
const p=extra=>normalizeSnapshot({id:'p',problem_version:1,revision:1,problem:'请证明 P，最多两小时，最多五只猫',problem_spec:{revision:2,problem_version:1,math_statement:'对所有 n ∈ ℕ，P(n) 成立。',research_description:'最多两小时，最多五只猫',normalization_state:'human_edited'},runs:[{id:'r',revision:4,state:'running',mode:'collaborative',limits:{max_partners:5}}],...extra});

test('pure problem leads; operational requirements and original prompt are collapsed',()=>{
  const html=problemHtml(p(),escapeHtml),descriptionAt=html.indexOf('class="wb-description"');
  assert.match(html.slice(0,descriptionAt),/对所有 n/);
  assert.doesNotMatch(html.slice(0,descriptionAt),/最多两小时|最多五只猫/);
  assert.match(html.slice(descriptionAt),/最多两小时/);
  assert.doesNotMatch(html,/<details[^>]*\sopen/);
});
test('pending mixed launch prompt never masquerades as normalized mathematics',()=>{
  const project=p({problem_spec:{math_statement:'MIXED_LAUNCH_PROMPT',original_input:'MIXED_LAUNCH_PROMPT',normalization_state:'pending'}});
  const html=problemHtml(project,escapeHtml),top=html.slice(0,html.indexOf('class="wb-description"'));
  assert.doesNotMatch(top,/MIXED_LAUNCH_PROMPT/);assert.match(top,/尚在整理数学题面/);
  assert.match(problemHtml(p({problem_spec:{math_statement:'P',normalization_state:'model_structured'}}),escapeHtml),/尚未独立核对/);
});
test('session details show only attributable public CLI returns, with timestamp and history flag',()=>{
  const s={id:'cat-a',run_id:'r',role:'partner'},project=p({events:[
    {type:'message.completed',created_at:'2026-09-07T00:00:00Z',payload:{session_id:'cat-a',text:'公开端点论证',stale:true}},
    {type:'message.completed',payload:{session_id:'cat-b',text:'OTHER_SESSION'}},
    {type:'discussion.activity',payload:{session_id:'cat-a',text:'DISCUSSION'}},
    {type:'tool.completed',payload:{session_id:'cat-a',tool:'command_execution',status:'completed'}}]});
  assert.equal(sessionActivity(project,s).length,2);
  const html=sessionHtml(project,s,escapeHtml);assert.match(html,/公开端点论证/);assert.match(html,/历史调用/);assert.doesNotMatch(html,/OTHER_SESSION|DISCUSSION|命令执行|工具完成/);
});
test('lab uses actual session focus and candidate review object, never pending task as current work',()=>{
  const project=p({sessions:[{id:'cat',run_id:'r',role:'partner',state:'active',focus:'当前端点焦点'},{id:'review',run_id:'r',role:'reviewer',state:'active',candidate_id:'c'}],candidates:[{id:'c',run_id:'r',status:'submitted',claim:'正在审核的精确命题'}],pending_assignments:[{partner_session_id:'cat',state:'pending',focus:'NEXT_TASK_SHOULD_NOT_LOOK_CURRENT'}]});
  const html=laboratoryHtml(project,escapeHtml);assert.match(html,/当前端点焦点/);assert.match(html,/正在审核的精确命题/);assert.match(html,/当前任务完成后生效/);assert.doesNotMatch(html,/NEXT_TASK_SHOULD_NOT_LOOK_CURRENT/);
});
test('multiple statements remain searchable and never inherit parent mathematical assurance',()=>{
  const node={id:'node:n',ref:{kind:'node',id:'n',revision:1},title:'组合结论',exact_statement:'总体',math_kind:'claim',admission_state:'accepted',validity:'current',mathematical_statements:[{id:'a',kind:'lemma',title:'第一引理',statement:'对所有 x，A(x)'},{id:'b',kind:'conjecture',title:'第二猜想',statement:'存在唯一 y 满足 B(y)'}]};
  const model=authoritativeProofModel({nodes:[node],edges:[]});assert.match(model.nodes[0].detail,/存在唯一 y/);
  const html=statementsHtml(node,escapeHtml);assert.match(html,/引理/);assert.match(html,/猜想/);assert.match(html,/存在唯一 y/);assert.equal((html.match(/本条陈述尚未独立审查/g)||[]).length,2);
});
test('review queue resolves active review receipts and sessions independently of candidate status',()=>{
  const project=p({sessions:[{id:'reviewer',run_id:'r',role:'reviewer',state:'active',candidate_id:'c',candidate_snapshot:{claim:'冻结审核命题'}}],candidates:[{id:'c',run_id:'r',claim:'冻结审核命题',status:'changes_requested',problem_version:2,snapshot_hash:'abc123frozen'},{id:'done',run_id:'r',status:'submitted',claim:'已结束审核'}],reviews:[{id:'v',candidate_id:'c',reviewer_session_id:'reviewer',state:'running'},{id:'old',candidate_id:'done',state:'completed',verdict:'accepted'}]});
  const html=laboratoryHtml(project,escapeHtml);assert.match(html,/审核列表 · 1 项/);assert.match(html,/冻结候选 c · 快照 abc123frozen · 题目 v2/);assert.doesNotMatch(html,/已结束审核/);
});
test('human plan approval is tied to pending proposal and running collaborative mode',()=>{
  const plan={id:'plan',run_id:'r',revision:3,state:'pending',actions:[{type:'request_partner',focus:'检查特殊情形'}],main_next_phase:'research'};
  const html=planningHtml(p({planning_proposals:[plan]}),escapeHtml);assert.match(html,/通过并执行/);assert.match(html,/新增伙伴猫/);assert.match(html,/检查特殊情形/);
  assert.doesNotMatch(planningHtml(p({runs:[{id:'r',state:'paused',mode:'collaborative'}],planning_proposals:[plan]}),escapeHtml),/data-decision="approve"/);
  assert.doesNotMatch(planningHtml(p({planning_proposals:[{...plan,state:'approved'}]}),escapeHtml),/data-decision="approve"/);
  assert.equal(planningAction({type:'withdraw_assignment'},p()).title,'撤回伙伴猫后续安排');
});
test('open doubts expose actual review gaps plus fixed evidence and resolution history',()=>{
  const html=doubtsHtml(p({open_questions:[{id:'q',title:'审核疑点',statement:'命题 P',issues:[{issue:'尚未覆盖 n=0'}],source_artifact_id:'report',affected_refs:[{kind:'node',id:'n',revision:2}],owner_session_id:'cat',state:'resolved',resolution:'已补上零维情形'}]}),escapeHtml);
  assert.match(html,/尚未覆盖 n=0/);assert.match(html,/已补上零维情形/);assert.match(html,/阅读原稿或审核报告/);assert.match(html,/&quot;revision&quot;:2/);
});
test('runtime interruptions and blocked attempts cannot be displayed as mathematical refutations',()=>{
  const html=failuresHtml(p({route_outcomes:[{id:'r1',title:'计算中断',outcome:'runtime_failure',reason:{summary:'进程断开，未得到结论'}},{id:'r2',title:'参数未闭合',outcome:'blocked',reason:'上界尚未一致'}]}),escapeHtml);
  assert.match(html,/运行故障/);assert.match(html,/暂时受阻/);assert.match(html,/进程断开，未得到结论/);assert.doesNotMatch(html,/wb-badge [^"]*">数学路线被否定|\[object Object\]/);
});
test('timeline separates human plan decisions and math edits from continuous CLI stream',()=>{
  const project=p({events:[{type:'planning.proposed',created_at:'2026-09-07T01:00:00Z',payload:{reason:'分工内容'}},{type:'planning.decided',created_at:'2026-09-07T02:00:00Z',payload:{decision:'approve'}},{type:'problem.edit_requested',created_at:'2026-09-07T03:00:00Z',payload:{}},{type:'activity.delta',payload:{text:'STREAM'}}],planning_proposals:[{id:'old',state:'approved',created_at:'2026-09-06T00:00:00Z'}]});
  assert.equal(timelineItems(project).length,4);const html=timelineHtml(project,escapeHtml);assert.match(html,/人工批准分工/);assert.match(html,/提交问题或说明修改/);assert.doesNotMatch(html,/STREAM|undefined/);
});
test('planning output boundaries never claim proposals and approved proposals retain creation history',()=>{
  const plan={id:'plan',state:'approved',created_at:'2026-09-07T01:00:00Z',decided_at:'2026-09-07T02:00:00Z',reason:'初始研究安排',decision_reason:'先研究一般情形'};
  const project=p({events:[{type:'planning.output_received',payload:{staged:false,run_id:'r',batch:{mode:'delegated'}}},{type:'planning.output_received',payload:{staged:true,batch:{planning_proposal_id:'plan',staged:true}}}],planning_proposals:[plan]});
  const items=timelineItems(project);assert.equal(items.length,2);
  assert.equal(items[0].title,'人工批准分工');assert.equal(items[0].at,plan.decided_at);assert.equal(items[0].body,plan.decision_reason);
  assert.equal(items[1].title,'领研猫提出分工');assert.equal(items[1].at,plan.created_at);assert.equal(items[1].body,plan.reason);
  assert.equal(timelineItems(p({events:project.events,planning_proposals:[]})).length,0);
});
test('direct proposal decision payloads show each human choice and suppress duplicate fallback decisions',()=>{
  const created='2026-09-07T01:00:00Z',deferred='2026-09-07T02:00:00Z',approved='2026-09-07T03:00:00Z';
  const plan={id:'plan',state:'approved',created_at:created,decided_at:approved,reason:'研究安排',decision_reason:'确认执行'};
  const items=timelineItems(p({planning_proposals:[plan],events:[
    {type:'planning.decided',created_at:deferred,payload:{...plan,state:'deferred',decision_reason:'稍后检查',decided_at:deferred}},
    {type:'planning.decided',created_at:approved,payload:plan},
    {type:'planning.decided',created_at:approved,payload:{id:'other',state:'rejected',decision_reason:'请重新安排'}}]}));
  assert.equal(items.filter(x=>x.planId==='plan').length,3);
  assert.equal(items.filter(x=>x.planId==='plan'&&x.title==='人工批准分工').length,1);
  assert.equal(items.filter(x=>x.planId==='plan'&&x.title==='领研猫提出分工').length,1);
  assert.ok(items.some(x=>x.title==='人工暂缓分工'&&x.body==='稍后检查'));
  assert.ok(items.some(x=>x.title==='人工退回分工'&&x.body==='请重新安排'));
});
test('problem edit submission event never claims a mathematical edit has completed',()=>{
  const html=timelineHtml(p({events:[{type:'problem.edit_requested',payload:{summary:'请求已登记'}}]}),escapeHtml);
  assert.match(html,/提交问题或说明修改/);assert.doesNotMatch(html,/问题或说明已修改|数学问题已修改|修改已保存/);
});
test('reframe receipts retain pending response and do not claim execution without result refs',()=>{
  const html=reframeHistoryHtml(p({feedback_traces:[{command_id:'f',kind:'reframe_goal',body:'重新审视一般情形',status:'queued'}]}),escapeHtml);
  assert.match(html,/重新审视一般情形/);assert.match(html,/尚未关联安排变化/);assert.match(html,/尚无明确处置/);
});
test('new writes carry fixed edit, run, and approval revisions and feedback target',async()=>{
  const calls=[];const classic=new ClassicResearchClient({fetchImpl:async(url,options)=>{calls.push({url,method:options.method,body:JSON.parse(options.body)});return new Response(JSON.stringify({contract:'mathcat-research/v2',ok:true}));}});
  const client=new WhiteboardClient(classic,{storage:null}),project=p();
  await client.problemSpec(project,{research_description:'三小时'},'conversation_edit');
  await client.mode(project,'delegated');
  await client.planningDecision(project,{id:'plan',revision:7},'reject','需要关注一般情形');
  await client.feedback(project,{body:'请重新检查研究目标',kind:'reframe_goal',target:{kind:'project',id:'p'}});
  assert.equal(calls[0].method,'PATCH');assert.equal(calls[0].body.expected_revision,2);assert.equal(calls[0].body.source,'conversation_edit');assert.equal(calls[0].body.math_statement,undefined);
  assert.equal(calls[1].body.expected_revision,4);assert.equal(calls[1].body.mode,'delegated');
  assert.equal(calls[2].body.expected_revision,7);assert.equal(calls[2].body.decision,'reject');assert.match(calls[2].url,/planning-proposals\/plan\/decision/);
  assert.deepEqual(calls[3].body.target,{kind:'project',id:'p'});assert.equal(calls[3].body.kind,'reframe_goal');
});
test('refresh preserves open descriptions and scroll and skips unchanged HTML',()=>{
  const detail={dataset:{wbStable:'description'},open:true,querySelector:()=>({textContent:'描述'})};let writes=0;
  const element={scrollTop:73,scrollLeft:12,querySelectorAll:selector=>selector==='details[open]'?[detail]:[detail],set innerHTML(value){writes++;detail.open=false;this.value=value;}};
  replacePreservingView(element,'new');assert.equal(detail.open,true);assert.equal(element.scrollTop,73);assert.equal(element.scrollLeft,12);replacePreservingView(element,'new');assert.equal(writes,1);
});
test('late linked artifact cannot open a modal in another research project',async()=>{
  let finish,opened=false;const view=new ResearchWhiteboard({}, {storage:null,api:{artifact:()=>new Promise(resolve=>{finish=resolve;})},renderText:escapeHtml});
  view.project=p();view.openModal=()=>{opened=true;};const reading=view.openLinkedReference({kind:'artifact',id:'draft'});
  view.project={...p(),id:'another-project'};finish({text:'OLD_PROJECT_PROOF',url:'/draft'});await reading;assert.equal(opened,false);
});
