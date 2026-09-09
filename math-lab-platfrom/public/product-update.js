const labels={checking:'正在确认最新版本…',downloading:'正在下载安装包…',extracting:'正在校验并解压安装包…',preparing:'正在准备新版程序，首次编译可能需要几分钟…',restarting:'正在重启并连接新版…',completed:'更新完成。',failed:'更新失败'};
export function versionMessage(value){return value.state==='current'?'已经是最新版本':value.state==='ahead'?'本地版本较新，无需更新':value.state==='available'?'发现新版本':value.error||'正在检查 GitHub 最新版本…';}
export function mountProductUpdate({button,dialog,fetchImpl=fetch}){
  let latest=null,polling=false;
  const find=name=>dialog.querySelector(`[data-update-${name}]`);
  async function request(url,options){const response=await fetchImpl(url,{cache:'no-store',...options});const value=await response.json();if(!response.ok)throw new Error(value.error||'请求失败');return value;}
  function message(text,error=false){find('message').textContent=text;find('message').classList.toggle('is-error',error);}
  async function check(){
    find('check').disabled=true;find('install').hidden=true;find('latest').textContent='查询中…';message('正在检查 GitHub 最新版本…');
    try{const value=await request('/api/product-update/check');latest=value;find('local').textContent=value.localVersion;find('latest').textContent=value.latestVersion?(value.latestVersion+(value.prerelease?' · 预发布':'')):'获取失败';message(versionMessage(value),value.state==='error');find('reason').textContent=value.reason||'';find('install').hidden=!value.canUpdate;find('release').hidden=!value.releaseUrl;if(value.releaseUrl)find('release').href=value.releaseUrl;}
    catch(error){message(error.message,true);find('latest').textContent='获取失败';}finally{find('check').disabled=false;}
  }
  async function follow(){
    if(polling)return;polling=true;find('check').disabled=true;find('install').hidden=true;const started=Date.now();let disconnectedAt=null;
    try{while(Date.now()-started<30*60*1000){
      try{const job=await request('/api/product-update/status');
        if(job.state==='failed'){message(job.error||'更新失败，请重试。',true);return;}
        if(job.state==='idle'){const health=await request('/api/health');if(health.version===latest?.latestVersion){location.reload();return;}await check();return;}
        message((labels[job.state]||'正在更新…')+(job.state==='downloading'?` ${job.progress||0}%`:''));
        if(job.state==='completed'){location.reload();return;}
        if(job.state==='restarting')disconnectedAt??=Date.now();
      }catch{disconnectedAt??=Date.now();message('正在重启并连接新版…');}
      if(disconnectedAt){try{const health=await request('/api/health');if(health.version===latest?.latestVersion){location.reload();return;}}catch{}if(Date.now()-disconnectedAt>120000){message('暂时无法连接新版。请使用新版目录的启动器重试；更新日志保存在原版本 runtime/updates。',true);return;}}
      await new Promise(resolve=>setTimeout(resolve,1200));
    }message('更新准备时间较长，可关闭此页，稍后重新打开查看进度。');}
    finally{polling=false;find('check').disabled=false;}
  }
  button.addEventListener('click',async()=>{dialog.showModal();try{const job=await request('/api/product-update/status');if(!['idle','failed','completed'].includes(job.state)){latest={latestVersion:job.targetVersion};await follow();return;}}catch{}await check();});
  find('close').addEventListener('click',()=>dialog.close());find('check').addEventListener('click',check);
  find('install').addEventListener('click',async()=>{find('install').disabled=true;try{await request('/api/product-update/install',{method:'POST',headers:{'Content-Type':'application/json','X-MathCat-Update':'1'},body:JSON.stringify({version:latest.latestVersion})});await follow();}catch(error){message(error.message,true);}finally{find('install').disabled=false;}});
}
