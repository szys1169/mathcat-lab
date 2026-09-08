import crypto from "node:crypto";
import {V2_PREFIX,unwrapProject} from "./research-v2-client.mjs";
import {readMaterialText} from "./research-materials.mjs";
import {explicitProblemEdit} from "../public/problem-edit-intent.js";

export class ResearchV2Bridge {
  constructor({store,client,onRunStarted,resolveModel}={}){this.store=store;this.client=client;this.onRunStarted=onRunStarted;this.resolveModel=resolveModel;this.pending=new Map();}
  async start(conversationId,text,{durationSeconds=7200,reviewMode="strict",maxPartners=5,autoDeliver=true}={}) {
    if(!Number.isInteger(durationSeconds)||durationSeconds<=0)throw Object.assign(new Error("研究时长必须是正整数秒。"),{status:422});
    if(!["automatic","strict","balanced"].includes(reviewMode))throw Object.assign(new Error("请选择自动安排分工或每轮由我批准。"),{status:422});
    if(!Number.isInteger(maxPartners)||maxPartners<0||maxPartners>20)throw Object.assign(new Error("伙伴猫上限须为 0 到 20 的整数。"),{status:422});
    if(typeof autoDeliver!=='boolean')throw Object.assign(new Error("请明确是否整理双语成果。"),{status:422});
    if(this.pending.has(conversationId))throw Object.assign(new Error("该对话正在提交研究，请等待回执。"),{status:409});
    const job=this.startOnce(conversationId,text,{durationSeconds,reviewMode,maxPartners,autoDeliver});this.pending.set(conversationId,job);
    try{return await job;}finally{this.pending.delete(conversationId);}
  }
  async importMaterials(conversation,projectId){
    for(const material of conversation.researchMaterials||[]){
      if(material.importedProjectId===projectId)continue;
      const result=await this.client.request(V2_PREFIX+"/projects/"+encodeURIComponent(projectId)+"/material-imports",{method:"POST",idempotencyKey:material.id,body:{filename:material.filename+".extracted.txt",content:await readMaterialText(material),media_type:"text/plain",material_role:material.materialRole||'unclassified_input',provenance:{original_filename:material.filename,extraction_method:material.extraction_method,source_sha256:material.sha256}}});
      await this.store.updateConversation(conversation.id,row=>{const m=row.researchMaterials.find(item=>item.id===material.id);m.importedProjectId=projectId;m.artifactId=result.artifact?.id||result.artifact?.artifact_id||null;});
    }
  }
  async startOnce(conversationId,text,{durationSeconds,reviewMode,maxPartners,autoDeliver}) {
    const conversation=this.store.getConversation(conversationId);
    if(!conversation)throw Object.assign(new Error("Conversation not found."),{status:404});
    if(!conversation.researchStartKey){const selected=await this.resolveModel?.()||{};await this.store.updateConversation(conversationId,row=>{row.researchStartKey=crypto.randomUUID();row.researchStartOptions={durationSeconds,reviewMode,maxPartners,autoDeliver,...selected};});}
    let project;
    if(conversation.researchProjectId){
      project=unwrapProject(await this.client.request(V2_PREFIX+"/projects/"+encodeURIComponent(conversation.researchProjectId)+"/snapshot"));
      const active=(project.runs||[]).find(run=>run.state!=="ended");
      if(active){await this.store.updateConversation(conversationId,row=>{row.researchRunId=active.id;row.status=active.state||"running";});return {ok:true,contract:"mathcat-research/v2",projectId:project.id,run:active,reused:true};}
      if(conversation.researchRunId)throw Object.assign(new Error("本次研究已有回执，请在白板显式开始新的 Run；不会重复启动旧任务。"),{status:409});
    }else{
      const created=await this.client.request(V2_PREFIX+"/projects",{method:"POST",idempotencyKey:conversation.researchStartKey,body:{title:conversation.title,problem:text,...(conversation.workspaceId?{workspace_id:conversation.workspaceId}:{})}});
      project=unwrapProject(created);
      await this.store.updateConversation(conversationId,row=>{row.researchProjectId=project.id;row.researchContract="mathcat-research/v2";row.researchOutputPath=project.workspace_path||null;row.status="idle";row.messages.push({id:crypto.randomUUID(),role:"user",content:text,createdAt:new Date().toISOString(),executor:"codex",capabilityId:"rethlas-research",researchAgent:"MathCat 2.5.1"});});
    }
    await this.importMaterials(conversation,project.id);
    const options=conversation.researchStartOptions||{durationSeconds,reviewMode,maxPartners,autoDeliver};
    const result=await this.client.request(V2_PREFIX+"/projects/"+encodeURIComponent(project.id)+"/runs",{method:"POST",idempotencyKey:conversation.researchStartKey+"-run",body:{start_authorized:true,duration_seconds:options.durationSeconds,mode:options.reviewMode==="automatic"?"delegated":"collaborative",limits:{enforcement:"best_effort",max_partners:options.maxPartners??5},wait_policy:"critical_only",...(options.model?{model:options.model}:{}),...(options.reasoning_effort?{reasoning_effort:options.reasoning_effort}:{})}});
    await this.store.updateConversation(conversationId,row=>{row.researchRunId=result.run?.id||result.id||null;row.status="running";});
    await this.onRunStarted?.(project.id,result.run||result,options);
    return {ok:true,contract:"mathcat-research/v2",projectId:project.id,run:result.run||result};
  }
  async bind(conversationId,projectId) {
    const project=unwrapProject(await this.client.request(V2_PREFIX+"/projects/"+encodeURIComponent(projectId)+"/snapshot"));
    await this.store.updateConversation(conversationId,row=>{if(row.researchProjectId&&row.researchProjectId!==project.id)throw Object.assign(new Error("对话已关联其他研究项目。"),{status:409});row.researchProjectId=project.id;row.researchContract="mathcat-research/v2";row.researchOutputPath=project.workspace_path||null;});
    return this.store.getConversation(conversationId);
  }
  async editProblem(conversationId,{text,expected_revision},idempotencyKey){
    if(!idempotencyKey||typeof idempotencyKey!=="string")throw Object.assign(new Error("修改请求缺少回执标识。"),{status:422});
    if(!Number.isInteger(expected_revision)||expected_revision<1)throw Object.assign(new Error("请刷新数学问题版本后再修改。"),{status:422});
    const patch=explicitProblemEdit(text);
    if(!patch)throw Object.assign(new Error("请使用明确的完整替换指令，例如：将数学问题改为：……。也可以在白板直接编辑。"),{status:422});
    const conversation=this.store.getConversation(conversationId);
    if(!conversation?.researchProjectId)throw Object.assign(new Error("此对话尚未关联研究项目。"),{status:409});
    const requestBody={expected_revision,...patch,source:"conversation_edit"};
    const signature=JSON.stringify({text,requestBody});
    const prior=conversation.researchProblemEdits?.find(row=>row.key===idempotencyKey);
    if(prior&&prior.signature!==signature)throw Object.assign(new Error("同一回执不能用于不同的修改。"),{status:409});
    if(prior?.result)return prior.result;
    const result=await this.client.request(V2_PREFIX+"/projects/"+encodeURIComponent(conversation.researchProjectId)+"/problem-spec",{method:"PATCH",idempotencyKey,body:requestBody});
    await this.store.updateConversation(conversationId,row=>{
      row.researchProblemEdits??=[];
      if(row.researchProblemEdits.some(item=>item.key===idempotencyKey))return;
      row.researchProblemEdits.push({key:idempotencyKey,signature,result});
      const createdAt=new Date().toISOString();
      row.messages.push({id:crypto.randomUUID(),role:"user",content:text,createdAt});
      row.messages.push({id:crypto.randomUUID(),role:"assistant",content:(patch.math_statement?"数学问题":"研究说明")+"已按明确指令更新。原始输入和既有成果版本保留，可在白板查看。",createdAt,recordOnly:true});
    });
    return result;
  }
}
