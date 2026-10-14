import {extractOfficeParts} from '/ui/office-input.js';
// Raw cells, never formula evaluation or a claim about Excel's rendered view.
const fail=code=>Object.assign(new Error(code),{spreadsheetInputCode:code});
const NS=['http://schemas.openxmlformats.org/spreadsheetml/2006/main','http://purl.oclc.org/ooxml/spreadsheetml/main'];
const decodeXstring=value=>value.replace(/_x([0-9A-Fa-f]{4})_/g,(_,hex)=>String.fromCharCode(parseInt(hex,16)));
const REL=['http://schemas.openxmlformats.org/officeDocument/2006/relationships/','http://purl.oclc.org/ooxml/officeDocument/relationships/'];
export async function extractSpreadsheetText(bytes,deadline,signal) {
  const check=()=>{if(signal.aborted)throw fail('cancelled');if(performance.now()>=deadline)throw fail('timeout');};
  try {
    const parts=await extractOfficeParts(bytes,deadline,signal,'xlsx');check();
    let nodes=0,cells=0;
    const parse=(name,namespaces,root)=>{
      const source=parts?.[name];
      if(typeof source!=='string'||!source.isWellFormed()||/<!\s*(DOCTYPE|ENTITY)/i.test(source))throw fail('format');
      const doc=new DOMParser().parseFromString(source,'application/xml');check();
      if(doc.getElementsByTagName('parsererror').length||![namespaces].flat().includes(doc.documentElement.namespaceURI)||doc.documentElement.localName!==root)throw fail('format');
      const stack=[doc.documentElement];
      while(stack.length){check();const n=stack.pop();
        for(const a of n.attributes)if(a.namespaceURI&&!['http://www.w3.org/2000/xmlns/','http://www.w3.org/XML/1998/namespace'].includes(a.namespaceURI)&&!(n.localName==='sheet'&&a.localName==='id'&&REL.some(p=>a.namespaceURI===p.slice(0,-1))))throw fail('unsupported');
        if(n.namespaceURI!==doc.documentElement.namespaceURI)throw fail('unsupported');if(++nodes>50000)throw fail('budget');for(const c of n.children)stack.push(c);}
      return doc.documentElement;
    };
    const children=(node,allowed)=>{
      const result=[...node.children];
      if(result.some(n=>n.namespaceURI!==node.namespaceURI||!allowed.includes(n.localName)))throw fail('unsupported');
      return result;
    };
    const plainText=node=>{
      if(node.children.length)throw fail('format');const value=decodeXstring(node.textContent);
      if(!value.isWellFormed()||/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/.test(value))throw fail('text');return value;
    };
    const string=node=>{
      let value='';
      for(const c of children(node,['t','r'])) {
        if(c.localName==='t')value+=plainText(c);
        else for(const r of children(c,['rPr','t'])){if(r.localName==='t')value+=plainText(r);else for(const property of children(r,['rFont','charset','family','b','i','strike','outline','shadow','condense','extend','color','sz','u','vertAlign','scheme']))if(property.children.length)throw fail('unsupported');}
      }
      if(new TextEncoder().encode(value).length>16384)throw fail('budget');return value;
    };
    const typeNS='http://schemas.openxmlformats.org/package/2006/content-types';
    const types=children(parse('[Content_Types].xml',typeNS,'Types'),['Default','Override']);
    if(types.some(n=>n.children.length))throw fail('unsupported');
    const mime=(name,expected)=>{const t=types.filter(n=>n.localName==='Override'&&n.getAttribute('PartName')==='/'+name);if(t.length!==1||t[0].getAttribute('ContentType')!==expected)throw fail('format');};
    mime('xl/workbook.xml','application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml');
    const relationships=name=>{
      const rows=children(parse(name,'http://schemas.openxmlformats.org/package/2006/relationships','Relationships'),['Relationship']),ids=new Set();
      for(const r of rows){if(r.children.length)throw fail('unsupported');const id=r.getAttribute('Id');if(!id||ids.has(id)||!['','Internal'].includes(r.getAttribute('TargetMode')||''))throw fail('unsupported');ids.add(id);}
      return rows;
    };
    const root=relationships('_rels/.rels').filter(n=>REL.some(p=>n.getAttribute('Type')===p+'officeDocument'));
    if(root.length!==1||!['xl/workbook.xml','/xl/workbook.xml'].includes(root[0].getAttribute('Target')))throw fail('format');
    const workbook=parse('xl/workbook.xml',NS,'workbook'),ns=workbook.namespaceURI;
    const wb=children(workbook,['fileVersion','workbookPr','bookViews','sheets','calcPr']);
    for(const c of wb)if(c.localName==='bookViews'){for(const view of children(c,['workbookView']))if(view.children.length)throw fail('unsupported');}else if(c.localName!=='sheets'&&c.children.length)throw fail('unsupported');
    const properties=wb.filter(n=>n.localName==='workbookPr'),sheetContainers=wb.filter(n=>n.localName==='sheets');
    if(properties.length>1||sheetContainers.length!==1)throw fail('format');
    const dateFlag=properties[0]?.getAttribute('date1904')||'0';if(!['0','1','false','true'].includes(dateFlag))throw fail('format');
    const sheets=children(sheetContainers[0],['sheet']);if(!sheets.length||sheets.length>8)throw fail('budget');
    const relations=relationships('xl/_rels/workbook.xml.rels'),targets=new Set();
    for(const r of relations){
      const type=r.getAttribute('Type'),target=r.getAttribute('Target');
      if(!REL.some(p=>['worksheet','sharedStrings','styles','theme'].some(t=>type===p+t)))throw fail('unsupported');
      if(!target||targets.has(target)||target.includes('..')||target.includes('\\')||target.includes(':')||target.includes('?')||target.includes('#'))throw fail('format');targets.add(target);
    }
    const sharedRelations=relations.filter(r=>REL.some(p=>r.getAttribute('Type')===p+'sharedStrings'));
    let strings=[];
    if(sharedRelations.length>1)throw fail('format');
    if(sharedRelations.length){
      if(!['sharedStrings.xml','/xl/sharedStrings.xml'].includes(sharedRelations[0].getAttribute('Target')))throw fail('format');
      mime('xl/sharedStrings.xml','application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml');
      const sst=parse('xl/sharedStrings.xml',ns,'sst'),items=children(sst,['si']);strings=items.map(string);
    } else if(parts['xl/sharedStrings.xml']!==undefined)throw fail('format');
    const names=new Set(),sheetIds=new Set(),used=new Set(),worksheets=[];
    for(const sheet of sheets){
      check();if(sheet.children.length)throw fail('unsupported');const name=decodeXstring(sheet.getAttribute('name')||''),id=sheet.getAttribute('sheetId'),relationship=REL.map(p=>sheet.getAttributeNS(p.slice(0,-1),'id')).find(Boolean);
      if(!name||!name.isWellFormed()||name.length>31||/[\p{Cc}\p{Bidi_Control}]/u.test(name)||names.has(name.toLowerCase())||!id||sheetIds.has(id)||!['','visible'].includes(sheet.getAttribute('state')||''))throw fail('unsupported');
      names.add(name.toLowerCase());sheetIds.add(id);
      const rs=relations.filter(r=>r.getAttribute('Id')===relationship&&REL.some(p=>r.getAttribute('Type')===p+'worksheet'));
      if(rs.length!==1)throw fail('format');
      const target=rs[0].getAttribute('Target'),part=target.startsWith('/xl/')?target.slice(1):'xl/'+target;
      if(!/^xl\/worksheets\/[A-Za-z0-9_-]+\.xml$/.test(part)||used.has(part))throw fail('format');used.add(part);
      mime(part,'application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml');
      const doc=parse(part,ns,'worksheet');
      const content=children(doc,['sheetPr','dimension','sheetViews','sheetFormatPr','cols','sheetData','printOptions','pageMargins','pageSetup','headerFooter']);
      const zeroSize=(node,key)=>node.hasAttribute(key)&&(!Number.isFinite(Number(node.getAttribute(key)))||Number(node.getAttribute(key))<=0);
      const metadata={sheetPr:['tabColor','outlinePr','pageSetUpPr'],sheetViews:['sheetView'],sheetView:['pane','selection'],headerFooter:['oddHeader','oddFooter','evenHeader','evenFooter','firstHeader','firstFooter']};
      const inspectMetadata=node=>{for(const child of children(node,metadata[node.localName]||[]))inspectMetadata(child);};
      for(const item of content)if(!['sheetData','cols'].includes(item.localName))inspectMetadata(item);
      for(const format of content.filter(n=>n.localName==='sheetFormatPr'))if((!['','0','false'].includes(format.getAttribute('zeroHeight')||'')||zeroSize(format,'defaultRowHeight')))throw fail('unsupported');
      for(const cols of content.filter(n=>n.localName==='cols'))for(const col of children(cols,['col']))if(col.children.length||(!['','0','false'].includes(col.getAttribute('hidden')||'')||zeroSize(col,'width')))throw fail('unsupported');
      const data=content.filter(n=>n.localName==='sheetData');if(data.length!==1)throw fail('format');
      let previousRow=0;const output=[];
      for(const row of children(data[0],['row'])){
        const r=row.getAttribute('r');if(!/^[1-9]\d{0,6}$/.test(r)||Number(r)>1048576||Number(r)<=previousRow||(!['','0','false'].includes(row.getAttribute('hidden')||'')||zeroSize(row,'ht')))throw fail('unsupported');previousRow=Number(r);let previousCol=0;
        for(const cell of children(row,['c'])){
          check();if(++cells>2048)throw fail('budget');
          const ref=cell.getAttribute('r'),match=/^([A-Z]{1,3})([1-9]\d{0,6})$/.exec(ref||'');
          if(!match||match[2]!==r)throw fail('format');let col=0;for(const c of match[1])col=col*26+c.charCodeAt(0)-64;
          if(col<=previousCol||col>16384)throw fail('format');previousCol=col;
          const type=cell.getAttribute('t')||'n',values=children(cell,['v','is']);let value;
          if(values.length>1)throw fail('format');
          if(!values.length){if(!['n','inlineStr'].includes(type))throw fail('format');value='';}
          else if(type==='inlineStr'){if(values[0].localName!=='is')throw fail('format');value=string(values[0]);}
          else {
            if(values[0].localName!=='v')throw fail('format');value=plainText(values[0]);
            if(type==='s'){if(!/^(0|[1-9]\d*)$/.test(value)||Number(value)>=strings.length)throw fail('format');value=strings[Number(value)];}
            else if(type==='n'){if(!/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[Ee][+-]?\d+)?$/.test(value))throw fail('format');}
            else if(type==='b'){if(!['0','1'].includes(value))throw fail('format');}
            else if(!['e','d','str'].includes(type))throw fail('unsupported');
          }
          output.push({cell:ref,type:values.length?type:'blank',value});
        }
      }
      worksheets.push({name,cells:output});
    }
    // Every captured worksheet and relationship must belong to this workbook.
    if(relations.filter(r=>REL.some(p=>r.getAttribute('Type')===p+'worksheet')).length!==used.size||Object.keys(parts).filter(p=>p.startsWith('xl/worksheets/')).some(p=>!used.has(p)))throw fail('format');
    const text=JSON.stringify({format:'xlsx-raw-cells-v1',date1904:['1','true'].includes(dateFlag),worksheets})+'\n';
    check();if(!cells)throw fail('empty');if(new TextEncoder().encode(text).length>16384)throw fail('budget');
    return {text,sheets:sheets.length,cells};
  } catch(error){throw error?.spreadsheetInputCode?error:fail(error?.officeInputCode||'format');}
}
