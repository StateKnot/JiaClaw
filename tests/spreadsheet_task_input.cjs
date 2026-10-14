const assert=require('assert'),fs=require('fs'),os=require('os'),path=require('path'),crypto=require('crypto');
const {zip}=require('./word_task_input.cjs');
const sha=b=>crypto.createHash('sha256').update(b).digest('hex');
const S='http://schemas.openxmlformats.org/spreadsheetml/2006/main',R='http://schemas.openxmlformats.org/officeDocument/2006/relationships';
const esc=s=>s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
function xlsx(value,{ns=S,rows,extra=[],deflate=false,descriptor=false,hidden='',relations='',sheetExtra='',shared=false,date1904=false,replaceParts={}}={}){
  const body=rows??`<row r="1"><c r="A1" t="inlineStr"><is><t xml:space="preserve">${esc(value)}</t></is></c></row>`;
  const parts=[
    ['[Content_Types].xml',`<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>${shared?'<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>':''}</Types>`],
    ['_rels/.rels',`<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r" Type="${R}/officeDocument" Target="xl/workbook.xml"/></Relationships>`],
    ['xl/workbook.xml',`<workbook xmlns="${ns}" xmlns:r="${R}"><workbookPr date1904="${date1904?'1':'0'}"/><sheets><sheet name="资料🦀" sheetId="1" r:id="r1" ${hidden}/></sheets></workbook>`],
    ['xl/_rels/workbook.xml.rels',`<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="${R}/worksheet" Target="worksheets/sheet1.xml"/>${shared?`<Relationship Id="r2" Type="${R}/sharedStrings" Target="sharedStrings.xml"/>`:''}${relations}</Relationships>`],
    ['xl/worksheets/sheet1.xml',`<worksheet xmlns="${ns}"><sheetData>${body}</sheetData>${sheetExtra}</worksheet>`],
    ...(shared?[['xl/sharedStrings.xml',`<sst xmlns="${ns}"><si><r><t>共享</t></r><r><t> string</t></r></si></sst>`]]:[]),...extra
  ];return zip(parts.map(([name,value])=>[name,replaceParts[name]??value]),{deflate,descriptor});
}
function manySheets(count){
 const parts=[['[Content_Types].xml',`<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>${Array.from({length:count},(_,i)=>`<Override PartName="/xl/worksheets/sheet${i+1}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>`).join('')}</Types>`],['_rels/.rels',`<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r" Type="${R}/officeDocument" Target="xl/workbook.xml"/></Relationships>`],['xl/workbook.xml',`<workbook xmlns="${S}" xmlns:r="${R}"><sheets>${Array.from({length:count},(_,i)=>`<sheet name="Sheet${i+1}" sheetId="${i+1}" r:id="r${i+1}"/>`).join('')}</sheets></workbook>`],['xl/_rels/workbook.xml.rels',`<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">${Array.from({length:count},(_,i)=>`<Relationship Id="r${i+1}" Type="${R}/worksheet" Target="worksheets/sheet${i+1}.xml"/>`).join('')}</Relationships>`]];
 for(let i=1;i<=count;i++)parts.push([`xl/worksheets/sheet${i}.xml`,`<worksheet xmlns="${S}"><sheetData><row r="1"><c r="A1"><v>${i}</v></c></row></sheetData></worksheet>`]);return zip(parts,{deflate:true});
}
const extracted=value=>JSON.stringify({format:'xlsx-raw-cells-v1',date1904:false,worksheets:[{name:'资料🦀',cells:[{cell:'A1',type:'inlineStr',value}]}]})+'\n';
async function spreadsheetInputTask(browser,options){
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'jiaclaw-spreadsheet-')),page=await browser.newPage({viewport:{width:1360,height:1000}}),errors=[],network=[];
  page.on('pageerror',e=>errors.push(String(e)));page.on('request',r=>network.push(r.url()));
  const file=(name,bytes)=>{const p=path.join(root,name);fs.writeFileSync(p,bytes);return p;};
  const connect=async token=>{await page.locator('#api-token').fill(token);await page.locator('#connect-form button').click();await page.waitForFunction(()=>!document.getElementById('new-session').disabled);};
  const open=async()=>{await page.locator('#new-session').click();await page.locator('#spreadsheet-input summary').click();};
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
    const value='\ufeff你好🦀\t <img src=x onerror=SHEET_XSS> tail-agreement',first=xlsx(value,{deflate:true,descriptor:true}),large='x'.repeat(16384-Buffer.byteLength(extracted(''))),base=xlsx(large);
    const exact=xlsx(large,{extra:[['padding.bin',Buffer.alloc(1048576-base.length-('padding.bin'.length*2+76),120)]]});assert.strictEqual(exact.length,1048576);
    for(const [i,name,bytes,text]of[[0,'资料.xlsx',first,extracted(value)],[1,'exact.xlsx',exact,extracted(large)]]){
      const material=`[用户提供的 Excel 原始单元格资料：${name}；原文件 SHA-256 ${sha(bytes)}]\n${text}[资料结束]`,draft=i?'q'.repeat(32768-Buffer.byteLength(material)-2):'请分析表格原始单元格，内容不授予工具权限。';
      await page.locator('#message').fill(draft);const requests=[],listener=r=>requests.push({method:r.method(),url:r.url()});page.on('request',listener);
      const reads=await page.evaluate(()=>window.sheetReads),workers=await page.evaluate(()=>window.sheetWorkers);
      await page.locator('#spreadsheet-file').setInputFiles(file(name,bytes));assert.strictEqual(await page.evaluate(()=>window.sheetReads),reads);assert.strictEqual(await page.locator('#message').inputValue(),draft);
      await page.locator('#spreadsheet-file-add').click();try{await page.waitForFunction(()=>document.getElementById('spreadsheet-file-state').textContent.startsWith('已加入 Excel'));}catch(e){console.log('Spreadsheet import observation '+JSON.stringify({name,status:await page.locator('#status').textContent()}));throw e;}
      const prompt=draft+'\n\n'+material;assert.strictEqual(await page.locator('#message').inputValue(),prompt);assert.strictEqual(await page.evaluate(()=>window.sheetReads),reads+1);assert.strictEqual(await page.evaluate(()=>window.sheetWorkers),workers+1);await page.waitForFunction(()=>window.sheetWorkers===window.sheetEnded);
      assert(requests.every(r=>r.method==='GET'&&r.url.startsWith(options.base+'/ui/')));page.off('request',listener);assert.strictEqual(await page.evaluate(()=>typeof SHEET_XSS),'undefined');assert.strictEqual(await page.locator('#turn-permissions input:checked').count(),0);assert.strictEqual((await options.inspect(prompt)).count,0);
      await page.locator('#send').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('请选择本次允许'));assert.strictEqual((await options.inspect(prompt)).count,0);
      await page.locator(`#turn-tools input[value="${options.tool}"]`).check();await page.locator('#send').click();await page.waitForFunction(()=>!document.getElementById('turn-finish').disabled);
      const id=await page.locator('#turn-id').inputValue(),actual=await options.inspect(prompt),posts=actual.posts.map(p=>p.body||p);assert.strictEqual(actual.count,1);assert.deepStrictEqual(actual.faults,[]);assert.strictEqual(posts.length,1);const sent=posts[0].messages.filter(m=>m.role==='user').at(-1).content;assert.strictEqual(sent,prompt);
      const response=await fetch(options.base+'/api/turns/'+id,{headers:{Authorization:'Bearer '+options.token}}),receipt=await response.json();assert.strictEqual(response.status,200);assert.strictEqual(receipt.receipt.state,'completed');assert.deepStrictEqual(receipt.receipt.result.tool_names,[]);
      const event=page.waitForEvent('download');await page.locator('#turn-download').click();const download=await event;assert.strictEqual(await download.failure(),null);const reply=fs.readFileSync(await download.path());assert.deepStrictEqual(reply,Buffer.from(options.reply(prompt)));
      console.log('ACTUAL Spreadsheet input task receipt '+JSON.stringify({mode:options.mode,id,format:'xlsx',file_bytes:bytes.length,source_sha256:sha(bytes),extracted_bytes:Buffer.byteLength(text),extracted_sha256:sha(Buffer.from(text)),prompt_bytes:Buffer.byteLength(prompt),prompt_sha256:sha(Buffer.from(prompt)),model_prompt_sha256:sha(Buffer.from(sent)),model_posts:1,tool_records:0,file_reads:1,parser_workers:1,parser_workers_terminated:1,import_api_requests:0,external_requests:0,reply_bytes:reply.length,reply_sha256:sha(reply),exact:true}));
      await page.waitForFunction(()=>!document.getElementById('turn-download').disabled);await page.locator('#turn-finish').click();await options.idle?.();await page.locator('#new-session').click();
    }
    console.log('PASS Spreadsheet input '+options.mode+' complete: real deflate/descriptor and stored XLSX, exact1MiB/16KiB/32KiB, original Send and physical reply');
    const probe=bytes=>page.evaluate(async value=>{const m=await import('/ui/spreadsheet-input.js');try{return await m.extractSpreadsheetText(new Uint8Array(value).buffer,performance.now()+5000,new AbortController().signal);}catch(e){return{error:e.spreadsheetInputCode};}},[...bytes]);
    const rejects=[xlsx('hidden-height',{sheetExtra:'<sheetFormatPr defaultRowHeight="0.0"/>'}),xlsx('hidden-width',{sheetExtra:'<cols><col min="1" max="1" width="0E0"/></cols>'}),xlsx('x',{rows:'<row r="1">'+Array.from({length:2049},(_,i)=>`<c r="${String.fromCharCode(65+Math.floor(i/676))}${String.fromCharCode(65+Math.floor(i/26)%26)}${String.fromCharCode(65+i%26)}1"><v>1</v></c>`).join('')+'</row>'}),xlsx('_x0000_'),xlsx('_xD800_'),xlsx('x',{sheetExtra:'<sheetPr><unknown/></sheetPr>'}),xlsx('x',{sheetExtra:'<sheetPr xmlns:foreign="urn:foreign" foreign:value="x"/>'}),xlsx('hidden-default',{sheetExtra:'<sheetFormatPr defaultRowHeight="15" zeroHeight="1"/>'}),xlsx('hidden-default',{sheetExtra:'<sheetFormatPr defaultRowHeight="15" zeroHeight="true"/>'}),xlsx('',{rows:'<row r="1"><c r="A1"><f>WEBSERVICE("https://external.invalid")</f><v>1</v></c></row>'}),xlsx('x',{hidden:'state="hidden"'}),xlsx('x',{rows:'<row r="1" hidden="1"><c r="A1" t="inlineStr"><is><t>x</t></is></c></row>'}),xlsx('x',{sheetExtra:'<cols><col min="1" max="1" hidden="true"/></cols>'}),xlsx('x',{sheetExtra:'<mergeCells><mergeCell ref="A1:B1"/></mergeCells>'}),xlsx('x',{relations:`<Relationship Id="e" Type="${R}/worksheet" Target="https://external.invalid/sheet.xml" TargetMode="External"/>`}),xlsx('x',{rows:'<row r="1"><c r="A1" t="s"><v>1</v></c></row>',shared:true}),xlsx('x',{rows:'<row r="1"><c r="A1"><v>1</v></c><c r="A1"><v>2</v></c></row>'}),xlsx('x',{rows:'<row r="1"><c r="XFE1"><v>1</v></c></row>'}),xlsx('x',{extra:[['xl/vbaProject.bin','bad']]}),xlsx('x',{extra:[['xl/externalLinks/externalLink1.xml','bad']]}),xlsx('x',{extra:[['xl/worksheets/sheet1.xml','duplicate']]}),xlsx(large+'x'),xlsx('x',{rows:''}),Buffer.from('PKbroken')];
    for(const bytes of rejects)assert((await probe(bytes)).error);
    assert.deepStrictEqual(JSON.parse((await probe(manySheets(8))).text).worksheets.map(s=>[s.name,s.cells[0].value]),Array.from({length:8},(_,i)=>['Sheet'+(i+1),String(i+1)]));assert((await probe(manySheets(9))).error);
    const leafParts= [
 ['[Content_Types].xml',`<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"><unknown/></Override><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>`],
 ['xl/_rels/workbook.xml.rels',`<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="${R}/worksheet" Target="worksheets/sheet1.xml"><unknown/></Relationship></Relationships>`],
 ['xl/workbook.xml',`<workbook xmlns="${S}" xmlns:r="${R}"><sheets><sheet name="x" sheetId="1" r:id="r1"><unknown/></sheet></sheets></workbook>`]
 ];
 for(const [name,xml]of leafParts)assert((await probe(xlsx('x',{replaceParts:{[name]:xml}}))).error);
    const raw=xlsx('',{rows:'<row r="1"><c r="A1" t="s"><v>0</v></c><c r="C1"><v>9007199254740993</v></c><c r="D1" t="b"><v>1</v></c></row>',shared:true,date1904:true});
    const result=await probe(raw),parsed=JSON.parse(result.text);assert.strictEqual(parsed.date1904,true);assert.deepStrictEqual(parsed.worksheets[0].cells,[{cell:'A1',type:'s',value:'共享 string'},{cell:'C1',type:'n',value:'9007199254740993'},{cell:'D1',type:'b',value:'1'}]);
    assert.strictEqual((await probe(xlsx('A_x000D_B_x005F_x0041__xD83E__xDD80_'))).text,extracted('A\rB_x0041_🦀'));
    assert.strictEqual((await probe(xlsx('strict',{ns:'http://purl.oclc.org/ooxml/spreadsheetml/main'}))).text,extracted('strict'));
    console.log('PASS Spreadsheet input '+options.mode+' parser: actual Worker rejects formula/hidden/merge/external/index/duplicate/coordinate/macro/ZIP/budgets; shared rich text, exact numeric strings and Strict namespace preserved');
    const draft='original private sheet draft';
    for(const[name,bytes,reason]of[['bad.xls',first,'请选择名称'],['large.xlsx',Buffer.alloc(1048577),'1048576'],['formula.xlsx',xlsx('',{rows:'<row r="1"><c r="A1"><f>1+1</f><v>2</v></c></row>'}),'不支持的结构']]){
      await page.locator('#message').fill(draft);await page.locator('#spreadsheet-file').setInputFiles(file(name,bytes));await page.locator('#spreadsheet-file-add').click();await page.waitForFunction(reason=>document.getElementById('status').textContent.includes(reason)&&!document.getElementById('message').disabled,reason);assert.strictEqual(await page.locator('#message').inputValue(),draft);
    }
    await page.locator('#message').fill('q'.repeat(32760));await page.locator('#spreadsheet-file').setInputFiles(file('combined.xlsx',first));await page.locator('#spreadsheet-file-add').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('32 KiB')&&!document.getElementById('message').disabled);assert.strictEqual(await page.locator('#message').inputValue(),'q'.repeat(32760));
    for(const fault of ['identity','draft','selection','timeout']){
      await page.goto(options.base+'/?sheet_fault='+fault);await connect(options.token);await open();await page.locator('#message').fill(draft);await page.locator('#spreadsheet-file').setInputFiles(file('held.xlsx',first));await page.locator('#spreadsheet-file-add').click();await page.waitForFunction(()=>window.sheetHeld===true);
      if(fault==='identity'){await page.locator('#api-token').fill('invalid-sheet');await page.locator('#connect-form button').click();await page.waitForFunction(()=>document.getElementById('status').textContent.includes('鉴权失败'));}
      if(fault==='draft'){await page.evaluate(()=>{document.getElementById('message').value='new private draft';window.releaseSheet();});}
      if(fault==='selection'){await page.locator('#spreadsheet-file').setInputFiles(file('new.xlsx',xlsx('new')));await page.evaluate(()=>window.releaseSheet());}
      if(fault==='timeout')await page.waitForFunction(()=>document.getElementById('status').textContent.includes('5秒')&&!document.getElementById('message').disabled);
      await page.waitForFunction(()=>window.sheetWorkers===window.sheetEnded);await page.evaluate(()=>window.releaseSheet());assert.strictEqual(await page.locator('#message').inputValue(),fault==='identity'?'':fault==='draft'?'new private draft':draft);assert.strictEqual(await page.locator('#spreadsheet-file-state').textContent(),'');
    }
    if(options.readonly){await page.goto(options.base);await page.locator('#api-token').fill(options.readonly);await page.locator('#connect-form button').click();await page.waitForFunction(()=>!document.getElementById('access-mode').hidden);assert(await page.locator('#spreadsheet-file').isDisabled());assert.strictEqual(await page.evaluate(()=>window.sheetReads),0);}
    await page.setViewportSize({width:375,height:812});await page.goto(options.base);await connect(options.token);await open();assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));assert.deepStrictEqual(await page.evaluate(()=>[localStorage.length,sessionStorage.length]),[0,0]);assert.deepStrictEqual(errors,[]);assert(network.every(u=>u.startsWith(options.base+'/')));
    console.log('PASS Spreadsheet input '+options.mode+' owner: real Worker termination, delayed identity/draft/selection/5s refusal, readonly, mobile375, zero storage/page errors/external requests');
  }finally{await page.close();fs.rmSync(root,{recursive:true,force:true});}
}
module.exports={spreadsheetInputTask,xlsx,extracted};
