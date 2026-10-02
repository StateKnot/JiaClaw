// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Explicitly authorized commands executed only inside a hardened container.

use crate::Tool;
use async_trait::async_trait;
use jiaclaw_core::{ExecToolConfig, JiaClawError};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

fn fail(e: impl std::fmt::Display) -> JiaClawError {
    JiaClawError::ToolExecution(format!("exec: {e}"))
}

/// Validate the trusted sandbox policy before advertising any execution tool.
///
/// # Errors
/// Invalid pins, paths, command aliases, limits or root identities are rejected.
pub fn validate_exec_config(config: &ExecToolConfig) -> Result<(), JiaClawError> {
    if !config.enabled {
        return Ok(());
    }
    let digest = config
        .image
        .rsplit_once("@sha256:")
        .filter(|(name, digest)| {
            !name.is_empty()
                && !name.starts_with('-')
                && !name.chars().any(char::is_whitespace)
                && digest.len() == 64
                && digest.bytes().all(|b| b.is_ascii_hexdigit())
        });
    if digest.is_none() {
        return Err(fail("image 必须固定到 image@sha256:<64位摘要>"));
    }
    if !config.docker_path.is_absolute() || !config.docker_path.is_file() {
        return Err(fail("docker_path 必须是已安装 Docker CLI 的绝对路径"));
    }
    if !(1..=300).contains(&config.timeout_secs)
        || !(1..=1_048_576).contains(&config.max_output_bytes)
    {
        return Err(fail("期限或输出上限超出允许范围"));
    }
    let parts: Vec<_> = config.user.split(':').collect();
    if parts.len() != 2
        || parts
            .iter()
            .any(|part| part.parse::<u32>().map_or(true, |n| n == 0))
    {
        return Err(fail("user 必须是非 root 的数字 UID:GID"));
    }
    if config.commands.is_empty() || config.commands.len() > 32 {
        return Err(fail("commands 需要 1..=32 个显式白名单命令"));
    }
    for (name, executable) in &config.commands {
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || !executable.starts_with('/')
            || executable.split('/').any(|part| part == "..")
            || executable.contains('\0')
            || executable.len() > 512
        {
            return Err(fail("白名单名称或容器内可执行路径非法"));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecArgs {
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

async fn drain_bounded<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut kept = Vec::new();
    let mut buffer = vec![0u8; 8192];
    let mut truncated = false;
    loop {
        let n = reader.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        let retain = n.min(limit.saturating_sub(kept.len()));
        kept.extend_from_slice(&buffer[..retain]);
        truncated |= retain < n;
    }
    Ok((kept, truncated))
}

// A dropped tool future must remove the entire container, including descendants.
// Retry handles cancellation while Docker is still completing container creation.
struct ContainerGuard {
    docker: PathBuf,
    name: String,
    armed: bool,
}
impl ContainerGuard {
    async fn remove(&mut self) -> Result<(), JiaClawError> {
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            Command::new(&self.docker)
                .args(["rm", "--force", &self.name])
                .kill_on_drop(true)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status(),
        )
        .await;
        match result {
            Ok(Ok(status)) if status.success() => {
                self.armed = false;
                Ok(())
            }
            _ => Err(fail(
                "容器清理失败；需检查 Docker daemon 并删除残留的 jiaclaw-exec 容器",
            )),
        }
    }
}
impl Drop for ContainerGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let docker = self.docker.clone();
        let name = self.name.clone();
        // Independent thread also works when the Tokio runtime itself is stopping.
        std::thread::spawn(move || {
            for _ in 0..5 {
                let Ok(mut child) = std::process::Command::new(&docker)
                    .args(["rm", "--force", &name])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                else {
                    break;
                };
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) if status.success() => return,
                        Ok(Some(_)) | Err(_) => break,
                        Ok(None) if std::time::Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(50))
                        }
                        Ok(None) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            tracing::error!(container = %name, "取消后容器清理未确认，请检查 Docker daemon");
        });
    }
}

/// Container execution tool. No host command fallback exists.
pub struct ControlledExecTool {
    workspace: PathBuf,
    config: ExecToolConfig,
    name: &'static str,
}
impl ControlledExecTool {
    /// Construct and validate a configured tool.
    ///
    /// # Errors
    /// Invalid policy or an inaccessible workspace fails before registration.
    pub fn new(workspace: &Path, config: ExecToolConfig) -> Result<Self, JiaClawError> {
        validate_exec_config(&config)?;
        let workspace = workspace.canonicalize().map_err(fail)?;
        if !workspace.is_dir()
            || workspace
                .to_str()
                .is_none_or(|p| p.contains(',') || p.contains('\n'))
        {
            return Err(fail("工作区必须是已存在且可挂载的目录"));
        }
        Ok(Self {
            workspace,
            config,
            name: "exec",
        })
    }
    /// Compatibility name; it uses exactly the same sandbox policy.
    ///
    /// # Errors
    /// Invalid policy or an inaccessible workspace fails before registration.
    pub fn legacy(workspace: &Path, config: ExecToolConfig) -> Result<Self, JiaClawError> {
        let mut tool = Self::new(workspace, config)?;
        tool.name = "shell_exec";
        Ok(tool)
    }
    fn create_args(&self, name: &str, executable: &str, args: &[String]) -> Vec<String> {
        let mut mount = format!("type=bind,src={},dst=/workspace", self.workspace.display());
        if self.config.workspace_read_only {
            mount.push_str(",readonly");
        }
        let workspace_label = format!("jiaclaw.workspace={}", self.workspace.display());
        let mut command: Vec<String> = [
            "create",
            "--pull=never",
            "--name",
            name,
            "--label",
            "jiaclaw.sandbox=true",
            "--label",
            &workspace_label,
            "--network=none",
            "--read-only",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            "--pids-limit=64",
            "--memory=256m",
            "--memory-swap=256m",
            "--cpus=1",
            "--ipc=none",
            "--init",
            "--log-driver=none",
            "--tmpfs",
            "/tmp:rw,noexec,nosuid,nodev,size=16m",
            "--workdir=/workspace",
            "--user",
            &self.config.user,
            "--mount",
            &mount,
            "--entrypoint",
            executable,
            &self.config.image,
        ]
        .iter()
        .map(|s| (*s).into())
        .collect();
        command.extend_from_slice(args);
        command
    }
}
#[async_trait]
impl Tool for ControlledExecTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &'static str {
        "执行管理员授权的命令。仅在固定镜像的 Docker 沙箱中运行；无网络、非 root、资源受限、总超时、输出受限。command 是白名单别名，args 是字面量参数数组。"
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object","properties":{"command":{"type":"string","enum":self.config.commands.keys().collect::<Vec<_>>()},"args":{"type":"array","maxItems":128,"items":{"type":"string"}}},"required":["command"],"additionalProperties":false})
    }
    async fn execute(&self, raw: Value) -> Result<String, JiaClawError> {
        if !self.config.enabled {
            return Err(fail("执行权限默认关闭，需要配置 tools.exec"));
        }
        let args: ExecArgs = serde_json::from_value(raw).map_err(fail)?;
        let executable = self
            .config
            .commands
            .get(&args.command)
            .ok_or_else(|| fail("命令不在安全白名单中"))?;
        if args.args.len() > 128
            || args.args.iter().any(|a| a.contains('\0'))
            || args.args.iter().map(String::len).sum::<usize>() > 32768
        {
            return Err(fail("参数超出上限或含 NUL"));
        }
        let name = format!("jiaclaw-exec-{}", uuid::Uuid::new_v4());
        let mut guard = ContainerGuard {
            docker: self.config.docker_path.clone(),
            name: name.clone(),
            armed: true,
        };
        let run = async {
            let created = Command::new(&self.config.docker_path)
                .args(self.create_args(&name, executable, &args.args))
                .kill_on_drop(true)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(fail)?;
            // Docker diagnostics are bounded with the same drains as command output.
            let mut created = created;
            let out = created.stdout.take().ok_or_else(|| fail("缺少 stdout"))?;
            let err = created.stderr.take().ok_or_else(|| fail("缺少 stderr"))?;
            let (status, _, stderr) = tokio::try_join!(
                created.wait(),
                drain_bounded(out, 1024),
                drain_bounded(err, 4096)
            )
            .map_err(fail)?;
            if !status.success() {
                return Err(fail(format!(
                    "无法创建沙箱: {}",
                    String::from_utf8_lossy(&stderr.0)
                )));
            }
            let mut child = Command::new(&self.config.docker_path)
                .args(["start", "--attach", &name])
                .kill_on_drop(true)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(fail)?;
            let stdout = child.stdout.take().ok_or_else(|| fail("缺少 stdout"))?;
            let stderr = child.stderr.take().ok_or_else(|| fail("缺少 stderr"))?;
            let (status, stdout, stderr) = tokio::try_join!(
                child.wait(),
                drain_bounded(stdout, self.config.max_output_bytes),
                drain_bounded(stderr, self.config.max_output_bytes)
            )
            .map_err(fail)?;
            Ok(
                json!({"exit_code":status.code(),"stdout":String::from_utf8_lossy(&stdout.0),"stderr":String::from_utf8_lossy(&stderr.0),"stdout_truncated":stdout.1,"stderr_truncated":stderr.1}),
            )
        };
        let result =
            match tokio::time::timeout(Duration::from_secs(self.config.timeout_secs), run).await {
                Ok(result) => result,
                Err(_) => Err(fail(format!(
                    "执行超过 {} 秒，已请求终止整个容器",
                    self.config.timeout_secs
                ))),
            };
        let cleanup = guard.remove().await;
        match (result, cleanup) {
            (Ok(value), Ok(())) => Ok(value.to_string()),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(cleanup)) => Err(fail(format!("{error}; {cleanup}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn configured() -> ExecToolConfig {
        let mut config = ExecToolConfig {
            enabled: true,
            docker_path: std::env::current_exe().unwrap(),
            image: format!("alpine@sha256:{}", "a".repeat(64)),
            ..ExecToolConfig::default()
        };
        config.commands.insert("echo".into(), "/bin/echo".into());
        config
    }
    #[test]
    fn rejects_mutable_image_root_and_unbounded_limits() {
        let mut c = configured();
        validate_exec_config(&c).unwrap();
        c.image = "alpine:latest".into();
        assert!(validate_exec_config(&c).is_err());
        c = configured();
        c.user = "0:1".into();
        assert!(validate_exec_config(&c).is_err());
        c = configured();
        c.timeout_secs = 0;
        assert!(validate_exec_config(&c).is_err());
        c = configured();
        c.commands.insert("../sh".into(), "/bin/sh".into());
        assert!(validate_exec_config(&c).is_err());
    }
    #[tokio::test]
    async fn typed_arguments_allowlist_and_bounded_output() {
        let ws = tempfile::tempdir().unwrap();
        let tool = ControlledExecTool::new(ws.path(), configured()).unwrap();
        assert!(tool.execute(json!({"command":"sh"})).await.is_err());
        assert!(tool
            .execute(json!({"command":"echo","args":[17]}))
            .await
            .is_err());
        let (data, truncated) = drain_bounded(&b"123456789"[..], 3).await.unwrap();
        assert_eq!(data, b"123");
        assert!(truncated);
    }
    #[test]
    fn sandbox_has_no_host_shell_or_credentials() {
        let ws = tempfile::tempdir().unwrap();
        let tool = ControlledExecTool::new(ws.path(), configured()).unwrap();
        let args = tool.create_args("test", "/bin/echo", &["$(id); rm -rf /".into()]);
        assert!(args.contains(&"--network=none".into()));
        assert!(args.contains(&"--read-only".into()));
        assert!(args.iter().any(|a| a.ends_with("dst=/workspace,readonly")));
        assert_eq!(args.last().unwrap(), "$(id); rm -rf /");
    }
}

#[cfg(test)]
mod docker_tests {
    use super::*;
    fn config() -> ExecToolConfig {
        let mut c = ExecToolConfig {
            enabled: true,
            docker_path: std::env::var("JIACLAW_TEST_DOCKER")
                .unwrap_or_else(|_| "/usr/bin/docker".into())
                .into(),
            image: std::env::var("JIACLAW_TEST_EXEC_IMAGE")
                .expect("set JIACLAW_TEST_EXEC_IMAGE to a pre-pulled image@sha256:digest"),
            timeout_secs: 2,
            max_output_bytes: 128,
            ..ExecToolConfig::default()
        };
        for (name, path) in [
            ("cat", "/bin/cat"),
            ("sleep", "/bin/sleep"),
            ("printf", "/usr/bin/printf"),
            ("touch", "/bin/touch"),
        ] {
            c.commands.insert(name.into(), path.into());
        }
        c
    }
    #[tokio::test]
    #[ignore = "requires a Docker daemon and a digest-pinned Alpine image"]
    async fn real_container_isolation_timeout_output_and_cleanup() {
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(ws.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(ws.path().join("inside"), b"visible").unwrap();
        std::fs::write(outside.path().join("private"), b"HOST_SECRET").unwrap();
        let tool = ControlledExecTool::new(ws.path(), config()).unwrap();
        let result: Value = serde_json::from_str(
            &tool
                .execute(json!({"command":"cat","args":["inside"]}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["stdout"], "visible");
        let denied: Value = serde_json::from_str(
            &tool
                .execute(json!({"command":"cat","args":[outside.path().join("private")]}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(!denied["stdout"].as_str().unwrap().contains("HOST_SECRET"));
        let write: Value = serde_json::from_str(
            &tool
                .execute(json!({"command":"touch","args":["created"]}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_ne!(write["exit_code"], 0);
        assert!(!ws.path().join("created").exists());
        let output: Value = serde_json::from_str(
            &tool
                .execute(json!({"command":"printf","args":["%s","x".repeat(4096)]}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(output["stdout"].as_str().unwrap().len(), 128);
        assert_eq!(output["stdout_truncated"], true);
        let error = tool
            .execute(json!({"command":"sleep","args":["30"]}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("超过"), "{error}");
        let mut c = config();
        c.workspace_read_only = false;
        let writable = ControlledExecTool::new(ws.path(), c).unwrap();
        // Test only: permit the sandbox's non-root UID to write the fixture directory.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(ws.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        }
        let created: Value = serde_json::from_str(
            &writable
                .execute(json!({"command":"touch","args":["created"]}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(created["exit_code"], 0);
        assert!(ws.path().join("created").exists());
        // Cancellation must kill the whole sandbox even before the command timeout.
        let mut cancellation_config = config();
        cancellation_config.timeout_secs = 30;
        let docker = cancellation_config.docker_path.clone();
        let cancellable = ControlledExecTool::new(ws.path(), cancellation_config).unwrap();
        let filter = format!(
            "label=jiaclaw.workspace={}",
            cancellable.workspace.display()
        );
        let pending = tokio::spawn(async move {
            cancellable
                .execute(json!({"command":"sleep","args":["30"]}))
                .await
        });
        let containers = || async {
            let output = Command::new(&docker)
                .args(["ps", "-aq", "--filter", &filter])
                .output()
                .await
                .unwrap();
            assert!(output.status.success());
            !output.stdout.is_empty()
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while !containers().await {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(5), async {
            while containers().await {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("cancelled sandbox was not removed");
    }
}
