'use strict';
const $ = id => document.getElementById(id);
const MISSING_ROUTE = Symbol('missing-route');
let token = '', selected = null, connected = false, busy = false, readOnly = false, sessionList = [];
let identity = 0, operation = 0, scheduledJobs = false, jobList = [], selectedJob = null;
let jobsOffset = 0, jobsNext = null, jobsPrevious = [], runsOffset = 0, runsNext = null, runsPrevious = [];
let jobsMode = null, jobsHealth = null, pendingCreate = null, jobsIncludeDeleted = false;
let outboxEnabled = false, deliveries = [], delivery = null, outboxOffset = 0, outboxNext = false, deliveryFresh = false;
class StaleIdentity extends Error {}
class ApiError extends Error { constructor(message, code) { super(message); this.code = code; } }
const clipped = (text, length) => { text = String(text || ''); return text.length > length ? text.slice(0, length) + '\n[显示已截断；完整记录保留在服务端]' : text; };
const date = value => value == null ? '—' : new Date(value).toLocaleString();
function status(text, error = false) { $('status').textContent = text; $('status').classList.toggle('error', error); }
function controls() {
  $('new-session').disabled = !connected || readOnly || busy;
  $('access-mode').hidden = !connected || !readOnly;
  $('job-create').hidden = readOnly;
  $('refresh').disabled = !connected || busy;
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
}
function clearIdentity() {
  identity++; token = ''; connected = false; readOnly = false; selected = null; sessionList = [];
  $('access-mode').hidden = true; $('job-create').hidden = false;
  scheduledJobs = false; jobList = []; selectedJob = null; jobsMode = null; jobsHealth = null; pendingCreate = null;
  $('job-lookup').hidden = true; $('job-lookup-id').value = ''; $('job-create-tracking').hidden = true;
  outboxEnabled = false; deliveries = []; delivery = null; outboxOffset = 0; outboxNext = false; deliveryFresh = false;
  jobsOffset = 0; jobsNext = null; jobsPrevious = []; runsOffset = 0; runsNext = null; runsPrevious = [];
  $('api-token').value = ''; $('message').value = '';
  $('session-title').textContent = '开始一段对话';
  $('workspace-tabs').hidden = true; $('jobs-tab').hidden = true; $('outbox-tab').hidden = true; showView('chat');
  $('delivery-detail').hidden = true; $('delivery-evidence').value = ''; $('outbox-id').value = '';
  for (const id of ['outbox-list','outbox-count','outbox-health','delivery-title','delivery-summary','delivery-content','delivery-receipt','delivery-error']) $(id).replaceChildren();
  $('job-form').reset(); $('job-create').open = false; $('job-cron-fields').hidden = true; $('job-interval-field').hidden = false; $('job-interval').required = true;
  jobsIncludeDeleted = false; $('jobs-deleted').checked = false; $('job-detail').hidden = true;
  for (const id of ['jobs-list', 'job-runs', 'job-title', 'job-summary', 'job-detail-prompt', 'job-detail-contract', 'jobs-count', 'jobs-health', 'job-create-id', 'job-create-state']) $(id).replaceChildren();
  renderMessages([]); renderSessions();
}
async function api(path, method = 'GET', body, optional = false, timeout = 0, maxBytes = 0) {
  if (readOnly && method !== 'GET') throw new ApiError('当前密钥仅允许查看；修改内容或运行模型需要完整权限密钥。', 403);
  const owner = identity, credential = token, controller = new AbortController();
  const timer = timeout ? setTimeout(() => controller.abort(), timeout) : null;
  try {
    const response = await fetch(path, { method, signal: controller.signal, credentials: 'omit', cache: 'no-store', headers: { ...(credential ? { Authorization: `Bearer ${credential}` } : {}), ...(body ? { 'Content-Type': 'application/json' } : {}) }, ...(body ? { body: JSON.stringify(body) } : {}) });
    if (owner !== identity) throw new StaleIdentity();
    if (response.status === 401) { clearIdentity(); throw new ApiError('鉴权失败，请重新连接并检查 API Token。', 401); }
    if (optional === true && response.status === 404) return MISSING_ROUTE;
    if (Array.isArray(optional) && optional.includes(response.status)) return null;
    const reviewRequired = response.headers.get('x-jiaclaw-write-review') === 'required';
    const reviewMessage = '结果需要管理员核对。请保留当前内容，勿重复提交；确认后端空闲并处理未知结果后再继续。';
    if (response.status === 204) { if (reviewRequired) throw new Error(reviewMessage); return null; }
    let data;
    try { data = maxBytes ? await boundedJson(response, maxBytes, owner) : await response.json(); } catch (error) { if (owner !== identity) throw new StaleIdentity(); if (error instanceof ApiError) throw error; throw new ApiError(`服务响应异常（HTTP ${response.status}）`, response.status); }
    if (owner !== identity) throw new StaleIdentity();
    if (!response.ok) throw new ApiError(clipped(data?.error || `请求失败（HTTP ${response.status}）`, 1024), response.status);
    if (reviewRequired) throw new Error(reviewMessage);
    return data;
  } catch (error) {
    if (owner !== identity && !(error instanceof ApiError && error.code === 401)) throw new StaleIdentity();
    if (error.name === 'AbortError') throw new Error('请求超时；服务器可能仍在处理，请刷新并核对结果。');
    throw error;
  } finally { if (timer) clearTimeout(timer); }
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
      if (length > maximum) throw new ApiError('任务响应超过 2 MiB 上限；未跳过记录，请联系管理员核对。', response.status);
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
  for (const message of messages) {
    const article = document.createElement('article'); article.className = `message ${message.role === 'user' ? 'user' : 'assistant'}`;
    const role = document.createElement('div'); role.className = 'role'; role.textContent = { user: '你', assistant: 'JiaClaw', system: '会话上下文' }[message.role] || '消息';
    const text = document.createElement('p'); text.textContent = message.content;
    article.append(role, text); $('messages').append(article);
  }
  $('messages').scrollTop = $('messages').scrollHeight;
}
async function refresh() { sessionList = (await api('/api/sessions')).sessions; renderSessions(); }
async function select(id) {
  const session = await api(`/api/sessions/${encodeURIComponent(id)}`);
  selected = id; $('session-title').textContent = `会话 ${id.slice(0, 12)}`;
  showView('chat');
  renderMessages(session.messages); renderSessions(); status(readOnly ? '已连接 · 只读访问 · 会话就绪' : '已连接 · 会话就绪');
}
async function task(fn, replace = false) {
  if (busy && !replace) return;
  const current = ++operation;
  busy = true; controls();
  try { await fn(); } catch (error) { if (current === operation && !(error instanceof StaleIdentity)) status(error.message, true); }
  finally { if (current === operation) { busy = false; controls(); } }
}
$('connect-form').addEventListener('submit', event => {
  event.preventDefault(); const nextToken = $('api-token').value.trim(); clearIdentity(); token = nextToken;
  task(async () => {
    status('连接中…'); await refresh();
    const capabilities = await api('/api/gateway/capabilities', 'GET', undefined, true);
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
    connected = true; status(readOnly ? '已连接 · 只读访问 · 选择会话查看' : '已连接 · 选择或新建会话');
  }, true);
});
$('refresh').addEventListener('click', () => task(async () => { await refresh(); status('会话列表已更新'); }));
$('new-session').addEventListener('click', () => task(async () => {
  if (!connected || readOnly) return;
  const data = await api('/api/sessions', 'POST'); await refresh(); await select(data.session_id);
}));
$('delete-session').addEventListener('click', () => {
  if (!connected || readOnly || !selected || !confirm('删除这段会话及全部历史？此操作无法撤销。')) return;
  task(async () => { await api(`/api/sessions/${encodeURIComponent(selected)}`, 'DELETE'); selected = null; renderMessages([]); $('session-title').textContent = '开始一段对话'; await refresh(); status('会话已删除'); });
});
$('chat-form').addEventListener('submit', event => {
  event.preventDefault(); const text = $('message').value.trim(); if (!connected || readOnly || !text || !selected) return;
  task(async () => {
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
controls();
