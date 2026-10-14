'use strict';
// Same-origin server protocol. Preview never authorizes tools or proves a commit.
// Pure parser/validators are shared with Node contract tests; no persistent state.
(() => {
  const MiB = 1024 * 1024, encoder = new TextEncoder();
  const bytes = value => encoder.encode(value).length;
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
  const anyUuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
  const hash = /^[0-9a-f]{64}$/;
  const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
  function require(condition) { if (!condition) throw new Error('流式协议或原编号不匹配；请核对原请求，勿重复执行。'); }
  const text = (value, maximum) => typeof value === 'string' && bytes(value) <= maximum;
  const feedbackNote = value => text(value,1024) && !/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/u.test(value) && new TextDecoder('utf-8',{fatal:true,ignoreBOM:true}).decode(encoder.encode(value)) === value;
  function capabilities(c) {
    require(object(c) && c.protocol === 1 && typeof c.enabled === 'boolean' && typeof c.streaming === 'boolean' && c.streaming === c.enabled);
    require(c.session_prefix === 'http:' && c.stream_suffix === '/stream' && c.max_active === 1 && c.max_identities === 10000 && c.max_retained_results === 32);
    require(!c.streaming || (Number.isInteger(c.turn_budget_secs) && c.turn_budget_secs >= 1 && c.turn_budget_secs <= 300));
    require(c.max_stream_wire_bytes === 12 * MiB && c.max_preview_round_bytes === 2 * MiB && c.max_preview_total_bytes === 8 * MiB);
    require(c.listing === undefined || typeof c.listing === 'boolean');
    require(c.feedback_protocol === undefined || [0,1].includes(c.feedback_protocol));
    return c;
  }
  function canonicalDocumentPaths(paths) {
    let previous = null;
    for (const path of paths) {
      require(typeof path === 'string' && !/[\\\u0000-\u001f\u007f-\u009f\ud800-\udfff]/u.test(path) && bytes(path) <= 256 && path.startsWith('tenant-documents/'));
      const parts = path.split('/');
      require(parts.length <= 16 && parts.every(p => p && p.trim() === p && !p.startsWith('.') && !p.startsWith('\ufeff') && !p.endsWith('\ufeff')));
      const encoded = encoder.encode(path);
      if (previous) {
        let i = 0; while (i < Math.min(previous.length, encoded.length) && previous[i] === encoded[i]) i++;
        require(i === previous.length ? i < encoded.length : i < encoded.length && previous[i] < encoded[i]);
      }
      previous = encoded;
    }
  }
  function gatewayCapabilities(c) {
    const fields = ['protocol','gateway_protocol','enabled','streaming','listing','scope','session_prefix','max_identities','max_page','enabled_tools'];
    if (c?.gateway_protocol === 2) fields.push('stream_suffix','turn_budget_secs','max_stream_wire_bytes','max_preview_round_bytes','max_preview_total_bytes');
    if (Object.hasOwn(c || {}, 'file_read')) fields.push('file_read');
    if (Object.hasOwn(c || {}, 'file_download_protocol')) fields.push('file_download_protocol');
    if (Object.hasOwn(c || {}, 'file_write')) fields.push('file_write');
    if (Object.hasOwn(c || {}, 'skill_read')) fields.push('skill_read');
    if (Object.hasOwn(c || {}, 'memory_search')) fields.push('memory_search');
    if (Object.hasOwn(c || {}, 'tree')) fields.push('tree');
    if (Object.hasOwn(c || {}, 'feedback_protocol')) fields.push('feedback_protocol');
    require(object(c) && Object.keys(c).length === fields.length && fields.every(k => Object.hasOwn(c,k)));
    require(c.protocol === 1 && [1,2].includes(c.gateway_protocol) && c.scope === 'gateway' && c.enabled === true && c.streaming === (c.gateway_protocol === 2) && c.listing === true);
    require(c.session_prefix === 'http:' && c.max_identities === 10000 && c.max_page === 50);
    require(c.feedback_protocol === undefined || c.feedback_protocol === 1);
    if (c.gateway_protocol === 2) require(c.stream_suffix === '/stream' && Number.isInteger(c.turn_budget_secs) && c.turn_budget_secs >= 10 && c.turn_budget_secs <= 300 && c.max_stream_wire_bytes === 12 * MiB && c.max_preview_round_bytes === 2 * MiB && c.max_preview_total_bytes === 8 * MiB);
    require(c.file_download_protocol === undefined || (c.file_download_protocol === 1 && c.file_read !== undefined));
    const allowed = ['datetime_now','json_query'];
    if (c.file_read !== undefined) {
      const f = c.file_read;
      require(c.gateway_protocol === 2 && object(f) && Object.keys(f).length === 4 && ['protocol','paths','max_bytes','policy_sha256'].every(k => Object.hasOwn(f,k)));
      require(f.protocol === 1 && f.max_bytes === 262144 && typeof f.policy_sha256 === 'string' && hash.test(f.policy_sha256) && Array.isArray(f.paths) && f.paths.length >= 1 && f.paths.length <= 32);
      canonicalDocumentPaths(f.paths);
      allowed.push('file_read');
    }
    if (c.file_write !== undefined) {
      const f = c.file_write;
      require(c.gateway_protocol === 2 && object(f) && Object.keys(f).length === 5 && ['protocol','paths','mode','max_bytes','policy_sha256'].every(k => Object.hasOwn(f,k)));
      require(f.protocol === 1 && f.mode === 'overwrite' && f.max_bytes === 262144 && typeof f.policy_sha256 === 'string' && hash.test(f.policy_sha256) && Array.isArray(f.paths) && f.paths.length >= 1 && f.paths.length <= 32);
      canonicalDocumentPaths(f.paths);
      allowed.push('file_write');
    }
    if (c.skill_read !== undefined) {
      const f = c.skill_read;
      require(c.gateway_protocol === 2 && object(f) && Object.keys(f).length === 5 && ['protocol','skills','max_body_bytes','max_result_bytes','policy_sha256'].every(k => Object.hasOwn(f,k)));
      require(f.protocol === 1 && f.max_body_bytes === 131072 && f.max_result_bytes === 262144 && typeof f.policy_sha256 === 'string' && hash.test(f.policy_sha256) && Array.isArray(f.skills) && f.skills.length >= 1 && f.skills.length <= 16);
      let previous = null;
      for (const entry of f.skills) {
        require(object(entry) && Object.keys(entry).length === 2 && ['name','content_sha256'].every(k => Object.hasOwn(entry,k)));
        require(typeof entry.name === 'string' && entry.name.length > 0 && bytes(entry.name) <= 128 && entry.name.trim() === entry.name && !/[\u0000-\u001f\u007f-\u009f\ud800-\udfff]/u.test(entry.name) && typeof entry.content_sha256 === 'string' && hash.test(entry.content_sha256));
        const encoded = encoder.encode(entry.name);
        if (previous) {
          let i = 0; while (i < Math.min(previous.length, encoded.length) && previous[i] === encoded[i]) i++;
          require(i === previous.length ? i < encoded.length : i < encoded.length && previous[i] < encoded[i]);
        }
        previous = encoded;
      }
      allowed.push('skill_read');
    }
    if (c.memory_search !== undefined) {
      const f = c.memory_search;
      const keys = ['protocol','paths','mode','max_source_bytes','max_total_bytes','max_query_bytes','max_results','max_excerpt_bytes','max_result_bytes','policy_sha256'];
      require(c.gateway_protocol === 2 && object(f) && Object.keys(f).length === keys.length && keys.every(k => Object.hasOwn(f,k)));
      require(f.protocol === 1 && f.mode === 'keyword' && f.max_source_bytes === 524288 && f.max_total_bytes === 1572864 && f.max_query_bytes === 1024 && f.max_results === 20 && f.max_excerpt_bytes === 1024 && f.max_result_bytes === 65536);
      require(typeof f.policy_sha256 === 'string' && hash.test(f.policy_sha256) && Array.isArray(f.paths) && f.paths.length >= 1 && f.paths.length <= 3);
      let previous = null;
      for (const path of f.paths) {
        require(typeof path === 'string' && bytes(path) <= 256 && !/[\\\u0000-\u001f\u007f-\u009f\ud800-\udfff]/u.test(path));
        const parts = path.split('/');
        require(parts.length <= 16 && parts.every(p => p && p.trim() === p && !p.startsWith('.')));
        const encoded = encoder.encode(path);
        if (previous) {
          let i = 0; while (i < Math.min(previous.length, encoded.length) && previous[i] === encoded[i]) i++;
          require(i === previous.length ? i < encoded.length : i < encoded.length && previous[i] < encoded[i]);
        }
        previous = encoded;
      }
      allowed.push('memory_search');
    }
    if (c.tree !== undefined) {
      const f = c.tree, keys = ['protocol','roots','max_depth','max_entries','max_scanned','scan_budget_ms','max_result_bytes','policy_sha256'];
      require(c.gateway_protocol === 2 && object(f) && Object.keys(f).length === keys.length && keys.every(k => Object.hasOwn(f,k)));
      require(f.protocol === 1 && f.max_depth === 32 && f.max_entries === 1000 && f.max_scanned === 2000 && f.scan_budget_ms === 2000 && f.max_result_bytes === 65536);
      require(typeof f.policy_sha256 === 'string' && hash.test(f.policy_sha256) && Array.isArray(f.roots) && f.roots.length >= 1 && f.roots.length <= 8);
      canonicalDocumentPaths(f.roots);
      require(f.roots.every((r,i) => f.roots.slice(0,i).every(p => !r.startsWith(p+'/'))));
      allowed.push('tree');
    }
    require(Array.isArray(c.enabled_tools) && c.enabled_tools.length === allowed.length && new Set(c.enabled_tools).size === allowed.length && c.enabled_tools.every(t => allowed.includes(t)));
    return c;
  }
  function gatewayCatalog(value, expected) {
    const fields = ['protocol','scope','requests','limit','offset','has_more'];
    require(object(value) && Object.keys(value).length === fields.length && fields.every(k => Object.hasOwn(value,k)));
    require(value.protocol === 1 && value.scope === 'gateway' && value.limit === expected.limit && value.offset === expected.offset);
    require(Number.isInteger(value.limit) && value.limit >= 1 && value.limit <= 50 && Number.isInteger(value.offset) && value.offset >= 0 && value.offset <= 10000);
    require(typeof value.has_more === 'boolean' && Array.isArray(value.requests) && value.requests.length <= value.limit && (!value.has_more || value.requests.length === value.limit));
    const seen = new Set(); let previous = null;
    for (const r of value.requests) {
      require(object(r) && Object.keys(r).length === 3 && ['id','session_id','created_ms'].every(k => Object.hasOwn(r,k)));
      require(typeof r.id === 'string' && uuid.test(r.id) && typeof r.session_id === 'string' && r.session_id.startsWith('http:') && uuid.test(r.session_id.slice(5)) && !seen.has(r.id)); seen.add(r.id);
      require(Number.isSafeInteger(r.created_ms) && r.created_ms >= 0);
      require(!previous || previous.created_ms > r.created_ms || (previous.created_ms === r.created_ms && previous.id > r.id)); previous = r;
    }
    return value;
  }
  function catalog(value, expected) {
    require(object(value) && Object.keys(value).length === 6 && value.protocol === 1);
    require(['all','unresolved','running','completed','needs_review'].includes(value.state) && value.state === expected.state);
    require(Number.isInteger(value.limit) && value.limit >= 1 && value.limit <= 50 && value.limit === expected.limit);
    require(Number.isInteger(value.offset) && value.offset >= 0 && value.offset <= 10000 && value.offset === expected.offset);
    require(typeof value.has_more === 'boolean' && Array.isArray(value.turns) && value.turns.length <= value.limit && (!value.has_more || value.turns.length === value.limit));
    const fields = ['id','session_id','created_ms','finished_ms','state','session_committed','cancel_requested','result_purged','reviewed_ms'], seen = new Set();
    let previous = null;
    for (const r of value.turns) {
      require(object(r) && Object.keys(r).length === fields.length && fields.every(k => Object.hasOwn(r,k)));
      require(typeof r.id === 'string' && uuid.test(r.id) && typeof r.session_id === 'string' && r.session_id.startsWith('http:') && uuid.test(r.session_id.slice(5)) && !seen.has(r.id)); seen.add(r.id);
      for (const key of ['created_ms','finished_ms','reviewed_ms']) require((key !== 'created_ms' && r[key] === null) || (Number.isSafeInteger(r[key]) && r[key] >= 0));
      for (const key of ['session_committed','cancel_requested','result_purged']) require(typeof r[key] === 'boolean');
      require(['running','completed','needs_review'].includes(r.state));
      require(r.state !== 'running' || (r.finished_ms === null && !r.session_committed && !r.result_purged && r.reviewed_ms === null));
      require(r.state === 'running' || r.finished_ms !== null);
      require(r.state !== 'completed' || r.session_committed);
      require(r.reviewed_ms === null || r.state === 'needs_review');
      require(value.state === 'all' || (value.state === 'unresolved' ? (r.state === 'running' || (r.state === 'needs_review' && r.reviewed_ms === null)) : r.state === value.state));
      require(!previous || previous.created_ms > r.created_ms || (previous.created_ms === r.created_ms && previous.id > r.id)); previous = r;
    }
    return value;
  }
  function receipt(r, expected, tools = null) {
    require(object(r) && uuid.test(r.id) && typeof r.session_id === 'string' && r.session_id.startsWith('http:') && uuid.test(r.session_id.slice(5)));
    require(hash.test(r.request_hash) && hash.test(r.context_hash));
    require(Number.isSafeInteger(r.created_ms) && r.created_ms >= 0 && (r.finished_ms === null || Number.isSafeInteger(r.finished_ms)));
    for (const key of ['session_committed','cancel_requested','result_purged']) require(typeof r[key] === 'boolean');
    require(['running','completed','needs_review'].includes(r.state) && (r.error === null || text(r.error, 512)));
    require((r.reviewed_ms === null || Number.isSafeInteger(r.reviewed_ms)) && (r.review_note === null || text(r.review_note, 1024)));
    require(r.result === null || (object(r.result) && text(r.result.reply, 2 * MiB) && text(r.result.status, 64) && Array.isArray(r.result.tool_names) && r.result.tool_names.length <= 2048 && r.result.tool_names.every(name => text(name, 128))));
    require(!tools || !r.result || r.result.tool_names.every(name => tools.has(name)));
    require(!r.result_purged || r.result === null);
    require(r.state !== 'running' || (r.finished_ms === null && !r.session_committed && r.result === null && !r.result_purged && r.reviewed_ms === null));
    require(r.state === 'running' || (r.finished_ms !== null && r.finished_ms >= 0));
    require(r.state !== 'completed' || (r.session_committed && r.error === null && (r.result_purged || r.result?.status === 'completed')));
    require(r.reviewed_ms === null || (r.state === 'needs_review' && r.review_note !== null));
    require(r.feedback_revision === undefined || (Number.isInteger(r.feedback_revision) && r.feedback_revision > 0 && r.feedback_revision <= 2147483647));
    if (r.feedback !== undefined) {
      const f = r.feedback;
      require(object(f) && Object.keys(f).length === 4 && ['revision','decision','note','updated_ms'].every(k => Object.hasOwn(f,k)));
      require(f.revision === r.feedback_revision && ['usable','needs_changes'].includes(f.decision) && feedbackNote(f.note) && Number.isSafeInteger(f.updated_ms) && f.updated_ms >= 0);
      require(r.state === 'completed' && !r.result_purged);
    }
    if (expected) {
      require(r.id === expected.id && (!expected.session_id || r.session_id === expected.session_id));
      if (expected.request_hash) require(r.request_hash === expected.request_hash && r.context_hash === expected.context_hash && r.created_ms === expected.created_ms);
    }
    return r;
  }
  function snapshot(value, expected, tools = null) {
    require(object(value) && value.protocol === 1 && typeof value.active === 'boolean');
    receipt(value.receipt, expected, tools);
    return value;
  }
  function gatewaySnapshot(value, expected, tools = null) {
    require(object(value) && Object.keys(value).length === 3 && ['protocol','receipt','active'].every(k => Object.hasOwn(value,k)));
    snapshot(value, expected, tools);
    // Historical file receipts remain readable after the operator withdraws a
    // grant. This validates facts, while captured attempt tools validate authority.
    require(!value.receipt.result || value.receipt.result.tool_names.every(t => ['datetime_now','json_query','file_read','file_write','skill_read','memory_search','tree'].includes(t)));
    return value;
  }
  class Parser {
    constructor(expected, tools, onEvent) {
      this.expected = expected; this.tools = new Set(tools); this.onEvent = onEvent;
      // Incremental fatal decoding across bounded byte slices, including split UTF-8.
      this.decoder = new TextDecoder('utf-8', {fatal:true, ignoreBOM:true});
      this.parts = []; this.partial = ''; this.frameBytes = 0; this.lastLF = false; this.wire = 0; this.total = 0; this.roundBytes = 0;
      this.admitted = null; this.round = -1; this.operation = null; this.modelDone = false; this.terminal = false;
    }
    push(chunk) {
      require(chunk instanceof Uint8Array); this.wire += chunk.byteLength;
      require(this.wire <= 12 * MiB);
      for (let offset = 0; offset < chunk.length; offset += 4096) this.consume(this.decoder.decode(chunk.subarray(offset, offset + 4096), {stream:true}));
    }
    append(part) {
      this.frameBytes += bytes(part); require(this.frameBytes <= 2 * MiB + 16 * 1024 + 64);
      // Coalesce tiny network reads into finite pieces. Never rescan or copy the
      // entire growing terminal JSON on each incoming byte.
      this.partial += part;
      if (this.partial.length >= 8192) { this.parts.push(this.partial); this.partial = ''; }
    }
    consume(decoded) {
      if (!decoded) return;
      let piece = (this.lastLF ? '\n' : '') + decoded; this.lastLF = false;
      let boundary;
      while ((boundary = piece.indexOf('\n\n')) !== -1) {
        this.append(piece.slice(0, boundary));
        const frame = this.parts.join('') + this.partial;
        this.parts = []; this.partial = ''; this.frameBytes = 0;
        this.frame(frame); piece = piece.slice(boundary + 2);
      }
      if (piece.endsWith('\n')) { this.lastLF = true; piece = piece.slice(0,-1); }
      this.append(piece);
    }
    frame(frame) {
      if (frame === ': keepalive') { require(!this.terminal); return; }
      require(!this.terminal && !frame.includes('\r'));
      const match = /^event: ([a-z_]+)\ndata: ([^\n]+)$/.exec(frame); require(match);
      const value = JSON.parse(match[2]), name = match[1];
      require(object(value) && value.event === name);
      if (name === 'admitted' || name === 'done') {
        require(value.protocol === 1); receipt(value.receipt, this.admitted || this.expected, this.tools);
        if (name === 'admitted') {
          require(!this.admitted && value.receipt.state === 'running' && bytes(match[2]) <= 16384); this.admitted = value.receipt;
        } else {
          require(this.admitted && value.receipt.state !== 'running');
          this.terminal = true;
        }
      } else {
        require(this.admitted && bytes(match[2]) <= 8192);
        if (name === 'error') { require(value.code === 'http_stream_requires_review'); this.terminal = true; }
        else if (name === 'model_started') {
          require(value.turn_id === this.expected.id && anyUuid.test(value.operation_id) && anyUuid.test(value.remote_id));
          require(value.round === this.round + 1 && value.round < 33 && (this.round === -1 || this.modelDone) && text(value.model, 1024));
          this.round = value.round; this.operation = value.operation_id; this.modelDone = false; this.roundBytes = 0;
        } else if (name === 'preview') {
          require(value.round === this.round && this.round >= 0 && !this.modelDone && text(value.text, 1024));
          const length = bytes(value.text); this.roundBytes += length; this.total += length;
          require(this.roundBytes <= 2 * MiB && this.total <= 8 * MiB);
        } else if (name === 'model_completed') {
          require(this.round >= 0 && value.round === this.round && value.operation_id === this.operation && !this.modelDone); this.modelDone = true;
        } else if (name === 'tool_completed') {
          require(this.round >= 0 && value.round === this.round && this.modelDone && text(value.tool_call_id, 1024) && this.tools.has(value.tool_name));
        } else require(false);
      }
      this.onEvent(value);
    }
    finish() { this.consume(this.decoder.decode()); require(this.frameBytes === 0 && !this.lastLF && this.terminal); }
  }
  const api = Object.freeze({bytes, feedbackNote, uuid, capabilities, gatewayCapabilities, catalog, gatewayCatalog, receipt, snapshot, gatewaySnapshot, Parser});
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else globalThis.JiaClawTurnStream = api;
})();
