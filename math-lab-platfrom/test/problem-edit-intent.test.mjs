import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {explicitProblemEdit} from '../public/problem-edit-intent.js';
import {ResearchV2Bridge} from '../src/research-v2-bridge.mjs';
import {Store} from '../src/store.mjs';
import {ClassicResearchClient} from '../public/research-v2-classic.js';

test('direct conversation edits preserve complete mathematical text and do not interpret questions as instructions',()=>{
  const math='对所有 n≥2，存在 C(n)>0。\n证明：x≤C(n)y；不能令 C 与 n 无关。';
  assert.deepEqual(explicitProblemEdit('请将数学问题修改为：\n'+math),{math_statement:math});
  assert.deepEqual(explicitProblemEdit('研究说明改为：限时 2h，先尝试直接证明'),{research_description:'限时 2h，先尝试直接证明'});
  for(const text of ['是否应将数学问题改为：P？','建议将数学问题改为：P','他说“将数学问题改为：P”','将数学问题改为：','请解释研究说明'])assert.equal(explicitProblemEdit(text),null,text);
});

async function setup(client){
  const root=await fs.mkdtemp(path.join(os.tmpdir(),'mathcat-edit24-'));
  const store=await new Store(root).load();const conversation=await store.createConversation({title:'题面修改验收'});
  await store.updateConversation(conversation.id,row=>{row.researchProjectId='p1';});
  return {store,conversation,bridge:new ResearchV2Bridge({store,client})};
}

test('conversation edit uses the same versioned API, keeps a receipt and survives retry after restart',async()=>{
  const calls=[];const result={problem_spec:{revision:3,math_statement:'P'}};
  const {bridge,store,conversation}=await setup({request:async(url,input)=>{calls.push({url,...input});return result;}});
  const input={text:'将数学问题改为：P',expected_revision:2};
  assert.deepEqual(await bridge.editProblem(conversation.id,input,'edit-once'),result);
  assert.deepEqual(calls[0].body,{expected_revision:2,math_statement:'P',source:'conversation_edit'});
  assert.equal(calls[0].idempotencyKey,'edit-once');
  const restored=await new Store(store.runtimeRoot).load();
  const after=new ResearchV2Bridge({store:restored,client:{request:()=>assert.fail('Already applied; do not rewrite or call models.')}});
  assert.deepEqual(await after.editProblem(conversation.id,input,'edit-once'),result);
  assert.equal(restored.getConversation(conversation.id).messages.length,2);
  await assert.rejects(()=>after.editProblem(conversation.id,{...input,text:'将数学问题改为：Q'},'edit-once'),error=>error.status===409);
});

test('a rejected stale edit does not write a success message or alter local problem records',async()=>{
  const {bridge,store,conversation}=await setup({request:async()=>{throw Object.assign(new Error('stale revision'),{status:409});}});
  await assert.rejects(()=>bridge.editProblem(conversation.id,{text:'将研究说明改为：新说明',expected_revision:1},'stale'),error=>error.status===409);
  assert.equal(store.getConversation(conversation.id).messages.length,0);
  assert.equal(store.getConversation(conversation.id).researchProblemEdits,undefined);
});

test('new research defaults to automatic; human mode and retry retain the authorized mode',async()=>{
  for(const mode of [undefined,'automatic','strict','balanced']){
    const root=await fs.mkdtemp(path.join(os.tmpdir(),'mathcat-mode24-'));
    const store=await new Store(root).load();const c=await store.createConversation({title:'模式验收'});const calls=[];let fail=true;
    const client={request:async(url,input)=>{if(url.endsWith('/runs')){calls.push(input);if(fail){fail=false;throw new Error('lost response');}return {run:{id:'r1'}};}return {project:{id:'p1',runs:[]}};}};
    const bridge=new ResearchV2Bridge({store,client});
    await assert.rejects(()=>bridge.start(c.id,'证明 P',mode?{reviewMode:mode}:undefined),/lost response/);
    await bridge.start(c.id,'证明 P',{reviewMode:mode==='automatic'?'strict':'automatic'});
    assert.equal(calls[0].body.mode,mode&&mode!=='automatic'?'collaborative':'delegated');
    assert.deepEqual(calls[1],calls[0]);
  }
});

test('invalid startup bounds and modes fail before project creation',async()=>{
  const bridge=new ResearchV2Bridge({store:{},client:{request:()=>assert.fail('No side effects for invalid settings')}});
  for(const options of [{durationSeconds:-1},{durationSeconds:NaN},{reviewMode:'typo'}])await assert.rejects(()=>bridge.start('none','P',options),error=>error.status===422);
});

test('recovery of an already active run repairs the durable conversation receipt',async()=>{
  const {bridge,store,conversation}=await setup({request:async()=>({project:{id:'p1',runs:[{id:'running-run',state:'running'}]}})});
  const result=await bridge.start(conversation.id,'原题');
  assert.equal(result.reused,true);assert.equal(store.getConversation(conversation.id).researchRunId,'running-run');
  const restored=await new Store(store.runtimeRoot).load();assert.equal(restored.getConversation(conversation.id).status,'running');
  assert.equal(restored.getConversation(conversation.id).researchRunId,'running-run');
});

test('browser edit retry freezes its original version and key across a page reload',async()=>{
  const items=new Map(),storage={getItem:k=>items.get(k),setItem:(k,v)=>items.set(k,v)};
  const calls=[];let lose=true;
  const fetchImpl=async(url,options)=>{calls.push({url,...options});if(lose){lose=false;throw new Error('response lost after write');}return new Response(JSON.stringify({problem_spec:{revision:8}}));};
  const c={id:'c',researchProjectId:'p'},text='将数学问题改为：P';
  const first=new ClassicResearchClient({storage,fetchImpl});
  await assert.rejects(()=>first.editProblem(c,text,5),/response lost/);
  const reloaded=new ClassicResearchClient({storage,fetchImpl});
  await assert.rejects(()=>reloaded.editProblem(c,'将数学问题改为：Q',9),/原文重试/);
  await reloaded.editProblem(c,text,9);
  assert.equal(calls.length,2);assert.equal(calls[1].body,calls[0].body);assert.equal(JSON.parse(calls[1].body).expected_revision,5);
  assert.equal(calls[1].headers['Idempotency-Key'],calls[0].headers['Idempotency-Key']);
  assert.deepEqual(JSON.parse(items.get('mathcat.2.4.classic.problem-edits')),[]);
});
