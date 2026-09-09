import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import crypto from 'node:crypto';
import {ClassicResearchClient,classicProjection,renderClassicResearchBoard,isClassicResearch,needsResearchPolling} from '../public/research-v2-classic.js';
import {normalizeSnapshot,escapeHtml,displayResearchPath,researchResultCaption} from '../public/research-v2-state.js';
import {buildProofTree} from '../public/research-graph.js';

const project=extra=>normalizeSnapshot({id:'p',problem:'证明 n(n+1) 是偶数',problem_version:1,workspace_path:'version/workspaces/p',runs:[{id:'r',state:'running',control_epoch:1}],...extra});
const conversation={id:'c',researchProjectId:'p',researchContract:'mathcat-research/v2',messages:[]};
const response=value=>new Response(JSON.stringify({contract:'mathcat-research/v2',...value}),{headers:{'content-type':'application/json'}});

test('classic browser fetch is called with the global receiver, not the client instance',async()=>{
  const client=new ClassicResearchClient({fetchImpl:function(){assert.equal(this,globalThis);return Promise.resolve(response({project:project()}));}});
  await client.load(conversation);
});

test('shared rendering retains safe markup and actual mathematical scripts',async()=>{
  const {renderLatexText}=await import('../public/math-renderer.js');
  const {default:katex}=await import('katex');
  const html=renderLatexText('**结论** $J_f$ <script>alert(1)</script>',{escapeHtml,renderFormula:source=>katex.renderToString(source,{throwOnError:true})});
  assert.match(html,/<strong>结论<\/strong>/);assert.match(html,/class="katex"/);assert.doesNotMatch(html,/<script>|math-fallback/);
  const index=await fs.readFile(new URL('../public/index.html',import.meta.url),'utf8');
  for(const name of ['styles.css','whiteboard.css','vendor/katex/katex.min.css'])assert.ok(index.includes(name));
});

test('classic chat and whiteboard remain mutually exclusive full-page views',async()=>{
  const app=await fs.readFile(new URL('../public/app.js',import.meta.url),'utf8');
  assert.match(app,/\$\("messages"\)\.hidden=boardOpen;/);
  assert.match(app,/boardElement\.hidden=!boardOpen;/);
  assert.match(app,/document\.querySelector\("\.composer"\)\.hidden=boardOpen;/);
  assert.match(app,/if\(!boardOpen\)researchWhiteboard\?\.dismiss\(\)/);
  assert.doesNotMatch(app,/ResearchV2View|research-v2\.css|v2-split|v2Intent|v2Duration|v2Files|v2Attach|v2-projects/);
  assert.doesNotMatch(app,/brand strong/);assert.match(app,/isClassicResearch\(item\)/);
  assert.doesNotMatch(app,/if \(createdId\).*DELETE/);
});

test('classic projection shows remote records and truthful trust without invented routes',()=>{
  const p=project({sessions:[{id:'main',role:'main',run_id:'r'}],nodes:[{id:'problem',node_type:'problem',body:'原题',revision:1,work_state:'open',assurance:'unreviewed',validity:'current'},{id:'note',node_type:'note',author:'main',body:'公开证明',revision:1,work_state:'done',assurance:'unreviewed',validity:'current'}],discussions:[{id:'d',messages:[{id:'m',author:'local-owner',text:'仅提问'}]}]});
  const value=classicProjection(conversation,p,[{project_id:'p',seq:1,type:'tool.started',payload:{text:'正在计算'}}]);
  assert.equal(isClassicResearch(conversation),true);assert.equal(value.conversation.status,'running');assert.equal(value.conversation.messages.find(m=>m.id==='v2-note-note').content,'公开证明');assert.equal(value.conversation.messages.find(m=>m.id==='v2-discussion-m').role,'user');assert.equal(value.activity.details[0],'正在计算');assert.deepEqual(value.board.routes,[]);
  const graph=buildProofTree(value.board);assert.equal(graph.nodes.length,2);assert.equal(graph.nodes.find(n=>n.id==='note').trust,'unverified');assert.equal(graph.edges.length,0);
});

test('classic board reuses original cards and omits unsupported forced planner actions',()=>{
  const p=project({reports:[{id:'report',artifact_id:'a1',validity:'challenged'}]});const board=classicProjection(conversation,p).board;
  const html=renderClassicResearchBoard(board,{escapeHtml,renderText:x=>escapeHtml(x||''),graphCards:'<section class="board-card">旧图谱</section>'});
  assert.match(html,/board-grid/);assert.match(html,/human-collaboration-console/);assert.match(html,/旧图谱/);assert.match(html,/历史快照/);assert.doesNotMatch(html,/data-board-action="route"|data-add-route|逐路线审批<\/button>|v2-head|v2-tabs/);
});

test('classic follow-up discussion owns active Run and never sends steering implicitly',async()=>{
  const calls=[];const client=new ClassicResearchClient({fetchImpl:async(path,options)=>{calls.push({path,options,body:options.body?JSON.parse(options.body):null});if(path.endsWith('/snapshot'))return response({project:project()});if(path.endsWith('/discussions'))return response({discussion:{id:'d'}});return response({execution:{}});}});
  assert.equal(await client.discuss(conversation,'解释这个步骤'),true);const call=calls.find(c=>c.path.endsWith('/messages'));assert.equal(call.body.reply_mode,'explain');assert.deepEqual(call.body.execution_owner,{kind:'run',id:'r'});assert.equal(calls.some(c=>c.path.endsWith('/commands')),false);
});

test('classic ended discussion requires explicit independent authorization and supports cancellation',async()=>{
  const calls=[];const p=project({runs:[{id:'r',state:'ended',control_epoch:2}],interactions:[]});const client=new ClassicResearchClient({fetchImpl:async(path,options)=>{const body=options.body?JSON.parse(options.body):null;calls.push({path,body});if(path.endsWith('/snapshot'))return response({project:p});if(path.endsWith('/discussions'))return response({discussion:{id:'d'}});if(path.endsWith('/interaction-executions'))return response({interaction:{id:'i'}});return response({});}});
  assert.equal(await client.discuss(conversation,'解释',{authorize:()=>false}),false);assert.equal(calls.some(c=>c.body),false);
  await client.discuss(conversation,'解释',{authorize:()=>true});assert.equal(calls.find(c=>c.path.endsWith('/interaction-executions')).body.limits.duration_seconds,300);assert.deepEqual(calls.find(c=>c.path.endsWith('/messages')).body.execution_owner,{kind:'interaction',id:'i'});
  await client.cancelInteraction({...p,interactions:[{id:'i',revision:3}]},'i');assert.equal(calls.at(-1).body.expected_revision,3);
});

test('classic write retries use same idempotency key and Run start preserves two-hour explicit budget',async()=>{
  const calls=[];let fail=true;const client=new ClassicResearchClient({fetchImpl:async(path,options)=>{calls.push({path,options});if(fail){fail=false;throw new Error('response lost');}return response({run:{id:'r'}});}});client.projects.set('c',project({runs:[]}));
  const options={mode:'delegated',durationSeconds:7200,maxPartners:5,autoDeliver:true};
  await assert.rejects(()=>client.start(conversation,options),/response lost/);await client.start(conversation,options);assert.equal(calls[0].options.headers['Idempotency-Key'],calls[1].options.headers['Idempotency-Key']);const body=JSON.parse(calls[1].options.body);assert.equal(body.duration_seconds,7200);assert.equal(body.start_authorized,true);assert.equal(body.wait_policy,'critical_only');
  await client.start({...conversation,researchStartKey:'key',researchStartOptions:options},options);assert.match(calls.at(-1).path,/research-start$/);
});

test('classic pause stop and evidence challenge retain v2 version and no-paid-review safety',async()=>{
  const calls=[];const client=new ClassicResearchClient({fetchImpl:async(path,options)=>{calls.push(JSON.parse(options.body));return response({command:{status:'accepted'}});}});const p=project();
  await client.command(p,'pause_run');await client.command(p,'stop_run');await client.command(p,'challenge_evidence',{node:{id:'n',revision:4},text:'缺条件'});
  assert.equal(calls[0].apply_at,'immediate');assert.equal(calls[1].expected_versions.control_epoch,1);assert.equal(calls[2].target.revision,4);assert.equal(calls[2].payload.request_paid_review,false);
});

test('classic ended records stop polling unless independent work exists and result conflicts remain explicit',()=>{
  assert.equal(needsResearchPolling(project()),true);assert.equal(needsResearchPolling(project({runs:[{state:'ended'}]})),false);assert.equal(needsResearchPolling(project({runs:[{state:'ended'}],interactions:[{state:'running'}]})),true);
  assert.match(researchResultCaption({result_state:'unresolved'},{result_state:'reviewed_solution'}),/状态不一致/);assert.equal(displayResearchPath('\\\\?\\F:\\成果'),'F:\\成果');
});

test('classic independent explanation reuses owner and message key after response loss and page reload',async()=>{
  const saved=new Map();const storage={getItem:k=>saved.get(k),setItem:(k,v)=>saved.set(k,v)};const calls=[];let first=true;
  const fetchImpl=async(path,options)=>{const body=options.body?JSON.parse(options.body):null;calls.push({path,body,key:options.headers['Idempotency-Key']});if(path.endsWith('/snapshot'))return response({project:project({runs:[{id:'r',state:'ended'}]})});if(path.endsWith('/discussions'))return response({discussion:{id:'d'}});if(path.endsWith('/interaction-executions'))return response({interaction:{id:'i'}});if(path.endsWith('/messages')&&first){first=false;throw new Error('lost accepted reply');}return response({});};
  const client=new ClassicResearchClient({fetchImpl,storage});await assert.rejects(()=>client.discuss(conversation,'问题',{authorize:()=>true}),/lost accepted/);
  const restored=new ClassicResearchClient({fetchImpl,storage});await restored.discuss(conversation,'问题',{authorize:()=>{throw new Error('must reuse authorization');}});
  assert.equal(calls.filter(c=>c.path.endsWith('/interaction-executions')).length,1);const messages=calls.filter(c=>c.path.endsWith('/messages'));assert.equal(messages.length,2);assert.equal(messages[0].key,messages[1].key);assert.deepEqual(messages[0].body,messages[1].body);assert.equal(restored.pendingMessages.size,0);
});

test('classic explicit interaction cancellation clears matching pending explanation',async()=>{
  const client=new ClassicResearchClient({fetchImpl:async()=>response({})});client.pendingMessages.set('pending',{projectId:'p',owner:{kind:'interaction',id:'i'}});await client.cancelInteraction(project({interactions:[{id:'i',revision:2}]}),'i');assert.equal(client.pendingMessages.size,0);
});

test('classic critical question is visible and answered explicitly rather than hidden in readonly chat',async()=>{
  const p=project({runs:[{id:'r',state:'waiting_human',question:'是否假设 Noetherian？'}]});const view=classicProjection(conversation,p);assert.match(view.conversation.messages.at(-1).content,/是否假设 Noetherian/);
  const html=renderClassicResearchBoard(view.board,{escapeHtml,renderText:escapeHtml,graphCards:''});assert.match(html,/猫猫提问卡/);assert.match(html,/data-classic-action="answer"/);assert.match(html,/答复并继续研究/);
  const app=await fs.readFile(new URL('../public/app.js',import.meta.url),'utf8');assert.match(app,/new ResearchWhiteboard/);const newView=await fs.readFile(new URL('../public/whiteboard-view.js',import.meta.url),'utf8');assert.match(newView,/this\.api\.answer\(p,q,/);assert.match(app,/普通发送仍是旁观讨论|classicResearch\.discuss/);assert.match(app,/人工参与/);assert.doesNotMatch(app,/此研究内核暂不支持/);const {questionsHtml}=await import('../public/whiteboard-lab.js');const current=project({runs:[{id:'r',state:'waiting_human',human_question_id:'q'}],human_questions:[{id:'q',run_id:'r',state:'open',problem_version:1,question:'是否假设 Noetherian？'}]});assert.match(questionsHtml(current,escapeHtml),/回答这条问题/);
});

test('classic history keeps note metadata stable instead of attaching later unrelated logs',()=>{
  const p=project({sessions:[{id:'main',role:'main',run_id:'r'}],nodes:[{id:'n',node_type:'note',body:'旧回复',author:'main',problem_version:1,assurance:'unreviewed',validity:'current'}]});
  const a=classicProjection(conversation,p,[{type:'tool.started',payload:{text:'旧事件'}}]);const b=classicProjection(conversation,p,[{type:'tool.started',payload:{text:'新的无关事件'}}]);assert.deepEqual(a.conversation.messages.find(m=>m.id==='v2-note-n').activityDetails,b.conversation.messages.find(m=>m.id==='v2-note-n').activityDetails);
});

test('classic record deletion confirms project-index semantics and catches active-task rejection',async()=>{
  const app=await fs.readFile(new URL('../public/app.js',import.meta.url),'utf8');assert.match(app,/const hasRecords = Number\(conversation\.messageCount\) > 0 \|\| isClassicResearch\(conversation\)/);assert.match(app,/项目、研究记录与成果文件仍保留/);assert.match(app,/catch\(error\)\{alert\("无法删除对话："\+error\.message\)/);assert.match(app,/catch\(error\)\{alert\("无法移除工作区："\+error\.message\)/);
});
