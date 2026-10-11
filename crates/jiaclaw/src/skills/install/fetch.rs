// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Exact reviewed HTTPS capture. No skill policy or runtime publication.
use super::*;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

const JOURNAL: &str = ".jiaclaw-skill-fetches";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Current observation of one saved download identity, not runtime installation.
pub enum SkillFetchState {
    /// Identity directory exists without a complete intent; retain for manual review.
    PreparationIncomplete,
    /// A GET may have occurred but no fully verified package or failure receipt exists.
    OutcomeUnknown,
    /// Verified package saved; explicit finish may publish its historical receipt.
    Captured,
    /// Verified package and historical receipt saved.
    Completed,
    /// Returned network/content failure recorded; the UUID is never reusable.
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
/// Read-only result tied to an original UUID and captured intent/package.
pub struct SkillFetchStatus {
    /// Original canonical nonzero UUID.
    pub operation_id: String,
    /// Observed saved state.
    pub state: SkillFetchState,
    /// Original intent's complete byte hash, if published.
    pub intent_sha256: Option<String>,
    /// Required reviewed exact raw package hash.
    pub bundle_sha256: Option<String>,
    /// Workspace-relative captured input for independent local installation.
    pub bundle_path: Option<String>,
    /// Parsed package target, only after complete validation.
    pub directory: Option<String>,
    /// Stable bounded failure category, without credential-bearing diagnostics.
    pub failure: Option<String>,
    /// Always false; this Module never loads or installs skills.
    pub runtime_applied: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FetchIntent {
    version: u32,
    operation_id: String,
    url: String,
    bundle_sha256: String,
    allow_private: bool,
}

#[derive(Clone)]
/// Operator-only exact HTTPS package capture with original identity recovery.
pub struct SkillFetcher {
    workspace: PathBuf,
    lock_required: bool,
}

impl SkillFetcher {
    /// Select the administrator's workspace and explicit required-lock policy.
    #[must_use]
    pub fn new(workspace: &Path, lock_required: bool) -> Self {
        Self {
            workspace: workspace.into(),
            lock_required,
        }
    }

    /// One explicit GET per UUID. Dropping this future does not release the
    /// blocking owner's writer; it settles transport and persistence first.
    pub async fn fetch(
        &self,
        operation_id: &str,
        url: &str,
        bundle_hash: &str,
        allow_private: bool,
    ) -> Result<SkillFetchStatus, JiaClawError> {
        self.validate(operation_id)?;
        if !super::super::valid_hash(bundle_hash) {
            return Err(invalid("下载要求审阅过的完整包SHA256"));
        }
        let url = checked_url(url)?;
        let intent = FetchIntent {
            version: 1,
            operation_id: operation_id.into(),
            url: url.to_string(),
            bundle_sha256: bundle_hash.into(),
            allow_private,
        };
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.fetch_owned(intent))
            .await
            .map_err(invalid)?
    }

    fn fetch_owned(&self, intent: FetchIntent) -> Result<SkillFetchStatus, JiaClawError> {
        memory_io::with_workspace_writer(&self.workspace, |root| {
            // Resolve/validate once before creating identity or contacting HTTPS.
            // The real owner remains held across DNS, GET and all disk publications.
            let url = checked_url(&intent.url)?;
            let addresses = resolved_addresses(&url, intent.allow_private)?;
            lock::CatalogLock::parse(&required(root, "skills.lock.json", SKILL_FILE_BYTES)?)?;
            update::ensure_installs_settled(root, &intent.operation_id)?;
            update::ensure_settled(root, &intent.operation_id)?;
            remove::ensure_settled(root, &intent.operation_id)?;
            ensure_settled(root, &intent.operation_id)?;
            let journal =
                directory(root, JOURNAL, true, true)?.ok_or_else(|| invalid("下载记录无法创建"))?;
            if operation_ids(&journal)?.len() >= OPERATIONS {
                return Err(invalid("下载记录已达64份；由操作员审阅归档"));
            }
            absent(&journal, &intent.operation_id)?;
            let op = directory(&journal, &intent.operation_id, true, true)?
                .ok_or_else(|| invalid("下载记录无法创建"))?;
            let raw_intent = serde_json::to_string(&intent).map_err(invalid)?;
            write_new(&op, "intent.json", &raw_intent)?;
            memory_io::sync_dir(&journal)?;
            memory_io::sync_dir(root)?;
            let downloaded = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(invalid)?
                .block_on(download(&url, &addresses));
            let validated = downloaded.and_then(|raw| {
                if digest(&raw) != intent.bundle_sha256 {
                    return Err("digest_mismatch");
                }
                Bundle::parse(&raw).map_err(|_| "invalid_bundle")?;
                Ok(raw)
            });
            match validated {
                Ok(raw) => {
                    write_new(&op, "bundle.json", &raw)?;
                    finish_owned(&op, &intent.operation_id, &digest(&raw_intent))
                }
                Err(code) => {
                    let mut receipt = status(&intent, &raw_intent, SkillFetchState::Failed);
                    receipt.failure = Some(code.into());
                    write_new(
                        &op,
                        "receipt.json",
                        &serde_json::to_string(&receipt).map_err(invalid)?,
                    )?;
                    Err(invalid(format!(
                        "下载失败({code})；原身份已记录，不自动重发"
                    )))
                }
            }
        })
    }

    /// Read-only, including after SIGKILL. Never contacts the origin or installs.
    pub fn inspect(&self, id: &str) -> Result<SkillFetchStatus, JiaClawError> {
        self.validate(id)?;
        memory_io::with_workspace_writer(&self.workspace, |root| {
            let op = operation_fetch(root, id)?;
            observe(&op, id)
        })
    }

    /// Only settle a completely saved bundle, conditional on the original intent.
    /// A missing capture is unknown/manual review and never triggers another GET.
    pub fn finish(&self, id: &str, intent_hash: &str) -> Result<SkillFetchStatus, JiaClawError> {
        self.validate(id)?;
        memory_io::with_workspace_writer(&self.workspace, |root| {
            let op = operation_fetch(root, id)?;
            finish_owned(&op, id, intent_hash)
        })
    }

    fn validate(&self, id: &str) -> Result<(), JiaClawError> {
        if !self.lock_required || !valid_id(id) {
            return Err(invalid("下载要求显式必需来源锁与非零规范UUID"));
        }
        Ok(())
    }
}

fn status(intent: &FetchIntent, raw: &str, state: SkillFetchState) -> SkillFetchStatus {
    SkillFetchStatus {
        operation_id: intent.operation_id.clone(),
        state,
        intent_sha256: Some(digest(raw)),
        bundle_sha256: Some(intent.bundle_sha256.clone()),
        bundle_path: None,
        directory: None,
        failure: None,
        runtime_applied: false,
    }
}

fn operation_fetch(root: &Dir, id: &str) -> Result<Dir, JiaClawError> {
    let journal =
        directory(root, JOURNAL, false, true)?.ok_or_else(|| invalid("下载记录不存在"))?;
    operation_ids(&journal)?;
    directory(&journal, id, false, true)?.ok_or_else(|| invalid("原下载身份不存在"))
}

fn observe(op: &Dir, id: &str) -> Result<SkillFetchStatus, JiaClawError> {
    let mut count = 0;
    for entry in op.entries().map_err(invalid)? {
        count += 1;
        let name = entry
            .map_err(invalid)?
            .file_name()
            .into_string()
            .map_err(|_| invalid("下载状态名称无效"))?;
        if count > 16
            || (!matches!(
                name.as_str(),
                "intent.json" | "bundle.json" | "receipt.json"
            ) && !name.strip_prefix(".jiaclaw-memory-").is_some_and(valid_id))
        {
            return Err(invalid("下载状态包含未知条目或超限"));
        }
        private(&op.symlink_metadata(&name).map_err(invalid)?, false)?;
        snapshot(
            op,
            &name,
            if name == "intent.json" || name == "receipt.json" {
                INTENT_BYTES
            } else {
                BUNDLE_BYTES
            },
        )?
        .ok_or_else(|| invalid("下载状态文件消失"))?;
    }
    let intent = snapshot(op, "intent.json", INTENT_BYTES)?;
    let bundle = snapshot(op, "bundle.json", BUNDLE_BYTES)?;
    let receipt = snapshot(op, "receipt.json", INTENT_BYTES)?;
    let Some(raw_intent) = intent else {
        if bundle.is_some() || receipt.is_some() {
            return Err(invalid("下载捕获无原意图"));
        }
        return Ok(SkillFetchStatus {
            operation_id: id.into(),
            state: SkillFetchState::PreparationIncomplete,
            intent_sha256: None,
            bundle_sha256: None,
            bundle_path: None,
            directory: None,
            failure: None,
            runtime_applied: false,
        });
    };
    let intent: FetchIntent =
        serde_json::from_str(&raw_intent).map_err(|_| invalid("下载意图schema无效"))?;
    if intent.version != 1
        || intent.operation_id != id
        || !super::super::valid_hash(&intent.bundle_sha256)
        || checked_url(&intent.url)?.as_str() != intent.url
    {
        return Err(invalid("下载原身份或来源无效"));
    }
    let mut expected = status(&intent, &raw_intent, SkillFetchState::OutcomeUnknown);
    if let Some(raw) = bundle {
        if digest(&raw) != intent.bundle_sha256 {
            return Err(invalid("原下载包已改变；保留证据"));
        }
        let bundle = Bundle::parse(&raw)?;
        expected.state = SkillFetchState::Captured;
        expected.bundle_path = Some(format!("{JOURNAL}/{id}/bundle.json"));
        expected.directory = Some(bundle.directory);
    }
    if let Some(raw) = receipt {
        let saved: SkillFetchStatus =
            serde_json::from_str(&raw).map_err(|_| invalid("下载收据schema无效"))?;
        match (&expected.state, &saved.state, saved.failure.as_deref()) {
            (SkillFetchState::Captured, SkillFetchState::Completed, None) => {
                expected.state = SkillFetchState::Completed
            }
            (SkillFetchState::OutcomeUnknown, SkillFetchState::Failed, Some(code))
                if matches!(
                    code,
                    "transport"
                        | "unexpected_status"
                        | "encoding"
                        | "too_large"
                        | "invalid_utf8"
                        | "digest_mismatch"
                        | "invalid_bundle"
                ) =>
            {
                expected.state = SkillFetchState::Failed;
                expected.failure = saved.failure.clone();
            }
            _ => return Err(invalid("下载收据与原捕获冲突")),
        }
        if saved != expected {
            return Err(invalid("下载收据与原身份冲突"));
        }
    }
    Ok(expected)
}

fn finish_owned(op: &Dir, id: &str, hash: &str) -> Result<SkillFetchStatus, JiaClawError> {
    let mut observed = observe(op, id)?;
    if observed.intent_sha256.as_deref() != Some(hash) {
        return Err(invalid("下载意图已改变，拒绝结算"));
    }
    match observed.state {
        SkillFetchState::Completed => return Ok(observed),
        SkillFetchState::Captured => (),
        _ => {
            return Err(invalid(
                "下载尚无完整已核验捕获；只读核对并人工审阅，不重发",
            ))
        }
    }
    observed.state = SkillFetchState::Completed;
    write_new(
        op,
        "receipt.json",
        &serde_json::to_string(&observed).map_err(invalid)?,
    )?;
    Ok(observed)
}

pub(super) fn ensure_settled(root: &Dir, new_id: &str) -> Result<(), JiaClawError> {
    if let Some(journal) = directory(root, JOURNAL, false, true)? {
        for id in operation_ids(&journal)? {
            if id == new_id {
                return Err(invalid("原UUID已有下载记录；禁止新操作"));
            }
            let op =
                directory(&journal, &id, false, true)?.ok_or_else(|| invalid("下载记录缺失"))?;
            if !matches!(
                observe(&op, &id)?.state,
                SkillFetchState::Completed | SkillFetchState::Failed
            ) {
                return Err(invalid("有未结算下载；先核对原身份，禁止新请求"));
            }
        }
    }
    Ok(())
}

fn checked_url(raw: &str) -> Result<reqwest::Url, JiaClawError> {
    let url = reqwest::Url::parse(raw).map_err(|_| invalid("下载URL无效"))?;
    let authority = raw
        .split_once("://")
        .map(|(_, rest)| rest.split('/').next().unwrap_or(rest));
    if raw.len() > 2048
        || url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || authority.is_some_and(|host| host.contains('@'))
    {
        return Err(invalid("下载只接受无凭据/query/fragment的有界HTTPS URL"));
    }
    Ok(url)
}

fn resolved_addresses(
    url: &reqwest::Url,
    allow_private: bool,
) -> Result<Vec<SocketAddr>, JiaClawError> {
    let host = url
        .host_str()
        .ok_or_else(|| invalid("下载主机缺失"))?
        .trim_start_matches('[')
        .trim_end_matches(']');
    let port = url
        .port_or_known_default()
        .ok_or_else(|| invalid("下载端口缺失"))?;
    let addresses: Vec<_> = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        (host, port)
            .to_socket_addrs()
            .map_err(|_| invalid("下载DNS解析失败"))?
            .take(17)
            .collect()
    };
    if addresses.is_empty()
        || addresses.len() > 16
        || addresses.iter().any(|a| !allowed_ip(a.ip(), allow_private))
    {
        return Err(invalid(
            "下载拒绝本地/私有/保留或超限DNS地址；内网仅操作员显式allow-private",
        ));
    }
    Ok(addresses)
}

fn allowed_ip(ip: IpAddr, private: bool) -> bool {
    if ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            if ip.is_broadcast() {
                return false;
            }
            private
                || !(a == 0
                    || a == 10
                    || a == 127
                    || a >= 224
                    || (a == 100 && (64..=127).contains(&b))
                    || (a == 169 && b == 254)
                    || (a == 172 && (16..=31).contains(&b))
                    || (a == 192
                        && (b == 168 || (b == 0 && (c == 0 || c == 2)) || (b == 88 && c == 99)))
                    || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                    || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return allowed_ip(IpAddr::V4(v4), private);
            }
            let s = ip.segments();
            private
                || (s[0] & 0xe000 == 0x2000
                    && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
                    && s[0] != 0x2002
                    && !(s[0] == 0x3fff && s[1] < 0x1000))
        }
    }
}

async fn download(url: &reqwest::Url, addresses: &[SocketAddr]) -> Result<String, &'static str> {
    let host = url.host_str().ok_or("transport")?;
    let client = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .http1_only()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .resolve_to_addrs(host, addresses)
        .build()
        .map_err(|_| "transport")?;
    let mut response = client
        .get(url.clone())
        .header("Accept", "application/json")
        .header("Accept-Encoding", "identity")
        .header("User-Agent", "JiaClaw-Skill-Fetch/1")
        .send()
        .await
        .map_err(|_| "transport")?;
    if response.status() != reqwest::StatusCode::OK {
        return Err("unexpected_status");
    }
    if response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .is_some_and(|v| v != "identity")
    {
        return Err("encoding");
    }
    if response
        .content_length()
        .is_some_and(|n| n > BUNDLE_BYTES as u64)
    {
        return Err("too_large");
    }
    let mut raw = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "transport")? {
        if raw.len().saturating_add(chunk.len()) > BUNDLE_BYTES {
            return Err("too_large");
        }
        raw.extend_from_slice(&chunk);
    }
    String::from_utf8(raw).map_err(|_| "invalid_utf8")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[tokio::test]
    async fn fetch_cancelled_waiter_retains_real_transport_owner_until_settlement() {
        use std::io::Read;
        let work = tempfile::tempdir().unwrap();
        fs::write(
            work.path().join("skills.lock.json"),
            "{\"version\":2,\"skills\":[]}",
        )
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!(
            "https://127.0.0.1:{}/bundle",
            listener.local_addr().unwrap().port()
        );
        let (admitted, observation) = tokio::sync::oneshot::channel();
        let (release, held) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(std::time::Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            // macOS accept inherits the listener's nonblocking flag. The
            // original bounded ClientHello read must actually wait for bytes.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut hello = [0; 1];
            assert_eq!(stream.read(&mut hello).unwrap(), 1);
            // Real TLS ClientHello reached this TCP trap; it deliberately sends
            // no certificate or HTTP response and holds the actual transport.
            admitted.send(()).unwrap();
            held.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        let id = uuid::Uuid::new_v4().to_string();
        let fetcher = SkillFetcher::new(work.path(), true);
        let child_fetcher = fetcher.clone();
        let child_id = id.clone();
        let task = tokio::spawn(async move {
            child_fetcher
                .fetch(&child_id, &url, &"a".repeat(64), true)
                .await
        });
        tokio::time::timeout(Duration::from_secs(12), observation)
            .await
            .unwrap()
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(fetcher.inspect(&id).is_err()); // original nonqueue writer still held
        release.send(()).unwrap();
        server.join().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let observed = loop {
            if let Ok(observed) = fetcher.inspect(&id) {
                break observed;
            }
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(observed.state, SkillFetchState::Failed);
        assert_eq!(observed.failure.as_deref(), Some("transport"));
        assert!(fetcher
            .finish(&id, observed.intent_sha256.as_ref().unwrap())
            .is_err());
    }
    fn capture_fixture() -> (tempfile::TempDir, String, String, String) {
        let work = tempfile::tempdir().unwrap();
        fs::write(
            work.path().join("skills.lock.json"),
            "{\"version\":2,\"skills\":[]}",
        )
        .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let root = Dir::open_ambient_dir(work.path(), cap_std::ambient_authority()).unwrap();
        let journal = directory(&root, JOURNAL, true, true).unwrap().unwrap();
        let op = directory(&journal, &id, true, true).unwrap().unwrap();
        let body = "---\nname: fetched\ndescription: capture\n---\noriginal\n";
        let raw = serde_json::json!({"version":1,"directory":"fetched","source":{
            "repository":"https://example.com/skills","revision":"a".repeat(40),"skill_sha256":digest(body)},
            "files":[{"path":"SKILL.md","text":body}]}).to_string();
        let intent = serde_json::to_string(&FetchIntent {
            version: 1,
            operation_id: id.clone(),
            url: "https://example.com/bundle.json".into(),
            bundle_sha256: digest(&raw),
            allow_private: false,
        })
        .unwrap();
        write_new(&op, "intent.json", &intent).unwrap();
        (work, id, raw, digest(&intent))
    }
    #[test]
    fn fetch_admission_rejects_credentials_and_special_addresses() {
        for raw in [
            "http://example.com/a",
            "https://a:b@example.com/a",
            "https://@example.com/a",
            "https://example.com/a?token=x",
            "https://example.com/#x",
        ] {
            assert!(checked_url(raw).is_err(), "{raw}");
        }
        for raw in [
            "0.0.0.0",
            "127.0.0.1",
            "100.64.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "2001:db8::1",
            "2002::1",
            "3fff::1",
        ] {
            assert!(!allowed_ip(raw.parse().unwrap(), false), "{raw}");
        }
        for raw in ["1.1.1.1", "8.8.8.8", "2606:4700::1111"] {
            assert!(allowed_ip(raw.parse().unwrap(), false), "{raw}");
        }
        assert!(allowed_ip("127.0.0.1".parse().unwrap(), true));
        assert!(!allowed_ip("0.0.0.0".parse().unwrap(), true));
    }
    #[test]
    fn fetch_unknown_cannot_finish_or_allow_a_new_ledger_identity() {
        let (work, id, _, hash) = capture_fixture();
        let fetcher = SkillFetcher::new(work.path(), true);
        assert_eq!(
            fetcher.inspect(&id).unwrap().state,
            SkillFetchState::OutcomeUnknown
        );
        assert!(fetcher.finish(&id, &hash).is_err());
        let root = Dir::open_ambient_dir(work.path(), cap_std::ambient_authority()).unwrap();
        assert!(ensure_settled(&root, &uuid::Uuid::new_v4().to_string()).is_err());
        assert!(ensure_settled(&root, &id).is_err());
        assert!(!work
            .path()
            .join(JOURNAL)
            .join(id)
            .join("receipt.json")
            .exists());
    }
    #[test]
    fn fetch_saved_capture_settles_only_original_bytes_and_preserves_conflicts() {
        let (work, id, raw, hash) = capture_fixture();
        let root = Dir::open_ambient_dir(work.path(), cap_std::ambient_authority()).unwrap();
        let op = operation_fetch(&root, &id).unwrap();
        write_new(&op, "bundle.json", &raw).unwrap();
        let fetcher = SkillFetcher::new(work.path(), true);
        assert_eq!(
            fetcher.inspect(&id).unwrap().state,
            SkillFetchState::Captured
        );
        assert!(fetcher.finish(&id, &"0".repeat(64)).is_err());
        let receipt = fetcher.finish(&id, &hash).unwrap();
        assert_eq!(receipt.state, SkillFetchState::Completed);
        assert!(!receipt.runtime_applied);
        assert_eq!(fetcher.finish(&id, &hash).unwrap(), receipt);
        assert!(ensure_settled(&root, &id).is_err());
        ensure_settled(&root, &uuid::Uuid::new_v4().to_string()).unwrap();
        op.write("bundle.json", format!("{raw} ")).unwrap();
        assert!(fetcher.inspect(&id).is_err());
        assert!(fetcher.finish(&id, &hash).is_err());
        assert_eq!(op.read_to_string("bundle.json").unwrap(), format!("{raw} "));
    }
    #[test]
    fn fetch_inspection_rejects_linked_or_inconsistent_saved_state() {
        let (work, id, _, _) = capture_fixture();
        let path = work.path().join(JOURNAL).join(&id);
        let f = SkillFetcher::new(work.path(), true);
        fs::write(path.join("unexpected"), "evidence").unwrap();
        assert!(f.inspect(&id).is_err());
        fs::remove_file(path.join("unexpected")).unwrap();
        fs::hard_link(path.join("intent.json"), path.join("receipt.json")).unwrap();
        assert!(f.inspect(&id).is_err());
        assert!(path.join("intent.json").exists());
    }
}
