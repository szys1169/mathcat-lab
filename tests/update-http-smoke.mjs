import fs from 'node:fs/promises';
import path from 'node:path';
import http from 'node:http';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
const root=path.resolve(import.meta.dirname,'..'),resultRoot=path.join(root,'tests','results','update-http-'+Date.now());await fs.mkdir(resultRoot,{recursive:true});
const tokenFile=path.join(resultRoot,'api-token');await fs.writeFile(tokenFile,'isolated-update-http-fixture-token');
const port=async()=>{const server=http.createServer();server.listen(0,'127.0.0.1');await once(server,'listening');const value=server.address().port;await new Promise(resolve=>server.close(resolve));return value;};
const backendPort=await port(),webPort=await port(),backend=`http://127.0.0.1:${backendPort}`,web=`http://127.0.0.1:${webPort}`;
const children=[],logs=[];const launch=async(command,args,options={})=>{const output=await fs.open(path.join(resultRoot,`${children.length}.log`),'w');logs.push(output);const child=spawn(command,args,{shell:false,windowsHide:true,stdio:['ignore',output.fd,output.fd],...options});children.push(child);await once(child,'spawn');return child;};
async function wait(url){for(let i=0;i<100;i++){try{const response=await fetch(url);if(response.ok)return response.json();}catch{}await new Promise(resolve=>setTimeout(resolve,150));}throw new Error('HTTP startup timeout');}
const receipt={version:'2.5.3',real_model_calls:false,checks:[]};
try{
  await launch(path.join(root,'math-research-mvp/target/release',process.platform==='win32'?'mathcat-v2.exe':'mathcat-v2'),['--bind',`127.0.0.1:${backendPort}`,'--database',path.join(resultRoot,'state.sqlite'),'--data-root',path.join(resultRoot,'workspaces'),'--token-file',tokenFile]);
  assert.equal((await wait(backend+'/health')).version,'2.5.3');
  await launch(process.execPath,[path.join(root,'math-lab-platfrom/src/server.mjs')],{cwd:path.join(root,'math-lab-platfrom'),env:{...process.env,MATH_LAB_PORT:String(webPort),MATH_LAB_RUNTIME_ROOT:path.join(resultRoot,'platform'),MATHCAT_V2_API_URL:backend,MATHCAT_V2_TOKEN_FILE:tokenFile}});
  assert.equal((await wait(web+'/api/health')).version,'2.5.3');receipt.checks.push('real backend and platform version');
  const page=await (await fetch(web)).text();assert.match(page,/id="updateButton"/);receipt.checks.push('update button served');
  const status=await (await fetch(web+'/api/product-update/status')).json();assert.equal(status.state,'idle');receipt.checks.push('idle update status');
  assert.equal((await fetch(web+'/api/product-update/check',{headers:{Origin:'https://example.invalid'}})).status,403);receipt.checks.push('cross-origin rejected');
  assert.equal((await fetch(web+'/api/product-update/install',{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'})).status,403);receipt.checks.push('install requires UI header');
  const install=await fetch(web+'/api/product-update/install',{method:'POST',headers:{'Content-Type':'application/json','X-MathCat-Update':'1'},body:JSON.stringify({version:'2.5.3'})});assert.equal(install.status,409);assert.match((await install.json()).error,/启动器/);receipt.checks.push('unmanaged test server cannot stop a live installation');
  const projects=await (await fetch(web+'/api/v2/research/projects')).json();assert.deepEqual(projects.projects,[]);receipt.checks.push('no research or model calls created');receipt.passed=true;
}catch(error){receipt.passed=false;receipt.error=error.stack;process.exitCode=1;}
finally{for(const child of children.reverse()){if(child.exitCode===null){child.kill();await Promise.race([once(child,'exit'),new Promise(resolve=>setTimeout(resolve,4000))]);}}for(const file of logs)await file.close();await fs.writeFile(path.join(resultRoot,'receipt.json'),JSON.stringify(receipt,null,2));console.log(JSON.stringify({resultRoot,...receipt}));}
