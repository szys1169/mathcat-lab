import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import path from 'node:path';
import {Store} from '../src/store.mjs';
import {storeResearchMaterial} from '../src/research-materials.mjs';
import {ResearchV2Bridge} from '../src/research-v2-bridge.mjs';

test('import preserves explicit input role and does not guess that every attachment is the problem',async()=>{
  const base=fileURLToPath(new URL('../../tests/results/material-role/',import.meta.url));
  await fs.mkdir(base,{recursive:true});
  const runtimeRoot=await fs.mkdtemp(path.join(base,'fixture-'));
  const store=await new Store(runtimeRoot).load();
  const conversation=await store.createConversation({title:'Short request with attachments'});
  for(const materialRole of [undefined,'problem_statement','reference']){
    const material=await storeResearchMaterial({runtimeRoot,conversationId:conversation.id,name:'input.md',contentBase64:Buffer.from('Exact input, never a reviewed lemma.').toString('base64'),materialRole});
    await store.updateConversation(conversation.id,row=>{(row.researchMaterials??=[]).push(material);});
  }
  const requests=[];
  const bridge=new ResearchV2Bridge({store,client:{request:async(url,options)=>{requests.push(options.body);return {artifact:{id:'artifact-'+requests.length}};}}});
  await bridge.importMaterials(conversation,'project');
  assert.deepEqual(requests.map(r=>r.material_role),['unclassified_input','problem_statement','reference']);
  assert(requests.every(r=>r.content==='Exact input, never a reviewed lemma.'));
  assert(requests.every(r=>r.problem_version===undefined),'backend owns the current problem version');
  await assert.rejects(()=>storeResearchMaterial({runtimeRoot,conversationId:conversation.id,name:'bad.md',contentBase64:'eA==',materialRole:'verified_fact'}),{status:422});
});
