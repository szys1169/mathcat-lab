import fs from 'node:fs/promises';
import path from 'node:path';
import crypto from 'node:crypto';
import {QueuedJsonWriter} from './fs-utils.mjs';
import {V2_PREFIX,unwrapProject} from './research-v2-client.mjs';
import {createPaperWriter} from './paper-writer.mjs';
import {runCodex} from './executors.mjs';
import {syncResearchConversationStatus} from './research-conversation-status.mjs';
import {deliveryError,safeSegment,projectFile,sourceIdentity,freezeSource,readEvidence,stageReports,publishPair,publishLanguage,unixPath} from './delivery-files.mjs';

const now=()=>new Date().toISOString();
const STEP_NAMES={source:'整理精确成果来源',zh:'中文论文写作',compile_zh:'中文编译与页面检查',en:'英文稿同步',compile_en:'英文编译与页面检查',sync:'双语交付检查与发布'};
const ACTIVE=new Set(['queued','running']);
const PAUSING=new Set(['pausing','paused','stopping','stopped']);
const clone=value=>structuredClone(value);

export class ResearchDeliveries {
  constructor({config,client,store,writer,readArtifact,modelSettings}={}){
    this.config=config;this.client=client;this.store=store;this.file=path.join(config.runtimeRoot,'research-deliveries.json');
    this.storage=new QueuedJsonWriter(this.file);this.state={schema:1,projects:{}};this.active=new Map();this.locks=new Map();this.observing=false;
    this.readArtifact=readArtifact||((project,id)=>readEvidence(this.client,project,id));this.modelSettings=modelSettings;
    this.writer=writer||createPaperWriter({config,capabilityRoot:path.resolve(config.appRoot,'../mathcat-lab/capabilities/math-paper-writing'),runModel:async({workspace,prompt,signal,onProgress,taskDir,modelSelection})=>
      runCodex({config,workspace:typeof workspace==='string'?{path:workspace}:workspace,conversation:{messages:[]},text:prompt,permission:'workspace-write',taskDir,signal,onProgress,modelSelection})});
  }
  async load(){
    try{this.state=JSON.parse(await fs.readFile(this.file,'utf8'));if(this.state.schema!==1||!this.state.projects)throw new Error('交付状态文件版本不支持。');}
    catch(error){if(error.code!=='ENOENT')throw error;}
    for(const row of Object.values(this.state.projects))for(const job of row.jobs){if(ACTIVE.has(job.state)){job.state='interrupted';job.updated_at=now();job.warnings.push('平台重新启动；已保留产物，请选择未完成步骤继续。');const step=job.steps.find(s=>!['completed','skipped'].includes(s.state));if(step)step.state='interrupted';}}
    await this.save();return this;
  }
  save(){return this.storage.save(this.state);}
  row(id){if(!safeSegment(id))throw deliveryError('研究项目编号无效。',422);return this.state.projects[id]??={control:{state:'running'},automatic_runs:{},handled_runs:{},jobs:[],receipts:{}};}
  async project(id){if(!safeSegment(id))throw deliveryError('研究项目编号无效。',422);return unwrapProject(await this.client.request(V2_PREFIX+'/projects/'+encodeURIComponent(id)+'/snapshot'));}
  async locked(id,action){const before=this.locks.get(id)||Promise.resolve();const task=before.catch(()=>{}).then(action);this.locks.set(id,task);try{return await task;}finally{if(this.locks.get(id)===task)this.locks.delete(id);}}
  startMonitor(){if(this.timer)return;this.timer=setInterval(()=>{void this.observe().catch(error=>{this.lastMonitorError=error.message;});},2500);this.timer.unref?.();}
  close(){clearInterval(this.timer);this.timer=null;for(const work of this.active.values())work.controller.abort();}
  async registerRun(projectId,run,options={}){
    if(!run?.id)return;await this.locked(projectId,async()=>{const row=this.row(projectId),existing=row.automatic_runs[run.id],enabled=options.autoDeliver!==false;if(existing){if(existing.enabled!==enabled)throw deliveryError('此轮研究的双语交付选择已冻结，重复请求不能更改。');return;}row.automatic_runs[run.id]={enabled,registered_at:now()};if(row.control.state==='stopped')row.control={state:'running',updated_at:now()};await this.save();});
  }
  async observe(){
    if(this.observing)return;this.observing=true;
    try{
      const conversations=this.store?.listConversations?.()||[];
      const ids=new Set([...Object.keys(this.state.projects),...conversations.filter(c=>c.researchProjectId).map(c=>c.researchProjectId)]);
      for(const id of ids){
        try{
          const project=await this.project(id),row=this.row(id);
          if(['pausing','paused','stopping','stopped'].includes(project.project_control?.state)&&this.active.has(id)){
            const activeJob=row.jobs.find(j=>j.id===this.active.get(id).jobId);
            if(activeJob?.kind==='paper')this.active.get(id).controller.abort();
          }
          const projectConversations=conversations.filter(c=>c.researchProjectId===id);
          for(const conversation of projectConversations)await syncResearchConversationStatus(this.store,conversation,project);
          const conversation=projectConversations[0];
          const run=(project.runs||[]).at(-1);if(!run)continue;
          if(!row.automatic_runs[run.id]&&conversation?.researchRunId===run.id){row.automatic_runs[run.id]={enabled:conversation.researchStartOptions?.autoDeliver!==false,registered_at:now()};await this.save();}
          if(run.state!=='ended'||row.handled_runs[run.id]||!row.automatic_runs[run.id])continue;
          if(row.control.state==='pausing'||row.control.state==='paused'||['pausing','paused'].includes(project.project_control?.state))continue;
          const canWrite=row.automatic_runs[run.id].enabled&&row.control.state==='running'&&(!project.project_control||project.project_control.state==='running')&&run.stop_reason==='goal_satisfied'&&project.result_state==='reviewed_solution'&&!run.outstanding_cancellation;
          await this.start(id,{automatic:true,stageOnly:!canWrite,runId:run.id},'automatic-'+run.id);
        }catch(error){this.lastMonitorError=error.message;}
      }
    }finally{this.observing=false;}
  }
  async start(id,input={},key){
    if(!key)throw deliveryError('请求缺少回执标识。',422);
    return this.locked(id,async()=>{
      const project=await this.project(id),row=this.row(id),signature=JSON.stringify(input);
      const prior=row.receipts[key];if(prior){if(prior.signature!==signature||prior.action!=='start')throw deliveryError('相同回执对应不同请求。');return this.view(project);}
      if(this.active.has(id)||row.jobs.some(j=>ACTIVE.has(j.state)))throw deliveryError('当前项目已有交付任务，请等待、暂停或重试该任务。');
      if(['pausing','paused'].includes(row.control.state)||['pausing','paused'].includes(project.project_control?.state))throw deliveryError('当前项目已暂停，请先明确恢复研究。');
      const sourceKey=sourceIdentity(project),run=(project.runs||[]).find(r=>r.id===input.runId)||(project.runs||[]).at(-1);
      const hasReviewed=(project.facts||[]).some(f=>f.validity==='current'&&f.problem_version===project.problem_version&&['model_reviewed','formally_checked'].includes(f.assurance));
      const kind=input.stageOnly||!hasReviewed?'stage_report':'paper';
      if(kind==='paper'&&(PAUSING.has(row.control.state)||PAUSING.has(project.project_control?.state)))throw deliveryError('当前项目已停止或暂停，请先恢复研究再生成论文。');
      const existing=row.jobs.findLast(j=>j.kind===kind&&j.state!=='stopped'&&j.source_key===sourceKey&&(j.run_id||null)===(run?.id||null));
      if(existing){row.receipts[key]={action:'start',signature,job_id:existing.id};if(run)row.handled_runs[run.id]=existing.id;await this.save();return this.view(project);}
      const job={id:'delivery-'+crypto.randomUUID(),run_id:run?.id||null,problem_version:project.problem_version,source_key:sourceKey,state:'queued',kind,created_at:now(),updated_at:now(),steps:Object.entries(STEP_NAMES).map(([id,label])=>({id,label,state:'pending',attempts:0})),prepared:{},results:{},files:[],warnings:[]};
      if(kind==='stage_report')for(const step of job.steps)if(!['source','sync'].includes(step.id))step.state='skipped';
      row.jobs.push(job);row.receipts[key]={action:'start',signature,job_id:job.id};if(run)row.handled_runs[run.id]=job.id;
      await this.save();this.launch(id,job.id);return this.view(project);
    });
  }
  launch(id,jobId){
    if(this.active.has(id))return;const controller=new AbortController();const work={controller,jobId,promise:null};this.active.set(id,work);
    work.promise=this.run(id,jobId,controller.signal).catch(error=>{this.lastMonitorError=error.message;}).finally(()=>{if(this.active.get(id)===work)this.active.delete(id);});
  }
  async run(id,jobId,signal){
    const row=this.row(id),job=row.jobs.find(j=>j.id===jobId);let step;
    const paused=()=>signal.aborted||['pausing','paused','stopping'].includes(row.control.state)||(job.kind==='paper'&&row.control.state==='stopped');
    try{
      if(paused())throw new DOMException('任务已暂停。','AbortError');
      let project=await this.project(id);job.state='running';await this.save();
      for(step of job.steps){
        if(['completed','skipped'].includes(step.state))continue;
        if(paused())throw new DOMException('任务已暂停。','AbortError');
        project=await this.project(id);
        if(['pausing','paused','stopping'].includes(project.project_control?.state)||(job.kind==='paper'&&project.project_control?.state==='stopped'))throw new DOMException('项目已暂停。','AbortError');
        if(sourceIdentity(project)!==job.source_key)throw deliveryError('数学成果来源已更新或被撤回，旧交付已保留；请基于当前成果重新整理。');
        step.state='running';step.attempts++;step.started_at=now();delete step.error;job.current_step=step.id;job.updated_at=now();await this.save();
        if(step.id==='source'){
          if(job.kind==='paper'){job.source=await freezeSource(project,job,this.readArtifact);job.warnings=job.source.warnings;if(!job.source.hasResults)throw deliveryError('没有足够的已审查成果可用于论文写作。');}
          else job.files=await stageReports(project,job);
        }else if(['zh','en'].includes(step.id)){
          const language=step.id;
          if(!job.prepared[language]){job.prepared[language]=await this.writer.prepare({workspace:project.workspace_path,sourcePacket:job.source.packetPath,selectedFiles:job.source.selectedFiles,title:project.title,language,taskId:job.id+'-'+language,signal});await this.save();}
          const modelSelection=await this.modelSettings?.forProject(project);job.write_models??={};job.write_models[language]=modelSelection||null;await this.save();
          await this.writer.write({prepared:job.prepared[language],language,primary:language==='en'?job.prepared.zh:undefined,signal,modelSelection,onProgress:progress=>{job.progress=typeof progress==='string'?progress:progress.summary||'';}});
        }else if(step.id.startsWith('compile_')){
          const language=step.id.slice(8),result=await this.writer.finalize({prepared:job.prepared[language],signal,publishCurrent:false});job.results[language]=result;
          for(const warning of result.warnings||[])if(!job.warnings.includes(String(warning)))job.warnings.push(String(warning));
          if(result.status!=='completed'||result.compile_status!=='passed'||result.render_status!=='passed')throw deliveryError((result.errors||[]).join('；')||'编译或页面检查未通过。请查看保留的稿件并重试此步骤。');
          job.files=[...job.files.filter(f=>f.language!==language),...await publishLanguage(project,job,language)];
        }else if(step.id==='sync'&&job.kind==='paper')job.files=await publishPair(project,job);
        if(paused())throw new DOMException('任务已暂停。','AbortError');
        step.state='completed';step.finished_at=now();job.updated_at=now();await this.save();
      }
      job.state='completed';job.current_step=null;job.updated_at=now();await this.save();
    }catch(error){
      const aborted=paused()||error.name==='AbortError';
      if(step&&step.state==='running'){step.state=aborted?(row.control.state==='stopped'||row.control.state==='stopping'?'stopped':'paused'):'failed';step.error=aborted?'本步骤已中断，已有文件保留。':error.message;}
      job.state=aborted?(row.control.state==='stopped'||row.control.state==='stopping'?'stopped':'paused'):'failed';job.updated_at=now();job.error=error.message;await this.save();
    }
  }
  async retry(id,{step:stepId}={},key){
    if(!key)throw deliveryError('请求缺少回执标识。',422);
    return this.locked(id,async()=>{
      const project=await this.project(id),row=this.row(id),signature=JSON.stringify({step:stepId});
      if(row.receipts[key]){const r=row.receipts[key];if(r.action!=='retry'||r.signature!==signature)throw deliveryError('相同回执对应不同请求。');return this.view(project);}
      if(this.active.has(id))throw deliveryError('仍有交付任务在执行。');
      if(PAUSING.has(row.control.state)||['pausing','paused','stopping'].includes(project.project_control?.state))throw deliveryError('请先明确恢复当前项目，再重试交付步骤。');
      const job=row.jobs.at(-1),step=job?.steps.find(s=>s.id===stepId);
      if(!job||!step||!this.retryable(job,step))throw deliveryError('此步骤不需要重试，已完成的内容会保留。');
      if(sourceIdentity(project)!==job.source_key)throw deliveryError('成果来源已变化，请重新整理当前成果，不能沿用失效证明。');
      if(job.steps.slice(0,job.steps.indexOf(step)).some(s=>!['completed','skipped'].includes(s.state)))throw deliveryError('请先处理前一个未完成步骤。');
      for(const later of job.steps.slice(job.steps.indexOf(step))){if(later.state!=='skipped'){later.state='pending';delete later.error;}}
      if(step.id==='en'){delete job.results.en;job.files=job.files.filter(f=>f.language!=='en');}
      job.state='queued';delete job.error;row.receipts[key]={action:'retry',signature,job_id:job.id};await this.save();this.launch(id,job.id);return this.view(project);
    });
  }
  async control(id,{type}={},key){
    if(!['pause','resume','stop'].includes(type)||!key)throw deliveryError('控制请求需要明确类型和回执标识。',422);
    return this.locked(id,async()=>{
      let project=await this.project(id);const row=this.row(id),signature=JSON.stringify({type});
      const old=row.receipts[key];if(old&&(old.action!=='control'||old.signature!==signature))throw deliveryError('相同回执对应不同控制请求。');if(old?.completed)return this.view(project);
      row.receipts[key]={action:'control',signature,completed:false};
      if(type==='resume'){
        if(this.active.has(id)||project.project_control?.outstanding_cancellation)throw deliveryError('部分调用尚未确认暂停，请等待确认后恢复。');
        await this.client.request(V2_PREFIX+'/projects/'+id+'/project-control',{method:'POST',idempotencyKey:key,body:{type}});
        row.control={state:'running',updated_at:now()};
        const job=row.jobs.findLast(j=>j.state==='paused');if(job){job.state='queued';for(const step of job.steps)if(step.state==='paused')step.state='pending';}
        row.receipts[key].completed=true;await this.save();if(job)this.launch(id,job.id);
      }else{
        row.control={state:type==='pause'?'pausing':'stopping',requested_at:now()};await this.save();this.active.get(id)?.controller.abort();
        for(const job of row.jobs)if(job.state==='queued')job.state=type==='pause'?'paused':'stopped';
        try{await this.client.request(V2_PREFIX+'/projects/'+id+'/project-control',{method:'POST',idempotencyKey:key,body:{type}});row.receipts[key].completed=true;}
        catch(error){row.control.error=error.message;await this.save();throw error;}
        row.control.state=type==='pause'?'paused':'stopped';row.control.updated_at=now();await this.save();
      }
      project=await this.project(id);return this.view(project);
    });
  }
  mergedControl(project){
    const row=this.state.projects[project.id],local=row?.control?.state||'running',research=project.project_control?.state||'running',active=this.active.has(project.id);
    let state=research;
    if(local==='pausing'||local==='paused')state=active||research!=='paused'?'pausing':'paused';
    else if(local==='stopping'||local==='stopped')state=active||!['stopped','paused'].includes(research)?'stopping':'stopped';
    else if(active)state=['pausing','paused','stopping'].includes(research)?'pausing':'running';
    return {state,research_state:research,delivery_state:active?'running':local,outstanding_cancellation:Boolean(project.project_control?.outstanding_cancellation||active&&['pausing','paused','stopping','stopped'].includes(local)),error:row?.control?.error||null};
  }
  publicFile(id,file){const url='/api/research-projects/'+encodeURIComponent(id)+'/files?path='+encodeURIComponent(file.path);return {...file,url,download_url:url+'&download=1'};}
  retryable(job,step){return ['failed','interrupted','paused'].includes(step.state)||(step.id==='en'&&step.state==='completed'&&job.steps.some(s=>['compile_en','sync'].includes(s.id)&&s.state==='failed'));}
  async records(project){
    const files=[];
    for(const folder of ['成果','研究记录']){
      let dir;try{dir=await projectFile(project,folder);}catch(error){if(error.code==='ENOENT')continue;throw error;}
      for(const item of await fs.readdir(dir,{withFileTypes:true})){
        if(!item.isFile()||item.name.startsWith('.')||!['.md','.pdf','.tex','.json','.bib','.zip'].includes(path.extname(item.name).toLowerCase()))continue;
        files.push(this.publicFile(project.id,{path:folder+'/'+item.name,name:item.name,label:item.name,kind:path.extname(item.name)==='.pdf'?'pdf':'report',language:null}));
      }
    }
    for(const job of this.state.projects[project.id]?.jobs||[])if(job.source?.base){
      for(const name of ['source-packet.json','来源状态.json']){const relative=job.source.base+'/'+name;try{await projectFile(project,relative);files.push(this.publicFile(project.id,{path:relative,name,label:name,kind:'report',language:null}));}catch(error){if(error.code!=='ENOENT')throw error;}}
      for(const source of job.source.selectedFiles||[]){const relative=unixPath(path.relative(await fs.realpath(project.workspace_path),await fs.realpath(source)));await projectFile(project,relative);files.push(this.publicFile(project.id,{path:relative,name:path.basename(source),label:'精确来源 · '+path.basename(source),kind:'report',language:null}));}
    }
    return files;
  }
  async view(projectOrId){
    const project=typeof projectOrId==='string'?await this.project(projectOrId):projectOrId,row=this.state.projects[project.id];
    const present=job=>({...clone(job),source_changed:sourceIdentity(project)!==job.source_key,prepared:undefined,results:undefined,source:undefined,
      steps:job.steps.map(s=>({...s,retryable:this.retryable(job,s)&&!this.active.has(project.id)&&!PAUSING.has(this.mergedControl(project).state)&&sourceIdentity(project)===job.source_key})),files:job.files.map(f=>this.publicFile(project.id,f))});
    return {project_id:project.id,workspace_path:project.workspace_path,control:this.mergedControl(project),current:row?.jobs.length?present(row.jobs.at(-1)):null,history:(row?.jobs||[]).slice(0,-1).reverse().map(present),records:await this.records(project)};
  }
  async registeredFile(id,relative){
    const project=await this.project(id),view=await this.view(project);
    const files=[...view.records,...[view.current,...view.history].filter(Boolean).flatMap(j=>j.files)];
    if(!files.some(f=>f.path===relative))throw deliveryError('此文件尚未登记为当前项目的可交付成果。',403);
    const file=await projectFile(project,relative);if(!(await fs.stat(file)).isFile())throw deliveryError('请求的成果不是文件。',422);return file;
  }
  async directory(id,folder){
    const project=await this.project(id);if(folder==='project')return fs.realpath(project.workspace_path);
    const names={results:'成果',papers:'论文',records:'研究记录'};if(!names[folder])throw deliveryError('目录类型无效。',422);
    const dir=await projectFile(project,names[folder],{mustExist:false});await fs.mkdir(dir,{recursive:true});return dir;
  }
}
