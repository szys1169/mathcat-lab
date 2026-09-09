import {test} from 'node:test';
import assert from 'node:assert/strict';
import {registeredBodyDraft} from '../public/proof-material.js';
test('only follows an explicit registered draft with matching digest',()=>{
 const p={artifacts:[{id:'draft',sha256:'known'}]},body={text:JSON.stringify({draft_artifact_refs:[{id:'draft',sha256:'known'}]})};
 assert.equal(registeredBodyDraft(body,p),'draft');
 assert.equal(registeredBodyDraft(body,{artifacts:[]}),null);
 assert.equal(registeredBodyDraft({text:JSON.stringify({draft_artifact_refs:[{id:'draft',sha256:'wrong'}]})},p),null);
});
test('does not infer a draft from prose, filenames, or unstructured proof text',()=>{
 assert.equal(registeredBodyDraft({text:'read full-proof.md'},{}),null);
 assert.equal(registeredBodyDraft({text:JSON.stringify({draft_path:'full-proof.md'})},{}),null);
});
