'use strict';
const $ = id => document.getElementById(id);
let token = '', selected = null, connected = false, busy = false, sessionList = [];
let identity = 0, operation = 0, scheduledJobs = false, jobList = [], selectedJob = null;
let jobsOffset = 0, jobsNext = null, jobsPrevious = [], runsOffset = 0, runsNext = null, runsPrevious = [];
class StaleIdentity extends Error {}
const clipped = (text, length) => { text = String(text || ''); return text.length > length ? text.slice(0, length) + '\n[显示已截断；完整记录保留在服务端]' : text; };
const date = value => value == null ? '—' : new Date(value).toLocaleString();
function status(text, error = false) { $('status').textContent = text; $('status').classList.toggle('error', error); }
function controls() {
  $('new-session').disabled = !connected || busy;
  $('refresh').disabled = !connected || busy;
  $('delete-session').disabled = !connected || !selected || busy;
  $('message').disabled = !connected || !selected || busy;
  $('send').disabled = !connected || !selected || busy;
  for (const button of $('sessions').querySelectorAll('button')) button.disabled = !connected || busy;
  for (const element of $('jobs-view').querySelectorAll('button,input,textarea,select')) element.disabled = !connected || !scheduledJobs || busy;
  $('jobs-prev').disabled ||= jobsPrevious.length === 0;
  $('jobs-next').disabled ||= jobsNext === null;
  $('runs-prev').disabled ||= runsPrevious.length === 0;
  $('runs-next').disabled ||= runsNext === null;
  $('job-toggle').disabled ||= !selectedJob || selectedJob.deleted;
  $('job-delete').disabled ||= !selectedJob || selectedJob.deleted;
  $('runs-refresh').disabled ||= !selectedJob;
  $('chat-tab').disabled = busy;
  $('jobs-tab').disabled = busy;
}
function clearIdentity() {
  identity++; token = ''; connected = false; selected = null; sessionList = [];
  scheduledJobs = false; jobList = []; selectedJob = null;
  jobsOffset = 0; jobsNext = null; jobsPrevious = []; runsOffset = 0; runsNext = null; runsPrevious = [];
  $('api-token').value = ''; $('message').value = '';
  $('session-title').textContent = '开始一段对话';
  $('workspace-tabs').hidden = true; $('jobs-view').hidden = true; $('chat-view').hidden = false;
  $('chat-tab').setAttribute('aria-pressed', 'true'); $('jobs-tab').setAttribute('aria-pressed', 'false');
  $('job-form').reset(); $('job-create').open = false; $('job-cron-fields').hidden = true; $('job-interval-field').hidden = false; $('job-interval').required = true;
  $('jobs-deleted').checked = false; $('job-detail').hidden = true;
  for (const id of ['jobs-list', 'job-runs', 'job-title', 'job-summary', 'job-detail-prompt', 'jobs-count']) $(id).replaceChildren();
  renderMessages([]); renderSessions();
}
async function api(path, method = 'GET', body, optional = false) {
  const owner = identity, credential = token;
  const response = await fetch(path, { method, credentials: 'omit', cache: 'no-store', headers: { ...(credential ? { Authorization: `Bearer ${credential}` } : {}), ...(body ? { 'Content-Type': 'application/json' } : {}) }, ...(body ? { body: JSON.stringify(body) } : {}) });
  if (owner !== identity) throw new StaleIdentity();
  if (response.status === 401) { clearIdentity(); throw new Error('鉴权失败，请重新连接并检查 API Token。'); }
  if (optional && response.status === 404) return null;
  const reviewRequired = response.headers.get('x-jiaclaw-write-review') === 'required';
  const reviewMessage = '结果需要管理员核对。请保留当前内容，勿重复提交；确认后端空闲并处理未知结果后再继续。';
  if (response.status === 204) { if (reviewRequired) throw new Error(reviewMessage); return null; }
  let data;
  try { data = await response.json(); } catch { if (owner !== identity) throw new StaleIdentity(); throw new Error(`服务响应异常（HTTP ${response.status}）`); }
  if (owner !== identity) throw new StaleIdentity();
  if (!response.ok) throw new Error(data.error || `请求失败（HTTP ${response.status}）`);
  if (reviewRequired) throw new Error(reviewMessage);
  return data;
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
  $('chat-view').hidden = false; $('jobs-view').hidden = true;
  $('chat-tab').setAttribute('aria-pressed', 'true'); $('jobs-tab').setAttribute('aria-pressed', 'false');
  renderMessages(session.messages); renderSessions(); status('已连接 · 会话就绪');
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
    scheduledJobs = capabilities?.scheduled_jobs === true; $('workspace-tabs').hidden = !scheduledJobs;
    connected = true; status('已连接 · 选择或新建会话');
  }, true);
});
$('refresh').addEventListener('click', () => task(async () => { await refresh(); status('会话列表已更新'); }));
$('new-session').addEventListener('click', () => task(async () => {
  const data = await api('/api/sessions', 'POST'); await refresh(); await select(data.session_id);
}));
$('delete-session').addEventListener('click', () => {
  if (!connected || !selected || !confirm('删除这段会话及全部历史？此操作无法撤销。')) return;
  task(async () => { await api(`/api/sessions/${encodeURIComponent(selected)}`, 'DELETE'); selected = null; renderMessages([]); $('session-title').textContent = '开始一段对话'; await refresh(); status('会话已删除'); });
});
$('chat-form').addEventListener('submit', event => {
  event.preventDefault(); const text = $('message').value.trim(); if (!connected || !text || !selected) return;
  task(async () => {
    status('JiaClaw 正在处理…');
    await api('/api/chat', 'POST', { messages: [{ role: 'user', content: text }], session_id: selected, stream: false });
    $('message').value = ''; await select(selected); await refresh(); status('回复已保存');
  });
});
$('message').addEventListener('keydown', event => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); $('chat-form').requestSubmit(); } });
function page(data, offset) {
  if (!data || !Array.isArray(data.items) || data.items.length > 5 || !(data.next_offset === null || (Number.isInteger(data.next_offset) && data.next_offset > offset && data.next_offset <= 10000))) throw new Error('任务分页响应异常');
  return data;
}
function renderJobs() {
  $('jobs-list').replaceChildren();
  for (const job of jobList) {
    const button = document.createElement('button');
    button.textContent = `${job.spec.name} · ${job.deleted ? '已删除' : job.enabled ? '已启用' : '已暂停'}`;
    button.setAttribute('aria-current', String(job.id === selectedJob?.id));
    button.addEventListener('click', () => task(() => selectJob(job.id)));
    $('jobs-list').append(button);
  }
  $('jobs-count').textContent = jobList.length ? `本页 ${jobList.length} 项` : '没有任务'; controls();
}
async function refreshJobs() {
  const data = page(await api(`/api/jobs?limit=5&offset=${jobsOffset}&include_deleted=${$('jobs-deleted').checked}`), jobsOffset);
  jobList = data.items; jobsNext = data.next_offset; renderJobs();
}
async function selectJob(id) {
  selectedJob = await api(`/api/jobs/${encodeURIComponent(id)}`);
  const job = selectedJob; $('job-detail').hidden = false; $('job-title').textContent = job.spec.name;
  $('job-summary').textContent = `${job.deleted ? '已删除' : job.enabled ? '已启用' : '已暂停'} · 下次时间：${date(job.next_due_ms)} · ID：${job.id}`;
  $('job-detail-prompt').textContent = clipped(job.spec.prompt, 4096);
  $('job-toggle').textContent = job.enabled ? '暂停任务' : '恢复任务';
  runsOffset = 0; runsPrevious = []; await refreshRuns(); renderJobs();
}
async function refreshRuns() {
  if (!selectedJob) return;
  const data = page(await api(`/api/jobs/${encodeURIComponent(selectedJob.id)}/runs?limit=5&offset=${runsOffset}`), runsOffset);
  runsNext = data.next_offset; $('job-runs').replaceChildren();
  for (const run of data.items) {
    const article = document.createElement('article'); article.className = 'job-run';
    const title = document.createElement('h4'); title.textContent = `${date(run.scheduled_for_ms)} · ${{running:'运行中',completed:'已完成',failed:'失败，需核对',needs_review:'需核对',interrupted:'已中断，需核对',skipped:'已跳过'}[run.status] || run.status} · ${run.id}`;
    const text = document.createElement('pre'); text.textContent = clipped(run.response?.message?.content || run.error || '结果尚未提交。', 16384);
    article.append(title, text); $('job-runs').append(article);
  }
  if (!data.items.length) $('job-runs').textContent = '尚无运行记录。';
  controls();
}
$('chat-tab').addEventListener('click', () => { $('chat-view').hidden = false; $('jobs-view').hidden = true; $('chat-tab').setAttribute('aria-pressed','true'); $('jobs-tab').setAttribute('aria-pressed','false'); });
$('jobs-tab').addEventListener('click', () => task(async () => {
  if (!scheduledJobs) return;
  $('chat-view').hidden = true; $('jobs-view').hidden = false; $('chat-tab').setAttribute('aria-pressed','false'); $('jobs-tab').setAttribute('aria-pressed','true');
  await refreshJobs(); status('定时任务已加载');
}));
$('jobs-refresh').addEventListener('click', () => task(async () => { await refreshJobs(); if (selectedJob) await selectJob(selectedJob.id); status('任务已更新'); }));
$('jobs-deleted').addEventListener('change', () => task(async () => { jobsOffset = 0; jobsPrevious = []; await refreshJobs(); }));
for (const [prefix, refreshPage] of [['jobs', refreshJobs], ['runs', refreshRuns]]) {
  $(prefix+'-next').addEventListener('click', () => task(async () => { if (prefix === 'jobs') { jobsPrevious.push(jobsOffset); jobsOffset = jobsNext; } else { runsPrevious.push(runsOffset); runsOffset = runsNext; } await refreshPage(); }));
  $(prefix+'-prev').addEventListener('click', () => task(async () => { if (prefix === 'jobs') jobsOffset = jobsPrevious.pop(); else runsOffset = runsPrevious.pop(); await refreshPage(); }));
}
$('runs-refresh').addEventListener('click', () => task(refreshRuns));
$('job-toggle').addEventListener('click', () => task(async () => {
  const id = selectedJob.id;
  if (!selectedJob.enabled && !confirm('确认已核对上次运行和外部结果？恢复会安排新的运行，不会重放旧运行。')) return;
  await api(`/api/jobs/${encodeURIComponent(id)}/${selectedJob.enabled ? 'pause' : 'resume'}`, 'POST'); await refreshJobs(); await selectJob(id); status('任务状态已更新');
}));
$('job-delete').addEventListener('click', () => {
  if (!selectedJob || !confirm('删除任务并停止后续运行？运行记录会保留，已提交的工作可能继续。')) return;
  task(async () => { const id = selectedJob.id; await api(`/api/jobs/${encodeURIComponent(id)}`, 'DELETE'); await refreshJobs(); await selectJob(id); status('任务已删除，运行记录已保留'); });
});
$('job-kind').addEventListener('change', () => {
  const cron = $('job-kind').value === 'cron'; $('job-cron-fields').hidden = !cron; $('job-interval-field').hidden = cron; $('job-interval').required = !cron;
});
$('job-form').addEventListener('submit', event => {
  event.preventDefault(); if (!scheduledJobs || !connected) return;
  task(async () => {
    const name = $('job-name').value.trim(), prompt = $('job-prompt').value.trim();
    if (!name || new TextEncoder().encode(name).length > 128 || !prompt || new TextEncoder().encode(prompt).length > 32768) throw new Error('名称最多 128 UTF-8 字节，任务内容最多 32 KiB，且不能为空。');
    const tools = [$('job-clock').checked && 'datetime_now', $('job-json').checked && 'json_query'].filter(Boolean);
    if (!tools.length) throw new Error('至少选择一项允许的工具。');
    const schedule = $('job-kind').value === 'cron' ? {kind:'cron',expression:$('job-cron').value.trim(),timezone:$('job-timezone').value.trim()} : {kind:'interval',seconds:Number($('job-interval').value)};
    const job = await api('/api/jobs', 'POST', { name, prompt, schedule, enabled_tools:tools, timeout_secs:Number($('job-timeout').value) });
    $('job-form').reset(); $('job-create').open = false; $('job-cron-fields').hidden = true; $('job-interval-field').hidden = false; $('job-interval').required = true;
    jobsOffset = 0; jobsPrevious = []; await refreshJobs(); await selectJob(job.id); status('任务已创建并启用');
  });
});
controls();
