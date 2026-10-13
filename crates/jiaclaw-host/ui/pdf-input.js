// PDF.js 6.4.299 is bundled locally, unchanged from Mozilla's fixed release.
// This interface accepts one already-captured file; it never reads or uploads it.
export async function extractPdfText(bytes, deadline, signal) {
  const fail = code => { const error = new Error(code); error.pdfInputCode = code; return error; };
  const check = () => {
    if (signal.aborted) throw fail('cancelled');
    if (performance.now() >= deadline) throw fail('timeout');
  };
  check();
  if (!(bytes instanceof ArrayBuffer) || !bytes.byteLength || bytes.byteLength > 1048576 ||
      !/^%PDF-(?:1\.[0-7]|2\.0)[\r\n]/.test(new TextDecoder('ascii').decode(bytes.slice(0, 9)))) throw fail('format');
  let port, pdfWorker, loading, timer, abort;
  const stop = () => { port?.terminate(); pdfWorker?.destroy(); loading?.destroy().catch(() => {}); };
  const interrupted = new Promise((_, reject) => {
    abort = () => { stop(); reject(fail('cancelled')); };
    signal.addEventListener('abort', abort, {once:true});
    timer = setTimeout(() => { stop(); reject(fail('timeout')); }, Math.max(0, deadline - performance.now()));
  });
  const read = async () => {
    const pdfjs = await import('./pdfjs/pdf.mjs');
    check();
    if (pdfjs.version !== '6.4.299' || typeof Worker !== 'function') throw fail('unsupported');
    port = new Worker('/ui/pdfjs/pdf.worker.mjs', {type:'module'});
    port.onerror = event => { event.preventDefault(); abort(); };
    pdfWorker = pdfjs.PDFWorker.create({port, verbosity:0});
    // No document URL, rendering, XFA, external fonts/CMaps/WASM, GPU or eval.
    class NoExternalData { async fetch() { throw fail('unsupported'); } }
    loading = pdfjs.getDocument({data:new Uint8Array(bytes), worker:pdfWorker, verbosity:0,
      stopAtErrors:true, enableXfa:false, disableFontFace:true, useSystemFonts:false,
      useWorkerFetch:false, useWasm:false, BinaryDataFactory:NoExternalData,
      disableWorkerRendering:true, isOffscreenCanvasSupported:false,
      isImageDecoderSupported:false, maxImageSize:1, canvasMaxAreaInBytes:0,
      disableRange:true, disableStream:true, disableAutoFetch:true});
    let doc;
    try { doc = await loading.promise; }
    catch (error) { throw fail(error?.name === 'PasswordException' ? 'password' : 'format'); }
    check();
    if (!Number.isInteger(doc.numPages) || doc.numPages < 1 || doc.numPages > 32) throw fail('pages');
    const metadata = await doc.getMetadata(); check();
    const info = metadata?.info;
    if (!info || typeof info.IsXFAPresent !== 'boolean' ||
        !(info.EncryptFilterName === null || typeof info.EncryptFilterName === 'string')) throw fail('format');
    if (info.EncryptFilterName !== null) throw fail('password');
    if (info.IsXFAPresent) throw fail('pages');
    const encoder = new TextEncoder(); let text = '', length = 0;
    const append = value => {
      if (value.length > 16384 - length) throw fail('budget');
      if (!value.isWellFormed() || /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/.test(value)) throw fail('text');
      length += encoder.encode(value).length;
      if (length > 16384) throw fail('budget');
      text += value;
    };
    for (let pageNumber = 1; pageNumber <= doc.numPages; pageNumber++) {
      check();
      const page = await doc.getPage(pageNumber); check();
      append(`[PDF 第 ${pageNumber} 页]\n`);
      const reader = page.streamTextContent({disableNormalization:true}).getReader();
      let hasText = false;
      try {
        while (true) {
          const chunk = await reader.read(); check();
          if (chunk.done) break;
          if (!Array.isArray(chunk.value?.items)) throw fail('text');
          for (const item of chunk.value.items) {
            if (typeof item.str !== 'string' || typeof item.hasEOL !== 'boolean') throw fail('text');
            if (item.str.trim()) hasText = true;
            append(item.str + (item.hasEOL ? '\n' : ' '));
          }
        }
      } finally { reader.cancel().catch(() => {}); reader.releaseLock(); page.cleanup(); }
      if (!hasText) throw fail('empty');
      append('\n');
    }
    check(); return {text, pages:doc.numPages};
  };
  try { return await Promise.race([read(), interrupted]); }
  catch (error) { throw error?.pdfInputCode ? error : fail('format'); }
  finally { clearTimeout(timer); signal.removeEventListener('abort', abort); if (port) port.onerror = null; stop(); }
}
