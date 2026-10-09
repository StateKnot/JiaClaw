const {chromium}=require('playwright');
const assert=require('assert'), fs=require('fs'), crypto=require('crypto');
const {base,token,control}=JSON.parse(fs.readFileSync(process.argv[2]));
const wait=ms=>new Promise(r=>setTimeout(r,ms));
async function ctl(action, caseName='none') {const r=await fetch(control,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({action,case:caseName})});const j=await r.json();assert(r.ok,JSON.stringify(j));return j.value;}
async function until(fn,label){const deadline=Date.now()+15000;while(Date.now()<deadline){const v=await fn();if(v)return v;await wait(30)}throw new Error('timed out: '+label);}
(async()=>{
 const browser=await chromium.launch({headless:true,...(process.env.JIACLAW_TEST_CHROMIUM?{executablePath:process.env.JIACLAW_TEST_CHROMIUM}:{})});
 console.log('BROWSER VERSION '+browser.version());
 const page=await browser.newPage({viewport:{width:1360,height:1000}}), errors=[], submissions=[];
 page.on('pageerror',e=>errors.push(String(e)));page.on('request',r=>{if(r.method()==='PUT'&&new URL(r.url()).pathname.endsWith('/stream'))submissions.push({url:r.url(),body:r.postDataJSON(),auth:r.headers().authorization});});
 const connect=async()=>{await page.locator('#api-token').fill(token);await page.locator('#connect-form button').click();await page.waitForFunction(()=>document.getElementById('status').textContent==='已连接 · 选择或新建会话');};
 const begin=async(caseName)=>{await page.locator('#new-session').click();await page.locator('#turn-permissions summary').click();await page.locator('#turn-tools input[value="file_write"]').check();await page.locator('#message').fill(caseName);await page.locator('#send').click();await page.waitForFunction(()=>document.querySelectorAll('.provisional').length>0);return page.locator('#turn-id').inputValue();};
 const finish=async()=>{await page.waitForFunction(()=>!document.getElementById('turn-finish').disabled);await page.locator('#turn-finish').click();};
 const releaseAndCheck=async(caseName,id)=>{await ctl('release',caseName);await until(async()=>{const s=await ctl('status',caseName);const r=await fetch(base+'/api/turns/'+id,{headers:{Authorization:'Bearer '+token}});const value=await r.json();return s.rows.some(r=>r.id===id&&r.state!=='running')&&!value.active},'actual terminal');await page.locator('#turn-check').click();await page.waitForFunction(()=>!document.getElementById('turn-abandon').disabled);};
 try {
  await page.goto(base);
  // Synthetic tool DTOs exercise only the catalog/selection wire contract;
  // the 20 skills below are really installed and discovered by the server.
  await page.route('**/api/tools',async route=>{const response=await route.fetch({maxRetries:0,maxRedirects:0});assert.strictEqual(response.status(),200);const body=await response.json();body.tools.push(...Array.from({length:128},(_,i)=>({name:'browser_catalog_'+i,description:'Catalog contract fixture'})));await route.fulfill({response,json:body});});
  await connect();assert.strictEqual(await page.locator('#turn-skills input').count(),20);assert(await page.locator('#turn-tools input').count()>128);assert.strictEqual(await page.locator('#turn-permissions input:checked').count(),0);
  await page.locator('#new-session').click();await page.locator('#turn-permissions summary').click();await page.locator('#turn-tools input[value="file_write"]').check();
  await page.locator('#turn-tools input').evaluateAll(inputs=>{for(const input of inputs)if(input.value.startsWith('browser_catalog_'))input.checked=true;});
  await page.locator('#message').fill('catalog-permission-negative');await page.locator('#send').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('单次最多授权'));
  await page.locator('#turn-tools input').evaluateAll(inputs=>{for(const input of inputs)if(input.value.startsWith('browser_catalog_'))input.checked=false;});
  await page.locator('#turn-skills input').evaluateAll(inputs=>{inputs.forEach((input,index)=>input.checked=index<17);});await page.locator('#send').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('单次最多授权')&&!document.getElementById('send').disabled);
  assert.strictEqual(submissions.length,0);assert.strictEqual(await page.locator('#turn-id').inputValue(),'');assert.strictEqual(await page.evaluate(()=>location.hash),'');assert.strictEqual((await ctl('status','catalog-permission-negative')).count,0);assert.strictEqual((await ctl('status','catalog-permission-negative')).rows.length,0);
  await page.unroute('**/api/tools');await connect();assert.strictEqual(await page.locator('#turn-skills input').count(),20);assert.strictEqual(await page.locator('#turn-permissions input:checked').count(),0);
  console.log('PASS web stream 10: real 20-skill catalog plus oversized tool DTO catalog, no implicit permissions, per-turn limits reject before UUID/admission/model/effects');
  await page.locator('#new-session').click();await page.locator('#message').fill('permission-negative');await page.locator('#send').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('请选择本次允许的工具'));
  assert.strictEqual(submissions.length,0);assert.strictEqual((await ctl('status','permission-negative')).count,0);
  await ctl('gate','stream-tools');
  await page.locator('#turn-tools input[value="file_write"]').check();await page.locator('#message').fill('stream-tools');await page.locator('#send').click();
  await page.waitForFunction(()=>document.querySelector('.provisional p')?.textContent.includes('临时预览'));
  const id=await page.locator('#turn-id').inputValue();let s=await ctl('status','stream-tools');
  assert.strictEqual(s.count,1);assert(s.rows.some(r=>r.id===id&&r.state==='running'&&r.session_committed===0));assert.strictEqual(s.effects.length,0);assert(s.ledger.some(r=>r.turn_id===id&&r.state==='submitting'));
  assert.strictEqual(await page.locator('#turn-finish').isDisabled(),true);
  await ctl('release','stream-tools');await finish();
  s=await ctl('status','stream-tools');assert.strictEqual(s.count,2);assert.deepStrictEqual(s.effects,['stream-tools.txt']);assert.strictEqual(await page.locator('.provisional').count(),0);assert((await page.locator('#messages').textContent()).includes('最终🦀回复'));assert.strictEqual(await page.locator('#message').inputValue(),'');
  assert.deepStrictEqual(submissions[0].body.enabled_tools,['file_write']);assert.deepStrictEqual(submissions[0].body.enabled_skills,[]);assert.strictEqual(submissions[0].auth,'Bearer '+token);
  console.log('PASS web stream 1: authenticated gated preview before commit, explicit permissions, two native rounds and stored Unicode result');

  await ctl('gate','stream-cancel');const cancelId=await begin('stream-cancel');await page.locator('#turn-cancel').click();
  await until(async()=>{s=await ctl('status','stream-cancel');return s.rows.some(r=>r.id===cancelId&&r.cancel_requested===1)},'durable cancel');
  assert.strictEqual(await page.locator('#message').inputValue(),'stream-cancel');assert.strictEqual(await page.locator('#new-session').isDisabled(),true);
  await releaseAndCheck('stream-cancel',cancelId);assert.strictEqual((await ctl('status','stream-cancel')).effects.includes('stream-cancel.txt'),false);
  await page.locator('#turn-review summary').click();await page.locator('#turn-review-note').fill('Actual local model receipt checked; no native file effect.');
  page.once('dialog',d=>d.accept());await page.locator('#turn-abandon').click();await finish();
  console.log('PASS web stream 2: explicit persisted cancellation, current model settles without later tools, manual hold review');

  await ctl('gate','stream-drop');const dropId=await begin('stream-drop');const before=submissions.length;
  await page.reload();assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);
  await page.locator('#api-token').fill(token);await page.locator('#connect-form button').click();
  await page.waitForFunction(id=>document.getElementById('turn-id').value===id&&document.getElementById('turn-state').textContent.includes('处理中'),dropId);
  assert.strictEqual(submissions.length,before);assert.strictEqual(await page.locator('#send').isDisabled(),true);
  // Verify the original HTTP body owner actually ended before releasing the model.
  await until(async()=>{s=await ctl('status','stream-drop');return s.delivery_stopped},'actual body delivery stop');
  await releaseAndCheck('stream-drop',dropId);assert.strictEqual((await ctl('status','stream-drop')).effects.includes('stream-drop.txt'),false);
  await page.locator('#turn-review summary').click();await page.locator('#turn-review-note').fill('Checked original model receipt and workspace after browser reload.');page.once('dialog',d=>d.accept());await page.locator('#turn-abandon').click();await finish();
  console.log('PASS web stream 3: reload retains only UUID, authenticated original GET, no reconnect/replay or lost permission snapshot');

  await ctl('fail-commit');const rollbackId=await begin('browser-rollback');
  await page.waitForFunction(()=>document.getElementById('status').textContent.includes('勿重复执行')&&!document.getElementById('turn-check').disabled);
  assert.strictEqual(await page.locator('#message').inputValue(),'browser-rollback');assert.strictEqual(await page.locator('#turn-finish').isDisabled(),true);
  await ctl('restore-commit');await page.locator('#turn-check').click();await page.waitForFunction(()=>!document.getElementById('turn-abandon').disabled);
  s=await ctl('status','browser-rollback');assert(s.rows.some(r=>r.id===rollbackId&&r.state==='running'&&r.session_committed===0));
  await page.locator('#turn-review summary').click();await page.locator('#turn-review-note').fill('SQL commit failed; original completed model receipt checked.');page.once('dialog',d=>d.accept());await page.locator('#turn-abandon').click();await finish();
  console.log('PASS web stream 4: real SQLite terminal rollback has no browser success, original orphan lookup and explicit review');

  await page.route('**/api/turns/*/stream',async route=>{const response=await route.fetch({maxRetries:0,maxRedirects:0});assert.strictEqual(response.status(),202);await route.abort('failed');});
  await page.locator('#new-session').click();await page.locator('#turn-permissions summary').click();await page.locator('#turn-tools input[value="file_write"]').check();await page.locator('#message').fill('lost-reply');await page.locator('#send').click();
  await page.waitForFunction(()=>!document.getElementById('turn-retry').disabled);
  const retryId=await page.locator('#turn-id').inputValue(), original=submissions[submissions.length-1];s=await ctl('status','lost-reply');assert.strictEqual(s.count,2);assert(s.rows.some(r=>r.id===retryId&&r.state==='completed'));assert(s.effects.includes('lost-reply.txt'));assert.strictEqual(await page.locator('#message').inputValue(),'lost-reply');
  await page.unroute('**/api/turns/*/stream');page.once('dialog',d=>d.accept());await page.locator('#turn-retry').click();await finish();
  assert.strictEqual(submissions[submissions.length-1].url,original.url);assert.deepStrictEqual(submissions[submissions.length-1].body,original.body);assert.strictEqual((await ctl('status','lost-reply')).count,2);
  console.log('PASS web stream 6: actual committed response lost, explicit same UUID/body/permissions returns JSON without a second execution');

  await ctl('gate','stream-slow');const largeId=await begin('stream-slow');
  await page.waitForFunction(()=>document.querySelector('.provisional p')?.textContent.includes('预览显示已截断'));
  assert((await page.locator('.provisional p').textContent()).length<66000);assert.strictEqual(await page.locator('.provisional').count(),1);assert.strictEqual((await ctl('status','stream-slow')).rows.find(r=>r.id===largeId).state,'running');
  await ctl('release','stream-slow');await finish();assert((await page.locator('#messages').textContent()).includes('显示已截断'));assert((await page.locator('#messages').textContent()).length<100000);
  console.log('PASS web stream 7: actual 1900 KiB preview/result consumed with bounded DOM, complete receipt and explicit history truncation');

  await page.clock.install();await ctl('gate','stream-stop');const deadlineId=await begin('stream-stop');
  await page.clock.fastForward(85001);await page.waitForFunction(()=>document.getElementById('status').textContent.includes('交付已停止或超时'));
  await until(async()=>{s=await ctl('status','stream-stop');return s.delivery_stopped},'browser deadline body owner stopped');assert.strictEqual(s.rows.find(r=>r.id===deadlineId).cancel_requested,0);assert(s.ledger.some(r=>r.turn_id===deadlineId&&r.state==='submitting'));
  await releaseAndCheck('stream-stop',deadlineId);assert.strictEqual((await ctl('status','stream-stop')).effects.includes('stream-stop.txt'),false);
  await page.locator('#turn-review summary').click();await page.locator('#turn-review-note').fill('Checked original model after browser deadline and actual body close.');page.once('dialog',d=>d.accept());await page.locator('#turn-abandon').click();await finish();await page.clock.resume();
  console.log('PASS web stream 8: browser original total deadline aborts delivery without fake cancel or early model owner release');

  // Actual commit wins cancellation; only its final delivery is fault-injected.
  await page.route('**/api/turns/*/stream',async route=>{const response=await route.fetch({maxRetries:0,maxRedirects:0});assert.strictEqual(response.status(),202);const body=await response.text();assert(body.includes('event: done\n'));await route.fulfill({status:202,headers:{'Content-Type':'text/event-stream; charset=utf-8'},body:body.slice(0,body.indexOf('event: done\n'))});});
  const raceId=await begin('cancel-after-commit');await page.waitForFunction(()=>document.getElementById('status').textContent.includes('勿重复执行')&&!document.getElementById('turn-cancel').disabled);
  s=await ctl('status','cancel-after-commit');assert(s.rows.some(r=>r.id===raceId&&r.state==='completed'&&r.cancel_requested===0));
  await page.unroute('**/api/turns/*/stream');await page.locator('#turn-cancel').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('本次未新增停止意图'));
  assert.strictEqual((await ctl('status','cancel-after-commit')).rows.find(r=>r.id===raceId).cancel_requested,0);assert.strictEqual(await page.locator('#message').inputValue(),'');await finish();assert.strictEqual((await ctl('status','cancel-after-commit')).count,1);
  console.log('PASS web stream 9: actual terminal commit wins cancel, no invented intent, stored history reconciles before draft release');

  // Malformed transport is a frontend contract fault fixture, not a supplier test.
  await page.route('**/api/turns/*/stream',route=>{const body=route.request().postDataJSON(),rid=new URL(route.request().url()).pathname.split('/')[3];const receipt={id:rid,session_id:body.session_id,request_hash:'a'.repeat(64),context_hash:'b'.repeat(64),created_ms:1,finished_ms:null,state:'running',session_committed:false,error:null,result:null,result_purged:false,cancel_requested:false,reviewed_ms:null,review_note:null};const values=[{event:'admitted',protocol:1,receipt},{event:'model_started',turn_id:rid,operation_id:rid,remote_id:rid,round:0,model:'fixture'},{event:'preview',round:0,text:'<img src=x onerror="window.STREAM_XSS=1">临时🦀'}];return route.fulfill({status:202,headers:{'Content-Type':'text/event-stream; charset=utf-8'},body:values.map(v=>`event: ${v.event}\ndata: ${JSON.stringify(v)}\n\n`).join('')+'event: done\ndata: {"event":"done"'});});
  await begin('browser-corrupt');await page.waitForFunction(()=>document.getElementById('status').textContent.includes('勿重复执行'));
  assert.strictEqual(await page.locator('#messages img').count(),0);assert.strictEqual(await page.evaluate(()=>typeof window.STREAM_XSS),'undefined');assert.strictEqual(await page.locator('#message').inputValue(),'browser-corrupt');assert.strictEqual(await page.locator('#send').isDisabled(),true);
  await page.setViewportSize({width:380,height:900});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  await page.unroute('**/api/turns/*/stream');await connect();
  await ctl('gate','browser-identity');const identityId=await begin('browser-identity');
  await page.locator('#api-token').fill('invalid-other-identity');await page.locator('#connect-form button').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));await ctl('release','browser-identity');
  await until(async()=>{const r=await fetch(base+'/api/turns/'+identityId,{headers:{Authorization:'Bearer '+token}});const v=await r.json();return v.receipt.state!=='running'&&!v.active},'old identity actual settlement');
  assert.strictEqual(await page.locator('#messages .message').count(),0);assert.strictEqual(await page.locator('#turn-id').inputValue(),'');assert.strictEqual(await page.locator('#message').inputValue(),'');assert.strictEqual(await page.evaluate(()=>location.hash),'');
  assert.deepStrictEqual(errors,[]);assert.strictEqual((await ctl('status','browser-identity')).faults.length,0);
  console.log('PASS web stream 5: partial terminal never saves/replays, text-only XSS/mobile bounds and stale identity clears delivery');
 } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1});
