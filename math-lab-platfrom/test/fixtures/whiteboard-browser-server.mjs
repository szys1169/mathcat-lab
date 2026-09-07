// Explicit opt-in UI fixture. This server never starts a model or a research service.
import http from 'node:http';
import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../public');
const ref=(id,revision=1,kind='node')=>({kind,id,revision});
const fixture=()=>{
  const now=new Date().toISOString(),deadline=new Date(Date.now()+7200000).toISOString();
  const nodes=[
    {id:'node:goal',ref:ref('goal'),title:'证明连续函数在闭区间上有界',math_kind:'problem',exact_statement:'设 f : [a,b] → ℝ 连续。证明存在 M，使任意 x ∈ [a,b] 满足 |f(x)| ≤ M。',work_state:'working',admission_state:'not_submitted',review_state:'unreviewed',validity:'current',problem_version:1},
    {id:'node:compact',ref:ref('compact'),title:'有限子覆盖引理',math_kind:'claim',exact_statement:'闭区间的每个开覆盖均有有限子覆盖。',work_state:'done',admission_state:'accepted',review_state:'accepted',validity:'current',proof_artifact_id:'proof1',candidate_id:'c1',proof_refs:[ref('c1',1,'candidate')],problem_version:1},
    {id:'node:local',ref:ref('local'),title:'局部有界',math_kind:'claim',exact_statement:'连续函数在每一点的某个相对邻域有界。',work_state:'done',admission_state:'not_admitted',review_state:'changes_requested',validity:'current',proof_artifact_id:'proof2',candidate_id:'c2',proof_refs:[ref('c2',1,'candidate')],problem_version:1},
    {id:'node:global',ref:ref('global'),title:'合并有限个局部界',math_kind:'claim',exact_statement:'有限个局部界的最大值给出全局界。',work_state:'working',admission_state:'not_admitted',review_state:'unreviewed',validity:'current',problem_version:1}
  ];
  const edges=[{id:'m1',from:ref('goal'),to:ref('global'),relation:'goal_membership',status:'proposed'},
    {id:'d1',from:ref('compact'),to:ref('global'),relation:'dependency',status:'checked',premise_group_id:'proof:3'},
    {id:'d2',from:ref('local'),to:ref('global'),relation:'dependency',status:'proposed',premise_group_id:'proof:3'}];
  const proofs=[{ref:ref('c1',1,'candidate'),conclusion_ref:ref('compact'),candidate_id:'c1',proof_artifact_id:'proof1',admission_state:'accepted',declared_premises:[],premise_group_id:'proof:c1'},
    {ref:ref('c2',1,'candidate'),conclusion_ref:ref('local'),candidate_id:'c2',proof_artifact_id:'proof2',admission_state:'not_admitted',declared_premises:[],premise_group_id:'proof:c2'}];
  return {id:'fixture',title:'白板界面验收（模拟数据，不启动模型）',revision:1,problem_version:1,problem:nodes[0].exact_statement,updated_at:now,
    problem_spec:{revision:1,problem_version:1,math_statement:nodes[0].exact_statement,research_description:'研究时长上限 2 小时，伙伴猫上限 5。希望获得完整证明并说明各引理之间的依赖。',original_input:'请在 2 小时内证明闭区间连续函数有界，安排不超过 5 只伙伴猫。'+nodes[0].exact_statement,normalization_state:'model_structured'},
    runs:[{id:'r',state:'running',mode:'collaborative',revision:1,control_epoch:1,started_at:now,deadline_at:deadline,result_state:'unresolved',limits:{max_partners:5,advisor_interval_seconds:2400,max_invocations:1000}}],
    nodes:nodes.map(n=>({id:n.ref.id,node_type:n.math_kind==='problem'?'problem':'claim',...n.ref,title:n.title,body:n.exact_statement,proof_artifact_id:n.proof_artifact_id,revision:1,problem_version:1})),
    proof_tree:{project_id:'fixture',revision:1,problem_version:1,nodes,edges,proofs,goals:[{ref:ref('goal'),exact_statement:nodes[0].exact_statement,status:'open',goal_kind:'root',coverage_evidence:[]}],warnings:[]},
    candidates:[{id:'c1',run_id:'r',problem_version:1,status:'accepted',revision:1,claim:nodes[1].exact_statement,proof_artifact_id:'proof1',snapshot_hash:'sha1'},{id:'c2',run_id:'r',problem_version:1,status:'reviewing',revision:1,claim:nodes[2].exact_statement,proof_artifact_id:'proof2',snapshot_hash:'sha2-frozen-local-bound'}],
    reviews:[{id:'review1',candidate_id:'c1',state:'completed',verdict:'accepted',report_validated:true,raw_verification:{verdict:'correct'}},{id:'review2',candidate_id:'c2',run_id:'r',reviewer_session_id:'reviewer',state:'running',issues:[]}],
    sessions:[{id:'main',run_id:'r',role:'main',state:'waiting',phase:'waiting_plan',focus:'利用有限子覆盖拼接局部有界性，完成全局有界证明。'},{id:'worker',run_id:'r',role:'partner',state:'active',phase:'research',focus:'验证相对拓扑中的端点邻域',task_id:'t',route_id:'route'},{id:'reviewer',run_id:'r',role:'reviewer',state:'active',candidate_id:'c2',phase:'research'},{id:'advisor',run_id:'r',role:'advisor',state:'idle',phase:'waiting',focus:'检查紧性路线是否直接推进最终问题'}],
    tasks:[{id:'t',objective:'检查闭区间端点附近的相对邻域'}],usage:[{id:'call',run_id:'r',session_id:'main',state:'completed',started_at:now,ended_at:now},{id:'worker-call',run_id:'r',session_id:'worker',state:'running',started_at:now},{id:'review-call',run_id:'r',session_id:'reviewer',state:'running',started_at:now}],
    feedback_traces:[],human_questions:[],messages:[],commands:[],annotations:[],discussions:[],interactions:[],facts:[],artifacts:[],events:[{event_id:'event1',seq:1,type:'planning.proposed',created_at:now,payload:{proposal_id:'plan1',session_id:'main',reason:'请伙伴猫核对端点，领研猫准备合并局部界。'}},{event_id:'event2',seq:2,type:'activity.delta',created_at:now,payload:{session_id:'worker',invocation_id:'worker-call',text:'在端点 a 处，应使用 [a,b] 的相对邻域，不能直接要求双侧邻域包含于定义域。'}},{event_id:'event3',seq:3,type:'message.completed',created_at:now,payload:{session_id:'worker',text:'取 ε=1，由连续性找到 δ>0；若 x∈[a,b] 且 |x-a|<δ，则 |f(x)|≤|f(a)|+1。'}}],
    planning_proposals:[{id:'plan1',run_id:'r',session_id:'main',revision:1,state:'pending',main_next_phase:'research',reason:'局部有界性已取得进展，下一轮聚焦端点核对与全局拼接。',created_at:now,actions:[{type:'reassign_partner',partner_session_id:'worker',focus:'核对有限子覆盖下，每个局部上界的选取与最大值的依赖。'},{type:'request_partner',focus:'独立检查证明是否遗漏 a=b 的退化区间。'}]}],
    open_questions:[{id:'doubt1',title:'端点处的邻域是否覆盖完整？',statement:'在闭区间端点使用连续性时，局部有界论证是否仍成立？',conditions:'f 在 [a,b] 的相对拓扑下连续。',owner_session_id:'worker',state:'open',affected_refs:[ref('local')],evidence_refs:[ref('c2',1,'candidate')],issues:[{issue:'原稿中的双侧开区间未限定到定义域，需要改为相对邻域。'}]}],
    route_outcomes:[{id:'outcome1',title:'从逐点有界直接取上确界',goal:'由每个点的局部界导出统一上界',method:'直接取所有局部界的上确界',conditions:'未先提取有限子覆盖',outcome:'blocked',reason:'局部界可能依赖于点，尚未证明它们的上确界有限。',scope:'该直接步骤暂不成立，紧性拼接仍可继续。',useful_result_refs:[ref('local')],evidence_refs:[ref('c2',1,'candidate')]},{id:'outcome2',title:'数值网格探索',outcome:'runtime_failure',reason:{summary:'模拟计算进程中断，未形成数学结论。'}}],
    cycles:[{from_phase:'coordination',to_phase:'research',reason:'先完成局部有界引理，再核对全局覆盖。',created_at:now}],
    routes:[{id:'route',status:'active',title:'紧性路线'}],advisories:[],pending_assignments:[{session_id:'worker',objective:'当前任务结束后，核对有限最大值。',created_at:now}],
    proof_checkpoints:[],display_summaries:[{created_at:now,summary:'等待局部有界引理的审核。',snapshot_revision:1}],memory_entries:[],background_jobs:[]};
};
const html='<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>白板验收模拟页面</title><link rel="stylesheet" href="/whiteboard.css"><style>body{margin:0;background:#f0f4eb;font-family:system-ui,sans-serif}#fixture{max-width:1800px;margin:auto}</style><section id="fixture"></section><script type="module">import {ResearchWhiteboard} from "/whiteboard-view.js";import {WhiteboardClient} from "/whiteboard-client.js";import {ClassicResearchClient} from "/research-v2-classic.js";import {normalizeSnapshot,escapeHtml} from "/research-v2-state.js";const classic=new ClassicResearchClient();const api=new WhiteboardClient(classic);let view;async function refresh(){const p=normalizeSnapshot((await (await fetch("/state")).json()).project);classic.projects.set("chat",p);view.update(await api.supplement(p),{id:"chat",researchProjectId:"fixture"});}view=new ResearchWhiteboard(document.querySelector("#fixture"),{api,classic,refresh,renderText:t=>"<div>"+escapeHtml(t).replaceAll("\\n","<br>")+"</div>"});await refresh();globalThis.whiteboardFixture={view,refresh};</script></html>';
if(process.argv.includes('--serve')){
  const p=fixture(),requests=[];
  const server=http.createServer(async(req,res)=>{
    const url=new URL(req.url,'http://localhost');const send=(value,status=200)=>{res.writeHead(status,{'content-type':'application/json'});res.end(JSON.stringify({contract:'mathcat-research/v2',...value}));};
    if(url.pathname==='/'){res.writeHead(200,{'content-type':'text/html; charset=utf-8'});res.end(html);return;}
    if(url.pathname==='/state'||url.pathname.endsWith('/snapshot')){send({project:p});return;}
    if(url.pathname==='/requests'){send({requests});return;}
    if(req.method==='POST'||req.method==='PATCH'){
      let raw='';for await(const chunk of req)raw+=chunk;const body=JSON.parse(raw||'{}');requests.push({path:url.pathname,body});p.revision++;p.proof_tree.revision=p.revision;
      if(url.pathname.endsWith('/problem-spec')){if(body.expected_revision!==p.problem_spec.revision){send({error:'版本冲突'},409);return;}Object.assign(p.problem_spec,body,{revision:p.problem_spec.revision+1,normalization_state:body.math_statement?'human_edited':p.problem_spec.normalization_state});if(body.math_statement){p.problem_version++;p.problem_spec.problem_version=p.problem_version;}send({problem_spec:p.problem_spec});return;}
      if(url.pathname.endsWith('/mode')){p.runs[0].mode=body.mode;p.runs[0].revision++;send({run:p.runs[0]});return;}
      if(url.pathname.includes('/planning-proposals/')&&url.pathname.endsWith('/decision')){const plan=p.planning_proposals.find(x=>x.id===url.pathname.split('/').at(-2));if(!plan||body.expected_revision!==plan.revision){send({error:'版本冲突'},409);return;}plan.state={approve:'approved',reject:'rejected',defer:'deferred'}[body.decision];plan.revision++;plan.decided_at=new Date().toISOString();plan.decision_reason=body.reason;send({planning_proposal:plan});return;}
      if(url.pathname.endsWith('/feedback')){const c={command_id:'feedback'+requests.length,id:'feedback'+requests.length,body:body.body,priority:body.priority,kind:body.kind,target:body.target,status:'queued',revision:1,created_at:new Date().toISOString()};p.feedback_traces.push(c);send({command:c});return;}
      if(url.pathname.endsWith('/discussions')){const d={id:'d',messages:[]};p.discussions.push(d);send({discussion:d});return;}
      if(url.pathname.endsWith('/messages')){p.discussions[0].messages.push({id:'m'+requests.length,author:'local-owner',text:body.text,created_at:new Date().toISOString(),selection_ref:body.selection_ref});send({execution:{status:'queued'}});return;}
      if(url.pathname.endsWith('/annotations')){const a={id:'a'+requests.length,revision:1,...body};p.annotations.push(a);send({annotation:a});return;}
      send({command:{id:'control'+requests.length,status:'accepted',revision:1}});return;
    }
    if(url.pathname.includes('/nodes/')){const node=p.nodes.find(n=>n.id===url.pathname.split('/').at(-1));send({node});return;}
    if(url.pathname.includes('/statements/')){const id=url.pathname.split('/').at(-1),c=p.candidates.find(c=>c.id===id);send({statement:c?{...c,text:c.claim}:{id,text:'原始草稿'}});return;}
    if(url.pathname.includes('/artifacts/')){res.writeHead(200,{'content-type':'text/plain; charset=utf-8'});res.end('固定证明原文。\n\n给定一点 x₀，由连续性取 ε = 1，存在 δ > 0，使 |f(x) − f(x₀)| < 1。\n因此相对邻域中 |f(x)| ≤ |f(x₀)| + 1。\n\n本页为界面验收模拟材料，不代表数学研究测试。');return;}
    const file=path.resolve(root,'.'+url.pathname);
    if(!file.startsWith(root+path.sep)){send({error:'not found'},404);return;}
    try{const data=await fs.readFile(file);res.writeHead(200,{'content-type':file.endsWith('.js')?'text/javascript':file.endsWith('.css')?'text/css':'application/octet-stream'});res.end(data);}catch{send({error:'not found'},404);}
  });
  server.listen(Number(process.env.MATHCAT_WHITEBOARD_FIXTURE_PORT||0),'127.0.0.1',()=>console.log('WHITEBOARD_FIXTURE http://127.0.0.1:'+server.address().port));
}
