import {extractOfficeParts} from '/ui/office-input.js';
// One already-captured local DOCX. No upload, rendering or external resolution.
const fail=code=>{const e=new Error(code);e.wordInputCode=code;return e;};
export async function extractWordText(bytes,deadline,signal) {
  const check=()=>{if(signal.aborted)throw fail('cancelled');if(performance.now()>=deadline)throw fail('timeout');};
  check();
  if(!(bytes instanceof ArrayBuffer)||!bytes.byteLength||bytes.byteLength>1048576)throw fail('format');
  if(typeof Worker!=='function'||typeof DecompressionStream!=='function')throw fail('unsupported');
  try {
    const parts=await extractOfficeParts(bytes,deadline,signal,'word');
    check();
    const parse=(name,ns,root)=>{
      const source=parts?.[name];
      if(typeof source!=='string'||!source.isWellFormed()||/<!\s*(DOCTYPE|ENTITY)/i.test(source))throw fail('format');
      const doc=new DOMParser().parseFromString(source,'application/xml');check();
      if(doc.getElementsByTagName('parsererror').length || ![ns].flat().includes(doc.documentElement.namespaceURI) || doc.documentElement.localName!==root)throw fail('format');
      return doc;
    };
    const types=parse('[Content_Types].xml','http://schemas.openxmlformats.org/package/2006/content-types','Types');
    const matches=[...types.documentElement.children].filter(e=>e.namespaceURI===types.documentElement.namespaceURI&&e.localName==='Override'&&e.getAttribute('PartName')==='/word/document.xml');
    if(matches.length!==1||matches[0].getAttribute('ContentType')!=='application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml')throw fail('format');
    const rel=parse('_rels/.rels','http://schemas.openxmlformats.org/package/2006/relationships','Relationships');
    const main=[...rel.documentElement.children].filter(e=>e.namespaceURI===rel.documentElement.namespaceURI&&e.localName==='Relationship'&&['http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument','http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument'].includes(e.getAttribute('Type')));
    if(main.length!==1||!['word/document.xml','/word/document.xml'].includes(main[0].getAttribute('Target'))||!['','Internal'].includes(main[0].getAttribute('TargetMode')||''))throw fail('format');
    const doc=parse('word/document.xml',['http://schemas.openxmlformats.org/wordprocessingml/2006/main','http://purl.oclc.org/ooxml/wordprocessingml/main'],'document'), ns=doc.documentElement.namespaceURI;
    const bodies=[...doc.documentElement.children].filter(e=>e.namespaceURI===ns&&e.localName==='body');
    if(bodies.length!==1)throw fail('format');
    let text='',length=0,nodes=0;const encoder=new TextEncoder(),stack=[[bodies[0],false]];
    const append=value=>{if(!value.isWellFormed()||/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/.test(value))throw fail('text');length+=encoder.encode(value).length;if(length>16384)throw fail('budget');text+=value;};
    while(stack.length) {
      check();const [node,exit]=stack.pop();if(++nodes>50000)throw fail('format');
      if(node.namespaceURI===ns) {
        const tag=node.localName;
        // Revision marks also live in properties, cells and range boundaries.
        // Refuse the whole body instead of choosing an implicit revision view.
        if(/Change$|^(?:ins|del|delText|delInstrText|cellIns|cellDel|cellMerge|moveFrom(?:RangeStart|RangeEnd)?|moveTo(?:RangeStart|RangeEnd)?|customXml(?:Ins|Del|MoveFrom|MoveTo)Range(?:Start|End))$/.test(tag)||['altChunk','drawing','pict','object','txbxContent'].includes(tag))throw fail('unsupported');
        if(exit) {if(tag==='p')append('\n');continue;}
        if(tag==='t') {if(node.children.length)throw fail('format');append(node.textContent);continue;}
        if(tag==='tab') {append('\t');continue;}
        if(tag==='br'||tag==='cr') {append('\n');continue;}
        if(tag==='instrText')continue;
        stack.push([node,true]);
      } else if(node!==bodies[0])throw fail('unsupported');
      for(let i=node.children.length-1;i>=0;i--)stack.push([node.children[i],false]);
    }
    check();if(!text.trim())throw fail('empty');return {text};
  } catch(error) {throw error?.wordInputCode?error:fail(error?.officeInputCode||'format');}
}
