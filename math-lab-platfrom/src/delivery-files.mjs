import fs from 'node:fs/promises';
import path from 'node:path';
import crypto from 'node:crypto';
import {verifyPaperSources} from './paper-writer.mjs';

export const digest = value => crypto.createHash('sha256').update(value).digest('hex');
export const deliveryError = (message, status=409) => Object.assign(new Error(message), {status});
export const safeSegment = value => /^[A-Za-z0-9_-]{1,100}$/.test(String(value));
export const unixPath = value => value.split(path.sep).join('/');
const inside = (root, file) => {const relative=path.relative(root,file);return relative===''||(!path.isAbsolute(relative)&&relative!=='..'&&!relative.startsWith('..'+path.sep));};

export async function projectFile(project, relative, {mustExist=true}={}) {
  if (typeof relative!=='string' || !relative || relative.includes('\\') || path.isAbsolute(relative) || relative.split('/').some(p=>!p||p==='.'||p==='..') || /[:\0]/.test(relative)) throw deliveryError('文件路径无效。',422);
  const root=await fs.realpath(project.workspace_path);
  const file=path.resolve(root,...relative.split('/'));
  if(!inside(root,file))throw deliveryError('文件不在当前研究项目中。',403);
  let probe=file;
  while(true){
    try {const resolved=await fs.realpath(probe);if(!inside(root,resolved))throw deliveryError('文件链接超出当前研究项目。',403);break;}
    catch(error){if(error.code!=='ENOENT')throw error;if(mustExist||probe===root)throw error;probe=path.dirname(probe);}
  }
  return file;
}

export function sourceIdentity(project) {
  return digest(JSON.stringify({problem:project.problem,problem_version:project.problem_version,
    facts:(project.facts||[]).map(f=>[f.id,f.revision,f.validity,f.assurance,f.problem_version,f.proof_sha256]),
    candidates:(project.candidates||[]).map(c=>[c.id,c.status,c.problem_version,c.proof_artifact_id,c.snapshot_hash])}));
}

export async function writeFrozen(file, value) {
  const bytes=Buffer.isBuffer(value)?value:Buffer.from(value);
  await fs.mkdir(path.dirname(file),{recursive:true});
  try{await fs.writeFile(file,bytes,{flag:'wx'});}
  catch(error){if(error.code!=='EEXIST')throw error;if(!(await fs.readFile(file)).equals(bytes))throw deliveryError('已有文件内容不同，已保留原文件：'+path.basename(file));}
}

export async function readEvidence(client, project, artifactId) {
  const artifact=(project.artifacts||[]).find(a=>a.id===artifactId);
  if(!artifact)throw deliveryError('成果引用的原始证明文件未注册：'+artifactId);
  const url=client.baseUrl+'/api/v2/research/projects/'+encodeURIComponent(project.id)+'/artifacts/'+encodeURIComponent(artifactId)+'/content';
  const response=await client.fetchImpl(url,{headers:await client.headers(),redirect:'error',signal:AbortSignal.timeout(30000)});
  if(!response.ok)throw deliveryError('无法读取原始成果，HTTP '+response.status,502);
  const bytes=Buffer.from(await response.arrayBuffer());
  if(bytes.length>32*1024*1024)throw deliveryError('单份原始成果超过 32 MB，请明确缩小交付材料范围。',413);
  if(!artifact.sha256||digest(bytes)!==artifact.sha256)throw deliveryError('原始成果哈希不符，无法整理为论文。');
  return {bytes,artifact};
}

export async function freezeSource(project, job, readArtifact) {
  const base='研究记录/交付来源/'+job.id;
  await fs.mkdir(await projectFile(project,base,{mustExist:false}),{recursive:true});
  const version=project.problem_version;
  const facts=(project.facts||[]).filter(f=>f.validity==='current' && ['model_reviewed','formally_checked'].includes(f.assurance) && f.problem_version===version);
  const selectedFiles=[],results=[],materials=[],citations=new Set();
  const pending=(project.candidates||[]).filter(c=>c.problem_version===version&&!facts.some(f=>f.candidate_id===c.id));
  for(const fact of facts){
    if(!safeSegment(fact.id))throw deliveryError('成果编号无效。');
    const {bytes,artifact}=await readArtifact(project,fact.proof_artifact_id);
    if(fact.proof_sha256 && fact.proof_sha256!==artifact.sha256)throw deliveryError('已审查结论与证明文件版本不一致。');
    const relative=base+'/proof-'+fact.id+'.md',file=await projectFile(project,relative,{mustExist:false});
    await writeFrozen(file,bytes);selectedFiles.push(file);
    results.push({id:fact.id,statement:fact.claim,status:'audited',sources:[file],allowed_role:'保留原始数学陈述与证明内容；审查状态 '+fact.assurance+'，不等于形式化认证。'});
    materials.push({id:artifact.id,path:file,sha256:artifact.sha256,kind:'exact_proof',review_ids:fact.review_ids||[]});
  }
  const selectedIds=new Set(facts.flatMap(f=>(project.candidates||[]).find(c=>c.id===f.candidate_id)?.source_artifact_ids||[]));
  for(const id of selectedIds){
    if(!safeSegment(id))throw deliveryError('引用材料编号无效。');
    const {bytes,artifact}=await readArtifact(project,id),bib=/\.bib(?:\.|$)/i.test(artifact.name||artifact.filename||'');
    const file=await projectFile(project,base+'/material-'+id+(bib?'.bib':'.txt'),{mustExist:false});await writeFrozen(file,bytes);selectedFiles.push(file);
    materials.push({id,path:file,sha256:artifact.sha256,kind:bib?'bibliography':'cited_source',assurance:artifact.assurance||'unreviewed'});
    if(bib)for(const match of bytes.toString('utf8').matchAll(/@(?:article|book|inproceedings|incollection|misc|phdthesis|mastersthesis|techreport|unpublished|proceedings)\s*\{\s*([^,\s]+)\s*,/gi))citations.add(match[1]);
  }
  const gaps=pending.map(c=>'候选尚未纳入本次已审查成果：'+c.claim);
  if(project.result_state!=='reviewed_solution')gaps.unshift('尚未确认原问题已完整解决。只陈述已有结果，不能写成完成原题。');
  const packet={schema_version:'1.0',writing_goal:{deliverable:'同一份精确成果的中文和英文数学论文',audience:'数学研究者',language:'bilingual'},core_results:results,source_materials:materials,citation_whitelist:[...citations],evidence_gaps:gaps,non_claims:['写作和排版检查不是新增数学审查。','不得虚构原创性、引用、作者、单位或投稿信息。','书目白名单仅界定已有条目，仍需按精确来源核对正文支撑。','原始问题：'+project.problem]};
  const packetPath=await projectFile(project,base+'/source-packet.json',{mustExist:false});
  await writeFrozen(packetPath,JSON.stringify(packet,null,2)+'\n');
  await writeFrozen(await projectFile(project,base+'/来源状态.json',{mustExist:false}),JSON.stringify({project_id:project.id,problem_version:version,source_key:sourceIdentity(project),facts,pending,created_at:job.created_at},null,2)+'\n');
  return {packetPath,selectedFiles,hasResults:results.length>0,source_key:sourceIdentity(project),warnings:gaps,base};
}

export async function stageReports(project, job) {
  const files=[];
  const exact=(project.candidates||[]).filter(c=>c.problem_version===project.problem_version).map(c=>`- ${c.claim}\n  - ${c.status} · ${c.proof_artifact_id||''}`).join('\n')||'—';
  for(const language of ['zh','en']){
    const name=language==='zh'?'中文阶段报告.md':'English-progress-report.md';
    const relative='成果/'+job.id+'/'+name;
    const text=language==='zh'?`# ${project.title}：阶段研究报告\n\n## 原始数学问题\n\n${project.problem}\n\n## 当前状态\n\n${project.result_state}。本次未自动生成完整论文。尚未确认的候选不构成已完成证明。\n\n## 候选原始陈述\n\n${exact}\n\n完整原文、审查和未完成步骤保留在项目研究记录中。此报告由已保存状态整理，没有新增模型调用。\n`:
      `# ${project.title}: research progress report\n\n## Original mathematical problem (verbatim)\n\n${project.problem}\n\n## Current status\n\n${project.result_state}. No complete paper was generated automatically. An unconfirmed candidate is not a completed proof.\n\n## Candidate statements (original language, verbatim)\n\n${exact}\n\nExact sources, reviews and unfinished work remain in the project records. This report is assembled from saved state without a new model call.\n`;
    await writeFrozen(await projectFile(project,relative,{mustExist:false}),text);files.push({path:relative,name,label:language==='zh'?'中文阶段报告':'English progress report',language,kind:'report'});
  }
  return files;
}

function crc32(bytes){let c=0xffffffff;for(const byte of bytes){c^=byte;for(let bit=0;bit<8;bit++)c=(c>>>1)^((c&1)?0xedb88320:0);}return(c^0xffffffff)>>>0;}
// Store-only ZIP avoids platform-dependent archive commands and includes the exact source files.
export async function zipSources(verifiedFiles) {
  const entries=verifiedFiles.map(file=>({name:Buffer.from(file.path),bytes:file.bytes}));
  if(entries.length>2000||entries.reduce((n,e)=>n+e.bytes.length,0)>64*1024*1024)throw deliveryError('论文源文件包过大，请缩小附件范围。',413);
  const parts=[],central=[];let offset=0;
  for(const {name,bytes} of entries){const crc=crc32(bytes),header=Buffer.alloc(30);header.writeUInt32LE(0x04034b50);header.writeUInt16LE(20,4);header.writeUInt16LE(0x800,6);header.writeUInt32LE(crc,14);header.writeUInt32LE(bytes.length,18);header.writeUInt32LE(bytes.length,22);header.writeUInt16LE(name.length,26);
    parts.push(header,name,bytes);const c=Buffer.alloc(46);c.writeUInt32LE(0x02014b50);c.writeUInt16LE(20,4);c.writeUInt16LE(20,6);c.writeUInt16LE(0x800,8);c.writeUInt32LE(crc,16);c.writeUInt32LE(bytes.length,20);c.writeUInt32LE(bytes.length,24);c.writeUInt16LE(name.length,28);c.writeUInt32LE(offset,42);central.push(c,name);offset+=header.length+name.length+bytes.length;}
  const centralBytes=Buffer.concat(central),end=Buffer.alloc(22);end.writeUInt32LE(0x06054b50);end.writeUInt16LE(entries.length,8);end.writeUInt16LE(entries.length,10);end.writeUInt32LE(centralBytes.length,12);end.writeUInt32LE(offset,16);
  return Buffer.concat([...parts,centralBytes,end]);
}

export async function publishLanguage(project,job,language,base){
  const prepared=job.prepared[language],result=job.results[language],prefix=language==='zh'?'中文论文':'English-paper';
  if(!result||result.status!=='completed'||result.compile_status!=='passed'||result.render_status!=='passed')throw deliveryError('稿件尚未通过编译和页面检查。');
  await projectFile(project,unixPath(path.relative(await fs.realpath(project.workspace_path),await fs.realpath(prepared.writerDir))));
  const verified=await verifyPaperSources(prepared,result),files=[];
  base??='成果/'+job.id+'/已检查稿件/'+language+'-'+job.steps.find(s=>s.id==='compile_'+language).attempts;
  for(const kind of ['pdf','tex']){
    const artifact=(result.artifacts||[]).find(a=>['article_'+kind,kind].includes(a.type||a.kind));
    if(!artifact?.sha256)throw deliveryError('缺少已检查稿件的文件指纹。');
    const source=artifact.absolutePath||path.resolve(prepared.runDir,artifact.path);
    await projectFile(project,unixPath(path.relative(await fs.realpath(project.workspace_path),await fs.realpath(source))));
    const bytes=await fs.readFile(source);if(digest(bytes)!==artifact.sha256)throw deliveryError('论文文件在检查后发生变化，请重新检查再发布。');
    const relative=base+'/'+prefix+'.'+kind;await writeFrozen(await projectFile(project,relative,{mustExist:false}),bytes);
    files.push({path:relative,name:prefix+'.'+kind,label:(language==='zh'?'中文论文':'English paper')+' · '+kind.toUpperCase(),language,kind});
  }
  const relative=base+'/'+prefix+'-source.zip';await writeFrozen(await projectFile(project,relative,{mustExist:false}),await zipSources(verified));
  files.push({path:relative,name:prefix+'-source.zip',label:language==='zh'?'中文完整源文件':'English source files',language,kind:'source'});
  return files;
}

export async function publishPair(project, job) {
  const files=[],texts={};
  const primary=JSON.parse(await fs.readFile(path.join(job.prepared.en.runDir,'primary_source.json'),'utf8'));
  if(await fs.realpath(primary.runDir)!==await fs.realpath(job.prepared.zh.runDir)||primary.fingerprint!==job.results.zh.source_fingerprint)throw deliveryError('英文稿对应的中文主稿与本次交付不一致，请重新同步英文。');
  for(const language of ['zh','en']){
    const prepared=job.prepared[language],result=job.results[language];
    if(!result||result.status!=='completed'||result.compile_status!=='passed'||result.render_status!=='passed')throw deliveryError('中英文稿都完成编译和页面检查后才能发布。');
    const tex=await fs.readFile(path.join(prepared.writerDir,'article_candidate.tex'),'utf8');texts[language]=tex;
  }
  const keys=text=>[...text.matchAll(/\\(label|(?:eq)?ref|cite\w*)\*?(?:\[[^\]]*\])*\{([^}]+)\}/g)].map(m=>m[1]+':'+m[2].split(',').map(x=>x.trim()).sort().join(',')).sort();
  if(JSON.stringify(keys(texts.zh))!==JSON.stringify(keys(texts.en)))throw deliveryError('中英文的定理、公式标签或引用未同步；请修正对应稿件后重试双语交付检查。');
  const base='成果/'+job.id+'/完整交付-'+job.steps.find(s=>s.id==='sync').attempts;
  for(const language of ['zh','en'])files.push(...await publishLanguage(project,job,language,base));
  await writeFrozen(await projectFile(project,base+'/交付说明.md',{mustExist:false}),`# ${project.title}：双语论文交付\n\n成果来源：数学问题 v${job.problem_version}。\n\n本交付包含中文及英文论文、PDF、LaTeX 和完整源文件包。两版均通过编译和机械页面检查，引用与标签已核对。写作检查不等于数学证明或人工审稿。\n\n${(job.warnings||[]).map(w=>'- '+w).join('\n')}\n`);
  files.push({path:base+'/交付说明.md',name:'交付说明.md',label:'交付说明',language:null,kind:'report'});
  return files;
}
