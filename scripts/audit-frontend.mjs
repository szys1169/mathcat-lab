import fs from 'node:fs/promises';
import path from 'node:path';
import crypto from 'node:crypto';
import {spawnSync} from 'node:child_process';
const root=path.resolve(import.meta.dirname,'..');
const previous=path.resolve(root,'..','MathCat-Lab-2.4.3');
const baseline=JSON.parse(await fs.readFile(path.join(root,'docs/baselines/2.4.3-source.json'),'utf8'));
const files=baseline.files.filter(f=>f.path.startsWith('math-lab-platfrom/public/'));
if(!files.length)throw new Error('Frontend manifest must not be empty.');
const previousChanges=[],syntaxFailures=[],changes=[];
const known=new Set(files.map(f=>f.path));
for(const f of files){
  const before=await fs.readFile(path.join(previous,f.path));
  if(crypto.createHash('sha256').update(before).digest('hex')!==f.sha256)previousChanges.push(f.path);
  const after=await fs.readFile(path.join(root,f.path)).catch(()=>null);
  if(!after||!after.equals(before))changes.push({path:f.path,kind:after?'modified':'removed'});
}
async function inspect(relative){
  for(const entry of await fs.readdir(path.join(root,relative),{withFileTypes:true})){
    const target=relative+'/'+entry.name;
    if(entry.isSymbolicLink()){syntaxFailures.push({path:target,error:'Source symlink is not permitted'});continue;}
    if(entry.isDirectory()){await inspect(target);continue;}
    if(!known.has(target))changes.push({path:target,kind:'added'});
    if(target.endsWith('.js')&&!target.includes('/vendor/')){
      const result=spawnSync(process.execPath,['--check',path.join(root,target)],{encoding:'utf8',windowsHide:true});
      if(result.status!==0)syntaxFailures.push({path:target,error:result.stderr||result.error?.message});
    }
  }
}
await inspect('math-lab-platfrom/public');
const result={version:'2.5.3',policy:'Authorized role naming and version-label changes in 2.5.3; this receipt checks immutable 2.4.3 frontend and syntax only. Behavior requires separate platform and browser receipts.',passed:!previousChanges.length&&!syntaxFailures.length,previous_files_checked:files.length,previous_changes:previousChanges,syntax_failures:syntaxFailures,changes};
await fs.mkdir(path.join(root,'tests/results'),{recursive:true});
await fs.writeFile(path.join(root,'tests/results/frontend-isolation.json'),JSON.stringify(result,null,2));
console.log(JSON.stringify(result));if(!result.passed)process.exitCode=1;
