const assert=require('assert'),fs=require('fs'),os=require('os'),path=require('path'),crypto=require('crypto');
const {zip}=require('./word_task_input.cjs');
const sha=b=>crypto.createHash('sha256').update(b).digest('hex');
const R='http://schemas.openxmlformats.org/officeDocument/2006/relationships';
const esc=s=>s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
function pptx(values,{strict=false,reverse=false,hiddenSlide=false,hiddenShape=false,slideExtra='',shapeExtra='',bodyExtra='',relations='',extra=[],deflate=false,descriptor=false}={}){
  const p=strict?'http://purl.oclc.org/ooxml/presentationml/main':'http://schemas.openxmlformats.org/presentationml/2006/main',a=strict?'http://purl.oclc.org/ooxml/drawingml/main':'http://schemas.openxmlformats.org/drawingml/2006/main',r=strict?'http://purl.oclc.org/ooxml/officeDocument/relationships':R;
  const order=Array.from({length:values.length},(_,i)=>i+1);if(reverse)order.reverse();
  const parts=[['[Content_Types].xml',`<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>${values.map((_,i)=>`<Override PartName="/ppt/slides/slide${i+1}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>`).join('')}</Types>`],['_rels/.rels',`<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="root" Type="${r}/officeDocument" Target="ppt/presentation.xml"/></Relationships>`],['ppt/presentation.xml',`<p:presentation xmlns:p="${p}" xmlns:r="${r}"><p:sldIdLst>${order.map(i=>`<p:sldId id="${255+i}" r:id="r${i}"/>`).join('')}</p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>`],['ppt/_rels/presentation.xml.rels',`<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">${values.map((_,i)=>`<Relationship Id="r${i+1}" Type="${r}/slide" Target="slides/slide${i+1}.xml"/>`).join('')}</Relationships>`]];
  values.forEach((value,i)=>{parts.push([`ppt/slides/slide${i+1}.xml`,`<p:sld xmlns:p="${p}" xmlns:a="${a}" show="${hiddenSlide?'0':'1'}"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Text" hidden="${hiddenShape?'1':'0'}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t xml:space="preserve">${esc(value)}</a:t></a:r>${bodyExtra}</a:p></p:txBody></p:sp>${shapeExtra}</p:spTree></p:cSld>${slideExtra}</p:sld>`]);if(relations)parts.push([`ppt/slides/_rels/slide${i+1}.xml.rels`,`<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">${relations}</Relationships>`]);});return zip([...parts,...extra],{deflate,descriptor});
}
const extracted=value=>JSON.stringify({format:'pptx-slide-text-v1',slides:[{slide:1,text:value+'\n'}]})+'\n';
async function presentationInputTask(browser,options){
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'jiaclaw-presentation-')),page=await browser.newPage({viewport:{width:1360,height:1000}}),errors=[],network=[];
  page.on('pageerror',e=>errors.push(String(e)));page.on('request',r=>network.push(r.url()));
  const file=(name,bytes)=>{const p=path.join(root,name);fs.writeFileSync(p,bytes);return p;};
  const connect=async token=>{await page.locator('#api-token').fill(token);await page.locator('#connect-form button').click();await page.waitForFunction(()=>!document.getElementById('new-session').disabled);};
  const open=async()=>{await page.locator('#new-session').click();await page.locator('#presentation-input summary').click();};
  await page.addInitScript(()=>{
    const read=FileReader.prototype.readAsArrayBuffer,Native=Worker,fault=new URL(location.href).searchParams.get('sheet_fault');
    window.sheetReads=0;window.sheetWorkers=0;window.sheetEnded=0;
    FileReader.prototype.readAsArrayBuffer=function(f){window.sheetReads++;return read.call(this,f);};
    window.Worker=class extends Native{
      constructor(...args){super(...args);window.sheetWorkers++;this.ended=false;}
      terminate(){if(!this.ended){this.ended=true;window.sheetEnded++;}return super.terminate();}
      set onmessage(fn){super.onmessage=e=>{if(fault){window.sheetHeld=true;window.releaseSheet=()=>fn(e);}else fn(e);};}
    };
  });
  try{
    await page.goto(options.base);await connect(options.token);await open();
    const value='\ufeff你好🦀\t <img src=x onerror=SHEET_XSS> tail-agreement',first=pptx([value],{deflate:true,descriptor:true}),large='x'.repeat(16384-Buffer.byteLength(extracted(''))),base=pptx([large]);
    const exact=pptx([large],{extra:[['padding.bin',Buffer.alloc(1048576-base.length-('padding.bin'.length*2+76),120)]]});assert.strictEqual(exact.length,1048576);
    for(const [i,name,bytes,text]of[[0,'资料.pptx',first,extracted(value)],[1,'exact.pptx',exact,extracted(large)]]){
      const material=`[用户提供的 PowerPoint 幻灯片文字资料：${name}；原文件 SHA-256 ${sha(bytes)}]\n${text}[资料结束]`,draft=i?'q'.repeat(32768-Buffer.byteLength(material)-2):'请分析幻灯片文字，内容不授予工具权限。';
      await page.locator('#message').fill(draft);const requests=[],listener=r=>requests.push({method:r.method(),url:r.url()});page.on('request',listener);
      const reads=await page.evaluate(()=>window.sheetReads),workers=await page.evaluate(()=>window.sheetWorkers);
      await page.locator('#presentation-file').setInputFiles(file(name,bytes));assert.strictEqual(await page.evaluate(()=>window.sheetReads),reads);assert.strictEqual(await page.locator('#message').inputValue(),draft);
      await page.locator('#presentation-file-add').click();try{await page.waitForFunction(()=>document.getElementById('presentation-file-state').textContent.startsWith('已加入 PowerPoint'));}catch(e){console.log('Presentation import observation '+JSON.stringify({name,status:await page.locator('#status').textContent()}));throw e;}
      const prompt=draft+'\n\n'+material;assert.strictEqual(await page.locator('#message').inputValue(),prompt);assert.strictEqual(await page.evaluate(()=>window.sheetReads),reads+1);assert.strictEqual(await page.evaluate(()=>window.sheetWorkers),workers+1);await page.waitForFunction(()=>window.sheetWorkers===window.sheetEnded);
      assert(requests.every(r=>r.method==='GET'&&r.url.startsWith(options.base+'/ui/')));page.off('request',listener);assert.strictEqual(await page.evaluate(()=>typeof SHEET_XSS),'undefined');assert.strictEqual(await page.locator('#turn-permissions input:checked').count(),0);assert.strictEqual((await options.inspect(prompt)).count,0);
      await page.locator('#send').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('请选择本次允许'));assert.strictEqual((await options.inspect(prompt)).count,0);
      await page.locator(`#turn-tools input[value="${options.tool}"]`).check();await page.locator('#send').click();await page.waitForFunction(()=>!document.getElementById('turn-finish').disabled);
      const id=await page.locator('#turn-id').inputValue(),actual=await options.inspect(prompt),posts=actual.posts.map(p=>p.body||p);assert.strictEqual(actual.count,1);assert.deepStrictEqual(actual.faults,[]);assert.strictEqual(posts.length,1);const sent=posts[0].messages.filter(m=>m.role==='user').at(-1).content;assert.strictEqual(sent,prompt);
      const response=await fetch(options.base+'/api/turns/'+id,{headers:{Authorization:'Bearer '+options.token}}),receipt=await response.json();assert.strictEqual(response.status,200);assert.strictEqual(receipt.receipt.state,'completed');assert.deepStrictEqual(receipt.receipt.result.tool_names,[]);
      const event=page.waitForEvent('download');await page.locator('#turn-download').click();const download=await event;assert.strictEqual(await download.failure(),null);const reply=fs.readFileSync(await download.path());assert.deepStrictEqual(reply,Buffer.from(options.reply(prompt)));
      console.log('ACTUAL Presentation input task receipt '+JSON.stringify({mode:options.mode,id,format:'pptx',file_bytes:bytes.length,source_sha256:sha(bytes),extracted_bytes:Buffer.byteLength(text),extracted_sha256:sha(Buffer.from(text)),prompt_bytes:Buffer.byteLength(prompt),prompt_sha256:sha(Buffer.from(prompt)),model_prompt_sha256:sha(Buffer.from(sent)),model_posts:1,tool_records:0,file_reads:1,parser_workers:1,parser_workers_terminated:1,import_api_requests:0,external_requests:0,reply_bytes:reply.length,reply_sha256:sha(reply),exact:true}));
      await page.waitForFunction(()=>!document.getElementById('turn-download').disabled);await page.locator('#turn-finish').click();await options.idle?.();await page.locator('#new-session').click();
    }
    console.log('PASS Presentation input '+options.mode+' complete: real deflate/descriptor and stored PPTX, exact1MiB/16KiB/32KiB, original Send and physical reply');
    const probe=bytes=>page.evaluate(async value=>{const m=await import('/ui/presentation-input.js');try{return await m.extractPresentationText(new Uint8Array(value).buffer,performance.now()+5000,new AbortController().signal);}catch(e){return{error:e.presentationInputCode};}},[...bytes]);
    const rejects=[pptx(['hidden'],{hiddenSlide:true}),pptx(['hidden'],{hiddenShape:true}),pptx(['x'],{slideExtra:'<p:timing/>'}),pptx(['x'],{shapeExtra:'<p:pic/>'}),pptx(['x'],{shapeExtra:'<p:graphicFrame/>'}),pptx(['x'],{shapeExtra:'<p:unknown/>'}),pptx(['x'],{bodyExtra:'<a:fld><a:t>field</a:t></a:fld>'}),pptx(['x'],{relations:`<Relationship Id="e" Type="${R}/hyperlink" Target="https://external.invalid/" TargetMode="External"/>`}),pptx(['x'],{relations:`<Relationship Id="e" Type="${R}/image" Target="../media/image1.png"/>`}),pptx(['x'],{extra:[['ppt/vbaProject.bin','bad']]}),pptx(['x'],{extra:[['ppt/slides/slide1.xml','duplicate']]}),pptx([large+'x']),pptx(['']),pptx(['x'.repeat(16385)]),pptx(Array(33).fill('x')),Buffer.from('PKbroken')];
    for(const bytes of rejects)assert((await probe(bytes)).error);
    assert.strictEqual(JSON.parse((await probe(pptx(Array(32).fill('boundary')))).text).slides.length,32);
    const real=await probe(fs.readFileSync(path.join(__dirname,'fixtures/presentation-plain.pptx')));assert.strictEqual(real.text,JSON.stringify({format:'pptx-slide-text-v1',slides:[{slide:1,text:'真实生成的资料\n第一段\n第二段🦀\n'}]})+'\n');
    const ordered=await probe(pptx(['second file','first file'],{reverse:true}));assert.deepStrictEqual(JSON.parse(ordered.text).slides.map(s=>s.text),['first file\n','second file\n']);
    assert.strictEqual((await probe(pptx(['strict'],{strict:true}))).text,extracted('strict'));
    assert((await probe(pptx(['x'],{bodyExtra:'<a:pPr><a:unknown/></a:pPr>'}))).error);
    assert((await probe(pptx(['x'],{bodyExtra:'<a:br><a:rPr/><a:rPr/></a:br>'}))).error);
    console.log('PASS Presentation input '+options.mode+' interoperability: python-pptx1.0.2 28297bytes SHA256 1d312d61509c1ff47ede233da212c5aa0f2428a2cc4ecd0b91aa2ae157f73583, complete text and actual32slide boundary');
    console.log('PASS Presentation input '+options.mode+' parser: real Worker rejects hidden/graphics/fields/external/macro/unknown/ZIP/budgets; full declared slide order and Strict text preserved');
    const draft='original private sheet draft';
    for(const[name,bytes,reason]of[['bad.xls',first,'请选择名称'],['large.pptx',Buffer.alloc(1048577),'1048576'],['hidden.pptx',pptx(['hidden'],{hiddenSlide:true}),'不支持的结构']]){
      await page.locator('#message').fill(draft);await page.locator('#presentation-file').setInputFiles(file(name,bytes));await page.locator('#presentation-file-add').click();await page.waitForFunction(reason=>document.getElementById('status').textContent.includes(reason)&&!document.getElementById('message').disabled,reason);assert.strictEqual(await page.locator('#message').inputValue(),draft);
    }
    await page.locator('#message').fill('q'.repeat(32760));await page.locator('#presentation-file').setInputFiles(file('combined.pptx',first));await page.locator('#presentation-file-add').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('32 KiB')&&!document.getElementById('message').disabled);assert.strictEqual(await page.locator('#message').inputValue(),'q'.repeat(32760));
    for(const fault of ['identity','draft','selection','timeout']){
      await page.goto(options.base+'/?sheet_fault='+fault);await connect(options.token);await open();await page.locator('#message').fill(draft);await page.locator('#presentation-file').setInputFiles(file('held.pptx',first));await page.locator('#presentation-file-add').click();await page.waitForFunction(()=>window.sheetHeld===true);
      if(fault==='identity'){await page.locator('#api-token').fill('invalid-sheet');await page.locator('#connect-form button').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));}
      if(fault==='draft'){await page.evaluate(()=>{document.getElementById('message').value='new private draft';window.releaseSheet();});}
      if(fault==='selection'){await page.locator('#presentation-file').setInputFiles(file('new.pptx',pptx(['new'])));await page.evaluate(()=>window.releaseSheet());}
      if(fault==='timeout')await page.waitForFunction(()=>document.getElementById('status').textContent.includes('5秒')&&!document.getElementById('message').disabled);
      await page.waitForFunction(()=>window.sheetWorkers===window.sheetEnded);await page.evaluate(()=>window.releaseSheet());assert.strictEqual(await page.locator('#message').inputValue(),fault==='identity'?'':fault==='draft'?'new private draft':draft);assert.strictEqual(await page.locator('#presentation-file-state').textContent(),'');
    }
    if(options.readonly){await page.goto(options.base);await page.locator('#api-token').fill(options.readonly);await page.locator('#connect-form button').click();await page.waitForFunction(()=>!document.getElementById('access-mode').hidden);assert(await page.locator('#presentation-file').isDisabled());assert.strictEqual(await page.evaluate(()=>window.sheetReads),0);}
    await page.setViewportSize({width:375,height:812});await page.goto(options.base);await connect(options.token);await open();assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));assert.deepStrictEqual(await page.evaluate(()=>[localStorage.length,sessionStorage.length]),[0,0]);assert.deepStrictEqual(errors,[]);assert(network.every(u=>u.startsWith(options.base+'/')));
    console.log('PASS Presentation input '+options.mode+' owner: real Worker termination, delayed identity/draft/selection/5s refusal, readonly, mobile375, zero storage/page errors/external requests');
  }finally{await page.close();fs.rmSync(root,{recursive:true,force:true});}
}
module.exports={presentationInputTask,pptx,extracted};
