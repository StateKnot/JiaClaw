const { chromium } = require('playwright');
const fs = require('fs'), os = require('os'), path = require('path'), crypto = require('crypto'), child = require('child_process'), assert = require('assert');
(async () => {
 const root = fs.mkdtempSync(path.join(os.tmpdir(), 'jiaclaw-browser-'));
 const token = crypto.randomUUID();
 fs.mkdirSync(path.join(root, 'workspace'));
 fs.writeFileSync(path.join(root,'config.json'),JSON.stringify({agent:{name:'JiaClaw',description:'Browser acceptance',system_instructions:'Test assistant',max_turns:10,workspace_path:path.join(root,'workspace')},provider:{provider_type:'stub'},http:{bind:'127.0.0.1:0',api_token:token,persist:true,persist_path:'../state/sessions.sqlite3'}}));
 const server = child.spawn(path.resolve(process.argv[2] || 'target/debug/jiaclaw'),['serve','--config',path.join(root,'config.json')],{env:{...process.env,JIACLAW_LOG_LEVEL:'info',JIACLAW_LOG_FORMAT:'text',JIACLAW_API_TOKEN:token}});
 let log='', browser;
 server.stdout.on('data',d=>log+=d);server.stderr.on('data',d=>log+=d);
 try {
  const base = await new Promise((resolve,reject)=>{const deadline=Date.now()+20000;function poll(){let m=log.match(/HTTP 服务已启动于 (http:\/\/127\.0\.0\.1:\d+)/);if(m)return resolve(m[1]);if(Date.now()>deadline)return reject(new Error(log));setTimeout(poll,50)}poll()});
  browser=await chromium.launch({headless:true,...(process.env.JIACLAW_TEST_CHROMIUM ? {executablePath:process.env.JIACLAW_TEST_CHROMIUM} : {})});
  const page=await browser.newPage({viewport:{width:1360,height:900}});
  const errors=[];page.on('pageerror',e=>errors.push(String(e)));
  await page.goto(base);
  await page.locator('#api-token').fill(token);await page.getByRole('button',{name:'连接',exact:true}).click();
  await page.getByRole('button',{name:'＋ 新建',exact:true}).click();
  await page.locator('#message').fill('帮我整理今天的工作计划。');await page.locator('#send').click();
  await page.waitForFunction(()=>document.querySelectorAll('.message').length===2 && document.getElementById('status').textContent==='回复已保存');

  await page.locator('#message').fill('<img src=x onerror="window.XSS=1">');await page.locator('#send').click();
  await page.waitForFunction(()=>document.querySelectorAll('.message').length===4 && document.getElementById('status').textContent==='回复已保存');
  assert.strictEqual(await page.locator('#messages img').count(),0);
  assert.strictEqual(await page.evaluate(()=>typeof window.XSS),'undefined');
  assert.strictEqual(await page.locator('#api-token').inputValue(),'');
  assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);
  // A failed identity switch must not leave the previous user's data or draft visible.
  await page.locator('#message').fill('private draft from previous identity');
  await page.locator('#api-token').fill('invalid-other-identity');
  await page.getByRole('button',{name:'连接',exact:true}).click();
  await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));
  assert.strictEqual(await page.locator('#messages .message').count(),0);
  assert.strictEqual(await page.locator('#sessions button').count(),0);
  assert.strictEqual(await page.locator('#message').inputValue(),'');
  for (const id of ['new-session','refresh','delete-session','message','send']) assert.strictEqual(await page.locator('#'+id).isDisabled(),true);
  await page.locator('#api-token').fill(token);await page.getByRole('button',{name:'连接',exact:true}).click();
  await page.waitForFunction(()=>document.getElementById('status').textContent==='已连接 · 选择或新建会话');
  assert.strictEqual(await page.locator('#messages .message').count(),0);
  assert.strictEqual(await page.locator('#send').isDisabled(),true);
  await page.locator('#sessions button').first().click();
  await page.waitForFunction(()=>document.querySelectorAll('.message').length===4);
  // Revocation can return a non-JSON 401: clear identity before parsing the body.
  await page.route('**/api/sessions',route=>route.fulfill({status:401,contentType:'text/plain',body:'unauthorized'}));
  await page.locator('#message').fill('private draft after reconnect');
  await page.locator('#refresh').click();
  await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));
  assert.strictEqual(await page.locator('#messages .message').count(),0);
  assert.strictEqual(await page.locator('#sessions button').count(),0);
  assert.strictEqual(await page.locator('#message').inputValue(),'');
  assert.strictEqual(await page.locator('#send').isDisabled(),true);
  await page.unroute('**/api/sessions');
  await page.locator('#api-token').fill(token);await page.getByRole('button',{name:'连接',exact:true}).click();
  await page.locator('#sessions button').first().click();
  await page.waitForFunction(()=>document.querySelectorAll('.message').length===4);
  // A 2xx response may still carry an unresolved write receipt: preserve the draft.
  let reviewedChatCalls=0;
  await page.route('**/api/chat',route=>{reviewedChatCalls++;return route.fulfill({status:200,contentType:'application/json',headers:{'x-jiaclaw-write-review':'required'},body:JSON.stringify({status:'requireshumaninput'})});});
  await page.locator('#message').fill('chat draft requiring review');await page.locator('#send').click();
  await page.waitForFunction(()=>document.getElementById('status').textContent.includes('勿重复提交'));
  assert.strictEqual(await page.locator('#message').inputValue(),'chat draft requiring review');
  assert.strictEqual(await page.locator('#messages .message').count(),4);assert.strictEqual(reviewedChatCalls,1);
  await page.unroute('**/api/chat');
  // Standalone offers outbox audit but no gateway jobs capability.
  assert.strictEqual(await page.locator('#jobs-tab').isVisible(),false);
  assert.strictEqual(await page.locator('#outbox-tab').isVisible(),true);
  const taskId=crypto.randomUUID(), runId=crypto.randomUUID();
  let job={id:taskId,spec:{name:'<img src=x onerror="window.JOB_XSS=1">',prompt:'private task prompt',schedule:{kind:'interval',seconds:3600},enabled_tools:['datetime_now'],timeout_secs:120},enabled:true,deleted:false,next_due_ms:Date.now()+3600000,session_id:'job:'+taskId};
  let created=false, heldRun=null, delayRun=false;
  const calls=[];
  await page.route('**/api/gateway/capabilities',route=>route.fulfill({status:200,contentType:'application/json',body:JSON.stringify({scheduled_jobs:true})}));
  // This synthetic jobs gateway has no tracked-turn feature. Do not expose the
  // underlying standalone server's differently scoped turn capabilities.
  await page.route('**/api/turns/capabilities',route=>route.fulfill({status:404,body:''}));
  await page.route('**/api/jobs**',async route=>{
   const req=route.request(), u=new URL(req.url()), method=req.method();calls.push([method,u.pathname,u.search]);
   if(u.pathname==='/api/jobs/status')return route.fulfill({status:200,contentType:'application/json',body:JSON.stringify({state:'running'})});
   if(u.pathname==='/api/jobs'&&method==='POST'){const spec=req.postDataJSON();assert.deepStrictEqual(spec.enabled_tools,['datetime_now']);job={...job,spec};created=true;return route.fulfill({status:201,contentType:'application/json',headers:spec.name==='review-required-draft'?{'x-jiaclaw-write-review':'required'}:{},body:JSON.stringify(job)});}
   if(u.pathname==='/api/jobs'&&method==='GET'){assert.strictEqual(u.searchParams.get('limit'),'5');return route.fulfill({status:200,contentType:'application/json',body:JSON.stringify({items:created&&(!job.deleted||u.searchParams.get('include_deleted')==='true')?[job]:[],next_offset:null})});}
   if(u.pathname.endsWith('/runs')){if(delayRun){delayRun=false;await new Promise(r=>heldRun=r);}return route.fulfill({status:200,contentType:'application/json',body:JSON.stringify({items:[{id:runId,job_id:taskId,status:'completed',scheduled_for_ms:Date.now(),response:{message:{content:'<img src=x onerror="window.RUN_XSS=1">'+ 'x'.repeat(20000)}}}],next_offset:null})});}
   if(u.pathname.endsWith('/pause'))job.enabled=false;
   if(u.pathname.endsWith('/resume'))job.enabled=true;
   if(method==='DELETE'){job.deleted=true;job.enabled=false;return route.fulfill({status:204,body:''});}
   return route.fulfill({status:200,contentType:'application/json',body:JSON.stringify(job)});
  });
  await page.locator('#api-token').fill(token);await page.getByRole('button',{name:'连接',exact:true}).click();
  await page.waitForFunction(()=>document.getElementById('status').textContent==='已连接 · 选择或新建会话');
  await page.locator('#jobs-tab').click();await page.locator('#job-create summary').click();
  await page.locator('#job-name').fill(job.spec.name);await page.locator('#job-prompt').fill(job.spec.prompt);await page.locator('#job-submit').click();
  await page.waitForFunction(()=>document.getElementById('status').textContent==='任务已创建并启用');
  assert.strictEqual(await page.locator('#jobs-view img').count(),0);
  assert.strictEqual(await page.evaluate(()=>typeof window.JOB_XSS+typeof window.RUN_XSS),'undefinedundefined');
  assert((await page.locator('#job-runs').textContent()).includes('显示已截断'));
  assert.strictEqual(await page.locator('#jobs-view a').count(),0); // No broken job: session links.
  await page.setViewportSize({width:390,height:844});
  assert(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth));
  await page.setViewportSize({width:1360,height:900});
  await page.locator('#job-toggle').click();await page.waitForFunction(()=>document.getElementById('job-toggle').textContent==='恢复任务');
  page.on('dialog',d=>d.accept());
  await page.locator('#job-toggle').click();await page.waitForFunction(()=>document.getElementById('job-toggle').textContent==='暂停任务');
  await page.locator('#job-delete').click();await page.waitForFunction(()=>document.getElementById('status').textContent==='任务已删除，运行记录已保留');
  assert.strictEqual(await page.locator('#job-delete').isDisabled(),true);
  assert(calls.some(([method,p])=>method==='DELETE'&&p==='/api/jobs/'+taskId));
  await page.locator('#job-create summary').click();
  await page.locator('#job-name').fill('review-required-draft');await page.locator('#job-prompt').fill('job draft requiring review');await page.locator('#job-submit').click();
  await page.waitForFunction(()=>document.getElementById('status').textContent.includes('勿重复提交'));
  assert.strictEqual(await page.locator('#job-name').inputValue(),'review-required-draft');
  assert.strictEqual(await page.locator('#job-prompt').inputValue(),'job draft requiring review');
  assert.strictEqual(calls.filter(([method,p])=>method==='POST'&&p==='/api/jobs').length,2);
  // An old result response arriving after a failed identity switch cannot repopulate the task DOM.
  delayRun=true;await page.locator('#runs-refresh').click();
  await page.waitForFunction(()=>document.getElementById('runs-refresh').disabled);
  while(!heldRun)await new Promise(r=>setTimeout(r,10));
  await page.locator('#job-create summary').click();await page.locator('#api-token').fill('invalid-task-identity');
  await page.getByRole('button',{name:'连接',exact:true}).click();
  await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));
  heldRun();await page.waitForTimeout(100);
  for(const id of ['jobs-list','job-runs','job-title','job-summary','job-detail-prompt'])assert.strictEqual(await page.locator('#'+id).textContent(),'');
  assert.strictEqual(await page.locator('#job-name').inputValue(),'');assert.strictEqual(await page.locator('#job-prompt').inputValue(),'');
  assert.strictEqual(await page.locator('#workspace-tabs').isVisible(),false);
  await page.unroute('**/api/jobs**');await page.unroute('**/api/gateway/capabilities');await page.unroute('**/api/turns/capabilities');
  await page.locator('#api-token').fill(token);await page.getByRole('button',{name:'连接',exact:true}).click();
  await page.locator('#sessions button').first().click();
  await page.setViewportSize({width:390,height:844});
  assert(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth));
  await page.locator('#delete-session').click();
  await page.waitForFunction(()=>document.getElementById('status').textContent==='会话已删除');
  assert.strictEqual(await page.locator('#sessions button').count(),0);
  // Actual server catalog, not a synthetic page response. DOM keeps one page.
  for(let i=0;i<53;i++){const response=await fetch(base+'/api/sessions',{method:'POST',headers:{authorization:'Bearer '+token}});assert.strictEqual(response.status,200);}
  await page.locator('#refresh').click();await page.waitForFunction(()=>document.querySelectorAll('#sessions button').length===50&&!document.getElementById('session-next').disabled);
  const firstIds=await page.locator('#sessions button').evaluateAll(buttons=>buttons.map(b=>b.title));
  await page.locator('#sessions button').first().click();
  await page.waitForFunction(id=>document.getElementById('session-title').textContent===`会话 ${id.slice(0,12)}`&&document.getElementById('status').textContent==='已连接 · 会话就绪'&&!document.getElementById('send').disabled, firstIds[0]);
  const title=await page.locator('#session-title').textContent();
  await page.locator('#session-next').click();await page.waitForFunction(()=>document.getElementById('status').textContent==='已读取下一页会话');
  assert.strictEqual(await page.locator('#sessions button').count(),3);assert(await page.locator('#session-next').isDisabled());
  assert.strictEqual(await page.locator('#session-title').textContent(),title);assert.strictEqual(await page.locator('#send').isDisabled(),false);
  const lastIds=await page.locator('#sessions button').evaluateAll(buttons=>buttons.map(b=>b.title));assert.strictEqual(new Set([...firstIds,...lastIds]).size,53);
  await page.locator('#refresh').click();await page.waitForFunction(()=>document.querySelectorAll('#sessions button').length===50&&!document.getElementById('session-next').disabled);
  let releasePage, pageReached=false;
  await page.route('**/api/sessions?after=*',async route=>{const response=await route.fetch();pageReached=true;await new Promise(resolve=>releasePage=resolve);await route.fulfill({response});});
  await page.locator('#session-next').click();while(!pageReached)await new Promise(resolve=>setTimeout(resolve,10));
  await page.locator('#api-token').fill('invalid-pagination-identity');await page.getByRole('button',{name:'连接',exact:true}).click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));
  releasePage();await page.waitForTimeout(100);assert.strictEqual(await page.locator('#sessions button').count(),0);assert(await page.locator('#session-next').isDisabled());assert.strictEqual(await page.locator('#messages .message').count(),0);
  await page.unroute('**/api/sessions?after=*');
  console.log('PASS: real Chromium session catalog bounded pages, stable selection, first-page reset and stale identity response cleared');
  await page.reload();assert.strictEqual(await page.locator('#new-session').isDisabled(),true);
  assert.deepStrictEqual(errors,[]);
  console.log('PASS: real Chromium connect/create/chat/delete; identity-switch and revoked-key state clearing; text-only model rendering; capability-gated task CRUD/204, 2xx review headers preserve chat/job drafts, text-only bounded results and stale identity guard; memory-only token; mobile layout');
 } finally {if(browser)await browser.close();server.kill('SIGTERM');await new Promise(r=>server.once('exit',r));fs.rmSync(root,{recursive:true,force:true});}
})().catch(e=>{console.error(e);process.exitCode=1});
