#!/usr/bin/env node
import fs from "node:fs";
import fsp from "node:fs/promises";
import path from "node:path";
import { randomBytes } from "node:crypto";
import { spawn } from "node:child_process";
import { backendRoot,platformRoot,runtimeRoot,logRoot,dataRoot,tokenFile,backendUrl,platformUrl,expectedVersion,health,assertPortFree,run,waitFor } from "./version-runtime.mjs";

const flags=new Set(process.argv.slice(2));
if(flags.has("--help")){console.log("用法: node scripts/start-version.mjs [--no-browser] [--rebuild]");process.exit(0);}
if(Number(process.versions.node.split(".")[0])<22)throw new Error("需要 Node.js 22 或更高版本。");
await Promise.all([runtimeRoot,logRoot,dataRoot].map(directory=>fsp.mkdir(directory,{recursive:true})));
const codex=process.env.CODEX_BIN||"codex";
const codexCheck=await new Promise(resolve=>{const child=spawn(codex,["--version"],{stdio:"ignore",shell:false});child.once("error",()=>resolve(false));child.once("exit",code=>resolve(code===0));});
if(!codexCheck)throw new Error("找不到可用的 Codex CLI。请先安装、登录，或设置 CODEX_BIN。");
if(!fs.existsSync(path.join(platformRoot,"node_modules","katex")))run(process.platform==="win32"?"npm.cmd":"npm",["ci","--ignore-scripts"],{cwd:platformRoot});
let token="";try{token=(await fsp.readFile(tokenFile,"utf8")).trim();}catch{}
if(!token){token=randomBytes(32).toString("base64url");await fsp.writeFile(tokenFile,token+"\n",{mode:0o600,flag:"wx"});}

let backendHealth=await health(`${backendUrl}/health`);
if(backendHealth&&backendHealth.version!==expectedVersion)throw new Error("研究端口属于其他版本；没有更改现有进程。");
if(!backendHealth){
  await assertPortFree(8900);
  const backendBin=path.join(backendRoot,"target","release",process.platform==="win32"?"mathcat-v2.exe":"mathcat-v2");
  if(flags.has("--rebuild")||!fs.existsSync(backendBin))run("cargo",["build","--release","--bin","mathcat-v2"],{cwd:backendRoot});
  const out=fs.openSync(path.join(logRoot,"research.stdout.log"),"a"),err=fs.openSync(path.join(logRoot,"research.stderr.log"),"a");
  const child=spawn(backendBin,["--bind","127.0.0.1:8900","--database",path.join(runtimeRoot,"state_v2.sqlite"),"--data-root",dataRoot,"--token-file",tokenFile,"--codex-command",codex],{cwd:backendRoot,detached:true,stdio:["ignore",out,err],shell:false});
  fs.closeSync(out);fs.closeSync(err);
  await fsp.writeFile(path.join(runtimeRoot,"research.pid"),String(child.pid));backendHealth=await waitFor(`${backendUrl}/health`,child,"研究服务");child.unref();
  if(backendHealth.version!==expectedVersion)throw new Error("新研究服务报告了错误版本，请重新构建。");
}
let platformHealth=await health(`${platformUrl}/api/health`);
if(platformHealth&&platformHealth.version!==expectedVersion)throw new Error("平台端口属于其他版本；没有更改现有进程。");
if(!platformHealth){
  await assertPortFree(4335);
  const out=fs.openSync(path.join(logRoot,"platform.stdout.log"),"a"),err=fs.openSync(path.join(logRoot,"platform.stderr.log"),"a");
  const child=spawn(process.execPath,[path.join(platformRoot,"src","server.mjs")],{cwd:platformRoot,detached:true,stdio:["ignore",out,err],shell:false,env:{...process.env,CODEX_BIN:codex,MATH_LAB_PORT:"4335",MATH_LAB_RUNTIME_ROOT:path.join(platformRoot,"runtime-data"),MATHCAT_V2_API_URL:backendUrl,MATHCAT_V2_TOKEN_FILE:tokenFile}});
  fs.closeSync(out);fs.closeSync(err);
  await fsp.writeFile(path.join(runtimeRoot,"platform.pid"),String(child.pid));platformHealth=await waitFor(`${platformUrl}/api/health`,child,"平台服务");child.unref();
  if(platformHealth.version!==expectedVersion)throw new Error("新平台服务报告了错误版本。");
}
console.log(`MathCat Lab ${expectedVersion}: ${platformUrl}`);
if(!flags.has("--no-browser")){
  const [command,args]=process.platform==="darwin"?["open",[platformUrl]]:process.platform==="win32"?["cmd.exe",["/d","/s","/c","start","",platformUrl]]:["xdg-open",[platformUrl]];
  const opener=spawn(command,args,{detached:true,stdio:"ignore",shell:false});opener.unref();
}
