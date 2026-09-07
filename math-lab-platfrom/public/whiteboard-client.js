import {commandFor, expectedVersions, latestRun} from './research-v2-state.js';

const arrays={cycles:'cycles',routes:'routes',messages:'messages',advisories:'advisories',
  'pending-assignments':'pending_assignments','proof-checkpoints':'proof_checkpoints',
  'display-summaries':'display_summaries','memory-entries':'memory_entries',jobs:'background_jobs',
  'human-questions':'human_questions',feedback:'feedback_traces'};

export class WhiteboardClient {
  constructor(classic,{storage=globalThis.sessionStorage}={}) {
    this.classic=classic;this.storage=storage;this.pending=new Map();this.inflight=new Map();this.transfers=new Map();
    try{this.pending=new Map(JSON.parse(storage?.getItem('mathcat.2.3.whiteboard.pending')||'[]'));}catch{}
    try{this.transfers=new Map(JSON.parse(storage?.getItem('mathcat.2.3.whiteboard.annotation-transfers')||'[]'));}catch{}
  }
  save(){try{this.storage?.setItem('mathcat.2.3.whiteboard.pending',JSON.stringify([...this.pending]));}catch{}}
  path(project,suffix){return this.classic.path(project,suffix);}
  async read(project,path){return this.classic.request(this.path(project,path));}
  async supplement(project) {
    const result={...project,board_errors:{},board_available:{},board_sources:{}};
    const endpoints=[...Object.entries(arrays).filter(([,field])=>{
      if(Array.isArray(project[field])){result.board_available[field]=true;result.board_sources[field]=project.revision;return false;}
      return true;
    }),...(project.proof_tree?(result.board_available.proof_tree=true,result.board_sources.proof_tree=project.revision,[]):[['proof-tree','proof_tree']])];
    const values=await Promise.allSettled(endpoints.map(([path])=>this.read(project,'/'+path)));
    endpoints.forEach(([path,field],i)=>{
      const value=values[i];if(value.status==='rejected'){result.board_errors[field]=value.reason.message;result.board_available[field]=false;return;}
      const data=value.value[field]??value.value.items;
      if(data===undefined){result.board_errors[field]='服务未提供 '+field+' 字段';result.board_available[field]=false;return;}
      const revision=value.value.revision??value.value.source_revision??data?.revision;
      result.board_sources[field]=revision??null;
      if(revision!==undefined&&revision!==project.revision){result.board_errors[field]='来源修订 '+revision+' 与当前快照 '+project.revision+' 不同，等待一致快照';result.board_available[field]=false;return;}
      if(revision===undefined){result.board_errors[field]='独立读取未提供来源修订，不能确认与当前快照同时';result.board_available[field]=false;return;}
      result[field]=data;result.board_available[field]=true;
    });
    try{result.delivery=await this.classic.request('/api/research-projects/'+encodeURIComponent(project.id)+'/delivery');result.project_control=result.delivery.control;}
    catch(error){result.board_errors.delivery=error.message;}
    result.board_read_at=new Date().toISOString();return result;
  }
  async write(project,path,body,{method='POST'}={}) {
    const url=path.startsWith('/api/research-projects/')?path:this.path(project,path),signature=method+' '+url+' '+JSON.stringify(body);
    const unfinished=[...this.pending.values()].find(p=>p.url===url&&p.signature!==signature);
    if(unfinished)throw new Error('该入口有结果待确认的请求；请先核对或重试原请求，避免重复操作。');
    if(!this.pending.has(signature)){this.pending.set(signature,{signature,url,method,body,key:crypto.randomUUID(),project_id:project.id,created_at:new Date().toISOString()});this.save();}
    return this.retry(signature);
  }
  async retry(signature) {
    if(this.inflight.has(signature))return this.inflight.get(signature);
    const pending=this.pending.get(signature);if(!pending)throw new Error('原请求已处理，请刷新记录。');
    const task=(async()=>{
      try{
        const value=await this.classic.request(pending.url,{method:pending.method,body:pending.body,idempotencyKey:pending.key});
        this.pending.delete(signature);this.save();return value;
      }catch(error){
        // A validated client rejection is conclusive; lost responses and 5xx are not.
        if(error.status>=400&&error.status<500&&error.status!==408){this.pending.delete(signature);this.save();}
        throw error;
      }finally{this.inflight.delete(signature);}
    })();this.inflight.set(signature,task);return task;
  }
  feedback(project,{body,priority='normal',selection,annotation,kind,target}={}) {
    return this.write(project,'/feedback',{run_id:latestRun(project)?.id,body,summary:body,priority,
      expected_versions:expectedVersions(project,latestRun(project)),evidence_refs:selection?[selection]:[],
      ...(selection?{selection_ref:selection}:{}),...(kind?{kind}:{}),...(target?{target}:{}),...(annotation?{annotation_id:annotation.id,annotation_revision:annotation.revision}: {})});
  }
  feedbackAction(project,trace,action,reason='') {
    return this.write(project,'/feedback/'+encodeURIComponent(trace.command_id)+'/'+action,{expected_revision:trace.revision,reason});
  }
  answer(project,question,body) {
    return this.write(project,'/human-questions/'+encodeURIComponent(question.id)+'/answer',{
      run_id:question.run_id,expected_revision:question.revision,body,expected_versions:expectedVersions(project,latestRun(project))});
  }
  command(project,type,{target,payload,text='',applyAt}={}) {
    const body=commandFor(project,type,{text,payload,applyAt});
    if(target)body.target=target;
    return this.write(project,'/commands',body);
  }
  annotate(project,selection,body) {
    return this.write(project,'/annotations',{node_id:selection.kind==='node'?selection.id:undefined,
      anchor:{node_revision:selection.revision},selection_ref:selection,kind:'comment',body});
  }
  async annotateAndFeedback(project,selection,body) {
    const key=JSON.stringify([project.id,selection,body]);
    let annotation=this.transfers.get(key);
    if(!annotation){const value=await this.annotate(project,selection,body);annotation=value.annotation||value;
      this.transfers.set(key,annotation);this.saveTransfers();}
    const result=await this.feedback(project,{body,selection,annotation});
    this.transfers.delete(key);this.saveTransfers();return result;
  }
  saveTransfers(){try{this.storage?.setItem('mathcat.2.3.whiteboard.annotation-transfers',JSON.stringify([...this.transfers]));}catch{}}
  async detail(project,object,reference) {
    const ref=reference||object.ref||{kind:object.kind,id:object.id,revision:object.revision};
    const path=ref.kind==='node'?'/nodes/'+encodeURIComponent(ref.id)+'?revision='+encodeURIComponent(ref.revision):
      '/statements/'+encodeURIComponent(ref.id);
    const value=await this.read(project,path);
    return value.node||value.statement||value;
  }
  async artifact(project,id) {
    const response=await this.classic.fetch(this.path(project,'/artifacts/'+encodeURIComponent(id)+'/content'));
    if(!response.ok)throw new Error('原文读取失败：HTTP '+response.status);
    const type=response.headers.get('content-type')||'';
    if(type&&!/text|json|markdown/.test(type))return {text:null,media_type:type,url:this.path(project,'/artifacts/'+encodeURIComponent(id)+'/content')};
    return {text:await response.text(),media_type:type,url:this.path(project,'/artifacts/'+encodeURIComponent(id)+'/content')};
  }
  async revisePreview(project,text) {
    return this.write(project,'/problem-revision-previews',{new_problem:text,base_problem_version:project.problem_version,change_reason:'用户在证明树白板提出修订'});
  }
  reviseCommit(project,preview) {
    return this.command(project,'replace_problem',{payload:{...preview.proposal,confirmed_impact_preview_id:preview.id},applyAt:'immediate'});
  }
  limits(project,limits){const run=latestRun(project);if(!run)throw new Error('尚无运行');return this.write(project,'/runs/'+encodeURIComponent(run.id)+'/limits',{limits,expected_revision:run.revision},{method:'PATCH'});}
  problemSpec(project,fields,source='direct_edit'){
    if(!project.problem_spec)throw new Error('题面版本尚未读取，请刷新后重试。');
    return this.write(project,'/problem-spec',{expected_revision:project.problem_spec.revision,...fields,source},{method:'PATCH'});
  }
  mode(project,mode){const run=latestRun(project);if(!run)throw new Error('请先建立研究轮次。');return this.write(project,'/runs/'+encodeURIComponent(run.id)+'/mode',{expected_revision:run.revision,mode});}
  projectPath(project,suffix){return '/api/research-projects/'+encodeURIComponent(project.id)+suffix;}
  control(project,type){return this.write(project,this.projectPath(project,'/control'),{type});}
  deliveryStart(project){return this.write(project,this.projectPath(project,'/delivery/start'),{automatic:false});}
  deliveryRetry(project,step){return this.write(project,this.projectPath(project,'/delivery/retry'),{step});}
  reveal(project,folder){return this.write(project,this.projectPath(project,'/reveal'),{folder});}
  planningDecision(project,plan,decision,reason=''){
    return this.write(project,'/planning-proposals/'+encodeURIComponent(plan.id)+'/decision',{expected_revision:plan.revision,decision,reason});
  }
}
