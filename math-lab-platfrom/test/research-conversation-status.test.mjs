import test from 'node:test';
import assert from 'node:assert/strict';
import {syncResearchConversationStatus} from '../src/research-conversation-status.mjs';

function fixture() {
  const row={id:'conversation',researchProjectId:'project',researchRunId:'run-1',status:'running',messages:[{content:'original research conversation'}]};
  let writes=0;
  const store={getConversation:()=>row,updateConversation:async(id,mutate)=>{assert.equal(id,row.id);mutate(row);writes++;}};
  return {row,store,writes:()=>writes};
}

test('time-limited research stops showing running without losing uncertainty or messages',async()=>{
  const f=fixture();
  const p={id:'project',runs:[{id:'run-1',state:'ended',result_state:'unresolved',outstanding_cancellation:true}]};
  await syncResearchConversationStatus(f.store,f.row,p);
  assert.equal(f.row.status,'idle');
  assert.equal(f.row.researchRunState,'ended');
  assert.equal(f.row.researchResultState,'unresolved');
  assert.equal(f.row.researchOutstandingCancellation,true);
  assert.deepEqual(f.row.messages,[{content:'original research conversation'}]);
  await syncResearchConversationStatus(f.store,f.row,p);
  assert.equal(f.writes(),1,'unchanged snapshots do not rewrite the conversation');
});

test('stale snapshot cannot overwrite a newer run or a different project binding',async()=>{
  const f=fixture();
  const p={id:'project',runs:[{id:'old-run',state:'ended'}]};
  await syncResearchConversationStatus(f.store,f.row,p);
  assert.equal(f.row.status,'running');
  p.runs=[{id:'run-1',state:'ended'}];p.id='another-project';
  await syncResearchConversationStatus(f.store,f.row,p);
  assert.equal(f.writes(),0);
});

test('paused research is distinguishable and resumes with the same run',async()=>{
  const f=fixture();const p={id:'project',runs:[{id:'run-1',state:'paused',result_state:'unresolved'}]};
  await syncResearchConversationStatus(f.store,f.row,p);assert.equal(f.row.status,'paused');
  p.runs[0].state='running';
  await syncResearchConversationStatus(f.store,f.row,p);assert.equal(f.row.status,'running');
  assert.equal(f.row.researchRunId,'run-1');
});
