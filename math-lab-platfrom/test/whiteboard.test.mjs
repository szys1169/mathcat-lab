import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import {ClassicResearchClient} from '../public/research-v2-classic.js';
import {normalizeSnapshot,escapeHtml} from '../public/research-v2-state.js';
import {proofModel,authoritativeProofModel,currentStatus,activityCards,feedbackTrace,cyclicDependencies,quoteReference,controlReference,canAnswerQuestion} from '../public/whiteboard-model.js';
import {headerHtml,overviewHtml,feedbackHtml} from '../public/whiteboard-render.js';
import {WhiteboardClient} from '../public/whiteboard-client.js';
import {ResearchWhiteboard} from '../public/whiteboard-view.js';
const ref=(id,revision=1,kind='node')=>({kind,id,revision});
const project=extra=>normalizeSnapshot({id:'p',revision:7,problem_version:1,problem:'证明目标 P',runs:[{id:'run',state:'running',revision:2,control_epoch:1,limits:{max_partners:5}}],...extra});
const response=value=>new Response(JSON.stringify({contract:'mathcat-research/v2',...value}));
const store=()=>{const values=new Map();return {getItem:k=>values.get(k),setItem:(k,v)=>values.set(k,v)};};
const mathNode=(id,revision=1,extra={})=>({id:'node:'+id+(revision>1?'@'+revision:''),ref:ref(id,revision),title:id,exact_statement:id,math_kind:'claim',admission_state:'not_admitted',validity:'current',...extra});

test('proof tree resolves shared versioned premises without attaching history to latest',()=>{
  const tree={problem_version:1,nodes:[mathNode('root',1,{math_kind:'problem',problem_version:1}),mathNode('lemma'),mathNode('lemma',2),mathNode('a'),mathNode('b')],edges:[
    {id:'a',from:ref('lemma'),to:ref('a'),relation:'dependency',status:'declared'},
    {id:'b',from:ref('lemma'),to:ref('b'),relation:'dependency',status:'proposed'},
    {id:'missing',from:ref('lemma',3),to:ref('a'),relation:'dependency',status:'declared'}]};
  const model=authoritativeProofModel(tree);
  assert.equal(model.nodes.length,5);assert.equal(model.edges.length,2);assert.equal(model.edges[0].from,'node:lemma');assert.equal(model.edges[0].to,'node:a');
  assert.equal(model.edges[1].conditional,true);assert.equal(model.missing.length,1);assert.equal(model.nodes.find(n=>n.id==='node:lemma@2').trust,'unverified');
});
test('candidate/fact aliases preserve original proof refs and bound versions',()=>{
  const source=mathNode('lemma'),target=mathNode('a');
  const model=authoritativeProofModel({nodes:[source,target],edges:[{from:ref('f',1,'fact'),from_node_ref:ref('lemma'),to:ref('c',1,'candidate'),to_node_ref:ref('a'),relation:'dependency',status:'checked',proof_ref:ref('c',1,'candidate'),premise_group_id:'proof:c'}]});
  assert.equal(model.edges[0].from,'node:lemma');assert.equal(model.edges[0].record.from.kind,'fact');assert.equal(model.edges[0].record.premise_group_id,'proof:c');
});
test('cyclic mathematical dependencies warn; ownership cycles never certify or imply a proof cycle',()=>{
  const owner=[{from:'p',to:'q',mathematical:false},{from:'q',to:'p',mathematical:false}];
  assert.equal(cyclicDependencies(owner).size,0);
  assert.deepEqual([...cyclicDependencies([{from:'a',to:'b',mathematical:true},{from:'b',to:'a',mathematical:true},...owner])].sort(),['a','b']);
});
test('ended run and unknown delivery override stale waiting summary, without changing user-stop motive',()=>{
  const p=project({runs:[{id:'run',state:'ended',stop_reason:'user_stop',result_state:'unresolved',outstanding_cancellation:true}],usage:[{run_id:'run',state:'unknown'}],display_summaries:[{summary:'仍在等待审核'}]});
  assert.equal(currentStatus(p).ended,true);assert.equal(currentStatus(p).unknown.length,1);
  const html=headerHtml(p);assert.match(html,/本轮已结束/);assert.match(html,/具体发起原因见控制记录/);assert.match(html,/未知用量不按零/);assert.doesNotMatch(html,/全部.*已停止/);
});
test('current call target is not synthesized from old proof checkpoint or pending assignment',()=>{
  const p=project({sessions:[{id:'main',run_id:'run',role:'main',phase:'research'}],usage:[{id:'call',session_id:'main',state:'running',started_at:'2026-09-07T00:00:00Z'}],proof_checkpoints:[{session_id:'main',goal:'旧目标',created_at:'old'}],pending_assignments:[{session_id:'main',objective:'新任务',status:'pending'}]});
  const c=activityCards(p)[0];assert.equal(c.active,true);assert.equal(c.currentGoal,null);assert.equal(c.checkpoint.goal,'旧目标');assert.equal(c.pending.length,1);
});
test('accepted auxiliary proof never implies root/stage coverage and raw correct is not admission',()=>{
  const p=project({result_state:'unresolved'});const model={objects:[],goals:[{exact_statement:'整题',status:'open'},{exact_statement:'辅助引理',status:'reviewed_coverage'}]};
  const html=overviewHtml(p,model,escapeHtml);assert.match(html,/尚无有效覆盖记录/);assert.match(html,/该目标已有有效审查覆盖/);assert.match(html,/局部成果入库不代表整题/);
  const tree=authoritativeProofModel({nodes:[mathNode('draft',1,{review_state:'correct',admission_state:'not_admitted'})]});
  assert.equal(tree.nodes[0].trust,'unverified');
});
test('feedback completed with no explicit disposition or action refs does not imply accepted work',()=>{
  const p=project({commands:[{id:'cmd',status:'completed',effect_refs:[{kind:'artifact',id:'reply'}]}]});
  const t=feedbackTrace(p,{command_id:'cmd',status:'completed'});
  assert.equal(t.hasExplicitDisposition,false);assert.equal(t.results.length,0);
  const html=feedbackHtml({...p,feedback_traces:[t]},escapeHtml);assert.match(html,/尚无明确处置/);assert.match(html,/尚未关联到执行变化/);
});
test('same-snapshot projections only read independent delivery control and retain revision',async()=>{
  const calls=[];const classic=new ClassicResearchClient({fetchImpl:async url=>{calls.push(url);assert.match(String(url),/\/delivery$/);return response({control:{state:'running'}});}});
  const client=new WhiteboardClient(classic,{storage:store()});
  const p=project({proof_tree:{revision:7,nodes:[],edges:[]},cycles:[],routes:[],messages:[],advisories:[],pending_assignments:[],proof_checkpoints:[],display_summaries:[],memory_entries:[],background_jobs:[],human_questions:[],feedback_traces:[]});
  const value=await client.supplement(p);assert.deepEqual(value.board_errors,{});assert.equal(value.board_sources.proof_tree,7);assert.equal(calls.length,1);assert.equal(value.project_control.state,'running');
});
test('mismatched supplemental revision and failed reads remain missing, never fake zero data',async()=>{
  const classic=new ClassicResearchClient({fetchImpl:async path=>path.endsWith('/proof-tree')?response({proof_tree:{revision:8,nodes:[]}}):new Response(JSON.stringify({error:'offline'}),{status:503})});
  const value=await new WhiteboardClient(classic,{storage:store()}).supplement(project());
  assert.match(value.board_errors.proof_tree,/修订 8/);assert.equal(value.proof_tree,undefined);assert.equal(value.board_available.advisories,false);
});
test('lost feedback response survives reload and explicit retry reuses exact key/payload',async()=>{
  const storage=store(),calls=[];let fail=true;
  const classic=new ClassicResearchClient({fetchImpl:async(path,options)=>{calls.push({path,body:options.body,key:options.headers['Idempotency-Key']});if(fail){fail=false;throw new Error('lost reply');}return response({command:{id:'c'}});}});
  const p=project(),first=new WhiteboardClient(classic,{storage});
  await assert.rejects(first.feedback(p,{body:'方法建议'}),/lost reply/);
  const restored=new WhiteboardClient(classic,{storage});assert.equal(restored.pending.size,1);
  await assert.rejects(restored.feedback(p,{body:'另一条内容'}),/结果待确认/);
  await restored.retry([...restored.pending.keys()][0]);assert.equal(calls.length,2);assert.equal(calls[0].key,calls[1].key);assert.equal(calls[0].body,calls[1].body);assert.equal(restored.pending.size,0);
});
test('double click shares one in-flight mutation and known rejection releases draft retry',async()=>{
  let complete,calls=0;const classic=new ClassicResearchClient({fetchImpl:()=>{calls++;return new Promise(resolve=>{complete=resolve;});}});
  const client=new WhiteboardClient(classic,{storage:store()}),p=project();
  const a=client.feedback(p,{body:'x'}),b=client.feedback(p,{body:'x'});assert.equal(calls,1);complete(response({command:{id:'c'}}));await Promise.all([a,b]);
  const bad=new WhiteboardClient(new ClassicResearchClient({fetchImpl:async()=>new Response(JSON.stringify({error:'版本冲突'}),{status:409})}),{storage:store()});
  await assert.rejects(bad.feedback(p,{body:'x'}),/版本冲突/);assert.equal(bad.pending.size,0);
});
test('annotation transfer retry does not recreate annotation after successful save then lost feedback response',async()=>{
  const storage=store();let annotations=0,feedback=0;
  const classic=new ClassicResearchClient({fetchImpl:async(path)=>{if(path.endsWith('/annotations')){annotations++;return response({annotation:{id:'a',revision:1}});}feedback++;if(feedback===1)throw new Error('lost');return response({command:{id:'f'}});}});
  const p=project(),selection=ref('n'),client=new WhiteboardClient(classic,{storage});
  await assert.rejects(client.annotateAndFeedback(p,selection,'缺少假设'),/lost/);
  await new WhiteboardClient(classic,{storage}).annotateAndFeedback(p,selection,'缺少假设');
  assert.equal(annotations,1);assert.equal(feedback,2);
});
test('discussion carries fixed selection in explanation only, while answer uses specific question and revision',async()=>{
  const calls=[],p=project(),selection={kind:'candidate',id:'c',revision:1,state_revision:2,quote:'对所有 n'};
  const classic=new ClassicResearchClient({fetchImpl:async(path,options)=>{const body=options.body?JSON.parse(options.body):undefined;calls.push({path,body});if(path.endsWith('/snapshot'))return response({project:p});if(path.endsWith('/discussions'))return response({discussion:{id:'d'}});return response({command:{id:'cmd'}});}});
  await classic.discuss({id:'chat',researchProjectId:'p'},'为何成立',{selectionRef:selection});
  assert.deepEqual(calls.find(c=>c.path.endsWith('/messages')).body.selection_ref,selection);
  assert.equal(calls.some(c=>c.path.endsWith('/commands')||c.path.endsWith('/feedback')),false);
  await new WhiteboardClient(classic,{storage:store()}).answer(p,{id:'q',run_id:'run',revision:3},'保留原假设');
  assert.match(calls.at(-1).path,/human-questions\/q\/answer$/);assert.equal(calls.at(-1).body.expected_revision,3);
});

test('proof quote keeps fixed mathematical identity while binding the actual proof artifact',()=>{
  const source={kind:'node',id:'n',revision:3,sha256:'statement-hash'};
  assert.deepEqual(quoteReference(source,'在端点取相对邻域','proof-3'),{...source,quote:'在端点取相对邻域',artifact_id:'proof-3'});
  assert.equal(quoteReference(source,'命题原文').artifact_id,undefined);
});

test('control revisions use current candidate/fact state and reject silently targeting a newer node',()=>{
  const p=project({nodes:[{id:'n',revision:3}],candidates:[{id:'c',revision:4}],facts:[{id:'f',revision:5}]});
  assert.equal(controlReference(p,ref('f',1,'fact')).revision,5);
  assert.equal(controlReference(p,ref('c',1,'candidate')).revision,4);
  assert.throws(()=>controlReference(p,ref('n',2)),/旧版本/);
  assert.deepEqual(controlReference(p,ref('n',3)),ref('n',3));
});

test('pending partner assignments follow actual partner_session_id/state and clear only on application',()=>{
  const p=project({sessions:[{id:'partner',run_id:'run',role:'partner'}],pending_assignments:[
    {id:'wait',partner_session_id:'partner',state:'pending'},
    {id:'applied',partner_session_id:'partner',state:'applied',applied_at:'now'},
    {id:'cancelled',partner_session_id:'partner',state:'cancelled'}]});
  assert.deepEqual(activityCards(p)[0].pending.map(a=>a.id),['wait']);
});

test('unavailable proof projection does not guess mathematics from unversioned source edges',()=>{
  const model=proofModel(project({nodes:[{id:'n',revision:1,node_type:'claim'}],candidates:[{id:'c',revision:1,dependency_ids:['n']}],edges:[{from:'c',to:'n',type:'depends_on'}]}));
  assert.equal(model.edges.some(e=>e.mathematical),false);assert.match(model.warnings.join(''),/未重建数学依赖/);
});

test('historical critical questions cannot revive paused or ended runs or another problem version',()=>{
  const q={id:'q',run_id:'run',state:'open',problem_version:1};
  const p=project({runs:[{id:'run',state:'waiting_human',human_question_id:'q'}],human_questions:[q]});
  assert.equal(canAnswerQuestion(p,q),true);
  for(const state of ['paused','ended','running']){
    const frozen={...p,runs:[{...p.runs[0],state}]};
    assert.equal(canAnswerQuestion(frozen,q),false);assert.doesNotMatch(overviewHtml(frozen,{objects:[]},escapeHtml),/data-wb-action="answer"/);
  }
  assert.equal(canAnswerQuestion(p,{...q,problem_version:2}),false);
});

test('late original read cannot cache the newly selected proof under the previous version',async()=>{
  let release;const artifacts=[];const a=mathNode('a'),b=mathNode('b');
  const p=project({proof_tree:{nodes:[a,b],edges:[],proofs:[{conclusion_ref:a.ref,proof_artifact_id:'proof-a'},{conclusion_ref:b.ref,proof_artifact_id:'proof-b'}]}});
  const reader={innerHTML:''},view=new ResearchWhiteboard({querySelector:()=>reader},{api:{
    detail:async(_p,_o,r)=>r.id==='a'?new Promise(resolve=>{release=resolve;}):{exact_statement:'B'},
    artifact:async(_p,id)=>{artifacts.push(id);return {text:id};}},storage:store()});
  view.project=p;view.model=proofModel(p);view.persist=()=>{};view.renderPanels=()=>{};view.renderReference=()=>{};view.renderReader=()=>{};view.openProof=()=>{};view.showProofSidebar=()=>{};
  const old=view.select(a.id);await view.select(b.id);release({exact_statement:'A'});await old;
  assert.deepEqual(artifacts,['proof-b']);assert.equal(view.detail.reference.id,'b');
  assert.equal([...view.readCache.keys()].some(key=>key.includes('"id":"a"')),false);
});

test('historical reader only attaches dependencies from the same projected version',async()=>{
  const latest=mathNode('n',2),old=mathNode('n',1);old.id='node:n@1';
  const p=project({proof_tree:{nodes:[latest,old],edges:[]}}),reader={innerHTML:''};
  const view=new ResearchWhiteboard({querySelector:()=>reader},{api:{detail:async()=>({exact_statement:'old'})},storage:store()});
  view.project=p;view.model=proofModel(p);view.persist=()=>{};view.renderPanels=()=>{};view.renderReference=()=>{};view.renderReader=()=>{};view.openProof=()=>{};view.showProofSidebar=()=>{};
  await view.select(latest.id,old.ref);
  assert.equal(view.detail.object.key,old.id);assert.notEqual(view.detail.object.key,latest.id);
});

test('budget form reads authoritative nested Run duration instead of clearing an existing deadline',async()=>{
  const p=project({runs:[{id:'run',state:'running',limits:{duration_seconds:7200,max_partners:5,advisor_interval_seconds:2400,max_invocations:100}}]});
  const view=new ResearchWhiteboard({}, {storage:store()});view.project=p;let formHtml='';
  view.openModal=(_title,body)=>{formHtml=body;};
  const button={dataset:{wbAction:'limits'}};
  await view.click({target:{closest:selector=>selector==='[data-wb-action]'?button:null}});
  assert.match(formHtml,/name="duration"[^>]+value="120"/);
});

test('new annotations and reviews refresh only evidence while fixed proof, page, reference and scroll stay intact',()=>{
  const n=mathNode('n'),other=mathNode('other');let writes=0,html='';
  const region={get innerHTML(){return html;},set innerHTML(value){html=value;writes++;},querySelectorAll:()=>[],querySelector:()=>null};
  const reader={scrollTop:145},status={innerHTML:''};
  const p=project({proof_tree:{nodes:[n,other],edges:[]},annotations:[]});
  const view=new ResearchWhiteboard({querySelector:s=>({'[data-wb-evidence]':region,'[data-wb-reader]':reader,'[data-wb-current-status]':status}[s])},{storage:store(),renderText:escapeHtml});
  const statement={exact_statement:'冻结陈述'},proof={text:'冻结证明'};
  view.project=p;view.model=proofModel(p);view.selection=n.ref;view.quote='选中的文字';
  view.detail={object:view.model.objects[0],reference:n.ref,statement,proof,proofPage:2};
  view.refreshReaderEvidence();view.refreshReaderEvidence();assert.equal(writes,1);
  p.annotations.push({id:'a',selection_ref:n.ref,body:'新保存的原文批注'});view.refreshReaderEvidence();assert.equal(writes,2);assert.match(html,/新保存的原文批注/);
  p.reviews=[{candidate_id:'c',verdict:'accepted',report_validated:true}];p.proof_tree.proofs=[{candidate_id:'c',conclusion_ref:n.ref,admission_state:'not_admitted'}];
  p.proof_tree.edges=[{from:other.ref,to:n.ref,relation:'dependency',status:'declared'}];view.model=proofModel(p);view.refreshReaderEvidence();
  assert.match(html,/独立审核结论：通过/);assert.doesNotMatch(html,/程序结果：已准入/);assert.match(status.innerHTML,/尚未准入/);assert.match(html,/声明依赖/);
  assert.equal(view.detail.statement,statement);assert.equal(view.detail.proof,proof);assert.equal(view.detail.proofPage,2);assert.equal(view.selection,n.ref);assert.equal(view.quote,'选中的文字');assert.equal(reader.scrollTop,145);
});
test('new frontend mounts the proof-tree workspace with persistent collaboration and version-safe continuous reading',async()=>{
  const app=await fs.readFile(new URL('../public/app.js',import.meta.url),'utf8'),view=await fs.readFile(new URL('../public/whiteboard-view.js',import.meta.url),'utf8');
  assert.match(app,/new ResearchWhiteboard/);assert.match(view,/data-wb-graph/);assert.match(view,/data-wb-input/);assert.match(view,/data-wb-proof-card/);assert.match(view,/data-wb-action="open-proof-page"/);assert.match(view,/data-wb-proof-dialog/);
  assert.match(view,/sameVersion/);assert.doesNotMatch(view,/rawProof\.slice\(/);assert.match(view,/ticke[t]!==this.readTicket/);assert.match(view,/data-wb-new-version/);
});
