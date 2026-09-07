const identity=node=>{
  if(node.nodeType!==1)return null;
  for(const attribute of ['data-wb-stable','id','data-wb-select','data-wb-session','data-id']){
    const value=node.getAttribute(attribute);if(value!==null)return node.nodeName+':'+attribute+':'+value+':'+(node.getAttribute('data-wb-action')||'');
  }
  return null;
};
const sameKind=(a,b)=>a?.nodeType===b.nodeType&&a?.nodeName===b.nodeName;

function updateNode(current,next){
  if(current.nodeType!==1){if(current.nodeValue!==next.nodeValue)current.nodeValue=next.nodeValue;return;}
  const detailsOpen=current.tagName==='DETAILS'?current.open:null;
  const editable=['INPUT','TEXTAREA','SELECT'].includes(current.tagName);
  const focused=current.ownerDocument.activeElement===current;
  const dirty=editable&&(focused||current.tagName==='SELECT'||current.value!==current.defaultValue||current.checked!==current.defaultChecked);
  const saved=dirty?{value:current.value,checked:current.checked,start:current.selectionStart,end:current.selectionEnd,direction:current.selectionDirection}:null;
  const scroll={top:current.scrollTop,left:current.scrollLeft};
  for(const attribute of [...current.attributes])if(!(current.tagName==='DETAILS'&&attribute.name==='open')&&!next.hasAttribute(attribute.name))current.removeAttribute(attribute.name);
  for(const attribute of next.attributes)if(!(current.tagName==='DETAILS'&&attribute.name==='open')&&current.getAttribute(attribute.name)!==attribute.value)current.setAttribute(attribute.name,attribute.value);
  reconcileChildren(current,next);
  if(detailsOpen!==null)current.open=detailsOpen;
  if(saved){current.value=saved.value;if('checked' in current)current.checked=saved.checked;if(focused&&saved.start!==null&&typeof current.setSelectionRange==='function'){try{current.setSelectionRange(saved.start,saved.end,saved.direction);}catch{}}}
  current.scrollTop=scroll.top;current.scrollLeft=scroll.left;
}

function reconcileChildren(parent,nextParent){
  const old=[...parent.childNodes],used=new Set(),keyed=new Map(old.map(node=>[identity(node),node]).filter(([key])=>key));
  let cursor=parent.firstChild;
  for(const next of [...nextParent.childNodes]){
    const key=identity(next);let current=key?keyed.get(key):cursor;
    if(!current||used.has(current)||!sameKind(current,next)||(!key&&identity(current)))current=null;
    if(!current){current=next.cloneNode(true);parent.insertBefore(current,cursor);}
    else{if(current!==cursor)parent.insertBefore(current,cursor);updateNode(current,next);}
    used.add(current);cursor=current.nextSibling;
  }
  for(const node of old)if(!used.has(node)&&node.parentNode===parent)parent.removeChild(node);
}

// Reuse summaries and controls instead of detaching the target between pointerdown and click.
export function patchLiveContent(element,html){
  if(!element||element._wbHtml===html)return;
  const template=element.ownerDocument.createElement('template');template.innerHTML=html;
  const focused=element.ownerDocument.activeElement,restoreFocus=focused&&element.contains(focused);
  const top=element.scrollTop,left=element.scrollLeft;
  reconcileChildren(element,template.content);element._wbHtml=html;
  if(restoreFocus&&focused.isConnected&&element.ownerDocument.activeElement!==focused)focused.focus({preventScroll:true});
  element.scrollTop=top;element.scrollLeft=left;
}
