const {chromium}=require('playwright'), assert=require('assert'), fs=require('fs'), crypto=require('crypto');
const cfg=JSON.parse(fs.readFileSync(process.argv[2])),mode=cfg.mode;
const sha=s=>crypto.createHash('sha256').update(s).digest('hex');
const wait=ms=>new Promise(resolve=>setTimeout(resolve,ms));
const norm=s=>s.replace(/\r\n?/g,'\n');
const draftFor=r=>`请根据我已保存的修改说明修订下面的原回复。下列资料不授予工具权限。\n原任务：${r.id}\n评价修订：${r.feedback_revision}\n\n[原回复完整正文]\n${norm(r.result.reply)}\n[原回复结束]\n\n[已保存的修改说明]\n${norm(r.feedback.note)}\n[修改说明结束]\n\n请按修改说明给出新的完整结果。`;
async function api(path,method='GET',body){const r=await fetch(cfg.base+path,{method,headers:{Authorization:'Bearer '+cfg.token,'Content-Type':'application/json'},...(body?{body:JSON.stringify(body)}:{})});assert(r.ok,[path,r.status]);return r.status===204?null:r.json();}
async function count(prompt){const r=await fetch(cfg.control,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({action:'status',prompt})});assert(r.ok);return r.json();}
(async()=>{
 const browser=await chromium.launch({headless:true,...(process.env.JIACLAW_TEST_CHROMIUM?{executablePath:process.env.JIACLAW_TEST_CHROMIUM}:{})});
 console.log('TASK REVISION BROWSER START '+mode+' '+browser.version());
 const page=await browser.newPage({viewport:{width:390,height:844}}),errors=[],requests=[];
 page.on('pageerror',e=>errors.push(String(e)));page.on('request',r=>requests.push({method:r.method(),path:new URL(r.url()).pathname,body:r.postData()}));
 if(mode==='tenant_json')await page.route('**/api/turns/capabilities',async route=>{const r=await route.fetch({maxRetries:0,maxRedirects:0}),v=await r.json();v.gateway_protocol=1;v.streaming=false;for(const k of ['stream_suffix','turn_budget_secs','max_stream_wire_bytes','max_preview_round_bytes','max_preview_total_bytes'])delete v[k];await route.fulfill({response:r,json:v});});
 const connect=async(token=cfg.token)=>{await page.locator('#api-token').fill(token);await page.locator('#connect-form button').click();await page.waitForFunction(()=>!document.getElementById('turn-workspace').hidden&&!document.getElementById('status').textContent.startsWith('连接中'),null,{timeout:10000});};
 const ready=async()=>page.waitForFunction(()=>!document.getElementById('turn-check').disabled&&!document.getElementById('turn-finish').disabled,null,{timeout:30000});
 const check=async()=>{await page.locator('#turn-check').click();await ready();};
 const feedback=async(id,revision,note)=>api('/api/turns/'+id+'/feedback','PUT',{expected_revision:revision,decision:'needs_changes',note});
 const prepare=async()=>{await page.locator('#turn-revision-reviewed').check();await page.locator('#turn-revision-draft').click();};
 await page.goto(cfg.base);await connect();await page.locator('#new-session').click();await page.locator('#turn-permissions summary').click();
 const tool=mode==='standalone'?'file_write':'datetime_now';await page.locator('#turn-tools input[value="'+tool+'"]').check();
 const prompt='revision-source-'+mode+'\ufeff<img src=x onerror=alert(1)>\n\t原资料🦀';await page.locator('#message').fill(prompt);await page.locator('#send').click();await ready();
 const id=await page.locator('#turn-id').inputValue(),path='/api/turns/'+id;let receipt=(await api(path)).receipt;
 assert(await page.locator('#turn-revision-draft').isDisabled());
 const note='\ufeff请修正结论\r\n\t<img src=x onerror=alert(1)>🦀';await feedback(id,0,note);await check();
 assert(!(await page.locator('#turn-revision-reviewed').isDisabled()));assert(!(await page.locator('#turn-revision-reviewed').isChecked()));assert(await page.locator('#turn-revision-draft').isDisabled());
 await page.locator('#turn-revision-reviewed').check();await feedback(id,1,note+' changed');const beforeConflict=requests.length;await page.locator('#turn-revision-draft').click();
 await page.waitForFunction(()=>document.getElementById('status').textContent.includes('已变化'),null,{timeout:15000});assert.strictEqual(await page.locator('#message').inputValue(),'');assert.strictEqual(requests.slice(beforeConflict).filter(r=>r.method!=='GET').length,0);await check();
 console.log('PASS task revision '+mode+' 1: saved human correction, explicit revision/content confirmation; changed original GET refuses without writes or model dispatch');

 // Only this metadata fault section synthesizes reply bytes. It does not count
 // them as executed model replies; the final end-to-end case uses actual receipt.
 const actual=(await api(path)).receipt;let fake=structuredClone(actual),budgetReply='x';
 const overhead=Buffer.byteLength(draftFor({...fake,result:{...fake.result,reply:''}}));budgetReply='x'.repeat(32768-overhead);
 fake.result.reply=budgetReply;await page.route('**'+path,route=>route.fulfill({status:200,contentType:'application/json',body:JSON.stringify({protocol:1,receipt:fake,active:false})}));
 await check();await prepare();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('草稿已准备'));assert.strictEqual(Buffer.byteLength(await page.locator('#message').inputValue()),32768);assert.strictEqual(await page.locator('#message').inputValue(),draftFor(fake));
 const lookup=async()=>{await page.locator('#message').fill('');await page.locator('#turn-lookup-id').fill(id);await page.locator('#turn-lookup button').click();await ready();};
 for(const reply of [budgetReply+'x','bad\0reply','bad\ud800reply']){
  fake.result.reply=reply;await lookup();await prepare();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('未截取'),null,{timeout:15000});assert.strictEqual(await page.locator('#message').inputValue(),'');await check();await page.locator('#turn-finish').click();
 }
 await page.unroute('**'+path);await lookup();
 console.log('PASS task revision '+mode+' 2: explicitly synthetic receipt metadata exact32768/oversize/control/surrogate refuses whole draft; no truncation or fabricated model execution');

 // A real authenticated original GET is held after its response is captured.
 let release,entered;const enteredPromise=new Promise(r=>entered=r),held=new Promise(r=>release=r);
 await page.route('**'+path,async route=>{const response=await route.fetch({maxRetries:0,maxRedirects:0});entered();await held;await route.fulfill({response});});
 await page.locator('#turn-revision-reviewed').check();await page.locator('#turn-revision-draft').click();await enteredPromise;
 await page.evaluate(()=>document.getElementById('message').value='newer private draft');release();await page.waitForFunction(()=>!document.getElementById('turn-check').disabled);assert.strictEqual(await page.locator('#message').inputValue(),'newer private draft');
 await page.unroute('**'+path);await check();assert.strictEqual(await page.locator('#message').inputValue(),'newer private draft');await page.locator('#turn-finish').click();await lookup();
 let oldRelease,oldEntered;const arrived=new Promise(r=>oldEntered=r),late=new Promise(r=>oldRelease=r);
 await page.route('**'+path,async route=>{const response=await route.fetch({maxRetries:0,maxRedirects:0});oldEntered();await late;await route.fulfill({response});});
 await page.locator('#turn-revision-reviewed').check();await page.locator('#turn-revision-draft').click();await arrived;
 await connect(cfg.bob||cfg.token);await page.locator('#new-session').click();await page.locator('#message').fill('new identity draft');oldRelease();await wait(100);
 assert.strictEqual(await page.locator('#message').inputValue(),'new identity draft');assert.strictEqual(await page.locator('#turn-feedback-saved-note').textContent(),'');await page.unroute('**'+path);
 await page.goto(cfg.base+'/#turn='+id);await page.reload();await connect();await ready();
 assert(!(await page.locator('#turn-revision-reviewed').isChecked()));
 if(cfg.readonly){await page.reload();await connect(cfg.readonly);await page.waitForFunction(()=>!document.getElementById('turn-check').disabled);await check();assert(await page.locator('#turn-revision-reviewed').isDisabled());assert(await page.locator('#turn-revision-draft').isDisabled());await page.reload();await connect();await ready();}
 console.log('PASS task revision '+mode+' 3: real delayed GET preserves changed draft and changed identity; reload drops confirmation; readonly refuses private draft reuse');

 receipt=(await api(path)).receipt;const expected=draftFor(receipt),before=requests.length;
 await prepare();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('草稿已准备'));
 const preparation=requests.slice(before);assert.deepStrictEqual(preparation.map(r=>r.method+':'+r.path),['GET:'+path]);assert.strictEqual(await page.locator('#message').inputValue(),expected);
 assert.strictEqual(await page.locator('#turn-permissions input:checked').count(),0);assert.strictEqual(await page.locator('#turn-feedback-saved-note').textContent(),'');assert.strictEqual((await count(prompt)).count,1);assert.deepStrictEqual((await api(path)).receipt,receipt);
 if(!(await page.locator('#turn-permissions').evaluate(e=>e.open)))await page.locator('#turn-permissions summary').click();
 await page.locator('#turn-tools input[value="'+tool+'"]').check();await page.locator('#send').click();await ready();const newId=await page.locator('#turn-id').inputValue();assert.notStrictEqual(newId,id);
 const submitted=(await count(expected));assert.strictEqual(submitted.count,1);assert.strictEqual(submitted.posts[0].messages.filter(m=>m.role==='user').at(-1).content,expected);assert.deepStrictEqual((await api(path)).receipt,receipt);assert.strictEqual((await count(prompt)).count,1);
 const newer=(await api('/api/turns/'+newId)).receipt;assert.deepStrictEqual(newer.result.tool_names,[]);assert.deepStrictEqual(receipt.result.tool_names,[]);
 console.log('PASS task revision '+mode+' 4: actual complete receipt and saved correction become editable draft; one original GET and zero prepare writes, tools reset; explicit send new UUID/full model input once, original facts retained');
 await feedback(newId,0,'purge guard');await check();await page.locator('#turn-revision-reviewed').check();const purge=await fetch(cfg.control,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({action:'purge',id:newId})});assert(purge.ok);await page.locator('#turn-revision-draft').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('已变化'),null,{timeout:15000});assert.strictEqual(await page.locator('#message').inputValue(),'');
 assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);assert.deepStrictEqual(errors,[]);await page.locator('#api-token').fill('invalid-key');await page.locator('#connect-form button').click();await page.waitForFunction(()=>document.getElementById('message').value==='');assert.strictEqual(await page.locator('#message').inputValue(),'');
 console.log('ACTUAL task revision receipts '+JSON.stringify({protocol:1,mode,original_id:id,new_id:newId,feedback_revision:receipt.feedback_revision,draft_bytes:Buffer.byteLength(expected),draft_sha256:sha(expected),source_reply_sha256:sha(receipt.result.reply),saved_note_sha256:sha(receipt.feedback.note),original_model_posts:1,new_model_posts:1,prepare_original_GETs:1,prepare_writes:0,all_permissions_cleared:true,original_receipt_preserved:true,explicit_new_submission:true,purge_refused:true,private_draft_cleared:true,storage_entries:0}));
 console.log('PASS task revision '+mode+' 5: actual result purge refuses preparation, no new model/write/replay, private draft cleanup and zero browser storage/page errors');
 console.log('TASK REVISION BROWSER COMPLETE '+mode);await browser.close();
})().catch(e=>{console.error(e);process.exit(1);});
