import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import crypto from 'node:crypto';
import {execute,search} from '../capabilities/mathcat-research/scripts/research-tools.mjs';
const root=path.resolve(import.meta.dirname,'results');
async function setup(){await fs.mkdir(root,{recursive:true});const cwd=await fs.mkdtemp(path.join(root,'tools-'));const bytes=Buffer.from('精确的证明材料');const evidence=path.join(cwd,'evidence');await fs.mkdir(evidence);const source=path.join(evidence,'proof');await fs.writeFile(source,bytes);const ctx={deadline_at:new Date(Date.now()+60000).toISOString(),problem_version:1,records:[{id:'f',claim:'紧致性引理',assurance:'model_reviewed',validity:'current'}],artifact_root:evidence,artifacts:[{id:'a',path:source,sha256:crypto.createHash('sha256').update(bytes).digest('hex')}]};await fs.writeFile(path.join(cwd,'.mathcat-context.json'),JSON.stringify(ctx));return {cwd,ctx,source};}
test('Chinese mathematical memory is searchable without spaces',()=>assert.equal(search([{id:'x',claim:'紧致空间存在有限子覆盖'}],'有限覆盖')[0].id,'x'));
test('search excerpts cannot duplicate a large source or inherit mathematical assurance',()=>{
  const text='For each parameter, '.repeat(100000);
  const result=search([{id:'source',body:text,proof:text,claim:text,nested:{raw:text},assurance:'model_reviewed',validity:'current',statement_id:'exact'}],'parameter')[0];
  assert(JSON.stringify(result).length<4000);assert.equal(result.body,undefined);assert.equal(result.proof,undefined);assert.equal(result.nested,undefined);assert.equal(result.assurance,undefined);
  assert.equal(result.source_assurance,'model_reviewed');assert.equal(result.statement_id,'exact');assert.equal(result.exact,false);
});
test('context summary exposes bounded discovery without exact source payloads',async()=>{
  const {cwd,ctx}=await setup();ctx.statements=Array.from({length:40},(_,i)=>({id:'s'+i,text:'precise theorem '.repeat(10000)}));
  await fs.writeFile(path.join(cwd,'.mathcat-context.json'),JSON.stringify(ctx));
  const result=await execute('context-summary',{},{cwd});assert.equal(result.lists.statements.count,40);assert.equal(result.lists.statements.entries.length,12);assert.equal(result.lists.statements.truncated,true);assert(!JSON.stringify(result).includes('precise theorem'));assert(JSON.stringify(result).length<10000);
});
test('evidence is read by ID and verified, never arbitrary path',async()=>{const s=await setup();assert.equal((await execute('read-evidence',{id:'a'},{cwd:s.cwd})).text,'精确的证明材料');await assert.rejects(execute('read-evidence',{id:'unknown'},{cwd:s.cwd}));await fs.writeFile(s.source,'changed');await assert.rejects(execute('read-evidence',{id:'a'},{cwd:s.cwd}),/hash mismatch/);});
test('review records cannot select arbitrary filenames',async()=>{const {cwd}=await setup();await assert.rejects(execute('memory-append',{channel:'../outside',record:{}},{cwd}));await execute('memory-append',{channel:'gaps',record:{}},{cwd}).then(()=>assert.fail(),()=>{});await execute('memory-append',{channel:'statement_checks',record:{location:'L1',issue:'missing hypothesis'}},{cwd});assert.equal((await execute('memory-query',{channel:'statement_checks'},{cwd})).records.length,1);});
test('external retrieval failure is inconclusive, not a mathematical rejection',async()=>{const {cwd}=await setup();const result=await execute('search-arxiv-theorems',{query:'necessary statement'},{cwd,fetch:async()=>{throw new Error('network unavailable');}});assert.equal(result.status,'inconclusive');assert.deepEqual(result.results,[]);});
test('deadline prevents starting a tool and no compute operation exists',async()=>{const {cwd,ctx}=await setup();await assert.rejects(execute('compute',{},{cwd}),/Unknown operation/);ctx.deadline_at='2000-01-01T00:00:00Z';await fs.writeFile(path.join(cwd,'.mathcat-context.json'),JSON.stringify(ctx));await assert.rejects(execute('search-memory',{query:'x'},{cwd}),/deadline/);});
test('fact and experience retrieval keep withdrawn and unfinished work separate',async()=>{
  const {cwd,ctx}=await setup();
  ctx.fact_records=[{id:'current',claim:'紧致性',validity:'current',assurance:'model_reviewed'}];
  ctx.experience_records=[...ctx.fact_records,{id:'withdrawn',claim:'紧致性失败尝试',validity:'challenged'},{id:'unfinished',text:'紧致性证明尚未完成',status:'unresolved'}];
  ctx.deadline_at=null;
  await fs.writeFile(path.join(cwd,'.mathcat-context.json'),JSON.stringify(ctx));
  assert.deepEqual((await execute('search-facts',{query:'紧致'},{cwd})).records.map(r=>r.id),['current']);
  const histories=(await execute('search-experiences',{query:'紧致'},{cwd})).records.map(r=>r.id);
  assert.ok(histories.includes('withdrawn'));assert.ok(histories.includes('unfinished'));
  assert.equal((await execute('search-memory',{query:'紧致'},{cwd})).mode,'experiences');
});
test('exact statements preserve quantifiers and constant dependence with untrusted local receipts',async()=>{
  const {cwd,ctx}=await setup();const text='For every fixed a, there exists C(a). This does not claim a uniform C.';
  ctx.session_id='author';ctx.invocation_id='invocation';ctx.statements=[{id:'s',revision:3,text,sha256:crypto.createHash('sha256').update(text).digest('hex'),assurance:'model_reviewed',validity:'current'}];
  await fs.writeFile(path.join(cwd,'.mathcat-context.json'),JSON.stringify(ctx));
  const result=await execute('read-statement',{id:'s',revision:3,length:64000},{cwd});
  assert.equal(result.text,text);assert.equal(result.complete,true);
  const journal=JSON.parse((await fs.readFile(path.join(cwd,'.mathcat-evidence-reads.jsonl'),'utf8')).trim());
  assert.equal(journal.trust,'untrusted_local_read_hint');assert.equal(journal.session_id,'author');
  assert.equal(journal.host_verified,undefined);
  await assert.rejects(execute('read-statement',{id:'s',revision:2},{cwd}),/revision mismatch/);
  const partial=await execute('read-statement',{id:'s',length:7},{cwd});assert.equal(partial.complete,false);assert.equal(partial.next_offset,7);
  ctx.statements[0].text='A stronger statement';await fs.writeFile(path.join(cwd,'.mathcat-context.json'),JSON.stringify(ctx));
  await assert.rejects(execute('read-statement',{id:'s'},{cwd}),/hash mismatch/);
});
test('exact reader rejects malformed ranges and nontext evidence instead of silently changing source',async()=>{
  const {cwd,ctx,source}=await setup();
  await assert.rejects(execute('read-evidence',{id:'a',offset:-1},{cwd}),/integers/);
  const bytes=Buffer.from([0xff,0xfe]);await fs.writeFile(source,bytes);ctx.artifacts[0].sha256=crypto.createHash('sha256').update(bytes).digest('hex');await fs.writeFile(path.join(cwd,'.mathcat-context.json'),JSON.stringify(ctx));
  await assert.rejects(execute('read-evidence',{id:'a'},{cwd}),/encoded data|encoding/i);
});
