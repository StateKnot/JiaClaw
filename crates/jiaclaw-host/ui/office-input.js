// Shared owner for an already-captured Office package. No additional file read.
const fail=code=>Object.assign(new Error(code),{officeInputCode:code});
export async function extractOfficeParts(bytes,deadline,signal,format) {
  const check=()=>{if(signal.aborted)throw fail('cancelled');if(performance.now()>=deadline)throw fail('timeout');};
  check();
  if(!(bytes instanceof ArrayBuffer)||!bytes.byteLength||bytes.byteLength>1048576||!['word','xlsx','pptx'].includes(format))throw fail('format');
  if(typeof Worker!=='function'||typeof DecompressionStream!=='function')throw fail('unsupported');
  let worker,timer,abort;
  try {
    const parts=await new Promise((resolve,reject)=>{
      abort=()=>reject(fail('cancelled'));signal.addEventListener('abort',abort,{once:true});
      timer=setTimeout(()=>reject(fail('timeout')),Math.max(0,deadline-performance.now()));
      worker=new Worker('/ui/word-worker.js');
      worker.onerror=event=>{event.preventDefault();reject(fail('format'));};
      worker.onmessage=event=>event.data?.error?reject(fail('format')):resolve(event.data?.parts);
      worker.postMessage(format==='word'?bytes:{format,bytes},[bytes]);
    });
    check();return parts;
  } finally {clearTimeout(timer);signal.removeEventListener('abort',abort);worker?.terminate();}
}
