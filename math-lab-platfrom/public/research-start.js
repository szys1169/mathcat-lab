import {escapeHtml as h} from './research-v2-state.js';

export function researchStartOptions(value={}) {
  const mode=value.mode||({strict:'collaborative',balanced:'collaborative',automatic:'delegated'}[value.reviewMode]);
  const durationSeconds=Number(value.durationSeconds??value.duration_seconds??7200);
  const maxPartners=Number(value.maxPartners??value.max_partners??value.limits?.max_partners??5);
  if(!['delegated','collaborative',undefined].includes(mode))throw new Error('请选择本轮分工方式。');
  if(!Number.isFinite(durationSeconds)||durationSeconds<60||durationSeconds>604800)throw new Error('研究时间应在 1 分钟至 7 天之间。');
  if(!Number.isInteger(maxPartners)||maxPartners<0||maxPartners>20)throw new Error('伙伴猫上限应为 0 至 20 的整数。');
  return {mode:mode||'delegated',durationSeconds,maxPartners,autoDeliver:value.autoDeliver??value.auto_deliver??true};
}

// The dialog owns a temporary selection. Closing it never writes settings or starts work.
export function chooseResearchStart({initial={},title='开始 MathCat 研究',retry=false,locked=false,modelSummary='模型暂不可用',documentImpl=document}={}) {
  const options=researchStartOptions(initial),dialog=documentImpl.createElement('dialog');
  dialog.className='wb-dialog research-start-dialog';
  dialog.setAttribute('aria-label',title);
  dialog.innerHTML='<form><h2>'+h(title)+'</h2><p>本次使用当前问题与已明确选定的材料。数学成果仍由审核猫审查。</p>'+
    '<p class="research-start-model">'+(locked?'本次启动的冻结模型：':'当前模型设置：')+h(modelSummary)+'<br><small>'+(locked?'重试保留原启动参数。':'如需切换，请取消后打开模型设置。')+'</small></p>'+
    (retry?'<p class="wb-warning">这是上次启动的重试。使用原来的启动回执与实际参数，避免重复建立研究轮次。</p>':'')+
    '<fieldset '+(locked?'disabled':'')+'><legend>本轮分工方式</legend><label class="research-start-choice"><input type="radio" name="mode" value="delegated" '+(options.mode==='delegated'?'checked':'')+'><span><strong>自动安排分工</strong><small>领研猫自行安排伙伴猫；你仍可提出建议，必要的问题仍会等待你回答。</small></span></label>'+
    '<label class="research-start-choice"><input type="radio" name="mode" value="collaborative" '+(options.mode==='collaborative'?'checked':'')+'><span><strong>每轮分工由我批准</strong><small>领研猫提交安排后先等你批准；已有任务按当前规则继续。</small></span></label>'+
    '<div class="research-start-limits"><label>本轮时间上限（分钟）<input name="minutes" type="number" min="1" max="10080" step="1" required value="'+h(options.durationSeconds/60)+'"></label><label>伙伴猫上限<input name="partners" type="number" min="0" max="20" step="1" required value="'+h(options.maxPartners)+'"></label></div></fieldset>'+
    '<label class="research-start-delivery"><input type="checkbox" name="autoDeliver" '+(options.autoDeliver?'checked':'')+' '+(locked?'disabled':'')+'>研究成功后整理中英文稿，整理过程可暂停</label><p class="wb-muted">使用已登录的 Codex。等待人工与暂停期间，总截止时间继续计算；选择工作区不会自动导入全部文件。</p><p data-start-error class="wb-error" role="alert"></p><div class="wb-actions"><button type="button" data-start-cancel>取消</button><button type="submit" class="primary">'+(retry?'重试这次启动':'开始研究')+'</button></div></form>';
  documentImpl.body.append(dialog);
  return new Promise(resolve=>{
    let settled=false;
    const finish=value=>{if(settled)return;settled=true;dialog.close();dialog.remove();resolve(value);};
    dialog.querySelector('[data-start-cancel]').onclick=()=>finish(null);
    dialog.oncancel=e=>{e.preventDefault();finish(null);};
    dialog.onclose=()=>finish(null);
    dialog.querySelector('form').onsubmit=e=>{e.preventDefault();try{const form=e.currentTarget;finish(locked?options:researchStartOptions({mode:form.elements.mode.value,durationSeconds:Number(form.elements.minutes.value)*60,maxPartners:Number(form.elements.partners.value),autoDeliver:form.elements.autoDeliver.checked}));}catch(error){dialog.querySelector('[data-start-error]').textContent=error.message;}};
    dialog.showModal();
  });
}
