import fs from 'node:fs/promises';
import path from 'node:path';
import crypto from 'node:crypto';
import {fileURLToPath} from 'node:url';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const baseline=JSON.parse(await fs.readFile(path.join(root,'docs/baselines/2.4.3-source.json'),'utf8'));
const previous=path.resolve(root,'..','MathCat-Lab-2.4.3');
const changed=[];
if(!baseline.files.length)throw new Error('Source manifest must not be empty.');
for(const record of baseline.files){
  const source=path.resolve(previous,record.path);
  if(!source.startsWith(previous+path.sep))throw new Error('Invalid source manifest path.');
  const bytes=await fs.readFile(source).catch(()=>null);
  if(!bytes||crypto.createHash('sha256').update(bytes).digest('hex')!==record.sha256)changed.push(record.path);
}
const result={version:'2.5.3',checked_at:new Date().toISOString(),baseline:'docs/baselines/2.4.3-source.json',checked_files:baseline.files.length,unchanged:changed.length===0,changed};
await fs.mkdir(path.join(root,'tests/results'),{recursive:true});
await fs.writeFile(path.join(root,'tests/results/previous-version-isolation.json'),JSON.stringify(result,null,2));
console.log(JSON.stringify(result));if(changed.length)process.exitCode=1;
