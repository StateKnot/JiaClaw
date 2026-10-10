// Real gateway, SQLite backend and Chromium; only localhost model fixtures.
const {chromium}=require('playwright');
const fs=require('fs'),os=require('os'),path=require('path'),crypto=require('crypto'),child=require('child_process'),http=require('http'),assert=require('assert'),util=require('util');
const execFile=util.promisify(child.execFile),sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms));
(async()=>{
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'jiaclaw-read-only-browser-')),workspace=path.join(root,'workspace');fs.mkdirSync(workspace);
 const binary=path.resolve(process.argv[2]||'target/debug/jiaclaw'),backendToken=crypto.randomUUID(),modelToken=crypto.randomUUID();
 const env=Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('JIACLAW_')));Object.assign(env,{JIACLAW_LOG_LEVEL:'info',JIACLAW_LOG_FORMAT:'text'});
 const processes=[],errors=[],writes=[];let browser,base,backendBase,models=0,releaseCapability,releaseReceipt,receiptReached=false;
 const pausePosts=[],dispatchIds=[];let gated=false;const receiptGate=new Promise(resolve=>releaseReceipt=resolve);
 // Withhold one real native dispatch receipt after its run has committed.
 // A readable completed run does not release the gateway execution owner.
 const bridge=http.createServer((req,res)=>{
  const chunks=[];let bytes=0;req.on('data',chunk=>{bytes+=chunk.length;assert(bytes<=512*1024);chunks.push(chunk);});
  if(req.method==='POST'&&/\/api\/jobs\/[^/]+\/pause$/.test(req.url))pausePosts.push(req.headers['x-request-id']);
  const forward=http.request(new URL(req.url,backendBase),{method:req.method,headers:req.headers},upstream=>{
   const body=[];let size=0;upstream.on('data',chunk=>{size+=chunk.length;if(size>2*1024*1024){errors.push('oversized bridge body');forward.destroy();return;}body.push(chunk);});
   upstream.on('end',async()=>{try{
    if(req.url==='/internal/scheduler/dispatch'&&upstream.statusCode===200&&!gated){
     gated=true;dispatchIds.push(JSON.parse(Buffer.concat(chunks).toString()).request_id);receiptReached=true;await receiptGate;
    }
    if(!res.destroyed){res.writeHead(upstream.statusCode,upstream.headers);res.end(Buffer.concat(body));}
   }catch(error){errors.push(error.name);res.destroy();}});
   upstream.on('error',()=>{errors.push('bridge upstream error');res.destroy();});
  });
  forward.on('error',()=>{errors.push('bridge request error');res.destroy();});req.pipe(forward);
 });
 const model=http.createServer(async(req,res)=>{try{
  assert.strictEqual(req.url,'/v1/chat/completions');assert.strictEqual(req.headers.authorization,'Bearer '+modelToken);assert(req.headers['idempotency-key']);
  let raw='';for await(const chunk of req)raw+=chunk;const body=JSON.parse(raw);models++;
  const prompt=[...body.messages].reverse().find(message=>message.role==='user').content;
  res.writeHead(200,{'content-type':'application/json'});res.end(JSON.stringify({choices:[{message:{role:'assistant',content:'saved: '+prompt},finish_reason:'stop'}]}));
 }catch(error){errors.push(error.name);res.writeHead(500);res.end();}});
 const cfg=path.join(root,'gateway.json');
 async function cliBefore(deadline,...args){const result=await execFile(binary,['gateway',...args,'--config',cfg],{env,timeout:Math.max(1,Math.min(15000,deadline-Date.now()))});return JSON.parse(result.stdout);}
 async function cli(...args){return cliBefore(Infinity,...args);}
 async function eventually(check,label,timeout=20000){const until=Date.now()+timeout;while(Date.now()<until){assert.deepStrictEqual(errors,[]);const result=await check();if(result&&Date.now()<=until)return result;await sleep(Math.max(0,Math.min(40,until-Date.now())));}throw new Error(label+' timed out');}
 async function launch(args,pattern){let log='';const process=child.spawn(binary,args,{env});processes.push(process);process.stdout.on('data',data=>log+=data);process.stderr.on('data',data=>log+=data);return eventually(()=>{assert.strictEqual(process.exitCode,null,'host exited before startup');const match=log.match(pattern);return match&&match[1];},'startup');}
 async function api(key,url,method='GET',body,deadline=Infinity){const response=await fetch(base+url,{method,headers:{authorization:'Bearer '+key,...(body?{'content-type':'application/json'}:{})},body:body?JSON.stringify(body):undefined,signal:AbortSignal.timeout(Math.max(1,Math.min(10000,deadline-Date.now())))});const raw=await response.text();return {status:response.status,body:raw?JSON.parse(raw):null,request_id:response.headers.get('x-request-id')};}
 try{
  await new Promise(resolve=>model.listen(0,'127.0.0.1',resolve));
  const settings={agent:{name:'viewer',description:'Local acceptance',system_instructions:'Respond with text.',max_turns:10,workspace_path:workspace},provider:{provider_type:'brokerrouter',base_url:'http://127.0.0.1:'+model.address().port,api_key:modelToken,model:'fixture'},http:{bind:'127.0.0.1:0',api_token:backendToken,persist:true,persist_path:'../state/sessions.sqlite3',shutdown_timeout_secs:1},scheduler:{enabled:true,gateway_driven:true}};
  const backendCfg=path.join(root,'backend.json');fs.writeFileSync(backendCfg,JSON.stringify(settings));
  const backend=await launch(['serve','--config',backendCfg],/HTTP 服务已启动于 (http:\/\/127\.0\.0\.1:\d+)/);backendBase=backend;
  await new Promise(resolve=>bridge.listen(0,'127.0.0.1',resolve));const bridged='http://127.0.0.1:'+bridge.address().port;
  // Reserve a free local port; production gateway config intentionally requires a fixed bind.
  const reservation=http.createServer();await new Promise(resolve=>reservation.listen(0,'127.0.0.1',resolve));const gatewayPort=reservation.address().port;await new Promise(resolve=>reservation.close(resolve));
  const secret=path.join(root,'backend.secret');fs.writeFileSync(secret,backendToken,{mode:0o600});
  fs.writeFileSync(cfg,JSON.stringify({bind:'127.0.0.1:'+gatewayPort,registry_path:path.join(root,'registry/users.sqlite3'),request_timeout_seconds:150,max_in_flight:4,scheduled_jobs:true,backends:[{id:'viewer',url:bridged,token_file:secret}]}));
  const full=await cli('user-add','--backend','viewer'),read=await cli('key-add','--user',full.user_id,'--read-only');assert.strictEqual(full.read_only,false);assert.strictEqual(read.read_only,true);
  // Wait through health rather than depending on a gateway log message.
  const gateway=child.spawn(binary,['gateway','serve','--config',cfg],{env});processes.push(gateway);gateway.stdout.resume();gateway.stderr.resume();base='http://127.0.0.1:'+gatewayPort;
  await eventually(async()=>{assert.strictEqual(gateway.exitCode,null);try{return (await fetch(base+'/health',{signal:AbortSignal.timeout(1000)})).status===200;}catch{return false;}},'gateway startup');
  const session=(await api(full.token,'/api/sessions','POST')).body.session_id;
  assert.strictEqual((await api(full.token,'/api/chat','POST',{session_id:session,messages:[{role:'user',content:'private history <img src=x onerror="window.XSS=1">'}],stream:false})).status,200);
  const job=(await api(full.token,'/api/jobs','POST',{name:'private task',prompt:'private scheduled result',schedule:{kind:'interval',seconds:1},enabled_tools:['datetime_now'],timeout_secs:120})).body;
  await eventually(async()=>{const response=await api(full.token,'/api/jobs/'+job.id+'/runs?limit=5&offset=0');return response.body.items.some(run=>run.status==='completed')&&receiptReached;},'scheduled result');
  const rejected=[];
  function knownBusy(response){
   assert.strictEqual(response.status,429);assert.deepStrictEqual(response.body,{error:'user request already in progress'});
   assert(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(response.request_id));rejected.push(response.request_id);
  }
  const busy=await api(full.token,'/api/jobs/'+job.id+'/pause','POST');knownBusy(busy);
  const held=(await cli('user-list')).users[0].hold;
  assert.strictEqual(held.state,'in_flight');assert.strictEqual(held.request_id,dispatchIds[0]);assert.strictEqual(pausePosts.length,0);
  releaseReceipt();releaseReceipt=null;const pauseDeadline=Date.now()+20000;
  // Only the exact gateway rejection before durable admission permits another
  // explicit attempt. Unknown/forwarded responses fail; never replay them.
  const paused=await eventually(async()=>{
   const response=await api(full.token,'/api/jobs/'+job.id+'/pause','POST',undefined,pauseDeadline);
   if(response.status===429){knownBusy(response);return false;}
   assert.strictEqual(response.status,200);assert.strictEqual(response.body.id,job.id);assert.strictEqual(response.body.enabled,false);return response;
  },'explicit pause after original gateway receipt',Math.max(1,pauseDeadline-Date.now()));
  assert.deepStrictEqual(pausePosts,[paused.request_id]);
  const audit=await cliBefore(pauseDeadline,'audit-list','--user',full.user_id,'--limit','100');assert.strictEqual(audit.has_more,false);
  assert(audit.events.some(event=>event.request_id===paused.request_id));
  for(const id of rejected)assert(!audit.events.some(event=>event.request_id===id),'known rejected pause cannot have a durable admission');
  console.log('PASS: completed native run with held receipt yields pre-admission 429; original hold retained, rejected IDs unforwarded/unaudited, one successful pause within original 20s');
  // Pause blocks future dispatches; wait for any admitted completion before checking model counts.
  await eventually(async()=>{const users=(await cliBefore(pauseDeadline,'user-list')).users;return users[0].hold===null;},'gateway idle',Math.max(1,pauseDeadline-Date.now()));
  browser=await chromium.launch({headless:true,...(process.env.JIACLAW_TEST_CHROMIUM?{executablePath:process.env.JIACLAW_TEST_CHROMIUM}:{})});const page=await browser.newPage({viewport:{width:1360,height:900}});
  page.on('pageerror',error=>errors.push(error.name));page.on('dialog',dialog=>dialog.accept());
  page.on('request',request=>{const url=new URL(request.url());if(url.pathname.startsWith('/api/')&&request.method()!=='GET')writes.push([request.method(),url.pathname]);});
  async function idle(){await page.waitForFunction(()=>!document.getElementById('refresh').disabled);}
  async function connect(key,readonly){await page.locator('#api-token').fill(key);await page.getByRole('button',{name:'连接',exact:true}).click();await idle();assert.strictEqual(await page.locator('#access-mode').isVisible(),readonly);}
  async function viewer(){await connect(read.token,true);await page.locator('#sessions button').filter({hasText:session.slice(0,24)}).click();await idle();}
  await page.goto(base);await viewer();
  assert((await page.locator('#messages').textContent()).includes('private history'));assert.strictEqual(await page.locator('#messages img').count(),0);
  for(const id of ['new-session','delete-session','message','send'])assert(await page.locator('#'+id).isDisabled(),id);
  assert(await page.locator('#refresh').isEnabled());assert.strictEqual(await page.locator('#outbox-tab').isVisible(),false);
  const baselineModels=models,baselineWrites=writes.length;
  // Forced DOM actions still hit the permission guard before fetch/confirmation.
  await page.evaluate(()=>{
   document.getElementById('new-session').disabled=false;document.getElementById('new-session').click();
   document.getElementById('delete-session').disabled=false;document.getElementById('delete-session').click();
   document.getElementById('message').disabled=false;document.getElementById('message').value='must not submit';
   document.getElementById('chat-form').dispatchEvent(new Event('submit',{bubbles:true,cancelable:true}));
  });await idle();
  await page.locator('#jobs-tab').click();await page.waitForFunction(()=>!document.getElementById('jobs-refresh').disabled);
  await page.locator('#jobs-list button').filter({hasText:'private task'}).click();await page.waitForFunction(()=>!document.getElementById('runs-refresh').disabled);
  assert((await page.locator('#job-runs').textContent()).includes('private scheduled result'));
  assert(await page.locator('#job-toggle').isDisabled());assert(await page.locator('#job-delete').isDisabled());assert.strictEqual(await page.locator('#job-create').isVisible(),false);
  await page.evaluate(()=>{
   for(const id of ['job-toggle','job-delete','job-create-retry']){document.getElementById(id).disabled=false;document.getElementById(id).click();}
   document.getElementById('job-create').hidden=false;document.getElementById('job-name').value='must not create';document.getElementById('job-prompt').value='must not run';
   document.getElementById('job-form').dispatchEvent(new Event('submit',{bubbles:true,cancelable:true}));
  });await page.waitForFunction(()=>!document.getElementById('jobs-refresh').disabled);
  await page.locator('#runs-refresh').click();await page.waitForFunction(()=>!document.getElementById('runs-refresh').disabled);
  assert.strictEqual(models,baselineModels);assert.strictEqual(writes.length,baselineWrites);
  assert.strictEqual((await api(read.token,'/api/jobs/'+job.id)).body.enabled,false);
  assert.strictEqual((await cli('user-list')).users[0].hold,null);
  assert.strictEqual(await page.locator('#api-token').inputValue(),'');assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);
  await page.setViewportSize({width:390,height:844});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth));await page.setViewportSize({width:1360,height:900});
  // Switching permission clears private drafts, selection and tasks before re-enabling writes.
  await connect(full.token,false);assert(await page.locator('#new-session').isEnabled());assert.strictEqual(await page.locator('#job-prompt').inputValue(),'');assert.strictEqual(await page.locator('#message').inputValue(),'');assert.strictEqual(await page.locator('#messages .message').count(),0);
  await page.locator('#new-session').click();await idle();await page.locator('#message').fill('full permission still works');await page.locator('#send').click();await page.waitForFunction(()=>document.getElementById('status').textContent==='回复已保存');assert(models>baselineModels);
  const afterFull=writes.length;await viewer();assert.strictEqual(writes.length,afterFull);
  // An old full-permission capability response must not upgrade the new identity.
  let waiting=false;
  await page.route('**/api/gateway/capabilities',async route=>{if(route.request().headers().authorization!=='Bearer '+full.token)return route.continue();waiting=true;await new Promise(resolve=>releaseCapability=resolve);return route.fulfill({status:200,contentType:'application/json',body:JSON.stringify({scheduled_jobs:true,read_only:false})});});
  await page.locator('#api-token').fill(full.token);await page.getByRole('button',{name:'连接',exact:true}).click();await eventually(()=>waiting,'held capability');
  await connect(read.token,true);releaseCapability();releaseCapability=null;await sleep(100);assert(await page.locator('#access-mode').isVisible());assert(await page.locator('#new-session').isDisabled());await page.unroute('**/api/gateway/capabilities');
  // Malformed privilege metadata fails closed and clears fetched history.
  await page.route('**/api/gateway/capabilities',route=>route.fulfill({status:200,contentType:'application/json',body:JSON.stringify({scheduled_jobs:true,read_only:'false'})}));
  await page.locator('#api-token').fill(read.token);await page.getByRole('button',{name:'连接',exact:true}).click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('权限信息响应异常'));
  assert(await page.locator('#refresh').isDisabled());assert.strictEqual(await page.locator('#sessions button').count(),0);assert.strictEqual(await page.locator('#access-mode').isVisible(),false);await page.unroute('**/api/gateway/capabilities');
  // Readonly paging goes through the real gateway allowlist and private DB.
  await viewer();
  const priorSessions=(await api(read.token,'/api/sessions')).body.sessions.length;
  for(let i=0;i<53;i++)assert.strictEqual((await api(full.token,'/api/sessions','POST')).status,200);
  await page.locator('#refresh').click();await page.waitForFunction(()=>document.querySelectorAll('#sessions button').length===50&&!document.getElementById('session-next').disabled);
  const listed=await page.locator('#sessions button').evaluateAll(buttons=>buttons.map(b=>b.title));
  await page.locator('#session-next').click();await page.waitForFunction(()=>document.getElementById('status').textContent==='已读取下一页会话');
  const next=await page.locator('#sessions button').evaluateAll(buttons=>buttons.map(b=>b.title));assert.strictEqual(new Set([...listed,...next]).size,53+priorSessions);assert.strictEqual(next.length,53+priorSessions-50);assert(await page.locator('#session-next').isDisabled());
  assert(await page.locator('#new-session').isDisabled());assert(await page.locator('#send').isDisabled());
  console.log('PASS: actual readonly gateway Chromium catalog pagination, private history authority retained');
  await cli('key-revoke','--key',read.key_id);await page.locator('#refresh').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));
  assert.strictEqual(await page.locator('#sessions button').count(),0);assert.strictEqual(await page.locator('#messages .message').count(),0);assert.strictEqual(await page.locator('#access-mode').isVisible(),false);assert(await page.locator('#new-session').isDisabled());
  await page.reload();assert(await page.locator('#refresh').isDisabled());assert.deepStrictEqual(errors,[]);
  console.log('PASS: real gateway/SQLite/Chromium read-only history and scheduled results; forced DOM sends zero mutations/models/holds; full-key switch, stale capability, malformed metadata and revocation fail closed; memory-only token/mobile layout');
 }finally{
  if(releaseReceipt)releaseReceipt();if(releaseCapability)releaseCapability();if(browser)await browser.close();
  for(const process of processes.reverse()){if(process.exitCode===null&&process.signalCode===null){const ended=new Promise(resolve=>process.once('exit',resolve));process.kill('SIGTERM');let timer;await Promise.race([ended,new Promise(resolve=>timer=setTimeout(()=>{process.kill('SIGKILL');resolve();},10000))]);clearTimeout(timer);}}
  await new Promise(resolve=>bridge.close(resolve));await new Promise(resolve=>model.close(resolve));fs.rmSync(root,{recursive:true,force:true});
 }
})().catch(error=>{console.error(error);process.exitCode=1;});
