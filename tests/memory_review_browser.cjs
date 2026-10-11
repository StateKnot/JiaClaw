'use strict';
// The real browser reviews real standalone HTTP memory; no model requests.
const {chromium} = require('playwright');
const fs = require('fs'), os = require('os'), path = require('path'), crypto = require('crypto'), child = require('child_process'), assert = require('assert');
(async () => {
 const root = fs.mkdtempSync(path.join(os.tmpdir(), 'jiaclaw-memory-review-browser-'));
 const ws = path.join(root, 'workspace'), source = path.join(ws, 'notes/private.md');
 fs.mkdirSync(path.dirname(source), {recursive:true});
 const original = 'Private 茶\r\n<img src=x onerror="window.MEMORY_XSS=1">\n';
 fs.writeFileSync(source, original);
 const token = crypto.randomUUID(), config = {agent:{name:'review-browser',description:'Disposable',workspace_path:ws,max_turns:10,system_instructions:''},provider:{provider_type:'stub'},memory:{path:'notes/private.md'},http:{bind:'127.0.0.1:0',api_token:token,memory_review:true,persist:false,shutdown_timeout_secs:1}};
 fs.writeFileSync(path.join(root,'config.json'),JSON.stringify(config));
 const env = Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('JIACLAW_')));env.JIACLAW_LOG_LEVEL='info';
 const server = child.spawn(path.resolve(process.argv[2] || 'target/debug/jiaclaw'), ['serve','--config',path.join(root,'config.json')], {env});
 let log='', browser, release;
 server.stdout.on('data',data=>log+=data);server.stderr.on('data',data=>log+=data);
 try {
  const base = await new Promise((resolve,reject)=>{const end=Date.now()+15000;function poll(){const m=log.match(/HTTP 服务已启动于 (http:\/\/127\.0\.0\.1:\d+)/);if(m)return resolve(m[1]);if(server.exitCode!==null||Date.now()>end)return reject(new Error(log));setTimeout(poll,20);}poll();});
  browser = await chromium.launch({headless:true,...(process.env.JIACLAW_TEST_CHROMIUM?{executablePath:process.env.JIACLAW_TEST_CHROMIUM}:{})});
  const page = await browser.newPage({viewport:{width:1360,height:900}}), errors=[], requests=[];
  page.on('pageerror',error=>errors.push(String(error)));
  page.on('request',request=>{const u=new URL(request.url());if(u.pathname.startsWith('/api/memory')) requests.push({path:u.pathname,method:request.method()});});
  const idle = () => page.waitForFunction(()=>!document.getElementById('refresh').disabled);
  async function connect(key=token){await page.locator('#api-token').fill(key);await page.locator('#connect-form button').click();if(key===token)await idle();else await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));}
  await page.goto(base);assert.strictEqual(await page.locator('#memory-tab').isVisible(),false);assert.strictEqual(requests.length,0);
  await connect();assert.strictEqual(await page.locator('#memory-content').textContent(),'');
  await page.locator('#memory-tab').click();await idle();
  assert.strictEqual(await page.locator('#memory-content').textContent(),original);
  assert((await page.locator('#memory-summary').textContent()).includes(crypto.createHash('sha256').update(original).digest('hex')));
  assert.strictEqual(await page.locator('#memory-view img').count(),0);assert.strictEqual(await page.evaluate(()=>typeof window.MEMORY_XSS),'undefined');
  assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);
  assert(requests.every(request=>request.method==='GET'));
  console.log('PASS memory browser 1: explicit real standalone capability and complete raw UTF8/CRLF snapshot, no XSS or browser storage');

  const exact='\u0000'.repeat(32768);fs.writeFileSync(source,exact);
  await page.locator('#memory-refresh').click();await idle();assert.strictEqual(await page.locator('#memory-content').textContent(),exact);
  fs.writeFileSync(source,'');await page.locator('#memory-refresh').click();await idle();assert.strictEqual(await page.locator('#memory-state').textContent(),'当前记忆为空。');
  fs.writeFileSync(source,original);await page.locator('#memory-refresh').click();await idle();
  fs.writeFileSync(source,'x'.repeat(32769));await page.locator('#memory-refresh').click();await idle();assert.strictEqual(await page.locator('#memory-content').textContent(),'');assert.strictEqual(await page.locator('#memory-summary').textContent(),'');
  fs.writeFileSync(source,original);await page.locator('#memory-refresh').click();await idle();
  await page.setViewportSize({width:390,height:844});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth));
  await page.setViewportSize({width:1360,height:900});
  console.log('PASS memory browser 2: complete escaped32KiB and empty snapshots, failed refresh clears old text, mobile layout');

  let waiting;
  await page.route('**/api/memory',async route=>{waiting=true;await new Promise(resolve=>release=resolve);await route.fulfill({status:200,contentType:'application/json',body:JSON.stringify({path:'notes/private.md',content:original,content_sha256:crypto.createHash('sha256').update(original).digest('hex'),bytes:Buffer.byteLength(original)})});});
  await page.locator('#memory-refresh').click();
  for(let count=0;!waiting&&count<200;count++)await new Promise(resolve=>setTimeout(resolve,10));assert(waiting);
  await connect('wrong');release();release=null;await page.waitForTimeout(100);
  assert.strictEqual(await page.locator('#memory-content').textContent(),'');assert.strictEqual(await page.locator('#memory-summary').textContent(),'');assert.strictEqual(await page.locator('#memory-tab').isVisible(),false);
  await page.unroute('**/api/memory');await connect();
  console.log('PASS memory browser 3: identity switch clears private contents and late old response cannot repopulate the page');

  for(const contract of [{scheduled_jobs:false},{scheduled_jobs:false,read_only:true}]) {
   // This synthetic gateway has no tracked-turn contract. Do not inherit
   // the actual standalone server's differently scoped turn capability.
   await page.route('**/api/turns/capabilities',route=>route.fulfill({status:404,body:''}));
   await page.route('**/api/gateway/capabilities',route=>route.fulfill({status:200,contentType:'application/json',body:JSON.stringify(contract)}));
   const count=requests.length;await connect();assert.strictEqual(await page.locator('#memory-tab').isVisible(),false);assert.strictEqual(requests.length,count);
   await page.unroute('**/api/gateway/capabilities');
   await page.unroute('**/api/turns/capabilities');
  }
  await page.route('**/api/memory/capabilities',route=>route.fulfill({status:403,contentType:'application/json',body:'{"error":"disabled"}'}));await connect();assert.strictEqual(await page.locator('#memory-tab').isVisible(),false);await page.unroute('**/api/memory/capabilities');
  await page.route('**/api/memory/capabilities',route=>route.fulfill({status:200,contentType:'application/json',body:'{"protocol":1,"scope":"gateway","read_only":true,"max_bytes":32768}'}));
  await page.locator('#api-token').fill(token);await page.locator('#connect-form button').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('记忆审阅权限响应异常'));assert.strictEqual(await page.locator('#memory-tab').isVisible(),false);
  await page.unroute('**/api/memory/capabilities');await connect();
  await page.route('**/api/memory',route=>route.fulfill({status:200,contentType:'application/json',body:'{"path":"private","content":"wrong","bytes":0,"content_sha256":"'+'a'.repeat(64)+'"}'}));await page.locator('#memory-tab').click();await idle();assert.strictEqual(await page.locator('#memory-content').textContent(),'');assert((await page.locator('#status').textContent()).includes('记忆快照不完整'));
  assert.deepStrictEqual(errors,[]);assert(requests.every(request=>request.method==='GET'));fs.writeFileSync(path.join(root,'requests.json'),JSON.stringify(requests));
  console.log('PASS memory browser 4: gateway/read-only never probes private memory; disabled/malformed contracts and incomplete snapshot refuse');
 } finally {
  if(release)release();if(browser)await browser.close();
  if(server.exitCode===null&&server.signalCode===null){const ended=new Promise(resolve=>server.once('exit',resolve));server.kill('SIGTERM');let timer;await Promise.race([ended,new Promise((_,reject)=>timer=setTimeout(()=>{server.kill('SIGKILL');reject(new Error('server shutdown deadline'));},8000))]);clearTimeout(timer);}
  fs.writeFileSync(path.join(root,'serve.log'),log);
  if(process.env.JIACLAW_KEEP_TEST_OUTPUT)console.log('Evidence '+root);else fs.rmSync(root,{recursive:true,force:true});
 }
})().catch(error=>{console.error(error);process.exit(1);});
