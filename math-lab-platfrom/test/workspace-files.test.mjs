import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import http from 'node:http';
import {resolveWorkspaceEntry,workspaceDirectoryPage,workspaceFileRoute} from '../src/workspace-files.mjs';

async function fixture(t){
  const base=path.resolve(import.meta.dirname,'../../tests/results');await fs.mkdir(base,{recursive:true});const directory=await fs.mkdtemp(path.join(base,'workspace-link-regression-'));
  t.after(async()=>{const target=path.resolve(directory);if(!target.startsWith(base+path.sep))throw Error('Unsafe cleanup path');await fs.rm(target,{recursive:true,force:true});});
  const workspace={path:path.join(directory,'workspace'),name:'研究 <目录>'};await fs.mkdir(path.join(workspace.path,'成果','v2'),{recursive:true});await fs.writeFile(path.join(workspace.path,'成果','v2','report.md'),'# 原文\n精确数学陈述');await fs.writeFile(path.join(workspace.path,'problem.md'),'只给原题');await fs.writeFile(path.join(directory,'outside.md'),'不属于该对话');return {directory,workspace};
}
test('files resolve relative to workspace, reject traversal and reject external junctions',async t=>{
  const {directory,workspace}=await fixture(t),file=await resolveWorkspaceEntry(workspace,'成果/v2/report.md');assert.equal(await fs.readFile(file.realFile,'utf8'),'# 原文\n精确数学陈述');
  assert.equal((await resolveWorkspaceEntry(workspace,path.join(workspace.path,'problem.md'))).relative,'problem.md');
  await assert.rejects(resolveWorkspaceEntry(workspace,'../outside.md'),e=>e.status===403);await assert.rejects(resolveWorkspaceEntry(workspace,path.join(directory,'outside.md')),e=>e.status===403);await assert.rejects(resolveWorkspaceEntry(workspace,'problem.md:secret'),e=>e.status===400);
  const external=path.join(directory,'external');await fs.mkdir(external);await fs.writeFile(path.join(external,'secret.md'),'outside');await fs.symlink(external,path.join(workspace.path,'escape'),'junction');
  await assert.rejects(resolveWorkspaceEntry(workspace,'escape/secret.md'),e=>e.status===403);await assert.rejects(resolveWorkspaceEntry(workspace,'escape'),e=>e.status===403);
  const root=await resolveWorkspaceEntry(workspace,'.'),html=await workspaceDirectoryPage(workspace,root,'c');assert.doesNotMatch(html,/>escape\//);assert.match(html,/研究 &lt;目录&gt;/);
});
test('directory HTTP response is browsable, child files and original question stay in workspace',async t=>{
  const {workspace}=await fixture(t),revealed=[];const store={getConversation:id=>id==='c'?{id}:null,workspaceFor:()=>workspace};
  const sendJson=(res,status,value)=>{res.writeHead(status,{'content-type':'application/json'});res.end(JSON.stringify(value));};
  const server=http.createServer(async(req,res)=>{try{if(!await workspaceFileRoute({req,res,url:new URL(req.url,'http://localhost'),store,sendJson,userFileHeaders:file=>({'content-type':file.endsWith('.md')?'text/plain; charset=utf-8':'application/octet-stream','x-content-type-options':'nosniff'}),reveal:async file=>revealed.push(file)}))sendJson(res,404,{});}catch(error){sendJson(res,error.status||(error.code==='ENOENT'?404:500),{error:error.message});}});
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));t.after(()=>new Promise(resolve=>server.close(resolve)));const origin='http://127.0.0.1:'+server.address().port;
  const get=relative=>fetch(origin+'/api/workspace-file?conversationId=c&path='+encodeURIComponent(relative));
  const listing=await get('成果/v2');assert.equal(listing.status,200);assert.match(listing.headers.get('content-type'),/text\/html/);const html=await listing.text();assert.match(html,/返回上一级/);assert.match(html,/在资源管理器中打开此目录/);assert.match(html,/report.md/);assert.doesNotMatch(html,/href="file:/);
  const link=html.match(/href="([^\"]+)">report\.md<\/a>/)[1].replaceAll('&amp;','&');assert.equal(await (await fetch(origin+link)).text(),'# 原文\n精确数学陈述');assert.equal(await (await get('problem.md')).text(),'只给原题');assert.equal((await get('../outside.md')).status,403);
  assert.equal(revealed.length,0);const reveal=await fetch(origin+'/api/reveal-file?conversationId=c&path='+encodeURIComponent('成果/v2'));assert.equal(reveal.status,200);assert.equal(revealed.length,1);assert.equal((await get('missing.md')).status,404);
});
