// Standalone scheduler UI against a real host and temporary SQLite. Model and
// Telegram endpoints are localhost fixtures; creation identity is never forged.
const {chromium}=require('playwright');
const fs=require('fs'),os=require('os'),path=require('path'),crypto=require('crypto'),child=require('child_process'),http=require('http'),assert=require('assert');
const sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms));
(async()=>{
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'jiaclaw-scheduler-browser-')),workspace=path.join(root,'workspace');fs.mkdirSync(workspace);
 const binary=path.resolve(process.argv[2]||'target/debug/jiaclaw'),token=crypto.randomUUID(),modelToken=crypto.randomUUID();
 const botToken='123456:'+crypto.randomUUID(),webhookSecret=crypto.randomUUID(),attack='<img src=x onerror="window.JOB_XSS=1">';
 const models=[],errors=[],calls=[],logs=[];let host,browser,page,base;
 const server=http.createServer(async(req,res)=>{try{
  assert.strictEqual(req.url,'/v1/chat/completions');assert.strictEqual(req.headers.authorization,'Bearer '+modelToken);assert(req.headers['idempotency-key']);
  let raw='';for await(const chunk of req)raw+=chunk;const body=JSON.parse(raw),prompt=[...body.messages].reverse().find(message=>message.role==='user').content;
  const name=prompt.match(/scheduler-case:([a-z_]+)/)[1];models.push(name);
  assert.deepStrictEqual(body.tools.map(tool=>tool.function.name),['datetime_now']);
  if(name==='failure'){res.writeHead(503,{'content-type':'application/json'});res.end(JSON.stringify({error:'fixture unavailable'}));return;}
  const done=body.messages.at(-1).role==='tool';
  const message=done?{role:'assistant',content:'scheduler-case:'+name+' '+attack+'x'.repeat(20000)}:{role:'assistant',content:null,tool_calls:[{id:'clock-'+crypto.randomUUID(),type:'function',function:{name:'datetime_now',arguments:'{}'}}]};
  res.writeHead(200,{'content-type':'application/json'});res.end(JSON.stringify({choices:[{message,finish_reason:done?'stop':'tool_calls'}]}));
 }catch(error){errors.push(error.name);if(!res.destroyed){res.writeHead(500);res.end();}}});
 await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));const fixtureBase='http://127.0.0.1:'+server.address().port;
 const config={agent:{name:'Scheduler browser',description:'Local acceptance',system_instructions:'Use the clock.',max_turns:10,workspace_path:workspace},provider:{provider_type:'brokerrouter',base_url:fixtureBase,api_key:modelToken,model:'fixture'},scheduler:{enabled:true},http:{bind:'127.0.0.1:0',api_token:token,persist:true,persist_path:'../state/sessions.sqlite3',shutdown_timeout_secs:1,telegram_secret:webhookSecret,telegram_bot_token:botToken,channels:[{channel:'telegram',installation_id:'123456',allowed_senders:['987654'],allowed_conversations:['-100123'],enabled_tools:['datetime_now'],timeout_secs:120,local_test_api_base:fixtureBase,scheduled_destinations:[{conversation_id:'-100123',thread_id:'201'}]}]}};
 const env=Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('JIACLAW_')));Object.assign(env,{JIACLAW_LOG_LEVEL:'info',JIACLAW_LOG_FORMAT:'text'});
 async function eventually(check,label,timeout=20000){const until=Date.now()+timeout;while(Date.now()<until){assert.deepStrictEqual(errors,[]);const result=await check();if(result)return result;await sleep(40);}throw new Error(label+' timed out; '+JSON.stringify({models:models.length,http:calls.map(call=>[call.method,call.path,call.search])}));}
 async function api(url,method='GET',body){const response=await fetch(base+url,{method,headers:{authorization:'Bearer '+token,...(body?{'content-type':'application/json'}:{})},body:body?JSON.stringify(body):undefined,signal:AbortSignal.timeout(10000)});const raw=await response.text();return {status:response.status,body:raw?(response.headers.get('content-type')||'').includes('application/json')?JSON.parse(raw):raw:null};}
 async function start(){fs.writeFileSync(path.join(root,'config.json'),JSON.stringify(config));let log='';logs.push(()=>log);host=child.spawn(binary,['serve','--config',path.join(root,'config.json')],{env});host.stdout.on('data',d=>log+=d);host.stderr.on('data',d=>log+=d);await eventually(()=>{assert.strictEqual(host.exitCode,null);const match=log.match(/HTTP 服务已启动于 (http:\/\/127\.0\.0\.1:\d+)/);if(match){base=match[1];return true;}},'startup');assert.strictEqual((await api('/api/jobs/status')).body.state,'running');}
 async function stop(){if(!host||host.exitCode!==null||host.signalCode!==null)return;const ended=new Promise(resolve=>host.once('exit',resolve));host.kill('SIGTERM');let timer;await Promise.race([ended,new Promise((_,reject)=>timer=setTimeout(()=>reject(new Error('shutdown timeout')),15000))]);clearTimeout(timer);host=null;}
 const spec=(name='future')=>({name:'fixture-'+crypto.randomUUID(),prompt:'scheduler-case:'+name,schedule:{kind:'interval',seconds:3600},enabled_tools:['datetime_now'],timeout_secs:120});
 async function idle(){await page.waitForFunction(()=>!document.getElementById('jobs-refresh').disabled);}
 async function connect(value=token){await page.locator('#api-token').fill(value);await page.getByRole('button',{name:'连接',exact:true}).click();if(value===token)await page.waitForFunction(()=>!document.getElementById('new-session').disabled);else await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));}
 async function jobsView(){await page.locator('#jobs-tab').click();await idle();}
 async function select(id){await idle();await page.locator('#job-lookup-id').fill(id);await page.locator('#job-lookup button').click();await idle();await page.waitForFunction(id=>document.getElementById('job-summary').textContent.includes(id),id);}
 async function fill(name,kind='interval',seconds=3600){await idle();if(!await page.locator('#job-create').evaluate(node=>node.open))await page.locator('#job-create summary').click();await page.locator('#job-name').fill(name);await page.locator('#job-prompt').fill('scheduler-case:'+name);await page.locator('#job-kind').selectOption(kind);if(kind==='interval')await page.locator('#job-interval').fill(String(seconds));else{await page.locator('#job-cron').fill('0 0 1 '+(((new Date().getUTCMonth()+6)%12)+1)+' *');await page.locator('#job-timezone').fill('Asia/Shanghai');}await page.locator('#job-timeout').fill('120');await page.locator('#job-clock').check();await page.locator('#job-json').uncheck();}
 async function confirmClick(selector){const waiting=page.waitForEvent('dialog'),clicked=page.locator(selector).click(),dialog=await waiting;assert.strictEqual(dialog.type(),'confirm');await dialog.accept();await clicked;await idle();}
 async function job(id){const result=await api('/api/jobs/'+id);assert.strictEqual(result.status,200);return result.body;}
 const writes=()=>calls.filter(call=>call.method==='PUT');
 async function created(name){const request=await eventually(()=>[...writes()].reverse().find(call=>call.body.name===name),'create PUT '+name);const id=request.path.split('/').at(-1);assert(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id));await eventually(async()=>{const result=await api('/api/jobs/'+id);return result.status===200&&result.body;},'created job');await idle();return id;}
 const createId=()=>page.locator('#job-create-id').evaluate(node=>node.value||node.textContent);
 async function create(name,kind='interval',seconds=3600){await fill(name,kind,seconds);await page.locator('#job-submit').click();return created(name);}
 async function run(id,status){return eventually(async()=>{const result=await api('/api/jobs/'+id+'/runs?limit=1&offset=0');assert(Array.isArray(result.body));return result.body.find(run=>run.status===status);},'run '+status);}
 try{
  await start();const health=(await api('/api/jobs/status')).body;assert.strictEqual(health.create_identity_protocol,'job-id-v1');
  const legacySpec={...spec(),name:'legacy '+attack,timeout_secs:600,delivery:{channel:'telegram',installation_id:'123456',conversation_id:'-100123',thread_id:'201'}};
  legacySpec.schedule={kind:'cron',expression:'0 0 1 '+(((new Date().getUTCMonth()+6)%12)+1)+' *',timezone:'Asia/Shanghai'};
  const legacy=await api('/api/jobs','POST',legacySpec);assert.strictEqual(legacy.status,201);
  for(let index=0;index<5;index++)assert.strictEqual((await api('/api/jobs','POST',spec())).status,201);
  browser=await chromium.launch({headless:true,...(process.env.JIACLAW_TEST_CHROMIUM?{executablePath:process.env.JIACLAW_TEST_CHROMIUM}:{})});page=await browser.newPage({viewport:{width:1360,height:900}});const browserErrors=[];page.on('pageerror',error=>browserErrors.push(String(error)));
  page.on('request',request=>{const url=new URL(request.url());if(url.pathname.startsWith('/api/'))calls.push({method:request.method(),path:url.pathname,search:url.search,body:['PUT','POST'].includes(request.method())&&request.postData()?request.postDataJSON():undefined});});
  await page.goto(base);await sleep(80);assert(!calls.some(call=>call.path==='/api/jobs/status'));assert.strictEqual(await page.locator('#jobs-tab').isVisible(),false);
  // An explicitly disabled gateway capability must not fall back to standalone.
  await page.route('**/api/gateway/capabilities',route=>route.fulfill({status:200,contentType:'application/json',body:JSON.stringify({scheduled_jobs:false})}));await connect();assert.strictEqual(await page.locator('#jobs-tab').isVisible(),false);assert(!calls.some(call=>call.path==='/api/jobs/status'));await page.unroute('**/api/gateway/capabilities');
  for(const status of [200,204]){
   await page.route('**/api/gateway/capabilities',route=>route.fulfill({status,...(status===200?{contentType:'application/json',body:'null'}:{body:''})}));
   // A present but malformed permission contract now clears identity instead of granting writes.
   await page.locator('#api-token').fill(token);await page.getByRole('button',{name:'连接',exact:true}).click();
   await page.waitForFunction(()=>document.getElementById('status').textContent.includes('权限信息响应异常'));
   assert(await page.locator('#new-session').isDisabled());assert.strictEqual(await page.locator('#sessions button').count(),0);
   assert.strictEqual(await page.locator('#jobs-tab').isVisible(),false);assert(!calls.some(call=>call.path==='/api/jobs/status'),'only an actual capability404 may trigger fallback');await page.unroute('**/api/gateway/capabilities');
  }
  // Old standalone status lacks the durable creation contract and stays hidden.
  await page.route('**/api/jobs/status',route=>route.fulfill({status:200,contentType:'application/json',body:JSON.stringify({state:'running',max_concurrent_runs:4})}));await connect();assert.strictEqual(await page.locator('#jobs-tab').isVisible(),false);await page.unroute('**/api/jobs/status');
  await connect();assert.strictEqual(await page.locator('#jobs-tab').isVisible(),true);await jobsView();assert.strictEqual(await page.locator('#jobs-list button').count(),5);
  const firstPage=await page.locator('#jobs-list').textContent();
  await page.route('**/api/jobs?limit=5&offset=0&include_deleted=true',route=>route.abort('failed'));
  await page.locator('#jobs-deleted').check();await idle();assert.strictEqual(await page.locator('#jobs-deleted').isChecked(),false);assert.strictEqual(await page.locator('#jobs-list').textContent(),firstPage);assert(await page.locator('#jobs-prev').isDisabled());
  await page.unroute('**/api/jobs?limit=5&offset=0&include_deleted=true');
  await page.route('**/api/jobs?limit=5&offset=5&include_deleted=false',route=>route.abort('failed'));
  await page.locator('#jobs-next').click();await idle();assert.strictEqual(await page.locator('#jobs-list').textContent(),firstPage);assert(await page.locator('#jobs-prev').isDisabled());await page.unroute('**/api/jobs?limit=5&offset=5&include_deleted=false');
  // Protocol-only response injection: an oversized JSON page must be rejected
  // before it replaces the real first page or commits the requested offset.
  const oversized={...legacy.body,spec:{...legacy.body.spec,name:'must-not-replace-valid-page',prompt:'x'.repeat(2*1024*1024)}};
  await page.route('**/api/jobs?limit=5&offset=5&include_deleted=false',route=>route.fulfill({status:200,contentType:'application/json',body:JSON.stringify([oversized])}));
  await page.locator('#jobs-next').click();await idle();assert.strictEqual(await page.locator('#jobs-list').textContent(),firstPage);assert(await page.locator('#jobs-prev').isDisabled());await page.unroute('**/api/jobs?limit=5&offset=5&include_deleted=false');
  await page.locator('#jobs-next').click();await idle();assert.strictEqual(calls.filter(call=>call.path==='/api/jobs'&&call.search==='?limit=5&offset=5&include_deleted=false').length,3);assert.strictEqual(await page.locator('#jobs-list button').count(),1);await page.locator('#jobs-prev').click();await idle();
  await select(legacy.body.id);const contract=await page.locator('#job-detail-contract').textContent();for(const value of ['600','datetime_now','telegram','-100123','201','Asia/Shanghai',legacySpec.schedule.expression])assert(contract.includes(value),value);assert.strictEqual(await page.locator('#job-detail img').count(),0);
  const basic=await create('basic','interval',1);const firstRun=await run(basic,'completed');await page.locator('#job-toggle').click();await idle();assert.strictEqual((await job(basic)).enabled,false);
  await page.locator('#runs-refresh').click();await idle();const visible=await page.locator('#job-runs').textContent();assert(visible.includes('scheduler-case:basic'));assert(visible.includes('显示已截断'));assert(visible.includes('datetime_now'));assert.strictEqual(await page.locator('#job-runs img').count(),0);
  // Protocol-only archived-size response, built from an actual completed run:
  // the stored response is below 1 MiB while its UI excerpt remains bounded.
  const archived=JSON.parse(JSON.stringify(firstRun));archived.response.message.content='archived-large '+ '界'.repeat(330000);
  const archivedBytes=Buffer.byteLength(JSON.stringify(archived.response));assert(archivedBytes>950000&&archivedBytes<1024*1024);
  await page.route('**/api/jobs/'+basic+'/runs?limit=1&offset=0',route=>route.fulfill({status:200,contentType:'application/json',body:JSON.stringify([archived])}));
  await page.locator('#runs-refresh').click();await idle();assert((await page.locator('#job-runs .run-content').textContent()).includes('archived-large'));assert((await page.locator('#job-runs .run-content').textContent()).length<17000);
  await page.unroute('**/api/jobs/'+basic+'/runs?limit=1&offset=0');await page.locator('#runs-refresh').click();await idle();
  await confirmClick('#job-toggle');await eventually(async()=>{const rows=(await api('/api/jobs/'+basic+'/runs?limit=5')).body;return rows.find(row=>row.status==='completed'&&row.id!==firstRun.id);},'future run after resume');await page.locator('#job-toggle').click();await idle();
  await page.locator('#runs-refresh').click();await idle();assert(calls.some(call=>call.path==='/api/jobs/'+basic+'/runs'&&call.search==='?limit=1&offset=0'));const latestRunText=await page.locator('#job-runs').textContent();
  await page.route('**/api/jobs/'+basic+'/runs?limit=1&offset=1',route=>route.abort('failed'));await page.locator('#runs-next').click();await idle();assert.strictEqual(await page.locator('#job-runs').textContent(),latestRunText);assert(await page.locator('#runs-prev').isDisabled());await page.unroute('**/api/jobs/'+basic+'/runs?limit=1&offset=1');
  await page.locator('#runs-next').click();await idle();assert.strictEqual(calls.filter(call=>call.path==='/api/jobs/'+basic+'/runs'&&call.search==='?limit=1&offset=1').length,2);assert(await page.locator('#runs-prev').isEnabled());
  await confirmClick('#job-delete');assert.strictEqual((await job(basic)).deleted,true);assert(await page.locator('#job-toggle').isDisabled());
  const cron=await create('cron','cron');assert.strictEqual((await job(cron)).spec.schedule.timezone,'Asia/Shanghai');await page.locator('#job-toggle').click();await idle();assert.strictEqual((await job(cron)).enabled,false);
  const failed=await create('failure','interval',1),failedRun=await run(failed,'failed');assert(failedRun.error&&failedRun.response.message.content);await select(failed);const failedText=await page.locator('#job-runs').textContent();assert(failedText.includes(failedRun.error));assert(failedText.includes(failedRun.response.message.content));assert.strictEqual((await job(failed)).enabled,false);
  // Keep all future seed jobs well away from this test; only the two bounded
  // running cases above may contact the model. Management retries must not.
  const modelCount=models.length;
  for(const state of ['failed','stopping']){
   await page.route('**/api/jobs/status',route=>route.fulfill({status:200,contentType:'application/json',body:JSON.stringify({...health,state})}));await page.locator('#jobs-refresh').click();await idle();assert(await page.locator('#job-submit').isDisabled());assert(await page.locator('#job-toggle').isDisabled());
   await select(legacy.body.id);assert(await page.locator('#job-toggle').isEnabled());assert(await page.locator('#job-delete').isEnabled());await select(failed);await page.unroute('**/api/jobs/status');
  }
  await page.locator('#jobs-refresh').click();await idle();
  // A real committed PUT loses its browser acknowledgement. Before dropping it,
  // pause the created record: reconciliation must preserve its current state.
  let lostId,lostBody,lostCount=0;await page.route('**/api/jobs/*',async route=>{if(route.request().method()!=='PUT')return route.continue();lostCount++;lostId=new URL(route.request().url()).pathname.split('/').at(-1);lostBody=route.request().postDataJSON();const response=await route.fetch();assert.strictEqual(response.status(),201);assert.strictEqual((await api('/api/jobs/'+lostId+'/pause','POST')).status,200);await route.abort('failed');});
  await fill('lost_committed');await page.locator('#job-submit').click();await eventually(()=>lostId,'lost committed ID');await eventually(async()=>await page.locator('#job-summary').textContent().then(text=>text.includes(lostId)),'GET-reconciled selection');await idle();
  assert.strictEqual(lostCount,1);assert.strictEqual((await job(lostId)).enabled,false);assert.deepStrictEqual((await job(lostId)).spec,lostBody);assert.strictEqual(await page.locator('#job-prompt').inputValue(),'');await page.unroute('**/api/jobs/*');
  // A POST never reached the server. The locked draft keeps its exact identity;
  // GET check does not submit, and only an explicit retry may repeat the PUT.
  let missingId,missingBody,aborted=0;await page.route('**/api/jobs/*',route=>{if(route.request().method()!=='PUT')return route.continue();aborted++;missingId=new URL(route.request().url()).pathname.split('/').at(-1);missingBody=route.request().postDataJSON();return route.abort('failed');});
  await fill('not_submitted');await page.locator('#job-submit').click();await eventually(()=>missingId,'unsubmitted identity');await page.waitForFunction(()=>!document.getElementById('job-create-check').disabled);
  assert.strictEqual(aborted,1);assert.strictEqual((await api('/api/jobs/'+missingId)).status,404);assert((await createId()).includes(missingId));assert(await page.locator('#job-name').isDisabled());assert.strictEqual(await page.locator('#job-prompt').inputValue(),'scheduler-case:not_submitted');
  await page.locator('#job-create-check').click();await idle();assert.strictEqual(aborted,1);assert(await page.locator('#job-name').isDisabled());await page.unroute('**/api/jobs/*');
  await confirmClick('#job-create-retry');await eventually(async()=> (await api('/api/jobs/'+missingId)).status===200,'explicit same-ID retry');assert.deepStrictEqual((await job(missingId)).spec,missingBody);const retries=writes().filter(call=>call.path==='/api/jobs/'+missingId);assert.strictEqual(retries.length,2);assert.deepStrictEqual(retries[0].body,retries[1].body);
  // When both the committed acknowledgement and follow-up GET are lost, the
  // explicit same-ID retry returns the current deleted record and never revives it.
  let deletedAttemptId,deletedAttemptBody,deletedSubmissions=0;
  await page.route('**/api/jobs/*',async route=>{const request=route.request();if(request.method()==='PUT'){deletedSubmissions++;deletedAttemptId=new URL(request.url()).pathname.split('/').at(-1);deletedAttemptBody=request.postDataJSON();const response=await route.fetch();assert.strictEqual(response.status(),201);assert.strictEqual((await api('/api/jobs/'+deletedAttemptId,'DELETE')).status,204);return route.abort('failed');}if(request.method()==='GET'&&new URL(request.url()).pathname==='/api/jobs/'+deletedAttemptId)return route.abort('failed');return route.continue();});
  await fill('lost_deleted');await page.locator('#job-submit').click();await page.waitForFunction(()=>!document.getElementById('job-create-retry').disabled);assert.strictEqual(deletedSubmissions,1);assert.strictEqual((await job(deletedAttemptId)).deleted,true);assert(await page.locator('#job-name').isDisabled());await page.unroute('**/api/jobs/*');
  await confirmClick('#job-create-retry');assert.strictEqual((await job(deletedAttemptId)).deleted,true);assert.strictEqual((await job(deletedAttemptId)).enabled,false);assert.deepStrictEqual((await job(deletedAttemptId)).spec,deletedAttemptBody);assert.strictEqual(writes().filter(call=>call.path==='/api/jobs/'+deletedAttemptId).length,2);assert(await page.locator('#job-toggle').isDisabled());
  // Purging a successfully created job leaves a permanent identity tombstone.
  // Lose its initial acknowledgement, then make a real same-ID retry: the
  // following GET404 must not erase the PUT409 cause or re-enable retries.
  let retiredId,retiredBody,retiredSubmissions=0;
  await page.route('**/api/jobs/*',async route=>{const request=route.request();if(request.method()!=='PUT')return route.continue();retiredSubmissions++;retiredId=new URL(request.url()).pathname.split('/').at(-1);retiredBody=request.postDataJSON();const response=await route.fetch();assert.strictEqual(response.status(),201);assert.strictEqual((await api('/api/jobs/'+retiredId,'DELETE')).status,204);assert.strictEqual((await api('/api/jobs/'+retiredId+'?purge=true','DELETE')).status,204);await route.abort('failed');});
  await fill('retired_identity');await page.locator('#job-submit').click();await page.waitForFunction(()=>!document.getElementById('job-create-retry').disabled);assert.strictEqual(retiredSubmissions,1);assert.strictEqual((await api('/api/jobs/'+retiredId)).status,404);await page.unroute('**/api/jobs/*');
  await confirmClick('#job-create-retry');assert(await page.locator('#job-create-retry').isDisabled());assert(await page.locator('#job-name').isDisabled());assert.strictEqual(await page.locator('#job-prompt').inputValue(),retiredBody.prompt);assert((await createId()).includes(retiredId));assert((await page.locator('#job-create-state').textContent()).includes('permanently retired'));
  const retiredPuts=writes().filter(call=>call.path==='/api/jobs/'+retiredId);assert.strictEqual(retiredPuts.length,2);assert.deepStrictEqual(retiredPuts[0].body,retiredPuts[1].body);assert.strictEqual((await api('/api/jobs/'+retiredId)).status,404);
  await page.locator('#job-create-check').click();await idle();assert((await page.locator('#job-create-state').textContent()).includes('permanently retired'));assert(await page.locator('#job-create-retry').isDisabled());assert.strictEqual(writes().filter(call=>call.body.name==='retired_identity').length,2);
  await confirmClick('#job-create-abandon');assert((await createId()).includes(retiredId));assert(await page.locator('#job-name').isEnabled());
  // Abandon only removes page tracking; it must not delete or cancel any job.
  let abandonedId;await page.route('**/api/jobs/*',route=>{if(route.request().method()!=='PUT')return route.continue();abandonedId=new URL(route.request().url()).pathname.split('/').at(-1);return route.abort('failed');});await fill('abandoned');await page.locator('#job-submit').click();await page.waitForFunction(()=>!document.getElementById('job-create-abandon').disabled);const deleteCount=calls.filter(call=>call.method==='DELETE').length;await confirmClick('#job-create-abandon');assert.strictEqual(calls.filter(call=>call.method==='DELETE').length,deleteCount);assert.strictEqual((await api('/api/jobs/'+abandonedId)).status,404);assert(await page.locator('#job-name').isEnabled());assert((await createId()).includes(abandonedId));assert(await page.locator('#job-create-check').isDisabled());assert(await page.locator('#job-create-retry').isDisabled());await page.unroute('**/api/jobs/*');
  assert.strictEqual(models.length,modelCount);
  await stop();await start();await page.goto(base);await connect();await jobsView();await select(basic);assert.strictEqual((await job(basic)).deleted,true);assert(await page.locator('#job-toggle').isDisabled());assert(await page.locator('#job-delete').isDisabled());assert((await page.locator('#job-runs').textContent()).includes('scheduler-case:basic'));
  await select(legacy.body.id);await page.setViewportSize({width:390,height:844});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth));await page.screenshot({path:'/tmp/jiaclaw-scheduler-mobile.png',fullPage:true});await page.setViewportSize({width:1360,height:900});await page.screenshot({path:'/tmp/jiaclaw-scheduler-desktop.png',fullPage:true});
  assert.strictEqual(await page.evaluate(()=>typeof window.JOB_XSS),'undefined');assert.strictEqual(await page.locator('#jobs-view img').count(),0);assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);assert.strictEqual(await page.locator('#api-token').inputValue(),'');
  // A slow old-identity page response cannot overwrite cleared state.
  let release,held=false;await page.route('**/api/jobs?limit=5&offset=0&include_deleted=false',async route=>{const response=await route.fetch();held=true;await new Promise(resolve=>release=resolve);await route.fulfill({response});});await page.locator('#jobs-refresh').click();await eventually(()=>held,'held page');await connect('invalid-identity');release();await sleep(100);
  for(const id of ['jobs-list','job-title','job-summary','job-detail-contract','job-runs'])assert.strictEqual(await page.locator('#'+id).textContent(),'');assert.strictEqual(await page.locator('#job-lookup-id').inputValue(),'');assert.strictEqual(await page.locator('#job-prompt').inputValue(),'');assert.strictEqual(await page.locator('#jobs-tab').isVisible(),false);await page.unroute('**/api/jobs?limit=5&offset=0&include_deleted=false');
  await connect();await jobsView();await page.route('**/api/jobs/status',route=>route.fulfill({status:401,contentType:'text/plain',body:'unauthorized'}));await page.locator('#jobs-refresh').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));assert.strictEqual(await page.locator('#jobs-list').textContent(),'');assert.strictEqual(await page.locator('#jobs-tab').isVisible(),false);
  assert.deepStrictEqual(browserErrors,[]);assert.deepStrictEqual(errors,[]);assert.strictEqual(models.length,modelCount);assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);
  console.log('PASS: real standalone scheduler Chromium; protocol/capability gate; legacy POST and full contract; durable PUT identities with committed/unsubmitted failures and explicit same-ID retry; native interval run/pause/resume/delete/restart; cron/timezone; runtime error and bounded text/XSS; atomic jobs/runs pagination; failed/stopping controls; lookup/deleted audit; identity/401/stale clearing; mobile and memory-only credentials');
 }finally{if(browser)await browser.close();await stop();await new Promise(resolve=>server.close(resolve));const log=logs.map(read=>read()).join('\n');for(const secret of [token,modelToken,botToken,webhookSecret])assert(!log.includes(secret),'secret in host log');fs.rmSync(root,{recursive:true,force:true});}
})().catch(error=>{console.error(error);process.exitCode=1;});
