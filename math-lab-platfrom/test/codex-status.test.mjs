import test from 'node:test';
import assert from 'node:assert/strict';
import {CodexStatusController,normalizeCodexStatus,durationLabel,rateLimitRows,quotaSummary,selectionSummary,selectedModelPayload} from '../public/codex-status.js';
import {chooseResearchStart} from '../public/research-start.js';
import {ClassicResearchClient} from '../public/research-v2-classic.js';

const models=[{id:'model-a',model:'model-a',display_name:'模型 A',default_reasoning_effort:'high',supported_reasoning_efforts:[{effort:'high',description:'较深入'},{effort:'low',description:'较快'}]},{id:'model-b',model:'model-b',display_name:'模型 B',default_reasoning_effort:'medium',supported_reasoning_efforts:['medium']}];
const record=(projectId=null,extra={})=>({status:'available',models_status:'available',rate_limits_status:'available',updated_at:'2026-09-08T00:00:00Z',rate_limits_updated_at:'2026-09-08T00:01:00Z',models,rate_limits:[{id:'shared',name:'共享额度',windows:[{used_percent:25,remaining_percent:75,window_minutes:180,resets_at:1800000000}]},{id:'special',name:'专属额度',windows:[{used_percent:null,remaining_percent:null,window_minutes:2880,resets_at:null}]}],selection:{scope:projectId?'project':'platform',...(projectId?{project_id:projectId}:{}),model:'model-a',reasoning_effort:'high',revision:3},...extra});
const reply=(value,status=200)=>new Response(JSON.stringify(value),{status,headers:{'content-type':'application/json'}});
const storage=()=>{const data=new Map();return {getItem:key=>data.get(key)||null,setItem:(key,value)=>data.set(key,value)};};

test('quota labels use actual windows and distinct buckets; unknown values are never zero',()=>{
  const status=normalizeCodexStatus(record());
  assert.equal(durationLabel(180),'3 小时');assert.equal(durationLabel(2880),'2 天');assert.equal(durationLabel(null),'时长未提供');
  assert.equal(rateLimitRows(status)[1].remaining_percent,null);assert.equal(rateLimitRows(status)[1].resets_at,null);
  assert.match(quotaSummary(status),/共享额度 3 小时剩余 75%/);assert.match(quotaSummary(status),/专属额度 2 天剩余 未提供/);
  assert.equal(normalizeCodexStatus({rate_limits:[{windows:[{used_percent:120}]}]}).rate_limits[0].windows[0].remaining_percent,0);
});

test('unavailable refresh preserves last confirmed data and quota acquisition time',async()=>{
  let failure=false;const controller=new CodexStatusController({storage:storage(),fetchImpl:function(){assert.equal(this,globalThis);if(failure)throw new Error('offline');return Promise.resolve(reply(record()));}});
  await controller.refresh();failure=true;await controller.refresh({force:true});
  assert.equal(controller.current.updated_at,'2026-09-08T00:00:00Z');assert.equal(controller.current.rate_limits_updated_at,'2026-09-08T00:01:00Z');
  assert.match(quotaSummary(controller.current),/额度暂不可用.*上次/);assert.match(selectionSummary(controller.current),/暂不可用.*上次模型/);
  assert.equal(controller.current.models.length,2);
});

test('project and platform payloads validate supported efforts and retain revision',()=>{
  assert.deepEqual(selectedModelPayload(record('p'),{model:'model-b',reasoning_effort:'medium'},'p'),{project_id:'p',model:'model-b',reasoning_effort:'medium',expected_revision:3});
  assert.deepEqual(selectedModelPayload(record(),{model:'model-a',reasoning_effort:'low'},null),{model:'model-a',reasoning_effort:'low',expected_revision:3});
  assert.throws(()=>selectedModelPayload(record(),{model:'model-b',reasoning_effort:'high'}),/明确支持/);
  assert.throws(()=>selectedModelPayload(record('q'),{model:'model-a',reasoning_effort:'high'},'p'),/项目设置已经切换/);
});

test('late project response cannot overwrite current project or publish its selection',async()=>{
  const pending=new Map(),changes=[];const controller=new CodexStatusController({storage:storage(),onChange:(status,id)=>changes.push(id),fetchImpl:url=>new Promise(resolve=>pending.set(url,resolve))});
  controller.setContext('p');const first=controller.refresh();controller.setContext('q');const second=controller.refresh();
  pending.get('/api/codex/status?project_id=q')(reply(record('q',{selection:{...record('q').selection,model:'model-b',reasoning_effort:'medium'}})));await second;
  pending.get('/api/codex/status?project_id=p')(reply(record('p')));await first;
  assert.equal(controller.context,'q');assert.equal(controller.current.selection.model,'model-b');assert.deepEqual(changes,['q']);assert.equal(controller.getStatus('p').selection.model,'model-a');
});

test('opening and discarding model draft is read only; saving retries uncertain request with same key',async()=>{
  const requests=[];let fail=true;const controller=new CodexStatusController({storage:storage(),fetchImpl:async(url,options={})=>{requests.push({url,...options});if(options.method==='POST'){if(fail){fail=false;throw new Error('lost response');}return reply(record('p',{selection:{...record('p').selection,model:'model-b',reasoning_effort:'medium',revision:4}}));}return reply(record('p'));}});
  controller.setContext('p');await controller.refresh();controller.draft={model:'model-b',reasoning_effort:'medium'};controller.dirty=true;controller.resetDraft();assert.equal(controller.draft.model,'model-a');assert.equal(requests.filter(r=>r.method==='POST').length,0);
  controller.draft={model:'model-b',reasoning_effort:'medium'};controller.dirty=true;assert.equal(await controller.saveSelection(),null);await controller.saveSelection();
  const writes=requests.filter(r=>r.method==='POST');assert.equal(writes.length,2);assert.equal(writes[0].headers['idempotency-key'],writes[1].headers['idempotency-key']);assert.equal(writes[0].body,writes[1].body);assert.equal(JSON.parse(writes[0].body).project_id,'p');assert.equal(controller.current.selection.revision,4);
});

test('fresh partial catalog supports its listed models, while stale cached models require refresh',async()=>{
  const writes=[];let stale=false;const controller=new CodexStatusController({storage:storage(),fetchImpl:async(url,options={})=>{if(options.method==='POST')writes.push(JSON.parse(options.body));return reply(record(null,{models_status:'partial',models_stale:stale}));}});
  await controller.refresh();controller.draft={model:'model-b',reasoning_effort:'medium'};controller.dirty=true;await controller.saveSelection();assert.equal(writes.length,1);assert.equal('project_id' in writes[0],false);
  stale=true;await controller.refresh();controller.draft={model:'model-b',reasoning_effort:'medium'};controller.dirty=true;await controller.saveSelection();assert.equal(writes.length,1);assert.match(controller.error,/暂不可用/);
});

test('startup preview renders escaped actual model and cancel sends no research request',async()=>{
  const controls=new Map(),dialog={innerHTML:'',setAttribute(){},querySelector(key){if(!controls.has(key))controls.set(key,{});return controls.get(key);},showModal(){},close(){this.onclose?.();},remove(){}};
  const result=chooseResearchStart({modelSummary:'模型 A <draft> · high',documentImpl:{createElement:()=>dialog,body:{append(){}}}});
  assert.match(dialog.innerHTML,/模型 A &lt;draft&gt; · high/);controls.get('[data-start-cancel]').onclick();assert.equal(await result,null);
  const startContexts=[],writes=[];const classic=new ClassicResearchClient({storage:storage(),fetchImpl:async(...args)=>writes.push(args),chooseStart:async options=>{startContexts.push(options);return null;}});
  classic.projects.set('c',{id:'p',runs:[{state:'ended',mode:'delegated'}]});assert.equal(await classic.start({id:'c',researchProjectId:'p'}),false);
  assert.equal(startContexts[0].projectId,'p');assert.equal(writes.length,0);
});
