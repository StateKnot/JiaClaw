'use strict';
// Administrator page qualification, not an Agent browser tool or supplier.
const {chromium} = require('playwright');
const fs = require('fs'), os = require('os'), path = require('path'), crypto = require('crypto'), child = require('child_process'), assert = require('assert');
(async () => {
 const root = fs.mkdtempSync(path.join(os.tmpdir(),'jiaclaw-memory-edit-browser-'));
 const ws = path.join(root,'workspace'), source = path.join(ws,'notes/private.md');
 fs.mkdirSync(path.dirname(source),{recursive:true});
 fs.writeFileSync(source,'Original 私人\r\n<img src=x onerror="window.BAD_MEMORY=1">\n');
 const token = crypto.randomUUID(), binary = path.resolve(process.argv[2] || 'target/debug/jiaclaw');
 const env = Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('JIACLAW_'))); env.JIACLAW_LOG_LEVEL='info';
 const servers=[]; let browser, release;
 async function start(edit) {
  const config=path.join(root,'config-'+servers.length+'.json');
  fs.writeFileSync(config,JSON.stringify({agent:{name:'memory-edit-browser',description:'Disposable',workspace_path:ws,max_turns:10,system_instructions:''},provider:{provider_type:'stub'},memory:{path:'notes/private.md'},http:{bind:'127.0.0.1:0',api_token:token,memory_review:true,memory_edit:edit,persist:false,shutdown_timeout_secs:1}}));
  const server=child.spawn(binary,['serve','--config',config],{env}), record={server,log:''};servers.push(record);
  server.stdout.on('data',d=>record.log+=d);server.stderr.on('data',d=>record.log+=d);
  return await new Promise((resolve,reject)=>{const end=Date.now()+15000;function poll(){const m=record.log.match(/HTTP 服务已启动于 (http:\/\/127\.0\.0\.1:\d+)/);if(m)return resolve(m[1]);if(server.exitCode!==null||Date.now()>end)return reject(new Error(record.log));setTimeout(poll,20);}poll();});
 }
 try {
  const base=await start(true);
  browser=await chromium.launch({headless:true,...(process.env.JIACLAW_TEST_CHROMIUM?{executablePath:process.env.JIACLAW_TEST_CHROMIUM}:{})});
  const page=await browser.newPage({viewport:{width:1360,height:900}}), errors=[], requests=[];
  page.on('pageerror',e=>errors.push(String(e)));
  page.on('request',r=>{if(new URL(r.url()).pathname.startsWith('/api/memory'))requests.push({path:new URL(r.url()).pathname,method:r.method()});});
  const idle=()=>page.waitForFunction(()=>!document.getElementById('refresh').disabled);
  const count=method=>requests.filter(r=>r.method===method).length;
  async function connect(key=token){await page.locator('#api-token').fill(key);await page.locator('#connect-form button').click();if(key===token)await idle();else await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));}
  async function read(){await page.locator('#memory-refresh').click();await idle();}
  async function save(text){await page.locator('#memory-candidate').fill(text);await page.locator('#memory-reviewed').check();await page.locator('#memory-save').click();await idle();}
  await page.goto(base);await connect();assert(await page.locator('#memory-edit-form').isVisible()===false);
  await page.locator('#memory-tab').click();await idle();
  assert(await page.locator('#memory-edit-form').isVisible());assert(await page.locator('#memory-save').isDisabled());
  assert.strictEqual(await page.evaluate(()=>typeof window.BAD_MEMORY),'undefined');
  await page.locator('#memory-candidate').fill('Corrected 私人\n');
  await page.locator('#memory-reviewed').check();await page.locator('#memory-candidate').fill('Corrected 私人\nreviewed');
  assert(!await page.locator('#memory-reviewed').isChecked());assert(await page.locator('#memory-save').isDisabled());assert.strictEqual(count('POST'),0);
  await page.locator('#memory-reviewed').check();const gets=count('GET');await page.locator('#memory-save').click();await idle();
  assert.strictEqual(count('POST'),1);assert.strictEqual(count('GET'),gets);assert.strictEqual(fs.readFileSync(source,'utf8'),'Corrected 私人\nreviewed');
  assert.strictEqual(await page.locator('#memory-content').textContent(),'');assert.strictEqual(await page.locator('#memory-candidate').inputValue(),'');assert(await page.locator('#memory-save').isDisabled());
  await read();assert.strictEqual(await page.locator('#memory-content').textContent(),fs.readFileSync(source,'utf8'));
  assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);
  console.log('PASS memory edit browser 1: actual explicit checked full-text submission and matching disk receipt; no auto GET, XSS, storage or repeat POST');

  fs.writeFileSync(source,'Later external edit');const inode=fs.statSync(source).ino;
  await save('Stale replacement');assert.strictEqual(fs.readFileSync(source,'utf8'),'Later external edit');assert.strictEqual(fs.statSync(source).ino,inode);
  assert(await page.locator('#memory-save').isDisabled());assert((await page.locator('#memory-state').textContent()).includes('结果未确认'));assert.strictEqual(await page.locator('#memory-content').textContent(),'');
  await read();assert.strictEqual(await page.locator('#memory-candidate').inputValue(),'Later external edit');
  await save('');assert.strictEqual(fs.readFileSync(source,'utf8'),'');await read();assert.strictEqual(await page.locator('#memory-state').textContent(),'当前记忆为空。');
  await page.setViewportSize({width:390,height:844});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth));await page.setViewportSize({width:1360,height:900});
  console.log('PASS memory edit browser 2: actual stale condition preserves bytes/inode and disables retry; manual read then explicit empty correction, mobile layout');

  fs.writeFileSync(source,'before unknown receipt');await read();
  await page.route('**/api/memory/replace',async route=>{
   const response=await route.fetch();assert.strictEqual(response.status(),200);
   await route.fulfill({status:200,contentType:'application/json',body:'{"path":"wrong"}'});
  });
  const beforePosts=count('POST'), beforeGets=count('GET');await save('committed but receipt corrupted');
  assert.strictEqual(fs.readFileSync(source,'utf8'),'committed but receipt corrupted');assert.strictEqual(count('POST'),beforePosts+1);assert.strictEqual(count('GET'),beforeGets);
  assert(await page.locator('#memory-save').isDisabled());assert.strictEqual(await page.locator('#memory-candidate').inputValue(),'committed but receipt corrupted');
  await page.unroute('**/api/memory/replace');await read();assert.strictEqual(await page.locator('#memory-content').textContent(),'committed but receipt corrupted');
  let waiting=false;
  await page.route('**/api/memory/replace',async route=>{waiting=true;await new Promise(resolve=>release=resolve);await route.fulfill({status:200,contentType:'application/json',body:'{"path":"wrong"}'});});
  await page.locator('#memory-candidate').fill('private outstanding draft');await page.locator('#memory-reviewed').check();await page.locator('#memory-save').click();
  for(let n=0;!waiting&&n<200;n++)await new Promise(resolve=>setTimeout(resolve,10));assert(waiting);
  await connect('wrong');release();release=null;await page.waitForTimeout(100);
  assert.strictEqual(await page.locator('#memory-candidate').inputValue(),'');assert.strictEqual(await page.locator('#memory-content').textContent(),'');assert(!await page.locator('#memory-edit-form').isVisible());
  await page.unroute('**/api/memory/replace');await connect();
  console.log('PASS memory edit browser 3: committed effect with corrupt response requires manual inspection, no automatic replay; identity switch clears draft and late response');

  for(const contract of [{scheduled_jobs:false},{scheduled_jobs:false,read_only:true}]) {
   await page.route('**/api/turns/capabilities',r=>r.fulfill({status:404,body:''}));
   await page.route('**/api/gateway/capabilities',r=>r.fulfill({status:200,contentType:'application/json',body:JSON.stringify(contract)}));
   const before=requests.length;await connect();assert.strictEqual(requests.length,before);assert(!await page.locator('#memory-edit-form').isVisible());
   await page.unroute('**/api/gateway/capabilities');await page.unroute('**/api/turns/capabilities');
  }
  await page.route('**/api/memory/edit-capabilities',r=>r.fulfill({status:200,contentType:'application/json',body:'{"protocol":1,"scope":"gateway","operation":"replace","conditions":"path-and-two-raw-sha256","max_bytes":32768}'}));
  await page.locator('#api-token').fill(token);await page.locator('#connect-form button').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('记忆纠错权限响应异常'));assert(!await page.locator('#memory-edit-form').isVisible());
  await page.unroute('**/api/memory/edit-capabilities');
  const readonlyBase=await start(false);await page.goto(readonlyBase);await connect();await page.locator('#memory-tab').click();await idle();
  assert(!await page.locator('#memory-edit-form').isVisible());assert.strictEqual(await page.locator('#memory-content').textContent(),fs.readFileSync(source,'utf8'));
  assert.deepStrictEqual(errors,[]);
  console.log('PASS memory edit browser 4: independent default-off capability, malformed contract refuses and gateway/read-only identities never probe');
 } finally {
  if(release)release();if(browser)await browser.close();
  for(let i=0;i<servers.length;i++){
   const {server,log}=servers[i];
   if(server.exitCode===null&&server.signalCode===null){const ended=new Promise(resolve=>server.once('exit',resolve));server.kill('SIGTERM');let timer;await Promise.race([ended,new Promise((_,reject)=>timer=setTimeout(()=>{server.kill('SIGKILL');reject(new Error('server shutdown deadline'));},8000))]);clearTimeout(timer);}
   assert.strictEqual(server.exitCode,0,log);fs.writeFileSync(path.join(root,'serve-'+i+'.log'),log);
  }
  if(process.env.JIACLAW_KEEP_TEST_OUTPUT)console.log('Evidence '+root);else fs.rmSync(root,{recursive:true,force:true});
 }
})().catch(e=>{console.error(e);process.exit(1);});
