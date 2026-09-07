import {escapeHtml as h} from './research-v2-state.js';

const list=value=>Array.isArray(value)?value:[];
const numeric=value=>typeof value==='number'&&Number.isFinite(value)?value:null;
const percent=value=>numeric(value)===null?null:Math.max(0,Math.min(100,value));
const contextKey=id=>id?'project:'+id:'platform';
const successful=value=>['ok','available','ready','configured'].includes(value);
const modelsAvailable=status=>successful(status?.models_status)||(status?.models_status==='partial'&&!status.models_stale);
export function durationLabel(minutes){
  const value=numeric(minutes);if(value===null||value<=0)return '时长未提供';
  if(value%1440===0)return value/1440+' 天';
  if(value%60===0)return value/60+' 小时';
  return value+' 分钟';
}
export function reasoningEffortsFor(model){
  return list(model?.supported_reasoning_efforts).map(item=>typeof item==='string'?{effort:item,description:''}:{effort:item?.effort,description:item?.description||''}).filter(item=>typeof item.effort==='string'&&item.effort);
}
export function normalizeCodexStatus(value={},previous=null){
  const models=list(value.models).map(item=>({...item,id:item.id||item.model,display_name:item.display_name||item.model||item.id,supported_reasoning_efforts:reasoningEffortsFor(item)})).filter(item=>item.id);
  const rates=list(value.rate_limits).map(group=>({id:group.id,name:group.name,windows:list(group.windows).map(window=>{
    const used=percent(window.used_percent),remaining=percent(window.remaining_percent);
    return {name:window.name,used_percent:used,remaining_percent:remaining??(used===null?null:100-used),window_minutes:numeric(window.window_minutes),resets_at:numeric(window.resets_at)};
  })}));
  const modelsStatus=value.models_status||value.status||'unavailable',ratesStatus=value.rate_limits_status||value.status||'unavailable';
  const lastUpdated=value.updated_at||previous?.updated_at||null;
  return {...value,status:value.status||'unavailable',updated_at:lastUpdated,models_status:modelsStatus,rate_limits_status:ratesStatus,
    models_updated_at:value.models_updated_at||previous?.models_updated_at||lastUpdated,rate_limits_updated_at:value.rate_limits_updated_at||previous?.rate_limits_updated_at||lastUpdated,
    models:models.length||successful(modelsStatus)?models:previous?.models||[],rate_limits:rates.length||successful(ratesStatus)?rates:previous?.rate_limits||[],
    selection:value.selection||previous?.selection||null,active_calls:list(value.active_calls)};
}
export function rateLimitRows(status){
  return list(status?.rate_limits).flatMap(group=>list(group.windows).map((window,index)=>({...window,key:(group.id||group.name||'quota')+':'+index,group:group.name||group.id||'Codex',duration:durationLabel(window.window_minutes)})));
}
export function selectionSummary(status){
  const selection=status?.selection;if(!selection?.model)return '模型暂不可用';
  const model=list(status.models).find(item=>item.id===selection.model||item.model===selection.model);
  return (['unavailable','stale'].includes(status.status)?'暂不可用 · 上次模型：':'')+(model?.display_name||selection.model)+(selection.reasoning_effort?' · '+selection.reasoning_effort:' · 推理强度未提供');
}
export function quotaSummary(status){
  const rows=rateLimitRows(status),unavailable=Boolean(status?.rate_limits_stale)||!successful(status?.rate_limits_status);
  const values=rows.map(row=>row.group+' '+row.duration+'剩余 '+(row.remaining_percent===null?'未提供':Number(row.remaining_percent.toFixed(1))+'%'));
  if(!values.length)return '额度暂不可用';
  return (unavailable?'额度暂不可用 · 上次：':'')+values.join(' · ');
}
export function weeklyQuotaSummary(status){
  const groups=list(status?.rate_limits),weekly=group=>list(group?.windows).filter(window=>window.window_minutes===10080);
  const shared=weekly(groups.find(group=>group.id==='codex')),all=groups.flatMap(weekly);
  const window=shared.length===1?shared[0]:all.length===1?all[0]:null;
  const remaining=percent(window?.remaining_percent),stale=Boolean(status?.rate_limits_stale)||!successful(status?.rate_limits_status);
  const reset=numeric(window?.resets_at),date=reset===null?null:new Date(reset*1000);
  return {remaining:remaining===null?'每周额度暂不可用':(stale?'每周剩余（上次）':'每周剩余')+' '+Number(remaining.toFixed(1))+'%',reset:date&&!Number.isNaN(date.valueOf())?'重置：'+date.toLocaleString('zh-CN',{month:'numeric',day:'numeric',hour:'2-digit',minute:'2-digit',hour12:false}):'重置时间未提供'};
}
export function selectedModelPayload(status,draft,projectId){
  const model=list(status?.models).find(row=>row.id===draft?.model||row.model===draft?.model);
  if(!model)throw new Error('请从已读取的可用模型列表中选择。');
  if(!reasoningEffortsFor(model).some(row=>row.effort===draft.reasoning_effort))throw new Error('请选择该模型明确支持的推理强度。');
  if(projectId&&status.selection?.project_id&&status.selection.project_id!==projectId)throw new Error('项目设置已经切换，请刷新后再保存。');
  return {...(projectId?{project_id:projectId}:{}),model:model.model||model.id,reasoning_effort:draft.reasoning_effort,
    ...(Number.isInteger(status.selection?.revision)?{expected_revision:status.selection.revision}:{})};
}
const displayTime=value=>{const date=value?new Date(value):null;return date&&!Number.isNaN(date.valueOf())?date.toLocaleString('zh-CN',{hour12:false}):'尚无成功获取记录';};
const roleName=role=>({main:'领研猫',partner:'伙伴猫',reviewer:'审核猫',advisor:'研究顾问猫',memory:'记忆猫',display:'展示猫',writer:'论文整理',discussion:'讨论解释'}[role]||role||'模型调用');

export class CodexStatusController{
  constructor({summaryElement=null,panelElement=null,fetchImpl=fetch,storage=globalThis.sessionStorage,onChange=()=>{}}={}){
    Object.assign(this,{summaryElement,panelElement,storage,onChange});this.fetchImpl=(...args)=>fetchImpl.call(globalThis,...args);this.readScopes=new Set();this.cache=new Map();this.inflight=new Map();this.pending=new Map();this.context=null;this.draft=null;this.dirty=false;this.loading=false;this.saving=false;this.error='';this.notice='';
    this.mountPanel();this.render();
  }
  get current(){return this.cache.get(contextKey(this.context))||null;}
  getStatus(projectId=this.context){return this.cache.get(contextKey(projectId))||null;}
  getDraft(){const s=this.current?.selection;return s?{model:s.model||'',reasoning_effort:s.reasoning_effort||''}:{model:'',reasoning_effort:''};}
  setContext(projectId=null){
    const changed=this.context!==projectId;this.context=projectId;
    if(changed){this.dirty=false;this.error='';this.notice='';this.loading=false;this.saving=false;}
    const key=contextKey(projectId);
    if(!this.cache.has(key)){try{const saved=JSON.parse(this.storage?.getItem('mathcat.2.5.codex-status.'+key)||'null');if(saved)this.cache.set(key,{...saved,status:'stale',models_status:'stale',rate_limits_status:'stale'});}catch{}}
    if(changed||!this.draft)this.draft=this.getDraft();this.render();
    if(changed||!this.readScopes.has(key))void this.refresh();
  }
  resetDraft(){this.draft=this.getDraft();this.dirty=false;this.error='';this.notice='';this.render();}
  async refresh({force=false,projectId=this.context}={}){
    const key=contextKey(projectId);if(this.inflight.has(key))return this.inflight.get(key);this.readScopes.add(key);
    if(projectId===this.context){this.loading=true;this.render();}
    const request=(async()=>{
      try{
        const query=[...(force?['refresh=1']:[]),...(projectId?['project_id='+encodeURIComponent(projectId)]:[])].join('&');
        const response=await this.fetchImpl('/api/codex/status'+(query?'?'+query:''));const value=await response.json();
        if(!response.ok)throw Object.assign(new Error(value.error?.message||value.error||'Codex 状态暂不可用'),{status:response.status});
        if(projectId&&value.selection?.project_id&&value.selection.project_id!==projectId)throw new Error('返回的模型设置不属于当前项目。');
        const status=normalizeCodexStatus(value,this.cache.get(key));this.cache.set(key,status);
        const pending=this.pending.get(key);if(pending&&status.selection?.model===pending.body.model&&status.selection.reasoning_effort===pending.body.reasoning_effort)this.pending.delete(key);
        this.remember(key,status);
        if(projectId===this.context){if(!this.dirty)this.draft=this.getDraft();this.error=value.error?.message||value.error||'';this.onChange(status,projectId);}
        return status;
      }catch(error){
        const status=normalizeCodexStatus({status:'unavailable',models_status:'unavailable',rate_limits_status:'unavailable',error:error.message},this.cache.get(key));this.cache.set(key,status);
        if(projectId===this.context){this.error=error.message;this.onChange(status,projectId);}return status;
      }finally{this.inflight.delete(key);if(projectId===this.context){this.loading=false;this.render();}}
    })();this.inflight.set(key,request);return request;
  }
  remember(key,status){try{this.storage?.setItem('mathcat.2.5.codex-status.'+key,JSON.stringify({status:status.status,updated_at:status.updated_at,models_updated_at:status.models_updated_at,rate_limits_updated_at:status.rate_limits_updated_at,models_status:status.models_status,rate_limits_status:status.rate_limits_status,models:status.models,rate_limits:status.rate_limits,selection:status.selection}));}catch{}}
  async saveSelection(){
    if(this.saving)return;const projectId=this.context,key=contextKey(projectId),status=this.current;
    try{
      if(!modelsAvailable(status))throw new Error('模型列表暂不可用，请先刷新确认可用模型，再保存。');
      const body=selectedModelPayload(status,this.draft,projectId),signature=JSON.stringify(body),old=this.pending.get(key);
      if(old&&old.signature!==signature)throw new Error('上次保存的结果尚待确认，请先刷新状态或重试原选择。');
      const pending=old||{body,signature,key:crypto.randomUUID()};this.pending.set(key,pending);this.saving=true;this.error='';this.notice='';this.render();
      const response=await this.fetchImpl('/api/codex/model-selection',{method:'POST',headers:{'content-type':'application/json','idempotency-key':pending.key},body:JSON.stringify(pending.body)});
      const value=await response.json();
      if(!response.ok){if(response.status>=400&&response.status<500&&response.status!==408)this.pending.delete(key);throw Object.assign(new Error(value.error?.message||value.error||'模型设置未能保存'),{status:response.status});}
      if(projectId&&value.selection?.project_id&&value.selection.project_id!==projectId)throw new Error('返回的模型设置不属于保存的项目，请刷新确认。');
      this.pending.delete(key);const next=normalizeCodexStatus(value,status);this.cache.set(key,next);this.remember(key,next);
      if(projectId===this.context){this.dirty=false;this.draft=this.getDraft();this.notice=projectId?'已保存到当前项目，从下一次模型调用生效。':'已保存后续任务默认值，正在运行的项目不受影响。';this.onChange(next,projectId);}
      return next;
    }catch(error){if(projectId===this.context)this.error=error.message;return null;}
    finally{if(projectId===this.context){this.saving=false;this.render();}}
  }
  mountPanel(){
    const panel=this.panelElement;if(!panel)return;
    panel.innerHTML='<section class="codex-settings" aria-label="Codex 模型与额度"><div class="codex-settings-head"><h3>Codex 模型与额度</h3><button type="button" data-codex-refresh>刷新状态</button></div><p data-codex-scope class="codex-scope"></p><div class="codex-model-fields"><label>模型<select data-codex-model aria-label="Codex 模型"></select></label><label>推理强度<select data-codex-effort aria-label="Codex 推理强度"></select></label></div><p data-codex-effort-description class="codex-muted"></p><p class="codex-muted">新设置从下一次模型调用生效，不打断正在生成的内容，也不改变暂停或运行状态。</p><div data-codex-active-calls></div><div class="codex-save-row"><button type="button" data-codex-save>保存模型设置</button><span data-codex-save-status role="status"></span></div><p data-codex-error class="codex-error" role="alert"></p><div class="codex-quota-head"><h4>账号额度剩余</h4><small data-codex-updated></small></div><p class="codex-muted">额度属于当前 Codex 账号，由该账号的任务共享，不是本项目独占。不同额度池按各自适用范围计算。</p><div data-codex-quotas></div></section>';
    panel.querySelector('[data-codex-refresh]').onclick=()=>this.refresh({force:true});
    panel.querySelector('[data-codex-save]').onclick=()=>this.saveSelection();
    panel.querySelector('[data-codex-model]').onchange=event=>{
      const model=list(this.current?.models).find(item=>item.id===event.target.value||item.model===event.target.value),efforts=reasoningEffortsFor(model),previous=this.draft?.reasoning_effort;
      this.draft={model:event.target.value,reasoning_effort:efforts.some(item=>item.effort===previous)?previous:efforts.some(item=>item.effort===model?.default_reasoning_effort)?model.default_reasoning_effort:''};this.dirty=true;this.notice='';this.render();
    };
    panel.querySelector('[data-codex-effort]').onchange=event=>{this.draft={...this.draft,reasoning_effort:event.target.value};this.dirty=true;this.notice='';this.render();};
  }
  render(){
    const status=this.current;
    if(this.summaryElement){const weekly=weeklyQuotaSummary(status);this.summaryElement.innerHTML='<span>当前模型：'+h(selectionSummary(status))+'</span><span class="codex-summary-quota">'+h(weekly.remaining)+'</span><span class="codex-summary-quota">'+h(weekly.reset)+'</span>';this.summaryElement.title='额度获取时间：'+displayTime(status?.rate_limits_updated_at||status?.updated_at);}
    const panel=this.panelElement;if(!panel)return;
    panel.querySelector('[data-codex-scope]').textContent=this.context?'作用于当前研究项目。保存只修改本项目；其他项目和后续任务默认值保留。':'作用于后续新任务的默认设置。保存不会修改已有研究项目。';
    const models=list(status?.models),draft=this.draft||this.getDraft(),available=modelsAvailable(status)&&models.length>0,modelSelect=panel.querySelector('[data-codex-model]');
    const modelOptions=(!models.some(m=>m.id===draft.model||m.model===draft.model)?'<option value="'+h(draft.model)+'">'+h(draft.model?draft.model+'（当前设置，列表未提供）':available?'请选择模型':'模型列表暂不可用')+'</option>':'')+models.map(model=>'<option value="'+h(model.model||model.id)+'">'+h(model.display_name||model.model||model.id)+'</option>').join('');
    if(this.modelMarkup!==modelOptions){modelSelect.innerHTML=modelOptions;this.modelMarkup=modelOptions;}modelSelect.value=draft.model;modelSelect.disabled=!available||this.saving;
    const model=models.find(m=>m.id===draft.model||m.model===draft.model),efforts=reasoningEffortsFor(model),effortSelect=panel.querySelector('[data-codex-effort]');
    const effortOptions=(!efforts.some(e=>e.effort===draft.reasoning_effort)?'<option value="'+h(draft.reasoning_effort)+'">'+h(draft.reasoning_effort?draft.reasoning_effort+'（当前设置，支持情况待确认）':efforts.length?'请选择推理强度':'推理强度暂不可用')+'</option>':'')+efforts.map(e=>'<option value="'+h(e.effort)+'">'+h(e.effort)+'</option>').join('');
    if(this.effortMarkup!==effortOptions){effortSelect.innerHTML=effortOptions;this.effortMarkup=effortOptions;}effortSelect.value=draft.reasoning_effort;effortSelect.disabled=!available||!efforts.length||this.saving;
    panel.querySelector('[data-codex-effort-description]').textContent=efforts.find(e=>e.effort===draft.reasoning_effort)?.description||'';
    const refresh=panel.querySelector('[data-codex-refresh]');refresh.disabled=this.loading||this.saving;refresh.textContent=this.loading?'正在获取…':'刷新状态';
    const save=panel.querySelector('[data-codex-save]');save.disabled=!this.dirty||!available||this.saving||!efforts.some(e=>e.effort===draft.reasoning_effort);save.textContent=this.saving?'正在保存…':this.context?'保存到当前项目':'保存后续任务默认值';
    panel.querySelector('[data-codex-save-status]').textContent=this.notice||(this.dirty?'尚未保存':'');panel.querySelector('[data-codex-error]').textContent=this.error||'';
    panel.querySelector('[data-codex-updated]').textContent='额度获取时间：'+displayTime(status?.rate_limits_updated_at||status?.updated_at);
    const rows=rateLimitRows(status),stale=Boolean(status?.rate_limits_stale)||!successful(status?.rate_limits_status);
    panel.querySelector('[data-codex-quotas]').innerHTML=(stale?'<p class="codex-unavailable">额度暂不可用。'+(rows.length?'下面保留上次获取的数据。':'未把缺失额度当作零。')+'</p>':'')+(rows.length?rows.map(row=>'<article class="codex-quota-row"><div><strong>'+h(row.group+' · '+row.duration)+'</strong><span>'+h(row.remaining_percent===null?'剩余未提供':Number(row.remaining_percent.toFixed(1))+'% 剩余')+'</span></div>'+(row.remaining_percent!==null?'<meter min="0" max="100" value="'+row.remaining_percent+'" aria-label="'+h(row.duration+'额度剩余')+'"></meter>':'')+'<small>'+(row.resets_at===null?'重置时间未提供':'重置：'+h(displayTime(row.resets_at*1000)))+'</small></article>').join(''):'<p class="codex-muted">尚无可确认的额度窗口。</p>');
    const calls=panel.querySelector('[data-codex-active-calls]'),callsOpen=calls.querySelector('details')?.open;
    calls.innerHTML=list(status?.active_calls).length?'<details class="codex-active-calls" '+(callsOpen?'open':'')+'><summary>正在执行的研究调用保留原模型 · '+status.active_calls.length+'</summary>'+status.active_calls.map(call=>'<p>'+h(roleName(call.role)+' · '+(call.model||'模型未提供')+(call.reasoning_effort?' · '+call.reasoning_effort:''))+'</p>').join('')+'</details>':'';
  }
}
