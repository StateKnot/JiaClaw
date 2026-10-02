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
  await page.setViewportSize({width:390,height:844});
  assert(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth));
  page.on('dialog',d=>d.accept());await page.locator('#delete-session').click();
  await page.waitForFunction(()=>document.getElementById('status').textContent==='会话已删除');
  assert.strictEqual(await page.locator('#sessions button').count(),0);
  await page.reload();assert.strictEqual(await page.locator('#new-session').isDisabled(),true);
  assert.deepStrictEqual(errors,[]);
  console.log('PASS: real Chromium connect/create/chat/delete; identity-switch and revoked-key state clearing; text-only model rendering; memory-only token; mobile layout');
 } finally {if(browser)await browser.close();server.kill('SIGTERM');await new Promise(r=>server.once('exit',r));fs.rmSync(root,{recursive:true,force:true});}
})().catch(e=>{console.error(e);process.exitCode=1});
