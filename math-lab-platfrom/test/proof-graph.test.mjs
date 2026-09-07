import test from 'node:test';
import assert from 'node:assert/strict';
import {layoutProofGraph,directProofRelations,proofTextLines,routeProofGraphEdges} from '../public/proof-graph.js';

function graph(ids,links) {
  return {kind:'proof',rootId:'goal',nodes:ids.map(id=>({id,kind:id==='goal'?'theorem':'lemma'})),
    edges:links.map(([from,to,mathematical=true],index)=>({id:'e'+index,from,to,mathematical,
      relation:mathematical?'checked':'ownership',relationLabel:mathematical?'已核对使用':'目标归属'}))};
}

function enters([x1,y1],[x2,y2],box) {
  const epsilon=.001;
  if (y1===y2) return y1>box.y+epsilon && y1<box.y+box.height-epsilon
    && Math.max(x1,x2)>box.x+epsilon && Math.min(x1,x2)<box.x+box.width-epsilon;
  assert.equal(x1,x2,'segments must be orthogonal');
  return x1>box.x+epsilon && x1<box.x+box.width-epsilon
    && Math.max(y1,y2)>box.y+epsilon && Math.min(y1,y2)<box.y+box.height-epsilon;
}

function safeGeometry(result,layout) {
  for (const edge of result.edges) {
    assert.doesNotMatch(edge.path,/NaN|Infinity|C/);
    for (let i=1;i<edge.points.length;i++) {
      for (const [id,box] of layout.positions) assert.equal(enters(edge.points[i-1],edge.points[i],box),false,edge.id+' crosses '+id);
    }
    for (const [x,y] of edge.points) assert.ok(x>=0 && y>=0 && x<=result.width && y<=result.height);
  }
}

test('large nodes preserve separated branches, original graph and stored dependency direction',()=>{
  const model=graph(['goal','a','b','source-a','source-b'],[['a','goal'],['b','goal'],['source-a','a'],['source-b','b']]);
  const before=structuredClone(model),layout=layoutProofGraph(model);
  for (const box of layout.positions.values()) {assert.equal(box.width,320);assert.equal(box.height,152);}
  assert.ok(layout.positions.get('goal').y<layout.positions.get('a').y);
  assert.ok(layout.positions.get('a').y<layout.positions.get('source-a').y);
  assert.ok(layout.positions.get('a').x+320<layout.positions.get('b').x);
  assert.deepEqual(model,before);
  const routed=routeProofGraphEdges(model,layout,{selectedId:'a'});
  const edge=routed.edges.find(e=>e.from==='a'&&e.to==='goal');
  const from=layout.positions.get('a'),to=layout.positions.get('goal');
  assert.deepEqual(edge.points[0],[from.x+from.width/2,from.y]);
  assert.deepEqual(edge.points.at(-1),[to.x+to.width/2,to.y+to.height]);
  assert.equal(edge.arrow,true);assert.equal(edge.showLabel,true);
  safeGeometry(routed,layout);
});

test('membership is never promoted to a mathematical prerequisite or arrow',()=>{
  const model=graph(['goal','result','lemma','downstream','unrelated'],[['goal','result',false],['lemma','result'],['result','downstream'],['unrelated','goal']]);
  const relations=directProofRelations(model,'result');
  assert.deepEqual([...relations.prerequisiteIds],['lemma']);
  assert.deepEqual([...relations.consequenceIds],['downstream']);
  assert.deepEqual([...relations.relatedIds].sort(),['downstream','lemma','result']);
  assert.equal(relations.ownership.length,1);
  assert.equal(directProofRelations(model,'missing').relatedIds.size,0);
  const routed=routeProofGraphEdges(model,layoutProofGraph(model),{selectedId:'result'});
  assert.equal(routed.edges.find(e=>!e.mathematical).arrow,false);
  assert.equal(routed.edges.find(e=>!e.mathematical).relationLabel,'目标归属');
  assert.equal(routed.edges.find(e=>e.from==='unrelated').selected,false);
});

test('parallel, cross-level and cyclic references remain distinct and avoid all nodes',()=>{
  const model=graph(['goal','a','b','c','d','side'],[['a','goal'],['b','a'],['c','b'],['d','c'],['d','a'],['d','a'],['side','a'],['side','b'],['goal','d'],['c','c']]);
  const layout=layoutProofGraph(model),routed=routeProofGraphEdges(model,layout,{selectedId:'d'});
  assert.equal(routed.edges.length,model.edges.length);
  const parallel=routed.edges.filter(e=>e.from==='d'&&e.to==='a');
  assert.equal(parallel.length,2);assert.notEqual(parallel[0].path,parallel[1].path);
  assert.ok(parallel.every(e=>e.showLabel&&e.arrow));
  assert.ok(routed.edges.some(e=>e.crossLink&&e.showLabel));
  safeGeometry(routed,layout);
  const reordered={...model,nodes:[...model.nodes].reverse(),edges:[...model.edges].reverse()};
  assert.deepEqual(routeProofGraphEdges(reordered,layoutProofGraph(reordered),{selectedId:'d'}),routed);
});

test('missing edge endpoints do not create phantom graph nodes or neighbors',()=>{
  const model=graph(['goal','a'],[['a','goal'],['missing','a']]);
  const layout=layoutProofGraph(model);
  assert.equal(layout.positions.size,2);
  assert.equal(routeProofGraphEdges(model,layout).edges.length,1);
  assert.equal(directProofRelations(model,'a').prerequisiteIds.size,0);
});

test('node placement supports pinned coordinates without mutating the position map',()=>{
  const model=graph(['goal','a'],[['a','goal']]);
  const manualPositions=new Map([['a',{x:1100,y:850}]]),before=structuredClone(manualPositions);
  const layout=layoutProofGraph(model,{manualPositions});
  assert.equal(layout.positions.get('a').x,1100);assert.equal(layout.positions.get('a').y,850);
  assert.ok(layout.width>1420);assert.ok(layout.height>1002);
  assert.deepEqual(manualPositions,before);
});

test('labels wrap mixed Chinese and mathematical text with bounded width and a visible truncation',()=>{
  assert.deepEqual(proofTextLines('有限维 A-module 的自由性引理',{maxUnits:16,maxLines:3}),['有限维 A-module','的自由性引理']);
  const truncated=proofTextLines('一二三四五六七八九十十一十二十三十四十五十六',{maxUnits:12,maxLines:2});
  assert.equal(truncated.length,2);assert.ok(truncated[1].endsWith('…'));
  for (const line of truncated) assert.ok(Array.from(line).reduce((n,ch)=>n+(/^[\x00-\x7f]$/.test(ch)?1:2),0)<=12);
  assert.deepEqual(proofTextLines(' \n\t '),[]);
  assert.deepEqual(proofTextLines('abcdef',{maxLines:0}),[]);
  assert.equal(proofTextLines('𝔪-primary',{maxUnits:10,maxLines:2}).join(''),'𝔪-primary');
});
