// Public browser/file boundary shared by the existing real turn fixtures.
const assert = require('assert'), fs = require('fs'), crypto = require('crypto');
async function downloadExact(page, id, reply) {
  await page.waitForFunction(() => !document.getElementById('turn-download').disabled);
  const requests = [], capture = r => requests.push([r.method(), new URL(r.url()).pathname]);
  page.on('request', capture);
  try {
    const pending = page.waitForEvent('download');
    await page.locator('#turn-download').click();
    const file = await pending;
    assert.strictEqual(file.suggestedFilename(), 'jiaclaw-' + id + '.txt');
    assert.strictEqual(await file.failure(), null);
    const bytes = fs.readFileSync(await file.path());
    assert.deepStrictEqual(bytes, Buffer.from(reply, 'utf8'));
    assert.deepStrictEqual(requests, [['GET', '/api/turns/' + id]]);
    await page.waitForFunction(() => !document.getElementById('turn-download').disabled);
    assert((await page.locator('#status').textContent()).includes('下载列表确认'));
    assert.strictEqual(await page.evaluate(() => localStorage.length + sessionStorage.length), 0);
    console.log('ACTUAL turn download file ' + JSON.stringify({id, bytes:bytes.length, sha256:crypto.createHash('sha256').update(bytes).digest('hex'), original_gets:requests.length, exact:true}));
  } finally { page.off('request', capture); }
}
async function rejectDownloadChanges(page, id) {
  const path = '**/api/turns/' + id, downloads = [], listener = d => downloads.push(d);
  page.on('download', listener);
  try {
    // Only the GET response is changed; the actual model/SQL receipt remains
    // complete. Exercise the download's independent re-read, not a new task.
    for (const mode of ['purged', 'active', 'wrong-id', 'wrong-hash', 'surrogate', 'unavailable']) {
      let reads = 0;
      await page.route(path, async route => {
        assert.strictEqual(route.request().method(), 'GET'); reads++;
        const r = await route.fetch({maxRetries:0, maxRedirects:0}), value = await r.json();
        assert.strictEqual(r.status(), 200);
        if (mode === 'unavailable') return route.fulfill({status:503, json:{error:'registry unavailable'}});
        if (mode === 'purged') { value.receipt.result = null; value.receipt.result_purged = true; }
        if (mode === 'active') value.active = true;
        if (mode === 'wrong-id') value.receipt.id = '00000000-0000-4000-8000-000000000000';
        if (mode === 'wrong-hash') value.receipt.request_hash = '0'.repeat(64);
        if (mode === 'surrogate') value.receipt.result.reply = '\ud800';
        return route.fulfill({response:r, json:value});
      });
      await page.locator('#turn-download').click();
      await page.waitForFunction(() => !document.getElementById('turn-check').disabled);
      assert.strictEqual(reads, 1); assert.strictEqual(downloads.length, 0);
      assert.strictEqual(await page.locator('#turn-download').isDisabled(), true);
      await page.unroute(path);
      await page.locator('#turn-check').click();
      await page.waitForFunction(() => !document.getElementById('turn-download').disabled);
    }
  } finally { await page.unroute(path); page.off('download', listener); }
}
async function staleDownload(page, id, switchIdentity) {
  const path = '**/api/turns/' + id, downloads = [], listener = d => downloads.push(d);
  let release, received, settled;
  const gate = new Promise(resolve => { release = resolve; });
  const seen = new Promise(resolve => { received = resolve; });
  const done = new Promise(resolve => { settled = resolve; });
  page.on('download', listener);
  await page.route(path, async route => {
    const response = await route.fetch({maxRetries:0, maxRedirects:0});
    received(); await gate;
    try { await route.fulfill({response}); } finally { settled(); }
  });
  try {
    await page.locator('#turn-download').click(); await seen;
    await switchIdentity();
    release(); await done; await page.unroute(path);
    await page.evaluate(() => new Promise(resolve => setTimeout(resolve, 30)));
    assert.strictEqual(downloads.length, 0);
    assert.strictEqual(await page.locator('#turn-download').isDisabled(), true);
    assert.strictEqual(await page.locator('#turn-id').inputValue(), '');
    assert.strictEqual(await page.locator('#messages .message').count(), 0);
  } finally { release(); await page.unroute(path); page.off('download', listener); }
}
async function revokedDownload(page, id) {
  const path = '**/api/turns/' + id, downloads = [], listener = d => downloads.push(d);
  page.on('download', listener);
  await page.route(path, route => route.fulfill({status:401, contentType:'text/plain', body:'revoked'}));
  try {
    await page.locator('#turn-download').click();
    await page.waitForFunction(() => document.getElementById('status').textContent.includes('鉴权失败'));
    assert.strictEqual(downloads.length, 0);
    assert.strictEqual(await page.locator('#turn-id').inputValue(), '');
    assert.strictEqual(await page.locator('#turn-download').isDisabled(), true);
    assert.strictEqual(await page.locator('#messages .message').count(), 0);
  } finally { await page.unroute(path); page.off('download', listener); }
}
module.exports = {downloadExact, rejectDownloadChanges, staleDownload, revokedDownload};
