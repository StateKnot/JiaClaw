import {extractOfficeParts} from '/ui/office-input.js';
// Slide-owned text only. Neither a renderer nor a document-code interpreter.
const fail=code=>Object.assign(new Error(code),{presentationInputCode:code});
const P=['http://schemas.openxmlformats.org/presentationml/2006/main','http://purl.oclc.org/ooxml/presentationml/main'];
const A=['http://schemas.openxmlformats.org/drawingml/2006/main','http://purl.oclc.org/ooxml/drawingml/main'];
const R=['http://schemas.openxmlformats.org/officeDocument/2006/relationships/','http://purl.oclc.org/ooxml/officeDocument/relationships/'];
const BODY_ATTRIBUTES={presentation:['serverZoom','firstSlideNum','showSpecialPlsOnTitleSld','rtl','removePersonalInfoOnSave','compatMode','strictFirstAndLastChars','embedTrueTypeFonts','saveSubsetFonts','autoCompressPictures','bookmarkIdSeed','conformance'],bodyPr:['rot','spcFirstLastPara','vertOverflow','horzOverflow','vert','wrap','lIns','tIns','rIns','bIns','numCol','spcCol','rtlCol','fromWord','anchor','anchorCtr','forceAA','compatLnSpc','upright'],sldIdLst:[],sld:['show','showMasterSp','showMasterPhAnim'],cSld:['name'],spTree:[],sp:['useBgFill'],grpSp:[],nvSpPr:[],nvGrpSpPr:[],cNvPr:['id','name','descr','hidden','title'],cNvSpPr:['txBox'],cNvGrpSpPr:[],nvPr:['isPhoto','userDrawn'],txBody:[],p:[],r:[],br:[],t:[],sldId:['id']};
export async function extractPresentationText(bytes,deadline,signal) {
  const check=()=>{if(signal.aborted)throw fail('cancelled');if(performance.now()>=deadline)throw fail('timeout');};
  try {
    const parts=await extractOfficeParts(bytes,deadline,signal,'pptx');check();let nodes=0;
    const parse=(name,namespaces,root)=>{
      const source=parts?.[name];
      if(typeof source!=='string'||!source.isWellFormed()||/<!\s*(DOCTYPE|ENTITY)/i.test(source))throw fail('format');
      const doc=new DOMParser().parseFromString(source,'application/xml');check();
      if(doc.getElementsByTagName('parsererror').length||![namespaces].flat().includes(doc.documentElement.namespaceURI)||doc.documentElement.localName!==root)throw fail('format');
      const namespace=doc.documentElement.namespaceURI,profile=P.indexOf(namespace);
      const allowed=profile<0?[namespace]:[namespace,A[profile]],stack=[doc.documentElement];
      while(stack.length){check();const n=stack.pop();if(++nodes>50000)throw fail('budget');
        if(!allowed.includes(n.namespaceURI))throw fail('unsupported');
        if(n.localName!=='t'&&[...n.childNodes].some(c=>[3,4].includes(c.nodeType)&&c.textContent.trim()))throw fail('unsupported');
        for(const a of n.attributes)if(a.namespaceURI&&!['http://www.w3.org/2000/xmlns/','http://www.w3.org/XML/1998/namespace',...R.map(r=>r.slice(0,-1))].includes(a.namespaceURI))throw fail('unsupported');

        const fields=BODY_ATTRIBUTES[n.localName];
        if(fields)for(const attr of n.attributes){
          if(attr.namespaceURI==='http://www.w3.org/2000/xmlns/')continue;
          if(n.localName==='t'&&attr.namespaceURI==='http://www.w3.org/XML/1998/namespace'&&attr.localName==='space'&&['default','preserve'].includes(attr.value))continue;
          if(n.localName==='sldId'&&R.includes(attr.namespaceURI+'/')&&attr.localName==='id')continue;
          if(attr.namespaceURI||!fields.includes(attr.localName))throw fail('unsupported');
        }
        for(const c of n.children)stack.push(c);
      }
      return doc.documentElement;
    };
    const children=(node,allowed)=>{const rows=[...node.children];if(rows.some(n=>!allowed.includes(n.namespaceURI+'|'+n.localName)))throw fail('unsupported');return rows;};
    const same=(node,tags)=>children(node,tags.map(t=>node.namespaceURI+'|'+t));
    const leaf=node=>{if(node.children.length)throw fail('unsupported');};
    const flag=(node,name,defaultValue)=>{const v=node.getAttribute(name);if(v===null)return defaultValue;if(!['0','1','false','true'].includes(v))throw fail('format');return ['1','true'].includes(v);};
    const metadataTags=new Set(('sldMasterIdLst sldMasterId notesMasterIdLst notesMasterId sldSz notesSz defaultTextStyle clrMapOvr masterClrMapping overrideClrMapping nvGrpSpPr cNvPr cNvGrpSpPr nvPr ph grpSpPr nvSpPr cNvSpPr spLocks grpSpLocks spPr style bodyPr lstStyle pPr rPr endParaRPr defPPr defRPr lvl1pPr lvl2pPr lvl3pPr lvl4pPr lvl5pPr lvl6pPr lvl7pPr lvl8pPr lvl9pPr lnSpc spcPct spcPts spcBef spcAft buClrTx buClr buSzTx buSzPct buSzPts buFontTx buFont buNone buAutoNum buChar tabLst tab noAutofit normAutofit spAutoFit xfrm off ext chOff chExt prstGeom avLst gd noFill solidFill gradFill gsLst gs srgbClr schemeClr sysClr prstClr hslClr scrgbClr alpha tint shade lumMod lumOff satMod satOff hueMod hueOff redMod greenMod blueMod lin path fillToRect tileRect ln prstDash custDash ds round bevel miter headEnd tailEnd effectLst outerShdw innerShdw glow softEdge reflection blur fillRef lnRef effectRef fontRef highlight uFill uFillTx uLn uLnTx latin ea cs sym').split(' '));
    const presentationMetadata=new Set('sldMasterIdLst sldMasterId notesMasterIdLst notesMasterId sldSz notesSz defaultTextStyle clrMapOvr nvGrpSpPr cNvPr cNvGrpSpPr nvPr ph grpSpPr nvSpPr cNvSpPr spPr style'.split(' '));
    const metadata=node=>{
      const stack=[node];while(stack.length){check();const n=stack.pop();
        if(!metadataTags.has(n.localName)||n.namespaceURI!==(presentationMetadata.has(n.localName)?p:a)||flag(n,'hidden',false))throw fail('unsupported');
        for(const c of n.children)stack.push(c);
      }
    };
    const relType=(row,kind)=>R.some(p=>row.getAttribute('Type')===p+kind);
    const relations=name=>{
      const rows=same(parse(name,'http://schemas.openxmlformats.org/package/2006/relationships','Relationships'),['Relationship']),ids=new Set();
      for(const r of rows){leaf(r);const id=r.getAttribute('Id'),target=r.getAttribute('Target');
        if(!id||ids.has(id)||!target||!['','Internal'].includes(r.getAttribute('TargetMode')||'')||/[\\:?#\p{Cc}]/u.test(target))throw fail('unsupported');ids.add(id);
      }
      return rows;
    };
    const types=same(parse('[Content_Types].xml','http://schemas.openxmlformats.org/package/2006/content-types','Types'),['Default','Override']);types.forEach(leaf);
    const mime=(name,expected)=>{const rows=types.filter(t=>t.localName==='Override'&&t.getAttribute('PartName')==='/'+name);if(rows.length!==1||rows[0].getAttribute('ContentType')!==expected)throw fail('format');};
    mime('ppt/presentation.xml','application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml');
    const roots=relations('_rels/.rels').filter(r=>relType(r,'officeDocument'));
    if(roots.length!==1||!['ppt/presentation.xml','/ppt/presentation.xml'].includes(roots[0].getAttribute('Target')))throw fail('format');
    const doc=parse('ppt/presentation.xml',P,'presentation'),p=doc.namespaceURI,a=A[P.indexOf(p)];
    const content=same(doc,['sldMasterIdLst','notesMasterIdLst','sldIdLst','sldSz','notesSz','defaultTextStyle']);
    const lists=content.filter(n=>n.localName==='sldIdLst');if(lists.length!==1)throw fail('format');
    content.filter(n=>n.localName!=='sldIdLst').forEach(metadata);
    const ids=same(lists[0],['sldId']);if(!ids.length||ids.length>32)throw fail('budget');
    const main=relations('ppt/_rels/presentation.xml.rels');
    if(main.some(r=>!['slide','slideMaster','notesMaster','theme','presProps','viewProps','tableStyles','printerSettings'].some(k=>relType(r,k))))throw fail('unsupported');
    const used=new Set(),slideIds=new Set(),slides=[];let textBytes=0;
    for(const [index,id]of ids.entries()){
      check();leaf(id);const key=id.getAttribute('id'),rid=id.getAttributeNS(R[P.indexOf(p)].slice(0,-1),'id');
      if(!/^[1-9]\d*$/.test(key||'')||slideIds.has(key))throw fail('format');slideIds.add(key);
      const rs=main.filter(r=>r.getAttribute('Id')===rid&&relType(r,'slide'));if(rs.length!==1)throw fail('format');
      const target=rs[0].getAttribute('Target'),part=target.startsWith('/ppt/')?target.slice(1):'ppt/'+target;
      if(!/^ppt\/slides\/[A-Za-z0-9_-]+\.xml$/.test(part)||used.has(part))throw fail('format');used.add(part);
      mime(part,'application/vnd.openxmlformats-officedocument.presentationml.slide+xml');
      const slide=parse(part,p,'sld');if(!flag(slide,'show',true))throw fail('unsupported');
      const s=same(slide,['cSld','clrMapOvr']),cs=s.filter(n=>n.localName==='cSld');if(cs.length!==1)throw fail('format');
      s.filter(n=>n.localName!=='cSld').forEach(metadata);
      const csRows=same(cs[0],['spTree']),trees=csRows.filter(n=>n.localName==='spTree');if(trees.length!==1)throw fail('format');
      const slideRels=part.replace('/slides/','/slides/_rels/')+'.rels';
      if(parts[slideRels]!==undefined)for(const r of relations(slideRels)){
        const t=r.getAttribute('Target');
        if(!(relType(r,'slideLayout')&&/^\.\.\/slideLayouts\/[A-Za-z0-9_-]+\.xml$/.test(t))&&!(relType(r,'notesSlide')&&/^\.\.\/notesSlides\/[A-Za-z0-9_-]+\.xml$/.test(t)))throw fail('unsupported');
      }
      let text='';
      const append=value=>{if(!value.isWellFormed()||/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/.test(value))throw fail('text');textBytes+=new TextEncoder().encode(value).length;if(textBytes>16384)throw fail('budget');text+=value;};
      const paragraph=node=>{
        const rows=children(node,['pPr','r','br','endParaRPr'].map(t=>a+'|'+t));
        if(rows.filter(n=>n.localName==='pPr').length>1||rows.filter(n=>n.localName==='endParaRPr').length>1||rows.some((n,i)=>n.localName==='pPr'&&i!==0||n.localName==='endParaRPr'&&i!==rows.length-1))throw fail('format');
        for(const c of rows){
          if(c.localName==='r'){
            const runs=same(c,['rPr','t']),texts=runs.filter(n=>n.localName==='t');if(texts.length!==1||runs.filter(n=>n.localName==='rPr').length>1||runs.at(-1)!==texts[0])throw fail('format');leaf(texts[0]);append(texts[0].textContent);
            runs.filter(n=>n.localName==='rPr').forEach(metadata);
          }else if(c.localName==='br'){const props=same(c,['rPr']);if(props.length>1)throw new Error('format');props.forEach(metadata);append('\n');}
          else metadata(c);
        }
        append('\n');
      };
      const shapeTree=tree=>{
        const shapes=same(tree,['nvGrpSpPr','grpSpPr','sp','grpSp']);
        if(shapes.filter(n=>n.localName==='nvGrpSpPr').length!==1||shapes.filter(n=>n.localName==='grpSpPr').length!==1)throw fail('format');
        for(const shape of shapes){
          if(shape.localName==='grpSp'){shapeTree(shape);continue;}
          if(shape.localName!=='sp'){metadata(shape);continue;}
          const pieces=same(shape,['nvSpPr','spPr','style','txBody']);
          if(pieces.filter(n=>n.localName==='spPr').length!==1||pieces.filter(n=>n.localName==='style').length>1)throw fail('format');
          const nv=pieces.filter(n=>n.localName==='nvSpPr');if(nv.length!==1)throw fail('format');
          const nvRows=same(nv[0],['cNvPr','cNvSpPr','nvPr']),properties=nvRows.filter(n=>n.localName==='cNvPr');if(properties.length!==1)throw fail('format');
          if(nvRows.filter(n=>n.localName==='cNvSpPr').length!==1||nvRows.filter(n=>n.localName==='nvPr').length!==1)throw fail('format');
          if(flag(properties[0],'hidden',false)||properties[0].children.length)throw fail('unsupported');
          pieces.filter(n=>n.localName!=='txBody').forEach(metadata);
          const bodies=pieces.filter(n=>n.localName==='txBody');if(bodies.length>1)throw fail('format');
          for(const body of bodies){const bodyRows=children(body,['bodyPr','lstStyle','p'].map(t=>a+'|'+t));if(bodyRows.filter(n=>n.localName==='bodyPr').length!==1||bodyRows.filter(n=>n.localName==='lstStyle').length>1||!bodyRows.some(n=>n.localName==='p'))throw fail('format');for(const n of bodyRows)if(n.localName==='p')paragraph(n);else metadata(n);}
        }
      };
      shapeTree(trees[0]);if(!text.trim())throw fail('empty');slides.push({slide:index+1,text});
    }
    if(main.filter(r=>relType(r,'slide')).length!==used.size||Object.keys(parts).filter(n=>/^ppt\/slides\/[^/]+\.xml$/.test(n)).some(n=>!used.has(n)))throw fail('format');
    const text=JSON.stringify({format:'pptx-slide-text-v1',slides})+'\n';check();if(new TextEncoder().encode(text).length>16384)throw fail('budget');return{text,slides:slides.length};
  }catch(error){throw error?.presentationInputCode?error:fail(error?.officeInputCode||'format');}
}
