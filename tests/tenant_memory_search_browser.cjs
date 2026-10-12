// Real management-page keyword task against the actual gateway/backend/model fixture.
const {chromium}=require('playwright'),assert=require('assert'),fs=require('fs');
const cfg=JSON.parse(fs.readFileSync(process.argv[2]));
(async()=>{
 const browser=await chromium.launch({headless:true,...(process.env.JIACLAW_TEST_CHROMIUM?{executablePath:process.env.JIACLAW_TEST_CHROMIUM}:{})});
 console.log('BROWSER VERSION '+browser.version());
 const page=await browser.newPage(),requests=[],puts=[],errors=[];
 page.on('pageerror',e=>errors.push(String(e)));
 page.on('request',r=>{requests.push({method:r.method(),path:new URL(r.url()).pathname});if(r.method()==='PUT')puts.push({path:new URL(r.url()).pathname,body:r.postDataJSON()});});
 const connect=async token=>{await page.locator('#api-token').fill(token);await page.locator('#connect-form button').click();await page.waitForFunction(()=>!document.getElementById('status').textContent.startsWith('连接中'));};
 try{
  await page.goto(cfg.base);await connect(cfg.alice);
  await page.waitForSelector('#turn-tools input[value="memory_search"]',{state:'attached'});
  assert.strictEqual(await page.locator('#turn-tools input:checked').count(),0);
  assert((await page.locator('#turn-tools pre').textContent()).includes('MEMORY.md'));
  await page.locator('#new-session').click();await page.setViewportSize({width:375,height:800});await page.locator('#turn-permissions summary').click();
  assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  await page.locator('#turn-permissions summary').click();await page.setViewportSize({width:1280,height:900});
  await page.locator('#turn-permissions summary').click();
  await page.locator('#turn-tools input[value="memory_search"]').check();await page.locator('#message').fill('browser-memory');await page.locator('#send').click();
  await page.waitForFunction(()=>!document.getElementById('turn-finish').disabled);
  assert.strictEqual(puts.length,1);assert(puts[0].path.endsWith('/stream'));assert.deepStrictEqual(puts[0].body.enabled_tools,['memory_search']);
  assert(/^[0-9a-f]{64}$/.test(puts[0].body.memory_search_policy_sha256));
  const identity=await page.locator('#turn-id').inputValue();
  assert((await page.locator('#messages').textContent()).includes('alice-needle-remembered'));
  assert.strictEqual(await page.locator('#message').inputValue(),'');await page.locator('#turn-finish').click();
  assert(!requests.some(r=>['/api/tools','/api/memorys','/api/memory','/api/memory/semantic'].includes(r.path)));
  console.log('PASS tenant memory browser 1: complete private grant, unchecked selection, actual memory tool/SSE/GET and captured permission SHA');
  await page.reload();await connect(cfg.readonly);
  await page.locator('#turn-lookup-id').fill(identity);await page.locator('#turn-lookup button').click();
  await page.waitForFunction(()=>!document.getElementById('turn-finish').disabled);
  assert.strictEqual(await page.locator('#send').isDisabled(),true);assert.strictEqual(puts.length,1);
  await page.locator('#turn-finish').click();
  console.log('PASS tenant memory browser 2: read-only original memory fact GET without memory dispatch or private catalog probing');
  await connect(cfg.bob);await page.waitForSelector('#turn-tools input[value="memory_search"]',{state:'attached'});
  const text=await page.locator('#turn-tools pre').textContent();assert(text.includes('MEMORY.md')&&!text.includes('alice-REVIEWED'));
  assert.strictEqual(await page.locator('#turn-tools input:checked').count(),0);
  assert(!(await page.locator('#messages').textContent()).includes('alice-needle-remembered'));
  assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);
  console.log('PASS tenant memory browser 3: identity switch clears old memory/result/selection, isolated full grant and no storage');
  await page.addInitScript(()=>{
    const original=crypto.subtle.digest.bind(crypto.subtle);let once=true;
    crypto.subtle.digest=async (...args)=>{
      if(once){once=false;window.__memoryDigestHeld=true;await new Promise(resolve=>window.__releaseMemoryDigest=resolve);}
      return original(...args);
    };
  });
  await page.reload();await page.locator('#api-token').fill(cfg.alice);await page.locator('#connect-form button').click();
  await page.waitForFunction(()=>window.__memoryDigestHeld===true);
  await connect(cfg.bob);await page.waitForSelector('#turn-tools input[value="memory_search"]',{state:'attached'});
  const bobScope=await page.locator('#turn-tools pre').textContent();assert(!bobScope.includes('notes/USER.md'));
  await page.evaluate(()=>window.__releaseMemoryDigest());
  await page.waitForFunction(()=>document.getElementById('status').textContent.includes('已连接'));
  assert.strictEqual(await page.locator('#turn-tools pre').textContent(),bobScope);
  assert.strictEqual(await page.locator('#turn-tools input:checked').count(),0);assert.strictEqual(puts.length,1);
  await page.route('**/api/turns/capabilities',async route=>{const r=await route.fetch();const c=await r.json();c.memory_search.policy_sha256='a'.repeat(64);await route.fulfill({response:r,json:c});});
  await connect(cfg.alice);await page.waitForFunction(()=>document.getElementById('status').textContent.includes('记忆检索授权摘要不匹配'));
  assert.strictEqual(await page.locator('#turn-tools input').count(),0);assert.strictEqual(puts.length,1);
  assert.strictEqual(errors.length,0,errors.join('\n'));
  console.log('PASS tenant memory browser 4: actual late WebCrypto cannot restore old identity; malformed complete grant digest fails closed without implicit dispatch or stale DOM');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
