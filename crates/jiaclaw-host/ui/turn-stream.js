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
  function capabilities(c) {
    require(object(c) && c.protocol === 1 && typeof c.enabled === 'boolean' && typeof c.streaming === 'boolean' && c.streaming === c.enabled);
    require(c.session_prefix === 'http:' && c.stream_suffix === '/stream' && c.max_active === 1 && c.max_identities === 10000 && c.max_retained_results === 32);
    require(!c.streaming || (Number.isInteger(c.turn_budget_secs) && c.turn_budget_secs >= 1 && c.turn_budget_secs <= 300));
    require(c.max_stream_wire_bytes === 12 * MiB && c.max_preview_round_bytes === 2 * MiB && c.max_preview_total_bytes === 8 * MiB);
    require(c.listing === undefined || typeof c.listing === 'boolean');
    return c;
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
  function receipt(r, expected) {
    require(object(r) && uuid.test(r.id) && typeof r.session_id === 'string' && r.session_id.startsWith('http:') && uuid.test(r.session_id.slice(5)));
    require(hash.test(r.request_hash) && hash.test(r.context_hash));
    require(Number.isSafeInteger(r.created_ms) && r.created_ms >= 0 && (r.finished_ms === null || Number.isSafeInteger(r.finished_ms)));
    for (const key of ['session_committed','cancel_requested','result_purged']) require(typeof r[key] === 'boolean');
    require(['running','completed','needs_review'].includes(r.state) && (r.error === null || text(r.error, 512)));
    require((r.reviewed_ms === null || Number.isSafeInteger(r.reviewed_ms)) && (r.review_note === null || text(r.review_note, 1024)));
    require(r.result === null || (object(r.result) && text(r.result.reply, 2 * MiB) && text(r.result.status, 64) && Array.isArray(r.result.tool_names) && r.result.tool_names.length <= 2048 && r.result.tool_names.every(name => text(name, 128))));
    require(!r.result_purged || r.result === null);
    require(r.state !== 'running' || (r.finished_ms === null && !r.session_committed && r.result === null && !r.result_purged && r.reviewed_ms === null));
    require(r.state === 'running' || (r.finished_ms !== null && r.finished_ms >= 0));
    require(r.state !== 'completed' || (r.session_committed && r.error === null && (r.result_purged || r.result?.status === 'completed')));
    require(r.reviewed_ms === null || (r.state === 'needs_review' && r.review_note !== null));
    if (expected) {
      require(r.id === expected.id && (!expected.session_id || r.session_id === expected.session_id));
      if (expected.request_hash) require(r.request_hash === expected.request_hash && r.context_hash === expected.context_hash && r.created_ms === expected.created_ms);
    }
    return r;
  }
  function snapshot(value, expected) {
    require(object(value) && value.protocol === 1 && typeof value.active === 'boolean');
    receipt(value.receipt, expected);
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
        require(value.protocol === 1); receipt(value.receipt, this.admitted || this.expected);
        if (name === 'admitted') {
          require(!this.admitted && value.receipt.state === 'running' && bytes(match[2]) <= 16384); this.admitted = value.receipt;
        } else { require(this.admitted && value.receipt.state !== 'running'); this.terminal = true; }
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
  const api = Object.freeze({bytes, uuid, capabilities, catalog, receipt, snapshot, Parser});
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  else globalThis.JiaClawTurnStream = api;
})();
