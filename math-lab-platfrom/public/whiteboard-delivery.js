import {escapeHtml as h,latestRun} from './research-v2-state.js';

const rows=value=>Array.isArray(value)?value:[];
export const deliveryStateLabel=value=>({queued:'等待整理',running:'正在处理',paused:'已暂停',pausing:'正在暂停',stopping:'正在停止',stopped:'已停止',pending:'尚未开始',waiting:'等待前一步',failed:'需要处理',completed:'已完成',passed:'已完成',interrupted:'执行已中断',skipped:'无需此步骤'}[value]||'状态待确认');
export const projectModelsPaused=project=>['pausing','paused','stopping','stopped'].includes(project.project_control?.state||project.delivery?.control?.state);
export function attentionItems(project) {
  const run=latestRun(project),items=[];
  for(const plan of rows(project.planning_proposals))if(plan.run_id===run?.id&&['pending','deferred'].includes(plan.state))items.push({id:'plan:'+plan.id,kind:'planning',label:'分工待批准',reason:plan.reason||'领研猫等待你处理本轮安排'});
  for(const q of rows(project.human_questions))if(q.run_id===run?.id&&['pending','open','waiting'].includes(q.status||q.state))items.push({id:'question:'+q.id,kind:'questions',label:'问题待回答',reason:typeof(q.body||q.question||q.text)==='string'?(q.body||q.question||q.text):'猫猫需要你的明确答复'});
  if(run?.state==='waiting_human'&&!items.some(i=>i.kind==='questions'))items.push({id:'run-question',kind:'questions',label:'研究等待答复',reason:typeof run.question==='string'?run.question:'请查看猫猫提问箱'});
  for(const step of rows(project.delivery?.current?.steps))if(['failed','interrupted'].includes(step.state))items.push({id:'delivery:'+step.id,kind:'delivery',label:step.label||'成果整理需要处理',reason:step.error||'此步骤未完成，可检查后局部重试'});
  if(project.project_control?.outstanding_cancellation)items.push({id:'cancellation',kind:'control',label:'仍有调用待确认停止',reason:'暂停请求已保存，等待执行器确认；尚不能显示全部已暂停'});
  if(run?.stop_reason==='environment_error')items.push({id:'environment',kind:'records',label:'研究执行异常',reason:'本轮因运行环境问题结束，保存的数学成果仍可查看'});
  for(const job of rows(project.background_jobs))if(job.run_id===run?.id&&job.status==='failed'&&job.error)items.push({id:'job:'+job.id,kind:'records',label:'后台任务异常',reason:typeof job.error==='string'?job.error:'请查看后台工作回执'});
  if(project.board_errors?.delivery)items.push({id:'delivery-read',kind:'delivery',label:'成果状态未能读取',reason:project.board_errors.delivery});
  return items;
}
export function attentionHtml(project) {
  const items=attentionItems(project);
  if(!items.length)return '';
  return '<section class="wb-attention" aria-label="需要你处理"><div><strong>需要你处理 · '+items.length+' 项</strong><span>点击事项前往对应位置</span></div><div class="wb-attention-items">'+items.map(item=>'<button type="button" data-wb-action="attention-jump" data-target="'+h(item.kind)+'" title="'+h(item.reason)+'"><strong>'+h(item.label)+'</strong><span>'+h(item.reason.slice(0,140))+'</span></button>').join('')+'</div></section>';
}
const action=(id,label,extra='')=>'<button type="button" data-wb-action="'+id+'" '+extra+'>'+h(label)+'</button>';
function fileUrl(project,file,download=false) {
  // Build local URLs from the file manifest, never accept an external executable link.
  return '/api/research-projects/'+encodeURIComponent(project.id)+'/files?path='+encodeURIComponent(file.path)+(download?'&download=1':'');
}
function fileLink(project,file,download=false,label=file.label||file.name) {
  return '<a href="'+h(fileUrl(project,file,download))+'" '+(download?'download':'target="_blank" rel="noopener"')+'>'+h(label||file.path)+'</a>';
}
export function deliveryHtml(project) {
  const data=project.delivery,current=data?.current,paused=projectModelsPaused(project),files=rows(current?.files);
  const folders='<div class="wb-actions wb-folder-actions">'+[['project','项目目录'],['results','成果目录'],['papers','论文目录'],['records','研究记录目录']].map(([folder,text])=>action('reveal-folder','打开'+text,'data-folder="'+folder+'"')).join('')+'</div>';
  const heading='<div class="wb-section-heading"><div><span class="wb-eyebrow">保存与交付</span><h3>成果</h3></div><span class="wb-badge">'+h(current?deliveryStateLabel(current.state):data?'尚未整理':'状态待读取')+'</span></div>';
  if(!data)return '<section class="wb-card wb-delivery">'+heading+'<p class="wb-warning">'+h(project.board_errors?.delivery||'正在读取已保存的成果状态。')+'</p>'+folders+'</section>';
  const byLanguage=['zh','en'].map(language=>{
    const relevant=files.filter(f=>f.language===language),pdf=relevant.find(f=>f.kind==='pdf'),tex=relevant.find(f=>f.kind==='tex');
    return '<article class="wb-delivery-language"><h4>'+(language==='zh'?'中文稿':'English manuscript')+'</h4><div class="wb-actions">'+(pdf?fileLink(project,pdf,false,'预览 PDF')+fileLink(project,pdf,true,'下载 PDF'):'<span class="wb-muted">PDF 尚未就绪</span>')+(tex?fileLink(project,tex,true,'下载 LaTeX 源文件'):'')+'</div>'+relevant.filter(f=>!['pdf','tex'].includes(f.kind)).map(f=>fileLink(project,f,true)).join(' ')+'</article>';
  }).join('');
  const steps=rows(current?.steps).map(step=>'<li class="wb-delivery-step '+h(step.state)+'"><div><strong>'+h(step.label||step.id)+'</strong><span>'+h(deliveryStateLabel(step.state))+'</span></div>'+(step.error?'<p class="wb-error">'+h(step.error)+'</p>':'')+(step.retryable?action('delivery-retry',step.state==='completed'?'重新生成英文稿':'仅重试这一步','data-step="'+h(step.id)+'" data-state="'+h(step.state)+'" '+(paused?'disabled title="先恢复本项目，才能重试模型任务"':'')):'')+'</li>').join('');
  return '<section class="wb-card wb-delivery">'+heading+
    '<p>'+(current?.kind==='stage_report'?'本轮按实际完成程度保存阶段报告，未解决的问题会明确保留。':current?'中英文稿基于同一份冻结成果整理；排版通过不改变原始数学可信状态。':'研究成功且已选择自动整理时，会开始准备中英文稿；也可明确整理当前成果。')+'</p>'+
    (current?.source_changed?'<p class="wb-warning">研究来源已有更新。当前稿件对应之前冻结的成果，不能当作最新研究结果。</p>':'')+
    (paused?'<p class="wb-pause-note">本项目已暂停新的模型任务，已保存的文件仍可预览与下载。</p>':'')+
    (steps?'<ol class="wb-delivery-steps">'+steps+'</ol>':'')+
    (current?.kind==='paper'?'<div class="wb-delivery-languages">'+byLanguage+'</div>':'')+
    '<div class="wb-delivery-files">'+files.filter(f=>!f.language||current?.kind!=='paper').map(f=>fileLink(project,f,f.kind!=='pdf')).join(' ')+'</div>'+
    rows(current?.warnings).map(w=>'<p class="wb-warning">'+h(w)+'</p>').join('')+
    ((!current||['completed','failed','stopped','interrupted'].includes(current.state))?'<div class="wb-actions">'+action('delivery-start',current?'按当前成果重新整理':'整理当前成果',''+(paused?'disabled title="本项目暂停中"':''))+'</div>':'')+
    '<details class="wb-delivery-records"><summary>研究资料与审查记录（'+rows(data.records).length+'）</summary>'+rows(data.records).map(f=>'<p>'+fileLink(project,f,f.kind!=='pdf')+'</p>').join('')+'</details>'+
    (rows(data.history).length?'<details><summary>此前交付 · '+data.history.length+' 次</summary>'+data.history.map(item=>'<article class="wb-entry"><strong>'+h(deliveryStateLabel(item.state))+'</strong><small>'+h(item.created_at||'')+'</small><p>'+rows(item.files).map(f=>fileLink(project,f,f.kind!=='pdf')).join(' ')+'</p></article>').join('')+'</details>':'')+folders+'</section>';
}
