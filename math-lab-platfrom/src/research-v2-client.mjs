import fs from "node:fs/promises";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";

export const V2_PREFIX = "/api/v2/research";
export const V2_CONTRACT = "mathcat-research/v2";

export function validateV2Path(value) {
  const raw = String(value || "");
  if (!raw.startsWith(`${V2_PREFIX}/`) || /[\\\r\n#]/.test(raw)) throw Object.assign(new Error("Invalid research API path."), {status:400});
  const decoded = decodeURIComponent(raw.split("?")[0]);
  if (decoded.split("/").some(part => part === "." || part === "..") || /%2f|%5c/i.test(decoded)) throw Object.assign(new Error("Invalid research API path."), {status:400});
  return raw;
}

export function assertLocalRequest(req, port) {
  const allowed = new Set([`127.0.0.1:${port}`, `localhost:${port}`, `[::1]:${port}`]);
  if (!allowed.has(String(req.headers.host || "").toLowerCase())) throw Object.assign(new Error("Unrecognized local host."), {status:403});
  if (req.headers.origin && ![...allowed].some(host => req.headers.origin === `http://${host}`)) throw Object.assign(new Error("Cross-origin access is not allowed."), {status:403});
  if (req.headers["sec-fetch-site"] === "cross-site") throw Object.assign(new Error("Cross-site access is not allowed."), {status:403});
}

export class ResearchV2Client {
  constructor({baseUrl="http://127.0.0.1:8899", tokenFile, timeoutMs=15000, fetchImpl=fetch}) {
    const url = new URL(baseUrl);
    if (url.protocol!=="http:" || !["127.0.0.1","localhost","[::1]"].includes(url.hostname) || url.username || url.password || url.pathname!=="/") throw new Error("Research backend must be a local HTTP origin.");
    this.baseUrl=url.origin; this.tokenFile=tokenFile; this.timeoutMs=timeoutMs; this.fetchImpl=fetchImpl;
  }
  async headers(extra={}) {
    let token;
    try { token=(await fs.readFile(this.tokenFile,"utf8")).trim(); }
    catch { throw Object.assign(new Error("MathCat 2.5.0 尚未启动：找不到本版本的本地连接凭据。请使用 2.5.0 启动器。"),{status:503}); }
    if (!token || /[\r\n]/.test(token)) throw Object.assign(new Error("本版本的本地连接凭据无效，请重新启动 MathCat 2.5.0。"),{status:503});
    return {...extra,authorization:`Bearer ${token}`};
  }
  async request(apiPath,{method="GET",body,idempotencyKey,signal}={}) {
    const headers=await this.headers({accept:"application/json",...(body!==undefined?{"content-type":"application/json"}:{}),...(idempotencyKey?{"idempotency-key":idempotencyKey}:{})});
    const response=await this.fetchImpl(this.baseUrl+validateV2Path(apiPath),{method,headers,body:body===undefined?undefined:JSON.stringify(body),redirect:"error",signal:signal||AbortSignal.timeout(this.timeoutMs)});
    const value=await response.json();
    if(!response.ok)throw Object.assign(new Error(value.error?.message||value.message||(typeof value.error==="string"?value.error:`Research API HTTP ${response.status}`)),{status:response.status,code:value.error?.code,details:value.error?.details});
    if(value.contract && value.contract!==V2_CONTRACT)throw Object.assign(new Error("研究服务协议不匹配，请检查是否启动了 MathCat 2.5.0。"),{status:502});
    return value;
  }
  async proxy(req,res) {
    const apiPath=validateV2Path(req.url);
    const headers=await this.headers(Object.fromEntries(["content-type","idempotency-key","last-event-id","accept"].filter(k=>req.headers[k]).map(k=>[k,req.headers[k]])));
    const chunks=[];let size=0;
    for await(const chunk of req){size+=chunk.length;if(size>2_000_000)throw Object.assign(new Error("Request is too large."),{status:413});chunks.push(chunk);}
    const controller=new AbortController();
    const onClose=()=>{if(!res.writableEnded)controller.abort();};res.once("close",onClose);
    const timer=setTimeout(()=>controller.abort(),this.timeoutMs);timer.unref?.();
    try {
      const response=await this.fetchImpl(this.baseUrl+apiPath,{method:req.method,headers,body:["GET","HEAD"].includes(req.method)?undefined:Buffer.concat(chunks),redirect:"error",signal:controller.signal});
      clearTimeout(timer);
      const responseHeaders={"content-type":response.headers.get("content-type")||"application/json; charset=utf-8","cache-control":"no-store","x-content-type-options":"nosniff"};
      for(const key of ["content-disposition","content-security-policy"])if(response.headers.has(key))responseHeaders[key]=response.headers.get(key);
      res.writeHead(response.status,responseHeaders);res.flushHeaders();
      if(response.body)await pipeline(Readable.fromWeb(response.body),res);else res.end();
    } catch(error) { if(!controller.signal.aborted)throw error; if(!res.headersSent)throw Object.assign(new Error("研究服务连接超时。已有任务状态尚未改变，请刷新确认。"),{status:504}); }
    finally {clearTimeout(timer);res.removeListener("close",onClose);}
  }
}

export function unwrapProject(value) {
  const project=value.project||value;
  if(!project?.id)throw Object.assign(new Error("研究服务没有返回项目身份，未创建虚假白板。"),{status:502});
  return project;
}
