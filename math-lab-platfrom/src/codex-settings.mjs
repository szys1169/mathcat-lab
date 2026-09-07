import fs from 'node:fs/promises';
import path from 'node:path';
import {QueuedJsonWriter} from './fs-utils.mjs';
import {V2_PREFIX,unwrapProject} from './research-v2-client.mjs';

const error=(message,status=409)=>Object.assign(new Error(message),{status});
const projectId=value=>typeof value==='string'&&/^[A-Za-z0-9_-]{1,100}$/.test(value);
const fields=value=>({model:typeof value?.model==='string'?value.model:null,reasoning_effort:typeof value?.reasoning_effort==='string'?value.reasoning_effort:null});

// Match the kernel's effective selection, preserving an explicit model on a newer Run.
export function effectiveProjectModel(project){
  const selected=project.model_selection,run=(project.runs||[]).at(-1);
  if(selected?.model&&Number(selected.revision)>Number(run?.model_selection_revision??-1))return {...fields(selected),source:'project'};
  if(run?.model)return {...fields(run),source:'run'};
  if(selected?.model)return {...fields(selected),source:'project'};
  return null;
}

export function modelArguments(selection){
  const args=[];
  if(selection?.model){if(!/^[A-Za-z0-9][A-Za-z0-9._:/-]{0,127}$/.test(selection.model))throw error('模型名称无效。',422);args.push('--model',selection.model);}
  if(selection?.reasoning_effort){if(!['none','minimal','low','medium','high','xhigh','max','ultra'].includes(selection.reasoning_effort))throw error('推理强度无效。',422);args.push('-c','model_reasoning_effort="'+selection.reasoning_effort+'"');}
  return args;
}

export class CodexSettings{
  constructor({config,client,account}){this.config=config;this.client=client;this.account=account;this.state={schema:1,selection:null,revision:0,receipts:{}};this.file=path.join(config.runtimeRoot,'codex-model-selection.json');this.storage=new QueuedJsonWriter(this.file);this.pending=Promise.resolve();}
  async load(){try{this.state=JSON.parse(await fs.readFile(this.file,'utf8'));if(this.state.schema!==1)throw error('模型设置文件版本不支持。',500);}catch(e){if(e.code!=='ENOENT')throw e;}return this;}
  async project(id){if(!projectId(id))throw error('研究项目编号无效。',422);return unwrapProject(await this.client.request(V2_PREFIX+'/projects/'+id+'/snapshot'));}
  async defaultSelection(){if(this.state.selection?.model)return fields(this.state.selection);return fields((await this.account.read()).configured);}
  async forProject(project){return effectiveProjectModel(project)||await this.defaultSelection();}
  async runInput(id,input,key){
    if(!projectId(id)||typeof key!=='string'||!key)throw error('启动请求缺少项目或回执标识。',422);
    const operation=this.pending.catch(()=>{}).then(async()=>{
      const signature=JSON.stringify({id,input}),receiptKey='run:'+key;
      const existing=this.state.receipts[receiptKey];if(existing){if(existing.signature!==signature)throw error('同一启动回执不能用于不同的参数。');return structuredClone(existing.body);}
      const body=structuredClone(input);
      if(!body.model){const selected=await this.forProject(await this.project(id));if(selected.model)body.model=selected.model;if(body.reasoning_effort===undefined&&selected.reasoning_effort)body.reasoning_effort=selected.reasoning_effort;}
      this.state.receipts[receiptKey]={signature,body,completed:true};await this.storage.save(this.state);return body;
    });this.pending=operation;return operation;
  }
  async status({project_id,force=false}={}){
    const [account,project]=await Promise.all([this.account.read({force}),project_id?this.project(project_id):null]);
    const fallback=this.state.selection?.model?{...fields(this.state.selection),source:'platform'}:{...fields(account.configured),source:'codex_default'};
    const selection=project?{...(effectiveProjectModel(project)||fallback),scope:'project',project_id:project.id,revision:project.model_selection?.revision??0}:{...fallback,scope:'platform',revision:this.state.revision};
    const active=(project?.usage||[]).filter(call=>['reserved','running'].includes(call.state)).map(call=>{
      const session=project.sessions?.find(s=>s.id===call.session_id);return {session_id:call.session_id,role:session?.role,model:call.model??session?.model??null,reasoning_effort:call.reasoning_effort??session?.reasoning_effort??null};
    });
    return {...account,selection,active_calls:active};
  }
  async select(input,key){
    if(typeof key!=='string'||!key||key.length>200)throw error('模型切换请求缺少有效回执标识。',422);
    const operation=this.pending.catch(()=>{}).then(()=>this.selectOnce(input,key));this.pending=operation;return operation;
  }
  async selectOnce(input,key){
    const {project_id,model,reasoning_effort,expected_revision}=input;
    if(project_id!==undefined&&!projectId(project_id))throw error('研究项目编号无效。',422);
    const signature=JSON.stringify({project_id:project_id||null,model,reasoning_effort,expected_revision});
    const receipt=this.state.receipts[key];if(receipt&&receipt.signature!==signature)throw error('相同回执不能切换到不同的模型。');
    if(receipt?.completed)return this.status({project_id});
    modelArguments({model,reasoning_effort});
    const account=await this.account.read();
    const available=(account.models||[]).find(row=>row.model===model||row.id===model);
    if(!available)throw error('当前 Codex 可用列表没有此模型，请刷新模型列表后选择。',422);
    const supported=(available.supported_reasoning_efforts||[]).map(value=>typeof value==='string'?value:value.effort);
    if(!supported.includes(reasoning_effort))throw error('这个模型不支持所选推理强度，请重新选择。',422);
    if(expected_revision!==undefined&&(!Number.isInteger(expected_revision)||expected_revision<0))throw error('模型设置版本无效，请刷新后重试。',422);
    if(!project_id&&expected_revision!==undefined&&expected_revision!==this.state.revision)throw error('默认模型已在别处更新，请刷新后再选择。');
    this.state.receipts[key]={signature,completed:false};await this.storage.save(this.state);
    if(project_id)await this.client.request(V2_PREFIX+'/projects/'+project_id+'/model-selection',{method:'POST',idempotencyKey:key,body:{model,reasoning_effort,...(expected_revision!==undefined?{expected_revision}:{})}});
    else {this.state.selection={model,reasoning_effort,updated_at:new Date().toISOString()};this.state.revision++;}
    this.state.receipts[key].completed=true;await this.storage.save(this.state);return this.status({project_id});
  }
}

export async function codexSettingsRoute({req,res,url,pathname,settings,sendJson,readBody}){
  if(pathname==='/api/codex/status'&&req.method==='GET'){sendJson(res,200,await settings.status({project_id:url.searchParams.get('project_id')||undefined,force:url.searchParams.get('refresh')==='1'}));return true;}
  if(pathname==='/api/codex/model-selection'&&req.method==='POST'){sendJson(res,200,await settings.select(await readBody(req),req.headers['idempotency-key']));return true;}
  return false;
}
