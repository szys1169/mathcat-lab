import fs from 'node:fs/promises';
import path from 'node:path';
import {spawnSync} from 'node:child_process';
import {compareVersions} from '../math-lab-platfrom/src/product-update.mjs';
export async function forwardUpdate(root,action,args){
  let pointer;try{pointer=JSON.parse(await fs.readFile(path.join(root,'runtime','updates','installed.json'),'utf8'));}catch(error){if(error.code==='ENOENT')return;throw error;}
  const destination=path.resolve(pointer.destination),version=(await fs.readFile(path.join(root,'VERSION'),'utf8')).trim();
  if(path.dirname(destination)!==path.dirname(root)||path.basename(destination)!==`MathCat-Lab-${pointer.version}`||compareVersions(pointer.version,version)<=0)throw new Error('Invalid installed update path.');
  const result=spawnSync(process.execPath,[path.join(destination,'scripts',`${action}-version.mjs`),...args],{stdio:'inherit',shell:false});if(result.error)throw result.error;process.exit(result.status??1);
}
