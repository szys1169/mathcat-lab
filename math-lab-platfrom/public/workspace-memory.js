export function createWorkspaceMemory(storage=globalThis.localStorage) {
  const prefix='mathcat.2.5.workspace.';
  const read=(key,fallback)=>{try{return JSON.parse(storage?.getItem(prefix+key)||'null')??fallback;}catch{return fallback;}};
  const write=(key,value)=>{try{storage?.setItem(prefix+key,JSON.stringify(value));}catch{}};
  return {
    session:()=>read('session',{}),saveSession:value=>write('session',value),
    draft:key=>read('draft.'+key,''),saveDraft:(key,text)=>write('draft.'+key,text),
    view:key=>read('view.'+key,{}),saveView:(key,value)=>write('view.'+key,value)
  };
}
