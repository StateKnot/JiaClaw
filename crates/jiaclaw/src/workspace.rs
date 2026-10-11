// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 工作空间管理

use crate::memory_io::{initialize_workspace_file, read_text};
use jiaclaw_core::{JiaClawError, MEMORY_PROMPT_MAX_BYTES};
use std::path::{Path, PathBuf};

/// 工作空间文件
#[derive(Debug, Clone)]
pub struct Workspace {
    /// 工作空间路径
    pub path: PathBuf,

    /// AGENTS.md - Agent 配置和元数据
    pub agents: Option<String>,

    /// SOUL.md - Agent 性格和指令
    pub soul: Option<String>,

    /// USER.md - 用户信息和偏好
    pub user: Option<String>,

    /// MEMORY.md - 长期记忆和上下文
    pub memory: Option<String>,
}

impl Workspace {
    /// 加载工作空间文件，每个文件最多读取并保留 32 KiB 的 UTF-8 文本。
    ///
    /// 工作空间或文件缺失时，对应字段为 `None`；空文件保留 `Some("")`。
    ///
    /// # Errors
    ///
    /// 拒绝符号链接、硬链接和特殊文件；读取失败或文本不是有效 UTF-8 时返回错误。
    pub fn load(path: &Path) -> Result<Self, JiaClawError> {
        let agents = Self::load_file(path, "AGENTS.md")?;
        let soul = Self::load_file(path, "SOUL.md")?;
        let user = Self::load_file(path, "USER.md")?;
        let memory = Self::load_file(path, "MEMORY.md")?;

        Ok(Self {
            path: path.to_path_buf(),
            agents,
            soul,
            user,
            memory,
        })
    }

    /// 初始化工作空间，只补齐缺失的默认文件，保留已有常规文件。
    ///
    /// # Errors
    ///
    /// 如果目录或文件不安全，或无法创建目录、读取或写入文件，返回错误。
    pub fn init(path: &Path) -> Result<Self, JiaClawError> {
        Self::init_with_overwrite(path, false)
    }

    /// 初始化工作空间；仅在 `overwrite = true` 时覆盖已有常规默认文件。
    ///
    /// 顶层默认文件与两个示例技能使用相同的受限路径和原子写入规则。
    /// 初始化中断后可以重试，默认保留已经创建或用户编辑的文件。
    /// 已有普通单链接 skills.lock.json 时，两种模式都不修改或补种示例技能。
    ///
    /// # Errors
    ///
    /// 符号链接、硬链接、特殊文件以及目录或文件 IO 错误均返回错误。
    pub fn init_with_overwrite(path: &Path, overwrite: bool) -> Result<Self, JiaClawError> {
        // 显式工作空间根目录是授权边界；所有子路径交给受限文件 helper。
        std::fs::create_dir_all(path)
            .map_err(|e| JiaClawError::Configuration(format!("无法创建工作空间目录: {e}")))?;

        let defaults = [
            ("AGENTS.md", Self::default_agents_content()),
            ("SOUL.md", Self::default_soul_content()),
            ("USER.md", Self::default_user_content()),
            ("MEMORY.md", Self::default_memory_content()),
            ("skills/search/SKILL.md", Self::search_skill_content()),
            (
                "skills/calculator/SKILL.md",
                Self::calculator_skill_content(),
            ),
        ];
        for (relative_path, content) in defaults {
            initialize_workspace_file(path, relative_path, content, overwrite)?;
        }

        Self::load(path)
    }

    /// 加载一个可选文件，并明确记录截断。
    fn load_file(workspace_path: &Path, filename: &str) -> Result<Option<String>, JiaClawError> {
        let Some(file) = read_text(workspace_path, filename, MEMORY_PROMPT_MAX_BYTES, true)? else {
            return Ok(None);
        };
        if file.truncated {
            tracing::warn!(
                workspace = %workspace_path.display(),
                filename,
                size_bytes = file.size_bytes,
                limit_bytes = MEMORY_PROMPT_MAX_BYTES,
                "工作空间文件过大，截断后加载"
            );
        }
        Ok(Some(file.text))
    }

    // 默认内容模板

    fn default_agents_content() -> &'static str {
        r"# JiaClaw Agents

This file describes the agents in your workspace.

## Primary Agent

**Name**: JiaClaw  
**Type**: Personal Assistant  
**Description**: A helpful personal assistant for daily tasks and knowledge work.

## Capabilities

- Natural language conversation
- Tool usage (when configured)
- Skill-based task execution
- Long-term memory (when configured)

## Configuration

See `config/jiaclaw.toml` for detailed configuration options.
"
    }

    fn default_soul_content() -> &'static str {
        r"# Agent Soul / 人格

此文件在每次对话开始时注入系统提示（独立区块 `## Soul（人格）`）。

- 只写稳定人格：语气、价值观、沟通风格；不要写临时任务状态。
- 默认路径：`{workspace}/SOUL.md`（可通过配置 `[identity] soul_path` 覆盖）。
- 文件不存在或为空时对话不会报错；超过 32KiB 时截断注入并 warn。
- 可用工具 `soul_write` 覆盖（默认）或追加。

## Personality Traits

- **Helpful**: Always eager to assist with tasks
- **Patient**: Takes time to understand user needs
- **Concise**: Provides clear, direct answers
- **Honest**: Admits limitations and uncertainties

## Communication Style

- Use simple, clear language
- Ask clarifying questions when needed
- Provide step-by-step explanations for complex topics
- Be proactive in suggesting helpful actions

## Values

- Privacy: Never share user information externally
- Accuracy: Prioritize correctness over speed
- Transparency: Explain reasoning and sources
"
    }

    fn default_user_content() -> &'static str {
        r"# User Profile / 用户画像

此文件在每次对话开始时注入系统提示（独立区块 `## User（用户画像）`）。

- 记录稳定的用户信息与偏好，便于个性化；不要写一次性上下文。
- 默认路径：`{workspace}/USER.md`（可通过配置 `[identity] user_path` 覆盖）。
- 文件不存在或为空时对话不会报错；超过 32KiB 时截断注入并 warn。
- 可用工具 `user_write` 覆盖（默认）或追加。

## About You

**Name**: [Your Name]  
**Role**: [Your Role/Occupation]  
**Timezone**: [Your Timezone]  

## Preferences

- **Communication**: [e.g., formal/casual, concise/detailed]
- **Language**: [Primary language(s)]
- **Working Hours**: [e.g., 9 AM - 5 PM]

## Common Tasks

List your frequently performed tasks here:

- [Task 1]
- [Task 2]
- [Task 3]

## Background Context

Add any relevant background information that helps JiaClaw understand your needs better.
"
    }

    fn default_memory_content() -> &'static str {
        r"# Long-term Memory / 长期记忆

此文件在每次对话开始时注入系统提示（内容原样）。可手动编辑，或让 Agent 调用 `memory_write`（`mode=append` 追加 / `mode=overwrite` 覆盖）或 `memory_append` 写入跨会话稳定事实。文件变长后可用 `memory_search` 按关键词检索片段。

- 只记录可复用的事实（偏好、约定、长期项目），不要写临时任务状态。
- 默认路径：`{workspace}/MEMORY.md`（可通过配置 `[memory] path` 覆盖）。
- 文件过大（超过 32KiB）时截断注入，并在日志中发出警告。

## Key Facts

- [Important fact 1]
- [Important fact 2]

## Ongoing Projects

### [Project Name]

- **Status**: [In Progress/Completed]
- **Description**: [Brief description]
- **Next Steps**: [What to do next]

## Learned Preferences

- [Preference 1]
- [Preference 2]
"
    }

    fn search_skill_content() -> &'static str {
        r#"---
name: search
description: Web search capability for finding information online
triggers:
  - search
  - 搜索
  - find
  - 查找
  - look up
---

# Search Skill

This skill provides web search capabilities for finding information, news, and current events.

## Tools

- `web_search` - Search the web using a search engine
- `web_fetch` - Fetch a URL and extract readable text

## Usage

When the user asks to search for information, news, or current events, use this skill.

## Examples

- "Search for the latest news on AI"
- "Find information about Rust programming"
- "Look up the weather forecast"
- "搜索最新的 AI 新闻"

## Implementation Status

⏳ **Planned** - Awaiting tool system implementation (M2)

## Dependencies

- HTTP client for web requests
- Search engine API (e.g., DuckDuckGo, Google Custom Search)
"#
    }

    fn calculator_skill_content() -> &'static str {
        r#"---
name: calculator
description: Mathematical computation and evaluation skill
triggers:
  - calculate
  - 计算
  - math
  - 数学
  - compute
---

# Calculator Skill

This skill provides mathematical computation and evaluation capabilities.

## Tools

- `evaluate` - Evaluate mathematical expressions
- `convert_units` - Convert between units (length, weight, temperature, etc.)

## Usage

When the user asks to perform calculations or unit conversions, use this skill.

## Examples

- "Calculate 15% tip on $85"
- "Convert 100 kilometers to miles"
- "What's the square root of 144?"
- "计算 123 * 456"

## Implementation Status

⏳ **Planned** - Awaiting tool system implementation (M2)

## Dependencies

- Math expression parser
- Unit conversion library
"#
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const DEFAULT_FILES: [&str; 6] = [
        "AGENTS.md",
        "SOUL.md",
        "USER.md",
        "MEMORY.md",
        "skills/search/SKILL.md",
        "skills/calculator/SKILL.md",
    ];

    fn assert_missing(workspace: &Workspace) {
        assert!(workspace.agents.is_none());
        assert!(workspace.soul.is_none());
        assert!(workspace.user.is_none());
        assert!(workspace.memory.is_none());
    }

    #[test]
    fn test_workspace_init() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        let workspace = Workspace::init(&root).unwrap();

        assert!(workspace.agents.is_some());
        assert!(workspace.soul.is_some());
        assert!(workspace.user.is_some());
        assert!(workspace.memory.is_some());
        for relative in DEFAULT_FILES {
            assert!(root.join(relative).is_file(), "missing {relative}");
        }
    }

    #[test]
    fn test_workspace_load_missing() {
        let temp = tempfile::tempdir().unwrap();
        assert_missing(&Workspace::load(temp.path()).unwrap());
        assert_missing(&Workspace::load(&temp.path().join("missing")).unwrap());
    }

    #[test]
    fn workspace_load_preserves_empty_files() {
        let temp = tempfile::tempdir().unwrap();
        for relative in &DEFAULT_FILES[..4] {
            fs::write(temp.path().join(relative), "").unwrap();
        }
        let workspace = Workspace::load(temp.path()).unwrap();
        for text in [
            workspace.agents,
            workspace.soul,
            workspace.user,
            workspace.memory,
        ] {
            assert_eq!(text.as_deref(), Some(""));
        }
    }

    #[test]
    fn workspace_load_bounds_each_file_at_utf8_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = "x".repeat(MEMORY_PROMPT_MAX_BYTES - 1);
        let content = format!("{prefix}中{}", "文".repeat(MEMORY_PROMPT_MAX_BYTES));
        for relative in &DEFAULT_FILES[..4] {
            fs::write(temp.path().join(relative), &content).unwrap();
        }
        let workspace = Workspace::load(temp.path()).unwrap();
        for text in [
            workspace.agents,
            workspace.soul,
            workspace.user,
            workspace.memory,
        ] {
            assert_eq!(text.as_deref(), Some(prefix.as_str()));
        }
    }

    #[test]
    fn workspace_load_propagates_invalid_utf8_and_non_file_errors() {
        for relative in &DEFAULT_FILES[..4] {
            let temp = tempfile::tempdir().unwrap();
            let target = temp.path().join(relative);
            fs::write(&target, [b'a', 0xff, b'b']).unwrap();
            assert!(Workspace::load(temp.path()).is_err(), "{relative}");
            fs::remove_file(&target).unwrap();
            fs::create_dir(&target).unwrap();
            assert!(Workspace::load(temp.path()).is_err(), "{relative}");
        }
    }

    #[test]
    fn workspace_init_preserves_all_existing_files_and_fills_missing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        Workspace::init(root).unwrap();
        for relative in DEFAULT_FILES {
            fs::write(root.join(relative), format!("user-content:{relative}")).unwrap();
        }
        Workspace::init(root).unwrap();
        for relative in DEFAULT_FILES {
            assert_eq!(
                fs::read_to_string(root.join(relative)).unwrap(),
                format!("user-content:{relative}")
            );
        }
        fs::remove_file(root.join(DEFAULT_FILES[5])).unwrap();
        Workspace::init(root).unwrap();
        for relative in &DEFAULT_FILES[..5] {
            assert_eq!(
                fs::read_to_string(root.join(relative)).unwrap(),
                format!("user-content:{relative}")
            );
        }
        assert_eq!(
            fs::read_to_string(root.join(DEFAULT_FILES[5])).unwrap(),
            Workspace::calculator_skill_content()
        );
    }

    #[test]
    fn workspace_init_preserves_locked_skills_in_both_modes() {
        use sha2::Digest;
        for overwrite in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            fs::create_dir_all(root.join("skills/search")).unwrap();
            let raw =
                "---\nname: reviewed\ndescription: Approved\ntriggers: []\n---\nOperator body";
            fs::write(root.join("skills/search/SKILL.md"), raw).unwrap();
            let lock = serde_json::to_string(&serde_json::json!({"version":2,"skills":[{
                "directory":"search","enabled":true,"source":{
                    "repository":"https://github.com/example/reviewed","revision":"a".repeat(40),
                    "skill_sha256":format!("{:x}",sha2::Sha256::digest(raw.as_bytes()))
                }
            }]}))
            .unwrap();
            fs::write(root.join("skills.lock.json"), &lock).unwrap();
            Workspace::init_with_overwrite(root, overwrite).unwrap();
            assert_eq!(
                fs::read_to_string(root.join("skills.lock.json")).unwrap(),
                lock
            );
            assert_eq!(
                fs::read_to_string(root.join("skills/search/SKILL.md")).unwrap(),
                raw
            );
            assert!(!root.join("skills/calculator").exists());
            assert_eq!(
                crate::SkillDiscovery::new(root)
                    .with_lock_required(true)
                    .discover()
                    .unwrap()
                    .len(),
                1
            );
            for relative in &DEFAULT_FILES[..4] {
                assert!(root.join(relative).is_file());
            }
        }
    }

    #[test]
    fn workspace_init_explicit_overwrite_replaces_all_regular_defaults() {
        let temp = tempfile::tempdir().unwrap();
        Workspace::init(temp.path()).unwrap();
        let expected: Vec<_> = DEFAULT_FILES
            .iter()
            .map(|relative| fs::read(temp.path().join(relative)).unwrap())
            .collect();
        for relative in DEFAULT_FILES {
            fs::write(temp.path().join(relative), "user-content").unwrap();
        }
        Workspace::init_with_overwrite(temp.path(), true).unwrap();
        for (relative, contents) in DEFAULT_FILES.iter().zip(expected) {
            assert_eq!(fs::read(temp.path().join(relative)).unwrap(), contents);
        }
    }

    #[test]
    fn workspace_init_rejects_wrong_directory_and_file_types() {
        for overwrite in [false, true] {
            for relative in DEFAULT_FILES {
                let temp = tempfile::tempdir().unwrap();
                let target = temp.path().join(relative);
                fs::create_dir_all(&target).unwrap();
                assert!(Workspace::init_with_overwrite(temp.path(), overwrite).is_err());
                assert!(target.is_dir());
            }
            for relative in ["skills", "skills/search", "skills/calculator"] {
                let temp = tempfile::tempdir().unwrap();
                let target = temp.path().join(relative);
                fs::create_dir_all(target.parent().unwrap()).unwrap();
                fs::write(&target, "keep").unwrap();
                assert!(Workspace::init_with_overwrite(temp.path(), overwrite).is_err());
                assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn workspace_load_rejects_links_and_special_files() {
        use std::os::unix::{fs::symlink, net::UnixListener};

        for relative in &DEFAULT_FILES[..4] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("workspace");
            fs::create_dir(&root).unwrap();
            let outside = temp.path().join("outside");
            fs::write(&outside, "private").unwrap();
            let target = root.join(relative);
            symlink(&outside, &target).unwrap();
            assert!(Workspace::load(&root).is_err(), "symlink {relative}");
            fs::remove_file(&target).unwrap();
            symlink(temp.path().join("absent"), &target).unwrap();
            assert!(Workspace::load(&root).is_err(), "dangling {relative}");
            fs::remove_file(&target).unwrap();
            fs::hard_link(&outside, &target).unwrap();
            assert!(Workspace::load(&root).is_err(), "hard link {relative}");
            fs::remove_file(&target).unwrap();
            let _socket = UnixListener::bind(&target).unwrap();
            assert!(Workspace::load(&root).is_err(), "socket {relative}");
            assert_eq!(fs::read_to_string(&outside).unwrap(), "private");
        }
    }

    #[cfg(unix)]
    #[test]
    fn workspace_init_rejects_linked_files_even_when_forced() {
        use std::os::unix::fs::symlink;

        for overwrite in [false, true] {
            for relative in DEFAULT_FILES {
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path().join("workspace");
                let target = root.join(relative);
                fs::create_dir_all(target.parent().unwrap()).unwrap();
                let outside = temp.path().join("outside");
                fs::write(&outside, "private").unwrap();
                symlink(&outside, &target).unwrap();
                assert!(Workspace::init_with_overwrite(&root, overwrite).is_err());
                assert_eq!(fs::read_to_string(&outside).unwrap(), "private");
                fs::remove_file(&target).unwrap();
                fs::hard_link(&outside, &target).unwrap();
                assert!(Workspace::init_with_overwrite(&root, overwrite).is_err());
                assert_eq!(fs::read_to_string(&outside).unwrap(), "private");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn workspace_init_rejects_linked_skill_directories() {
        use std::os::unix::fs::symlink;

        for overwrite in [false, true] {
            for relative in ["skills", "skills/search", "skills/calculator"] {
                for dangling in [false, true] {
                    let temp = tempfile::tempdir().unwrap();
                    let root = temp.path().join("workspace");
                    let target = root.join(relative);
                    fs::create_dir_all(target.parent().unwrap()).unwrap();
                    let outside = temp.path().join("outside");
                    if !dangling {
                        fs::create_dir(&outside).unwrap();
                        fs::write(outside.join("keep"), "private").unwrap();
                    }
                    symlink(&outside, &target).unwrap();
                    assert!(Workspace::init_with_overwrite(&root, overwrite).is_err());
                    if dangling {
                        assert!(!outside.exists());
                    } else {
                        assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "private");
                        assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
                    }
                }
            }
        }
    }
}
