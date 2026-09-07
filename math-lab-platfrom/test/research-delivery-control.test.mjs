import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {ResearchDeliveries} from '../src/research-delivery.mjs';
import {sourceIdentity} from '../src/delivery-files.mjs';
import {TaskManager} from '../src/task-manager.mjs';

const outputRoot=fileURLToPath(new URL('../../tests/results/',import.meta.url));
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return {promise,resolve};};
async function harness(t,{write}={}){
  await fs.mkdir(outputRoot,{recursive:true});
  const root=await fs.mkdtemp(path.join(outputRoot,'delivery-control-'));
  const project={id:'p',title:'纯模拟控制测试',workspace_path:path.join(root,'project'),problem:'测试题',problem_version:1,result_state:'unresolved',facts:[],candidates:[],artifacts:[],runs:[{id:'r',state:'running'}],project_control:{state:'running',outstanding_cancellation:false}};
  await fs.mkdir(project.workspace_path,{recursive:true});
  const calls=[],writes=[],client={async request(url,options){if(url.endsWith('/snapshot'))return {project:structuredClone(project)};if(url.endsWith('/project-control')){calls.push(structuredClone(options));if(client.failControl){client.failControl=false;throw Object.assign(new Error('模拟控制响应失败'),{status:503});}project.project_control.state={pause:'paused',resume:'running',stop:'stopped'}[options.body.type];return {project_control:project.project_control};}throw new Error('Unexpected fake endpoint '+url);}};
  const writer={async prepare({language}){return {language,runDir:root,writerDir:root};},async write(input){writes.push(input.language);return write?.(input);},async finalize(){throw new Error('此控制测试不应运行排版步骤');}};
  const config={runtimeRoot:path.join(root,'runtime'),appRoot:fileURLToPath(new URL('../',import.meta.url))};
  const service=await new ResearchDeliveries({config,client,writer}).load();
  t.after(async()=>{service.close();for(const work of service.active.values())await work.promise;const resolved=path.resolve(root),relative=path.relative(path.resolve(outputRoot),resolved);assert(relative&&!relative.startsWith('..')&&!path.isAbsolute(relative));await fs.rm(resolved,{recursive:true,force:true});});
  const addJob=(state='queued')=>{const job={id:'j',run_id:'r',state,kind:'paper',source_key:sourceIdentity(project),created_at:new Date().toISOString(),updated_at:new Date().toISOString(),source:{packetPath:'not-read-in-fake-writer',selectedFiles:[]},prepared:{},results:{},files:[],warnings:[],steps:[{id:'source',label:'来源',state:'completed',attempts:1},{id:'zh',label:'中文写作',state:state==='paused'?'paused':'pending',attempts:0}]};service.row('p').jobs.push(job);return job;};
  const drain=async()=>{const work=service.active.get('p');if(work)await work.promise;};
  return {service,project,client,writer,config,calls,writes,addJob,drain};
}

test('pause remains pausing until the active writer acknowledges interruption; replay does not duplicate control',async t=>{
  const entered=deferred(),release=deferred();let observedAbort=false;
  t.after(()=>release.resolve());
  const h=await harness(t,{write:async({signal})=>{signal.addEventListener('abort',()=>{observedAbort=true;},{once:true});entered.resolve();await release.promise;}});
  h.addJob();h.service.launch('p','j');await entered.promise;
  const paused=await h.service.control('p',{type:'pause'},'pause-one');
  assert.equal(paused.control.state,'pausing');assert.equal(paused.control.outstanding_cancellation,true);assert.equal(observedAbort,true);
  await h.service.control('p',{type:'pause'},'pause-one');assert.equal(h.calls.length,1);
  await assert.rejects(h.service.control('p',{type:'resume'},'resume-too-early'),/确认暂停|等待确认/);
  assert.equal(h.calls.length,1);
  release.resolve();await h.drain();
  const stopped=await h.service.view('p');assert.equal(stopped.control.state,'paused');assert.equal(stopped.current.state,'paused');assert.deepEqual(h.writes,['zh']);
});

test('resume continues paused work once and leaves completed steps and stopped jobs untouched',async t=>{
  const h=await harness(t);h.project.project_control.state='paused';h.service.row('p').control={state:'paused'};
  const job=h.addJob('paused');const old={...structuredClone(job),id:'old-stopped',state:'stopped'};h.service.row('p').jobs.unshift(old);
  await h.service.control('p',{type:'resume'},'resume-one');await h.drain();
  assert.equal(job.state,'completed');assert.equal(job.steps[0].attempts,1);assert.equal(old.state,'stopped');assert.deepEqual(h.writes,['zh']);
  await h.service.control('p',{type:'resume'},'resume-one');await h.drain();assert.equal(h.calls.length,1);assert.deepEqual(h.writes,['zh']);
});

test('reopening persisted paused state and reading delivery never starts a model',async t=>{
  const h=await harness(t);h.project.project_control.state='paused';h.service.row('p').control={state:'paused'};h.addJob('paused');await h.service.save();
  const reopened=await new ResearchDeliveries({config:h.config,client:h.client,writer:h.writer}).load();t.after(()=>reopened.close());
  const view=await reopened.view('p');assert.equal(view.control.state,'paused');assert.equal(view.current.state,'paused');assert.equal(reopened.active.size,0);assert.equal(h.writes.length,0);assert.equal(h.calls.length,0);
});

test('failed resume leaves writing paused and permits retry with the same receipt',async t=>{
  const h=await harness(t);h.project.project_control.state='paused';h.service.row('p').control={state:'paused'};const job=h.addJob('paused');h.client.failControl=true;
  await assert.rejects(h.service.control('p',{type:'resume'},'resume-retry'),/模拟控制响应失败/);
  assert.equal(h.service.row('p').control.state,'paused');assert.equal(job.state,'paused');assert.equal(h.writes.length,0);
  await h.service.control('p',{type:'resume'},'resume-retry');await h.drain();assert.equal(job.state,'completed');assert.deepEqual(h.writes,['zh']);assert.equal(h.calls[0].idempotencyKey,h.calls[1].idempotencyKey);
});

test('the automatic-delivery choice for an existing run cannot be changed by a repeated start registration',async t=>{
  const h=await harness(t);
  await h.service.registerRun('p',{id:'r'},{autoDeliver:false});
  try{await h.service.registerRun('p',{id:'r'},{autoDeliver:true});}catch(error){assert.equal(error.status,409);}
  assert.equal(h.service.row('p').automatic_runs.r.enabled,false,'A repeated upstream run response overwrote the frozen user choice');
});

test('a legacy choice cannot launch an unmanaged model after its conversation is bound to a research project',async()=>{
  const conversation={id:'c',workspaceId:null,researchProjectId:'p',researchContract:'mathcat-research/v2',messages:[{id:'old-choice',role:'assistant',choice:{selectedIndex:null,options:[{label:'继续',value:'继续旧任务'}]},permission:'workspace-write'}]};
  const store={getConversation:()=>conversation,workspaceFor:()=>({path:outputRoot}),async updateConversation(id,fn){fn(conversation);return conversation;}};
  const manager=new TaskManager({store,config:{},capabilities:[]});let launched=0;
  manager.launch=()=>{launched++;return {accepted:true};};
  await assert.rejects(manager.choose({conversationId:'c',messageId:'old-choice',optionIndex:0}));
  assert.equal(launched,0);assert.equal(conversation.messages[0].choice.selectedIndex,null,'Rejected legacy choices must not be marked as submitted');
  await assert.rejects(manager.start({conversationId:'c',text:'旧入口重试',executor:'codex'}),error=>error.status===409);
  const resumed=await manager.resumeMathCatWatcher('c',{status:'running'});assert.equal(resumed.accepted,false);assert.equal(launched,0);assert.equal(conversation.messages.length,1);
});

test('last-minute research binding is checked again at the final legacy dispatch boundary',()=>{
  const manager=new TaskManager({store:{getConversation:()=>({researchProjectId:'p'})},config:{},capabilities:[]});let executed=false;
  assert.throws(()=>manager.launch({conversationId:'c',conversation:{id:'c'},runner:()=>{executed=true;}}),error=>error.status===409);
  assert.equal(executed,false);assert.equal(manager.running.size,0);
});
