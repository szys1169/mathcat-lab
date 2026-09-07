// Scoped evidence tools: no mathematical calculation, model spawning or credentials.
import fs from 'node:fs/promises';
import path from 'node:path';
import crypto from 'node:crypto';
import {fileURLToPath} from 'node:url';
const hash=b=>crypto.createHash('sha256').update(b).digest('hex');
const channels=new Set(['statement_checks','reference_checks','verification_reports','failed_checks','events']);
export function terms(text){const s=String(text).toLowerCase();return [...new Set([...(s.match(/[a-z0-9_]+/g)||[]),...(s.match(/[\p{Script=Han}]/gu)||[])])];}
function excerpt(record){
  const result={};
  for(const key of ['id','revision','problem_version','collection','kind','status','created_at','session_id','artifact_id','statement_id','claim_statement_id','proof_artifact_id','source_id','sha256']){
    if(['string','number','boolean'].includes(typeof record[key]))result[key]=typeof record[key]==='string'?record[key].slice(0,300):record[key];
  }
  for(const key of ['source_refs','statement_ids','dependencies'])if(Array.isArray(record[key]))result[key]=record[key].filter(v=>typeof v==='string').slice(0,20).map(v=>v.slice(0,300));
  return {...result,title:String(record.title||'').slice(0,180),text:String(record.text||record.body||record.claim||record.summary||'').slice(0,2400),source_assurance:record.assurance??null,source_validity:record.validity??null,excerpt:true,exact:false,notice:'Search excerpt only. Read the exact statement or original evidence before proof use.'};
}
export function search(records,query){const ts=terms(query);return records.map(record=>{const content=JSON.stringify(record).toLowerCase();return {record,score:ts.filter(t=>content.includes(t)).length};}).filter(r=>!ts.length||r.score).sort((a,b)=>b.score-a.score).slice(0,20).map(({record,score})=>({...excerpt(record),score}));}
async function inside(root,file){const resolved=await fs.realpath(file);if(resolved!==root&&!resolved.startsWith(root+path.sep))throw new Error('Path escaped task workspace');return resolved;}
async function recordReadHint(cwd,context,result,kind){
  const file=path.join(cwd,'.mathcat-evidence-reads.jsonl');
  try{await inside(cwd,file);}catch(error){if(error.code!=='ENOENT')throw error;}
  // This writable journal is only a recovery/search hint. The host alone records
  // authoritative input delivery; editing this file cannot satisfy premise checks.
  await fs.appendFile(file,JSON.stringify({session_id:context.session_id,invocation_id:context.invocation_id,kind,id:result.id,revision:result.revision,sha256:result.sha256,offset:result.offset,next_offset:result.next_offset,range_unit:'utf16_code_units',at:new Date().toISOString(),trust:'untrusted_local_read_hint'})+'\n');
}
function page(text,input={}){
  const offset=Number(input.offset??0);const length=Number(input.length??16000);
  if(!Number.isSafeInteger(offset)||offset<0||!Number.isSafeInteger(length)||length<1)throw new Error('offset and length must be nonnegative/positive integers');
  const end=Math.min(text.length,offset+Math.min(64000,length));
  return {text:text.slice(offset,end),offset,next_offset:end<text.length?end:null,total_characters:text.length,complete:offset===0&&end===text.length,range_unit:'utf16_code_units'};
}
export async function execute(operation,input={},options={}){
  const cwd=await fs.realpath(options.cwd||process.cwd());
  const context=JSON.parse(await fs.readFile(await inside(cwd,path.join(cwd,'.mathcat-context.json')),'utf8'));
  const remaining=context.deadline_at===null?Infinity:Date.parse(context.deadline_at)-Date.now();
  if(Number.isNaN(remaining)||remaining<=0)throw new Error('Task deadline reached');
  if(operation==='context-summary'){
    const lists={};
    for(const key of ['statements','artifacts','fact_records','experience_records','records']){
      const records=Array.isArray(context[key])?context[key]:[];
      lists[key]={count:records.length,entries:records.slice(0,12).map(r=>({id:r.id,revision:r.revision,kind:r.kind,source_assurance:r.assurance,source_validity:r.validity,title:String(r.title||r.filename||r.name||'').slice(0,180)})),truncated:records.length>12};
    }
    return {session_id:context.session_id,invocation_id:context.invocation_id,problem_version:context.problem_version,snapshot_revision:context.snapshot_revision??null,deadline_at:context.deadline_at,lists,notice:'Invocation snapshot, not live status. Use targeted search-memory, read-statement or read-evidence; do not print the full context file. Summaries are not proof premises.'};
  }
  if(['search-memory','search-facts','search-experiences'].includes(operation)){
    const mode=operation==='search-facts'?'facts':operation==='search-experiences'?'experiences':(input.mode||'experiences');
    if(!['facts','experiences'].includes(mode))throw new Error('Memory mode must be facts or experiences');
    const records=mode==='facts'?(context.fact_records??(context.records||[]).filter(r=>r.collection==='facts'&&r.assurance==='model_reviewed'&&r.validity==='current')):(context.experience_records??context.records??[]);
    const selected=records.filter(r=>input.problem_version===undefined||r.problem_version===input.problem_version);
    return {records:search(selected,input.query||''),mode,snapshot_problem_version:context.problem_version,snapshot_revision:context.snapshot_revision??null,source_status:'invocation_snapshot_revalidate_before_admission',notice:mode==='facts'?'Only host-selected current facts from this invocation snapshot; exact statement still required.':'Historical failures, withdrawals and unfinished attempts remain searchable. These excerpts are not proof premises.'};
  }
  if(operation==='read-statement'){
    const statement=(context.statements||[]).find(s=>s.id===input.id);
    if(!statement)throw new Error('Statement ID not in this session context');
    if(input.revision!==undefined&&statement.revision!==input.revision)throw new Error('Statement revision mismatch');
    if(typeof statement.text!=='string'||hash(Buffer.from(statement.text,'utf8'))!==statement.sha256)throw new Error('Statement hash mismatch or exact text missing');
    const result={id:statement.id,revision:statement.revision,sha256:statement.sha256,kind:statement.kind,assurance:statement.assurance,validity:statement.validity,...page(statement.text,input),exact:true,read_receipt_trust:'local_hint_only_host_delivery_is_recorded_separately'};
    await recordReadHint(cwd,context,result,'statement');return result;
  }
  if(operation==='read-evidence'){
    const a=(context.artifacts||[]).find(a=>a.id===input.id);if(!a)throw new Error('Evidence ID not in this session context');
    const root=await fs.realpath(context.artifact_root);const bytes=await fs.readFile(await inside(root,a.path));if(hash(bytes)!==a.sha256)throw new Error('Evidence hash mismatch');
    const text=new TextDecoder('utf-8',{fatal:true}).decode(bytes);const result={id:a.id,sha256:a.sha256,...page(text,input)};await recordReadHint(cwd,context,result,'artifact');return result;
  }
  if(operation==='search-arxiv-theorems'){
    const query=String(input.query||'').trim();if(!query||query.length>12000)throw new Error('Supply only the necessary statement, at most 12000 characters');
    const endpoint='https://leansearch.net/thm/search';const started=new Date().toISOString();
    try{const response=await (options.fetch||fetch)(endpoint,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({query,task:'Given a math statement, retrieve useful references, such as theorems, lemmas, and definitions, that are useful for solving the given problem.',num_results:Math.min(10,Math.max(1,Number(input.num_results)||5))}),signal:AbortSignal.timeout(Math.max(1,Math.min(30000,remaining)))});if(!response.ok)throw new Error('HTTP '+response.status);const data=await response.json();if(!Array.isArray(data))throw new Error('Invalid search response');return {status:'retrieved_unverified',endpoint,started,query,results:data.slice(0,10).map(r=>({title:String(r.title||''),theorem:String(r.theorem||''),arxiv_id:String(r.arxiv_id||''),theorem_id:String(r.theorem_id||'')}))};}catch(e){return {status:'inconclusive',endpoint,started,query,reason:e.message,results:[]};}
  }
  if(['memory-init','memory-append','memory-query'].includes(operation)){
    const dir=path.join(cwd,'.verification-memory');await fs.mkdir(dir,{recursive:true});await inside(cwd,dir);
    if(operation==='memory-init')return {channels:[...channels],path:dir};
    if(!channels.has(input.channel))throw new Error('Unknown review channel');const file=path.join(dir,input.channel+'.jsonl');
    try{await inside(cwd,file);}catch(e){if(e.code!=='ENOENT')throw e;}
    if(operation==='memory-append'){if(!input.record||typeof input.record!=='object'||Array.isArray(input.record))throw new Error('Record must be an object');await fs.appendFile(file,JSON.stringify({at:new Date().toISOString(),record:input.record})+'\n');return {saved:true};}
    const text=await fs.readFile(file,'utf8').catch(e=>{if(e.code==='ENOENT')return '';throw e;});return {records:text.split('\n').filter(Boolean).map(s=>JSON.parse(s)).filter(r=>!input.contains||JSON.stringify(r).includes(input.contains)).slice(-Math.min(100,Math.max(1,Number(input.limit)||50)))};
  }
  throw new Error('Unknown operation');
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)){
  const [operation,inputFile]=process.argv.slice(2);
  if(!operation||operation==='--help')console.log('Usage: node .mathcat-tools.mjs OPERATION [input.json]. Operations: context-summary {}; search-memory {query,mode?:facts|experiences}; search-facts {query}; search-experiences {query,problem_version?}; read-statement {id,revision?,offset?,length?}; read-evidence {id,offset?,length?}; search-arxiv-theorems {query,num_results?}; memory-init {}; memory-append {channel,record}; memory-query {channel,contains?,limit?}. Statements retain exact assumptions and quantifiers; summaries are not certified. Read journals are untrusted hints, never proof of host delivery. Input file must be within this session. No mathematical computation tool. Write the final verification envelope to the host-provided control file; the host validates it.');
  else try{const cwd=await fs.realpath(process.cwd());const input=inputFile?JSON.parse(await fs.readFile(await inside(cwd,path.resolve(inputFile)),'utf8')):{};console.log(JSON.stringify(await execute(operation,input),null,2));}catch(e){console.error(JSON.stringify({status:'error',message:e.message}));process.exitCode=1;}
}
