import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {ResearchDeliveries} from '../src/research-delivery.mjs';
import {digest,projectFile,publishLanguage} from '../src/delivery-files.mjs';

const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../tests/results/delivery-2.5');
async function setup({failEnglish=0}={}){
  await fs.mkdir(root,{recursive:true});const workspace=await fs.mkdtemp(path.join(root,'fixture-'));
  const proof=Buffer.from('Fixture only. For any real x, x² ≥ 0 by order axioms.');
  const project={id:'fixture',title:'Engineering fixture',problem:'For every real x, prove x² ≥ 0.',problem_version:2,workspace_path:workspace,result_state:'reviewed_solution',project_control:{state:'running'},runs:[{id:'run1',state:'ended',stop_reason:'goal_satisfied'}],facts:[{id:'f1',candidate_id:'c1',claim:'For every real x, x² ≥ 0.',validity:'current',assurance:'model_reviewed',problem_version:2,proof_artifact_id:'a1',proof_sha256:digest(proof)}],candidates:[{id:'old',claim:'OLD PROBLEM SHOULD NOT APPEAR',problem_version:1,status:'submitted'},{id:'c1',claim:'For every real x, x² ≥ 0.',problem_version:2,status:'reviewed'}],artifacts:[{id:'a1',sha256:digest(proof)}]};
  const counts={prepare_zh:0,prepare_en:0,write_zh:0,write_en:0,compile_zh:0,compile_en:0};
  const writer={
    async prepare({language,taskId}){counts['prepare_'+language]++;const runDir=path.join(workspace,'论文','versions',taskId),writerDir=path.join(runDir,'writer');await fs.mkdir(writerDir,{recursive:true});await fs.writeFile(path.join(runDir,'task.json'),JSON.stringify({workspace,version:taskId,language}));return {runDir,writerDir,version:taskId,language};},
    async write({prepared,language,primary}){counts['write_'+language]++;await fs.writeFile(path.join(prepared.writerDir,'article_candidate.tex'),'Fixture '+language+' '+counts['write_'+language]+' \\label{theorem} $x^2\\geq0$');if(primary){const bytes=await fs.readFile(path.join(primary.writerDir,'article_candidate.tex'));await fs.writeFile(path.join(prepared.runDir,'primary_source.json'),JSON.stringify({runDir:await fs.realpath(primary.runDir),fingerprint:digest(JSON.stringify([{path:'article_candidate.tex',sha256:digest(bytes)}]))}));}},
    async finalize({prepared}){const language=prepared.language;counts['compile_'+language]++;if(language==='en'&&failEnglish-->0)return {status:'partial',compile_status:'failed',render_status:'not_run',errors:['fixture compile failure']};
      const tex=await fs.readFile(path.join(prepared.writerDir,'article_candidate.tex')),pdf=Buffer.from('%PDF-1.7 fixture '+language);await fs.writeFile(path.join(prepared.writerDir,'article_candidate.pdf'),pdf);
      const source_files=[{path:'article_candidate.tex',sha256:digest(tex)}];return {version:prepared.version,status:'completed',compile_status:'passed',render_status:'passed',source_files,source_fingerprint:digest(JSON.stringify(source_files)),artifacts:[{type:'article_tex',path:'writer/article_candidate.tex',sha256:digest(tex)},{type:'article_pdf',path:'writer/article_candidate.pdf',sha256:digest(pdf)}]};}
  };
  const config={runtimeRoot:path.join(workspace,'.runtime')};
  const client={async request(url,options){if(options?.body?.type)project.project_control.state={pause:'paused',resume:'running',stop:'stopped'}[options.body.type];return structuredClone(project);}};
  const service=await new ResearchDeliveries({config,client,writer,readArtifact:async()=>({bytes:proof,artifact:project.artifacts[0]})}).load();
  const settle=async()=>{while(service.active.has(project.id))await service.active.get(project.id).promise;return service.view(project.id);};
  return {workspace,project,service,counts,settle,config,client,writer};
}

test('English compile retry keeps Chinese files and both completed writing steps',async()=>{
  const f=await setup({failEnglish:1});await f.service.start('fixture',{},'first');let view=await f.settle();
  assert.equal(view.current.state,'failed');assert.equal(view.current.steps.find(s=>s.id==='compile_en').state,'failed');
  assert.equal(view.current.files.filter(file=>file.language==='zh').length,3);assert(view.records.some(r=>r.name==='source-packet.json'));
  await f.service.retry('fixture',{step:'compile_en'},'retry');view=await f.settle();assert.equal(view.current.state,'completed',view.current.error);
  assert.deepEqual(f.counts,{prepare_zh:1,prepare_en:1,write_zh:1,write_en:1,compile_zh:1,compile_en:2});
  assert.equal(view.current.files.filter(x=>x.kind==='pdf').length,2);
  const zip=view.current.files.find(x=>x.kind==='source');assert.equal((await fs.readFile(await f.service.registeredFile('fixture',zip.path))).readUInt32LE(0),0x04034b50);
  for(let i=0;i<3;i++)await f.service.view('fixture');assert.equal(f.counts.write_en,1);
  await assert.rejects(f.service.registeredFile('fixture','../secret'),/尚未登记/);
});

test('English rewrite after failure invalidates only English and subsequent steps',async()=>{
  const f=await setup({failEnglish:1});await f.service.start('fixture',{},'first');let view=await f.settle();assert(view.current.steps.find(s=>s.id==='en').retryable);
  await f.service.retry('fixture',{step:'en'},'rewrite');view=await f.settle();assert.equal(view.current.state,'completed',view.current.error);
  assert.equal(f.counts.write_zh,1);assert.equal(f.counts.compile_zh,1);assert.equal(f.counts.write_en,2);assert.equal(f.counts.prepare_en,1);
});

test('stage report can upgrade to a paper and excludes superseded problem candidates',async()=>{
  const f=await setup();await f.service.start('fixture',{stageOnly:true},'stage');let view=await f.settle();assert.equal(view.current.kind,'stage_report');
  const text=await fs.readFile(await f.service.registeredFile('fixture',view.current.files[0].path),'utf8');assert(!text.includes('OLD PROBLEM'));
  await f.service.start('fixture',{},'paper');view=await f.settle();assert.equal(view.current.kind,'paper');assert.equal(view.current.state,'completed',view.current.error);assert.equal(view.history.length,1);
});

test('queued startup interruption remains retryable without auto execution',async()=>{
  const f=await setup();f.service.launch=()=>{};await f.service.start('fixture',{},'queue');
  const reopened=await new ResearchDeliveries({config:f.config,client:f.client,writer:f.writer}).load();const view=await reopened.view('fixture');assert.equal(view.current.state,'interrupted');assert.equal(view.current.steps[0].state,'interrupted');assert(view.current.steps[0].retryable);assert.equal(f.counts.write_zh,0);
});

test('stop emits a mechanical progress report and refuses model delivery',async()=>{
  const f=await setup();await f.service.registerRun('fixture',f.project.runs[0],{autoDeliver:true});await f.service.control('fixture',{type:'stop'},'stop');await f.service.observe();const view=await f.settle();
  assert.equal(view.current.kind,'stage_report');assert.equal(view.current.state,'completed',view.current.error);assert.equal(view.control.state,'stopped');assert.equal(f.counts.write_zh,0);await assert.rejects(f.service.start('fixture',{},'paper'),/停止或暂停/);
});

test('revoked sources block old retries and a changed source tree cannot be published',async()=>{
  const f=await setup({failEnglish:1});await f.service.start('fixture',{},'first');await f.settle();f.project.facts[0].validity='withdrawn';await assert.rejects(f.service.retry('fixture',{step:'compile_en'},'stale'),/来源已变化/);
  const job=f.service.row('fixture').jobs[0];await fs.writeFile(path.join(job.prepared.zh.writerDir,'extra.tex'),'unexpected source');await assert.rejects(publishLanguage(f.project,job,'zh'),/完整论文源文件在检查后发生变化/);
  await assert.rejects(projectFile(f.project,'../secret'),/路径无效/);await assert.rejects(projectFile(f.project,'成果/x:ads'),/路径无效/);
});

test('automatic bilingual opt out never runs paper models',async()=>{
  const f=await setup();await f.service.registerRun('fixture',f.project.runs[0],{autoDeliver:false});await f.service.observe();const view=await f.settle();assert.equal(view.current.kind,'stage_report');assert.equal(f.counts.write_en,0);assert.equal(f.counts.write_zh,0);
});
