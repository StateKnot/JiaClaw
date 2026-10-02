'use strict';
const $ = id => document.getElementById(id);
let token = '', selected = null, connected = false, busy = false, sessionList = [];
function status(text, error = false) { $('status').textContent = text; $('status').classList.toggle('error', error); }
function controls() {
  $('new-session').disabled = !connected || busy;
  $('refresh').disabled = !connected || busy;
  $('delete-session').disabled = !selected || busy;
  $('message').disabled = !selected || busy;
  $('send').disabled = !selected || busy;
  for (const button of $('sessions').querySelectorAll('button')) button.disabled = busy;
  for (const element of $('connect-form').elements) element.disabled = busy;
}
async function api(path, method = 'GET', body) {
  const response = await fetch(path, { method, credentials: 'omit', cache: 'no-store', headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), ...(body ? { 'Content-Type': 'application/json' } : {}) }, ...(body ? { body: JSON.stringify(body) } : {}) });
  let data;
  try { data = await response.json(); } catch { throw new Error(`服务响应异常（HTTP ${response.status}）`); }
  if (!response.ok) {
    if (response.status === 401) { connected = false; throw new Error('鉴权失败，请重新连接并检查 API Token。'); }
    throw new Error(data.error || `请求失败（HTTP ${response.status}）`);
  }
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
  renderMessages(session.messages); renderSessions(); status('已连接 · 会话就绪');
}
async function task(fn) {
  if (busy) return;
  busy = true; controls();
  try { await fn(); } catch (error) { status(error.message, true); }
  finally { busy = false; controls(); }
}
$('connect-form').addEventListener('submit', event => {
  event.preventDefault(); task(async () => {
    token = $('api-token').value.trim(); $('api-token').value = ''; status('连接中…');
    await refresh(); connected = true;
    if (selected && sessionList.some(session => session.id === selected)) await select(selected);
    else { selected = null; renderMessages([]); $('session-title').textContent = '开始一段对话'; status('已连接 · 选择或新建会话'); }
  });
});
$('refresh').addEventListener('click', () => task(async () => { await refresh(); status('会话列表已更新'); }));
$('new-session').addEventListener('click', () => task(async () => {
  const data = await api('/api/sessions', 'POST'); await refresh(); await select(data.session_id);
}));
$('delete-session').addEventListener('click', () => {
  if (!selected || !confirm('删除这段会话及全部历史？此操作无法撤销。')) return;
  task(async () => { await api(`/api/sessions/${encodeURIComponent(selected)}`, 'DELETE'); selected = null; renderMessages([]); $('session-title').textContent = '开始一段对话'; await refresh(); status('会话已删除'); });
});
$('chat-form').addEventListener('submit', event => {
  event.preventDefault(); const text = $('message').value.trim(); if (!text || !selected) return;
  task(async () => {
    status('JiaClaw 正在处理…');
    await api('/api/chat', 'POST', { messages: [{ role: 'user', content: text }], session_id: selected, stream: false });
    $('message').value = ''; await select(selected); await refresh(); status('回复已保存');
  });
});
$('message').addEventListener('keydown', event => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); $('chat-form').requestSubmit(); } });
