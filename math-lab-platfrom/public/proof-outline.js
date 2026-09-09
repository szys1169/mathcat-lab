import {escapeHtml as h} from './research-v2-state.js';

export function outlineStatus(node){
  const o=node.object||node;
  if(node.status==='blocked'||(o.validity&&o.validity!=='current'))return '存在缺口';
  if(['rejected','incorrect','changes_requested'].includes(o.review_state))return '需要修订';
  if(node.trust==='verified'||(['accepted','admitted'].includes(o.admission_state)&&o.validity==='current'))return '已准入';
  if(o.review_state==='submitted')return '待审查';
  if(node.status==='active'||o.work_state==='working')return '研究中';
  return '尚未准入';
}

// Navigation is a projection only. Ownership never becomes a proof dependency.
export function proofOutline(model,{query='',filter='all'}={}){
  const nodes=new Map(model.nodes.map(n=>[n.id,n]));
  const children=new Map([...nodes.keys()].map(id=>[id,[]]));
  const parents=new Map([...nodes.keys()].map(id=>[id,new Set()]));
  for(const e of model.edges){
    if(!e.mathematical||!nodes.has(e.from)||!nodes.has(e.to))continue;
    if(!children.get(e.to).some(x=>x.id===e.from))children.get(e.to).push({id:e.from,planned:e.relation==='planned'||e.kind==='planned'||e.relationLabel?.includes('拟')||e.record?.dependency_state==='proposed'});
    parents.get(e.from).add(e.to);
  }
  const matches=new Set([...nodes].filter(([,n])=>{
    const text=[n.title,n.label,n.object?.title,n.object?.statement,n.object?.exact_statement].filter(Boolean).join(' ').toLocaleLowerCase();
    return (!query||text.includes(query.trim().toLocaleLowerCase()))&&(filter==='all'||(filter==='problems'?outlineStatus(n)==='存在缺口'||outlineStatus(n)==='需要修订':outlineStatus(n)!=='已准入'));
  }).map(([id])=>id));
  const keep=new Set(matches),queue=[...matches];
  for(let i=0;i<queue.length;i++)for(const parent of parents.get(queue[i])||[])if(!keep.has(parent)){keep.add(parent);queue.push(parent);}
  const covered=new Set(),visit=id=>{if(covered.has(id))return;covered.add(id);for(const c of children.get(id)||[])visit(c.id);};
  const roots=nodes.has(model.rootId)?[model.rootId]:[];roots.forEach(visit);
  const unlinked=[];
  for(const id of nodes.keys())if(!covered.has(id)&&![...parents.get(id)].some(p=>!covered.has(p))){unlinked.push(id);visit(id);}
  // Cyclic components have no natural root; retain a visible entry and stop at path cycles.
  for(const id of nodes.keys())if(!covered.has(id)){unlinked.push(id);visit(id);}
  return {nodes,children,parents,roots,unlinked,keep,matches,searching:Boolean(query.trim()||filter!=='all')};
}

export function outlineHtml(model,ui,renderText=h){
  const tree=proofOutline(model,ui),seen=new Set();
  const render=(id,path=[],planned=false)=>{
    if(!tree.keep.has(id))return '';
    const node=tree.nodes.get(id),cycle=path.includes(id),shared=seen.has(id);seen.add(id);
    const children=(tree.children.get(id)||[]).filter(c=>tree.keep.has(c.id));
    const expanded=!cycle&&!shared&&(tree.searching||ui.outlineExpanded?.has(id)||(!ui.outlineInitialized&&id===model.rootId));
    const title=node.object?.title||node.title||node.label||'未命名成果',count=tree.parents.get(id).size;
    return '<li><div class="wb-outline-row '+(ui.selected===id?'is-selected':'')+'">'+
      (children.length&&!cycle&&!shared?'<button class="wb-outline-toggle" data-wb-expand="'+h(id)+'" aria-label="'+h((expanded?'折叠':'展开')+title)+'" aria-expanded="'+expanded+'">'+(expanded?'▾':'▸')+'</button>':'<span class="wb-outline-spacer"></span>')+
      '<button class="wb-outline-title" data-wb-select="'+h(id)+'" '+(ui.selected===id?'aria-current="true"':'')+' title="'+h(title)+'">'+renderText(title)+'</button><span class="wb-outline-status">'+h(outlineStatus(node))+'</span></div>'+
      (planned?'<small class="wb-outline-note">拟依赖，尚未确认</small>':'')+
      (cycle?'<small class="wb-outline-note">循环依赖 · 请检查关系</small>':shared?'<small class="wb-outline-note">共享引用 · 点击名称阅读</small>':count>1?'<button class="wb-outline-reference" data-wb-select="'+h(id)+'" data-wb-show-relations>被 '+count+' 项成果引用</button>':'')+
      (expanded&&children.length?'<ul>'+children.map(c=>render(c.id,[...path,id],c.planned)).join('')+'</ul>':'')+'</li>';
  };
  const main=tree.roots.map(id=>render(id)).join(''),extra=tree.unlinked.map(id=>render(id)).join('');
  return '<nav class="wb-outline" aria-label="成果与直接前提"><p class="wb-outline-help">展开箭头查看前提，点击名称阅读原文。</p><ul>'+main+'</ul>'+
    (extra?'<details class="wb-outline-unlinked" '+(tree.searching||(ui.unlinkedOpen??!(tree.children.get(model.rootId)||[]).length)?'open':'')+'><summary>尚未关联的成果 <span>'+tree.unlinked.filter(id=>tree.keep.has(id)).length+'</span></summary><p class="wb-outline-help">未记录通向当前目标的数学依赖。</p><ul>'+extra+'</ul></details>':'')+
    (!main&&!extra?'<p class="wb-outline-help">没有匹配的成果。</p>':'')+'</nav>';
}
