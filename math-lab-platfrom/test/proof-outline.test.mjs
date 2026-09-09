import {test} from 'node:test';
import assert from 'node:assert/strict';
import {proofOutline,outlineHtml,outlineStatus} from '../public/proof-outline.js';
const model=(ids,edges)=>({rootId:'goal',nodes:ids.map(id=>({id,title:id,object:{title:id}})),edges:edges.map(([from,to,mathematical=true])=>({from,to,mathematical}))});
test('ownership does not become a mathematical prerequisite and input is immutable',()=>{
 const m=model(['goal','result','lemma','orphan'],[['result','goal'],['lemma','result'],['goal','orphan',false]]),before=JSON.stringify(m),tree=proofOutline(m);
 assert.deepEqual(tree.children.get('goal').map(x=>x.id),['result']);assert.deepEqual(tree.unlinked,['orphan']);assert.equal(JSON.stringify(m),before);
 const html=outlineHtml(m,{});assert.match(html,/data-wb-select="result"/);assert.doesNotMatch(html,/data-wb-select="lemma"/);
});
test('deep search retains ancestors and unfolds the path to a match',()=>{
 const m=model(['goal','result','needle','other'],[['result','goal'],['needle','result'],['other','goal']]);
 const tree=proofOutline(m,{query:'needle'});assert.deepEqual([...tree.keep].sort(),['goal','needle','result']);
 const html=outlineHtml(m,{query:'needle'});assert.match(html,/data-wb-select="needle"/);assert.doesNotMatch(html,/data-wb-select="other"/);
});
test('shared premises are reference rows and duplicate edges do not inflate counts',()=>{
 const m=model(['goal','a','b','shared'],[['a','goal'],['b','goal'],['shared','a'],['shared','b'],['shared','b']]);
 const html=outlineHtml(m,{outlineExpanded:new Set(['goal','a','b'])});assert.match(html,/被 2 项成果引用/);assert.match(html,/共享引用/);
});
test('cycles and disconnected cyclic components remain visible without recursion loops',()=>{
 const m=model(['goal','a','b'],[['a','b'],['b','a']]);const tree=proofOutline(m);assert.equal(tree.unlinked.length,1);
 assert.match(outlineHtml(m,{query:'a'}),/循环依赖/);
});
test('review acceptance without current admission is never labeled admitted',()=>{
 assert.equal(outlineStatus({review_state:'accepted'}),'尚未准入');assert.equal(outlineStatus({admission_state:'accepted',validity:'stale'}),'存在缺口');
 assert.equal(outlineStatus({admission_state:'accepted',validity:'current'}),'已准入');
});
test('large first level stays vertical and deeper branches remain collapsed; labels are escaped',()=>{
 const ids=['goal',...Array.from({length:200},(_,i)=>'lemma'+i),'<script>'];const m=model(ids,ids.slice(1).map(id=>[id,'goal']));
 const html=outlineHtml(m,{});assert.equal((html.match(/data-wb-select=/g)||[]).length,202);assert.match(html,/&lt;script&gt;/);assert.doesNotMatch(html,/<script>/);
});
