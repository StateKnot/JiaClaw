// Real Chromium + real host + disposable localhost model/Telegram fixtures.
// Outbox rows are created exclusively by production webhook/job processing.
const { chromium } = require('playwright');
const fs = require('fs'), os = require('os'), path = require('path');
const crypto = require('crypto'), child = require('child_process'), http = require('http'), assert = require('assert');
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
(async () => {
 const root = fs.mkdtempSync(path.join(os.tmpdir(), 'jiaclaw-outbox-browser-'));
 const binary = path.resolve(process.argv[2] || 'target/debug/jiaclaw');
 const token = 'host-' + crypto.randomUUID(), modelToken = 'model-' + crypto.randomUUID();
 const webhookSecret = 'webhook-' + crypto.randomUUID(), botToken = '123456:' + crypto.randomUUID();
 const privateError = 'PRIVATE-UPSTREAM-' + crypto.randomUUID();
 const workspace = path.join(root, 'workspace');fs.mkdirSync(workspace);
 const logs = [], fixtureErrors = [], modelCalls = [], sends = [], uiCalls = [];
 const topics = {event_long:101,resolve:102,delivered:103,extra:104,submitting:105,job_long:201};
 const attack = '<img src=x onerror="window.OUTBOX_XSS=1">';
 const text = name => 'outbox-case:' + name + ' ' + attack + (name.endsWith('_long') ? (' Unicode🙂 plain text '.repeat(260)) : '');
 let processHandle, browser, base, releaseSend;
 const platform = http.createServer(async (req, res) => {
  try {
   let raw='';for await(const chunk of req)raw+=chunk;
   const body=JSON.parse(raw);
   const respond=(status,value)=>{if(!res.destroyed){res.writeHead(status,{'content-type':'application/json'});res.end(JSON.stringify(value));}};
   if(req.url==='/v1/chat/completions') {
    assert.strictEqual(req.headers.authorization,'Bearer '+modelToken);assert(req.headers['idempotency-key']);
    assert.deepStrictEqual(body.tools.map(tool=>tool.function.name),['datetime_now']);
    const prompt=[...body.messages].reverse().find(message=>message.role==='user').content;
    const name=prompt.match(/outbox-case:([a-z_]+)/)[1];modelCalls.push(name);
    const done=body.messages.at(-1).role==='tool';
    const message=done ? {role:'assistant',content:text(name)} : {role:'assistant',content:null,tool_calls:[{id:'clock-'+crypto.randomUUID(),type:'function',function:{name:'datetime_now',arguments:'{}'}}]};
    respond(200,{choices:[{message,finish_reason:done?'stop':'tool_calls'}]});
   } else {
    assert.strictEqual(req.url,'/bot'+botToken+'/sendMessage');assert.strictEqual(body.chat_id,'-100123');
    const name=Object.keys(topics).find(name=>topics[name]===body.message_thread_id);assert(name);
    sends.push({name,text:body.text});
    if(name==='submitting')await new Promise(resolve=>releaseSend=resolve);
    if(name==='delivered'||name==='submitting')respond(200,{ok:true,result:{message_id:900+sends.length,chat:{id:-100123}}});
    else respond(502,{error:privateError+' '+botToken});
   }
  } catch(error) {fixtureErrors.push(error.name);if(!res.destroyed){res.writeHead(500);res.end();}}
 });
 await new Promise(resolve=>platform.listen(0,'127.0.0.1',resolve));
 const fixtureBase='http://127.0.0.1:'+platform.address().port;
 const config={agent:{name:'Outbox browser fixture',description:'Local acceptance',system_instructions:'Use the authorized clock.',max_turns:10,workspace_path:workspace},provider:{provider_type:'brokerrouter',base_url:fixtureBase,api_key:modelToken,model:'fixture'},scheduler:{enabled:true},http:{bind:'127.0.0.1:0',api_token:token,persist:true,persist_path:'../state/sessions.sqlite3',shutdown_timeout_secs:1,telegram_secret:webhookSecret,telegram_bot_token:botToken,channels:[{channel:'telegram',installation_id:'123456',allowed_senders:['987654'],allowed_conversations:['-100123'],enabled_tools:['datetime_now'],timeout_secs:30,local_test_api_base:fixtureBase,scheduled_destinations:[{conversation_id:'-100123',thread_id:String(topics.job_long)}]}]}};
 const env=Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('JIACLAW_')));
 Object.assign(env,{JIACLAW_LOG_LEVEL:'info',JIACLAW_LOG_FORMAT:'text'});
 async function eventually(check,label,timeout=20000) {
  const deadline=Date.now()+timeout;
  while(Date.now()<deadline){assert.deepStrictEqual(fixtureErrors,[]);if(processHandle)assert.strictEqual(processHandle.exitCode,null,'host exited during '+label);const value=await check();if(value)return value;await sleep(40);}
  throw new Error(label+' timed out; '+JSON.stringify({models:modelCalls.length,sends:sends.map(send=>send.name),fixtureErrors}));
 }
 async function api(endpoint,method='GET',body,headers={authorization:'Bearer '+token}) {
  const response=await fetch(base+endpoint,{method,headers:{...headers,...(body?{'content-type':'application/json'}:{})},body:body?JSON.stringify(body):undefined,signal:AbortSignal.timeout(10000)});
  const raw=await response.text();for(const secret of [token,modelToken,webhookSecret,botToken,privateError])assert(!raw.includes(secret),'private value leaked in API');
  return {status:response.status,body:raw?(response.headers.get('content-type')||'').includes('application/json')?JSON.parse(raw):raw:null};
 }
 async function start() {
  fs.writeFileSync(path.join(root,'config.json'),JSON.stringify(config));
  let log='';logs.push(()=>log);
  processHandle=child.spawn(binary,['serve','--config',path.join(root,'config.json')],{env});
  processHandle.stdout.on('data',data=>log+=data);processHandle.stderr.on('data',data=>log+=data);
  await eventually(()=>{const match=log.match(/HTTP 服务已启动于 (http:\/\/127\.0\.0\.1:\d+)/);if(match){base=match[1];return true;}},'host startup');
  assert.strictEqual((await api('/health')).status,200);
 }
 async function stop(signal='SIGTERM') {
  if(!processHandle||processHandle.exitCode!==null||processHandle.signalCode!==null)return;
  const stopped=new Promise(resolve=>processHandle.once('exit',resolve));processHandle.kill(signal);
  await Promise.race([stopped,sleep(15000).then(()=>{throw new Error('host shutdown timeout');})]);
  processHandle=null;
 }
 async function deliveries() {const result=await api('/api/channels/deliveries?limit=100');assert.strictEqual(result.status,200);return result.body;}
 async function record(name,state) {return eventually(async()=> (await deliveries()).find(row=>row.destination.thread_id===String(topics[name])&&row.ordinal===0&&row.state===state),'record '+name+' '+state);}
 async function webhook(name) {
  const id=topics[name];const result=await api('/hooks/telegram','POST',{update_id:id,message:{message_id:id,from:{id:987654,is_bot:false},chat:{id:-100123},message_thread_id:id,text:'outbox-case:'+name}},{'X-Telegram-Bot-Api-Secret-Token':webhookSecret});assert.strictEqual(result.status,200);
 }
 let page;
 async function connect(value=token) {
  await page.locator('#api-token').fill(value);await page.getByRole('button',{name:'连接',exact:true}).click();
  if(value===token)await eventually(async()=>await page.locator('#outbox-tab').isVisible(),'outbox capability');
  else await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));
 }
 async function choose(id) {
  await page.waitForFunction(()=>!document.getElementById('outbox-refresh').disabled);
  while(await page.locator('#outbox-prev').isEnabled()){await page.locator('#outbox-prev').click();await page.waitForFunction(()=>!document.getElementById('outbox-refresh').disabled);}
  for(let count=0;count<5;count++){
   const button=page.locator('#outbox-list [data-delivery-id="'+id+'"]');
   if(await button.count()){await button.click();await page.waitForFunction(id=>document.getElementById('delivery-title').textContent.includes(id)&&!document.getElementById('delivery-refresh').disabled,id);return;}
   assert(await page.locator('#outbox-next').isEnabled(),'record missing from pages');await page.locator('#outbox-next').click();await page.waitForFunction(()=>!document.getElementById('outbox-refresh').disabled);
  }
  throw new Error('record not found in bounded outbox');
 }
 async function disabledOrHidden(selector) {return !(await page.locator(selector).isVisible())||await page.locator(selector).isDisabled();}
 async function confirmClick(selector,accept=true) {
  const dialog=page.waitForEvent('dialog');const clicked=page.locator(selector).click();const prompt=await dialog;
  assert.strictEqual(prompt.type(),'confirm');await (accept?prompt.accept():prompt.dismiss());await clicked;
 }
 async function authoritative(id,state) {return eventually(async()=>{const response=await api('/api/channels/deliveries/'+id);return response.status===200&&response.body.state===state&&response.body;},'authoritative '+state);}
 try {
  await start();assert.strictEqual((await api('/api/channels/status')).body.state,'running');
  for(const name of ['event_long','resolve','delivered','extra']){await webhook(name);await record(name,name==='delivered'?'delivered':'unknown');}
  const jobResult=await api('/api/jobs','POST',{name:'outbox fixture',prompt:'outbox-case:job_long',schedule:{kind:'interval',seconds:1},enabled_tools:['datetime_now'],timeout_secs:30,delivery:{channel:'telegram',installation_id:'123456',conversation_id:'-100123',thread_id:String(topics.job_long)}});
  assert([200,201].includes(jobResult.status));const jobId=jobResult.body.id;
  const jobUnknown=await record('job_long','unknown');assert.strictEqual(jobUnknown.job_id,jobId);assert.strictEqual(jobUnknown.event_id,null);
  await webhook('submitting');const submitting=await record('submitting','submitting');await eventually(()=>releaseSend,'observed submitted platform request');
  const before=await deliveries(), sourceRows=name=>before.filter(row=>row.destination.thread_id===String(topics[name]));
  assert(before.length>5);assert(sourceRows('event_long').length>1);assert(sourceRows('job_long').length>1);
  assert(sourceRows('event_long').slice(1).every(row=>row.state==='pending'));assert(sourceRows('job_long').slice(1).every(row=>row.state==='pending'));
  browser=await chromium.launch({headless:true,...(process.env.JIACLAW_TEST_CHROMIUM?{executablePath:process.env.JIACLAW_TEST_CHROMIUM}:{})});
  page=await browser.newPage({viewport:{width:1360,height:900}});const errors=[];page.on('pageerror',error=>errors.push(String(error)));
  page.on('request',request=>{const url=new URL(request.url());if(url.pathname.startsWith('/api/'))uiCalls.push({method:request.method(),path:url.pathname,search:url.search});});
  await page.goto(base);assert.strictEqual(await page.locator('#outbox-tab').isVisible(),false);await sleep(100);
  assert(!uiCalls.some(call=>call.path==='/api/channels/status'),'empty token must not probe');
  await connect();await page.locator('#outbox-tab').click();await eventually(async()=>await page.locator('#outbox-list [data-delivery-id]').count()===5,'first five records');
  assert(uiCalls.some(call=>call.path==='/api/channels/deliveries'&&call.search==='?limit=5&offset=0'));
  const firstPageIds=await page.locator('#outbox-list [data-delivery-id]').evaluateAll(buttons=>buttons.map(button=>button.dataset.deliveryId));
  const firstPageCount=await page.locator('#outbox-count').textContent();
  await page.route('**/api/channels/deliveries?limit=5&offset=5',route=>route.abort('failed'));
  await page.locator('#outbox-next').click();await page.waitForFunction(()=>!document.getElementById('outbox-refresh').disabled);
  assert.deepStrictEqual(await page.locator('#outbox-list [data-delivery-id]').evaluateAll(buttons=>buttons.map(button=>button.dataset.deliveryId)),firstPageIds);
  assert.strictEqual(await page.locator('#outbox-count').textContent(),firstPageCount);assert(await page.locator('#outbox-prev').isDisabled());assert(await page.locator('#outbox-next').isEnabled());
  await page.unroute('**/api/channels/deliveries?limit=5&offset=5');
  await page.locator('#outbox-next').click();await page.waitForFunction(()=>!document.getElementById('outbox-refresh').disabled);
  assert.strictEqual(uiCalls.filter(call=>call.path==='/api/channels/deliveries'&&call.search==='?limit=5&offset=5').length,2);
  assert(uiCalls.some(call=>call.path==='/api/channels/deliveries'&&call.search==='?limit=5&offset=5'));
  assert(await page.locator('#outbox-prev').isEnabled());assert((await page.locator('#outbox-count').textContent()).length>0);
  await choose(submitting.id);assert(await disabledOrHidden('#delivery-resolve'));assert(await disabledOrHidden('#delivery-cancel'));
  await choose(sourceRows('delivered')[0].id);assert(await disabledOrHidden('#delivery-resolve'));assert(await disabledOrHidden('#delivery-cancel'));
  // Real in-flight platform request becomes unknown on restart, then audit is
  // still available with both channels and scheduler explicitly disabled.
  await stop('SIGKILL');releaseSend();delete config.http.telegram_secret;delete config.http.telegram_bot_token;config.http.channels=[];config.scheduler.enabled=false;
  await start();assert.strictEqual((await api('/api/channels/status')).body.state,'disabled');
  const recovered=await authoritative(submitting.id,'unknown');assert.strictEqual(recovered.id,submitting.id);
  const counts={models:modelCalls.length,sends:sends.length};assert.strictEqual(counts.models,12);assert.strictEqual(counts.sends,6);
  await page.goto(base);await connect();await page.locator('#outbox-tab').click();await choose(sourceRows('resolve')[0].id);
  const resolveId=sourceRows('resolve')[0].id, resolvePath='/api/channels/deliveries/'+resolveId+'/resolve';
  const detailPattern='**/api/channels/deliveries/'+resolveId;
  const evidence='Verified platform evidence '+attack;
  const invalidBefore=uiCalls.filter(call=>call.method==='POST').length;let invalidConfirms=0;
  const rejectUnexpected=dialog=>{invalidConfirms++;return dialog.dismiss();};page.on('dialog',rejectUnexpected);
  for(const invalid of ['', 'line\ncontrol', '汉'.repeat(1400)]){
   await page.locator('#delivery-evidence').fill(invalid);await page.locator('#delivery-resolve').click();
   await page.waitForFunction(()=>!document.getElementById('delivery-refresh').disabled);
   assert.strictEqual(uiCalls.filter(call=>call.method==='POST').length,invalidBefore);assert.strictEqual(await page.locator('#delivery-evidence').inputValue(),invalid);
  }
  page.off('dialog',rejectUnexpected);assert.strictEqual(invalidConfirms,0);
  await page.locator('#delivery-evidence').fill(evidence);
  assert.strictEqual(await page.locator('#delivery-content img').count(),0);assert((await page.locator('#delivery-content').textContent()).includes(attack));
  const writesBefore=uiCalls.filter(call=>call.method==='POST').length;
  await confirmClick('#delivery-resolve',false);assert.strictEqual(uiCalls.filter(call=>call.method==='POST').length,writesBefore);assert.strictEqual(await page.locator('#delivery-evidence').inputValue(),evidence);
  // Conflicts and network ambiguity retain evidence and perform authoritative
  // GETs, never an automatic second POST.
  let rejected=0;await page.route('**'+resolvePath,route=>{rejected++;return route.fulfill({status:409,contentType:'application/json',body:JSON.stringify({error:'fixture conflict'})});});
  let getBefore=uiCalls.filter(call=>call.path.endsWith('/'+resolveId)&&call.method==='GET').length;
  await confirmClick('#delivery-resolve');await eventually(()=>uiCalls.filter(call=>call.path.endsWith('/'+resolveId)&&call.method==='GET').length>getBefore,'GET after 409');
  await page.waitForFunction(()=>!document.getElementById('delivery-refresh').disabled);
  assert.strictEqual(rejected,1);assert.strictEqual(await page.locator('#delivery-evidence').inputValue(),evidence);await authoritative(resolveId,'unknown');await page.unroute('**'+resolvePath);
  let aborted=0;await page.route('**'+resolvePath,route=>{aborted++;return route.abort('failed');});
  getBefore=uiCalls.filter(call=>call.path.endsWith('/'+resolveId)&&call.method==='GET').length;
  await confirmClick('#delivery-resolve');await eventually(()=>uiCalls.filter(call=>call.path.endsWith('/'+resolveId)&&call.method==='GET').length>getBefore,'GET after failed POST');
  await page.waitForFunction(()=>!document.getElementById('delivery-refresh').disabled);assert.strictEqual(aborted,1);assert.strictEqual(await page.locator('#delivery-evidence').inputValue(),evidence);await page.unroute('**'+resolvePath);
  // If the authoritative refresh itself fails, further mutations must stay
  // disabled until an explicit successful refresh.
  await page.route('**'+resolvePath,route=>route.abort('failed'));await page.route(detailPattern,route=>route.abort('failed'));
  await confirmClick('#delivery-resolve');await page.waitForFunction(()=>!document.getElementById('delivery-refresh').disabled);
  assert(await disabledOrHidden('#delivery-resolve'));assert(await disabledOrHidden('#delivery-cancel'));assert.strictEqual(await page.locator('#delivery-evidence').inputValue(),evidence);
  await page.unroute('**'+resolvePath);await page.unroute(detailPattern);await page.locator('#delivery-refresh').click();await page.waitForFunction(()=>!document.getElementById('delivery-resolve').disabled);
  await confirmClick('#delivery-resolve');const resolved=await authoritative(resolveId,'delivered');assert.strictEqual(resolved.receipt,evidence);
  await page.waitForFunction(()=>document.getElementById('delivery-receipt').textContent.includes('Verified platform evidence'));
  assert.strictEqual(await page.locator('#delivery-receipt img').count(),0);assert(await disabledOrHidden('#delivery-resolve'));assert(await disabledOrHidden('#delivery-cancel'));
  // The cancel action applies to the entire event/run source, not just the
  // selected first piece, and does not touch the other source.
  for(const name of ['event_long','job_long']){
   const selected=sourceRows(name)[0];await choose(selected.id);await confirmClick('#delivery-cancel');
   await eventually(async()=>{const rows=await deliveries();return sourceRows(name).every(old=>rows.find(row=>row.id===old.id)?.state==='cancelled');},'cancel entire '+name);
   await eventually(async()=>await disabledOrHidden('#delivery-cancel')&&await disabledOrHidden('#delivery-resolve'),'cancelled controls');
  }
  assert.strictEqual((await authoritative(sourceRows('extra')[0].id,'unknown')).state,'unknown');
  assert.strictEqual(modelCalls.length,counts.models);assert.strictEqual(sends.length,counts.sends);
  // The host can accept a resolution even when its acknowledgement is lost.
  // Fetch the real POST first, drop only its browser response, then verify GET
  // discovers the committed receipt without an automatic repeat submission.
  await choose(submitting.id);const lostEvidence='Verified accepted fixture message after restart';
  await page.locator('#delivery-evidence').fill(lostEvidence);let committedPosts=0;
  const lostPath='**/api/channels/deliveries/'+submitting.id+'/resolve';
  await page.route(lostPath,async route=>{committedPosts++;const response=await route.fetch();assert.strictEqual(response.status(),204);await route.abort('failed');});
  await confirmClick('#delivery-resolve');const committed=await authoritative(submitting.id,'delivered');assert.strictEqual(committed.receipt,lostEvidence);
  await page.waitForFunction(()=>document.getElementById('delivery-receipt').textContent.includes('Verified accepted fixture message after restart')&&!document.getElementById('delivery-refresh').disabled);
  assert.strictEqual(committedPosts,1);assert.strictEqual(await page.locator('#delivery-evidence').inputValue(),lostEvidence);assert(await disabledOrHidden('#delivery-resolve'));assert(await disabledOrHidden('#delivery-cancel'));
  await page.unroute(lostPath);
  await page.locator('#delivery-evidence').fill('');
  const missingId=crypto.randomUUID();await page.locator('#outbox-id').fill(missingId);
  await page.locator('#outbox-lookup button').click();await page.waitForFunction(()=>!document.getElementById('outbox-refresh').disabled);
  assert.strictEqual(await page.locator('#delivery-detail').isVisible(),false);assert(await disabledOrHidden('#delivery-resolve'));assert(await disabledOrHidden('#delivery-cancel'));
  await page.locator('#outbox-id').fill(submitting.id);await page.locator('#outbox-lookup button').click();
  await page.waitForFunction(id=>document.getElementById('delivery-title').textContent.includes(id)&&!document.getElementById('delivery-refresh').disabled,submitting.id);
  assert(uiCalls.some(call=>call.path==='/api/channels/deliveries/'+missingId&&call.method==='GET'));
  assert.strictEqual(await page.locator('#delivery-detail').isVisible(),true);
  await page.screenshot({path:'/tmp/jiaclaw-outbox-desktop.png',fullPage:true});
  await page.setViewportSize({width:390,height:844});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth));
  assert(await page.locator('#sessions button').evaluateAll(buttons=>buttons.length>0&&buttons.every(button=>button.getBoundingClientRect().height>=30)),'mobile session rows must keep readable height');
  await page.screenshot({path:'/tmp/jiaclaw-outbox-mobile.png',fullPage:true});
  await page.locator('#delivery-content').scrollIntoViewIfNeeded();await page.screenshot({path:'/tmp/jiaclaw-outbox-mobile-detail.png',fullPage:true});
  await page.setViewportSize({width:1360,height:900});await page.screenshot({path:'/tmp/jiaclaw-outbox-desktop-detail.png',fullPage:true});
  assert.strictEqual(await page.evaluate(()=>typeof window.OUTBOX_XSS),'undefined');assert.strictEqual(await page.locator('#outbox-view img').count(),0);
  assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);assert.strictEqual(await page.locator('#api-token').inputValue(),'');
  // Read-only capability denial must hide the tab while ordinary login remains
  // usable. This is the gateway behavior without exposing a management route.
  for(const status of [403,404]){
   await page.route('**/api/channels/status',route=>route.fulfill({status,contentType:'application/json',body:JSON.stringify({error:'not available'})}));
   await page.locator('#api-token').fill(token);await page.getByRole('button',{name:'连接',exact:true}).click();await page.waitForFunction(()=>!document.getElementById('new-session').disabled);
   assert.strictEqual(await page.locator('#outbox-tab').isVisible(),false);await page.unroute('**/api/channels/status');
  }
  await connect();await page.locator('#outbox-tab').click();const staleId=sourceRows('extra')[0].id;await choose(staleId);await page.locator('#delivery-evidence').fill('private evidence from old identity');
  let releaseDetail, held=false;await page.route('**/api/channels/deliveries/'+staleId,async route=>{const response=await route.fetch();held=true;await new Promise(resolve=>releaseDetail=resolve);await route.fulfill({response});});
  await page.locator('#delivery-refresh').click();await eventually(()=>held,'held old identity response');
  await connect('invalid-other-identity');releaseDetail();await sleep(100);
  for(const id of ['outbox-list','delivery-title','delivery-summary','delivery-content','delivery-receipt','delivery-error'])assert.strictEqual(await page.locator('#'+id).textContent(),'');
  assert.strictEqual(await page.locator('#delivery-evidence').inputValue(),'');assert.strictEqual(await page.locator('#outbox-id').inputValue(),'');assert.strictEqual(await page.locator('#outbox-tab').isVisible(),false);
  await page.unroute('**/api/channels/deliveries/'+staleId);
  await connect();await page.locator('#outbox-tab').click();await choose(staleId);await page.locator('#delivery-evidence').fill('private evidence before revoked key');
  await page.route('**/api/channels/deliveries/'+staleId,route=>route.fulfill({status:401,contentType:'text/plain',body:'unauthorized'}));
  await page.locator('#delivery-refresh').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));
  assert.strictEqual(await page.locator('#outbox-list').textContent(),'');assert.strictEqual(await page.locator('#delivery-evidence').inputValue(),'');assert.strictEqual(await page.locator('#outbox-id').inputValue(),'');assert.strictEqual(await page.locator('#outbox-tab').isVisible(),false);
  await page.reload();assert.strictEqual(await page.locator('#outbox-tab').isVisible(),false);assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);
  assert.deepStrictEqual(errors,[]);assert.deepStrictEqual(fixtureErrors,[]);assert.strictEqual(modelCalls.length,counts.models);assert.strictEqual(sends.length,counts.sends);
  console.log('PASS: real Chromium outbox; real event/job multipart records; pagination and detail; submitting/delivered restrictions; SIGKILL then disabled-channel audit; confirm/409/network refresh and evidence preservation; real204 resolve/cancel entire source without replay; XSS/mobile; capability denial, identity/401/stale-response clearing; memory-only credentials');
 } finally {
  if(releaseSend)releaseSend();if(browser)await browser.close();await stop();await new Promise(resolve=>platform.close(resolve));
  const log=logs.map(read=>read()).join('\n');for(const secret of [token,modelToken,webhookSecret,botToken,privateError])assert(!log.includes(secret),'private value leaked in host log');
  fs.rmSync(root,{recursive:true,force:true});
 }
})().catch(error=>{console.error(error);process.exitCode=1;});
