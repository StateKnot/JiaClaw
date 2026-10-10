const {chromium}=require('playwright'), assert=require('assert'), fs=require('fs');
const cfg=JSON.parse(fs.readFileSync(process.argv[2])), wait=ms=>new Promise(r=>setTimeout(r,ms));
async function ctl(action,prompt='none',who='alice',id) {const r=await fetch(cfg.control,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({action,prompt,who,id})});const j=await r.json();assert(r.ok,JSON.stringify(j));return j.value;}
async function until(fn,label,seconds=15) {const end=Date.now()+seconds*1000;while(Date.now()<end){const r=await fn();if(r)return r;await wait(30);}throw new Error('timeout: '+label);}
const original=async(id,token=cfg.alice)=>{const r=await fetch(cfg.base+'/api/turns/'+id,{headers:{Authorization:'Bearer '+token}});const v=await r.json();assert.strictEqual(r.status,200,JSON.stringify(v));return v;};
(async()=>{
 const browser=await chromium.launch({headless:true,...(process.env.JIACLAW_TEST_CHROMIUM?{executablePath:process.env.JIACLAW_TEST_CHROMIUM}:{})});console.log('BROWSER VERSION '+browser.version());
 const page=await browser.newPage({viewport:{width:1360,height:1000}}), errors=[], requests=[], puts=[];
 page.on('pageerror',e=>errors.push(String(e)));page.on('request',r=>{const path=new URL(r.url()).pathname;requests.push({path,method:r.method(),auth:r.headers().authorization});if(r.method()==='PUT')puts.push({path,body:r.postDataJSON()});});
 const connect=async(token=cfg.alice)=>{await page.locator('#api-token').fill(token);await page.locator('#connect-form button').click();await page.waitForFunction(()=>!document.getElementById('turn-workspace').hidden&&!document.getElementById('status').textContent.startsWith('连接中'));};
 const begin=async prompt=>{await page.locator('#new-session').click();await page.locator('#turn-permissions summary').click();assert.strictEqual(await page.locator('#turn-permissions input:checked').count(),0);await page.locator('#turn-tools input[value="datetime_now"]').check();await page.locator('#message').fill(prompt);await page.locator('#send').click();await page.waitForFunction(()=>document.getElementById('turn-id').value.length===36);return page.locator('#turn-id').inputValue();};
 const finish=async()=>{await page.waitForFunction(()=>!document.getElementById('turn-finish').disabled);await page.locator('#turn-finish').click();};
 const idle=async(who='alice')=>until(async()=>!(await ctl('status','none',who)).holds.length,'actual gateway settlement');
 const terminal=async(id,token=cfg.alice)=>until(async()=>{const v=await original(id,token);return v.receipt.state!=='running'&&!v.active?v:null;},'actual terminal');
 const noStorage=async()=>assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);
 const authorityChecks=async(who='alice')=>{
  for(const mode of ['final-GET','duplicate-200']){
   let firstPut=true;
   await page.route('**/api/turns/**',async route=>{
    const req=route.request(),path=new URL(req.url()).pathname;
    if(mode==='final-GET'&&req.method()==='GET'&&/^\/api\/turns\/[0-9a-f-]{36}$/.test(path)){
     const r=await route.fetch({maxRetries:0,maxRedirects:0}),v=await r.json();v.receipt.result.tool_names=['json_query'];return route.fulfill({response:r,json:v});
    }
    if(mode==='duplicate-200'&&req.method()==='PUT'&&path.endsWith('/stream')){
     const r=await route.fetch({maxRetries:0,maxRedirects:0});
     if(firstPut){firstPut=false;assert.strictEqual(r.status(),202);return route.abort('failed');}
     assert.strictEqual(r.status(),200);const v=await r.json();v.receipt.result.tool_names=['json_query'];return route.fulfill({response:r,json:v});
    }
    return route.continue();
   });
   const prompt='preview-authority-'+mode,id=await begin(prompt);
   if(mode==='duplicate-200'){await page.waitForFunction(()=>!document.getElementById('turn-retry').disabled);page.once('dialog',d=>d.accept());await page.locator('#turn-retry').click();}
   await page.waitForFunction(()=>document.getElementById('status').textContent.includes('协议')||!document.getElementById('turn-finish').disabled);
   assert.strictEqual(await page.locator('#turn-finish').isDisabled(),true,mode+' must reject unselected result tools');assert.strictEqual(await page.locator('#message').inputValue(),prompt);assert.strictEqual(await page.locator('#turn-id').inputValue(),id);assert.strictEqual((await ctl('status',prompt,who)).count,1);
   const before=puts.length;await page.unroute('**/api/turns/**');await page.locator('#turn-check').click();await finish();assert.strictEqual(puts.length,before);await idle(who);
  }
  console.log('PASS tenant preview 9: actual normal done plus corrupted final GET/duplicate JSON 200 cannot authorize an unselected tool or clear the original draft; correct original GET reconciles once');
 };
 try {
  if(process.env.JIACLAW_TEST_PREVIEW_AUTHORITY_ONLY==='1'){await page.goto(cfg.base);await connect();await authorityChecks();return;}
  await page.goto(cfg.base);await connect();
  const c=await fetch(cfg.base+'/api/turns/capabilities',{headers:{Authorization:'Bearer '+cfg.alice}}).then(r=>r.json());assert.strictEqual(c.gateway_protocol,2);assert.strictEqual(c.streaming,true);
  const prompt='<img src=x onerror="window.TENANT_XSS=1">流式🦀';await ctl('gate',prompt);const first=await begin(prompt);
  await until(async()=>(await ctl('status',prompt)).ready,'actual model before settlement');
  assert.strictEqual(puts[0].path,'/api/turns/'+first+'/stream','protocol 2 must use actual stream route');
  await page.waitForFunction(()=>document.querySelector('.provisional p')?.textContent.includes('alice: '));
  let value=await original(first);assert.strictEqual(value.receipt.state,'running');assert.strictEqual(value.receipt.session_committed,false);assert.strictEqual(await page.locator('#turn-finish').isDisabled(),true);assert.strictEqual(await page.locator('#message').inputValue(),prompt);
  assert.deepStrictEqual(puts[0].body.enabled_tools,['datetime_now']);assert.deepStrictEqual(puts[0].body.enabled_skills,[]);assert.strictEqual((await ctl('status',prompt)).count,1);
  await ctl('release',prompt);await finish();value=await original(first);assert.strictEqual(value.receipt.state,'completed');assert((await page.locator('#messages').textContent()).includes('alice: '+prompt));assert.strictEqual(await page.locator('#messages img').count(),0);assert.strictEqual(await page.evaluate(()=>typeof window.TENANT_XSS),'undefined');assert.strictEqual(await page.locator('#message').inputValue(),'');assert(requests.some(r=>r.method==='GET'&&r.path==='/api/turns/'+first));await idle();await noStorage();
  console.log('PASS tenant preview 1: raw protocol-2 capability, actual pre-settlement preview, explicit tools, original GET/committed reply, text-only Unicode and no stored credentials');

  await ctl('gate','preview-reload');const reload=await begin('preview-reload');await page.waitForFunction(()=>document.querySelector('.provisional p')?.textContent.includes('alice: '));const beforeReload=puts.length;
  await page.reload();assert.strictEqual(await page.locator('#api-token').inputValue(),'');await connect(cfg.readonly);assert.strictEqual(await page.locator('#turn-id').inputValue(),reload);assert.strictEqual(await page.locator('#turn-cancel').isDisabled(),true);assert.strictEqual(await page.locator('#send').isDisabled(),true);assert.strictEqual(await page.locator('#turn-retry').isDisabled(),true);assert.strictEqual(puts.length,beforeReload);
  await ctl('release','preview-reload');value=await terminal(reload);
  if(value.receipt.state==='needs_review') {await until(async()=>(await ctl('status','preview-reload')).holds[0]?.state==='needs_review','reload review hold');await ctl('review','none','alice',reload);}
  await page.locator('#turn-check').click();await finish();assert.strictEqual(puts.length,beforeReload);assert.strictEqual((await ctl('status','preview-reload')).count,1);await idle();await noStorage();
  await connect(cfg.bob);await page.locator('#turn-lookup-id').fill(reload);await page.locator('#turn-lookup button').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('unavailable'));assert.strictEqual(await page.locator('#messages .message').count(),0);assert.strictEqual(await page.locator('#turn-finish').isDisabled(),true);await connect();
  console.log('PASS tenant preview 2: actual stream reload closes delivery, readonly original GET recovery, independent admin review when required, foreign identity hidden and no replay');

  await ctl('gate','preview-cancel');const cancelled=await begin('preview-cancel');await page.waitForFunction(()=>document.querySelector('.provisional p')?.textContent.includes('alice: '));await page.locator('#turn-cancel').click();await until(async()=>(await original(cancelled)).receipt.cancel_requested,'persisted original cancellation');
  assert.strictEqual((await original(cancelled)).active,true);assert.strictEqual(await page.locator('#message').inputValue(),'preview-cancel');await ctl('release','preview-cancel');value=await terminal(cancelled);assert.strictEqual(value.receipt.state,'needs_review');await until(async()=>(await ctl('status','preview-cancel')).holds[0]?.state==='needs_review','actual cancellation review hold');
  await page.locator('#turn-check').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('核对'));assert.strictEqual(await page.locator('#turn-finish').isDisabled(),true);assert.strictEqual(await page.locator('#turn-review').isVisible(),false);assert.strictEqual(await page.locator('#turn-abandon').isDisabled(),true);await ctl('review','none','alice',cancelled);await page.locator('#turn-check').click();await finish();assert.strictEqual((await ctl('status','preview-cancel')).count,1);await idle();
  console.log('PASS tenant preview 3: durable cancel precedes delivery abort, actual model owner continues, needs-review hold requires independent admin, personal UI cannot clear it');

  // Frontend terminal fault only: current real gateway and model complete once.
  // The final receipt claims a globally allowed but unselected tool. No complete
  // delivery/authority claim may clear this draft; only the original GET can.
  await page.route('**/api/turns/*/stream',async route=>{const r=await route.fetch({maxRetries:0,maxRedirects:0});assert.strictEqual(r.status(),202);const body=(await r.text()).replace(/event: done\ndata: ([^\n]+)/,(_all,raw)=>{const v=JSON.parse(raw);v.receipt.result.tool_names=['json_query'];return 'event: done\ndata: '+JSON.stringify(v);});await route.fulfill({response:r,body});});
  const bad=await begin('preview-invalid-terminal');await page.waitForFunction(()=>document.getElementById('status').textContent.includes('协议'));assert.strictEqual(await page.locator('#turn-id').inputValue(),bad);assert.strictEqual(await page.locator('#message').inputValue(),'preview-invalid-terminal');assert.strictEqual(await page.locator('#turn-finish').isDisabled(),true);assert.strictEqual(await page.locator('#send').isDisabled(),true);assert.strictEqual((await ctl('status','preview-invalid-terminal')).count,1);const faultPuts=puts.length;await page.unroute('**/api/turns/*/stream');await page.locator('#turn-check').click();await finish();assert.strictEqual(puts.length,faultPuts);await idle();
  console.log('PASS tenant preview 4: actual completed original with malformed terminal tool authority retains draft/identity; explicit original GET reconciles without dispatch');

  let lostBody;await page.route('**/api/turns/*/stream',async route=>{lostBody=route.request().postDataJSON();const r=await route.fetch({maxRetries:0,maxRedirects:0});assert.strictEqual(r.status(),202);await route.abort('failed');});
  const lost=await begin('preview-response-loss');await page.waitForFunction(()=>!document.getElementById('turn-retry').disabled);assert.strictEqual(await page.locator('#message').inputValue(),'preview-response-loss');const lostPuts=puts.length;await wait(250);assert.strictEqual(puts.length,lostPuts);await page.unroute('**/api/turns/*/stream');page.once('dialog',d=>d.accept());await page.locator('#turn-retry').click();await finish();assert.strictEqual(puts.at(-1).path,'/api/turns/'+lost+'/stream');assert.deepStrictEqual(puts.at(-1).body,lostBody);assert.strictEqual((await ctl('status','preview-response-loss')).count,1);await idle();
  console.log('PASS tenant preview 5: real 202 response loss has no automatic retry; confirmed identical stream PUT receives original JSON 200 plus GET and exactly one model call');

  await ctl('gate','preview-stale');const stale=await begin('preview-stale');await page.waitForFunction(()=>document.querySelector('.provisional p')?.textContent.includes('alice: '));await connect(cfg.bob);await ctl('release','preview-stale');value=await terminal(stale);assert.strictEqual(await page.locator('#turn-id').inputValue(),'');assert.strictEqual(await page.locator('#message').inputValue(),'');assert.strictEqual(await page.locator('#messages .message').count(),0);assert.strictEqual((await ctl('status','preview-stale')).count,1);
  if(value.receipt.state==='needs_review') {await until(async()=>(await ctl('status','preview-stale')).holds[0]?.state==='needs_review','stale identity review hold');await ctl('review','none','alice',stale);}
  await idle();const bob=await begin('preview-bob');await finish();assert((await page.locator('#messages').textContent()).includes('bob: preview-bob'));assert.strictEqual((await ctl('status','preview-bob','bob')).count,1);await idle('bob');
  console.log('PASS tenant preview 6: identity switch during actual preview aborts old delivery, late settlement cannot repopulate new identity, independent user succeeds');
  await authorityChecks('bob');

  // Explicit browser transport fault fixture, not a physical 85-second claim.
  // Shorten only the original long delivery timer; real server/model deadlines
  // and the 15-second control/GET budgets stay unchanged. Delay headers or the
  // authoritative final GET to verify both share that one pre-fetch timer.
  await page.addInitScript(()=>{
   const mode=new URL(location.href).searchParams.get('fixture_deadline');if(!mode)return;
   const timer=window.setTimeout.bind(window),nativeFetch=window.fetch.bind(window);window.deliveryTimers=0;window.finalGets=0;
   window.setTimeout=(fn,ms,...args)=>timer(fn,(ms>=80000&&ms<=86000?(window.deliveryTimers++,1400):ms),...args);
   window.fetch=async(input,options)=>{const r=await nativeFetch(input,options);const path=String(input);
    if(mode==='headers'&&options?.method==='PUT'&&path.endsWith('/stream'))await new Promise(resolve=>timer(resolve,900));
    if(mode==='confirmation'&&(!options?.method||options.method==='GET')&&/^\/api\/turns\/[0-9a-f-]{36}$/.test(path)){window.finalGets++;await new Promise(resolve=>timer(resolve,2500));}
    return r;};
  });
  await page.goto(cfg.base+'/?fixture_deadline=headers');await connect(cfg.bob);await ctl('gate','preview-deadline','bob');const started=Date.now(),deadline=await begin('preview-deadline');await page.waitForFunction(()=>document.querySelector('.provisional p')?.textContent.includes('bob: '));await page.waitForFunction(()=>document.getElementById('status').textContent.includes('超时'));assert(Date.now()-started<3500);assert.strictEqual(await page.evaluate(()=>window.deliveryTimers),1);assert.strictEqual(await page.locator('#turn-id').inputValue(),deadline);assert.strictEqual(await page.locator('#message').inputValue(),'preview-deadline');assert.strictEqual(await page.locator('#turn-finish').isDisabled(),true);assert.strictEqual((await original(deadline,cfg.bob)).active,true);
  await ctl('release','preview-deadline','bob');value=await terminal(deadline,cfg.bob);if(value.receipt.state==='needs_review'){await until(async()=>(await ctl('status','preview-deadline','bob')).holds[0]?.state==='needs_review','deadline hold');await ctl('review','none','bob',deadline);}await page.locator('#turn-check').click();await finish();assert.strictEqual((await ctl('status','preview-deadline','bob')).count,1);await idle('bob');
  console.log('PASS tenant preview 7: injected original delivery timer survives delayed response headers, abort releases browser delivery while actual owner remains, original GET/admin recovery without replay');

  await page.goto(cfg.base+'/?fixture_deadline=confirmation');await connect(cfg.bob);const confirm=await begin('preview-confirmation-deadline');await page.waitForFunction(()=>document.getElementById('status').textContent.includes('超时'));assert.strictEqual(await page.evaluate(()=>window.deliveryTimers),1);assert.strictEqual(await page.evaluate(()=>window.finalGets),1);assert.strictEqual((await original(confirm,cfg.bob)).receipt.state,'completed');assert.strictEqual(await page.locator('#turn-id').inputValue(),confirm);assert.strictEqual(await page.locator('#message').inputValue(),'preview-confirmation-deadline');assert.strictEqual(await page.locator('#turn-finish').isDisabled(),true);const confirmPuts=puts.length;await page.locator('#turn-check').click();await finish();assert.strictEqual(puts.length,confirmPuts);await idle('bob');
  await page.setViewportSize({width:380,height:900});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));await noStorage();assert(requests.every(r=>!r.path.endsWith('/review')&&!r.path.endsWith('/result')&&!r.path.startsWith('/api/tools')&&!r.path.startsWith('/api/skills')&&!r.path.startsWith('/api/sessions/http')));assert.deepStrictEqual(errors,[]);
  console.log('PASS tenant preview 8: same original timer cancels delayed authoritative final GET, complete done alone cannot clear draft, manual GET reconciles; mobile/no admin or history probes');
 } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
