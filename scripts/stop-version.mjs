#!/usr/bin/env node
import fs from "node:fs/promises";
import path from "node:path";
import { randomUUID } from "node:crypto";
import {forwardUpdate} from './forward-update.mjs';
await forwardUpdate(path.resolve(import.meta.dirname,'..'),'stop',process.argv.slice(2));
import { runtimeRoot,tokenFile,backendUrl,expectedVersion,readPid,ownedUnixProcess,health } from "./version-runtime.mjs";

if(process.argv.includes("--help")){console.log("用法: node scripts/stop-version.mjs");process.exit(0);}
if(process.platform==="win32")throw new Error("Windows 请使用 scripts/stop-version.ps1，以保留严格的进程归属检查。");
let token="";try{token=(await fs.readFile(tokenFile,"utf8")).trim();}catch{}
const request=async(url,options={})=>{const response=await fetch(url,{...options,headers:{authorization:`Bearer ${token}`,...options.headers},signal:AbortSignal.timeout(10_000)});if(!response.ok)throw new Error(`${response.status} ${await response.text()}`);return response.json();};
const researchPid=await readPid("research.pid");
const researchOwned=researchPid?await ownedUnixProcess(researchPid,"research.pid"):false;
const backendHealth=await health(`${backendUrl}/health`);
if(researchOwned&&!backendHealth)throw new Error("无法连接仍在运行的研究服务，不能确认取消状态；服务保持运行。");
if(backendHealth&&backendHealth.version!==expectedVersion)throw new Error("研究端口属于其他版本；没有停止任何进程。");
if(backendHealth){
  if(!token)throw new Error("服务令牌缺失，无法确认取消研究；服务保持运行。");
  let cursor=null,projects=[];do{const page=await request(`${backendUrl}/api/v2/research/projects?limit=100${cursor?`&cursor=${encodeURIComponent(cursor)}`:""}`);projects.push(...(page.projects||[]));cursor=page.next_cursor;}while(cursor);
  for(const project of projects){for(const run of project.runs||[])if(run.state!=="ended")await request(`${backendUrl}/api/v2/research/projects/${project.id}/commands`,{method:"POST",headers:{"content-type":"application/json","idempotency-key":randomUUID()},body:JSON.stringify({type:"stop_run",run_id:run.id,target:{kind:"run",id:run.id},apply_at:"immediate",payload:{reason:`User stopped MathCat Lab ${expectedVersion}`}})});for(const interaction of project.interactions||[])if(interaction.state!=="ended")await request(`${backendUrl}/api/v2/research/projects/${project.id}/interaction-executions/${interaction.id}/cancel`,{method:"POST",headers:{"content-type":"application/json","idempotency-key":randomUUID()},body:JSON.stringify({expected_revision:interaction.revision})});}
  const deadline=Date.now()+45_000;
  for(;;){
    let pending=false,cursor2=null;
    do{const page=await request(`${backendUrl}/api/v2/research/projects?limit=100${cursor2?`&cursor=${encodeURIComponent(cursor2)}`:""}`);for(const project of page.projects||[]){if((project.runs||[]).some(row=>row.state!=="ended")||(project.interactions||[]).some(row=>row.state!=="ended")||(project.sessions||[]).some(row=>row.state!=="closed")||(project.usage||[]).some(row=>["reserved","running"].includes(row.state)||(row.state==="unknown"&&!row.ended_at)))pending=true;}cursor2=page.next_cursor;}while(cursor2);
    if(!pending)break;if(Date.now()>=deadline)throw new Error("仍有执行尚未安全收束；服务保持运行，请查看白板和日志。");await new Promise(resolve=>setTimeout(resolve,500));
  }
}
for(const name of ["platform.pid","research.pid"]){const pid=await readPid(name);if(!pid)continue;if(!await ownedUnixProcess(pid,name)){console.warn(`PID ${pid} 不属于此版本或已结束，未处理。`);continue;}process.kill(pid,"SIGTERM");await fs.rm(path.join(runtimeRoot,name),{force:true});}
console.log(`MathCat Lab ${expectedVersion} 已请求停止；没有删除用户文件。`);
