import fs from "node:fs/promises";
import path from "node:path";
import crypto from "node:crypto";
import {execFile} from "node:child_process";
import {promisify} from "node:util";
import {safeId} from "./fs-utils.mjs";

const execute=promisify(execFile);
const TEXT_EXTENSIONS=new Set([".txt",".md",".tex",".bib",".csv",".json"]);
export async function storeResearchMaterial({runtimeRoot,conversationId,name,contentBase64,extractPdf,materialRole='unclassified_input'}){
  if(!['problem_statement','reference','unclassified_input'].includes(materialRole))throw Object.assign(new Error('材料用途须为题面、参考资料或未分类输入。'),{status:422});
  const filename=path.basename(String(name||"")).replace(/[\x00-\x1f]/g,"");
  const extension=path.extname(filename).toLowerCase();
  if(!filename||(!TEXT_EXTENSIONS.has(extension)&&extension!==".pdf"))throw Object.assign(new Error("研究材料暂支持 PDF、TXT、Markdown、TeX、Bib、CSV 和 JSON。"),{status:422});
  if(typeof contentBase64!=="string"||!/^[A-Za-z0-9+/]*={0,2}$/.test(contentBase64)||contentBase64.length>8_400_000)throw Object.assign(new Error("材料编码无效或超过 6 MB 上限。"),{status:413});
  const bytes=Buffer.from(contentBase64,"base64");if(bytes.length>6*1024*1024)throw Object.assign(new Error("每份研究材料最多 6 MB。"),{status:413});
  const sha256=crypto.createHash("sha256").update(bytes).digest("hex");const id=crypto.randomUUID();
  const directory=path.join(runtimeRoot,"research-materials",safeId(conversationId));await fs.mkdir(directory,{recursive:true});
  const file=path.join(directory,id+extension);await fs.writeFile(file,bytes,{flag:"wx"});
  let text;
  if(extension===".pdf"){
    try{text=extractPdf?await extractPdf(file):(await execute("pdftotext",["-layout","-enc","UTF-8",file,"-"],{windowsHide:true,timeout:30000,maxBuffer:12*1024*1024})).stdout;}
    catch{throw Object.assign(new Error("PDF 已保存，但无法可靠提取正文。请提供对应 TeX/Markdown；尚未将该材料交给研究员。"),{status:422});}
    if(!text.trim())throw Object.assign(new Error("PDF 没有可提取文本，可能是扫描件。请先提供 OCR 或 TeX 正文；不会假称已读。"),{status:422});
  }else{
    try{text=new TextDecoder("utf-8",{fatal:true}).decode(bytes);}
    catch{throw Object.assign(new Error("文本不是有效 UTF-8，请另存为 UTF-8 后上传。"),{status:422});}
  }
  if(text.length>1_000_000)throw Object.assign(new Error("提取正文超过本次导入上限，请将材料按章节拆分。原文件已保留。"),{status:413});
  const contentPath=path.join(directory,id+".extracted.txt");await fs.writeFile(contentPath,text,{encoding:"utf8",flag:"wx"});
  return {id,filename,size:bytes.length,sha256,storedPath:file,contentPath,textLength:text.length,materialRole,extraction_method:extension===".pdf"?"pdftotext-layout":"utf8",createdAt:new Date().toISOString()};
}

export async function readMaterialText(material){return fs.readFile(material.contentPath,"utf8");}
export function publicMaterial(material){const {storedPath,contentPath,...visible}=material;return visible;}
