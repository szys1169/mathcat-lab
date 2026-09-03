import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { Store } from "../src/store.mjs";
import { loadCapabilities } from "../src/capabilities.mjs";
import { parseChoiceResponse } from "../src/task-manager.mjs";
import { ResearchAgents } from "../src/research-agents.mjs";
import { ResearchBoards } from "../src/research-boards.mjs";

test("workspace and conversation persist", async () => {
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"math-lab-")); const workspace=path.join(root,"workspace"); await fs.mkdir(workspace);
  const store=await new Store(path.join(root,"runtime")).load(); const w=await store.addWorkspace({workspacePath:workspace,name:"Test"}); const c=await store.createConversation({workspaceId:w.id}); await store.updateConversation(c.id,row=>row.messages.push({role:"user",content:"hello"}));
  const restored=await new Store(path.join(root,"runtime")).load(); assert.equal(restored.getConversation(c.id).messages[0].content,"hello");
});

test("capability index exposes seven complete capabilities", async () => {
  const rows=await loadCapabilities(path.resolve(import.meta.dirname,"..","..","capabilities")); assert.equal(rows.length,7); for(const row of rows) { assert.ok(row.skillFile&&row.adapterFile); await fs.access(row.skillFile); await fs.access(row.adapterFile); }
});

test("deleting a conversation only removes its record", async () => {
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"math-lab-delete-chat-")); const workspace=path.join(root,"workspace"); await fs.mkdir(workspace); const marker=path.join(workspace,"keep.txt"); await fs.writeFile(marker,"keep");
  const store=await new Store(path.join(root,"runtime")).load(); const w=await store.addWorkspace({workspacePath:workspace,name:"Test"}); const c=await store.createConversation({workspaceId:w.id}); await store.deleteConversation(c.id);
  assert.equal(store.getConversation(c.id),null); assert.equal(await fs.readFile(marker,"utf8"),"keep"); assert.equal(store.listWorkspaces().length,1);
});

test("projectless conversations use an isolated internal directory", async () => {
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"math-lab-projectless-")); const store=await new Store(path.join(root,"runtime")).load(); const c=await store.createConversation({workspaceId:null,title:"独立对话"}); const virtual=store.workspaceFor(c);
  assert.equal(c.workspaceId,null); assert.equal(virtual.projectless,true); assert.ok(virtual.path.startsWith(path.join(root,"runtime","projectless"))); assert.equal((await fs.stat(virtual.path)).isDirectory(),true);
  const restored=await new Store(path.join(root,"runtime")).load(); assert.equal(restored.getConversation(c.id).title,"独立对话");
});

test("deleting a workspace cascades platform conversations but keeps files", async () => {
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"math-lab-delete-workspace-")); const workspace=path.join(root,"workspace"); await fs.mkdir(workspace); const marker=path.join(workspace,"keep.txt"); await fs.writeFile(marker,"keep");
  const store=await new Store(path.join(root,"runtime")).load(); const w=await store.addWorkspace({workspacePath:workspace,name:"Test"}); await store.createConversation({workspaceId:w.id}); const result=await store.deleteWorkspace(w.id);
  assert.equal(result.conversationIds.length,1); assert.equal(store.listWorkspaces().length,0); assert.equal(store.listConversations().length,0); assert.equal(await fs.readFile(marker,"utf8"),"keep");
});

test("choice blocks only recommend an option when an explicit reason exists", () => {
  const raw='请先选择。\n```math-lab-choice\n{"question":"选择 Source Packet","options":[{"label":"A","value":"D:/A","reason":"主题完全匹配"},{"label":"B","value":"D:/B","reason":"相关但较旧"}],"recommendedIndex":0,"recommendationReason":"A 的主题和任务要求完全一致，而 B 是旧版本。"}\n```';
  const parsed=parseChoiceResponse(raw); assert.equal(parsed.content,"请先选择。"); assert.equal(parsed.choice.options.length,2); assert.equal(parsed.choice.recommendedIndex,0); assert.match(parsed.choice.recommendationReason,/完全一致/);
  const neutral=parseChoiceResponse('```math-lab-choice\n{"question":"选择","options":[{"label":"A","value":"A","reason":"可用"},{"label":"B","value":"B","reason":"可用"}]}\n```'); assert.equal(neutral.choice.recommendedIndex,null);
});

test("research agents expose three choices and persist discovered roots", async () => {
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"math-lab-agents-")); const appRoot=path.join(root,"math-lab-platfrom"); const mathcatRoot=path.join(root,"math-research-mvp"); const rethlasRoot=path.join(root,"agents","Rethlas"); const runtimeRoot=path.join(root,"runtime");
  await fs.mkdir(appRoot,{recursive:true}); await fs.mkdir(mathcatRoot,{recursive:true}); await fs.mkdir(rethlasRoot,{recursive:true}); await fs.mkdir(runtimeRoot,{recursive:true});
  const agents=await new ResearchAgents({appRoot,runtimeRoot}).load(); const initial=await agents.list(); assert.deepEqual(initial.map(item=>item.id),["mathcat","rethlas","danus"]); assert.equal(initial.find(item=>item.id==="mathcat").path,mathcatRoot); assert.equal(initial.find(item=>item.id==="rethlas").configured,false);
  const configured=await agents.configure("rethlas"); assert.equal(configured.path,rethlasRoot); const restored=await new ResearchAgents({appRoot,runtimeRoot}).load(); assert.equal((await restored.get("rethlas")).configured,true);
});

test("research boards persist agent-specific structures and human decisions", async () => {
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"math-lab-boards-")); const boards=await new ResearchBoards(root).load();
  const mathcat=await boards.create({conversationId:"c1",workspaceId:null,agent:"mathcat",problem:"证明 P"}); assert.equal(mathcat.routes.length,2); assert.equal(mathcat.mode,"human_collaboration");
  const route=mathcat.routes[0]; await boards.routeCommand(mathcat.id,route.id,"approve"); assert.equal(boards.get(mathcat.id).routes[0].status,"active");
  const withDecision=await boards.addDecision(mathcat.id,{question:"允许加强假设吗？",context:"当前假设不足以闭合归纳步骤",options:[{label:"允许",value:"approve",reason:"先得到条件性结果"},{label:"不允许",value:"reject",description:"寻找不加强假设的路线"}],blockingEntityIds:["route_1"]}); const decision=withDecision.decisions[0]; assert.equal(decision.options[0].description,"先得到条件性结果"); assert.deepEqual(decision.blockingEntityIds,["route_1"]); await boards.answerDecision(mathcat.id,decision.id,"defer","先研究其它路线"); assert.equal(boards.get(mathcat.id).decisions[0].status,"pending"); await boards.answerDecision(mathcat.id,decision.id,"reject","不要改变原命题"); assert.equal(boards.get(mathcat.id).decisions[0].answer,"reject"); assert.equal(boards.get(mathcat.id).decisions[0].note,"不要改变原命题");
  const rethlas=await boards.create({conversationId:"c2",agent:"rethlas",problem:"证明 Q"}); assert.equal(rethlas.mode,"generation_verification"); assert.ok(rethlas.verification);
  const danus=await boards.create({conversationId:"c3",agent:"danus",problem:"证明 R"}); assert.equal(danus.mode,"orchestration_truth"); assert.ok(danus.memories&&danus.facts);
  const restored=await new ResearchBoards(root).load(); assert.equal(restored.byConversation("c1").revision,5);
  await restored.deleteByConversation("c2"); assert.equal(restored.byConversation("c2"),null); assert.ok(restored.byConversation("c1"));
});
