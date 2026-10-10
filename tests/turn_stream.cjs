// Incremental browser protocol contract, including byte boundaries and failure holds.
const assert = require('assert');
const {Parser,receipt,snapshot,capabilities,catalog} = require('../crates/jiaclaw-host/ui/turn-stream.js');
const id='11111111-1111-4111-8111-111111111111', session_id='http:22222222-2222-4222-8222-222222222222';
const initial={id,session_id,request_hash:'a'.repeat(64),context_hash:'b'.repeat(64),created_ms:1,finished_ms:null,state:'running',session_committed:false,error:null,result:null,result_purged:false,cancel_requested:false,reviewed_ms:null,review_note:null};
const terminal={...initial,finished_ms:2,state:'completed',session_committed:true,result:{reply:'最终🦀回复',status:'completed',routing:null,tool_names:['file_write']}};
const event=value=>Buffer.from(`event: ${value.event}\ndata: ${JSON.stringify(value)}\n\n`);
const values=[{event:'admitted',protocol:1,receipt:initial},{event:'model_started',turn_id:id,operation_id:id,remote_id:id,round:0,model:'fixture'},{event:'preview',round:0,text:'临时🦀预览'}, {event:'model_completed',operation_id:id,round:0},{event:'tool_completed',round:0,tool_call_id:'call-0',tool_name:'file_write'},{event:'done',protocol:1,receipt:terminal}];
const wire=Buffer.concat(values.map(event));
for(const width of [1,2,7,4096,wire.length]){const output=[],p=new Parser({id,session_id},['file_write'],v=>output.push(v));for(let offset=0;offset<wire.length;offset+=width)p.push(wire.subarray(offset,offset+width));p.finish();assert.deepStrictEqual(output,values);}
const longWire=Buffer.concat([event(values[0]),event({...values[5],receipt:{...terminal,result:{...terminal.result,reply:'🦀'.repeat(16384)}}})]);
const tiny=new Parser({id,session_id},[],()=>{});for(let i=0;i<longWire.length;i++)tiny.push(longWire.subarray(i,i+1));tiny.finish();assert(tiny.parts.length===0&&tiny.partial.length===0);
const parsed=(frames,tools=['file_write'])=>{const p=new Parser({id,session_id},tools,()=>{});for(const frame of frames)p.push(frame);p.finish();};
assert.doesNotThrow(()=>parsed([event(values[0]),Buffer.from(': keepalive\n\n'),...values.slice(1).map(event)]));
for(const bad of [wire.subarray(0,wire.length-1),Buffer.concat([wire,event(values[1])]),Buffer.concat([Buffer.from('\xef\xbb\xbf'),wire]),Buffer.from(wire.toString().replace(/\n/g,'\r\n')),Buffer.from('event: admitted\ndata: {}\nretry: 1\n\n'),Buffer.from([0xff]),event({...values[0],receipt:{...initial,id:session_id.slice(5)}}),Buffer.concat([event(values[0]),event({...values[1],turn_id:session_id.slice(5)})]),Buffer.concat([event(values[0]),event({...values[1],round:1})]),Buffer.concat([event(values[0]),event(values[1]),event({...values[2],text:'x'.repeat(1025)})]),Buffer.concat(values.map(v=>event(v.event==='done'?{...v,receipt:initial}:v)))])assert.throws(()=>parsed([bad]));
assert.throws(()=>parsed(values.map(event),['datetime_now']));
const roundOverflow=[event(values[0]),event(values[1]),...Array.from({length:2049},()=>event({...values[2],text:'x'.repeat(1024)}))];assert.throws(()=>parsed(roundOverflow));
const p=new Parser({id,session_id},[],()=>{});assert.throws(()=>p.push(Buffer.alloc(12*1024*1024+1)));
const big=new Parser({id,session_id},[],()=>{});assert.throws(()=>big.push(Buffer.from('event: done\ndata: '+ 'x'.repeat(2*1024*1024+20000))));
for(const change of [{id:session_id.slice(5)},{session_id:'other'},{request_hash:'z'.repeat(64)},{state:'completed'},{session_committed:1},{finished_ms:0},{reviewed_ms:3},{cancel_requested:null},{result_purged:true,result:terminal.result}])assert.throws(()=>receipt({...initial,...change},initial));
assert.throws(()=>snapshot({protocol:1,receipt:terminal},initial));
// Persistence timestamps use wall clock, while execution budgets use monotonic time.
// A clock correction must not turn an actually committed result into a false hold.
assert.doesNotThrow(()=>receipt({...terminal,created_ms:100,finished_ms:90}));
assert.doesNotThrow(()=>receipt({...terminal,state:'needs_review',error:'tool_failure'},initial));
const c={protocol:1,enabled:true,streaming:true,stream_suffix:'/stream',max_active:1,turn_budget_secs:10,max_identities:10000,max_retained_results:32,session_prefix:'http:',max_stream_wire_bytes:12*1024*1024,max_preview_round_bytes:2*1024*1024,max_preview_total_bytes:8*1024*1024};
assert.deepStrictEqual(capabilities(c),c);
assert.doesNotThrow(()=>capabilities({...c,enabled:false,streaming:false,turn_budget_secs:0}));
for(const change of [{protocol:2},{enabled:false},{turn_budget_secs:301},{stream_suffix:'https://other/'},{max_stream_wire_bytes:Infinity}])assert.throws(()=>capabilities({...c,...change}));
console.log('PASS browser SSE parser: split UTF-8, strict identity/order/terminal, bounded wire/frame/preview, authorization and capabilities');

const summary=Object.fromEntries(['id','session_id','created_ms','finished_ms','state','session_committed','cancel_requested','result_purged','reviewed_ms'].map(k=>[k,initial[k]]));
const discovery={protocol:1,state:'unresolved',limit:5,offset:0,has_more:false,turns:[summary]};
const expected={state:'unresolved',limit:5,offset:0};
assert.doesNotThrow(()=>catalog(discovery,expected));
assert.doesNotThrow(()=>catalog({...discovery,state:'all',turns:[{...summary,created_ms:100,finished_ms:1,state:'completed',session_committed:true}]},{...expected,state:'all'}));
for(const bad of [{...discovery,offset:1},{...discovery,limit:51},{...discovery,state:'all'},{...discovery,has_more:true},{...discovery,turns:[summary,summary]}, {...discovery,turns:[{...summary,result:{reply:'private'}}]}, {...discovery,turns:[{...summary,id:'<img src=x>'}]}, {...discovery,turns:[{...summary,finished_ms:1}]}, {...discovery,turns:[{...summary,reviewed_ms:2}]}, {...discovery,turns:[{...summary,created_ms:-1}]}, {...discovery,turns:[{...summary,state:'needs_review',finished_ms:2,reviewed_ms:3}]}])assert.throws(()=>catalog(bad,expected));
const second={...summary,id:session_id.slice(5)};
assert.throws(()=>catalog({...discovery,turns:[summary,second]},expected));
assert.doesNotThrow(()=>catalog({...discovery,turns:[second,summary]},expected));
console.log('PASS parser 7: finite payload-free catalog, exact identity/state/query, tied ordering, no result exposure or summary authority');

const gateway={protocol:1,gateway_protocol:1,enabled:true,streaming:false,listing:true,scope:'gateway',session_prefix:'http:',max_identities:10000,max_page:50,enabled_tools:['datetime_now','json_query']};
assert.doesNotThrow(()=>require('../crates/jiaclaw-host/ui/turn-stream.js').gatewayCapabilities(gateway));
const {gatewayCapabilities,gatewayCatalog,gatewaySnapshot}=require('../crates/jiaclaw-host/ui/turn-stream.js');
for(const change of [{scope:'backend'},{gateway_protocol:2},{streaming:true},{enabled:false},{enabled_tools:['datetime_now','datetime_now']},{enabled_tools:['file_write','datetime_now']},{max_page:51},{extra:true}])assert.throws(()=>gatewayCapabilities({...gateway,...change}));
assert.throws(()=>gatewayCapabilities(c));assert.throws(()=>capabilities(gateway));
const admission={id,session_id,created_ms:1}, admissions={protocol:1,scope:'gateway',limit:5,offset:0,has_more:false,requests:[admission]};
assert.doesNotThrow(()=>gatewayCatalog(admissions,{limit:5,offset:0}));
for(const change of [{scope:'backend'},{offset:1},{limit:51},{has_more:true},{requests:[admission,admission]},{requests:[{...admission,state:'completed'}]},{requests:[{...admission,result:'private'}]},{requests:[{...admission,session_id:'legacy'}]},{requests:[{...admission,created_ms:-1}]},{requests:[{...admission,created_ms:1.2}]},{state:'all'}])assert.throws(()=>gatewayCatalog({...admissions,...change},{limit:5,offset:0}));
assert.throws(()=>gatewayCatalog(discovery,{limit:5,offset:0}));
const metadata2={...admission,id:session_id.slice(5)};assert.throws(()=>gatewayCatalog({...admissions,requests:[admission,metadata2]},{limit:5,offset:0}));assert.doesNotThrow(()=>gatewayCatalog({...admissions,requests:[metadata2,admission]},{limit:5,offset:0}));
const ownResult={protocol:1,active:false,receipt:{...terminal,result:{...terminal.result,tool_names:['datetime_now']}}};
assert.doesNotThrow(()=>gatewaySnapshot(ownResult,initial));
assert.throws(()=>gatewaySnapshot({...ownResult,receipt:{...ownResult.receipt,session_id:'http:'+id}},initial));
assert.throws(()=>gatewaySnapshot({...ownResult,receipt:terminal},initial));
assert.throws(()=>gatewaySnapshot({...ownResult,state:'completed'},initial));
console.log('PASS parser 8: pinned tenant JSON scope, finite payload-free admissions, original receipt/session and fixed tool authority');

const streamGateway={...gateway,gateway_protocol:2,streaming:true,stream_suffix:'/stream',turn_budget_secs:15,max_stream_wire_bytes:12*1024*1024,max_preview_round_bytes:2*1024*1024,max_preview_total_bytes:8*1024*1024};
assert.doesNotThrow(()=>gatewayCapabilities(streamGateway));
for(const change of [{streaming:false},{gateway_protocol:1},{turn_budget_secs:301},{turn_budget_secs:9},{stream_suffix:'/anything'},{max_stream_wire_bytes:Infinity},{max_preview_round_bytes:3},{max_preview_total_bytes:0}])assert.throws(()=>gatewayCapabilities({...streamGateway,...change}));
console.log('PASS parser 9: explicit paired tenant SSE capability budgets; existing JSON workbench remains usable');
