import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import http from "node:http";
import {spawn} from "node:child_process";
import {storeResearchMaterial,publicMaterial} from "../src/research-materials.mjs";
import {ResearchV2Client,assertLocalRequest,validateV2Path,unwrapProject} from "../src/research-v2-client.mjs";
import {ResearchV2Bridge} from "../src/research-v2-bridge.mjs";
import {Store} from "../src/store.mjs";
import {TaskManager} from "../src/task-manager.mjs";
import {normalizeSnapshot,label,isTrusted,commandFor,remainingTime,uniqueEvents,escapeHtml,publicEventText,researchReplies,conversationCaption,problemVersionCaption} from "../public/research-v2-state.js";
import {renderClassicResearchBoard} from "../public/research-v2-classic.js";

test("v2 snapshot preserves independent assurance and validity without inventing facts",()=>{
  const p=normalizeSnapshot({contract:"mathcat-research/v2",project:{id:"p",nodes:[{id:"n",work_state:"done",assurance:"formally_checked",validity:"revoked"}]},event_cursor:44});
  assert.equal(p.event_cursor,44);assert.equal(isTrusted(p.nodes[0]),false);assert.equal(p.facts.length,0);
  assert.equal(isTrusted({assurance:"model_reviewed",validity:"current"}),true);
  assert.throws(()=>normalizeSnapshot({contract:"mathcat-research/v1",id:"p"}),/协议/);
  assert.throws(()=>normalizeSnapshot({nodes:[]}),/身份/);
});

test("v2 command receipts separate lifecycle from mathematical disposition",()=>{
  assert.equal(label("acknowledged"),"已回应");assert.equal(label("completed"),"已处理");assert.equal(label("declined"),"未采纳");
  const p={id:"p",problem_version:2,runs:[{id:"r",state:"running",control_epoch:5}]};
  const suggestion=commandFor(p,"suggest_idea",{node:{id:"n",revision:3},text:"试试紧性"});
  assert.equal(suggestion.apply_at,"next_safe_boundary");assert.equal(suggestion.target.revision,3);assert.equal(suggestion.expected_versions.problem_version,2);assert.equal(suggestion.expected_versions.control_epoch,5);
  const stop=commandFor(p,"stop_run");assert.equal(stop.target.kind,"run");assert.equal(stop.apply_at,"immediate");
  assert.equal(commandFor(p,"challenge_evidence",{node:{id:"n",revision:3},text:"缺量词"}).payload.request_paid_review,false);
  assert.throws(()=>commandFor({id:"p",runs:[]},"pause_run"),/尚无/);
});

test("v2 sidebar distinguishes problem versions and never calls bound research an empty idle chat",()=>{
  assert.equal(problemVersionCaption({problem_version:1}),"题目v1");
  assert.equal(problemVersionCaption({current_problem_version:4}),"题目v4");
  assert.equal(conversationCaption({researchProjectId:"p",messageCount:0,status:"idle"}),"研究记录 · 查看实时状态");
  assert.equal(conversationCaption({messageCount:3,status:"completed"}),"3 条 · completed");
});

test("v2 paused deadline never gains time and unknown time is null",()=>{
  assert.equal(remainingTime({state:"paused",deadline_at:"2026-09-06T00:01:00Z"},Date.parse("2026-09-06T00:00:45Z")),15);
  assert.equal(remainingTime({deadline_at:"2026-09-06T00:01:00Z"},Date.parse("2026-09-06T00:02:00Z")),0);
  assert.equal(remainingTime({state:"running"}),null);
});

test("v2 public replies follow real core notes and keep discussion output separate",()=>{
  const p=normalizeSnapshot({id:'p',sessions:[{id:'main',role:'main',run_id:'r'},{id:'explain',role:'discussion'}],nodes:[{id:'n',node_type:'note',author:'main',body:'主研究结论'},{id:'q',node_type:'note',author:'explain',body:'仅讨论'}],interactions:[{id:'i',state:'stopping'}]});
  assert.equal(researchReplies(p).length,1);assert.equal(researchReplies(p)[0].body,'主研究结论');assert.equal(p.interactions[0].state,'stopping');
  assert.equal(publicEventText({payload:{discussion_id:'d',activity:{type:'message.delta',text:'解释公开输出'}}}),'解释公开输出');
});

test("v2 report display preserves historical stop reason without declaring disputed result solved",()=>{
  const project=normalizeSnapshot({id:'p',runs:[{id:'r',state:'ended',stop_reason:'goal_satisfied',result_state:'disputed'}],reports:[{run_id:'r',validity:'challenged'}]});
  const html=renderClassicResearchBoard({project},{escapeHtml,renderText:x=>String(x||''),graphCards:''});
  assert.match(html,/结论有争议/);assert.match(html,/历史快照/);assert.doesNotMatch(html,/数学结果：已解决/);
});

test("v2 public event replay deduplicates and sorts without modifying inputs",()=>{
  const e=[{project_id:"p",seq:3},{project_id:"p",seq:2},{project_id:"p",seq:3}];assert.deepEqual(uniqueEvents(e).map(x=>x.seq),[2,3]);assert.equal(e.length,3);
  assert.equal(escapeHtml('<script x="1">'),"&lt;script x=&quot;1&quot;&gt;");
});

test("v2 proxy only accepts loopback host and same-origin browser requests",()=>{
  assert.doesNotThrow(()=>assertLocalRequest({headers:{host:"127.0.0.1:4335",origin:"http://127.0.0.1:4335"}},4335));
  assert.throws(()=>assertLocalRequest({headers:{host:"evil.example:4335"}},4335),/host/);
  assert.throws(()=>assertLocalRequest({headers:{host:"127.0.0.1:4335",origin:"https://evil.example"}},4335),/origin/);
  assert.throws(()=>assertLocalRequest({headers:{host:"127.0.0.1:4335","sec-fetch-site":"cross-site"}},4335),/Cross-site/);
  assert.throws(()=>new ResearchV2Client({baseUrl:"https://example.com"}),/local HTTP/);
  assert.throws(()=>validateV2Path("https://example.com/api/v2/research/projects"),/path/);
  assert.throws(()=>validateV2Path("/api/v2/research/%2e%2e/secrets"),/path/);
  assert.throws(()=>validateV2Path("/api/v2/research/projects\\secrets"),/path/);
});

test("v2 server client injects only version-local bearer and keeps it out of responses",async()=>{
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"mathcat-v2-token-"));const tokenFile=path.join(root,"token");await fs.writeFile(tokenFile,"private-test-token\n");
  let captured;
  const client=new ResearchV2Client({tokenFile,fetchImpl:async(url,options)=>{captured={url,options};return new Response(JSON.stringify({contract:"mathcat-research/v2",project:{id:"p"}}));}});
  const value=await client.request("/api/v2/research/projects",{method:"POST",body:{title:"test"},idempotencyKey:"once"});
  assert.equal(captured.url,"http://127.0.0.1:8900/api/v2/research/projects");assert.equal(captured.options.headers.authorization,"Bearer private-test-token");assert.equal(captured.options.headers["idempotency-key"],"once");assert.equal(JSON.stringify(value).includes("private-test-token"),false);
  const missing=new ResearchV2Client({tokenFile:path.join(root,"missing"),fetchImpl:()=>{throw new Error("must not fetch");}});await assert.rejects(()=>missing.request("/api/v2/research/projects"),/本版本/);
});

test("v2 bridge creates project once, binds before start and explicitly authorizes bounded Run",async()=>{
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"mathcat-v2-bridge-"));const store=await new Store(root).load();const c=await store.createConversation({title:"连续研究"});
  const calls=[];const client={request:async(url,options)=>{calls.push({url,...options});return url.endsWith("/runs")?{contract:"mathcat-research/v2",run:{id:"r"}}:{contract:"mathcat-research/v2",project:{id:"p",workspace_path:"version/workspaces/p"}};}};
  const bridge=new ResearchV2Bridge({store,client});const result=await bridge.start(c.id,"完整原题",{durationSeconds:7200,reviewMode:"balanced"});
  assert.equal(result.projectId,"p");assert.equal(store.getConversation(c.id).researchProjectId,"p");assert.equal(calls.length,2);assert.equal(calls[1].body.start_authorized,true);assert.equal(calls[1].body.duration_seconds,7200);assert.equal(calls[1].body.mode,"collaborative");
  assert.equal(calls.some(x=>x.url.includes("/routes")),false);await assert.rejects(()=>bridge.start(c.id,"再次"),/已有回执/);assert.equal(calls.filter(x=>x.method==="POST").length,2);
});

test("v2 failed preflight retains project and user question instead of orphaning its receipt",async()=>{
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"mathcat-v2-retain-"));const store=await new Store(root).load();const c=await store.createConversation({title:"预检"});
  const bridge=new ResearchV2Bridge({store,client:{request:async url=>{if(url.endsWith("/runs"))throw new Error("Auth unavailable");return{project:{id:"p"}};}}});
  await assert.rejects(()=>bridge.start(c.id,"原题保留"),/Auth unavailable/);assert.equal(store.getConversation(c.id).researchProjectId,"p");assert.equal(store.getConversation(c.id).messages[0].content,"原题保留");
  const restored=await new Store(root).load();assert.equal(restored.getConversation(c.id).researchContract,"mathcat-research/v2");
});

test("v2 Node restart does not relaunch Rust-owned research using legacy runner",async()=>{
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"mathcat-v2-recovery-"));const store=await new Store(root).load();const c=await store.createConversation({title:"恢复"});
  await store.updateConversation(c.id,row=>{row.status="running";row.researchProjectId="p";row.researchContract="mathcat-research/v2";});
  const manager=new TaskManager({store,config:{},capabilities:[],researchBoards:{rows:[]}});await manager.recoverInterrupted();
  assert.equal(store.getConversation(c.id).status,"running");assert.equal(store.getConversation(c.id).error,undefined);
});

test("v2 binding reuses project and refuses replacing a conversation's existing project",async()=>{
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"mathcat-v2-binding-"));const store=await new Store(root).load();const c=await store.createConversation({title:"节点讨论"});let target="p1";
  const bridge=new ResearchV2Bridge({store,client:{request:async()=>({project:{id:target}})}});
  await bridge.bind(c.id,"p1");target="p2";await assert.rejects(()=>bridge.bind(c.id,"p2"),/其他研究/);assert.equal(store.getConversation(c.id).researchProjectId,"p1");
  assert.throws(()=>unwrapProject({project:{}}),/身份/);
});

test("2.5.1 keeps the classic transport separate from the new whiteboard renderer",async()=>{
  const root=path.resolve(import.meta.dirname,"..");const app=await fs.readFile(path.join(root,"public","app.js"),"utf8");const adapter=await fs.readFile(path.join(root,"public","research-v2-classic.js"),"utf8");
  const whiteboard=await fs.readFile(path.join(root,"public","whiteboard-view.js"),"utf8");
  assert.match(app,/ClassicResearchClient/);assert.match(app,/MathCat Lab 2\.5\.1/);assert.doesNotMatch(app,/ResearchV2View|v2-split|v2RunConfig|v2-intent/);assert.doesNotMatch(adapter,/document\.|createElement|appendChild/);assert.match(adapter,/reply_mode:'explain'/);assert.match(whiteboard,/停止本轮受管研究、伙伴、审核及讨论/);
});

test("v2 materials import exact text before Run and fail clearly on unreadable PDF",async()=>{
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"mathcat-v2-material-"));const store=await new Store(root).load();const c=await store.createConversation({title:"材料"});
  const text="定理：对所有实数 x，x² ≥ 0。";const material=await storeResearchMaterial({runtimeRoot:root,conversationId:c.id,name:"theorem.tex",contentBase64:Buffer.from(text).toString("base64")});
  assert.equal(publicMaterial(material).contentPath,undefined);assert.equal(await fs.readFile(material.contentPath,"utf8"),text);
  await store.updateConversation(c.id,row=>{row.researchMaterials=[material];});
  const calls=[];const bridge=new ResearchV2Bridge({store,client:{request:async(url,options)=>{calls.push({url,...options});if(url.endsWith("/material-imports"))return{artifact:{id:"a1"}};if(url.endsWith("/runs"))return{run:{id:"r1"}};return{project:{id:"p1"}};}}});
  await bridge.start(c.id,"证明定理");assert.equal(calls[1].body.content,text);assert.equal(calls[1].body.provenance.source_sha256,material.sha256);assert.match(calls[2].url,/\/runs$/);assert.equal(c.researchMaterials[0].artifactId,"a1");
  await assert.rejects(()=>storeResearchMaterial({runtimeRoot:root,conversationId:c.id,name:"scan.pdf",contentBase64:Buffer.from("%PDF").toString("base64"),extractPdf:async()=>""}),/OCR/);
  await assert.rejects(()=>storeResearchMaterial({runtimeRoot:root,conversationId:c.id,name:"payload.html",contentBase64:"eA=="}),/暂支持/);
});

test("v2 unknown Run start can retry same idempotency key without recreating project or question",async()=>{
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"mathcat-v2-retry-"));const store=await new Store(root).load();const c=await store.createConversation({title:"重试"});const calls=[];let first=true;
  const bridge=new ResearchV2Bridge({store,client:{request:async(url,options={})=>{calls.push({url,...options});if(url.endsWith("/runs")){if(first){first=false;throw new Error("response lost");}return{run:{id:"r"}};}return{project:{id:"p",runs:[]}};}}});
  await assert.rejects(()=>bridge.start(c.id,"原题"),/response lost/);await bridge.start(c.id,"原题");
  const writes=calls.filter(c=>c.url.endsWith("/runs"));assert.equal(writes.length,2);assert.equal(writes[0].idempotencyKey,writes[1].idempotencyKey);assert.equal(calls.filter(c=>c.url.endsWith("/projects")).length,1);assert.equal(c.messages.length,1);
});

test("v2 real Node proxy guards activity deletion, routes cancel and preserves upstream status", {timeout:20000},async(t)=>{
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"mathcat-v2-http-"));const tokenFile=path.join(root,"token");const token="mock-token-012345678901234567890123456";await fs.writeFile(tokenFile,token);
  let stopped=false;const calls=[];const backend=http.createServer(async(req,res)=>{
    calls.push({url:req.url,authorization:req.headers.authorization});let body="";for await(const chunk of req)body+=chunk;
    res.setHeader("content-type","application/json");
    if(req.url.includes("/commands")){const command=JSON.parse(body);assert.equal(command.type,"stop_run");stopped=true;res.end(JSON.stringify({contract:"mathcat-research/v2",command:{id:"cmd",status:"completed"}}));return;}
    if(req.url.includes("unavailable")){res.writeHead(503);res.end(JSON.stringify({contract:"mathcat-research/v2",error:{message:"test unavailable"}}));return;}
    res.end(JSON.stringify({contract:"mathcat-research/v2",project:{id:"p",problem_version:1,runs:[{id:"r",control_epoch:0,state:stopped?"ended":"running"}]}}));
  });
  await new Promise(resolve=>backend.listen(0,"127.0.0.1",resolve));t.after(()=>backend.close());const backendPort=backend.address().port;
  const portProbe=http.createServer();await new Promise(resolve=>portProbe.listen(0,"127.0.0.1",resolve));const port=portProbe.address().port;await new Promise(resolve=>portProbe.close(resolve));
  const child=spawn(process.execPath,["src/server.mjs"],{cwd:path.resolve(import.meta.dirname,".."),windowsHide:true,stdio:["ignore","pipe","pipe"],env:{...process.env,MATH_LAB_PORT:String(port),MATH_LAB_RUNTIME_ROOT:path.join(root,"runtime"),MATHCAT_V2_API_URL:"http://127.0.0.1:"+backendPort,MATHCAT_V2_TOKEN_FILE:tokenFile}});t.after(()=>child.kill());
  let stderr="";child.stderr.on("data",data=>stderr+=data);child.stdout.resume();
  const base="http://127.0.0.1:"+port;let ready=false;
  for(let i=0;i<100;i++){try{ready=(await fetch(base+"/api/conversations")).ok;if(ready)break;}catch{}await new Promise(resolve=>setTimeout(resolve,30));}
  assert.equal(ready,true,stderr);
  const request=async(url,method="GET",body,headers={})=>fetch(base+url,{method,headers:{"content-type":"application/json",...headers},body:body===undefined?undefined:JSON.stringify(body)});
  const c=await (await request("/api/conversations","POST",{title:"v2 HTTP"})).json();
  assert.equal((await request("/api/conversations/"+c.id+"/research-project","POST",{projectId:"p"})).status,200);
  assert.equal((await (await request("/api/conversations/"+c.id+"/activity")).json()).running,true);
  assert.equal((await request("/api/conversations/"+c.id,"DELETE")).status,409);
  const badOrigin=await request("/api/v2/research/projects","GET",undefined,{origin:"https://evil.example"});assert.equal(badOrigin.status,403);
  const unavailable=await request("/api/v2/research/projects/unavailable");assert.equal(unavailable.status,503);
  assert.equal((await request("/api/conversations/"+c.id+"/cancel","POST",{})).status,202);
  assert.equal((await (await request("/api/conversations/"+c.id+"/activity")).json()).running,false);
  assert.equal((await request("/api/conversations/"+c.id,"DELETE")).status,200);
  assert.ok(calls.every(c=>c.authorization==="Bearer "+token));
  await fs.unlink(tokenFile);assert.equal((await request("/api/v2/research/projects")).status,503);
});
