import fs from 'node:fs/promises';
import path from 'node:path';
import {deliveryError,safeSegment} from './delivery-files.mjs';

export async function researchProductRoute({req,res,url,pathname,deliveries,sendJson,readBody,userFileHeaders,reveal}){
  const match=pathname.match(/^\/api\/research-projects\/([^/]+)\/(delivery(?:\/(?:start|retry))?|control|files|reveal)$/);
  if(!match)return false;
  const [,id,action]=match;if(!safeSegment(id))throw deliveryError('研究项目编号无效。',422);
  const key=req.headers['idempotency-key'];
  if(req.method==='GET'&&action==='delivery'){sendJson(res,200,await deliveries.view(id));return true;}
  if(req.method==='GET'&&action==='files'){
    const file=await deliveries.registeredFile(id,url.searchParams.get('path'));
    const stat=await fs.stat(file);if(stat.size>96*1024*1024)throw deliveryError('文件较大，请使用打开成果目录查看。',413);
    const headers=userFileHeaders(file);
    if(url.searchParams.get('download')==='1')headers['content-disposition']="attachment; filename*=UTF-8''"+encodeURIComponent(path.basename(file));
    const bytes=await fs.readFile(file);res.writeHead(200,{...headers,'content-length':bytes.length,'cache-control':'no-store'});res.end(bytes);return true;
  }
  if(req.method!=='POST')throw deliveryError('此接口不支持该操作。',405);
  const input=await readBody(req);
  if(action==='reveal'){
    const directory=await deliveries.directory(id,input.folder);await reveal(directory);sendJson(res,200,{ok:true,path:directory});return true;
  }
  if(!key||typeof key!=='string')throw deliveryError('请求缺少回执标识。',422);
  if(action==='delivery/start')sendJson(res,202,await deliveries.start(id,{},key));
  else if(action==='delivery/retry')sendJson(res,202,await deliveries.retry(id,{step:input.step},key));
  else if(action==='control')sendJson(res,202,await deliveries.control(id,{type:input.type},key));
  else throw deliveryError('未找到操作。',404);
  return true;
}
