export function bindSidebarNavigation({sidebar,main,trigger,closeButton,backdrop,document:doc=globalThis.document,media=globalThis.matchMedia('(max-width:760px)')}) {
  let open=false;
  const close=({restoreFocus=true}={})=>{
    const wasOpen=open;open=false;sidebar.classList.remove('is-open');backdrop.hidden=true;main.inert=false;
    trigger.setAttribute('aria-expanded','false');sidebar.removeAttribute('role');sidebar.removeAttribute('aria-modal');
    if(wasOpen&&restoreFocus&&media.matches)trigger.focus();
  };
  trigger.addEventListener('click',()=>{
    if(!media.matches)return;
    if(open){close();return;}
    open=true;sidebar.classList.add('is-open');backdrop.hidden=false;main.inert=true;
    trigger.setAttribute('aria-expanded','true');sidebar.setAttribute('role','dialog');sidebar.setAttribute('aria-modal','true');closeButton.focus();
  });
  closeButton.addEventListener('click',()=>close());backdrop.addEventListener('click',()=>close());
  doc.addEventListener('keydown',event=>{
    if(!open)return;
    if(event.key==='Escape'){event.preventDefault();close();return;}
    if(event.key!=='Tab')return;
    const items=[...sidebar.querySelectorAll('button:not([disabled]),a[href],input,select,textarea,[tabindex="0"]')].filter(e=>e.getClientRects().length);
    const first=items[0],last=items.at(-1);if(!first)return;
    if(event.shiftKey&&doc.activeElement===first){event.preventDefault();last.focus();}
    else if(!event.shiftKey&&doc.activeElement===last){event.preventDefault();first.focus();}
  });
  sidebar.addEventListener('click',event=>{
    if(event.target.closest('#settingsButton,#addWorkspace'))close();
  },true);
  media.addEventListener('change',()=>{if(!media.matches)close({restoreFocus:false});});
  return {close,get isOpen(){return open;}};
}
