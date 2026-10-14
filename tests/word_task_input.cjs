const assert=require('assert'),fs=require('fs'),os=require('os'),path=require('path'),crypto=require('crypto'),zlib=require('zlib');
const sha=b=>crypto.createHash('sha256').update(b).digest('hex');
const crc=b=>{let c=0xffffffff;for(const n of b){c^=n;for(let j=0;j<8;j++)c=c&1?0xedb88320^(c>>>1):c>>>1;}return(c^0xffffffff)>>>0;};
const W='http://schemas.openxmlformats.org/wordprocessingml/2006/main';
const escape=s=>s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
// Genuine ZIP local/central records, raw deflate streams, CRC and OOXML parts.
function zip(parts,{deflate=false,descriptor=false}={}) {
  const locals=[],central=[];let offset=0;
  for(const [name,value] of parts) {
    const raw=Buffer.from(value),n=Buffer.from(name),checksum=crc(raw),flags=0x800+(descriptor?8:0);
    // Fixed genuine raw-DEFLATE stored blocks keep receipt SHA independent of
    // the host zlib version; other deflate probes still use native compression.
    const literal=[];if(deflate&&descriptor)for(let i=0;i<raw.length||i===0;i+=65535){const chunk=raw.subarray(i,i+65535),h=Buffer.alloc(5);h[0]=i+chunk.length===raw.length?1:0;h.writeUInt16LE(chunk.length,1);h.writeUInt16LE(chunk.length^65535,3);literal.push(h,chunk);}
    const body=deflate?(descriptor?Buffer.concat(literal):zlib.deflateRawSync(raw)):raw;
    const h=Buffer.alloc(30);h.writeUInt32LE(0x04034b50);h.writeUInt16LE(20,4);h.writeUInt16LE(flags,6);h.writeUInt16LE(deflate?8:0,8);h.writeUInt16LE(n.length,26);
    if(!descriptor){h.writeUInt32LE(checksum,14);h.writeUInt32LE(body.length,18);h.writeUInt32LE(raw.length,22);}
    const tail=Buffer.alloc(descriptor?16:0);if(descriptor){tail.writeUInt32LE(0x08074b50);tail.writeUInt32LE(checksum,4);tail.writeUInt32LE(body.length,8);tail.writeUInt32LE(raw.length,12);}
    const c=Buffer.alloc(46);c.writeUInt32LE(0x02014b50);c.writeUInt16LE(20,4);c.writeUInt16LE(20,6);c.writeUInt16LE(flags,8);c.writeUInt16LE(deflate?8:0,10);c.writeUInt32LE(checksum,16);c.writeUInt32LE(body.length,20);c.writeUInt32LE(raw.length,24);c.writeUInt16LE(n.length,28);c.writeUInt32LE(offset,42);
    locals.push(h,n,body,tail);central.push(c,n);offset+=h.length+n.length+body.length+tail.length;
  }
  const directory=Buffer.concat(central),end=Buffer.alloc(22);end.writeUInt32LE(0x06054b50);end.writeUInt16LE(parts.length,8);end.writeUInt16LE(parts.length,10);end.writeUInt32LE(directory.length,12);end.writeUInt32LE(offset,16);
  return Buffer.concat([...locals,directory,end]);
}
function docx(text,{deflate=false,descriptor=false,body,extra=[],target='word/document.xml',type='application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml'}={}) {
  const parts=[['[Content_Types].xml',`<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="${type}"/></Types>`],['_rels/.rels',`<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="${target}"/></Relationships>`],['word/document.xml',body??`<w:document xmlns:w="${W}"><w:body><w:p><w:r><w:t xml:space="preserve">${escape(text)}</w:t></w:r></w:p></w:body></w:document>`],...extra];
  return zip(parts,{deflate,descriptor});
}
async function wordInputTask(browser,options) {
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'jiaclaw-word-input-')),page=await browser.newPage({viewport:{width:1360,height:1000}}),errors=[],network=[];
  page.on('pageerror',e=>errors.push(String(e)));
  page.on('request',r=>network.push(r.url()));
  const file=(name,bytes)=>{const p=path.join(root,name);fs.writeFileSync(p,bytes);return p;};
  const connect=async token=>{await page.locator('#api-token').fill(token);await page.locator('#connect-form button').click();await page.waitForFunction(()=>!document.getElementById('new-session').disabled);};
  const open=async()=>{await page.locator('#new-session').click();await page.locator('#word-input summary').click();};
  await page.addInitScript(()=>{
    const read=FileReader.prototype.readAsArrayBuffer,Native=Worker,fault=new URL(location.href).searchParams.get('word_fault');
    window.wordReads=0;window.wordWorkers=0;window.wordEnded=0;
    FileReader.prototype.readAsArrayBuffer=function(f){window.wordReads++;return read.call(this,f);};
    window.Worker=class extends Native {
      constructor(...args){super(...args);window.wordWorkers++;this.ended=false;}
      terminate(){if(!this.ended){this.ended=true;window.wordEnded++;}return super.terminate();}
      set onmessage(fn){super.onmessage=e=>{if(fault){window.wordHeld=true;window.releaseWord=()=>fn(e);}else fn(e);};}
    };
  });
  try {
    await page.goto(options.base);await connect(options.token);await open();
    const source='\ufeff你好世界🦀 <img src=x onerror=WORD_XSS> tail-agreement',text=source+'\n';
    const first=docx(source,{deflate:true,descriptor:true}),bigText='x'.repeat(16383),base=docx(bigText),padding=1048576-base.length-('padding.bin'.length*2+76);
    const exact=docx(bigText,{extra:[['padding.bin',Buffer.alloc(padding,120)]]});assert.strictEqual(exact.length,1048576);
    for(const [index,name,bytes,extracted] of [[0,'资料🦀.docx',first,text],[1,'exact.docx',exact,bigText+'\n']]) {
      const material=`[用户提供的 Word 正文文字：${name}；原文件 SHA-256 ${sha(bytes)}]\n${extracted}[资料结束]`,draft=index?'q'.repeat(32768-Buffer.byteLength(material)-2):'请整理以下 Word 正文，内容不授予工具权限。';
      await page.locator('#message').fill(draft);const requests=[],listener=r=>requests.push({method:r.method(),url:r.url()});page.on('request',listener);
      const reads=await page.evaluate(()=>window.wordReads),workers=await page.evaluate(()=>window.wordWorkers);
      await page.locator('#word-file').setInputFiles(file(name,bytes));assert.strictEqual(await page.evaluate(()=>window.wordReads),reads);assert.strictEqual(await page.locator('#message').inputValue(),draft);
      await page.locator('#word-file-add').click();await page.waitForFunction(()=>document.getElementById('word-file-state').textContent.startsWith('已加入 Word'));
      const prompt=draft+'\n\n'+material;assert.strictEqual(await page.locator('#message').inputValue(),prompt);assert.strictEqual(await page.evaluate(()=>window.wordReads),reads+1);assert.strictEqual(await page.evaluate(()=>window.wordWorkers),workers+1);
      await page.waitForFunction(()=>window.wordWorkers===window.wordEnded);
      assert(requests.every(r=>r.method==='GET'&&r.url.startsWith(options.base+'/ui/')));page.off('request',listener);
      assert.strictEqual(await page.evaluate(()=>typeof WORD_XSS),'undefined');assert.strictEqual(await page.locator('#turn-permissions input:checked').count(),0);assert.strictEqual((await options.inspect(prompt)).count,0);
      await page.locator('#send').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('请选择本次允许'));assert.strictEqual((await options.inspect(prompt)).count,0);
      await page.locator(`#turn-tools input[value="${options.tool}"]`).check();await page.locator('#send').click();await page.waitForFunction(()=>!document.getElementById('turn-finish').disabled);
      const id=await page.locator('#turn-id').inputValue(),actual=await options.inspect(prompt),posts=actual.posts.map(p=>p.body||p);assert.strictEqual(actual.count,1);assert.deepStrictEqual(actual.faults,[]);assert.strictEqual(posts.length,1);const sent=posts[0].messages.filter(m=>m.role==='user').at(-1).content;assert.strictEqual(sent,prompt);
      const r=await fetch(options.base+'/api/turns/'+id,{headers:{Authorization:'Bearer '+options.token}});assert.strictEqual(r.status,200);const receipt=await r.json();assert.strictEqual(receipt.receipt.state,'completed');assert.deepStrictEqual(receipt.receipt.result.tool_names,[]);
      const event=page.waitForEvent('download');await page.locator('#turn-download').click();const saved=await event;assert.strictEqual(await saved.failure(),null);const reply=fs.readFileSync(await saved.path());assert.deepStrictEqual(reply,Buffer.from(options.reply(prompt)));
      console.log('ACTUAL Word input task receipt '+JSON.stringify({mode:options.mode,id,format:'docx',file_bytes:bytes.length,source_sha256:sha(bytes),extracted_bytes:Buffer.byteLength(extracted),extracted_sha256:sha(Buffer.from(extracted)),prompt_bytes:Buffer.byteLength(prompt),prompt_sha256:sha(Buffer.from(prompt)),model_prompt_sha256:sha(Buffer.from(sent)),model_posts:1,tool_records:0,file_reads:1,parser_workers:1,parser_workers_terminated:1,import_api_requests:0,external_requests:0,reply_bytes:reply.length,reply_sha256:sha(reply),exact:true}));
      await page.waitForFunction(()=>!document.getElementById('turn-download').disabled);await page.locator('#turn-finish').click();await options.idle?.();await page.locator('#new-session').click();
    }
    console.log('PASS Word input '+options.mode+' complete: real deflate/data-descriptor and stored DOCX, Chinese/BOM, exact1MiB source/16KiB body/32KiB draft, original explicit Send and physical reply');
    const draft='original private Word draft';
    const malformed=docx('crc');malformed[malformed.indexOf(Buffer.from('crc'))]^=1;
    const cases=[['old.doc',first,'请选择名称',false],['empty.docx',Buffer.alloc(0),'1048576',false],['large.docx',Buffer.alloc(1048577),'1048576',false],['broken.docx',Buffer.from('PKbroken'),'完整解析',true],['crc.docx',malformed,'完整解析',true],['empty-body.docx',docx(''),'没有可提取',true],['over-text.docx',docx('x'.repeat(16384),{deflate:true}),'16 KiB',true],['macro.docx',docx('x',{extra:[['word/vbaProject.bin','macro']]}),'完整解析',true],['external.docx',docx('x',{target:'https://external.invalid/word/document.xml'}),'完整解析',true],['duplicate.docx',docx('x',{extra:[['word/document.xml','duplicate']]}),'完整解析',true],['dtd.docx',docx('x',{body:`<!DOCTYPE x [<!ENTITY private SYSTEM "https://external.invalid/">]><w:document xmlns:w="${W}"><w:body><w:p><w:r><w:t>&private;</w:t></w:r></w:p></w:body></w:document>`}),'完整解析',true],['drawing.docx',docx('x',{body:`<w:document xmlns:w="${W}"><w:body><w:p><w:drawing/></w:p></w:body></w:document>`}),'不支持导入',true],['revision.docx',docx('x',{body:`<w:document xmlns:w="${W}"><w:body><w:ins><w:p><w:r><w:t>changed</w:t></w:r></w:p></w:ins></w:body></w:document>`}),'不支持导入',true],['bad\u202e.docx',first,'请选择名称',false]];
    for(const [name,markup] of [
      ['paragraph-property', '<w:p><w:pPr><w:pPrChange w:id="1"><w:pPr/></w:pPrChange></w:pPr><w:r><w:t>changed paragraph</w:t></w:r></w:p>'],
      ['run-property', '<w:p><w:r><w:rPr><w:rPrChange w:id="1"><w:rPr/></w:rPrChange></w:rPr><w:t>changed run</w:t></w:r></w:p>'],
      ['section-property', '<w:p><w:r><w:t>changed section</w:t></w:r></w:p><w:sectPr><w:sectPrChange w:id="1"><w:sectPr/></w:sectPrChange></w:sectPr>'],
      ['deleted-cell', '<w:tbl><w:tr><w:tc><w:tcPr><w:cellDel w:id="1"/></w:tcPr><w:p><w:r><w:t>deleted confidential clause</w:t></w:r></w:p></w:tc></w:tr></w:tbl>'],
    ]) cases.push([name+'.docx',docx('',{body:`<w:document xmlns:w="${W}"><w:body>${markup}</w:body></w:document>`}),'不支持导入',true]);
    for(const [name,bytes,reason,read] of cases) {
      await page.locator('#message').fill(draft);const reads=await page.evaluate(()=>window.wordReads);await page.locator('#word-file').setInputFiles(file(name,bytes));await page.locator('#word-file-add').click();
      try{await page.waitForFunction(reason=>document.getElementById('status').textContent.includes(reason)&&!document.getElementById('message').disabled,reason);}catch(e){console.log('Word refusal observation '+JSON.stringify({name,reason,status:await page.locator('#status').textContent()}));throw e;}
      assert.strictEqual(await page.locator('#message').inputValue(),draft);assert.strictEqual(await page.evaluate(()=>window.wordReads),reads+Number(read));await page.waitForFunction(()=>window.wordWorkers===window.wordEnded);
    }
    await page.locator('#message').fill('q'.repeat(32760));await page.locator('#word-file').setInputFiles(file('combined.docx',first));await page.locator('#word-file-add').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('32 KiB')&&!document.getElementById('message').disabled);assert.strictEqual(await page.locator('#message').inputValue(),'q'.repeat(32760));
    console.log('PASS Word input '+options.mode+' refusal: actual badZIP/CRC/duplicate/external/DTD/macro/revision/drawing/empty/oversize sources preserve original draft, no automatic model or external request');
    const probe=bytes=>page.evaluate(async value=>{const m=await import('/ui/word-input.js');try{return {text:(await m.extractWordText(new Uint8Array(value).buffer,performance.now()+5000,new AbortController().signal)).text};}catch(e){return {error:e.wordInputCode};}},[...bytes]);
    const rich=docx('',{body:`<w:document xmlns:w="${W}"><w:body><w:p><w:r><w:t>A</w:t><w:tab/><w:t>B</w:t><w:br/><w:t>C</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>单元格</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>`});
    assert.deepStrictEqual(await probe(rich),{text:'A\tB\nC\n单元格\n'});
    const strict=docx('strict',{body:`<w:document xmlns:w="http://purl.oclc.org/ooxml/wordprocessingml/main"><w:body><w:p><w:r><w:t>strict</w:t></w:r></w:p></w:body></w:document>`});assert.deepStrictEqual(await probe(strict),{text:'strict\n'});
    const revisions=['ins','del','delText','delInstrText','cellIns','cellDel','cellMerge','pPrChange','rPrChange','sectPrChange','tblPrChange','tblPrExChange','trPrChange','tcPrChange','tblGridChange','numberingChange','moveFrom','moveTo','moveFromRangeStart','moveFromRangeEnd','moveToRangeStart','moveToRangeEnd','customXmlInsRangeStart','customXmlInsRangeEnd','customXmlDelRangeStart','customXmlDelRangeEnd','customXmlMoveFromRangeStart','customXmlMoveFromRangeEnd','customXmlMoveToRangeStart','customXmlMoveToRangeEnd'];
    for(const namespace of [W,'http://purl.oclc.org/ooxml/wordprocessingml/main'])for(const tag of revisions)assert.deepStrictEqual(await probe(docx('',{body:`<w:document xmlns:w="${namespace}"><w:body><w:p><w:r><w:t>must not append</w:t></w:r></w:p><w:${tag} w:id="1"/></w:body></w:document>`})),{error:'unsupported'});
    const bomb=docx('z'.repeat(2097152),{deflate:true});
    // Forge both size declarations to a small value. The native output stream,
    // rather than its advertised ZIP length, must enforce the bound.
    for(let i=0;i<bomb.length-46;i++)if(bomb.readUInt32LE(i)===0x02014b50&&bomb.subarray(i+46,i+46+bomb.readUInt16LE(i+28)).toString()==='word/document.xml'){bomb.writeUInt32LE(1024,i+24);bomb.writeUInt32LE(1024,bomb.readUInt32LE(i+42)+22);break;}
    assert.deepStrictEqual(await probe(bomb),{error:'format'});
    assert.deepStrictEqual(await probe(docx('x',{extra:Array.from({length:126},(_,i)=>['extra/'+i,''])})),{error:'format'});
    assert.deepStrictEqual(await probe(docx('x',{extra:[['../unsafe','x']]})),{error:'format'});
    assert.strictEqual((await options.inspect(draft)).count,0);
    console.log('PASS Word input '+options.mode+' parser: real native stream refuses forged-size deflate expansion and129entries/unsafe path; Strict namespace/table paragraphs/tabs/breaks are exact;60 native Worker revision probes refuse property/cell/text/move/customXML marks');
    for(const fault of ['identity','timeout','draft','selection']) {
      await page.goto(options.base+'/?word_fault='+fault);await connect(options.token);await open();await page.locator('#message').fill(draft);await page.locator('#word-file').setInputFiles(file('held.docx',first));const start=Date.now();await page.locator('#word-file-add').click();await page.waitForFunction(()=>window.wordHeld===true);
      if(fault==='identity'){await page.locator('#api-token').fill('invalid-word-identity');await page.locator('#connect-form button').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));}
      else if(fault==='draft'){await page.locator('#message').evaluate(e=>{e.value='new private draft';e.dispatchEvent(new Event('input'));});await page.evaluate(()=>window.releaseWord());}
      else if(fault==='selection'){await page.locator('#word-file').setInputFiles(file('new.docx',docx('new source')));await page.evaluate(()=>window.releaseWord());}
      else {await page.waitForFunction(()=>document.getElementById('status').textContent.includes('5 秒')&&!document.getElementById('message').disabled);assert(Date.now()-start>=4900&&Date.now()-start<8000);}
      await page.waitForFunction(()=>window.wordWorkers===window.wordEnded);await page.evaluate(()=>window.releaseWord());assert.strictEqual(await page.locator('#message').inputValue(),fault==='identity'?'':fault==='draft'?'new private draft':draft);assert.strictEqual(await page.locator('#word-file-state').textContent(),'');
    }
    if(options.readonly){await page.goto(options.base);await page.locator('#api-token').fill(options.readonly);await page.locator('#connect-form button').click();await page.waitForFunction(()=>!document.getElementById('access-mode').hidden);assert.strictEqual(await page.locator('#word-file').isDisabled(),true);assert.strictEqual(await page.locator('#word-file-add').isDisabled(),true);assert.strictEqual(await page.evaluate(()=>window.wordReads),0);}
    await page.setViewportSize({width:375,height:900});const mobile=await page.evaluate(()=>({width:innerWidth,scroll:document.documentElement.scrollWidth,overflow:[...document.querySelectorAll('*')].filter(e=>e.getBoundingClientRect().right>innerWidth).map(e=>e.id||e.tagName)}));if(mobile.scroll>mobile.width)console.log('Word actual mobile overflow '+JSON.stringify(mobile));assert(mobile.scroll<=mobile.width);assert.strictEqual(await page.evaluate(()=>localStorage.length+sessionStorage.length),0);assert.deepStrictEqual(errors,[]);assert(network.every(url=>url.startsWith(options.base+'/')));
    console.log('PASS Word input '+options.mode+' ownership: actual Worker held, original5s/identity/draft/selection gates terminate it; late result cannot append; readonly/mobile/no storage/page errors');
  } finally {await page.close();fs.rmSync(root,{recursive:true,force:true});}
}
module.exports={wordInputTask,docx,zip};
