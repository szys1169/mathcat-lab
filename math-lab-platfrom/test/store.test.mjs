import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { Store } from "../src/store.mjs";
import { loadCapabilities } from "../src/capabilities.mjs";
import { parseChoiceResponse, TaskManager } from "../src/task-manager.mjs";
import { ResearchAgents } from "../src/research-agents.mjs";
import { ResearchBoards } from "../src/research-boards.mjs";

test("workspace and conversation persist", async () => {
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"math-lab-")); const workspace=path.join(root,"workspace"); await fs.mkdir(workspace);
  const store=await new Store(path.join(root,"runtime")).load(); const w=await store.addWorkspace({workspacePath:workspace,name:"Test"}); const c=await store.createConversation({workspaceId:w.id}); await store.updateConversation(c.id,row=>row.messages.push({role:"user",content:"hello"}));
  const restored=await new Store(path.join(root,"runtime")).load(); assert.equal(restored.getConversation(c.id).messages[0].content,"hello");
});

test("restart preserves running conversations until the task manager reconciles their backend", async () => {
  const root=await fs.mkdtemp(path.join(os.tmpdir(),"math-lab-recovery-"));
  const store=await new Store(path.join(root,"runtime")).load();
  const conversation=await store.createConversation({workspaceId:null,title:"恢复测试"});
  await store.updateConversation(conversation.id,row=>{row.status="running";row.messages.push({role:"user",content:"继续研究"});});
  const restored=await new Store(path.join(root,"runtime")).load();
  assert.equal(restored.getConversation(conversation.id).status,"running");
  const board={status:"partial_success",revision:8,routes:[],goals:[],claims:[{id:"c1"}],events:[{text:"恢复后读取到终态"}]};
  const researchBoards={byConversation:id=>id===conversation.id?{agent:"mathcat",remoteProjectId:"p1"}:null,viewByConversation:async()=>board};
  const tasks=new TaskManager({store:restored,config:{},capabilities:[],researchBoards});
  await tasks.recoverInterrupted();
  const recovered=restored.getConversation(conversation.id);
  assert.equal(recovered.status,"idle");
  assert.equal(recovered.messages.at(-1).recovered,true);
  assert.match(recovered.messages.at(-1).content,/得到部分成果/);
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
  const mathcat=await boards.create({conversationId:"c1",workspaceId:null,agent:"mathcat",problem:"证明 P",reviewMode:"balanced"}); assert.equal(mathcat.routes.length,2); assert.equal(mathcat.mode,"human_collaboration"); assert.equal(mathcat.reviewMode,"balanced");
  const route=mathcat.routes[0]; await boards.routeCommand(mathcat.id,route.id,"approve"); assert.equal(boards.get(mathcat.id).routes[0].status,"active");
  const withDecision=await boards.addDecision(mathcat.id,{question:"允许加强假设吗？",context:"当前假设不足以闭合归纳步骤",options:[{label:"允许",value:"approve",reason:"先得到条件性结果"},{label:"不允许",value:"reject",description:"寻找不加强假设的路线"}],blockingEntityIds:["route_1"]}); const decision=withDecision.decisions[0]; assert.equal(decision.options[0].description,"先得到条件性结果"); assert.deepEqual(decision.blockingEntityIds,["route_1"]); await boards.answerDecision(mathcat.id,decision.id,"defer","先研究其它路线"); assert.equal(boards.get(mathcat.id).decisions[0].status,"pending"); await boards.answerDecision(mathcat.id,decision.id,"reject","不要改变原命题"); assert.equal(boards.get(mathcat.id).decisions[0].answer,"reject"); assert.equal(boards.get(mathcat.id).decisions[0].note,"不要改变原命题");
  const rethlas=await boards.create({conversationId:"c2",agent:"rethlas",problem:"证明 Q"}); assert.equal(rethlas.mode,"generation_verification"); assert.ok(rethlas.verification);
  const danus=await boards.create({conversationId:"c3",agent:"danus",problem:"证明 R"}); assert.equal(danus.mode,"orchestration_truth"); assert.ok(danus.memories&&danus.facts);
  const restored=await new ResearchBoards(root).load(); assert.equal(restored.byConversation("c1").revision,5);
  await restored.deleteByConversation("c2"); assert.equal(restored.byConversation("c2"),null); assert.ok(restored.byConversation("c1"));
});

test("automatic review chooses a conservative answer for MathCat questions", async () => {
  let answered = false; const answers = [];
  const client = {
    async board(){return {project_id:"p1",status:answered?"running":"needs_human_review",revision:answered?3:2,problem:{original_problem:"P",target_statement:"P"},human_questions:answered?[]:[{question_id:"q1",question:"如何处理有歧义的形式化？",options:[{value:"revise",label:"修改"},{value:"reject",label:"拒绝形式化"}],status:"open"}],routes:[],goals:[],claims:[],failed_routes:[],timeline:[]};},
    async answerQuestion(_projectId,_questionId,_board,answer){answers.push(answer);answered=true;}
  };
  const boards = new ResearchBoards("unused",client); boards.rows=[{id:"rb1",conversationId:"c-auto",agent:"mathcat",reviewMode:"automatic",remoteProjectId:"p1",decisions:[]}];
  const result=await boards.resolveAutomaticReview("c-auto");
  assert.deepEqual(answers,["reject"]);
  assert.equal(result.status,"running");
});

test("MathCat board readers share a snapshot and deletion stops an active remote project first", async () => {
  let reads=0; let stops=0;
  const client={
    async board(){reads+=1;return {project_id:"p-cache",status:"running",revision:2,problem:{original_problem:"P",target_statement:"P"},routes:[],goals:[],claims:[],failed_routes:[],timeline:[]};},
    async stopProject(){stops+=1;}
  };
  const boards=new ResearchBoards("unused",client,{cacheTtlMs:10_000});
  boards.rows=[{id:"rb-cache",conversationId:"c-cache",agent:"mathcat",remoteProjectId:"p-cache",problem:{original:"P"},reviewMode:"strict",decisions:[],events:[]}];
  await Promise.all([boards.viewByConversation("c-cache"),boards.viewByConversation("c-cache")]);
  await boards.viewByConversation("c-cache");
  assert.equal(reads,1);
  boards.save=async()=>{};
  await boards.deleteByConversation("c-cache");
  assert.equal(stops,1);
  assert.equal(boards.byConversation("c-cache"),null);
});

test("MathCat human collaboration mutations refresh revisions and persist board-level choices", async () => {
  let revision=1;
  const calls=[];
  const client={
    async board(){return {project_id:"p-control",status:"running",revision,problem:{original_problem:"P",target_statement:"P"},budget:{max_rounds:12,max_parallel_workers:3,max_minutes_per_task:45,max_model_calls_per_task:4,max_total_model_calls:120},routes:[],goals:[],claims:[],failed_routes:[],timeline:[]};},
    async createAndExecuteRoute(_projectId,board,input){calls.push(["route",board.revision,input.title]);revision+=1;return {execution_started:true,route_id:"route_1",task_id:"task_1"};},
    async addPlanningSuggestion(_projectId,board,input){calls.push(["suggestion",board.revision,input.content]);revision+=1;},
    async goalReview(_projectId,board,input){calls.push(["goal-review",board.revision,input.focus]);revision+=1;return {command_id:"command_goal_review",after_revision:revision};},
    async updateResearchSettings(_projectId,board,input){calls.push(["settings",board.revision,input.maxParallelWorkers,input.maxMinutesPerTask,input.reviewMode]);revision+=1;}
  };
  const boards=new ResearchBoards("unused",client,{cacheTtlMs:10_000});
  boards.rows=[{id:"rb-control",conversationId:"c-control",agent:"mathcat",reviewMode:"strict",remoteProjectId:"p-control",decisions:[],events:[]}];
  boards.save=async()=>{};
  await boards.addRoute("rb-control",{title:"局部化",summary:"局部证明",objective:"证明引理",completionContract:"闭合或报告缺口",executeImmediately:true});
  await boards.addPlanningSuggestion("rb-control",{content:"更多交换代数视角"});
  const reviewed=await boards.forceGoalReview("rb-control",{focus:"重审主目标与窄化目标"});
  assert.equal(reviewed.lastGoalReview.focus,"重审主目标与窄化目标");
  assert.equal(reviewed.lastGoalReview.status,"queued");
  assert.equal(reviewed.planningSuggestions[0].content,"更多交换代数视角");
  assert.equal(reviewed.planningSuggestions[0].effectiveRound,1);
  const updated=await boards.updateSettings("rb-control",{maxParallelWorkers:6,maxMinutesPerTask:90,reviewMode:"balanced"});
  assert.deepEqual(calls,[
    ["route",1,"局部化"],
    ["suggestion",2,"更多交换代数视角"],
    ["goal-review",3,"重审主目标与窄化目标"],
    ["settings",4,6,90,"balanced"]
  ]);
  assert.equal(updated.revision,5);
  assert.equal(updated.reviewMode,"balanced");
  assert.equal(boards.get("rb-control").reviewMode,"balanced");
});
