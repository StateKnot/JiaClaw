// Real management-page file task against the actual gateway/backend/model fixture.
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
  await page.waitForSelector('#turn-tools input[value="file_read"]',{state:'attached'});
  assert.strictEqual(await page.locator('#turn-tools input:checked').count(),0);
  assert((await page.locator('#turn-tools pre').textContent()).includes('tenant-documents/资料.txt'));
  await page.locator('#new-session').click();await page.setViewportSize({width:375,height:800});await page.locator('#turn-permissions summary').click();
  assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  await page.locator('#turn-permissions summary').click();await page.setViewportSize({width:1280,height:900});
  await page.locator('#turn-permissions summary').click();
  await page.locator('#turn-tools input[value="file_read"]').check();await page.locator('#message').fill('browser-file');await page.locator('#send').click();
  await page.waitForFunction(()=>!document.getElementById('turn-finish').disabled);
  assert.strictEqual(puts.length,1);assert(puts[0].path.endsWith('/stream'));assert.deepStrictEqual(puts[0].body.enabled_tools,['file_read']);
  assert(/^[0-9a-f]{64}$/.test(puts[0].body.file_read_policy_sha256));
  const identity=await page.locator('#turn-id').inputValue();
  assert((await page.locator('#messages').textContent()).includes('alice-PRIVATE-source'));
  assert.strictEqual(await page.locator('#message').inputValue(),'');await page.locator('#turn-finish').click();
  assert(!requests.some(r=>['/api/tools','/api/skills','/api/memory','/api/memory/semantic'].includes(r.path)));
  console.log('PASS tenant file browser 1: complete private grant, unchecked selection, actual file tool/SSE/GET and captured permission SHA');
  await page.reload();await connect(cfg.readonly);
  await page.locator('#turn-lookup-id').fill(identity);await page.locator('#turn-lookup button').click();
  await page.waitForFunction(()=>!document.getElementById('turn-finish').disabled);
  assert.strictEqual(await page.locator('#send').isDisabled(),true);assert.strictEqual(puts.length,1);
  await page.locator('#turn-finish').click();
  console.log('PASS tenant file browser 2: read-only original file fact GET without file dispatch or private catalog probing');
  await connect(cfg.bob);await page.waitForSelector('#turn-tools input[value="file_read"]',{state:'attached'});
  const text=await page.locator('#turn-tools pre').textContent();assert(text.includes('tenant-documents/notes.txt')&&!text.includes('资料.txt'));
  assert.strictEqual(await page.locator('#turn-tools input:checked').count(),0);
  assert(!(await page.locator('#messages').textContent()).includes('alice-PRIVATE-source'));
  assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);
  console.log('PASS tenant file browser 3: identity switch clears old source/result/selection, isolated full grant and no storage');
  await page.route('**/api/turns/capabilities',async route=>{const r=await route.fetch();const c=await r.json();c.file_read.policy_sha256='a'.repeat(64);await route.fulfill({response:r,json:c});});
  await connect(cfg.alice);await page.waitForFunction(()=>document.getElementById('status').textContent.includes('文件授权摘要不匹配'));
  assert.strictEqual(await page.locator('#turn-tools input').count(),0);assert.strictEqual(puts.length,1);
  assert.strictEqual(errors.length,0,errors.join('\n'));
  console.log('PASS tenant file browser 4: malformed complete grant digest fails closed without implicit dispatch or stale DOM');
 }finally{await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});
