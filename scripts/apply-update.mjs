import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {runCommand} from '../math-lab-platfrom/src/product-update.mjs';
export async function applyUpdate({root,destination,statusFile,launcher=defaultLauncher,waitStopped=waitForStop,readHealth=async()=>{const response=await fetch('http://127.0.0.1:4335/api/health',{signal:AbortSignal.timeout(5000)});if(!response.ok)throw new Error('无法读取新版状态。');return response.json();}}){
if(!root||!destination||path.dirname(root)!==path.dirname(destination)||path.resolve(statusFile)!==path.join(root,'runtime','updates','status.json'))throw new Error('更新交接路径无效。');
const job=JSON.parse(await fs.readFile(statusFile,'utf8'));
if(job.destination!==destination||job.state!=='restarting')throw new Error('更新交接状态无效。');
const write=async(state,error)=>{const temp=statusFile+'.helper.tmp';await fs.writeFile(temp,JSON.stringify({...job,state,error}));await fs.rename(temp,statusFile);};
try{
  await launcher(root,'stop');
  await waitStopped();
  await launcher(destination,'start');
  const health=await readHealth();if(health.version!==job.targetVersion)throw new Error('新版启动后返回的版本不匹配。');
  await fs.writeFile(path.join(root,'runtime','updates','installed.json'),JSON.stringify({destination,version:job.targetVersion}));
  await write('completed');
  return true;
}catch(error){await write('failed',`新版未成功启动：${error.message}。原数据保留，请查看更新日志并使用版本启动器重试。`);return false;}
}
const defaultLauncher=(directory,kind)=>process.platform==='win32'?runCommand('powershell.exe',['-NoProfile','-ExecutionPolicy','Bypass','-File',path.join(directory,'scripts',`${kind}-version.ps1`),...(kind==='start'?['-NoBrowser']:[])]):runCommand(process.execPath,[path.join(directory,'scripts',`${kind}-version.mjs`),...(kind==='start'?['--no-browser']:[])]);
async function waitForStop(){
  const until=Date.now()+20000;
  while(Date.now()<until){const states=await Promise.all([4335,8900].map(async port=>{try{await fetch(`http://127.0.0.1:${port}/${port===4335?'api/health':'health'}`,{signal:AbortSignal.timeout(1000)});return true;}catch{return false;}}));if(states.every(value=>!value))return;await new Promise(resolve=>setTimeout(resolve,400));}
  throw new Error('旧服务尚未退出，未启动新版。');
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)){
  const [root,destination,statusFile]=process.argv.slice(2);if(!await applyUpdate({root,destination,statusFile}))process.exitCode=1;
}
