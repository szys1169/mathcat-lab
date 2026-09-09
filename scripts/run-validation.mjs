import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {spawn} from 'node:child_process';

const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const stamp=new Date().toISOString().replaceAll(':','-').replaceAll('.','-');
const results=path.join(root,'tests','results',`offline-${stamp}`);
await fs.mkdir(results,{recursive:true});
const rust=path.join(root,'math-research-mvp');
const isolationAvailable=await fs.access(path.join(root,'docs/baselines/2.4.3-source.json')).then(()=>fs.access(path.join(root,'..','MathCat-Lab-2.4.3'))).then(()=>true).catch(()=>false);
const jobs=[
  ['proof-tests',process.execPath,['--test','test/proof-outline.test.mjs','test/proof-graph.test.mjs','test/proof-material.test.mjs'],path.join(root,'math-lab-platfrom')],
  ['update-tests',process.execPath,['--test','test/product-update.test.mjs','test/research-deletion-guard.test.mjs','../tests/cross-platform-runtime.test.mjs'],path.join(root,'math-lab-platfrom')],
  ['update-http',process.execPath,['tests/update-http-smoke.mjs'],root],
  ['rust-format','cargo',['fmt','--all','--','--check'],rust],
  ['rust-clippy','cargo',['clippy','--workspace','--all-targets','--','-D','warnings'],rust],
  ['rust-tests','cargo',['test','--workspace'],rust],
  ['rust-release','cargo',['build','--release'],rust],
  ['platform-tests',process.execPath,['--test','--test-isolation=none'],path.join(root,'math-lab-platfrom')],
  ...(process.platform==='win32'?[['launcher-tests','powershell.exe',['-NoProfile','-ExecutionPolicy','Bypass','-File',path.join(root,'tests','launchers.test.ps1')],root]]:[]),
  ['previous-version-isolation',process.execPath,['scripts/audit-previous-version.mjs'],root],
  ['frontend-isolation',process.execPath,['scripts/audit-frontend.mjs'],root],
  ['research-tools',process.execPath,['--test','tests/research-tools.test.mjs'],root],
];
const report={version:'2.5.3',kind:'offline-engineering',real_model_calls:false,started_at:new Date().toISOString(),build_environment:Object.fromEntries(['CARGO_INCREMENTAL','CARGO_PROFILE_DEV_DEBUG','CARGO_PROFILE_TEST_DEBUG'].map(key=>[key,process.env[key]??'toolchain_default'])),results:[]};
const selected=process.argv.find(arg=>arg.startsWith('--only='))?.slice(7).split(',');
if(selected?.some(name=>!jobs.some(job=>job[0]===name)))throw new Error('Unknown validation job; no passing empty receipt is allowed.');
report.scope=selected?'selected':process.argv.includes('--rust-only')?'rust-only':'full-engineering';
report.complete_suite=report.scope==='full-engineering'&&isolationAvailable;
report.unavailable_checks=isolationAvailable?[]:['historical source isolation requires the local archived baseline'];
for(const [name,bin,args,cwd] of jobs){
  if(selected&&!selected.includes(name))continue;
  if(process.argv.includes('--rust-only')&&!name.startsWith('rust-'))continue;
  const started=Date.now();
  const log=path.join(results,`${name}.log`);
  const handle=await fs.open(log,'wx');
  console.log(JSON.stringify({event:'started',name,log}));
  const exitCode=await new Promise(resolve=>{
    const child=spawn(bin,args,{cwd,shell:false,windowsHide:true,stdio:['ignore',handle.fd,handle.fd]});
    child.on('error',error=>{console.error(`${name}: ${error.message}`);resolve(-1);});
    child.on('exit',code=>resolve(code??-1));
  });
  await handle.close();
  const item={name,command:[bin,...args],exit_code:exitCode,elapsed_ms:Date.now()-started,log:path.relative(root,log).replaceAll('\\','/')};
  report.results.push(item);
  await fs.writeFile(path.join(results,'receipt.json'),JSON.stringify(report,null,2));
  console.log(JSON.stringify({event:'finished',...item}));
}
report.finished_at=new Date().toISOString();
report.passed=report.results.length>0&&report.results.every(r=>r.exit_code===0);
await fs.writeFile(path.join(results,'receipt.json'),JSON.stringify(report,null,2));
console.log(JSON.stringify({passed:report.passed,receipt:path.join(results,'receipt.json')}));
if(!report.passed)process.exitCode=1;
