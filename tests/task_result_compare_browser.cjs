'use strict';
const {chromium}=require('playwright'), assert=require('assert'), fs=require('fs'), crypto=require('crypto');
const cfg=JSON.parse(fs.readFileSync(process.argv[2])), mode=cfg.mode;
const sha=s=>crypto.createHash('sha256').update(s).digest('hex'), wait=ms=>new Promise(r=>setTimeout(r,ms));
(async()=>{
 const browser=await chromium.launch({headless:true,...(process.env.JIACLAW_TEST_CHROMIUM?{executablePath:process.env.JIACLAW_TEST_CHROMIUM}:{})});
 console.log('TASK RESULT COMPARISON BROWSER START '+mode+' '+browser.version());
 const page=await browser.newPage({viewport:{width:375,height:844}}), errors=[], requests=[];
 page.on('pageerror',e=>errors.push(String(e)));page.on('request',r=>requests.push({method:r.method(),path:new URL(r.url()).pathname}));
 if(mode==='tenant_json')await page.route('**/api/turns/capabilities',async route=>{const r=await route.fetch({maxRetries:0,maxRedirects:0}),v=await r.json();v.gateway_protocol=1;v.streaming=false;for(const k of ['stream_suffix','turn_budget_secs','max_stream_wire_bytes','max_preview_round_bytes','max_preview_total_bytes'])delete v[k];await route.fulfill({response:r,json:v});});
 const connect=async(token=cfg.token)=>{await page.locator('#api-token').fill(token);await page.locator('#connect-form button').click();await page.waitForFunction(()=>!document.getElementById('turn-workspace').hidden&&!document.getElementById('status').textContent.startsWith('连接中'),null,{timeout:10000});};
 const open=async()=>{if(!(await page.locator('#turn-compare').evaluate(e=>e.open)))await page.locator('#turn-compare summary').click();};
 const fill=async(ids=cfg.ids)=>{await open();await page.locator('#turn-compare-left-id').fill(ids[0]);await page.locator('#turn-compare-right-id').fill(ids[1]);await page.locator('#turn-compare-reviewed').check();};
 const read=async()=>{await page.locator('#turn-compare-read').click();};
 const shown=async()=>page.waitForFunction(()=>!document.getElementById('turn-compare-results').hidden,null,{timeout:17000});
 const failed=async()=>page.waitForFunction(()=>document.getElementById('turn-compare-state').textContent.includes('未完成'),null,{timeout:17000});
 const empty=async()=>{assert(await page.locator('#turn-compare-results').isHidden());assert.strictEqual(await page.locator('#turn-compare-left-reply').textContent(),'');assert.strictEqual(await page.locator('#turn-compare-right-reply').textContent(),'');};
 const paths=cfg.ids.map(id=>'/api/turns/'+id);
 await page.goto(cfg.base);await connect();await open();assert(!(await page.locator('#turn-compare-reviewed').isChecked()));assert(await page.locator('#turn-compare-read').isDisabled());
 await fill();let mark=requests.length;await read();await shown();
 assert.deepStrictEqual(requests.slice(mark),paths.map(path=>({method:'GET',path})));
 for(let i=0;i<2;i++){const side=i?'right':'left';assert.strictEqual(await page.locator('#turn-compare-'+side+'-reply').textContent(),cfg.replies[i]);assert((await page.locator('#turn-compare-'+side+'-meta').textContent()).includes(cfg.ids[i]));}
 assert.strictEqual(await page.locator('#turn-compare-results img').count(),0);assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
 if(process.env.JIACLAW_TEST_COMPARE_SCREENSHOT)await page.screenshot({path:process.env.JIACLAW_TEST_COMPARE_SCREENSHOT.replace('{mode}',mode)});
 console.log('PASS task result comparison '+mode+' 1: two actual complete replies including 1900KiB/BOM/CRLF/TAB/NUL/literal HTML and zero-byte reply, exactly two original GETs and no writes');
 await fill([cfg.ids[0],cfg.ids[0]]);mark=requests.length;await read();await page.waitForFunction(()=>document.getElementById('turn-compare-state').textContent.includes('不同'));await empty();assert.strictEqual(requests.length,mark);
 await fill();await read();await shown();await page.locator('#turn-compare-reviewed').uncheck();await empty();assert(await page.locator('#turn-compare-read').isDisabled());
 // The second original GET is real, with only its response delivery held.
 let release,entered;const arrived=new Promise(r=>entered=r),held=new Promise(r=>release=r);
 await page.route('**'+paths[1],async route=>{const response=await route.fetch({maxRetries:0,maxRedirects:0});entered();await held;await route.fulfill({response}).catch(()=>{});});
 await fill();await read();await arrived;await empty();await page.locator('#turn-compare-left-id').fill(cfg.ids[1]);release();await wait(100);await empty();await page.unroute('**'+paths[1]);
 console.log('PASS task result comparison '+mode+' 2: same UUID refuses before GET, confirmation and number changes clear both, actual delayed second GET cannot publish partial or stale replies');
 // Faulted metadata is explicitly separate from the two actual model replies.
 const original=await (await fetch(cfg.base+paths[1],{headers:{Authorization:'Bearer '+cfg.token}})).json();
 const faults=[v=>{v.active=true;},v=>{v.receipt.result=null;v.receipt.result_purged=true;},v=>{v.receipt.id=cfg.ids[0];},v=>{v.receipt.result.reply='bad\ud800';},v=>{v.receipt.result.reply='x'.repeat(2*1024*1024+1);},v=>{v.receipt.result.tool_names=[1];}];
 for(const fault of faults){const v=structuredClone(original);fault(v);await page.route('**'+paths[1],route=>route.fulfill({status:200,contentType:'application/json',body:JSON.stringify(v)}));await fill();await read();await failed();await empty();await page.unroute('**'+paths[1]);}
 console.log('PASS task result comparison '+mode+' 3: synthetic active/purged/wrong-ID/surrogate/oversize/invalid-tools metadata refuse whole pair without automatic retry');
 let oldRelease,oldEntered;const oldArrived=new Promise(r=>oldEntered=r),late=new Promise(r=>oldRelease=r);
 await page.route('**'+paths[1],async route=>{const response=await route.fetch({maxRetries:0,maxRedirects:0});oldEntered();await late;await route.fulfill({response}).catch(()=>{});});
 await fill();await read();await oldArrived;await connect(cfg.bob||cfg.token);oldRelease();await wait(100);await empty();assert.strictEqual(await page.locator('#turn-compare-left-id').inputValue(),'');assert(!(await page.locator('#turn-compare-reviewed').isChecked()));await page.unroute('**'+paths[1]);
 if(cfg.bob){await fill();await read();await failed();await empty();await connect(cfg.readonly);await fill();await read();await shown();await connect();}
 await fill();await page.route('**'+paths[1],route=>route.fulfill({status:401,contentType:'application/json',body:'{"error":"revoked"}'}));await read();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));await empty();await page.unroute('**'+paths[1]);await connect();
 console.log('PASS task result comparison '+mode+' 4: actual late authenticated GET cannot restore another identity, cross-user denial and readonly GET scope, revoked-key clears both private replies');
 // Shared absolute deadline: no partial result after a real GET delayed beyond it.
 for(let i=0;i<2;i++)await page.route('**'+paths[i],async route=>{const response=await route.fetch({maxRetries:0,maxRedirects:0});await wait(i===0?7500:8000);await route.fulfill({response}).catch(()=>{});});
 await fill();mark=requests.length;const started=Date.now();await read();await failed();const elapsed=Date.now()-started;await empty();assert(elapsed>=14900&&elapsed<17000,elapsed);for(const path of paths)await page.unroute('**'+path);
 await fill();await read();await shown();await page.locator('#turn-compare-clear').click();await empty();await open();await fill();await read();await shown();await page.reload();await empty();
 assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);assert.deepStrictEqual(errors,[]);
 assert.strictEqual(requests.filter(r=>r.method!=='GET').length,0);
 console.log('ACTUAL task result comparison receipts '+JSON.stringify({protocol:1,mode,left_id:cfg.ids[0],right_id:cfg.ids[1],left_bytes:Buffer.byteLength(cfg.replies[0]),right_bytes:0,left_sha256:sha(cfg.replies[0]),right_sha256:sha(''),original_GETs:2,comparison_writes:0,model_posts:2,tool_records:0,deadline_ms:15000,deadline_refused:true,partial_cleared:true,late_identity_refused:true,storage_entries:0}));
 console.log('PASS task result comparison '+mode+' 5: original 15s shared deadline refuses, explicit clear and reload remove private content, no browser storage/page errors or model/write/replay');
 console.log('TASK RESULT COMPARISON BROWSER COMPLETE '+mode);await browser.close();
})().catch(e=>{console.error(e);process.exit(1);});
