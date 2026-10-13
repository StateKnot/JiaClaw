const {chromium}=require('playwright'),assert=require('assert'),fs=require('fs'),crypto=require('crypto');
const cfg=JSON.parse(fs.readFileSync(process.argv[2])),mode=cfg.mode;
const wait=ms=>new Promise(r=>setTimeout(r,ms));
async function ctl(action,extra={}){const r=await fetch(cfg.control,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({action,mode,...extra})});assert(r.ok);return r.json();}
(async()=>{
 const browser=await chromium.launch({headless:true,...(process.env.JIACLAW_TEST_CHROMIUM?{executablePath:process.env.JIACLAW_TEST_CHROMIUM}:{})});
 console.log('TASK FEEDBACK BROWSER START '+mode+' '+browser.version());
 const page=await browser.newPage({viewport:{width:390,height:844}}),errors=[],writes=[];
 page.on('pageerror',e=>errors.push(String(e)));
 page.on('request',r=>{if(r.method()==='PUT'&&new URL(r.url()).pathname.endsWith('/feedback'))writes.push(r.postDataJSON());});
 if(mode==='tenant_json')await page.route('**/api/turns/capabilities',async route=>{const r=await route.fetch({maxRetries:0,maxRedirects:0}),v=await r.json();v.gateway_protocol=1;v.streaming=false;for(const k of ['stream_suffix','turn_budget_secs','max_stream_wire_bytes','max_preview_round_bytes','max_preview_total_bytes'])delete v[k];await route.fulfill({response:r,json:v});});
 const connect=async(token=cfg.token)=>{await page.locator('#api-token').fill(token);await page.locator('#connect-form button').click();await page.waitForFunction(()=>!document.getElementById('turn-workspace').hidden&&!document.getElementById('status').textContent.startsWith('连接中'),null,{timeout:10000});};
 await page.goto(cfg.base);await connect();await page.locator('#new-session').click();await page.locator('#turn-permissions summary').click();
 await page.locator('#turn-tools input[value="'+(mode==='standalone'?'file_write':'datetime_now')+'"]').check();
 const prompt='feedback-browser-'+mode;await page.locator('#message').fill(prompt);await page.locator('#send').click();
 await page.waitForFunction(()=>!document.getElementById('turn-finish').disabled,null,{timeout:30000});const id=await page.locator('#turn-id').inputValue();
 const lookup=async()=>{await page.locator('#turn-check').click();await page.waitForFunction(()=>!document.getElementById('turn-check').disabled);};
 const ready=async()=>page.waitForFunction(()=>!document.getElementById('turn-feedback-reviewed').disabled);
 await ready();assert(await page.locator('#turn-feedback-save').isDisabled());
 const note='\ufeff<img src=x onerror=alert(1)>\r\n\t私人🦀说明';
 await page.locator('#turn-feedback-note').fill(note);await page.locator('#turn-feedback-reviewed').check();await page.locator('#turn-feedback-save').click();
 await page.waitForFunction(()=>document.getElementById('turn-feedback-state').textContent.includes('已保存'),null,{timeout:15000});
 assert.strictEqual(await page.locator('#turn-feedback-saved-note').textContent(),note.replace(/\r\n/g,'\n'));assert.strictEqual(await page.locator('#turn-feedback-saved-note img').count(),0);assert.strictEqual(writes.length,1);
 let aborted=false;
 await page.route('**/api/turns/'+id+'/feedback',async route=>{const r=await route.fetch({maxRetries:0,maxRedirects:0});assert.strictEqual(r.status(),200);aborted=true;await route.abort('failed');});
 const exact='🦀'.repeat(256);await page.locator('#turn-feedback-decision').selectOption('needs_changes');await page.locator('#turn-feedback-note').fill(exact);await page.locator('#turn-feedback-reviewed').check();await page.locator('#turn-feedback-save').click();
 await page.waitForFunction(()=>document.getElementById('status').textContent.includes('核对'),null,{timeout:15000});
 assert(aborted);assert(await page.locator('#turn-feedback-save').isDisabled());assert.strictEqual(await page.locator('#turn-feedback-saved-note').textContent(),'');await wait(100);assert.strictEqual(writes.length,2);
 await page.unroute('**/api/turns/'+id+'/feedback');await lookup();await ready();assert.strictEqual(await page.locator('#turn-feedback-saved-note').textContent(),exact);
 const before=writes.length;await page.locator('#turn-feedback-note').fill('local proposal');await page.locator('#turn-feedback-reviewed').check();
 const response=await fetch(cfg.base+'/api/turns/'+id+'/feedback',{method:'PUT',headers:{Authorization:'Bearer '+cfg.token,'Content-Type':'application/json'},body:JSON.stringify({expected_revision:2,decision:'usable',note:'other client'})});assert.strictEqual(response.status,200);
 await page.locator('#turn-feedback-save').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('已变化'),null,{timeout:15000});assert.strictEqual(writes.length,before);
 await lookup();await ready();assert.strictEqual(await page.locator('#turn-feedback-saved-note').textContent(),'other client');
 const changed=await ctl('restart');cfg.base=changed.base;
 const priorEpoch=await page.evaluate(()=>performance.timeOrigin);await page.goto(cfg.base+'/#turn='+id);console.log('ACTUAL task feedback restart navigation '+JSON.stringify({mode,same_document_epoch:priorEpoch===await page.evaluate(()=>performance.timeOrigin),explicit_reload_follows:true}));await page.reload();await connect();await page.waitForFunction(()=>!document.getElementById('turn-finish').disabled,null,{timeout:15000});await ready();
 assert.strictEqual(await page.locator('#turn-feedback-saved-note').textContent(),'other client');assert.strictEqual(await page.locator('#turn-feedback-note').inputValue(),'');
 if(mode!=='standalone'){
  await page.reload();await connect(cfg.readonly);await page.waitForFunction(()=>!document.getElementById('turn-check').disabled,null,{timeout:10000});
  if(await page.locator('#turn-feedback-saved-note').textContent()!=='other client')await lookup();
  assert.strictEqual(await page.locator('#turn-feedback-saved-note').textContent(),'other client');assert(await page.locator('#turn-feedback-save').isDisabled());assert(await page.locator('#turn-feedback-note').isDisabled());
  await page.reload();await connect();await page.waitForFunction(()=>!document.getElementById('turn-finish').disabled,null,{timeout:15000});await ready();
 }
 await page.locator('#turn-feedback-reviewed').check();await page.locator('#turn-feedback-clear').click();
 await page.waitForFunction(()=>document.getElementById('turn-feedback-state').textContent.includes('已清除'),null,{timeout:15000});assert.strictEqual(await page.locator('#turn-feedback-saved-note').textContent(),'');
 assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);
 const count=await ctl('status',{prompt});assert.strictEqual(mode==='standalone'?count.posts:count.count,1);assert.deepStrictEqual(errors,[]);
 const savedWrites=writes.length;assert.strictEqual(savedWrites,3);
 await page.locator('#turn-finish').click();assert.strictEqual(await page.locator('#turn-feedback-note').inputValue(),'');assert.strictEqual(await page.locator('#turn-feedback-saved-note').textContent(),'');
 console.log('ACTUAL task feedback browser receipts '+JSON.stringify({protocol:1,mode,id,model_posts:1,browser_feedback_puts:3,lost_response_puts:1,conflict_extra_puts:0,exact_note_bytes:1024,restart_exact:true,readonly_rejected:mode!=='standalone',private_dom_cleared:true,notes_sha256:crypto.createHash('sha256').update(exact).digest('hex')}));
 console.log('PASS task feedback browser '+mode+': actual task, explicit user note/text-only rendering, original CAS/lost response GET-only recovery, actual restart and private DOM lifecycle');
 await browser.close();
})().catch(e=>{console.error(e);process.exit(1);});

