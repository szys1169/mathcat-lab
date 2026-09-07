export const normalizeLocalPath=value=>String(value).trim().replace(/^\/(?=[A-Za-z]:[\\/])/,'').replace(/\\_/g,'_');
export const workspaceFileUrl=(conversationId,filePath)=>'/api/workspace-file?conversationId='+encodeURIComponent(conversationId)+'&path='+encodeURIComponent(normalizeLocalPath(filePath));
export const workspaceRevealUrl=(conversationId,filePath)=>'/api/reveal-file?conversationId='+encodeURIComponent(conversationId)+'&path='+encodeURIComponent(normalizeLocalPath(filePath));
const h=value=>String(value).replace(/[&<>"']/g,char=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[char]));
export function classifyMessageLink(raw){
  let value=String(raw).trim();if(value.startsWith('<')&&value.endsWith('>'))value=value.slice(1,-1);
  if(!value||/[\x00-\x1f]/.test(value))return {kind:'invalid'};
  if(/^https?:\/\//i.test(value)||/^mailto:/i.test(value)||value.startsWith('#'))return {kind:'external',href:value};
  if(/^\/\//.test(value))return {kind:'external',href:value};
  if(/^(?:[A-Za-z][A-Za-z0-9+.-]*:)/.test(value)&&!/^\/?[A-Za-z]:[\\/]/.test(value))return {kind:'invalid'};
  // Existing application URLs remain application URLs; ordinary relative paths belong to the conversation.
  if(/^\/api\//.test(value))return {kind:'external',href:value};
  const hash=value.indexOf('#'),fragment=hash>=0?value.slice(hash):'';if(hash>=0)value=value.slice(0,hash);
  try{value=decodeURIComponent(value);}catch{return {kind:'invalid'};}
  return {kind:'workspace',path:normalizeLocalPath(value),fragment};
}

// Protect code and links before math rendering; model-authored HTML is never trusted.
export function prepareMessageLinks(value,conversationId,renderFile){
  const tokens=[],tokenFor=html=>{const token='@@MATHLAB_LINK_'+tokens.length+'@@';tokens.push(html);return token;};
  const codes=[];let text=String(value).replace(/```[\s\S]*?```|`[^`\r\n]*`/g,source=>{const token='@@MATHLAB_LITERAL_'+codes.length+'@@';codes.push(source);return token;});
  text=text.replace(/:codex-file-citation\{path=(?:"([^"]+)"|'([^']+)')(?:\s+purpose=(?:"[^"]*"|'[^']*'))?\}/g,(_,a,b)=>tokenFor(renderFile(conversationId,a||b,'打开文件')));
  text=text.replace(/(?<!!)\[([^\]\r\n]+)\]\(\s*(<[^>\r\n]+>|(?:[^()\r\n]|\([^()\r\n]*\))+)\s*\)/g,(source,label,destination)=>{
    const link=classifyMessageLink(destination);if(link.kind==='invalid')return source;
    if(link.kind==='workspace')return tokenFor(renderFile(conversationId,link.path,label,link.fragment));
    return tokenFor('<a href="'+h(link.href)+'" target="_blank" rel="noopener noreferrer">'+h(label)+'</a>');
  });
  text=text.replace(/(^|[\s：:<])([/]?[A-Za-z]:[\\/][^\r\n<>"|?*]+?\.(?:pdf|tex|md|json|pptx|docx|zip|bib|log|png|jpe?g))(?=$|[\s，。；、>)）])/g,(_,prefix,file)=>prefix+tokenFor(renderFile(conversationId,file)));
  codes.forEach((source,index)=>{text=text.replace('@@MATHLAB_LITERAL_'+index+'@@',source);});
  return {text,restore:rendered=>tokens.reduce((output,html,index)=>output.replace('@@MATHLAB_LINK_'+index+'@@',()=>html),rendered)};
}
