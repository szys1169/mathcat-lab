import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import {bindSidebarNavigation} from '../public/sidebar-navigation.js';

function fixture(matches=true){
  const doc={activeElement:null,listeners:{},addEventListener(type,fn){this.listeners[type]=fn;}};
  const element=()=>({attrs:{},listeners:{},hidden:false,classList:{add(){},remove(){}},setAttribute(k,v){this.attrs[k]=v;},removeAttribute(k){delete this.attrs[k];},addEventListener(k,v){this.listeners[k]=v;},focus(){doc.activeElement=this;},getClientRects(){return [1];}});
  const sidebar=element(),main=element(),trigger=element(),closeButton=element(),backdrop=element(),last=element();
  sidebar.querySelectorAll=()=>[closeButton,last];
  const media={matches,addEventListener(_event,fn){this.changed=fn;}};
  return {...{doc,sidebar,main,trigger,closeButton,backdrop,last,media},navigation:bindSidebarNavigation({sidebar,main,trigger,closeButton,backdrop,document:doc,media})};
}
test('narrow navigation opens the existing sidebar and closes without replacing conversation data',()=>{
  const f=fixture();f.trigger.listeners.click();assert.equal(f.navigation.isOpen,true);assert.equal(f.main.inert,true);assert.equal(f.trigger.attrs['aria-expanded'],'true');
  f.navigation.close();assert.equal(f.main.inert,false);assert.equal(f.backdrop.hidden,true);assert.equal(f.doc.activeElement,f.trigger);
});
test('drawer supports keyboard focus, Escape, backdrop and return to desktop',()=>{
  const f=fixture();f.trigger.listeners.click();f.doc.activeElement=f.last;
  let prevented=false;f.doc.listeners.keydown({key:'Tab',preventDefault(){prevented=true;}});assert.equal(prevented,true);assert.equal(f.doc.activeElement,f.closeButton);
  f.doc.listeners.keydown({key:'Escape',preventDefault(){}});assert.equal(f.navigation.isOpen,false);
  f.trigger.listeners.click();f.backdrop.listeners.click();assert.equal(f.navigation.isOpen,false);
  f.trigger.listeners.click();f.media.matches=false;f.media.changed();assert.equal(f.main.inert,false);assert.equal(f.sidebar.attrs.role,undefined);
});
test('desktop sidebar remains normal and conversation selection closes drawer before restoring board',async()=>{
  const f=fixture(false);f.trigger.listeners.click();assert.equal(f.navigation.isOpen,false);assert.notEqual(f.main.inert,true);
  const app=await fs.readFile(new URL('../public/app.js',import.meta.url),'utf8'),html=await fs.readFile(new URL('../public/index.html',import.meta.url),'utf8');
  assert.match(html,/id="sidebarToggle"[^>]+aria-controls="conversationSidebar"/);
  assert.match(app,/sidebarNavigation\.close\(\); clearTimeout\(state\.poll\)/);
  assert.match(app,/state\.conversationId===selectedId&&state\.currentBoard\?\.classicV2\)setResearchView\("board",\{refresh:false\}\)/);
});
