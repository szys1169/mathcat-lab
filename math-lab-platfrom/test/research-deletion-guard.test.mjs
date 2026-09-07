import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import vm from "node:vm";

const source = await fs.readFile(new URL("../src/server.mjs", import.meta.url), "utf8");
// Exercise the actual request callback without importing server bootstrap, opening a port,
// writing a Store, or touching the user's running processes.
const callbackSource = source.slice(source.indexOf("function sendJson("), source.lastIndexOf("server.listen("));
const timestamp = "2026-09-06T00:00:00.000Z";
const endedProject = () => ({id:"p",runs:[{id:"r",state:"ended"}],interactions:[],sessions:[],usage:[]});
function endedExplanation({unknown=false,outstanding=false}={}) {
  return {id:"p",runs:[{id:"r",state:"ended"}],interactions:[{id:"i",state:"ended",outstanding_cancellation:outstanding}],sessions:[{id:"s",role:"discussion",state:"closed",execution_owner:{kind:"interaction",id:"i"}}],usage:[{id:"u",session_id:"s",state:unknown?"unknown":"succeeded",ended_at:timestamp,execution_owner:{kind:"interaction",id:"i"},input_tokens:null,output_tokens:null,cached_input_tokens:null,cost:null}]};
}
function harness(snapshot,{secondSnapshot,upstreamError}={}) {
  let handler;
  const writes=[],requests=[];
  let conversations=[{id:"c",workspaceId:"w",researchProjectId:"p"}];
  if(secondSnapshot) conversations.push({id:"second",workspaceId:"w",researchProjectId:"q"});
  const store={getConversation:id=>conversations.find(row=>row.id===id),listConversations:()=>conversations,deleteConversation:async id=>{writes.push(["deleteConversation",id]);conversations=conversations.filter(row=>row.id!==id);},deleteWorkspace:async id=>{writes.push(["deleteWorkspace",id]);return{conversationIds:conversations.map(row=>row.id)};}};
  const context={Buffer,URL,console,config:{port:4325},V2_PREFIX:"/api/v2/research",assertLocalRequest:()=>{},store,tasks:{getActivity:()=>({running:false})},researchBoards:{deleteByConversation:async id=>writes.push(["deleteBoard",id]),deleteByConversations:async ids=>writes.push(["deleteBoards",ids])},researchV2:{request:async pathname=>{requests.push(pathname);if(upstreamError)throw upstreamError;return{project:pathname.includes("/q/")?secondSnapshot:snapshot};}},http:{createServer:callback=>{handler=callback;return{};}}};
  vm.runInNewContext(callbackSource,context,{filename:"server-deletion-callback.mjs"});
  return {writes,requests,blocked:project=>context.researchHasUnfinishedExecution(project),delete:async pathname=>{const before=JSON.stringify(snapshot);const response={writeHead(status){this.status=status;},end(data){this.body=JSON.parse(data);}};await handler({method:"DELETE",url:pathname,headers:{host:"127.0.0.1:4325"}},response);assert.equal(JSON.stringify(snapshot),before,"deletion check must not rewrite usage, flags, or backend history");return response;}};
}

test("deleting a conversation preserves an active independent explanation after its main Run ended",async()=>{
  const project=endedExplanation();project.interactions[0].state="running";project.sessions[0].state="active";project.usage[0].state="running";project.usage[0].ended_at=null;
  const server=harness(project);const response=await server.delete("/api/conversations/c");
  assert.equal(response.status,409);assert.match(response.body.error,/独立解释/);assert.deepEqual(server.writes,[]);
});
test("workspace deletion checks every linked research project before deleting any entry",async()=>{
  const active=endedExplanation();active.id="q";active.interactions[0].state="stopping";
  const server=harness(endedProject(),{secondSnapshot:active});const response=await server.delete("/api/workspaces/w");
  assert.equal(response.status,409);assert.equal(server.requests.length,2);assert.deepEqual(server.writes,[]);
});
for(const state of ["created","preflighting","running","waiting_human","pausing","paused","stopping"]){
  test(`non-ended Run ${state} still protects deletion`,()=>{const project=endedProject();project.runs[0].state=state;assert.equal(harness(project).blocked(project),true);});
}
for(const state of ["created","running","stopping"]){
  test(`non-ended independent interaction ${state} protects deletion even without a worker`,()=>{const project=endedProject();project.interactions.push({id:"i",state});assert.equal(harness(project).blocked(project),true);});
}
for(const state of ["active","idle","waiting","lost"]){
  test(`unclosed discussion session ${state} protects deletion`,()=>{const project=endedProject();project.sessions.push({id:"s",role:"discussion",state});assert.equal(harness(project).blocked(project),true);});
}
for(const state of ["reserved","running"]){
  test(`in-flight ${state} usage protects deletion even if owner state already says ended`,()=>{const project=endedExplanation();project.usage[0].state=state;assert.equal(harness(project).blocked(project),true);});
}
for(const ended_at of [null,"not-a-date"]){
  test(`unknown usage with invalid local end receipt ${ended_at} cannot lose its history entry`,async()=>{const project=endedExplanation({unknown:true});project.usage[0].ended_at=ended_at;const server=harness(project);assert.equal((await server.delete("/api/conversations/c")).status,409);assert.deepEqual(server.writes,[]);});
}
test("outstanding cancellation without matching local closure evidence remains protected",()=>{
  const project=endedProject();project.runs[0].outstanding_cancellation=true;assert.equal(harness(project).blocked(project),true);
  const interaction=endedExplanation({unknown:true,outstanding:true});interaction.usage[0].execution_owner.id="different-interaction";assert.equal(harness(interaction).blocked(interaction),true);
});
test("a completed independent explanation permits deletion of only the local entry",async()=>{
  const server=harness(endedExplanation());assert.equal((await server.delete("/api/conversations/c")).status,200);
  assert.ok(server.writes.some(([kind])=>kind==="deleteConversation"));assert.ok(server.requests.every(url=>url.endsWith("/snapshot")));
});
test("historical unknown billing with proven local closure does not permanently prevent deletion",async()=>{
  const project=endedExplanation({unknown:true,outstanding:true});const server=harness(project);
  assert.equal((await server.delete("/api/conversations/c")).status,200);
  assert.equal(project.usage[0].state,"unknown");assert.equal(project.usage[0].cost,null);assert.equal(project.interactions[0].outstanding_cancellation,true);
});
test("the same local-closure evidence handles historical Run cancellation without erasing unknown usage",async()=>{
  const project=endedProject();project.runs[0].outstanding_cancellation=true;project.sessions=[{id:"s",run_id:"r",state:"closed"}];project.usage=[{id:"u",run_id:"r",session_id:"s",state:"unknown",ended_at:timestamp,cost:null}];
  const server=harness(project);assert.equal((await server.delete("/api/workspaces/w")).status,200);assert.equal(project.usage[0].state,"unknown");
});
test("backend snapshot failure leaves all conversation and workspace records untouched",async()=>{
  const server=harness(endedProject(),{upstreamError:Object.assign(new Error("backend unavailable"),{status:503})});
  assert.equal((await server.delete("/api/conversations/c")).status,503);assert.equal((await server.delete("/api/workspaces/w")).status,503);assert.deepEqual(server.writes,[]);
});
test("malformed execution collections fail closed while a genuinely empty ended project remains deletable",()=>{
  const server=harness(endedProject());assert.equal(server.blocked({...endedProject(),interactions:{state:"running"}}),true);assert.equal(server.blocked({...endedProject(),sessions:[null]}),true);assert.equal(server.blocked(endedProject()),false);
});
