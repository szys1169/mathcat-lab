import http from "node:http";
import fs from "node:fs/promises";
import path from "node:path";
import { config } from "./config.mjs";
import { Store } from "./store.mjs";
import { loadCapabilities } from "./capabilities.mjs";
import { TaskManager } from "./task-manager.mjs";
import { ensureInside } from "./fs-utils.mjs";
import { ResearchAgents } from "./research-agents.mjs";
import { ResearchBoards } from "./research-boards.mjs";
import { MathCatClient } from "./mathcat-client.mjs";
import { resolveResearchProblem } from "./research-context.mjs";
import { ResearchV2Client, V2_PREFIX, assertLocalRequest } from "./research-v2-client.mjs";
import { ResearchV2Bridge } from "./research-v2-bridge.mjs";
import { storeResearchMaterial, publicMaterial } from "./research-materials.mjs";
import { ResearchDeliveries } from "./research-delivery.mjs";
import { researchProductRoute } from "./research-product-routes.mjs";
import { createCodexAccount } from './codex-account.mjs';
import { CodexSettings,codexSettingsRoute } from './codex-settings.mjs';
import { workspaceFileRoute } from './workspace-files.mjs';
import { pickFolder,revealInFileManager } from './desktop-integration.mjs';
import {spawn} from 'node:child_process';
import {ProductUpdater,updateFetch} from './product-update.mjs';
import {databasePath} from '../../scripts/version-runtime.mjs';

const store = await new Store(config.runtimeRoot).load();
const researchV2 = new ResearchV2Client(config.researchV2);
const codexAccount=createCodexAccount({config});
const modelSettings=await new CodexSettings({config,client:researchV2,account:codexAccount}).load();
const executionConfig={...config,resolveCodexModel:()=>modelSettings.defaultSelection()};
const deliveries = await new ResearchDeliveries({config:executionConfig,client:researchV2,store,modelSettings}).load();
const v2Bridge = new ResearchV2Bridge({store,client:researchV2,resolveModel:()=>modelSettings.defaultSelection(),onRunStarted:(id,run,options)=>deliveries.registerRun(id,run,options)});
deliveries.startMonitor();
process.once('SIGTERM',()=>{deliveries.close();server.close(()=>process.exit(0));});
process.once('SIGINT',()=>{deliveries.close();server.close(()=>process.exit(0));});
const capabilities = await loadCapabilities(config.capabilitiesRoot);
const researchAgents = await new ResearchAgents(config).load();
const mathcat = new MathCatClient(config.mathcat);
const researchBoards = await new ResearchBoards(config.runtimeRoot, mathcat).load();
const tasks = new TaskManager({ store, config:executionConfig, capabilities, researchBoards });
await tasks.recoverInterrupted();
async function resumeBoardWatcher(boardId, board) { const conversationId=researchBoards.get(boardId)?.conversationId;if(conversationId)await tasks.resumeMathCatWatcher(conversationId,board).catch(error=>console.error(`Failed to resume MathCat watcher for ${conversationId}:`,error)); }
const versionRoot=path.resolve(config.appRoot,'..');
let inflightWrites=0;
const updater=new ProductUpdater({root:versionRoot,localVersion:config.version,fetchImpl:updateFetch(versionRoot),
  dataPaths:{databasePath,workspacesRoot:config.workspacesRoot,platformDataRoot:config.runtimeRoot,tokenFile:config.researchV2.tokenFile},
  assertIdle:async()=>{
    const pid=Number((await fs.readFile(path.join(versionRoot,'runtime','platform.pid'),'utf8').catch(()=>'' )).trim());
    if(config.port!==4335||pid!==process.pid)throw Object.assign(new Error('请通过本版本的启动器打开应用后再安装更新。版本检查仍可使用。'),{status:409});
    const health=await researchV2.request(`${V2_PREFIX}/health`);
    if(health.version!==config.version)throw Object.assign(new Error('研究服务版本不匹配，请通过本版本启动器重新打开。'),{status:409});
    if(inflightWrites||deliveries.observing||deliveries.active.size||deliveries.locks.size||Object.values(deliveries.state.projects).some(row=>row.jobs.some(job=>['queued','running'].includes(job.state)))||store.listConversations().some(c=>tasks.getActivity(c.id).running))throw Object.assign(new Error('仍有任务或请求正在执行，请结束任务后再更新。'),{status:409});
    let cursor=null;do{const page=await researchV2.request(`${V2_PREFIX}/projects?limit=100${cursor?'&cursor='+encodeURIComponent(cursor):''}`);if(!Array.isArray(page.projects))throw new Error('无法确认研究状态，请稍后重试。');if(page.projects.some(researchHasUnfinishedExecution))throw Object.assign(new Error('仍有研究任务尚未结束，请在白板结束任务后再更新。'),{status:409});cursor=page.next_cursor;}while(cursor);
  },
  activate:async({destination,statusFile})=>{
    const log=await fs.open(path.join(versionRoot,'runtime','updates','restart.log'),'a');
    try{const child=spawn(process.execPath,[path.join(versionRoot,'scripts','apply-update.mjs'),versionRoot,destination,statusFile],{cwd:versionRoot,detached:true,windowsHide:true,shell:false,stdio:['ignore',log.fd,log.fd],env:{...process.env,CODEX_BIN:config.codexBin}});await new Promise((resolve,reject)=>{child.once('spawn',resolve);child.once('error',reject);});child.unref();}finally{await log.close();}
  }
});
const originalObserve=deliveries.observe.bind(deliveries);deliveries.observe=async()=>{if(!updater.busy)return originalObserve();};

function sendJson(res,status,value){const data=JSON.stringify(value);res.writeHead(status,{"content-type":"application/json; charset=utf-8","content-length":Buffer.byteLength(data),"cache-control":"no-store"});res.end(data)}
function fail(res,status,message){sendJson(res,status,{ok:false,error:message})}
function errorStatus(error){const explicit=Number(error?.status);if(Number.isInteger(explicit)&&explicit>=400&&explicit<600)return explicit;if(error?.code==="ENOENT")return 404;if(["TimeoutError","AbortError"].includes(error?.name))return 504;const networkCode=String(error?.cause?.code||error?.code||"");return /ECONNREFUSED|ECONNRESET|ENOTFOUND|UND_ERR|fetch failed/i.test(`${networkCode} ${error?.message||""}`)?502:500}
async function readBody(req,maxBytes=2_000_000){if(req.cachedJsonBody)return req.cachedJsonBody;let size=0;const chunks=[];for await(const chunk of req){size+=chunk.length;if(size>maxBytes)throw Object.assign(new Error("Request is too large."),{status:413});chunks.push(chunk)}return chunks.length?JSON.parse(Buffer.concat(chunks).toString("utf8")):{} }
function mime(file){return({".html":"text/html; charset=utf-8",".css":"text/css; charset=utf-8",".js":"text/javascript; charset=utf-8",".json":"application/json; charset=utf-8",".md":"text/markdown; charset=utf-8",".tex":"text/plain; charset=utf-8",".bib":"text/plain; charset=utf-8",".sty":"text/plain; charset=utf-8",".log":"text/plain; charset=utf-8",".txt":"text/plain; charset=utf-8",".svg":"image/svg+xml",".woff2":"font/woff2",".woff":"font/woff",".ttf":"font/ttf",".pdf":"application/pdf",".png":"image/png",".jpg":"image/jpeg",".jpeg":"image/jpeg",".pptx":"application/vnd.openxmlformats-officedocument.presentationml.presentation",".docx":"application/vnd.openxmlformats-officedocument.wordprocessingml.document",".zip":"application/zip"})[path.extname(file).toLowerCase()]||"application/octet-stream"}
const ACTIVE_CONTENT_EXTENSIONS=new Set([".html",".htm",".xhtml",".svg",".xml",".js",".mjs"]);
function userFileHeaders(file){const active=ACTIVE_CONTENT_EXTENSIONS.has(path.extname(file).toLowerCase());return{"content-type":active?"application/octet-stream":mime(file),"content-disposition":`${active?"attachment":"inline"}; filename*=UTF-8''${encodeURIComponent(path.basename(file))}`,"x-content-type-options":"nosniff","content-security-policy":"sandbox; default-src 'none'; frame-ancestors 'none'"}}
async function serveStatic(res,pathname){const file=ensureInside(config.publicRoot,path.join(config.publicRoot,pathname==="/"?"index.html":pathname.slice(1)));const data=await fs.readFile(file);res.writeHead(200,{"content-type":mime(file),"content-length":data.length});res.end(data)}

// Keep the project entry reachable until every locally owned execution has settled.
// An ended local receipt can still have unknown remote billing; never rewrite that receipt.
function researchHasUnfinishedExecution(project) {
  if (!project || typeof project !== "object") return true;
  const names = ["runs", "interactions", "sessions", "usage"];
  if (names.some(name => project[name] != null && !Array.isArray(project[name]))) return true;
  const [runs, interactions, sessions, usage] = names.map(name => project[name] || []);
  if ([...runs, ...interactions, ...sessions, ...usage].some(row => !row || typeof row !== "object")) return true;
  const endedAt = row => typeof row.ended_at === "string" && Number.isFinite(Date.parse(row.ended_at));
  const terminalUsage = new Set(["succeeded", "failed", "cancelled", "unknown"]);
  if (runs.some(row => row.state !== "ended") || interactions.some(row => row.state !== "ended")) return true;
  if (sessions.some(row => row.state !== "closed")) return true;
  if (usage.some(row => ["reserved", "running"].includes(row.state) || (row.state === "unknown" && !endedAt(row)))) return true;
  // Same conservative local-closure evidence as the version stop script. A lingering
  // outstanding flag is not cleared here, and missing ownership evidence fails closed.
  const locallyClosed = sessions.length > 0 && usage.length > 0
    && usage.every(row => endedAt(row) && terminalUsage.has(row.state) && row.session_id && sessions.some(session => session.id === row.session_id))
    && runs.filter(row => row.outstanding_cancellation === true).every(row => row.id && sessions.some(session => session.run_id === row.id) && usage.some(call => call.run_id === row.id))
    && interactions.filter(row => row.outstanding_cancellation === true).every(row => row.id && sessions.some(session => session.execution_owner?.kind === "interaction" && session.execution_owner.id === row.id) && usage.some(call => call.execution_owner?.kind === "interaction" && call.execution_owner.id === row.id));
  return [...runs, ...interactions, ...sessions, ...usage].some(row => row.outstanding_cancellation === true) && !locallyClosed;
}

const server=http.createServer(async(req,res)=>{try{assertLocalRequest(req,config.port);const url=new URL(req.url,`http://${req.headers.host||"localhost"}`);const pathname=decodeURIComponent(url.pathname);
if(req.method==='GET'&&pathname==='/api/product-update/check')return sendJson(res,200,await updater.check());
if(req.method==='GET'&&pathname==='/api/product-update/status')return sendJson(res,200,await updater.status());
if(req.method==='POST'&&pathname==='/api/product-update/install'){
  if(req.headers['x-mathcat-update']!=='1'||!req.headers['content-type']?.startsWith('application/json'))return fail(res,403,'请从软件更新页面操作。');
  const input=await readBody(req,1000);return sendJson(res,202,await updater.start(input.version));
}
if(!['GET','HEAD','OPTIONS'].includes(req.method)){
  if(updater.busy)return fail(res,409,'正在更新软件，请等待更新完成。');
  inflightWrites++;let counted=true;const settle=()=>{if(counted){counted=false;inflightWrites--;}};res.once('finish',settle);res.once('close',settle);
}
if(await codexSettingsRoute({req,res,url,pathname,settings:modelSettings,sendJson,readBody}))return;
if(await researchProductRoute({req,res,url,pathname,deliveries,sendJson,readBody,userFileHeaders,reveal:revealInFileManager}))return;
const productRunStart=pathname.match(/^\/api\/v2\/research\/projects\/([^/]+)\/runs$/);
if(req.method==='POST'&&productRunStart){
  const input=await readBody(req);if(input.auto_deliver!==undefined&&typeof input.auto_deliver!=='boolean')return fail(res,422,'请明确是否整理双语成果。');
  const {auto_deliver,...body}=await modelSettings.runInput(productRunStart[1],input,req.headers['idempotency-key']);
  const result=await researchV2.request(pathname,{method:'POST',body,idempotencyKey:req.headers['idempotency-key']});
  const options={autoDeliver:auto_deliver!==false,durationSeconds:body.duration_seconds,reviewMode:body.mode==='collaborative'?'strict':'automatic',maxPartners:body.limits?.max_partners??5,model:body.model,reasoning_effort:body.reasoning_effort};
  await deliveries.registerRun(productRunStart[1],result.run||result,options);
  for(const conversation of store.listConversations().filter(c=>c.researchProjectId===productRunStart[1]))await store.updateConversation(conversation.id,row=>{row.researchRunId=(result.run||result).id;row.researchStartOptions=options;row.status=(result.run||result).state||'running';});
  return sendJson(res,202,result);
}
if(pathname.startsWith(`${V2_PREFIX}/`))return await researchV2.proxy(req,res);
if(req.method==="GET"&&pathname==="/api/health"){const mathcatHealth=await researchV2.request(`${V2_PREFIX}/health`).catch(error=>({status:"unavailable",message:error.message}));return sendJson(res,200,{ok:true,service:"math-lab-platform",version:config.version,capabilities:capabilities.length,codexConfigured:Boolean(config.codexBin),mathcat:mathcatHealth,defaultWorkspaceRoot:config.workspacesRoot})}
if(req.method==="POST"&&pathname==="/api/workspaces/create"){const input=await readBody(req);const name=String(input.name||"新工作区").trim();if(!name||/[\\/:*?"<>|]/.test(name)||name==="."||name==="..")return fail(res,400,"工作区名称不能包含路径分隔符或特殊字符。");await fs.mkdir(config.workspacesRoot,{recursive:true});const directory=ensureInside(config.workspacesRoot,path.join(config.workspacesRoot,`${name}-${crypto.randomUUID().slice(0,8)}`));await fs.mkdir(directory);return sendJson(res,201,await store.addWorkspace({name,workspacePath:directory}));}
const v2bm=pathname.match(/^\/api\/conversations\/([^/]+)\/research-project$/);if(req.method==="POST"&&v2bm){if(tasks.getActivity(v2bm[1]).running)return fail(res,409,"此对话的旧任务仍在执行，请先结束它再关联研究项目。");const input=await readBody(req);return sendJson(res,200,await v2Bridge.bind(v2bm[1],String(input.projectId||"")));}
const v2edit=pathname.match(/^\/api\/conversations\/([^/]+)\/problem-edit$/);if(req.method==="POST"&&v2edit)return sendJson(res,200,await v2Bridge.editProblem(v2edit[1],await readBody(req),req.headers["idempotency-key"]));
const v2materials=pathname.match(/^\/api\/conversations\/([^/]+)\/materials$/);if(req.method==="POST"&&v2materials){const conversation=store.getConversation(v2materials[1]);if(!conversation)return fail(res,404,"Conversation not found.");const input=await readBody(req,8_500_000);const material=await storeResearchMaterial({runtimeRoot:config.runtimeRoot,conversationId:conversation.id,name:input.name,contentBase64:input.contentBase64,materialRole:input.materialRole});await store.updateConversation(conversation.id,row=>{row.researchMaterials??=[];row.researchMaterials.push(material);});if(conversation.researchProjectId)await v2Bridge.importMaterials(conversation,conversation.researchProjectId);return sendJson(res,201,{ok:true,material:publicMaterial(material),imported:Boolean(material.importedProjectId)});}
const v2retry=pathname.match(/^\/api\/conversations\/([^/]+)\/research-start$/);if(req.method==="POST"&&v2retry){if(tasks.getActivity(v2retry[1]).running)return fail(res,409,"此对话的旧任务仍在执行，请先结束它再启动研究。");const conversation=store.getConversation(v2retry[1]);if(!conversation)return fail(res,404,"Conversation not found.");const input=await readBody(req);if(input.start_authorized!==true)return fail(res,422,"开始研究需要明确授权。");return sendJson(res,202,await v2Bridge.start(conversation.id,conversation.messages.find(m=>m.role==="user")?.content||conversation.title,conversation.researchStartOptions));}
const v2control=pathname.match(/^\/api\/conversations\/([^/]+)(?:\/(activity|cancel))?$/);if(v2control){const conversation=store.getConversation(v2control[1]);if(conversation?.researchProjectId&&(req.method==="DELETE"||(req.method==="GET"&&v2control[2]==="activity")||(req.method==="POST"&&v2control[2]==="cancel"))){const value=await researchV2.request(V2_PREFIX+"/projects/"+encodeURIComponent(conversation.researchProjectId)+"/snapshot");const project=value.project||value;const run=(project.runs||[]).findLast(r=>r.state!=="ended")||(project.runs||[]).at(-1);if(req.method==="DELETE"&&(researchHasUnfinishedExecution(project)||deliveries.active.has(project.id)))return fail(res,409,"此对话仍有研究、独立解释或未确认收束的执行。请先在白板停止并确认，再删除对话；项目与成果仍保留。");if(v2control[2]==="activity")return sendJson(res,200,{running:Boolean(run&&run.state!=="ended"),state:run?.state||"created",current:"MathCat 2.5.3 · "+(run?.state||"尚未开始"),projectId:project.id,runId:run?.id||null,lines:[],details:[],elapsedSeconds:run?.started_at?Math.max(0,Math.floor((Date.now()-Date.parse(run.started_at))/1000)):0});if(v2control[2]==="cancel")return sendJson(res,202,await deliveries.control(project.id,{type:"stop"},req.headers["idempotency-key"]||crypto.randomUUID()));}}
const v2workspaceDelete=pathname.match(/^\/api\/workspaces\/([^/]+)$/);if(req.method==="DELETE"&&v2workspaceDelete){const projects=new Set(store.listConversations().filter(c=>c.workspaceId===v2workspaceDelete[1]&&c.researchProjectId).map(c=>c.researchProjectId));for(const projectId of projects){const value=await researchV2.request(V2_PREFIX+"/projects/"+encodeURIComponent(projectId)+"/snapshot");if(researchHasUnfinishedExecution(value.project||value)||deliveries.active.has(projectId))return fail(res,409,"工作区仍有研究、独立解释或未确认收束的执行。请先在白板停止并确认；不会通过删除记录隐式停止或丢弃项目。");}}
const v2mm=pathname.match(/^\/api\/conversations\/([^/]+)\/messages$/);if(req.method==="POST"&&v2mm){const input=await readBody(req);if(input.executor&&input.executor!=="codex")return fail(res,400,"MathCat 2.5.3 研究链当前仅接通 Codex CLI；不会静默更换执行器。");if(input.capabilityId==="autonomous-math-research")return fail(res,410,"旧全自动入口已移除，请从 MathCat 启动入口选择本次分工方式。");const conversation=store.getConversation(v2mm[1]);if(!conversation)return fail(res,404,"Conversation not found.");const text=String(input.text||"").trim();if(!text)return fail(res,400,"Message is required.");if(conversation.researchProjectId)return fail(res,409,"此对话属于 MathCat 2.5.3 项目，请使用白板的讨论、建议或指令入口。");if(input.capabilityId==="rethlas-research"&&(!input.researchAgent||input.researchAgent==="mathcat")&&tasks.getActivity(conversation.id).running)return fail(res,409,"此对话的旧任务仍在执行，请先结束它再启动 MathCat。");if(input.capabilityId==="rethlas-research"&&(!input.researchAgent||input.researchAgent==="mathcat"))return sendJson(res,202,await v2Bridge.start(conversation.id,text,{durationSeconds:input.durationSeconds===undefined?7200:Number(input.durationSeconds),reviewMode:input.mathcatReviewMode||"strict",maxPartners:input.maxPartners===undefined?5:Number(input.maxPartners),autoDeliver:input.autoDeliver!==false}));req.cachedJsonBody=input;}
if(req.method==="GET"&&pathname==="/api/capabilities")return sendJson(res,200,capabilities.map(({root,skillFile,adapterFile,...item})=>item));
if(req.method==="GET"&&pathname==="/api/research-agents")return sendJson(res,200,await researchAgents.list());
const rbcm=pathname.match(/^\/api\/research-boards\/by-conversation\/([^/]+)$/);if(req.method==="GET"&&rbcm){const board=await researchBoards.viewByConversation(rbcm[1]);return board?sendJson(res,200,board):fail(res,404,"Research board not found.")}
if(req.method==="POST"&&pathname==="/api/research-boards"){const input=await readBody(req);const conversation=store.getConversation(input.conversationId);if(!conversation)return fail(res,404,"Conversation not found.");return sendJson(res,201,await researchBoards.create({conversationId:conversation.id,workspaceId:conversation.workspaceId,agent:input.agent||"mathcat",problem:String(input.problem||conversation.title),title:conversation.title,reviewMode:input.reviewMode||"strict"}))}
const rbm=pathname.match(/^\/api\/research-boards\/([^/]+)$/);if(req.method==="GET"&&rbm){const board=await researchBoards.view(rbm[1]);return board?sendJson(res,200,board):fail(res,404,"Research board not found.")}
if(req.method==="PATCH"&&rbm)return sendJson(res,200,await researchBoards.updateProblem(rbm[1],await readBody(req)));
const rbrm=pathname.match(/^\/api\/research-boards\/([^/]+)\/routes$/);if(req.method==="POST"&&rbrm){const board=await researchBoards.addRoute(rbrm[1],await readBody(req));await resumeBoardWatcher(rbrm[1],board);return sendJson(res,201,board)}
const rbrcm=pathname.match(/^\/api\/research-boards\/([^/]+)\/routes\/([^/]+)\/commands$/);if(req.method==="POST"&&rbrcm){const input=await readBody(req);const board=await researchBoards.routeCommand(rbrcm[1],rbrcm[2],input.command);await resumeBoardWatcher(rbrcm[1],board);return sendJson(res,200,board)}
const rbsm=pathname.match(/^\/api\/research-boards\/([^/]+)\/suggestions$/);if(req.method==="POST"&&rbsm){const board=await researchBoards.addPlanningSuggestion(rbsm[1],await readBody(req));await resumeBoardWatcher(rbsm[1],board);return sendJson(res,201,board)}
const rbgrm=pathname.match(/^\/api\/research-boards\/([^/]+)\/goal-review$/);if(req.method==="POST"&&rbgrm){const board=await researchBoards.forceGoalReview(rbgrm[1],await readBody(req));await resumeBoardWatcher(rbgrm[1],board);return sendJson(res,202,board)}
const rbsetm=pathname.match(/^\/api\/research-boards\/([^/]+)\/settings$/);if(req.method==="PATCH"&&rbsetm){const board=await researchBoards.updateSettings(rbsetm[1],await readBody(req));await resumeBoardWatcher(rbsetm[1],board);return sendJson(res,200,board)}
const rbdm=pathname.match(/^\/api\/research-boards\/([^/]+)\/decisions$/);if(req.method==="POST"&&rbdm)return sendJson(res,201,await researchBoards.addDecision(rbdm[1],await readBody(req)));
const rbdam=pathname.match(/^\/api\/research-boards\/([^/]+)\/decisions\/([^/]+)$/);if(req.method==="POST"&&rbdam){const input=await readBody(req);const board=await researchBoards.answerDecision(rbdam[1],rbdam[2],input.answer,input.note||"");await resumeBoardWatcher(rbdam[1],board);return sendJson(res,200,board)}
const ram=pathname.match(/^\/api\/research-agents\/([^/]+)\/configure$/);if(req.method==="POST"&&ram){const input=await readBody(req);const result=await researchAgents.configure(ram[1],input.path||null);const conversation=await store.createConversation({workspaceId:null,title:`配置 ${result.name}`});await store.updateConversation(conversation.id,(row)=>{row.configuringResearchAgent=result.found?null:result.id;row.messages.push({id:crypto.randomUUID(),role:"user",content:`首次使用 ${result.name}，请查找本地智能体并完成配置。`,createdAt:new Date().toISOString(),executor:"codex",capabilityId:null});row.messages.push(result.found?{id:crypto.randomUUID(),role:"assistant",content:`已找到并配置 ${result.name}：\n${result.path}\n\n后续选择“研究喵 · ${result.name}”时将通过 Codex CLI 使用该智能体。`,createdAt:new Date().toISOString(),executor:"codex",capabilityId:null}:{id:crypto.randomUUID(),role:"assistant",content:`未在常用位置找到本地 ${result.name}。请选择下一步：`,createdAt:new Date().toISOString(),executor:"codex",capabilityId:null,permission:"workspace-write",choice:{question:`需要如何配置 ${result.name}？`,options:[{label:"从 GitHub 下载并配置",reason:`使用官方仓库 ${result.repo}`,value:`请从 ${result.repo} 下载 ${result.name}，检查项目说明并完成本地配置。`},{label:"手动输入智能体地址",reason:"由我提供已有的本地目录",value:`请提示我输入 ${result.name} 的本地目录地址；我下一条消息会粘贴绝对路径。`}],recommendedIndex:null,recommendationReason:null,selectedIndex:null}});});return sendJson(res,201,{...result,conversationId:conversation.id})}
if(req.method==="GET"&&pathname==="/api/workspaces")return sendJson(res,200,store.listWorkspaces());
if(req.method==="POST"&&pathname==="/api/workspaces")return sendJson(res,201,await store.addWorkspace(await readBody(req)));
const wm=pathname.match(/^\/api\/workspaces\/([^/]+)$/);if(req.method==="DELETE"&&wm){const conversations=store.listConversations().filter(item=>item.workspaceId===wm[1]);if(conversations.some(item=>tasks.getActivity(item.id).running))return fail(res,409,"Cannot remove a workspace while one of its conversations is running.");const conversationIds=conversations.map(item=>item.id);await researchBoards.deleteByConversations(conversationIds);const removed=await store.deleteWorkspace(wm[1]);return sendJson(res,200,{ok:true,removedConversations:removed.conversationIds.length})}
if(req.method==="GET"&&pathname==="/api/directories"){
  const requested=url.searchParams.get("path");
  if(!requested){const roots=[];for(let code=67;code<=90;code++){const drive=`${String.fromCharCode(code)}:\\`;try{if((await fs.stat(drive)).isDirectory())roots.push({name:drive,path:drive})}catch{}}return sendJson(res,200,{path:null,parent:null,entries:roots})}
  const current=path.resolve(requested);const stat=await fs.stat(current);if(!stat.isDirectory())return fail(res,400,"Path is not a directory.");
  const entries=(await fs.readdir(current,{withFileTypes:true})).filter(item=>item.isDirectory()).map(item=>({name:item.name,path:path.join(current,item.name)})).sort((a,b)=>a.name.localeCompare(b.name,"zh-CN"));
  const parsed=path.parse(current);return sendJson(res,200,{path:current,parent:current===parsed.root?null:path.dirname(current),entries});
}
if(req.method==="POST"&&pathname==="/api/folder-picker")return sendJson(res,200,{path:await pickFolder()});
if(req.method==="GET"&&pathname==="/api/conversations")return sendJson(res,200,store.listConversations());
if(req.method==="POST"&&pathname==="/api/conversations")return sendJson(res,201,await store.createConversation(await readBody(req)));
const cm=pathname.match(/^\/api\/conversations\/([^/]+)$/);if(req.method==="GET"&&cm){const row=store.getConversation(cm[1]);return row?sendJson(res,200,row):fail(res,404,"Conversation not found.")}
if(req.method==="DELETE"&&cm){if(tasks.getActivity(cm[1]).running)return fail(res,409,"Cannot remove a running conversation.");await researchBoards.deleteByConversation(cm[1]);await store.deleteConversation(cm[1]);return sendJson(res,200,{ok:true})}
const am=pathname.match(/^\/api\/conversations\/([^/]+)\/activity$/);if(req.method==="GET"&&am)return sendJson(res,200,tasks.getActivity(am[1]));
const cancelm=pathname.match(/^\/api\/conversations\/([^/]+)\/cancel$/);if(req.method==="POST"&&cancelm)return sendJson(res,202,await tasks.cancel(cancelm[1]));
const mm=pathname.match(/^\/api\/conversations\/([^/]+)\/messages$/);if(req.method==="POST"&&mm){const input=await readBody(req);const text=String(input.text||"").trim();if(!text)return fail(res,400,"Message is required.");if(input.executor&&input.executor!=="codex")return fail(res,400,"Only the Codex executor is available.");const currentConversation=store.getConversation(mm[1]);const configuring=currentConversation?.configuringResearchAgent;const manualPath=text.replace(/^["']|["']$/g,"");if(configuring&&/^[A-Za-z]:[\\/]/.test(manualPath)){const result=await researchAgents.configure(configuring,manualPath);await store.updateConversation(mm[1],row=>{row.configuringResearchAgent=null;row.messages.push({id:crypto.randomUUID(),role:"user",content:text,createdAt:new Date().toISOString(),executor:"codex",capabilityId:null});row.messages.push({id:crypto.randomUUID(),role:"assistant",content:`已验证并保存 ${result.name} 地址：\n${result.path}\n\n现在可以返回“研究喵”并选择 ${result.name}。`,createdAt:new Date().toISOString(),executor:"codex",capabilityId:null});});return sendJson(res,202,{ok:true,configured:true})}let researchAgent=null;if(input.capabilityId==="rethlas-research"){researchAgent=await researchAgents.get(input.researchAgent||"mathcat");if(!researchAgent?.configured)return fail(res,409,`${researchAgent?.name||"Research agent"} is not configured.`);const reviewMode=["automatic","balanced","strict"].includes(input.mathcatReviewMode)?input.mathcatReviewMode:"strict";const resolvedProblem=await resolveResearchProblem({message:text,workspace:store.workspaceFor(currentConversation)});await researchBoards.create({conversationId:mm[1],workspaceId:currentConversation?.workspaceId,agent:researchAgent.id,problem:resolvedProblem.problem,targetStatement:resolvedProblem.targetStatement,contextSources:resolvedProblem.sources,reviewMode})}return sendJson(res,202,await tasks.start({conversationId:mm[1],text,executor:"codex",capabilityId:input.capabilityId||null,researchAgent,permission:input.permission==="read-only"?"read-only":"workspace-write"}))}
const chm=pathname.match(/^\/api\/conversations\/([^/]+)\/choices$/);if(req.method==="POST"&&chm){if(store.getConversation(chm[1])?.researchProjectId)return fail(res,409,"此对话已进入 MathCat 研究，请从白板处理当前事项。");const input=await readBody(req);return sendJson(res,202,await tasks.choose({conversationId:chm[1],messageId:input.messageId,optionIndex:input.optionIndex}))}
if(req.method==="GET"&&pathname==="/api/artifact"){const c=store.getConversation(url.searchParams.get("conversationId"));if(!c)return fail(res,404,"Conversation not found.");const workspace=store.workspaceFor(c);const requested=path.resolve(url.searchParams.get("path")||"");ensureInside(workspace.path,requested);if(!c.messages.some(item=>(item.artifacts||[]).includes(requested)))return fail(res,403,"Artifact is not registered.");const data=await fs.readFile(requested);res.writeHead(200,{...userFileHeaders(requested),"content-length":data.length});return res.end(data)}
if(await workspaceFileRoute({req,res,url,store,userFileHeaders,sendJson,reveal:revealInFileManager}))return;
if(req.method==="GET")return await serveStatic(res,pathname);return fail(res,404,"Not found.")}catch(error){return fail(res,errorStatus(error),error.message)}});
server.listen(config.port,config.host,()=>console.log(`Math Lab Platform listening on http://${config.host}:${config.port}/`));
