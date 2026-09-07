import fs from 'node:fs/promises';
import path from 'node:path';

const fail=(message,status)=>Object.assign(new Error(message),{status});
const h=value=>String(value).replace(/[&<>"']/g,char=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[char]));
const within=(root,file)=>{const relative=path.relative(root,file);return !relative||!path.isAbsolute(relative)&&relative!=='..'&&!relative.startsWith('..'+path.sep);};
const slash=value=>value.split(path.sep).join('/');
const fileUrl=(id,relative)=>'/api/workspace-file?conversationId='+encodeURIComponent(id)+'&path='+encodeURIComponent(relative||'.');

export async function resolveWorkspaceEntry(workspace,raw){
  if(typeof raw!=='string'||!raw.trim()||/[\x00-\x1f]/.test(raw))throw fail('文件或目录路径无效。',400);
  const input=raw.trim().replace(/^\/(?=[A-Za-z]:[\\/])/,'').replace(/\\_/g,'_').replace(/^\\\\\?\\(?=[A-Za-z]:\\)/,'');
  if(/:/.test(input.replace(/^[A-Za-z]:[\\/]/,'')))throw fail('文件或目录路径无效。',400);
  const root=path.resolve(workspace.path),requested=path.resolve(root,input);
  if(!within(root,requested))throw fail('路径超出此对话的工作区。',403);
  const realRoot=await fs.realpath(root),realFile=await fs.realpath(requested);
  if(!within(realRoot,realFile))throw fail('文件链接指向此对话工作区之外。',403);
  const stat=await fs.stat(realFile);
  if(!stat.isFile()&&!stat.isDirectory())throw fail('此路径不是普通文件或目录。',400);
  return {root,requested,realRoot,realFile,relative:slash(path.relative(root,requested)),stat};
}

export async function workspaceDirectoryPage(workspace,entry,conversationId){
  const entries=await fs.readdir(entry.requested,{withFileTypes:true}),visible=[];
  for(const child of entries){
    try{const item=await resolveWorkspaceEntry(workspace,path.join(entry.requested,child.name));visible.push({name:child.name,directory:item.stat.isDirectory(),relative:item.relative});}
    catch(error){if(error.status===403||error.code==='ENOENT'||error.code==='EACCES')continue;throw error;}
  }
  visible.sort((a,b)=>Number(b.directory)-Number(a.directory)||a.name.localeCompare(b.name,'zh-CN'));
  const label=entry.relative||workspace.name||'工作区',parent=entry.relative?slash(path.dirname(entry.relative)):null;
  const reveal='/api/reveal-file?conversationId='+encodeURIComponent(conversationId)+'&path='+encodeURIComponent(entry.relative||'.');
  return '<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>'+h(label)+' · 文件与目录</title><style>body{font:15px/1.65 system-ui,sans-serif;margin:0;background:#f5f6f1;color:#263d32}main{max-width:960px;margin:30px auto;padding:24px;background:white;border:1px solid #a7b8aa;border-radius:14px}h1{font-size:24px;overflow-wrap:anywhere}nav{display:flex;gap:18px;flex-wrap:wrap}a{color:#246342;overflow-wrap:anywhere}ul{list-style:none;padding:0}li{display:flex;gap:12px;padding:13px 0;border-bottom:1px solid #dae2d9}small{color:#617567}li small{min-width:36px}@media(max-width:650px){main{margin:12px;padding:18px}}</style></head><body><main><small>此对话的工作区</small><h1>'+h(label)+'</h1><nav><a href="'+h(fileUrl(conversationId,'.'))+'">工作区首页</a>'+(parent!==null?'<a href="'+h(fileUrl(conversationId,parent))+'">返回上一级</a>':'')+'<a href="'+h(reveal)+'" target="_blank" rel="noopener">在资源管理器中打开此目录</a></nav><ul>'+visible.map(item=>'<li><small>'+h(item.directory?'目录':'文件')+'</small><a href="'+h(fileUrl(conversationId,item.relative))+'">'+h(item.name)+(item.directory?'/':'')+'</a></li>').join('')+'</ul>'+(visible.length?'':'<p>此目录没有可浏览的文件。</p>')+'</main></body></html>';
}

export async function workspaceFileRoute({req,res,url,store,userFileHeaders,sendJson,reveal}){
  if(req.method!=='GET'||!['/api/workspace-file','/api/reveal-file'].includes(url.pathname))return false;
  const conversation=store.getConversation(url.searchParams.get('conversationId'));
  if(!conversation)throw fail('Conversation not found.',404);
  const workspace=store.workspaceFor(conversation),entry=await resolveWorkspaceEntry(workspace,url.searchParams.get('path'));
  if(url.pathname==='/api/reveal-file'){await reveal(entry.realFile);sendJson(res,200,{ok:true,path:entry.requested});return true;}
  if(entry.stat.isDirectory()){
    const html=await workspaceDirectoryPage(workspace,entry,conversation.id);
    res.writeHead(200,{'content-type':'text/html; charset=utf-8','content-length':Buffer.byteLength(html),'cache-control':'no-store','x-content-type-options':'nosniff','content-security-policy':"default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'"});res.end(html);return true;
  }
  const data=await fs.readFile(entry.realFile),headers=userFileHeaders(entry.requested);
  if(url.searchParams.get('download')==='1')headers['content-disposition']="attachment; filename*=UTF-8''"+encodeURIComponent(path.basename(entry.requested));
  res.writeHead(200,{...headers,'cache-control':'no-store','content-length':data.length});res.end(data);return true;
}
