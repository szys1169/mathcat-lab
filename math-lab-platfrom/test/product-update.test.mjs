import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {ProductUpdater,selectRelease,compareVersions,validateArchiveEntries,runCommand} from '../src/product-update.mjs';
import {versionMessage} from '../public/product-update.js';
import {applyUpdate} from '../../scripts/apply-update.mjs';
const asset=(version='2.5.3')=>({name:`MathCat-Lab-${version}-Windows-x64.zip`,browser_download_url:`https://github.com/szys1169/mathcat-lab/releases/download/v${version}/MathCat-Lab-${version}-Windows-x64.zip`,digest:'sha256:'+'a'.repeat(64),size:100});
const releases=(version='2.5.3')=>[{tag_name:'v'+version,prerelease:true,assets:[asset(version)]}];
test('numeric versions, prereleases, equality and no downgrade',()=>{
  assert.equal(compareVersions('2.10.0','2.9.9'),1);
  assert.equal(selectRelease(releases(),'2.5.2','win32','x64').canUpdate,true);
  assert.equal(selectRelease(releases(),'2.5.3','win32','x64').state,'current');
  assert.equal(selectRelease(releases(),'2.5.4','win32','x64').canUpdate,false);
  assert.equal(selectRelease([...releases(),{tag_name:'v9.0.0',draft:true}],'2.5.2','win32','x64').latestVersion,'2.5.3');
  assert.equal(versionMessage({state:'current'}),'已经是最新版本');
});
test('unsupported system, asset URL or missing digest cannot install',()=>{
  assert.equal(selectRelease(releases(),'2.5.2','linux','x64').canUpdate,false);
  for(const patch of [{digest:null},{browser_download_url:'https://evil.test/download'}]){const list=releases();Object.assign(list[0].assets[0],patch);assert.equal(selectRelease(list,'2.5.2','win32','x64').canUpdate,false);}
  assert.throws(()=>validateArchiveEntries('ok\n../outside'));assert.throws(()=>validateArchiveEntries('C:\\outside'));assert.throws(()=>validateArchiveEntries('/etc/file'));
});
test('network error keeps local version and does not claim latest',async()=>{
  const updater=new ProductUpdater({root:os.tmpdir(),localVersion:'2.5.2',fetchImpl:async()=>{throw new Error('offline');}});
  const result=await updater.check();assert.equal(result.localVersion,'2.5.2');assert.equal(result.state,'error');assert.equal(result.latestVersion,null);
});
test('active research prevents any download or write',async()=>{
  const updater=new ProductUpdater({root:os.tmpdir(),localVersion:'2.5.2',assertIdle:async()=>{throw new Error('busy');},fetchImpl:()=>assert.fail('must not fetch')});
  await assert.rejects(updater.start('2.5.3'),/busy/);assert.equal(updater.busy,false);
});
async function fixture(t,{badDigest=false}={}){
  const base=await fs.mkdtemp(path.join(os.tmpdir(),'mathcat-updater-'));t.after(()=>fs.rm(base,{recursive:true,force:true}));
  const root=path.join(base,'MathCat-Lab-2.5.2'),source=path.join(base,'package');
  for(const dir of [root,source,path.join(source,'math-lab-platfrom'),path.join(source,'math-research-mvp/target/release')])await fs.mkdir(dir,{recursive:true});
  await fs.writeFile(path.join(source,'VERSION'),'2.5.3');await fs.writeFile(path.join(source,'release.json'),JSON.stringify({product:'MathCat Lab',version:'2.5.3',updaterProtocol:1}));await fs.writeFile(path.join(source,'math-research-mvp/target/release/mathcat-v2.exe'),'fixture only');
  const archive=path.join(base,'package.tar');await runCommand('tar',['-cf',archive,'-C',source,'.']);const bytes=await fs.readFile(archive);const list=releases();Object.assign(list[0].assets[0],{size:bytes.length,digest:'sha256:'+(badDigest?'b'.repeat(64):createHash('sha256').update(bytes).digest('hex'))});
  const tokenFile=path.join(root,'test-token');await fs.writeFile(tokenFile,'fixture-token');await fs.writeFile(path.join(root,'research.txt'),'original research');
  let activated=null,downloadCount=0;
  const updater=new ProductUpdater({root,localVersion:'2.5.2',platform:'win32',arch:'x64',dataPaths:{tokenFile,databasePath:path.join(root,'state.sqlite'),platformDataRoot:path.join(root,'platform-data'),workspacesRoot:root},fetchImpl:async url=>{if(url.includes('api.github.com'))return Response.json(list);downloadCount++;return new Response(bytes);},prepare:async(command,args,options)=>command==='npm.cmd'?'':runCommand(command,args,options),activate:async value=>{activated=value;}});
  return {updater,root,base,get activated(){return activated;},get downloadCount(){return downloadCount;}};
}
test('downloads, verifies, extracts and installs beside original; repeat click stays single-flight',async t=>{
  const f=await fixture(t);await f.updater.start('2.5.3');await f.updater.start('2.5.3');await f.updater.work;
  assert.equal(f.updater.job.state,'restarting',JSON.stringify(f.updater.job));assert.equal(f.downloadCount,1);assert.ok(f.activated);
  assert.equal(await fs.readFile(path.join(f.root,'research.txt'),'utf8'),'original research');
  const installed=path.join(f.base,'MathCat-Lab-2.5.3');const data=JSON.parse(await fs.readFile(path.join(installed,'update-data.json')));assert.equal(data.workspacesRoot,f.root);assert.equal(await fs.readFile(path.join(installed,'runtime/api-token'),'utf8'),'fixture-token');
});
test('digest mismatch never installs or activates',async t=>{const f=await fixture(t,{badDigest:true});await f.updater.start('2.5.3');await f.updater.work;assert.equal(f.updater.job.state,'failed');assert.match(f.updater.job.error,/完整性/);assert.equal(f.activated,null);await assert.rejects(fs.access(path.join(f.base,'MathCat-Lab-2.5.3')));});
test('existing version directory is preserved',async t=>{const f=await fixture(t);await fs.mkdir(path.join(f.base,'MathCat-Lab-2.5.3'));await f.updater.start('2.5.3');await f.updater.work;assert.equal(f.updater.job.state,'failed');assert.equal(f.downloadCount,0);});
test('restart handoff checks health before forwarding the original launcher',async t=>{
  const f=await fixture(t);await f.updater.start('2.5.3');await f.updater.work;const order=[];
  assert.equal(await applyUpdate({root:f.root,...f.activated,launcher:async(root,action)=>order.push([path.basename(root),action]),waitStopped:async()=>order.push('ports-free'),readHealth:async()=>({version:'2.5.3'})}),true);
  assert.deepEqual(order,[['MathCat-Lab-2.5.2','stop'],'ports-free',['MathCat-Lab-2.5.3','start']]);
  assert.equal(JSON.parse(await fs.readFile(path.join(f.root,'runtime/updates/installed.json'))).version,'2.5.3');
});
test('restart failure remains failed and never rewrites the launch target',async t=>{
  const f=await fixture(t);await f.updater.start('2.5.3');await f.updater.work;
  assert.equal(await applyUpdate({root:f.root,...f.activated,launcher:async()=>{},waitStopped:async()=>{},readHealth:async()=>({version:'2.5.2'})}),false);
  assert.equal((await f.updater.status()).state,'failed');assert.equal(f.updater.busy,false);await assert.rejects(fs.access(path.join(f.root,'runtime/updates/installed.json')));
});
