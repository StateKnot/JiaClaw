// Real gateway, SQLite backend and Chromium; only localhost model fixtures.
const {chromium}=require('playwright');
const fs=require('fs'),os=require('os'),path=require('path'),crypto=require('crypto'),child=require('child_process'),http=require('http'),assert=require('assert'),util=require('util');
const execFile=util.promisify(child.execFile),sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms));
(async()=>{
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'jiaclaw-read-only-browser-')),workspace=path.join(root,'workspace');fs.mkdirSync(workspace);
 const binary=path.resolve(process.argv[2]||'target/debug/jiaclaw'),backendToken=crypto.randomUUID(),modelToken=crypto.randomUUID();
 const env=Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('JIACLAW_')));Object.assign(env,{JIACLAW_LOG_LEVEL:'info',JIACLAW_LOG_FORMAT:'text'});
 const processes=[],errors=[],writes=[];let browser,base,models=0,releaseCapability;
 const model=http.createServer(async(req,res)=>{try{
  assert.strictEqual(req.url,'/v1/chat/completions');assert.strictEqual(req.headers.authorization,'Bearer '+modelToken);assert(req.headers['idempotency-key']);
  let raw='';for await(const chunk of req)raw+=chunk;const body=JSON.parse(raw);models++;
  const prompt=[...body.messages].reverse().find(message=>message.role==='user').content;
  res.writeHead(200,{'content-type':'application/json'});res.end(JSON.stringify({choices:[{message:{role:'assistant',content:'saved: '+prompt},finish_reason:'stop'}]}));
 }catch(error){errors.push(error.name);res.writeHead(500);res.end();}});
 const cfg=path.join(root,'gateway.json');
 async function cli(...args){const result=await execFile(binary,['gateway',...args,'--config',cfg],{env,timeout:15000});return JSON.parse(result.stdout);}
 async function eventually(check,label,timeout=20000){const until=Date.now()+timeout;while(Date.now()<until){assert.deepStrictEqual(errors,[]);const result=await check();if(result)return result;await sleep(40);}throw new Error(label+' timed out');}
 async function launch(args,pattern){let log='';const process=child.spawn(binary,args,{env});processes.push(process);process.stdout.on('data',data=>log+=data);process.stderr.on('data',data=>log+=data);return eventually(()=>{assert.strictEqual(process.exitCode,null,'host exited before startup');const match=log.match(pattern);return match&&match[1];},'startup');}
 async function api(key,url,method='GET',body){const response=await fetch(base+url,{method,headers:{authorization:'Bearer '+key,...(body?{'content-type':'application/json'}:{})},body:body?JSON.stringify(body):undefined,signal:AbortSignal.timeout(10000)});const raw=await response.text();return {status:response.status,body:raw?JSON.parse(raw):null};}
 try{
  await new Promise(resolve=>model.listen(0,'127.0.0.1',resolve));
  const settings={agent:{name:'viewer',description:'Local acceptance',system_instructions:'Respond with text.',max_turns:10,workspace_path:workspace},provider:{provider_type:'brokerrouter',base_url:'http://127.0.0.1:'+model.address().port,api_key:modelToken,model:'fixture'},http:{bind:'127.0.0.1:0',api_token:backendToken,persist:true,persist_path:'../state/sessions.sqlite3',shutdown_timeout_secs:1},scheduler:{enabled:true,gateway_driven:true}};
  const backendCfg=path.join(root,'backend.json');fs.writeFileSync(backendCfg,JSON.stringify(settings));
  const backend=await launch(['serve','--config',backendCfg],/HTTP 服务已启动于 (http:\/\/127\.0\.0\.1:\d+)/);
  // Reserve a free local port; production gateway config intentionally requires a fixed bind.
  const reservation=http.createServer();await new Promise(resolve=>reservation.listen(0,'127.0.0.1',resolve));const gatewayPort=reservation.address().port;await new Promise(resolve=>reservation.close(resolve));
  const secret=path.join(root,'backend.secret');fs.writeFileSync(secret,backendToken,{mode:0o600});
  fs.writeFileSync(cfg,JSON.stringify({bind:'127.0.0.1:'+gatewayPort,registry_path:path.join(root,'registry/users.sqlite3'),request_timeout_seconds:150,max_in_flight:4,scheduled_jobs:true,backends:[{id:'viewer',url:backend,token_file:secret}]}));
  const full=await cli('user-add','--backend','viewer'),read=await cli('key-add','--user',full.user_id,'--read-only');assert.strictEqual(full.read_only,false);assert.strictEqual(read.read_only,true);
  // Wait through health rather than depending on a gateway log message.
  const gateway=child.spawn(binary,['gateway','serve','--config',cfg],{env});processes.push(gateway);gateway.stdout.resume();gateway.stderr.resume();base='http://127.0.0.1:'+gatewayPort;
  await eventually(async()=>{assert.strictEqual(gateway.exitCode,null);try{return (await fetch(base+'/health',{signal:AbortSignal.timeout(1000)})).status===200;}catch{return false;}},'gateway startup');
  const session=(await api(full.token,'/api/sessions','POST')).body.session_id;
  assert.strictEqual((await api(full.token,'/api/chat','POST',{session_id:session,messages:[{role:'user',content:'private history <img src=x onerror="window.XSS=1">'}],stream:false})).status,200);
  const job=(await api(full.token,'/api/jobs','POST',{name:'private task',prompt:'private scheduled result',schedule:{kind:'interval',seconds:1},enabled_tools:['datetime_now'],timeout_secs:120})).body;
  await eventually(async()=>{const response=await api(full.token,'/api/jobs/'+job.id+'/runs?limit=5&offset=0');return response.body.items.some(run=>run.status==='completed');},'scheduled result');
  assert.strictEqual((await api(full.token,'/api/jobs/'+job.id+'/pause','POST')).status,200);
  // Pause blocks future dispatches; wait for any admitted completion before checking model counts.
  await eventually(async()=>{const users=(await cli('user-list')).users;return users[0].hold===null;},'gateway idle');
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
  await viewer();await cli('key-revoke','--key',read.key_id);await page.locator('#refresh').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));
  assert.strictEqual(await page.locator('#sessions button').count(),0);assert.strictEqual(await page.locator('#messages .message').count(),0);assert.strictEqual(await page.locator('#access-mode').isVisible(),false);assert(await page.locator('#new-session').isDisabled());
  await page.reload();assert(await page.locator('#refresh').isDisabled());assert.deepStrictEqual(errors,[]);
  console.log('PASS: real gateway/SQLite/Chromium read-only history and scheduled results; forced DOM sends zero mutations/models/holds; full-key switch, stale capability, malformed metadata and revocation fail closed; memory-only token/mobile layout');
 }finally{
  if(releaseCapability)releaseCapability();if(browser)await browser.close();
  for(const process of processes.reverse()){if(process.exitCode===null&&process.signalCode===null){const ended=new Promise(resolve=>process.once('exit',resolve));process.kill('SIGTERM');let timer;await Promise.race([ended,new Promise(resolve=>timer=setTimeout(()=>{process.kill('SIGKILL');resolve();},10000))]);clearTimeout(timer);}}
  await new Promise(resolve=>model.close(resolve));fs.rmSync(root,{recursive:true,force:true});
 }
})().catch(error=>{console.error(error);process.exitCode=1;});
