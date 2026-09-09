import fs from 'node:fs/promises';
import path from 'node:path';
import {createHash,randomUUID} from 'node:crypto';
import {spawn} from 'node:child_process';
import {pipeline} from 'node:stream/promises';
import {Readable,Transform} from 'node:stream';

const repository='https://github.com/szys1169/mathcat-lab';
const api='https://api.github.com/repos/szys1169/mathcat-lab/releases?per_page=100';
const headers={'User-Agent':'MathCat-Lab-Updater',Accept:'application/vnd.github+json'};
const problem=(message,status=409)=>Object.assign(new Error(message),{status});
export function updateFetch(root){
  return async(url,options)=>{
    try{return await fetch(url,options);}catch(error){
      if(process.platform!=='win32'||options?.signal?.aborted)throw error;
      // Windows PowerShell uses the user's system proxy, including PAC settings.
      const directory=path.join(root,'runtime','updates','network');await fs.mkdir(directory,{recursive:true});
      const file=path.join(directory,randomUUID());
      try{await runCommand('powershell.exe',['-NoProfile','-ExecutionPolicy','Bypass','-File',path.join(root,'scripts','download-update.ps1'),'-Uri',url,'-OutputFile',file]);return new Response(await fs.readFile(file));}
      finally{await fs.rm(file,{force:true});}
    }
  };
}
export function compareVersions(a,b){
  const parse=value=>{const match=/^v?(\d+)\.(\d+)\.(\d+)$/.exec(value);if(!match)throw problem('发布版本号格式不支持。');return match.slice(1).map(Number);};
  const left=parse(a),right=parse(b);for(let i=0;i<3;i++)if(left[i]!==right[i])return left[i]>right[i]?1:-1;return 0;
}
export function selectRelease(releases,local,platform=process.platform,arch=process.arch){
  if(!Array.isArray(releases))throw problem('GitHub 返回的版本列表无效。',502);
  const release=releases.filter(r=>!r.draft&&/^v?\d+\.\d+\.\d+$/.test(r.tag_name)).sort((a,b)=>compareVersions(b.tag_name,a.tag_name))[0];
  if(!release)throw problem('GitHub 尚无可识别的发布版本。',502);
  const latest=release.tag_name.replace(/^v/,''),comparison=compareVersions(local,latest);
  const suffix=platform==='win32'&&arch==='x64'?'Windows-x64.zip':platform==='darwin'?'macOS-universal-source.tar.gz':null;
  const name=suffix?`MathCat-Lab-${latest}-${suffix}`:null;
  const asset=release.assets?.find(a=>a.name===name);
  const expected=`${repository}/releases/download/${release.tag_name}/${name}`;
  const usable=asset?.browser_download_url===expected&&/^sha256:[a-f0-9]{64}$/.test(asset.digest||'')&&asset.size>0&&asset.size<300_000_000;
  return {localVersion:local,latestVersion:latest,prerelease:!!release.prerelease,releaseUrl:`${repository}/releases/tag/${release.tag_name}`,state:comparison===0?'current':comparison>0?'ahead':'available',canUpdate:comparison<0&&!!usable,reason:comparison<0&&!usable?'此系统暂无带完整性校验的安装包。':null,asset:usable?asset:null};
}
export function runCommand(command,args,options={}){
  return new Promise((resolve,reject)=>{const child=spawn(command,args,{shell:false,windowsHide:true,...options});let output='';child.stdout?.on('data',chunk=>{output+=chunk;if(output.length>8_000_000)child.kill();});child.stderr?.on('data',chunk=>{output+=chunk;if(output.length>8_000_000)child.kill();});child.once('error',reject);child.once('exit',code=>code===0?resolve(output):reject(new Error(`${path.basename(command)} 执行失败：${output.slice(-2000)}`)));});
}
export function validateArchiveEntries(listing){
  const entries=listing.split(/\r?\n/).filter(Boolean);
  if(!entries.length||entries.length>30000)throw problem('安装包文件列表无效。');
  for(const entry of entries){const normalized=entry.replaceAll('\\','/');if(normalized.startsWith('/')||normalized.includes(':')||normalized.split('/').includes('..'))throw problem('安装包包含不安全路径。');}
}
async function inspectTree(root){
  for(const entry of await fs.readdir(root,{withFileTypes:true})){if(entry.isSymbolicLink())throw problem('安装包含不支持的链接。');if(entry.isDirectory())await inspectTree(path.join(root,entry.name));}
}
export class ProductUpdater{
  constructor({root,localVersion,fetchImpl=fetch,assertIdle=async()=>{},activate,prepare=runCommand,platform=process.platform,arch=process.arch,dataPaths}){
    Object.assign(this,{root,localVersion,fetchImpl,assertIdle,activate,prepare,platform,arch,dataPaths});this.job={state:'idle'};this.busy=false;
    this.directory=path.join(root,'runtime','updates');this.statusFile=path.join(this.directory,'status.json');
  }
  async check(){
    try{const response=await this.fetchImpl(api,{headers,signal:AbortSignal.timeout(15000)});if(!response.ok)throw new Error(response.status===403||response.status===429?'GitHub 请求限额已到，请稍后重试。':`GitHub HTTP ${response.status}`);const result=selectRelease(await response.json(),this.localVersion,this.platform,this.arch);const {asset,...visible}=result;return visible;}
    catch(error){return {localVersion:this.localVersion,latestVersion:null,state:'error',canUpdate:false,error:`无法检查更新：${error.message}`};}
  }
  async status(){try{const persisted=JSON.parse(await fs.readFile(this.statusFile,'utf8'));if(!this.busy)this.job=persisted;else if(this.job.state==='restarting'&&persisted.state==='failed'){this.job=persisted;this.busy=false;}}catch{}return this.job;}
  async set(state,extra={}){this.job={...this.job,state,...extra};await fs.mkdir(this.directory,{recursive:true});const temp=this.statusFile+'.tmp';await fs.writeFile(temp,JSON.stringify(this.job));await fs.rename(temp,this.statusFile);}
  async start(version){
    if(this.busy)return this.job;
    this.busy=true;
    try{await this.assertIdle();await this.set('checking',{targetVersion:version,error:null});}
    catch(error){this.busy=false;throw error;}
    this.work=this.install(version).catch(async error=>{try{await this.set('failed',{error:error.message});}catch{this.job={...this.job,state:'failed',error:error.message};}finally{this.busy=false;}});return this.job;
  }
  async install(version){
    const response=await this.fetchImpl(api,{headers,signal:AbortSignal.timeout(15000)});if(!response.ok)throw problem(`GitHub HTTP ${response.status}`);
    const release=selectRelease(await response.json(),this.localVersion,this.platform,this.arch);
    if(!release.canUpdate||release.latestVersion!==version)throw problem('可用版本已改变，请重新检查更新。');
    const destination=path.join(path.dirname(this.root),`MathCat-Lab-${version}`);
    try{await fs.lstat(destination);throw problem('目标版本目录已存在，未覆盖。请从已有版本的启动器打开，或处理目录后重试。');}catch(error){if(error.code!=='ENOENT')throw error;}
    const stage=path.join(this.directory,randomUUID());await fs.mkdir(stage,{recursive:true});
    const archive=path.join(stage,release.asset.name);
    await this.set('downloading',{progress:0});
    const download=await this.fetchImpl(release.asset.browser_download_url,{headers:{'User-Agent':headers['User-Agent']},signal:AbortSignal.timeout(180000)});
    if(!download.ok||!download.body)throw problem(`下载安装包失败（${download.status}）。`);
    let bytes=0;const hash=createHash('sha256');
    const counter=new Transform({transform:(chunk,encoding,callback)=>{bytes+=chunk.length;if(bytes>release.asset.size)return callback(new Error('安装包大小超过发布记录。'));hash.update(chunk);this.job.progress=Math.floor(bytes/release.asset.size*100);callback(null,chunk);}});
    const handle=await fs.open(archive,'wx');await pipeline(Readable.fromWeb(download.body),counter,handle.createWriteStream());
    if(bytes!==release.asset.size||`sha256:${hash.digest('hex')}`!==release.asset.digest)throw problem('安装包完整性校验失败，未安装。');
    await this.set('extracting',{progress:100});
    validateArchiveEntries(await this.prepare('tar',['-tf',archive]));
    const details=await this.prepare('tar',['-tvf',archive]);if(details.split(/\r?\n/).some(line=>/^[lhbcps]/.test(line)))throw problem('安装包包含链接或特殊文件。');
    const unpack=path.join(stage,'unpacked');await fs.mkdir(unpack);await this.prepare('tar',['-xf',archive,'-C',unpack]);await inspectTree(unpack);
    let packageRoot=unpack;try{await fs.access(path.join(packageRoot,'release.json'));}catch{const entries=await fs.readdir(unpack,{withFileTypes:true});if(entries.length!==1||!entries[0].isDirectory())throw problem('无法识别安装包目录。');packageRoot=path.join(unpack,entries[0].name);}
    const manifest=JSON.parse(await fs.readFile(path.join(packageRoot,'release.json'),'utf8'));
    if(manifest.product!=='MathCat Lab'||manifest.version!==version||(await fs.readFile(path.join(packageRoot,'VERSION'),'utf8')).trim()!==version)throw problem('安装包版本与发布记录不符。');
    if(manifest.updaterProtocol!==1)throw problem('此安装包尚不支持保留数据的自动更新，请使用发布页的安装说明。');
    for(const name of ['runtime','workspaces','math-lab-platfrom/runtime-data','math-lab-platfrom/.env.local']){try{await fs.access(path.join(packageRoot,name));throw problem('安装包混入了运行数据。');}catch(error){if(error.code!=='ENOENT')throw error;}}
    await this.set('preparing');
    await this.prepare(this.platform==='win32'?'npm.cmd':'npm',['ci','--ignore-scripts'],{cwd:path.join(packageRoot,'math-lab-platfrom'),shell:this.platform==='win32'});
    if(this.platform==='darwin')await this.prepare('cargo',['build','--release','--bin','mathcat-v2'],{cwd:path.join(packageRoot,'math-research-mvp')});
    await fs.access(path.join(packageRoot,'math-research-mvp','target','release',this.platform==='win32'?'mathcat-v2.exe':'mathcat-v2'));
    await fs.writeFile(path.join(packageRoot,'update-data.json'),JSON.stringify(this.dataPaths));
    await fs.mkdir(path.join(packageRoot,'runtime'));await fs.copyFile(this.dataPaths.tokenFile,path.join(packageRoot,'runtime','api-token'));
    try{await fs.copyFile(path.join(this.root,'math-lab-platfrom','.env.local'),path.join(packageRoot,'math-lab-platfrom','.env.local'));}catch(error){if(error.code!=='ENOENT')throw error;}
    await this.assertIdle();
    await fs.rename(packageRoot,destination);
    await this.set('restarting',{destination});
    await this.activate({destination,statusFile:this.statusFile});
  }
}
