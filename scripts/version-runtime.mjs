import fs from "node:fs/promises";
import net from "node:net";
import path from "node:path";
import { spawn,spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

export const versionRoot=path.resolve(path.dirname(fileURLToPath(import.meta.url)),"..");
export const backendRoot=path.join(versionRoot,"math-research-mvp");
export const platformRoot=path.join(versionRoot,"math-lab-platfrom");
export const runtimeRoot=path.join(versionRoot,"runtime");
export const logRoot=path.join(runtimeRoot,"logs");
let inherited={};
try{inherited=JSON.parse(await fs.readFile(path.join(versionRoot,'update-data.json'),'utf8'));}catch(error){if(error.code!=='ENOENT')throw error;}
export const dataRoot=inherited.workspacesRoot||path.join(versionRoot,"workspaces");
export const databasePath=inherited.databasePath||path.join(runtimeRoot,'state_v2.sqlite');
export const platformDataRoot=inherited.platformDataRoot||path.join(platformRoot,'runtime-data');
export const tokenFile=path.join(runtimeRoot,"api-token");
export const backendUrl="http://127.0.0.1:8900";
export const platformUrl="http://127.0.0.1:4335";
export const expectedVersion="2.5.3";

export async function health(url,token="") {
  try { const response=await fetch(url,{headers:token?{authorization:`Bearer ${token}`}:{},signal:AbortSignal.timeout(2000)});return response.ok?await response.json():null; }
  catch{return null;}
}
export async function assertPortFree(port){await new Promise((resolve,reject)=>{const server=net.createServer();server.once("error",()=>reject(new Error(`端口 ${port} 已被占用；没有停止任何现有进程。`)));server.listen(port,"127.0.0.1",()=>server.close(resolve));});}
export function run(command,args,options={}){const result=spawnSync(command,args,{stdio:"inherit",shell:false,...options});if(result.error)throw result.error;if(result.status!==0)throw new Error(`${command} 执行失败（退出码 ${result.status}）`);}
export async function waitFor(url,child,label){for(let i=0;i<60;i++){if(child.exitCode!==null)throw new Error(`${label} 在启动期间退出，请检查 ${logRoot}`);const value=await health(url);if(value)return value;await new Promise(resolve=>setTimeout(resolve,300));}throw new Error(`${label} 启动超时，请检查 ${logRoot}`);}
export async function readPid(name){try{const value=Number.parseInt((await fs.readFile(path.join(runtimeRoot,name),"utf8")).trim(),10);return Number.isSafeInteger(value)&&value>0?value:null;}catch{return null;}}
export async function unixCommand(pid){if(process.platform==="win32")return "";const result=spawnSync("ps",["-p",String(pid),"-o","command="],{encoding:"utf8",shell:false});return result.status===0?result.stdout.trim():"";}
export async function ownedUnixProcess(pid,name){const command=await unixCommand(pid);if(!command)return false;const expected=name==="research.pid"?path.join(backendRoot,"target","release","mathcat-v2"):path.join(platformRoot,"src","server.mjs");return command.includes(expected);}
