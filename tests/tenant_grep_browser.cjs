// Actual personal-Key UI against the original native/gateway/model processes.
const {chromium}=require('playwright'),assert=require('assert'),fs=require('fs');
const cfg=JSON.parse(fs.readFileSync(process.argv[2]));
(async()=>{
 const browser=await chromium.launch({headless:true,...(process.env.JIACLAW_TEST_CHROMIUM?{executablePath:process.env.JIACLAW_TEST_CHROMIUM}:{})});
 console.log('BROWSER VERSION '+browser.version());
 const page=await browser.newPage(),puts=[],requests=[],errors=[];
 page.on('pageerror',e=>errors.push(String(e)));
 page.on('request',r=>{requests.push(new URL(r.url()).pathname);if(r.method()==='PUT')puts.push(r.postDataJSON());});
 const connect=async token=>{await page.locator('#api-token').fill(token);await page.locator('#connect-form button').click();await page.waitForFunction(()=>!document.getElementById('status').textContent.startsWith('连接中'));};
 try{
  await page.goto(cfg.base);await connect(cfg.alice);
  await page.waitForSelector('#turn-tools input[value="grep"]',{state:'attached'});
  assert.strictEqual(await page.locator('#turn-tools input:checked').count(),0);
  assert((await page.locator('#turn-tools pre').textContent()).includes('tenant-documents/alice'));
  await page.locator('#new-session').click();await page.locator('#turn-permissions summary').click();
  await page.setViewportSize({width:375,height:800});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  await page.setViewportSize({width:1280,height:900});await page.locator('#turn-tools input[value="grep"]').check();
  await page.locator('#message').fill('browser-grep');await page.locator('#send').click();
  await page.waitForFunction(()=>!document.getElementById('turn-finish').disabled);
  assert.strictEqual(puts.length,1);assert.deepStrictEqual(puts[0].enabled_tools,['grep']);assert(/^[0-9a-f]{64}$/.test(puts[0].grep_policy_sha256));
  const identity=await page.locator('#turn-id').inputValue();const content=await page.locator('#messages').textContent();
  assert(content.includes('Needle hidden')&&content.includes('literal.*')&&!content.includes('OUTSIDE-PRIVATE')&&!content.includes('report.bin'));
  await page.locator('#turn-finish').click();
  console.log('PASS tenant grep browser 1: actual explicit grep/SSE/original GET, canonical captured permission, complete root and mobile layout');
  await page.reload();await connect(cfg.readonly);await page.locator('#turn-lookup-id').fill(identity);await page.locator('#turn-lookup button').click();
  await page.waitForFunction(()=>!document.getElementById('turn-finish').disabled);assert(await page.locator('#send').isDisabled());
  assert.strictEqual(puts.length,1);await page.locator('#turn-finish').click();
  await connect(cfg.bob);await page.waitForSelector('#turn-tools input[value="grep"]',{state:'attached'});
  const bobScope=await page.locator('#turn-tools pre').textContent();assert(bobScope.includes('tenant-documents/bob')&&!bobScope.includes('tenant-documents/alice'));
  assert.strictEqual(await page.locator('#turn-tools input:checked').count(),0);assert(!(await page.locator('#messages').textContent()).includes('Needle hidden'));
  console.log('PASS tenant grep browser 2: readonly old fact only and identity switch clears private grep scope/result/selection');
  await page.addInitScript(()=>{
    const original=crypto.subtle.digest.bind(crypto.subtle);let once=true;
    crypto.subtle.digest=async(...args)=>{if(once){once=false;window.__grepHeld=true;await new Promise(resolve=>window.__grepRelease=resolve);}return original(...args);};
  });
  await page.reload();await page.locator('#api-token').fill(cfg.alice);await page.locator('#connect-form button').click();await page.waitForFunction(()=>window.__grepHeld===true);
  await connect(cfg.bob);await page.waitForSelector('#turn-tools input[value="grep"]',{state:'attached'});await page.evaluate(()=>window.__grepRelease());
  await page.waitForFunction(()=>document.getElementById('status').textContent.includes('已连接'));
  assert.strictEqual(await page.locator('#turn-tools pre').textContent(),bobScope);assert.strictEqual(puts.length,1);
  await page.route('**/api/turns/capabilities',async route=>{const r=await route.fetch();const c=await r.json();c.grep.policy_sha256='a'.repeat(64);await route.fulfill({response:r,json:c});});
  await connect(cfg.alice);await page.waitForFunction(()=>document.getElementById('status').textContent.includes('正文检索授权摘要不匹配'));
  assert.strictEqual(await page.locator('#turn-tools input').count(),0);assert.strictEqual(puts.length,1);
  assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);
  assert(!requests.some(p=>['/api/tools','/api/memory','/api/memory/semantic'].includes(p)));assert.deepStrictEqual(errors,[]);
  console.log('PASS tenant grep browser 3: real late Crypto cannot restore Alice scope; changed descriptor fails closed, no storage or implicit PUT');
  console.log('ACTUAL tenant grep browser receipt '+JSON.stringify({turn_id:identity,policy_sha256:puts[0].grep_policy_sha256,grep_puts:puts.length,selected_tools:puts[0].enabled_tools,storage_entries:await page.evaluate(()=>localStorage.length+sessionStorage.length),page_errors:errors.length}));
  console.log('ALL tenant grep browser acceptance passed');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
