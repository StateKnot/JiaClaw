'use strict';
// Administrator page evidence; not an Agent browser tool or supplier qualification.
const {chromium}=require('playwright');
const fs=require('fs'),os=require('os'),path=require('path'),http=require('http'),crypto=require('crypto'),child=require('child_process'),assert=require('assert');
(async()=>{
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'jiaclaw-semantic-review-browser-'));
 const workspace=path.join(root,'workspace');fs.mkdirSync(workspace);const source=path.join(workspace,'MEMORY.md');fs.writeFileSync(source,'private-source-text tea\n');
 const binary=path.resolve(process.argv[2]||'target/debug/jiaclaw'),token=crypto.randomUUID(),key='fixture-'+crypto.randomUUID();
 const env=Object.fromEntries(Object.entries(process.env).filter(([k])=>!k.startsWith('JIACLAW_')));env.JIACLAW_LOG_LEVEL='info';
 const servers=[];let browser,release;let modelCalls=0;const modelErrors=[];
 const gateway=http.createServer((req,res)=>{
  modelCalls++;let raw='';req.on('data',d=>raw+=d);req.on('end',()=>{
   try{assert.strictEqual(req.url,'/v1/embeddings');assert.strictEqual(req.headers.authorization,'Bearer '+key);assert(req.headers['idempotency-key']);
    const v=JSON.parse(raw);assert.strictEqual(v.model,'fixture-embedding');assert.strictEqual(v.dimensions,2);
    const result={object:'list',model:v.model,data:v.input.map((_,i)=>({object:'embedding',index:i,embedding:[1,0]})),usage:{prompt_tokens:v.input.length,total_tokens:v.input.length}};
    res.writeHead(200,{'content-type':'application/json','x-brokerrouter-request-id':crypto.randomUUID()});res.end(JSON.stringify(result));
   }catch(e){modelErrors.push(String(e));res.writeHead(500);res.end();}
  });
 });await new Promise(resolve=>gateway.listen(0,'127.0.0.1',resolve));
 function config(enabled){return {agent:{name:'semantic-review-browser',description:'Disposable fixture',workspace_path:workspace,max_turns:10,system_instructions:''},provider:{provider_type:'brokerrouter',base_url:'http://127.0.0.1:'+gateway.address().port,api_key:key,model:'must-not-chat'},memory:{path:'MEMORY.md',semantic:{enabled:true,model:'fixture-embedding',space_revision:'fixture-v1',dimensions:2,timeout_secs:2,index_path:'../state/semantic/index.sqlite3'}},http:{bind:'127.0.0.1:0',api_token:token,memory_review:true,memory_edit:true,semantic_review:enabled,persist:false,shutdown_timeout_secs:1}};}
 async function start(enabled){
  const cfg=path.join(root,'config-'+servers.length+'.json');fs.writeFileSync(cfg,JSON.stringify(config(enabled)));
  const process=child.spawn(binary,['serve','--config',cfg],{env}),record={process,log:''};servers.push(record);
  process.stdout.on('data',d=>record.log+=d);process.stderr.on('data',d=>record.log+=d);
  return await new Promise((resolve,reject)=>{const end=Date.now()+15000;function poll(){const m=record.log.match(/HTTP 服务已启动于 (http:\/\/127\.0\.0\.1:\d+)/);if(m)return resolve(m[1]);if(process.exitCode!==null||Date.now()>end)return reject(new Error(record.log));setTimeout(poll,20);}poll();});
 }
 async function stop(record){if(record.process.exitCode===null&&record.process.signalCode===null){const done=new Promise(resolve=>record.process.once('exit',resolve));record.process.kill('SIGTERM');let timer;await Promise.race([done,new Promise((_,reject)=>timer=setTimeout(()=>{record.process.kill('SIGKILL');reject(new Error('shutdown deadline'));},8000))]);clearTimeout(timer);}assert.strictEqual(record.process.exitCode,0,record.log);}
 try{
  const cfg=path.join(root,'refresh.json');fs.writeFileSync(cfg,JSON.stringify(config(true)));
  await new Promise((resolve,reject)=>{const p=child.spawn(binary,['memory','semantic','refresh','--config',cfg],{env});let log='';p.stdout.on('data',d=>log+=d);p.stderr.on('data',d=>log+=d);const timer=setTimeout(()=>{p.kill('SIGKILL');reject(new Error('refresh deadline'));},25000);p.once('exit',code=>{clearTimeout(timer);code===0?resolve():reject(new Error(log));});});
  const setupCalls=modelCalls;assert(setupCalls>0);assert.deepStrictEqual(modelErrors,[]);
  const base=await start(true);browser=await chromium.launch({headless:true,...(process.env.JIACLAW_TEST_CHROMIUM?{executablePath:process.env.JIACLAW_TEST_CHROMIUM}:{})});
  console.log('ACTUAL semantic review browser versions '+JSON.stringify({playwright:require('playwright/package.json').version,chromium:browser.version(),executable_override:process.env.JIACLAW_TEST_CHROMIUM||null}));
  const page=await browser.newPage({viewport:{width:1360,height:900}}),errors=[],requests=[];
  page.on('pageerror',e=>errors.push(String(e)));page.on('request',r=>{const p=new URL(r.url()).pathname;if(p.startsWith('/api/memory'))requests.push({path:p,method:r.method()});});
  const checks=()=>requests.filter(r=>r.path==='/api/memory/semantic').length;
  const idle=()=>page.waitForFunction(()=>!document.getElementById('refresh').disabled);
  async function connect(auth=token){await page.locator('#api-token').fill(auth);await page.locator('#connect-form button').click();if(auth===token)await idle();else await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));}
  async function inspect(){await page.locator('#semantic-check').click();await idle();}
  await page.goto(base);await connect();assert.strictEqual(checks(),0);await page.locator('#memory-tab').click();await idle();assert.strictEqual(checks(),0);
  assert(await page.locator('#semantic-review').isVisible());await inspect();assert.strictEqual(checks(),1);assert((await page.locator('#semantic-state').textContent()).includes('可由当前索引检索'));
  assert(!(await page.locator('#semantic-summary').textContent()).includes('private-source-text'));assert.strictEqual(modelCalls,setupCalls);
  await page.locator('#memory-candidate').fill('corrected saved memory');assert.strictEqual(await page.locator('#semantic-summary').textContent(),'');
  await page.locator('#memory-reviewed').check();await page.locator('#memory-save').click();await idle();assert.strictEqual(fs.readFileSync(source,'utf8'),'corrected saved memory');assert.strictEqual(checks(),1);
  await inspect();assert((await page.locator('#semantic-summary').textContent()).includes('索引陈旧'));assert.strictEqual(modelCalls,setupCalls);
  assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);
  await page.setViewportSize({width:390,height:844});
  const mobileFits=await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth);
  if(!mobileFits){console.log('ACTUAL semantic mobile overflow '+JSON.stringify(await page.evaluate(()=>Array.from(document.querySelectorAll('body *')).map(e=>({id:e.id,tag:e.tagName,right:e.getBoundingClientRect().right,width:e.getBoundingClientRect().width})).filter(e=>e.right>window.innerWidth+1))));await page.screenshot({path:path.join(root,'mobile-red.png'),fullPage:true});}
  assert(mobileFits);await page.setViewportSize({width:1360,height:900});
  console.log('PASS semantic review browser 1: actual explicit current check and saved correction then stale check, no automatic GET/model request/storage; mobile layout');

  await page.route('**/api/memory/semantic',async route=>{const response=await route.fetch(),v=await response.json();v.sources=['<img src=x onerror="window.BAD_SEMANTIC=1">'];await route.fulfill({response,json:v});});
  await inspect();assert.strictEqual(await page.evaluate(()=>typeof window.BAD_SEMANTIC),'undefined');assert((await page.locator('#semantic-summary').textContent()).includes('<img'));
  await page.unroute('**/api/memory/semantic');
  await page.route('**/api/memory/semantic',r=>r.fulfill({status:200,contentType:'application/json',body:'{"protocol_version":1,"freshness":"current","search_ready_at_capture":true}'}));
  await inspect();assert.strictEqual(await page.locator('#semantic-summary').textContent(),'');assert((await page.locator('#status').textContent()).includes('摘要不完整'));
  await page.unroute('**/api/memory/semantic');
  fs.writeFileSync(source,'private-source-text tea\n');
  const invalidGenerations=[
   ['missing',v=>delete v.generation],
   ['null-current',v=>v.generation=null],
   ['wrong-space',v=>v.generation.space='f'.repeat(64)],
   ['oversized-chunks',v=>v.generation.chunks=9999],
   ['negative-bytes',v=>v.generation.bytes=-1],
   ['oversized-bytes',v=>v.generation.bytes=8388609],
   ['current-chunk-mismatch',v=>v.generation.chunks=v.source_chunks+1]
  ];
  for(const [name,mutate] of invalidGenerations){
   await page.route('**/api/memory/semantic',async route=>{const response=await route.fetch(),v=await response.json();assert.strictEqual(v.freshness,'current');mutate(v);await route.fulfill({response,json:v});});
   await inspect();assert.strictEqual(await page.locator('#semantic-summary').textContent(),'',name);assert((await page.locator('#status').textContent()).includes('摘要不完整'),name);
   await page.unroute('**/api/memory/semantic');
  }
  fs.writeFileSync(source,'corrected saved memory');
  console.log('ACTUAL semantic generation parser: seven incomplete/inconsistent generation responses rejected');
  fs.writeFileSync(source,Buffer.from([255]));await inspect();assert.strictEqual(await page.locator('#semantic-summary').textContent(),'');assert.strictEqual(modelCalls,setupCalls);fs.writeFileSync(source,'corrected saved memory');
  console.log('PASS semantic review browser 2: textContent metadata cannot execute HTML; incomplete response and actual invalid UTF8 refusal clear prior snapshot without fallback');

  await inspect();let waiting=false;
  await page.route('**/api/memory/semantic',async route=>{const response=await route.fetch();waiting=true;await new Promise(resolve=>release=resolve);await route.fulfill({response});});
  await page.locator('#semantic-check').click();for(let i=0;!waiting&&i<200;i++)await new Promise(resolve=>setTimeout(resolve,10));assert(waiting);
  await connect('wrong');release();release=null;await page.waitForTimeout(100);assert.strictEqual(await page.locator('#semantic-summary').textContent(),'');assert(!await page.locator('#semantic-review').isVisible());
  await page.unroute('**/api/memory/semantic');await connect();assert.strictEqual(await page.locator('#semantic-summary').textContent(),'');assert.strictEqual(modelCalls,setupCalls);
  console.log('PASS semantic review browser 3: failed authentication/reconnect clears private metadata; delayed old identity response cannot restore it');

  for(const contract of [{scheduled_jobs:false},{scheduled_jobs:false,read_only:true}]){
   await page.route('**/api/turns/capabilities',r=>r.fulfill({status:404,body:''}));await page.route('**/api/gateway/capabilities',r=>r.fulfill({status:200,contentType:'application/json',body:JSON.stringify(contract)}));
   const before=requests.length;await connect();assert.strictEqual(requests.length,before);assert(!await page.locator('#semantic-review').isVisible());
   await page.unroute('**/api/gateway/capabilities');await page.unroute('**/api/turns/capabilities');
  }
  await page.route('**/api/memory/semantic-capabilities',r=>r.fulfill({status:200,contentType:'application/json',body:'{"protocol":1,"scope":"gateway","operation":"semantic-check","read_only":true,"local_only":true,"max_sources":3,"max_source_bytes":131072,"max_total_source_bytes":393216,"max_chunks":512}'}));
  await page.locator('#api-token').fill(token);await page.locator('#connect-form button').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('语义索引检查权限响应异常'));assert(!await page.locator('#semantic-review').isVisible());await page.unroute('**/api/memory/semantic-capabilities');
  await stop(servers[0]);const disabled=await start(false);await page.goto(disabled);await connect();await page.locator('#memory-tab').click();await idle();assert(!await page.locator('#semantic-review').isVisible());assert.deepStrictEqual(errors,[]);assert.strictEqual(modelCalls,setupCalls);
  console.log('PASS semantic review browser 4: default-off/malformed capability fails closed; gateway/readOnly identities perform zero memory/semantic probes');
 }finally{
  if(release)release();if(browser)await browser.close();for(const record of servers)await stop(record);await new Promise(resolve=>gateway.close(resolve));
  if(process.env.JIACLAW_KEEP_TEST_OUTPUT){for(let i=0;i<servers.length;i++)fs.writeFileSync(path.join(root,'serve-'+i+'.log'),servers[i].log);console.log('Evidence '+root);}else fs.rmSync(root,{recursive:true,force:true});
 }
})().catch(e=>{console.error(e);process.exit(1);});
