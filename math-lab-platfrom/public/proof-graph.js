import {layoutGraph, overrideGraphPositions} from './research-graph.js';
import {graphEdgeId, routeGraphEdges} from './graph-edges.js';

const rows = value => Array.isArray(value) ? value : [];
const relations = {declared:'声明依赖',planned:'拟依赖',checked:'已核对使用',ownership:'目标归属'};

// The tree reads from the goal down to its prerequisites. Stored mathematical
// edges still point from a prerequisite to the result that uses it.
function placementModel(model) {
  return {...model,kind:'proof',nodes:rows(model.nodes),edges:rows(model.edges).map(edge => {
    const normalized = {...edge,id:graphEdgeId(edge)};
    return edge.mathematical ? {...normalized,from:edge.to,to:edge.from} : normalized;
  })};
}

export function layoutProofGraph(model, {manualPositions=new Map(),nodeWidth=320,nodeHeight=152}={}) {
  const width = Number.isFinite(nodeWidth) && nodeWidth > 0 ? nodeWidth : 320;
  const height = Number.isFinite(nodeHeight) && nodeHeight > 0 ? nodeHeight : 152;
  const base = layoutGraph(placementModel(model));
  if (!base.positions.size) return {...base,width:460,height:300};
  const left = Math.min(...[...base.positions.values()].map(p => p.x));
  const top = Math.min(...[...base.positions.values()].map(p => p.y));
  const positions = new Map([...base.positions].map(([id,p]) => [id,{
    x:70+(p.x-left)*width/252,y:44+(p.y-top)*height/108,width,height
  }]));
  const right = Math.max(...[...positions.values()].map(p => p.x+p.width));
  const bottom = Math.max(...[...positions.values()].map(p => p.y+p.height));
  return overrideGraphPositions({...base,positions,width:right+70,height:bottom+44},manualPositions);
}

export function directProofRelations(model, selectedId) {
  const ids = new Set(rows(model.nodes).map(node => node.id));
  const valid = rows(model.edges).filter(edge => ids.has(edge.from) && ids.has(edge.to));
  const incoming = valid.filter(edge => edge.mathematical && edge.to === selectedId);
  const outgoing = valid.filter(edge => edge.mathematical && edge.from === selectedId);
  const ownership = valid.filter(edge => !edge.mathematical && (edge.from === selectedId || edge.to === selectedId));
  const prerequisiteIds = new Set(incoming.map(edge => edge.from));
  const consequenceIds = new Set(outgoing.map(edge => edge.to));
  const neighborIds = new Set([...prerequisiteIds,...consequenceIds]);
  neighborIds.delete(selectedId);
  const relatedIds = new Set(neighborIds);
  if (ids.has(selectedId)) relatedIds.add(selectedId);
  return {incoming,outgoing,ownership,prerequisiteIds,consequenceIds,neighborIds,relatedIds};
}

export function proofTextLines(value, {maxUnits=27,maxLines=3}={}) {
  const text = String(value ?? '').replace(/\s+/gu,' ').trim();
  if (!text || maxUnits < 1 || maxLines < 1) return [];
  const unitLimit = Math.max(2,Math.floor(maxUnits));
  const lineLimit = Math.floor(maxLines);
  const characters = Array.from(text);
  const weight = ch => /^[\x00-\x7f]$/u.test(ch) ? 1 : 2;
  const lines = [];
  let cursor = 0;
  while (cursor < characters.length && lines.length < lineLimit) {
    let line = '',units = 0;
    while (cursor < characters.length && units+weight(characters[cursor]) <= unitLimit) {
      const ch = characters[cursor++];line += ch;units += weight(ch);
    }
    lines.push(line.trim());
  }
  if (cursor < characters.length) {
    const last = Array.from(lines.at(-1));
    let units = last.reduce((sum,ch) => sum+weight(ch),0);
    while (last.length && units+2 > unitLimit) units -= weight(last.pop());
    lines[lines.length-1] = last.join('').trimEnd()+'…';
  }
  return lines;
}

function pathOf(points) {
  return points.map(([x,y],index) => `${index?'L':'M'}${x} ${y}`).join(' ');
}

function labelPosition(points) {
  // Prefer a long horizontal segment so a label does not sit on an arrowhead.
  let chosen = null;
  for (let i=1;i<points.length;i++) {
    const a=points[i-1],b=points[i],length=Math.abs(b[0]-a[0]);
    if (a[1]===b[1] && length > (chosen?.length ?? 0)) chosen={a,b,length};
  }
  if (!chosen) {
    const middle = Math.max(1,Math.floor(points.length/2));
    chosen={a:points[middle-1],b:points[middle]};
  }
  return {labelX:(chosen.a[0]+chosen.b[0])/2,labelY:(chosen.a[1]+chosen.b[1])/2-9};
}

export function routeProofGraphEdges(model, layout, {selectedId=null,showCrossLinks=true}={}) {
  const placement = placementModel(model);
  const originalById = new Map(rows(model.edges).map(edge => [graphEdgeId(edge),edge]));
  const result = routeGraphEdges(placement,layout,{selectedId,showCrossLinks});
  const edges = result.edges.map(routed => {
    const original = originalById.get(routed.id);
    const points = original.mathematical ? [...routed.points].reverse().map(p => [...p]) : routed.points.map(p => [...p]);
    const selected = original.from===selectedId || original.to===selectedId;
    return {...routed,...original,id:routed.id,points,path:pathOf(points),...labelPosition(points),
      selected,showLabel:selected,arrow:Boolean(original.mathematical),
      relationLabel:original.relationLabel||relations[original.relation]||'相关记录'};
  });
  return {...result,edges};
}
