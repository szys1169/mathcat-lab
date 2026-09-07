import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {CodexSettings,effectiveProjectModel,modelArguments} from '../src/codex-settings.mjs';

async function fixture(){
  const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../tests/results/codex-settings');await fs.mkdir(root,{recursive:true});const runtimeRoot=await fs.mkdtemp(path.join(root,'fixture-'));
  const account={async read(){return {status:'available',rate_limits:[],models:[{id:'model-a',supported_reasoning_efforts:[{effort:'high'}]},{id:'model-b',supported_reasoning_efforts:[{effort:'low'}]}],configured:{model:'model-a',reasoning_effort:'high'}};}};
  const projects={p:{id:'p',runs:[{model:'model-a',reasoning_effort:'high',model_selection_revision:0}],sessions:[{id:'s',model:'model-a',reasoning_effort:'high',role:'main'}],usage:[{session_id:'s',state:'running',model:'model-a',reasoning_effort:'high'}]},q:{id:'q',runs:[]}},writes=[];
  const client={async request(url,options){const id=url.split('/')[5],project=projects[id];if(options?.method==='POST'){assert.equal(options.body.expected_revision,project.model_selection?.revision??0);writes.push({id,...options.body});project.model_selection={...options.body,revision:(project.model_selection?.revision??0)+1};}return {project:structuredClone(project)};}};
  const config={runtimeRoot};const settings=await new CodexSettings({config,client,account}).load();return {settings,account,config,client,projects,writes};
}

test('platform model persists while current project and running call retain their own selection',async()=>{
  const f=await fixture();await f.settings.select({model:'model-b',reasoning_effort:'low',expected_revision:0},'change');
  const reopened=await new CodexSettings(f).load();assert.deepEqual(await reopened.defaultSelection(),{model:'model-b',reasoning_effort:'low'});
  const current=await reopened.status({project_id:'p'});assert.equal(current.selection.model,'model-a');assert.equal(current.active_calls[0].model,'model-a');assert.equal(f.writes.length,0);
});

test('project switch affects only that project and does not rewrite running usage; replay is harmless',async()=>{
  const f=await fixture(),input={project_id:'p',model:'model-b',reasoning_effort:'low',expected_revision:0};const before=JSON.stringify(f.projects.p.usage);
  const value=await f.settings.select(input,'switch');assert.equal(value.selection.model,'model-b');assert.equal(value.active_calls[0].model,'model-a');assert.equal(JSON.stringify(f.projects.p.usage),before);
  await f.settings.select(input,'switch');assert.equal(f.writes.length,1);assert.equal((await f.settings.status({project_id:'q'})).selection.model,'model-a');
  await assert.rejects(f.settings.select({...input,model:'model-a'},'switch'),/相同回执/);
});

test('unsupported model or effort and stale default revision are rejected before mutation',async()=>{
  const f=await fixture();await assert.rejects(f.settings.select({model:'unknown',reasoning_effort:'high'},'bad-model'),/可用列表/);
  await assert.rejects(f.settings.select({model:'model-b',reasoning_effort:'high'},'bad-effort'),/不支持/);
  await assert.rejects(f.settings.select({model:'model-a',reasoning_effort:'high',expected_revision:9},'stale'),/别处更新/);assert.equal(f.writes.length,0);
});

test('a lost start response retries the frozen model after a later model change',async()=>{
  const f=await fixture(),input={start_authorized:true,duration_seconds:120,auto_deliver:false};const first=await f.settings.runInput('p',input,'start');
  await f.settings.select({project_id:'p',model:'model-b',reasoning_effort:'low',expected_revision:0},'switch');
  assert.deepEqual(await f.settings.runInput('p',input,'start'),first);assert.equal(first.model,'model-a');assert.equal((await f.settings.runInput('p',input,'new-start')).model,'model-b');
});

test('explicit newer Run model wins until the next project selection, with safe CLI arguments',()=>{
  const project={model_selection:{model:'model-a',reasoning_effort:'high',revision:2},runs:[{model:'model-b',reasoning_effort:'low',model_selection_revision:2}]};
  assert.equal(effectiveProjectModel(project).model,'model-b');project.model_selection.revision=3;assert.equal(effectiveProjectModel(project).model,'model-a');
  assert.deepEqual(modelArguments({model:'gpt-6-astra',reasoning_effort:'high'}),['--model','gpt-6-astra','-c','model_reasoning_effort="high"']);
  assert.throws(()=>modelArguments({model:'bad" -c unsafe=true'}),/名称无效/);
});
