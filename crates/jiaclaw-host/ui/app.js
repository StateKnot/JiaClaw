'use strict';
const $ = id => document.getElementById(id);
const MISSING_ROUTE = Symbol('missing-route');
let token = '', selected = null, connected = false, busy = false, readOnly = false, sessionList = [];
let sessionNext = null;
let identity = 0, operation = 0, scheduledJobs = false, jobList = [], selectedJob = null;
let jobsOffset = 0, jobsNext = null, jobsPrevious = [], runsOffset = 0, runsNext = null, runsPrevious = [];
let jobsMode = null, jobsHealth = null, pendingCreate = null, jobsIncludeDeleted = false;
let outboxEnabled = false, deliveries = [], delivery = null, outboxOffset = 0, outboxNext = false, deliveryFresh = false;
class StaleIdentity extends Error {}
class ApiError extends Error { constructor(message, code) { super(message); this.code = code; } }
class ApiTimeout extends Error {}
const clipped = (text, length) => { text = String(text || ''); return text.length > length ? text.slice(0, length) + '\n[显示已截断；完整记录保留在服务端]' : text; };
const date = value => value == null ? '—' : new Date(value).toLocaleString();
function status(text, error = false) { $('status').textContent = text; $('status').classList.toggle('error', error); }
function controls() {
  $('new-session').disabled = !connected || readOnly || busy;
  $('access-mode').hidden = !connected || !readOnly;
  $('job-create').hidden = readOnly;
  $('refresh').disabled = !connected || busy;
  $('session-next').disabled = !connected || busy || sessionNext === null;
  $('delete-session').disabled = !connected || readOnly || !selected || busy;
  $('message').disabled = !connected || readOnly || !selected || busy;
  $('send').disabled = !connected || readOnly || !selected || busy;
  for (const button of $('sessions').querySelectorAll('button')) button.disabled = !connected || busy;
  for (const element of $('jobs-view').querySelectorAll('button,input,textarea,select')) element.disabled = !connected || !scheduledJobs || busy;
  $('jobs-prev').disabled ||= jobsPrevious.length === 0;
  $('jobs-next').disabled ||= jobsNext === null;
  $('runs-prev').disabled ||= runsPrevious.length === 0;
  $('runs-next').disabled ||= runsNext === null;
  for (const element of $('job-form').querySelectorAll('button,input,textarea,select')) element.disabled ||= readOnly || jobsHealth !== 'running' || !!pendingCreate;
  $('job-toggle').disabled ||= readOnly || !selectedJob || selectedJob.deleted || (!selectedJob.enabled && jobsHealth !== 'running');
  $('job-create-check').disabled ||= !pendingCreate; $('job-create-abandon').disabled ||= !pendingCreate;
  $('job-create-retry').disabled ||= readOnly || !pendingCreate || !!pendingCreate.conflict || jobsHealth !== 'running';
  $('job-delete').disabled ||= readOnly || !selectedJob || selectedJob.deleted;
  $('runs-refresh').disabled ||= !selectedJob;
  for (const element of $('outbox-view').querySelectorAll('button,input,textarea')) element.disabled = !connected || !outboxEnabled || busy;
  $('outbox-prev').disabled ||= outboxOffset === 0;
  $('outbox-next').disabled ||= !outboxNext;
  $('delivery-refresh').disabled ||= !delivery;
  $('delivery-evidence').disabled ||= !delivery;
  $('delivery-resolve').disabled ||= !deliveryFresh || delivery?.state !== 'unknown';
  $('delivery-cancel').disabled ||= !deliveryFresh || !['pending','retry_wait','unknown','permanent_failed','expired'].includes(delivery?.state);
  for (const id of ['chat-tab','jobs-tab','outbox-tab']) $(id).disabled = !connected || busy;
  turnControls();
}
function clearIdentity() {
  forgetTurns(); identity++; token = ''; connected = false; readOnly = false; selected = null; sessionList = [];
  sessionNext = null;
  $('access-mode').hidden = true; $('job-create').hidden = false;
  scheduledJobs = false; jobList = []; selectedJob = null; jobsMode = null; jobsHealth = null; pendingCreate = null;
  $('job-lookup').hidden = true; $('job-lookup-id').value = ''; $('job-create-tracking').hidden = true;
  outboxEnabled = false; deliveries = []; delivery = null; outboxOffset = 0; outboxNext = false; deliveryFresh = false;
  jobsOffset = 0; jobsNext = null; jobsPrevious = []; runsOffset = 0; runsNext = null; runsPrevious = [];
  $('api-token').value = ''; $('message').value = '';
  $('session-title').textContent = '开始一段对话';
  $('session-list-state').hidden = true; $('session-list-state').textContent = '';
  $('workspace-tabs').hidden = true; $('jobs-tab').hidden = true; $('outbox-tab').hidden = true; showView('chat');
  $('delivery-detail').hidden = true; $('delivery-evidence').value = ''; $('outbox-id').value = '';
  for (const id of ['outbox-list','outbox-count','outbox-health','delivery-title','delivery-summary','delivery-content','delivery-receipt','delivery-error']) $(id).replaceChildren();
  $('job-form').reset(); $('job-create').open = false; $('job-cron-fields').hidden = true; $('job-interval-field').hidden = false; $('job-interval').required = true;
  jobsIncludeDeleted = false; $('jobs-deleted').checked = false; $('job-detail').hidden = true;
  for (const id of ['jobs-list', 'job-runs', 'job-title', 'job-summary', 'job-detail-prompt', 'job-detail-contract', 'jobs-count', 'jobs-health', 'job-create-id', 'job-create-state']) $(id).replaceChildren();
  renderMessages([]); renderSessions();
}
async function api(path, method = 'GET', body, optional = false, timeout = 0, maxBytes = 0, parentSignal = null) {
  if (readOnly && method !== 'GET') throw new ApiError('当前密钥仅允许查看；修改内容或运行模型需要完整权限密钥。', 403);
  const owner = identity, credential = token, controller = new AbortController();
  const abort = () => controller.abort();
  parentSignal?.addEventListener('abort', abort, {once:true});
  if (parentSignal?.aborted) controller.abort();
  const timer = timeout ? setTimeout(() => controller.abort(), timeout) : null;
  try {
    const response = await fetch(path, { method, signal: controller.signal, credentials: 'omit', cache: 'no-store', headers: { ...(credential ? { Authorization: `Bearer ${credential}` } : {}), ...(body ? { 'Content-Type': 'application/json' } : {}) }, ...(body ? { body: JSON.stringify(body) } : {}) });
    if (owner !== identity) throw new StaleIdentity();
    controller.signal.throwIfAborted();
    if (response.status === 401) { clearIdentity(); throw new ApiError('鉴权失败，请重新连接并检查 API Token。', 401); }
    if (optional === true && response.status === 404) return MISSING_ROUTE;
    if (Array.isArray(optional) && optional.includes(response.status)) return null;
    const reviewRequired = response.headers.get('x-jiaclaw-write-review') === 'required';
    const reviewMessage = '结果需要管理员核对。请保留当前内容，勿重复提交；确认后端空闲并处理未知结果后再继续。';
    if (response.status === 204) { if (reviewRequired) throw new Error(reviewMessage); return null; }
    let data;
    try { data = maxBytes ? await boundedJson(response, maxBytes, owner) : await response.json(); } catch (error) { if (owner !== identity) throw new StaleIdentity(); if (error instanceof ApiError || error.name === 'AbortError') throw error; throw new ApiError(`服务响应异常（HTTP ${response.status}）`, response.status); }
    if (owner !== identity) throw new StaleIdentity();
    controller.signal.throwIfAborted();
    if (!response.ok) throw new ApiError(clipped(data?.error || `请求失败（HTTP ${response.status}）`, 1024), response.status);
    if (reviewRequired) throw new Error(reviewMessage);
    return data;
  } catch (error) {
    if (owner !== identity && !(error instanceof ApiError && error.code === 401)) throw new StaleIdentity();
    if (error.name === 'AbortError' && controller.signal.aborted) throw new ApiTimeout('请求超时；服务器可能仍在处理，请刷新并核对结果。');
    throw error;
  } finally { if (timer) clearTimeout(timer); parentSignal?.removeEventListener('abort', abort); controller.abort(); }
}
async function boundedJson(response, maximum, owner) {
  const reader = response.body?.getReader();
  if (!reader) throw new Error('empty response');
  const chunks = []; let length = 0;
  try {
    while (true) {
      const {done, value} = await reader.read();
      if (owner !== identity) throw new StaleIdentity();
      if (done) break;
      length += value.byteLength;
      if (length > maximum) throw new ApiError('响应超过本页读取上限；未跳过记录，请联系管理员核对。', response.status);
      chunks.push(value);
    }
  } catch (error) { await reader.cancel().catch(() => {}); throw error; }
  finally { reader.releaseLock(); }
  const bytes = new Uint8Array(length); let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  return JSON.parse(new TextDecoder('utf-8', {fatal:true}).decode(bytes));
}
const jobsApi = (path, method = 'GET', body, optional = false) => api(path, method, body, optional, 30000, 2 * 1024 * 1024);
function showView(view) {
  for (const name of ['chat','jobs','outbox']) { $(name+'-view').hidden = name !== view; $(name+'-tab').setAttribute('aria-pressed', String(name === view)); }
}
function renderSessions() {
  $('sessions').replaceChildren();
  for (const session of sessionList) {
    const button = document.createElement('button');
    button.textContent = `${session.id.slice(0, 24)} · ${session.message_count} 条`;
    button.title = session.id; button.setAttribute('aria-current', String(session.id === selected));
    button.addEventListener('click', () => task(() => select(session.id)));
    $('sessions').append(button);
  }
  controls();
}
function renderMessages(messages) {
  $('messages').replaceChildren();
  for (const message of messages.slice(-51)) {
    const article = document.createElement('article'); article.className = `message ${message.role === 'user' ? 'user' : 'assistant'}`;
    const role = document.createElement('div'); role.className = 'role'; role.textContent = { user: '你', assistant: 'JiaClaw', system: '会话上下文' }[message.role] || '消息';
    const text = document.createElement('p'); text.textContent = clipped(message.content, 65536);
    article.append(role, text); $('messages').append(article);
  }
  $('messages').scrollTop = $('messages').scrollHeight;
}
async function refresh(after = null) {
  const page = await api('/api/sessions' + (after === null ? '' : '?after=' + after), 'GET', undefined, false, 15000, 512 * 1024);
  const cursor = id => Array.from(new TextEncoder().encode(id), byte => byte.toString(16).padStart(2, '0')).join('');
  if (!page || page.limit !== 50 || typeof page.has_more !== 'boolean' || !Array.isArray(page.sessions) || page.sessions.length > 50 || page.sessions.some(s => !s || typeof s.id !== 'string' || !s.id || new TextEncoder().encode(s.id).length > 1024 || !Number.isSafeInteger(s.message_count) || s.message_count < 0) || new Set(page.sessions.map(s => s.id)).size !== page.sessions.length || (page.has_more ? page.sessions.length !== 50 || page.next_cursor !== cursor(page.sessions.at(-1).id) || page.next_cursor === after : page.next_cursor !== null)) throw new Error('会话目录响应异常；未跳过记录，请联系管理员核对。');
  sessionList = page.sessions; sessionNext = page.next_cursor;
  $('session-list-state').hidden = false;
  $('session-list-state').textContent = `${sessionList.length} 条会话${page.has_more ? ' · 还有下一页' : ' · 已到末页'}；刷新列表返回第一页。`;
  if (trackedSession() && !tenantTurns() && !sessionList.some(s => s.id === selected)) sessionList.unshift({id:selected, message_count:0});
  renderSessions();
}
async function select(id, allowMissing = false) {
  const session = await api(`/api/sessions/${encodeURIComponent(id)}`, 'GET', undefined, allowMissing ? true : false, 15000, 26 * 1024 * 1024);
  if (session === MISSING_ROUTE && allowMissing) {
    // A permanent turn receipt does not guarantee its history survived TTL or
    // deletion. Do not create an empty virtual session or preserve another view.
    selected = null; $('session-title').textContent = '原请求历史不可读取';
    showView('chat'); renderMessages([]); renderSessions(); return false;
  }
  if (!session || session.id !== id || !Array.isArray(session.messages) || session.messages.length > 51 || session.messages.some(m => typeof m.content !== 'string')) throw new Error('会话响应异常');
  selected = id; $('session-title').textContent = `会话 ${id.slice(0, 12)}`;
  showView('chat');
  renderMessages(session.messages); renderSessions(); status(readOnly ? '已连接 · 只读访问 · 会话就绪' : '已连接 · 会话就绪');
  return true;
}
async function task(fn, replace = false) {
  if (busy && !replace) return;
  const current = ++operation;
  busy = true; controls();
  try { await fn(); } catch (error) { if (current === operation && !(error instanceof StaleIdentity)) status(error.message, true); }
  finally { if (current === operation) { busy = false; controls(); } }
}
$('connect-form').addEventListener('submit', event => {
  event.preventDefault(); const nextToken = $('api-token').value.trim(), resume = resumeTurnId; resumeTurnId = null; clearIdentity(); token = nextToken;
  task(async () => {
    status('连接中…');
    const capabilities = await api('/api/gateway/capabilities', 'GET', undefined, true, 15000, 16384);
    if (capabilities !== MISSING_ROUTE && (!capabilities || typeof capabilities.scheduled_jobs !== 'boolean' || (Object.hasOwn(capabilities, 'read_only') && typeof capabilities.read_only !== 'boolean'))) {
      clearIdentity(); throw new Error('权限信息响应异常，请重新连接或联系管理员核对。');
    }
    readOnly = capabilities !== MISSING_ROUTE && capabilities.read_only === true;
    scheduledJobs = capabilities?.scheduled_jobs === true; jobsMode = scheduledJobs ? 'gateway' : null;
    if (capabilities === MISSING_ROUTE && token) {
      const scheduler = await jobsApi('/api/jobs/status', 'GET', undefined, [403,404]);
      if (scheduler?.create_identity_protocol === 'job-id-v1' && ['running','failed','stopping'].includes(scheduler.state)) { scheduledJobs = true; jobsMode = 'standalone'; jobsHealth = scheduler.state; }
    }
    $('job-lookup').hidden = jobsMode !== 'standalone'; $('jobs-tab').hidden = !scheduledJobs;
    // Only the authenticated administrator endpoint grants this capability. Gateways deny it.
    const channelStatus = token && !readOnly ? await api('/api/channels/status', 'GET', undefined, [403,404], 30000) : null;
    outboxEnabled = channelStatus && ['running','failed','stopping','disabled'].includes(channelStatus.state);
    $('outbox-tab').hidden = !outboxEnabled; $('workspace-tabs').hidden = !scheduledJobs && !outboxEnabled;
    await connectTurns(capabilities !== MISSING_ROUTE);
    try { await refresh(); }
    catch (error) {
      // Original-turn control remains available while real execution owns the
      // backend's primary lane. An unavailable legacy list is not an empty list.
      if (!tenantTurns() || !(error instanceof ApiError) || ![429,502,503].includes(error.code)) throw error;
      $('session-list-state').hidden = false; $('session-list-state').textContent = '会话列表暂不可读取；可继续核对原请求，执行结束后刷新列表。';
    }
    connected = true; controls();
    if (resume && turnCapabilities && (!readOnly || tenantTurns())) { trackTurn(resume); await checkTurn(); return; }
    status(readOnly ? '已连接 · 只读访问 · 选择会话查看' : '已连接 · 选择或新建会话');
  }, true);
});
$('refresh').addEventListener('click', () => task(async () => { await refresh(); status('会话列表已更新'); }));
$('session-next').addEventListener('click', () => task(async () => { if (sessionNext !== null) { await refresh(sessionNext); status('已读取下一页会话'); } }));
$('new-session').addEventListener('click', () => task(async () => {
  if (!connected || readOnly) return;
  if (pendingTurn) return;
  if (turnWritable()) { $('turn-permissions').open = false; for (const input of $('turn-permissions').querySelectorAll('input')) input.checked = false; selected = 'http:' + createId(); $('session-title').textContent = `会话 ${selected.slice(0,17)}`; renderMessages([]); if (!tenantTurns()) await refresh(); showView('chat'); status('新会话就绪；请显式选择本次允许的工具。'); return; }
  const data = await api('/api/sessions', 'POST'); await refresh(); await select(data.session_id);
}));
$('delete-session').addEventListener('click', () => {
  if (!connected || readOnly || !selected || pendingTurn || !confirm('删除这段会话及全部历史？此操作无法撤销。')) return;
  task(async () => { await api(`/api/sessions/${encodeURIComponent(selected)}`, 'DELETE'); selected = null; renderMessages([]); $('session-title').textContent = '开始一段对话'; await refresh(); status('会话已删除'); });
});
$('chat-form').addEventListener('submit', event => {
  event.preventDefault(); const text = $('message').value.trim(); if (!connected || readOnly || !text || !selected || pendingTurn) return;
  task(async () => {
    if (trackedSession()) {
      if (!turnWritable() || pendingTurn) return;
      if (Turn.bytes(text) > 32768) throw new Error('实时消息超过 32 KiB UTF-8 上限。');
      const names = id => [...$(id).querySelectorAll('input:checked')].map(e => e.value);
      const tools = names('turn-tools'), skills = names('turn-skills');
      if ($('turn-tools').childElementCount && !tools.length) { $('turn-permissions').open = true; throw new Error('请选择本次允许的工具；空白不会授权全部工具。'); }
      if (tools.length > 128 || skills.length > 16) { $('turn-permissions').open = true; throw new Error('单次最多授权 128 个工具和 16 个技能；请减少勾选。'); }
      const body = {session_id:selected, prompt:text, enabled_tools:tools, enabled_skills:skills};
      if (Turn.bytes(JSON.stringify(body)) > 65536) throw new Error('完整请求超过 64 KiB 上限。');
      const attempt = trackTurn(createId(), body); await submitTurn(attempt); return;
    }
    status('JiaClaw 正在处理…');
    await api('/api/chat', 'POST', { messages: [{ role: 'user', content: text }], session_id: selected, stream: false });
    $('message').value = ''; await select(selected); await refresh(); status('回复已保存');
  });
});
$('message').addEventListener('keydown', event => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); $('chat-form').requestSubmit(); } });
function page(data, offset, limit = 5) {
  if (jobsMode === 'standalone') {
    if (!Array.isArray(data) || data.length > limit) throw new Error('任务分页响应异常');
    return {items:data, next_offset:data.length === limit && offset + limit < 10000 ? offset + limit : null};
  }
  if (!data || !Array.isArray(data.items) || data.items.length > limit || !(data.next_offset === null || (Number.isInteger(data.next_offset) && data.next_offset > offset && data.next_offset <= 10000))) throw new Error('任务分页响应异常');
  return data;
}
const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
function jobRecord(job, expectedId) {
  if (!job || typeof job.id !== 'string' || !uuidPattern.test(job.id) || (expectedId && job.id !== expectedId) || !job.spec || typeof job.spec.name !== 'string' || typeof job.spec.prompt !== 'string' || !job.spec.schedule || !Array.isArray(job.spec.enabled_tools) || typeof job.enabled !== 'boolean' || typeof job.deleted !== 'boolean') throw new Error('任务记录响应异常');
  return job;
}
function jobContract(spec) {
  const schedule = spec.schedule?.kind === 'cron' ? `Cron：${spec.schedule.expression} · 时区：${spec.schedule.timezone}` : `固定间隔：${spec.schedule?.seconds} 秒（UTC）`;
  return clipped(`${schedule}\n允许工具：${(spec.enabled_tools || []).join(', ')}\n最长运行：${spec.timeout_secs} 秒\n通知目的地：${spec.delivery ? JSON.stringify(spec.delivery) : '无，仅保存在工作台'}\n运行完成不代表通知已送达；渠道状态请在发件箱核对。`, 16384);
}
async function refreshJobsHealth() {
  const health = await jobsApi('/api/jobs/status');
  if (!health || !['running','failed','stopping','disabled'].includes(health.state) || (jobsMode === 'standalone' && health.create_identity_protocol !== 'job-id-v1')) { jobsHealth = null; controls(); throw new Error('调度器能力或状态响应异常，请重新连接核对。'); }
  jobsHealth = health.state;
  $('jobs-health').textContent = `调度器：${{running:'运行中',failed:'故障，仅允许审计、暂停与删除；请检查服务器',stopping:'停止中，仅允许审计、暂停与删除',disabled:'已停止'}[jobsHealth]}`;
  controls();
}
function renderJobs() {
  $('jobs-list').replaceChildren();
  for (const job of jobList) {
    const button = document.createElement('button'); button.dataset.jobId = job.id;
    button.textContent = `${job.spec.name} · ${job.deleted ? '已删除' : job.enabled ? '已启用' : '已暂停'}`;
    button.setAttribute('aria-current', String(job.id === selectedJob?.id));
    button.addEventListener('click', () => task(() => selectJob(job.id)));
    $('jobs-list').append(button);
  }
  $('jobs-count').textContent = jobList.length ? `第 ${jobsOffset + 1}–${jobsOffset + jobList.length} 项` : '此页没有任务'; controls();
}
async function refreshJobs(targetOffset = jobsOffset, previous = jobsPrevious, includeDeleted = jobsIncludeDeleted) {
  const data = page(await jobsApi(`/api/jobs?limit=5&offset=${targetOffset}&include_deleted=${includeDeleted}`), targetOffset);
  const validated = data.items.map(job => jobRecord(job));
  jobList = validated; jobsOffset = targetOffset; jobsPrevious = previous; jobsNext = data.next_offset;
  jobsIncludeDeleted = includeDeleted; $('jobs-deleted').checked = includeDeleted; renderJobs();
}
async function selectJob(id) {
  const job = jobRecord(await jobsApi(`/api/jobs/${encodeURIComponent(id)}`), id);
  selectedJob = job; $('job-detail').hidden = false; $('job-title').textContent = job.spec.name;
  $('job-summary').textContent = `${job.deleted ? '已删除' : job.enabled ? '已启用' : '已暂停'} · 下次时间：${date(job.next_due_ms)} · ID：${job.id}`;
  $('job-detail-contract').textContent = jobContract(job.spec);
  $('job-detail-prompt').textContent = clipped(job.spec.prompt, 4096);
  $('job-toggle').textContent = job.enabled ? '暂停任务' : '恢复任务';
  $('job-runs').replaceChildren(); runsNext = null; runsOffset = 0; runsPrevious = []; renderJobs();
  await refreshRuns(0, []);
}
async function refreshRuns(targetOffset = runsOffset, previous = runsPrevious) {
  if (!selectedJob) return;
  const limit = jobsMode === 'standalone' ? 1 : 5;
  const data = page(await jobsApi(`/api/jobs/${encodeURIComponent(selectedJob.id)}/runs?limit=${limit}&offset=${targetOffset}`), targetOffset, limit);
  const fragment = document.createDocumentFragment();
  for (const run of data.items) {
    if (!run || typeof run.id !== 'string' || !uuidPattern.test(run.id) || typeof run.status !== 'string') throw new Error('运行记录响应异常');
    const article = document.createElement('article'); article.className = 'job-run'; article.dataset.runId = run.id;
    const title = document.createElement('h4'); title.textContent = `${date(run.scheduled_for_ms)} · ${{running:'运行中',completed:'已完成',failed:'失败，需核对',needs_review:'需核对',interrupted:'已中断，需核对',skipped:'已跳过'}[run.status] || run.status} · ${run.id}`;
    const text = document.createElement('pre'); text.className = 'run-content'; text.textContent = clipped(run.response?.message?.content || '尚无已保存的响应正文。', 16384);
    const error = document.createElement('pre'); error.className = 'run-error'; error.textContent = clipped(run.error || '', 4096);
    const contract = document.createElement('pre'); contract.className = 'run-contract'; contract.textContent = run.spec ? jobContract(run.spec) : '此运行未返回配置快照。';
    article.append(title, text, error, contract); fragment.append(article);
  }
  runsOffset = targetOffset; runsPrevious = previous; runsNext = data.next_offset;
  $('job-runs').replaceChildren(fragment);
  if (!data.items.length) $('job-runs').textContent = '此页没有运行记录。';
  controls();
}
$('chat-tab').addEventListener('click', () => showView('chat'));
$('jobs-tab').addEventListener('click', () => task(async () => {
  if (!scheduledJobs) return; showView('jobs');
  await refreshJobsHealth(); await refreshJobs(); status('定时任务已加载');
}));
$('jobs-refresh').addEventListener('click', () => task(async () => { await refreshJobsHealth(); await refreshJobs(); if (selectedJob) await selectJob(selectedJob.id); status('任务已更新'); }));
$('jobs-deleted').addEventListener('change', () => task(async () => {
  const owner = identity;
  try { await refreshJobs(0, [], $('jobs-deleted').checked); }
  catch (error) { if (owner === identity) $('jobs-deleted').checked = jobsIncludeDeleted; throw error; }
}));
$('jobs-next').addEventListener('click', () => task(async () => { if (jobsNext !== null) await refreshJobs(jobsNext, [...jobsPrevious, jobsOffset]); }));
$('jobs-prev').addEventListener('click', () => task(async () => { if (jobsPrevious.length) await refreshJobs(jobsPrevious.at(-1), jobsPrevious.slice(0,-1)); }));
$('runs-next').addEventListener('click', () => task(async () => { if (runsNext !== null) await refreshRuns(runsNext, [...runsPrevious, runsOffset]); }));
$('runs-prev').addEventListener('click', () => task(async () => { if (runsPrevious.length) await refreshRuns(runsPrevious.at(-1), runsPrevious.slice(0,-1)); }));
$('runs-refresh').addEventListener('click', () => task(() => refreshRuns()));
async function changeJob(action) {
  if (!connected || readOnly || !selectedJob) return;
  const job = selectedJob, owner = identity;
  const warning = action === 'resume' ? `确认已核对上次运行、外部结果以及以下全部授权？恢复安排未来运行，不重放历史。\n${jobContract(job.spec)}` : action === 'delete' ? '删除任务并停止后续运行？运行记录会保留，已提交的工作可能继续。' : null;
  if (warning && !confirm(`${warning}\n任务 ID：${job.id}`)) return;
  try {
    if (action === 'resume') { await refreshJobsHealth(); if (jobsHealth !== 'running') throw new Error('调度器未运行，不能恢复任务。'); }
    await jobsApi(`/api/jobs/${encodeURIComponent(job.id)}${action === 'delete' ? '' : '/' + action}`, action === 'delete' ? 'DELETE' : 'POST');
  } catch (error) {
    if (owner !== identity) throw error;
    try { await selectJob(job.id); await refreshJobs(); } catch (readError) { if (owner !== identity) throw readError; }
    throw new Error(`操作结果需要核对，勿直接重复提交；已尝试刷新服务器状态。${clipped(error.message,300)}`);
  }
  await refreshJobs(); await selectJob(job.id);
  status(action === 'delete' ? '任务已删除，运行记录已保留' : '任务状态已更新');
}
$('job-toggle').addEventListener('click', () => task(() => changeJob(selectedJob?.enabled ? 'pause' : 'resume')));
$('job-delete').addEventListener('click', () => task(() => changeJob('delete')));
$('job-lookup').addEventListener('submit', event => { event.preventDefault(); task(async () => {
  const id = $('job-lookup-id').value.trim(); if (!uuidPattern.test(id)) throw new Error('请输入完整的小写任务 UUID。');
  selectedJob = null; $('job-detail').hidden = true; await selectJob(id); status('任务记录已加载');
}); });
$('job-kind').addEventListener('change', () => {
  const cron = $('job-kind').value === 'cron'; $('job-cron-fields').hidden = !cron; $('job-interval-field').hidden = cron; $('job-interval').required = !cron;
});
function createId() {
  const bytes = crypto.getRandomValues(new Uint8Array(16)); bytes[6] = (bytes[6] & 15) | 64; bytes[8] = (bytes[8] & 63) | 128;
  const hex = Array.from(bytes, byte => byte.toString(16).padStart(2,'0')).join('');
  return `${hex.slice(0,8)}-${hex.slice(8,12)}-${hex.slice(12,16)}-${hex.slice(16,20)}-${hex.slice(20)}`;
}
function canonical(value) { return JSON.stringify(value, function(key,item) { return item && typeof item === 'object' && !Array.isArray(item) ? Object.fromEntries(Object.entries(item).sort(([a],[b]) => a.localeCompare(b))) : item; }); }
function checkCreated(job, attempt) {
  jobRecord(job, attempt.id);
  if (canonical(job.spec) !== canonical(attempt.spec)) throw new Error('创建 ID 对应的任务配置不一致，请保留 ID 并人工核对。');
  return job;
}
async function acceptCreated(job, attempt) {
  checkCreated(job, attempt); pendingCreate = null;
  $('job-form').reset(); $('job-cron-fields').hidden = true; $('job-interval-field').hidden = false; $('job-interval').required = true;
  $('job-create-state').textContent = `创建记录已确认：${job.deleted ? '已删除' : job.enabled ? '已启用' : '已暂停'}。查询和重试不会重新启用已暂停或已删除的任务。`;
  await refreshJobs(0, []); await selectJob(job.id);
  status(`任务创建记录已确认 · ${job.deleted ? '已删除' : job.enabled ? '已启用' : '已暂停'} · ${job.id}`);
}
async function checkCreation() {
  const attempt = pendingCreate; if (!attempt) return false;
  try {
    const job = checkCreated(await jobsApi(`/api/jobs/${attempt.id}`), attempt);
    await acceptCreated(job, attempt); return true;
  } catch (error) {
    if (error instanceof StaleIdentity || error.code === 401) throw error;
    if (!pendingCreate) throw error;
    $('job-create-state').textContent = attempt.conflict
      ? `创建被服务器拒绝：${attempt.conflict}。已停止重试，原 ID 与配置保留供核对；查询未确认匹配任务。不会自动更换 ID。`
      : error.code === 404 ? '暂未查到此 ID。不能据此认定创建未提交；可稍后查询，或显式用原 ID 和配置重试。' : `尚未确认创建结果；保存此 ID 并继续核对。${clipped(error.message,300)}`;
    return false;
  }
}
async function submitCreation() {
  if (!connected || readOnly) return;
  const attempt = pendingCreate; if (!attempt) return;
  try {
    const job = checkCreated(await jobsApi(`/api/jobs/${attempt.id}`, 'PUT', attempt.spec), attempt);
    await acceptCreated(job, attempt);
  } catch (error) {
    if (error instanceof StaleIdentity || error.code === 401) throw error;
    if (!pendingCreate) throw error;
    if (error.code === 409) attempt.conflict = clipped(error.message,300);
    $('job-create-state').textContent = `创建结果尚未确认；表单与 ID 已保留，正在查询。${clipped(error.message,300)}`;
    if (!(await checkCreation())) status(attempt.conflict ? '创建存在冲突，已停止重试；请保留 ID 并核对拒绝原因。' : '创建结果待核对；不会自动重发，也不会换新 ID 重试。', true);
  }
}
$('job-create-check').addEventListener('click', () => task(() => checkCreation()));
$('job-create-retry').addEventListener('click', () => task(async () => {
  if (readOnly || !pendingCreate || pendingCreate.conflict || !confirm(`用原 ID 和原配置重试创建？已有任务不会被重新启用。\n${pendingCreate.id}`)) return;
  await refreshJobsHealth(); if (jobsHealth !== 'running') throw new Error('调度器未运行，请先查询结果。');
  await submitCreation();
}));
$('job-create-abandon').addEventListener('click', () => {
  if (!pendingCreate || busy || !confirm(`放弃跟踪不会取消服务器任务；原创建仍可能成功。请保存此 ID 并先核对，避免重复创建。\n${pendingCreate.id}`)) return;
  pendingCreate = null; $('job-create-state').textContent = '已放弃本页跟踪；服务器任务未被取消。原 ID 仍可用于查询。'; controls();
});
$('job-form').addEventListener('submit', event => {
  event.preventDefault(); if (!scheduledJobs || !connected || readOnly || pendingCreate) return;
  task(async () => {
    const name = $('job-name').value.trim(), prompt = $('job-prompt').value.trim();
    if (!name || new TextEncoder().encode(name).length > 128 || !prompt || new TextEncoder().encode(prompt).length > 32768) throw new Error('名称最多 128 UTF-8 字节，任务内容最多 32 KiB，且不能为空。');
    const tools = [$('job-clock').checked && 'datetime_now', $('job-json').checked && 'json_query'].filter(Boolean);
    if (!tools.length) throw new Error('至少选择一项允许的工具。');
    const schedule = $('job-kind').value === 'cron' ? {kind:'cron',expression:$('job-cron').value.trim(),timezone:$('job-timezone').value.trim()} : {kind:'interval',seconds:Number($('job-interval').value)};
    const spec = {name, prompt, schedule, enabled_tools:tools, timeout_secs:Number($('job-timeout').value)};
    await refreshJobsHealth(); if (jobsHealth !== 'running') throw new Error('调度器未运行，不能创建任务。');
    if (jobsMode === 'standalone') {
      pendingCreate = {id:createId(), spec}; $('job-create-tracking').hidden = false;
      $('job-create-id').textContent = `创建 ID：${pendingCreate.id}`; $('job-create-state').textContent = '正在提交；请保留此 ID，用于结果核对。'; controls();
      await submitCreation();
    } else {
      const job = await jobsApi('/api/jobs', 'POST', spec);
      $('job-form').reset(); $('job-create').open = false; $('job-cron-fields').hidden = true; $('job-interval-field').hidden = false; $('job-interval').required = true;
      await refreshJobs(0, []); await selectJob(job.id); status('任务已创建并启用');
    }
  });
});
const deliveryStates = {pending:'等待发送',submitting:'正在提交',retry_wait:'等待重试',delivered:'已送达',unknown:'结果未知',permanent_failed:'发送失败',expired:'已过期',cancelled:'已取消'};
const deliveryState = value => deliveryStates[value] || '状态异常';
const outboxApi = (path, method = 'GET', body) => api(path, method, body, false, 30000);
function deliveryRecord(record, expectedId) {
  if (!record || typeof record.id !== 'string' || !/^[0-9a-f-]{36}$/.test(record.id) || (expectedId && record.id !== expectedId) || !Object.hasOwn(deliveryStates, record.state) || !record.destination || typeof record.destination.channel !== 'string' || typeof record.text !== 'string' || !Number.isInteger(record.ordinal) || record.ordinal < 0 || !(record.event_id && !record.job_id && !record.job_run_id || !record.event_id && record.job_id && record.job_run_id)) throw new Error('投递记录响应异常，请刷新后核对。');
  return record;
}
function renderOutbox() {
  $('outbox-list').replaceChildren();
  for (const row of deliveries) {
    const button = document.createElement('button');
    button.dataset.deliveryId = row.id; button.setAttribute('aria-current', String(row.id === delivery?.id));
    button.textContent = `${row.destination.channel} · ${deliveryState(row.state)} · ${row.event_id ? '入站回复' : '定时通知'} · 片段 ${row.ordinal + 1}\n${clipped(row.destination.conversation_id,256)} · ${row.id}`;
    button.addEventListener('click', () => task(async () => { await selectDelivery(row.id); status('投递记录已加载'); }));
    $('outbox-list').append(button);
  }
  $('outbox-count').textContent = deliveries.length ? `第 ${outboxOffset + 1}–${outboxOffset + deliveries.length} 条 · 按创建顺序，翻页时记录可能变化` : '此页没有投递记录';
  controls();
}
async function refreshOutbox(targetOffset = outboxOffset) {
  const health = await outboxApi('/api/channels/status');
  if (!health || !['running','failed','stopping','disabled'].includes(health.state)) throw new Error('渠道状态响应异常');
  $('outbox-health').textContent = `渠道服务：${{running:'运行中',failed:'异常，请检查服务器',stopping:'停止中',disabled:'已关闭，历史记录仍可核对'}[health.state]}`;
  const records = await outboxApi(`/api/channels/deliveries?limit=5&offset=${targetOffset}`);
  if (!Array.isArray(records) || records.length > 5) throw new Error('投递分页响应异常');
  const validated = records.map(record => deliveryRecord(record));
  deliveries = validated; outboxOffset = targetOffset;
  outboxNext = records.length === 5 && outboxOffset + 5 < 10000; renderOutbox();
}
function renderDelivery() {
  if (!delivery) return;
  const row = delivery, target = row.destination;
  $('delivery-detail').hidden = false;
  $('delivery-title').textContent = `${deliveryState(row.state)} · ${row.id}`;
  $('delivery-summary').textContent = clipped([
    `渠道：${target.channel} · 安装：${target.installation_id}`,
    `目的地：${target.conversation_id}${target.thread_id ? ' · 线程：' + target.thread_id : ''}`,
    row.event_id ? `入站事件：${row.event_id}` : `任务：${row.job_id}\n运行：${row.job_run_id}`,
    `片段：${row.ordinal + 1} · 已尝试：${row.attempts} 次`,
    `创建：${date(row.created_ms)} · 开始：${date(row.started_ms)} · 完成：${date(row.finished_ms)}`,
    ['pending','retry_wait'].includes(row.state) ? `记录最早尝试时间：${date(row.next_attempt_ms)}（安装限流、前序投递或权限仍可能阻止发送）` : ''
  ].filter(Boolean).join('\n'), 4096);
  $('delivery-content').textContent = clipped(row.text, 16384);
  $('delivery-receipt').textContent = clipped(row.receipt || '尚无回执', 4096);
  $('delivery-error').textContent = clipped(row.error || '无', 4096); controls();
}
async function selectDelivery(id, preserve = false) {
  const previousFresh = deliveryFresh;
  deliveryFresh = false; controls();
  if (!preserve && id !== delivery?.id) {
    if ($('delivery-evidence').value && !confirm('切换记录会清除当前证据草稿，是否继续？')) { deliveryFresh = previousFresh; controls(); return; }
    delivery = null; $('delivery-detail').hidden = true; $('delivery-evidence').value = '';
  }
  try {
    delivery = deliveryRecord(await outboxApi(`/api/channels/deliveries/${encodeURIComponent(id)}`), id);
    deliveryFresh = true; renderDelivery(); renderOutbox();
  } catch (error) {
    if (!(error instanceof StaleIdentity) && !(error instanceof ApiError && error.code === 401)) deliveryFresh = false;
    throw error;
  }
}
$('outbox-lookup').addEventListener('submit', event => {
  event.preventDefault(); if (!outboxEnabled || !connected) return;
  task(async () => {
    const id = $('outbox-id').value.trim();
    if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id) || id === '00000000-0000-0000-0000-000000000000') throw new Error('请输入规范的小写完整投递 UUID。');
    await selectDelivery(id); status('投递记录已加载');
  });
});
$('outbox-tab').addEventListener('click', () => task(async () => { if (!outboxEnabled) return; showView('outbox'); await refreshOutbox(); status('发件箱已加载'); }));
$('outbox-refresh').addEventListener('click', () => task(async () => { await refreshOutbox(); if (delivery) await selectDelivery(delivery.id, true); status('发件箱已更新'); }));
$('outbox-next').addEventListener('click', () => task(async () => { if (outboxNext) await refreshOutbox(outboxOffset + 5); }));
$('outbox-prev').addEventListener('click', () => task(async () => { await refreshOutbox(Math.max(0, outboxOffset - 5)); }));
$('delivery-refresh').addEventListener('click', () => task(async () => { if (delivery) { await selectDelivery(delivery.id, true); status('已读取服务器当前状态，请核对回执后再操作'); } }));
async function resolveDelivery(action) {
  if (!connected || !outboxEnabled || !delivery || !deliveryFresh) return;
  const row = delivery, owner = identity;
  const receipt = $('delivery-evidence').value;
  if (action === 'delivered' && (row.state !== 'unknown' || !receipt.trim() || new TextEncoder().encode(receipt).length > 4096 || /[\p{Cc}]/u.test(receipt))) throw new Error('仅未知投递可确认送达；请填写 1–4096 UTF-8 字节的证据，不允许换行或控制字符。');
  if (action === 'cancel' && !['pending','retry_wait','unknown','permanent_failed','expired'].includes(row.state)) return;
  const scope = row.event_id ? `入站事件 ${row.event_id}` : `任务运行 ${row.job_run_id}`;
  const warning = action === 'delivered' ? '已在渠道平台核实这一片段确实送达？确认后可能释放后续片段或其他被阻挡的投递，不会恢复已暂停的任务。' : '确认取消此来源的全部未送达片段？不会撤回已送达消息，也不会恢复已暂停的任务。';
  if (!confirm(`${warning}\n${scope}\n目的地：${row.destination.conversation_id}\n投递：${row.id}`)) return;
  deliveryFresh = false; controls();
  try {
    await outboxApi(`/api/channels/deliveries/${encodeURIComponent(row.id)}/resolve`, 'POST', {action, ...(action === 'delivered' ? {receipt} : {})});
  } catch (error) {
    if (owner !== identity) throw error;
    let refreshed = false;
    try { await selectDelivery(row.id, true); await refreshOutbox(); refreshed = true; } catch (refreshError) { if (owner !== identity) throw refreshError; deliveryFresh = false; }
    if (error.code === 409) status(`记录状态已变化或仍在提交，操作被拒绝。${refreshed ? '已刷新服务器状态；证据草稿保留，请重新核对。' : '读取失败，请刷新记录后核对。'}`, true);
    else status(`核对请求未能确认结果，勿直接重复提交。${refreshed ? '已读取服务器当前状态，请先核对状态和回执；证据草稿保留。' : '读取失败，操作已禁用；请刷新此记录。'} ${clipped(error.message,300)}`, true);
    return;
  }
  // An acknowledgement alone never substitutes for the authoritative record.
  try {
    await selectDelivery(row.id, true); await refreshOutbox();
    $('delivery-evidence').value = '';
    status(action === 'delivered' ? '人工送达证据已保存，服务器记录已刷新' : '此来源的剩余投递已取消，服务器记录已刷新');
  } catch (error) {
    if (owner !== identity) throw error;
    deliveryFresh = false;
    status('服务器已确认核对操作，但最新记录读取失败；证据草稿保留，请刷新此记录。', true);
  }
}
$('delivery-form').addEventListener('submit', event => { event.preventDefault(); task(() => resolveDelivery('delivered')); });
$('delivery-cancel').addEventListener('click', () => task(() => resolveDelivery('cancel')));

// A single original-turn attempt per connected identity. The fragment retains only
// its non-secret UUID across reload; credentials, prompts and permissions stay in memory.
const Turn = JiaClawTurnStream;
let turnCapabilities = null, pendingTurn = null, turnConnection = null, turnControlBusy = false;
let turnCatalogPage = null;
let resumeTurnId = /^#turn=([0-9a-f-]{36})$/.exec(location.hash)?.[1];
if (!Turn.uuid.test(resumeTurnId || '')) resumeTurnId = null;
const trackedSession = () => typeof selected === 'string' && selected.startsWith('http:');
const tenantTurns = () => turnCapabilities?.scope === 'gateway';
const turnWritable = () => !readOnly && turnCapabilities?.enabled && (tenantTurns() || turnCapabilities?.streaming);
const turnResolved = () => pendingTurn?.fresh && !pendingTurn.active && (pendingTurn.receipt?.state === 'completed' || pendingTurn.receipt?.reviewed_ms != null);
function turnControls() {
  $('turn-workspace').hidden = !connected || !turnCapabilities || (readOnly && !tenantTurns());
  $('turn-permissions').hidden = !trackedSession() || !turnWritable();
  $('chat-note').textContent = trackedSession() ? (turnCapabilities?.streaming ? '实时文字为临时预览；按原编号核对已保存结果。' : '回复按原编号保存在服务端；本页观察结束后仍可核对。') : '回复完成后显示；支持工具调用。';
  const held = !!pendingTurn;
  $('new-session').disabled ||= held;
  $('delete-session').disabled ||= held || (tenantTurns() && trackedSession());
  $('message').readOnly = held;
  $('message').disabled ||= trackedSession() && !turnWritable();
  $('send').disabled ||= held || (trackedSession() && !turnWritable());
  for (const element of $('turn-permissions').querySelectorAll('input')) element.disabled = busy || held || readOnly;
  $('turn-tracking').hidden = !held;
  $('turn-check').disabled = !held || turnControlBusy;
  $('turn-cancel').disabled = !held || readOnly || turnControlBusy || !turnCapabilities?.enabled || pendingTurn?.receipt?.state !== 'running';
  $('turn-retry').disabled = !held || turnControlBusy || busy || !!turnConnection || !pendingTurn?.body || !turnWritable() || !!pendingTurn?.receipt;
  $('turn-finish').disabled = !turnResolved() || !!turnConnection || turnControlBusy;
  const review = pendingTurn?.receipt?.state === 'needs_review' || (pendingTurn?.receipt?.state === 'running' && !pendingTurn?.active);
  $('turn-review').hidden = tenantTurns() || !review;
  $('turn-gateway-review').hidden = !tenantTurns() || !review;
  $('turn-abandon').disabled = !held || tenantTurns() || readOnly || !pendingTurn?.fresh || pendingTurn?.active || turnControlBusy || !!turnConnection;
  $('turn-lookup-id').disabled = !connected || !!pendingTurn || busy;
  $('turn-lookup').querySelector('button').disabled = !connected || !!pendingTurn || busy;
  const catalogDisabled = !connected || busy || turnControlBusy || turnCapabilities?.listing !== true;
  $('turn-catalog').hidden = turnCapabilities?.listing !== true || (readOnly && !tenantTurns());
  $('turn-catalog-filter').disabled = catalogDisabled || tenantTurns();
  $('turn-catalog-filter').hidden = tenantTurns(); $('turn-catalog-filter-label').hidden = tenantTurns();
  $('turn-catalog-refresh').disabled = catalogDisabled;
  $('turn-catalog-prev').disabled = catalogDisabled || !turnCatalogPage || turnCatalogPage.offset === 0;
  $('turn-catalog-next').disabled = catalogDisabled || !turnCatalogPage?.has_more || turnCatalogPage.offset + 5 > 10000;
  for (const button of $('turn-catalog-rows').querySelectorAll('button')) button.disabled = catalogDisabled || held;
}
function forgetTurns() {
  turnConnection?.abort(); turnConnection = null; turnControlBusy = false; pendingTurn = null; turnCapabilities = null;
  turnCatalogPage = null; $('turn-catalog').open = false; $('turn-catalog').hidden = true;
  $('turn-catalog-filter').value = 'unresolved'; $('turn-catalog-rows').replaceChildren(); $('turn-catalog-status').textContent = '';
  history.replaceState(null, '', location.pathname + location.search);
  for (const id of ['turn-tools','turn-skills','turn-state']) $(id).replaceChildren();
  for (const id of ['turn-id','turn-review-note','turn-lookup-id']) $(id).value = '';
  $('turn-workspace').hidden = true; $('turn-tracking').hidden = true; $('turn-permissions').open = false;
}
function trackTurn(id, body = null, session = null) {
  pendingTurn = {id, body, session_id:body?.session_id || session, receipt:null, active:false, fresh:false};
  if (tenantTurns()) { selected = body?.session_id || null; renderMessages([]); $('session-title').textContent = '原请求结果'; }
  history.replaceState(null, '', location.pathname + location.search + '#turn=' + id);
  $('turn-id').value = id; $('turn-review-note').value = ''; $('turn-review').open = false;
  $('turn-state').textContent = '尚未核对结果；请保留原编号，勿以新编号重复执行。'; controls();
  return pendingTurn;
}
const turnApi = (path, method = 'GET', body, timeout = 15000, signal = null) => api(path, method, body, false, timeout, 2 * 1024 * 1024 + 20 * 1024, signal);
function showTurnSnapshot(value, attempt) {
  (tenantTurns() ? Turn.gatewaySnapshot : Turn.snapshot)(value, attempt.receipt || attempt, attempt.body ? new Set(attempt.body.enabled_tools) : null);
  attempt.receipt = value.receipt; attempt.session_id = value.receipt.session_id; attempt.active = value.active; attempt.fresh = true;
  const r = value.receipt;
  $('turn-state').textContent = `${{running:'处理中',completed:'回复已保存',needs_review:'需要人工核对'}[r.state]} · ${value.active ? '实际执行仍占用资源' : '当前无活跃执行'}${r.cancel_requested ? ' · 已记录停止请求' : ''}${r.session_committed && r.state !== 'completed' ? ' · 会话已提交，结果仍需核对' : ''}${r.reviewed_ms != null ? ' · 已记录人工核对' : ''}${r.result_purged ? ' · 结果正文已清理' : ''}`;
  controls(); return value;
}
function showGatewayResult(attempt) {
  const r = attempt.receipt; selected = r.session_id;
  $('session-title').textContent = '原请求结果'; showView('chat');
  const messages = attempt.body ? [{role:'user',content:attempt.body.prompt}] : [];
  if (r.state === 'completed' && r.result) messages.push({role:'assistant',content:r.result.reply});
  renderMessages(messages);
  $('turn-state').textContent += r.state === 'completed' && r.result ? ' · 显示原收据中的回复，未读取或重建会话历史' : ' · 未取得可显示的已完成回复';
  if (r.state === 'completed' && r.result && attempt.body && $('message').value.trim() === attempt.body.prompt) $('message').value = '';
}
async function checkTurn(attempt = pendingTurn, timeout = 15000, signal = null) {
  if (!attempt) return;
  const owner = identity;
  const value = await turnApi('/api/turns/' + attempt.id, 'GET', undefined, timeout, signal);
  if (owner !== identity || pendingTurn !== attempt) throw new StaleIdentity();
  showTurnSnapshot(value, attempt);
  if (tenantTurns()) showGatewayResult(attempt);
  else if (value.receipt.session_committed) {
    const historyRead = await select(value.receipt.session_id, true); await refresh();
    if (!historyRead) $('turn-state').textContent += ' · 会话历史不可读取；原收据保留，未恢复历史';
    // Only a verified original receipt plus authoritative stored history clears a draft.
    if (historyRead && value.receipt.state === 'completed' && attempt.body && $('message').value.trim() === attempt.body.prompt) $('message').value = '';
  }
  status($('turn-state').textContent, value.receipt.state === 'needs_review');
}
async function turnControl(fn) {
  if (turnControlBusy || !pendingTurn) return;
  const owner = identity, attempt = pendingTurn; turnControlBusy = true; controls();
  try { await fn(attempt); }
  catch (error) { if (owner === identity && !(error instanceof StaleIdentity)) { attempt.fresh = false; status(error.message + ' 请继续按原编号核对。', true); } }
  finally { if (owner === identity && pendingTurn === attempt) { turnControlBusy = false; controls(); } }
}
function renderPermissionList(id, records) {
  // Catalog size and per-turn authorization are separate bounds. Installed
  // skills beyond the submission limit must remain individually selectable.
  if (!Array.isArray(records) || records.length > 4096 || records.some(r => !r || typeof r.name !== 'string' || !r.name || Turn.bytes(r.name) > 128 || typeof r.description !== 'string') || new Set(records.map(r => r.name)).size !== records.length) throw new Error('工具或技能注册表异常或超过 4096 项；未授权任何工具。');
  $(id).replaceChildren();
  for (const record of records) {
    const label = document.createElement('label'), input = document.createElement('input'), span = document.createElement('span');
    input.type = 'checkbox'; input.value = record.name; span.textContent = record.name; label.title = clipped(record.description, 512);
    label.append(input, span); $(id).append(label);
  }
}
async function connectTurns(gateway) {
  // A missing gateway route grants no standalone authority. Its authenticated
  // capabilities endpoint is required independently; read-only keys never probe it.
  if (!token || (readOnly && !gateway)) return;
  const c = await api('/api/turns/capabilities', 'GET', undefined, gateway ? [404] : [403,404,503], 15000, 16384);
  if (!c) return;
  turnCapabilities = gateway ? Turn.gatewayCapabilities(c) : Turn.capabilities(c);
  $('turn-mode').textContent = gateway ? (c.streaming ? '新请求显示临时预览，结束后按原编号核对已保存回复；断流或停止交付后仍需核对，结束跟踪不解除管理员核对占用。' : '新请求异步保存回复。本页最多观察 30 秒，之后可按原编号核对；结束跟踪不解除需要管理员核对的执行占用。') : '新建会话使用实时预览。旧会话保持原有发送方式；预览不代表已保存。';
  $('turn-catalog-note').textContent = gateway ? '列表只含本用户的原编号、会话编号和准入时间，不表示执行状态。选中后读取原收据；不会再次执行。新记录可能改变后续分页。' : '列表仅展示状态摘要。选中后核对原请求；不会再次执行。每页为读取时的状态，新记录可能改变后续分页。';
  $('turn-permissions-note').textContent = gateway ? '请选择本次需要的时间和 JSON 查询工具；默认不授权。' : '工具可能写文件或产生外部效果。未勾选的工具和技能不会获得授权。注册表变化后请重新连接。';
  if (gateway && !readOnly) {
    renderPermissionList('turn-tools', [{name:'datetime_now',description:'读取当前时间'},{name:'json_query',description:'查询 JSON'}]); renderPermissionList('turn-skills', []);
  } else if (!gateway && c.streaming) {
    const tools = await api('/api/tools', 'GET', undefined, false, 15000, 2 * 1024 * 1024);
    const skills = await api('/api/skills', 'GET', undefined, false, 15000, 2 * 1024 * 1024);
    renderPermissionList('turn-tools', tools.tools); renderPermissionList('turn-skills', skills.skills);
  }
}
async function loadTurnCatalog(offset = 0) {
  const owner = identity, expected = {state:$('turn-catalog-filter').value,limit:5,offset};
  // Clear old clickable rows before any new fetch/validation, including failures.
  turnCatalogPage = null; $('turn-catalog-rows').replaceChildren(); $('turn-catalog-status').textContent = '正在读取状态摘要…'; controls();
  let value;
  try {
    value = await api(`/api/turns?${tenantTurns() ? '' : 'state='+expected.state+'&'}limit=5&offset=${offset}`, 'GET', undefined, false, 15000, 32768);
    if (owner !== identity) throw new StaleIdentity();
    (tenantTurns() ? Turn.gatewayCatalog : Turn.catalog)(value, expected);
  } catch (error) {
    if (owner === identity) $('turn-catalog-status').textContent = '列表未通过核对；可按已知原编号单独查找。';
    throw error;
  }
  turnCatalogPage = value;
  const rows = tenantTurns() ? value.requests : value.turns;
  $('turn-catalog-status').textContent = rows.length ? `第 ${offset + 1}–${offset + rows.length} 项${tenantTurns() ? '准入编号' : '状态摘要'}；选中后读取原收据。` : '本页暂无请求；可更换范围或重新读取。';
  for (const row of rows) {
    const article = document.createElement('article'), description = document.createElement('p'), button = document.createElement('button');
    article.className = 'turn-catalog-row'; description.textContent = `${tenantTurns() ? '已保留原编号；执行状态须另行核对' : ({running:'处理中',completed:'回复已保存',needs_review:'需要人工核对'}[row.state] + (row.reviewed_ms !== null ? ' · 已记录核对' : '') + (row.result_purged ? ' · 正文已清理' : ''))}\n${row.id}\n${new Date(row.created_ms).toLocaleString()}`;
    button.type = 'button'; button.className = 'subtle'; button.textContent = '核对原请求'; button.dataset.turnId = row.id;
    button.addEventListener('click', () => {
      if (owner !== identity || pendingTurn || busy || turnControlBusy) return;
      // The page summary never marks a turn resolved or releases its session.
      trackTurn(row.id, null, row.session_id); turnControl(attempt => checkTurn(attempt));
    });
    article.append(description,button); $('turn-catalog-rows').append(article);
  }
  controls();
}
$('turn-catalog-refresh').addEventListener('click', () => task(() => loadTurnCatalog()));
$('turn-catalog-filter').addEventListener('change', () => task(() => loadTurnCatalog()));
$('turn-catalog-prev').addEventListener('click', () => task(() => loadTurnCatalog(Math.max(0, (turnCatalogPage?.offset || 0) - 5))));
$('turn-catalog-next').addEventListener('click', () => task(() => loadTurnCatalog((turnCatalogPage?.offset || 0) + 5)));
// Bound the displayed draft independently of the larger validated wire budget.
// One article per round, 64Ki characters each / 256Ki total, batched every frame.
function previewRenderer(owner, attempt) {
  let current = null, total = 0, frame = null;
  const flush = () => { frame = null; if (owner === identity && pendingTurn === attempt && current) { current.node.textContent = current.text + (current.truncated ? '\n[预览显示已截断；请核对服务端最终记录]' : ''); $('messages').scrollTop = $('messages').scrollHeight; } };
  const renderer = value => {
    if (value.event === 'model_started') {
      if (frame !== null) { cancelAnimationFrame(frame); flush(); }
      const article = document.createElement('article'), role = document.createElement('div'), node = document.createElement('p');
      article.className = 'message provisional'; role.className = 'role'; role.textContent = `临时预览 · 第 ${value.round + 1} 轮 · 尚未保存`;
      article.append(role, node); $('messages').append(article); current = {node, text:'', truncated:false};
    } else if (value.event === 'preview') {
      const available = Math.max(0, Math.min(65536 - current.text.length, 262144 - total));
      current.text += value.text.slice(0, available); total += Math.min(available, value.text.length); current.truncated ||= value.text.length > available;
      if (frame === null) frame = requestAnimationFrame(flush);
    } else if (value.event === 'tool_completed') $('turn-state').textContent = `工具 ${value.tool_name} 已返回；回复仍未确认保存。`;
  };
  renderer.close = () => { if (frame !== null) { cancelAnimationFrame(frame); flush(); } };
  return renderer;
}
const submitTurn = attempt => turnCapabilities?.streaming ? streamTurn(attempt) : jsonTurn(attempt);
async function jsonTurn(attempt) {
  const owner = identity, controller = new AbortController(); turnConnection = controller; attempt.fresh = false;
  // This finite observation window covers the initial PUT and all subsequent GETs.
  // It neither cancels detached execution nor releases its server-side capacity.
  const deadline = performance.now() + 30000;
  let observationLimited = false;
  const remaining = () => {
    const left = deadline - performance.now(); observationLimited = left <= 15000;
    return Math.max(1, Math.min(15000, Math.floor(left)));
  };
  try {
    status('正在提交原请求…');
    const value = await turnApi('/api/turns/' + attempt.id, 'PUT', attempt.body, remaining());
    if (owner !== identity || pendingTurn !== attempt) throw new StaleIdentity();
    showTurnSnapshot(value, attempt); showGatewayResult(attempt);
    while (!controller.signal.aborted && performance.now() < deadline) {
      if (owner !== identity || pendingTurn !== attempt) throw new StaleIdentity();
      if (attempt.fresh && !attempt.active && attempt.receipt?.state !== 'running') { status($('turn-state').textContent, attempt.receipt.state === 'needs_review'); return; }
      await new Promise(resolve => {
        const done = () => { clearTimeout(timer); controller.signal.removeEventListener('abort', done); resolve(); };
        const timer = setTimeout(done, Math.min(500, Math.max(1, deadline - performance.now())));
        controller.signal.addEventListener('abort', done, {once:true});
        if (controller.signal.aborted) done();
      });
      if (!controller.signal.aborted && performance.now() < deadline && !turnControlBusy) await checkTurn(attempt, remaining());
    }
    if (owner === identity && pendingTurn === attempt) status('本页观察已结束；原执行可能继续。请按原编号核对结果，勿重复执行。');
  } catch (error) {
    if (owner !== identity || pendingTurn !== attempt) throw new StaleIdentity();
    attempt.fresh = false;
    if (error instanceof ApiTimeout && observationLimited) {
      $('turn-state').textContent = '本页观察已结束；最后一次读取未确认，保留原编号和草稿，核对服务端结果。';
      status('本页观察已结束；原执行可能继续。请按原编号核对结果，勿重复执行。'); return;
    }
    $('turn-state').textContent = '交付未确认；保留原编号和草稿，核对服务端结果。';
    throw new Error(error.message + ' 请核对原请求，勿重复执行。');
  } finally {
    controller.abort();
    if (owner === identity && turnConnection === controller) { turnConnection = null; controls(); }
  }
}
async function streamTurn(attempt) {
  const owner = identity, credential = token, controller = new AbortController(); turnConnection = controller; attempt.fresh = false;
  // One browser transport deadline anchored before fetch, never reset by headers,
  // keepalives or events. It does not replace the server/current-model deadline.
  const deadline = performance.now() + (turnCapabilities.turn_budget_secs + 70) * 1000;
  const timer = setTimeout(() => controller.abort(), Math.max(1, deadline - performance.now()));
  const reconcile = () => {
    const left = deadline - performance.now();
    if (left <= 0 || controller.signal.aborted) throw new ApiTimeout('交付期限已到；请核对原请求。');
    return checkTurn(attempt, Math.max(1, Math.min(15000, Math.floor(left))), controller.signal);
  };
  let reader = null, done = false; const render = previewRenderer(owner, attempt);
  try {
    status('正在读取临时预览…');
    const response = await fetch('/api/turns/' + attempt.id + '/stream', {method:'PUT', signal:controller.signal, credentials:'omit', cache:'no-store', headers:{Authorization:'Bearer ' + credential, 'Content-Type':'application/json'}, body:JSON.stringify(attempt.body)});
    if (owner !== identity || pendingTurn !== attempt) throw new StaleIdentity();
    if (response.status === 401) { clearIdentity(); throw new ApiError('鉴权失败，请重新连接并检查 API Token。',401); }
    if (response.status === 200) {
      const value = await boundedJson(response, 2 * 1024 * 1024 + 20 * 1024, owner);
      showTurnSnapshot(value, attempt); render.close(); await reconcile(); return;
    }
    if (response.status !== 202) {
      const value = await boundedJson(response, 16384, owner);
      throw new ApiError(clipped(value?.error || `请求失败（HTTP ${response.status}）`,1024), response.status);
    }
    if (response.headers.get('content-type')?.toLowerCase() !== 'text/event-stream; charset=utf-8') throw new Error('流式响应类型异常');
    const parser = new Turn.Parser(attempt, attempt.body.enabled_tools, value => {
      if (owner !== identity || pendingTurn !== attempt) throw new StaleIdentity();
      if (tenantTurns() && ['admitted','done'].includes(value.event)) Turn.gatewaySnapshot({protocol:value.protocol,receipt:value.receipt,active:value.event === 'admitted'}, attempt.receipt || attempt);
      if (value.event === 'admitted') { attempt.receipt = value.receipt; attempt.active = true; $('turn-state').textContent = '已准入原请求；临时预览尚未保存。'; controls(); }
      else if (value.event === 'done') { attempt.receipt = value.receipt; done = true; controls(); }
      else if (value.event === 'error') throw new Error('交付停止，原模型或工具可能仍在结算。');
      else render(value);
    });
    reader = response.body?.getReader(); if (!reader) throw new Error('流式正文缺失');
    while (true) {
      const next = await reader.read(); if (owner !== identity || pendingTurn !== attempt) throw new StaleIdentity();
      if (next.done) break; parser.push(next.value);
    }
    parser.finish(); render.close();
    if (!done) throw new Error('未收到完整已提交结果');
    await reconcile();
  } catch (error) {
    if (owner !== identity) { if (error instanceof ApiError && error.code === 401) throw error; throw new StaleIdentity(); }
    attempt.fresh = false;
    $('turn-state').textContent = '交付未确认；保留原编号和草稿，核对服务端结果。停止交付不证明模型或工具已停止。';
    throw new Error((error.name === 'AbortError' ? '交付已停止或超时。' : error.message) + ' 请核对原请求，勿重复执行。');
  } finally {
    clearTimeout(timer); render.close(); controller.abort();
    if (reader) { await reader.cancel().catch(() => {}); reader.releaseLock(); }
    if (owner === identity && turnConnection === controller) { turnConnection = null; controls(); }
  }
}
$('turn-lookup').addEventListener('submit', event => {
  event.preventDefault(); const id = $('turn-lookup-id').value.trim();
  if (pendingTurn || !Turn.uuid.test(id)) { status('请输入规范 UUIDv4 原编号。',true); return; }
  trackTurn(id); turnControl(attempt => checkTurn(attempt));
});
$('turn-check').addEventListener('click', () => turnControl(attempt => checkTurn(attempt)));
$('turn-cancel').addEventListener('click', () => turnControl(async attempt => {
  const value = await turnApi('/api/turns/' + attempt.id + '/cancel', 'POST');
  showTurnSnapshot(value, attempt);
  // A terminal commit may win this race. Report only the persisted intent,
  // and reconcile its stored history before allowing the draft to be cleared.
  if (value.receipt.state !== 'running') {
    await checkTurn(attempt);
    status($('turn-state').textContent + (value.receipt.cancel_requested ? ' · 已记录停止请求' : ' · 本次未新增停止意图'), value.receipt.state === 'needs_review');
  } else if (value.receipt.cancel_requested) {
    turnConnection?.abort(); status('已记录停止请求；当前模型或工具可能继续结算，请核对原编号。');
  } else {
    throw new Error('服务端未确认停止意图；请核对原请求。');
  }
}));
$('turn-retry').addEventListener('click', () => {
  const attempt = pendingTurn;
  if (!attempt?.body || attempt.receipt || busy || !confirm('仅使用同一个原编号、原内容和原权限提交。不会换编号。继续？')) return;
  task(() => submitTurn(attempt));
});
$('turn-abandon').addEventListener('click', () => turnControl(async attempt => {
  if (tenantTurns() || readOnly) return;
  const note = $('turn-review-note').value.trim();
  if (!note || Turn.bytes(note) > 1024) throw new Error('核对记录需要 1–1024 UTF-8 字节');
  if (!confirm('已核对模型账本和实际效果？这只解除会话占用，不撤销或恢复未知操作。')) return;
  const value = await turnApi('/api/turns/' + attempt.id + '/review', 'POST', {decision:'abandon', note});
  showTurnSnapshot(value, attempt); status('已记录人工核对；模型账本的未知占用仍须单独处理。');
}));
$('turn-finish').addEventListener('click', () => {
  if (!turnResolved() || turnConnection) return;
  if (pendingTurn.body && $('message').value.trim() === pendingTurn.body.prompt) $('message').value = '';
  pendingTurn = null; history.replaceState(null,'',location.pathname + location.search); controls();
});
addEventListener('pagehide', () => turnConnection?.abort());

controls();
