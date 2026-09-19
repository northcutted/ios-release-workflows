// actionlint 1.7.12 predates GitHub's documented $/ self-repository syntax.
// Validate those references here, then normalize only their spelling for actionlint.
import fs from 'node:fs';import os from 'node:os';import path from 'node:path';import {execFileSync} from 'node:child_process';import YAML from 'yaml';
const root=process.cwd(), temp=fs.mkdtempSync(path.join(os.tmpdir(),'ios-actionlint-'));
try{
 const paths=[];
 for(const file of fs.readdirSync('.github/workflows').filter(f=>f.endsWith('.yml'))){
  const w=YAML.parse(fs.readFileSync('.github/workflows/'+file,'utf8'));
  for(const j of Object.values(w.jobs||{}))for(const s of [j,...(j.steps||[])])if(s.uses?.startsWith('$/')){
   const rel=s.uses.slice(2);
   if(rel.includes('..')||rel.includes('@')||!fs.existsSync(path.join(root,rel)+(rel.endsWith('.yml')?'':'/action.yml')))throw Error('Invalid self reference: '+s.uses);
   s.uses='./'+rel;
  }
  const p=path.join(temp,file);fs.writeFileSync(p,YAML.stringify(w));paths.push(p);
 }
 execFileSync('actionlint',['-config-file',path.join(root,'.github/actionlint.yaml'),...paths],{stdio:'inherit'});
}finally{fs.rmSync(temp,{recursive:true,force:true});}
