import fs from 'node:fs';
import path from 'node:path';
import {pathToFileURL} from 'node:url';
import YAML from 'yaml';
const slsa='slsa-framework/slsa-github-generator/.github/workflows/generator_generic_slsa3.yml@v2.1.0';
export function loadWorkflows(root='.github/workflows') {return Object.fromEntries(fs.readdirSync(root).filter(f=>f.endsWith('.yml')).map(f=>[f,YAML.parse(fs.readFileSync(path.join(root,f),'utf8'))]));}
export function validate(workflows) {
 const errors=[];const check=(value,message)=>{if(!value)errors.push(message);};
 for(const [file,w] of Object.entries(workflows)) {
  check(JSON.stringify(w.permissions)==='{"contents":"read"}',file+': default token must be read-only');
  check(!w.on?.pull_request_target,file+': pull_request_target is forbidden');
  const referenced=[...JSON.stringify(w.jobs).matchAll(/secrets\.([A-Z_]+)/g)].map(m=>m[1]);
  for(const key of referenced)check(Object.hasOwn(w.on?.workflow_call?.secrets||{},key),file+': environment secret requires explicit reusable contract: '+key);
  check(file!=='ci.yml'||!w.on?.workflow_call?.secrets,file+': CI cannot declare release secrets');
  for(const [name,j] of Object.entries(w.jobs||{})) {
   const label=file+'/'+name, body=JSON.stringify(j), scripts=(j.steps||[]).map(s=>s.run||'').join('\n');
   if(j.uses){check(j.uses===slsa || /^\$\/\.github\/workflows\/\w[\w-]*\.yml$/.test(j.uses),label+': reusable workflow must use pinned platform or generator exception');continue;}
   check(Number.isInteger(j['timeout-minutes']) && j['timeout-minutes']<=90,label+': bounded timeout required');
   check(!body.includes('self-hosted'),label+': hosted runners required');
   check(!/\$\{\{\s*(inputs\.|github\.event\.)/.test(scripts),label+': event/input values must use environment variables');
   for(const s of j.steps||[])if(s.uses){
    check(/@[a-f0-9]{40}$/.test(s.uses)||/^\$\/actions\/[a-z-]+$/.test(s.uses),label+': action must be pinned by SHA or self revision');
    if(s.uses.startsWith('actions/checkout@'))check(s.with?.['persist-credentials']===false,label+': checkout credentials must not persist');
    check(!s.uses.startsWith('actions/cache@'),label+': opaque executable caches forbidden');
   }
   if(file==='ci.yml') {check(!body.includes('secrets.')&&!j.environment&&!j.permissions?.['id-token'],label+': PR path must not receive secrets or privilege');}
   if(/fastlane\.rb" (build|test|analyze)|swiftlint lint/.test(scripts))check(!j.permissions?.['id-token']&&!j.permissions?.attestations,label+': compilation must not sign provenance');
   if(/fastlane\.rb" build/.test(scripts))check(j.environment==='signing'&&!body.includes('APP_STORE_CONNECT_API_KEY'),label+': compilation signing boundary');
   if(/fastlane\.rb" submit/.test(scripts))check(j.environment==='production'&&scripts.includes('fetch.py')&&j.concurrency?.['cancel-in-progress']===false,label+': review requires production approval and reauthentication');
   if(j.environment && !['signing','app-store-observe'].includes(j.environment))check(!/bundle exec fastlane|scripts\/ci\/download_release/.test(scripts),label+': privileged job may not execute consumer Fastlane');
  }
 }
 const prepare=workflows['prepare.yml'];
 check(![].concat(prepare.jobs.build.needs).includes('qa'),'Archive must run alongside QA');
 check(!prepare.jobs.upload&&!prepare.jobs.publish,'Preparation must not distribute');
 check(workflows['promote.yml'].jobs.upload.needs==='verify','Upload must depend on verification');
 check(workflows['ci.yml'].jobs.gate.if==='always()','CI gate must always report');
 return errors;
}
if(process.argv[1]&&import.meta.url===pathToFileURL(process.argv[1]).href){const errors=validate(loadWorkflows());if(errors.length){console.error(errors.join('\n'));process.exitCode=1;}else console.log('Workflow boundary policy passed.');}
