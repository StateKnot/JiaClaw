// Bounded ZIP metadata validation; inflation is the browser's native stream.
// No file paths, URLs, external relationships or document code are executed.
const fail = () => { throw new Error('format'); };
const crcTable = Uint32Array.from({length:256}, (_, n) => {
  for (let i=0;i<8;i++) n = n & 1 ? 0xedb88320 ^ (n >>> 1) : n >>> 1;
  return n >>> 0;
});
const crc32 = bytes => { let crc=0xffffffff; for (const b of bytes) crc=crcTable[(crc^b)&255]^(crc>>>8); return (crc^0xffffffff)>>>0; };
self.onmessage = async event => {
  try {
    const format=event.data instanceof ArrayBuffer?'word':event.data?.format;
    if(!['word','xlsx'].includes(format))fail();
    const bytes=new Uint8Array(format==='word'?event.data:event.data.bytes), view=new DataView(bytes.buffer);
    if (!bytes.length || bytes.length>1048576) fail();
    const u16=p=>view.getUint16(p,true), u32=p=>view.getUint32(p,true);
    const extras=(begin,length)=>{
      let p=begin;const end=begin+length;
      while(p<end) {if(p+4>end || [1,0x9901,0x7075].includes(u16(p)))fail();const size=u16(p+2);p+=4+size;if(p>end)fail();}
    };
    let end=-1;
    for(let p=bytes.length-22;p>=Math.max(0,bytes.length-65557);p--) {
      if(u32(p)===0x06054b50 && p+22+u16(p+20)===bytes.length) { end=p; break; }
    }
    if(end<0 || u16(end+4)!==0 || u16(end+6)!==0 || u16(end+8)!==u16(end+10)) fail();
    const count=u16(end+10), start=u32(end+16), size=u32(end+12);
    if(!count || count>128 || start+size!==end) fail();
    const names=new Set(), entries=[], intervals=[]; let cursor=start,total=0;
    for(let i=0;i<count;i++) {
      if(cursor+46>end || u32(cursor)!==0x02014b50) fail();
      const flags=u16(cursor+8), method=u16(cursor+10), crc=u32(cursor+16), compressed=u32(cursor+20), plain=u32(cursor+24);
      const nameLength=u16(cursor+28), extra=u16(cursor+30), comment=u16(cursor+32), local=u32(cursor+42);
      if((flags & ~0x080e)!==0 || ![0,8].includes(method) || (method===0 && (flags&6)) || u16(cursor+34)!==0 || !nameLength || nameLength>256 || plain>4194304) fail();
      total+=plain; if(total>8388608 || cursor+46+nameLength+extra+comment>end) fail();
      const rawName=bytes.subarray(cursor+46,cursor+46+nameLength), name=new TextDecoder('utf-8',{fatal:true,ignoreBOM:true}).decode(rawName);
      if(!name || name.startsWith('\ufeff') || /[\p{Cc}\p{Bidi_Control}\\]/u.test(name) || name.startsWith('/') || name.split('/').some((part,index,all)=>part==='.'||part==='..'||(!part && index!==all.length-1)) || names.has(name)) fail();
      extras(cursor+46+nameLength,extra);
      if(/(?:^|\/)vbaProject\.bin$/i.test(name)) fail();
      names.add(name);
      if(local+30>start || u32(local)!==0x04034b50 || u16(local+6)!==flags || u16(local+8)!==method || u16(local+26)!==nameLength) fail();
      const data=local+30+nameLength+u16(local+28), finish=data+compressed;
      if(finish>start || !rawName.every((b,n)=>bytes[local+30+n]===b)) fail();
      extras(local+30+nameLength,u16(local+28));
      const recorded=[u32(local+14),u32(local+18),u32(local+22)], expected=[crc,compressed,plain];
      if(recorded.some((n,j)=>n!==expected[j] && (!(flags&8)||n!==0))) fail();
      let limit=finish;
      if(flags&8) {
        if(limit+12>start) fail();
        if(u32(limit)===0x08074b50) limit+=4;
        if(limit+12>start || [u32(limit),u32(limit+4),u32(limit+8)].some((n,j)=>n!==expected[j])) fail();
        limit+=12;
      }
      intervals.push([local,limit]); entries.push({name,method,crc,plain,data,finish});
      cursor+=46+nameLength+extra+comment;
    }
    if(cursor!==end) fail();
    intervals.sort((a,b)=>a[0]-b[0]); let offset=0;
    for(const [begin,finish] of intervals) { if(begin!==offset) fail(); offset=finish; }
    if(offset!==start) fail();
    const wanted={'[Content_Types].xml':65536,'_rels/.rels':65536}, result={};
    if(format==='word')wanted['word/document.xml']=1048576;
    else {
      wanted['xl/workbook.xml']=65536;wanted['xl/_rels/workbook.xml.rels']=65536;
      for(const e of entries) {
        if(['xl/','xl/_rels/','xl/theme/','xl/worksheets/'].includes(e.name)&&e.plain===0&&e.data===e.finish)continue;
        if(e.name.startsWith('xl/') && !/^xl\/(?:workbook\.xml|_rels\/workbook\.xml\.rels|sharedStrings\.xml|styles\.xml|theme\/theme[0-9]+\.xml|worksheets\/[A-Za-z0-9_-]+\.xml)$/.test(e.name))fail();
        if(/^xl\/(?:sharedStrings\.xml|worksheets\/[A-Za-z0-9_-]+\.xml)$/.test(e.name))wanted[e.name]=1048576;
      }
    }
    for(const [name,budget] of Object.entries(wanted)) {
      const entry=entries.find(e=>e.name===name); if(!entry || !entry.plain || entry.plain>budget) fail();
      let output;
      if(entry.method===0) output=bytes.slice(entry.data,entry.finish);
      else {
        const reader=new Blob([bytes.subarray(entry.data,entry.finish)]).stream().pipeThrough(new DecompressionStream('deflate-raw')).getReader();
        let length=0; const chunks=[];
        try {
          while(true) { const c=await reader.read(); if(c.done) break; length+=c.value.byteLength; if(length>entry.plain || length>budget) fail(); chunks.push(c.value); }
        } finally { await reader.cancel().catch(()=>{}); reader.releaseLock(); }
        output=new Uint8Array(length); let pos=0; for(const chunk of chunks) { output.set(chunk,pos); pos+=chunk.length; }
      }
      if(output.length!==entry.plain || crc32(output)!==entry.crc) fail();
      result[name]=new TextDecoder('utf-8',{fatal:true}).decode(output);
    }
    self.postMessage({parts:result});
  } catch { self.postMessage({error:'format'}); }
};
