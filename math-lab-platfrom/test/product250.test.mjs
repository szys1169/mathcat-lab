import test from 'node:test';
import assert from 'node:assert/strict';
import {researchStartOptions,chooseResearchStart} from '../public/research-start.js';
import {createWorkspaceMemory} from '../public/workspace-memory.js';
import {attentionItems,deliveryHtml,projectModelsPaused} from '../public/whiteboard-delivery.js';
import {WhiteboardClient} from '../public/whiteboard-client.js';

test('startup preserves actual frozen legacy options and validates new limits',()=>{
  assert.deepEqual(researchStartOptions({reviewMode:'strict',durationSeconds:3600,maxPartners:3,autoDeliver:false}),{mode:'collaborative',durationSeconds:3600,maxPartners:3,autoDeliver:false});
  assert.equal(researchStartOptions().autoDeliver,true);
  assert.throws(()=>researchStartOptions({maxPartners:1.5}),/整数/);
  assert.throws(()=>researchStartOptions({durationSeconds:0}),/研究时间/);
});
test('canceling startup resolves without submitting or persisting any choice',async()=>{
  const controls=new Map(),dialog={innerHTML:'',setAttribute(){},querySelector(key){if(!controls.has(key))controls.set(key,{});return controls.get(key);},showModal(){},close(){this.onclose?.();},remove(){this.removed=true;}};
  const documentImpl={createElement:()=>dialog,body:{append(){}}};
  const result=chooseResearchStart({documentImpl});
  controls.get('[data-start-cancel]').onclick();
  assert.equal(await result,null);assert.equal(dialog.removed,true);
});
test('workspace keeps draft and reading state isolated by conversation and survives reopening',()=>{
  const data=new Map(),storage={getItem:k=>data.get(k),setItem:(k,v)=>data.set(k,v)};
  const a=createWorkspaceMemory(storage);a.saveDraft('conversation:a','尚未发送的数学建议');a.saveDraft('conversation:b','另一题');a.saveSession({conversationId:'a',activeView:'board'});a.saveView('a',{activeView:'board',chatScroll:72});
  const b=createWorkspaceMemory(storage);assert.equal(b.draft('conversation:a'),'尚未发送的数学建议');assert.equal(b.draft('conversation:b'),'另一题');assert.equal(b.session().conversationId,'a');assert.equal(b.view('a').chatScroll,72);
});
const project=()=>({id:'p',runs:[{id:'r',created_at:'2026-09-08',state:'running',mode:'collaborative'}],planning_proposals:[{id:'a',run_id:'r',state:'pending',reason:'分工安排'},{id:'old',run_id:'old',state:'pending'}],human_questions:[{id:'q',run_id:'r',status:'open',body:'定义域是什么？'}],project_control:{state:'paused',outstanding_cancellation:false},delivery:{current:{kind:'paper',state:'failed',steps:[{id:'zh',label:'中文稿',state:'completed'},{id:'compile_en',label:'英文排版',state:'failed',error:'缺少字体',retryable:true}],files:[{path:'论文/中文稿.pdf',kind:'pdf',language:'zh',name:'中文稿',url:'javascript:alert(1)'},{path:'论文/中文稿.tex',kind:'tex',language:'zh'}],warnings:[]},records:[],history:[]}});
test('attention includes only current-run approvals, questions and delivery failures',()=>{
  assert.deepEqual(attentionItems(project()).map(i=>i.id),['plan:a','question:q','delivery:compile_en']);
});
test('paused delivery keeps downloads available and disables model retries',()=>{
  const p=project(),html=deliveryHtml(p);assert.equal(projectModelsPaused(p),true);
  assert.match(html,/预览 PDF/);assert.match(html,/下载 LaTeX/);assert.match(html,/data-wb-action="delivery-start" disabled/);
  assert.doesNotMatch(html,/javascript:/);assert.match(html,/files\?path=/);assert.match(html,/download=1/);
});
test('project control and local retry preserve idempotency after lost response',async()=>{
  const writes=[];let fail=true;
  const classic={path:(p,s)=>'/api/v2/research/projects/'+p.id+s,async request(url,options){writes.push({url,...options});if(fail){fail=false;throw Object.assign(new Error('response lost'),{status:503});}return {ok:true};}};
  const storage={getItem:()=>null,setItem(){}};const client=new WhiteboardClient(classic,{storage});
  await assert.rejects(client.control({id:'p'},'pause'),/response lost/);
  await client.control({id:'p'},'pause');
  assert.equal(writes[0].url,'/api/research-projects/p/control');assert.equal(writes[0].idempotencyKey,writes[1].idempotencyKey);assert.deepEqual(writes[1].body,{type:'pause'});
  await client.deliveryRetry({id:'p'},'compile_en');assert.deepEqual(writes[2].body,{step:'compile_en'});assert.equal(writes[2].url,'/api/research-projects/p/delivery/retry');
});
