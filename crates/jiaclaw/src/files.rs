// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 工作区文件工具：`read_file` / `list_dir` / `write_file` / `delete_file` / `str_replace` / `grep` / `glob` / `mkdir` / `move`。
//!
//! 前五项使用目录句柄与有界 blocking I/O；其余工具沿用路径解析与各自限制。
//! 不调用 LLM，不执行 shell。

use crate::memory::{
    canonicalize_existing_or_clone, ensure_existing_within_workspace,
    resolve_workspace_relative_path,
};
use crate::tools::Tool;
use async_trait::async_trait;
use jiaclaw_core::JiaClawError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

/// `read_file` 单文件读取上限（字节）。超过则明确报错，不截断返回。
pub const READ_FILE_MAX_BYTES: usize = 256 * 1024;

/// `write_file` 结果文件上限（字节）。与 [`READ_FILE_MAX_BYTES`] 对齐。超过则报错且不落盘。
pub const WRITE_FILE_MAX_BYTES: usize = READ_FILE_MAX_BYTES;

/// `str_replace` 读入与写出上限（字节）。与 [`READ_FILE_MAX_BYTES`] / [`WRITE_FILE_MAX_BYTES`] 对齐。
pub const STR_REPLACE_MAX_BYTES: usize = READ_FILE_MAX_BYTES;

/// `grep` 单文件读取上限（字节）。超过则跳过该文件（单文件目标则报错）。与 [`READ_FILE_MAX_BYTES`] 对齐。
pub const GREP_FILE_MAX_BYTES: usize = READ_FILE_MAX_BYTES;

/// `grep` 的 `max_matches` 缺省值
pub const GREP_DEFAULT_MAX_MATCHES: usize = 50;

/// `grep` 的 `max_matches` 上限（含）
pub const GREP_MAX_MATCHES: usize = 200;

/// `grep` 返回 snippet 的最大字符数（按 Unicode 标量，超出截断）
pub const GREP_SNIPPET_MAX_CHARS: usize = 200;

/// `grep` 一次扫描的最大常规文件数（含 glob 未命中的文件）
pub const GREP_MAX_FILES_SCANNED: usize = 2000;

/// `grep` 字面量 `pattern` 最大字节数
pub const GREP_PATTERN_MAX_BYTES: usize = 512;

/// `grep` `glob` 模式最大字节数
pub const GREP_GLOB_MAX_BYTES: usize = 128;

/// `glob` 的 `max_results` 缺省值
pub const GLOB_DEFAULT_MAX_RESULTS: usize = 100;

/// `glob` 的 `max_results` 上限（含）
pub const GLOB_MAX_RESULTS: usize = 500;

/// `glob` 一次扫描的最大常规文件数（含 pattern 未命中的文件）
pub const GLOB_MAX_FILES_SCANNED: usize = 2000;

/// `glob` `pattern` 最大字节数
pub const GLOB_PATTERN_MAX_BYTES: usize = 256;

/// `list_dir` 的 `max_entries` 缺省值
pub const LIST_DIR_DEFAULT_MAX_ENTRIES: usize = 200;

/// `list_dir` 的 `max_entries` 上限（含）
pub const LIST_DIR_MAX_ENTRIES: usize = 1000;

/// 解析后的 `read_file` 参数。`offset` 为 **1-indexed 行号**；`limit` 为行数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadFileArgs {
    /// 工作区相对路径
    pub path: String,
    /// 起始行（从 1 起；缺省为 1）
    pub offset: usize,
    /// 最多返回的行数；`None` 表示读到文件末尾（仍受大小上限约束）
    pub limit: Option<usize>,
}

/// 解析后的 `list_dir` 参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListDirArgs {
    /// 工作区相对路径（缺省 `.`）
    pub path: String,
    /// 最多返回的条目数（钳制 1..=1000）
    pub max_entries: usize,
    /// 是否递归（默认 `false`）
    pub recursive: bool,
}

/// `write_file` 写入模式：覆盖或追加。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WriteFileMode {
    /// 覆盖整个文件（默认）
    Overwrite,
    /// 在已有内容后追加
    Append,
}

impl WriteFileMode {
    /// 解析 `overwrite` / `append`（大小写不敏感，首尾空白忽略）。
    ///
    /// # Errors
    ///
    /// 其它字符串返回错误。
    pub fn parse(raw: &str) -> Result<Self, JiaClawError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "overwrite" => Ok(Self::Overwrite),
            "append" => Ok(Self::Append),
            _ => Err(JiaClawError::ToolExecution(
                "参数 'mode' 必须是 overwrite 或 append（默认 overwrite）".to_string(),
            )),
        }
    }

    /// 配置 / JSON 用短名。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Overwrite => "overwrite",
            Self::Append => "append",
        }
    }

    /// `overwrite` 对应整文件替换。
    #[must_use]
    pub fn is_overwrite(self) -> bool {
        matches!(self, Self::Overwrite)
    }
}

/// 解析后的 `write_file` 参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteFileArgs {
    /// 工作区相对路径
    pub path: String,
    /// 要写入的内容
    pub content: String,
    /// 写入模式（默认 overwrite）
    pub mode: WriteFileMode,
}

/// 解析后的 `delete_file` 参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteFileArgs {
    /// 工作区相对路径
    pub path: String,
}

/// 解析后的 `str_replace` 参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrReplaceArgs {
    /// 工作区相对路径
    pub path: String,
    /// 要查找的精确子串（非空）
    pub old_str: String,
    /// 替换后的文本（可为空，表示删除匹配）
    pub new_str: String,
    /// 是否替换全部匹配（默认 `false`：必须恰好 1 次）
    pub replace_all: bool,
}

/// 解析后的 `grep` 参数。`pattern` 为**字面量**子串，不是正则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepArgs {
    /// 要查找的字面量子串（非空）
    pub pattern: String,
    /// 工作区相对目录或文件（缺省 `.`）
    pub path: String,
    /// 可选 glob（如 `*.rs`）；`None` 表示不过滤
    pub glob: Option<String>,
    /// 是否忽略大小写（默认 `false`）
    pub case_insensitive: bool,
    /// 最多返回的匹配条数（钳制 1..=200）
    pub max_matches: usize,
}

/// 解析后的 `glob` 参数。`pattern` 为 glob 模式（如 `**/*.rs`），不是正则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobArgs {
    /// glob 模式（必填，非空）
    pub pattern: String,
    /// 工作区相对搜索根（缺省 `.`）
    pub path: String,
    /// 最多返回的文件路径数（钳制 1..=500）
    pub max_results: usize,
}

/// 解析后的 `mkdir` 参数。`recursive` / `parents` 为别名，默认 `true`（等价 `mkdir -p`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MkdirArgs {
    /// 工作区相对路径
    pub path: String,
    /// 是否创建中间目录（默认 `true`）
    pub recursive: bool,
}

/// `move` 源/目标类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MoveKind {
    /// 常规文件
    File,
    /// 目录（含非空目录；同卷 [`std::fs::rename`]）
    Dir,
}

impl MoveKind {
    /// JSON / 文档用短名。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Dir => "dir",
        }
    }
}

/// 解析后的 `move` 参数。`from` / `source` 与 `to` / `destination` 为别名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveArgs {
    /// 源路径（工作区相对；文档主名 `from`）
    pub from: String,
    /// 目标路径（工作区相对；文档主名 `to`）
    pub to: String,
    /// 目标已存在时是否覆盖（默认 `false`）
    pub overwrite: bool,
}

/// 将 `max_entries` 钳制到 `1..=1000`。
#[must_use]
pub fn clamp_list_dir_max_entries(raw: u64) -> usize {
    usize::try_from(raw)
        .unwrap_or(LIST_DIR_MAX_ENTRIES)
        .clamp(1, LIST_DIR_MAX_ENTRIES)
}

/// 将 `max_matches` 钳制到 `1..=200`。
#[must_use]
pub fn clamp_grep_max_matches(raw: u64) -> usize {
    usize::try_from(raw)
        .unwrap_or(GREP_MAX_MATCHES)
        .clamp(1, GREP_MAX_MATCHES)
}

/// 将 `max_results` 钳制到 `1..=500`。
#[must_use]
pub fn clamp_glob_max_results(raw: u64) -> usize {
    usize::try_from(raw)
        .unwrap_or(GLOB_MAX_RESULTS)
        .clamp(1, GLOB_MAX_RESULTS)
}

fn parse_positive_usize(value: &Value, name: &str) -> Result<usize, JiaClawError> {
    let raw = value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()));
    match raw {
        Some(n) => usize::try_from(n)
            .map_err(|_| JiaClawError::ToolExecution(format!("参数 '{name}' 超出范围"))),
        None => Err(JiaClawError::ToolExecution(format!(
            "参数 '{name}' 必须是正整数"
        ))),
    }
}

fn parse_optional_bool_arg(
    args: &Value,
    name: &str,
    default_hint: &str,
) -> Result<Option<bool>, JiaClawError> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(_) => Err(JiaClawError::ToolExecution(format!(
            "参数 '{name}' 必须是布尔值（默认 {default_hint}）"
        ))),
    }
}

/// 解析 `read_file` 参数：必填 `path`；可选 `offset` / `limit`（按行，1-indexed）。
///
/// # Errors
///
/// `path` 缺失/空白，或 `offset`/`limit` 不是整数时返回错误。
pub fn parse_read_file_args(args: &Value) -> Result<ReadFileArgs, JiaClawError> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            JiaClawError::ToolExecution("缺少参数 'path'（工作区相对路径）".to_string())
        })?;

    let offset = match args.get("offset") {
        None | Some(Value::Null) => 1,
        Some(value) => {
            let n = parse_positive_usize(value, "offset")?;
            n.max(1)
        }
    };

    let limit = match args.get("limit") {
        None | Some(Value::Null) => None,
        Some(value) => Some(parse_positive_usize(value, "limit")?.max(1)),
    };

    Ok(ReadFileArgs {
        path: path.to_string(),
        offset,
        limit,
    })
}

/// 解析 `list_dir` 参数：可选 `path`（默认 `.`）、`max_entries`（默认 200）、`recursive`（默认 false）。
///
/// # Errors
///
/// `max_entries` 不是整数，或 `recursive` 不是布尔值时返回错误。
pub fn parse_list_dir_args(args: &Value) -> Result<ListDirArgs, JiaClawError> {
    let path = match args.get("path") {
        None | Some(Value::Null) => ".".to_string(),
        Some(value) => {
            let raw = value.as_str().ok_or_else(|| {
                JiaClawError::ToolExecution(
                    "参数 'path' 必须是字符串（工作区相对路径，默认 .）".to_string(),
                )
            })?;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                ".".to_string()
            } else {
                trimmed.to_string()
            }
        }
    };

    let max_entries = match args.get("max_entries") {
        None | Some(Value::Null) => LIST_DIR_DEFAULT_MAX_ENTRIES,
        Some(value) => {
            let raw = value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()));
            match raw {
                Some(n) => clamp_list_dir_max_entries(n),
                None => {
                    return Err(JiaClawError::ToolExecution(
                        "参数 'max_entries' 必须是整数（将钳制到 1..=1000）".to_string(),
                    ));
                }
            }
        }
    };

    let recursive = match args.get("recursive") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => {
            return Err(JiaClawError::ToolExecution(
                "参数 'recursive' 必须是布尔值（默认 false）".to_string(),
            ));
        }
    };

    Ok(ListDirArgs {
        path,
        max_entries,
        recursive,
    })
}

/// 解析 `write_file` 参数：必填 `path` / `content`；可选 `mode`（默认 `overwrite`）。
///
/// # Errors
///
/// `path` 缺失/空白、`content` 缺失或不是字符串、或 `mode` 非法时返回错误。
pub fn parse_write_file_args(args: &Value) -> Result<WriteFileArgs, JiaClawError> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            JiaClawError::ToolExecution("缺少参数 'path'（工作区相对路径）".to_string())
        })?;

    let content = match args.get("content") {
        Some(Value::String(text)) => text.clone(),
        None | Some(Value::Null) => {
            return Err(JiaClawError::ToolExecution(
                "缺少参数 'content'".to_string(),
            ));
        }
        Some(_) => {
            return Err(JiaClawError::ToolExecution(
                "参数 'content' 必须是字符串".to_string(),
            ));
        }
    };

    let mode = match args.get("mode") {
        None | Some(Value::Null) => WriteFileMode::Overwrite,
        Some(Value::String(raw)) => WriteFileMode::parse(raw)?,
        Some(_) => {
            return Err(JiaClawError::ToolExecution(
                "参数 'mode' 必须是 overwrite 或 append（默认 overwrite）".to_string(),
            ));
        }
    };

    Ok(WriteFileArgs {
        path: path.to_string(),
        content,
        mode,
    })
}

/// 解析 `delete_file` 参数：必填 `path`。
///
/// # Errors
///
/// `path` 缺失或空白时返回错误。
pub fn parse_delete_file_args(args: &Value) -> Result<DeleteFileArgs, JiaClawError> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            JiaClawError::ToolExecution("缺少参数 'path'（工作区相对路径）".to_string())
        })?;

    Ok(DeleteFileArgs {
        path: path.to_string(),
    })
}

/// 解析 `str_replace` 参数：必填 `path` / `old_str` / `new_str`；可选 `replace_all`（默认 `false`）。
///
/// # Errors
///
/// `path` 缺失/空白、`old_str` 缺失或为空、`new_str` 缺失或不是字符串、或 `replace_all` 不是布尔值时返回错误。
pub fn parse_str_replace_args(args: &Value) -> Result<StrReplaceArgs, JiaClawError> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            JiaClawError::ToolExecution("缺少参数 'path'（工作区相对路径）".to_string())
        })?;

    let old_str = match args.get("old_str") {
        Some(Value::String(text)) if !text.is_empty() => text.clone(),
        Some(Value::String(_)) => {
            return Err(JiaClawError::ToolExecution(
                "参数 'old_str' 不能为空".to_string(),
            ));
        }
        None | Some(Value::Null) => {
            return Err(JiaClawError::ToolExecution(
                "缺少参数 'old_str'".to_string(),
            ));
        }
        Some(_) => {
            return Err(JiaClawError::ToolExecution(
                "参数 'old_str' 必须是字符串".to_string(),
            ));
        }
    };

    let new_str = match args.get("new_str") {
        Some(Value::String(text)) => text.clone(),
        None | Some(Value::Null) => {
            return Err(JiaClawError::ToolExecution(
                "缺少参数 'new_str'".to_string(),
            ));
        }
        Some(_) => {
            return Err(JiaClawError::ToolExecution(
                "参数 'new_str' 必须是字符串".to_string(),
            ));
        }
    };

    let replace_all = match args.get("replace_all") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => {
            return Err(JiaClawError::ToolExecution(
                "参数 'replace_all' 必须是布尔值（默认 false）".to_string(),
            ));
        }
    };

    Ok(StrReplaceArgs {
        path: path.to_string(),
        old_str,
        new_str,
        replace_all,
    })
}

/// 解析 `grep` 参数：必填 `pattern`（字面量）；可选 `path`（默认 `.`）、`glob`、`case_insensitive`（默认 false）、`max_matches`（默认 50）。
///
/// # Errors
///
/// `pattern` 缺失/为空/过长，`path`/`glob` 不是字符串，`case_insensitive` 不是布尔值，或 `max_matches` 不是整数时返回错误。
pub fn parse_grep_args(args: &Value) -> Result<GrepArgs, JiaClawError> {
    let pattern = match args.get("pattern") {
        Some(Value::String(text)) if !text.is_empty() => text.clone(),
        Some(Value::String(_)) => {
            return Err(JiaClawError::ToolExecution(
                "参数 'pattern' 不能为空".to_string(),
            ));
        }
        None | Some(Value::Null) => {
            return Err(JiaClawError::ToolExecution(
                "缺少参数 'pattern'（字面量子串，非正则）".to_string(),
            ));
        }
        Some(_) => {
            return Err(JiaClawError::ToolExecution(
                "参数 'pattern' 必须是字符串".to_string(),
            ));
        }
    };
    if pattern.len() > GREP_PATTERN_MAX_BYTES {
        return Err(JiaClawError::ToolExecution(format!(
            "参数 'pattern' 超过上限 {GREP_PATTERN_MAX_BYTES} 字节"
        )));
    }

    let path = match args.get("path") {
        None | Some(Value::Null) => ".".to_string(),
        Some(value) => {
            let raw = value.as_str().ok_or_else(|| {
                JiaClawError::ToolExecution(
                    "参数 'path' 必须是字符串（工作区相对目录或文件，默认 .）".to_string(),
                )
            })?;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                ".".to_string()
            } else {
                trimmed.to_string()
            }
        }
    };

    let glob = match args.get("glob") {
        None | Some(Value::Null) => None,
        Some(Value::String(raw)) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else if trimmed.len() > GREP_GLOB_MAX_BYTES {
                return Err(JiaClawError::ToolExecution(format!(
                    "参数 'glob' 超过上限 {GREP_GLOB_MAX_BYTES} 字节"
                )));
            } else {
                Some(trimmed.to_string())
            }
        }
        Some(_) => {
            return Err(JiaClawError::ToolExecution(
                "参数 'glob' 必须是字符串（如 *.rs）".to_string(),
            ));
        }
    };

    let case_insensitive = match args.get("case_insensitive") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => {
            return Err(JiaClawError::ToolExecution(
                "参数 'case_insensitive' 必须是布尔值（默认 false）".to_string(),
            ));
        }
    };

    let max_matches = match args.get("max_matches") {
        None | Some(Value::Null) => GREP_DEFAULT_MAX_MATCHES,
        Some(value) => {
            let raw = value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()));
            match raw {
                Some(n) => clamp_grep_max_matches(n),
                None => {
                    return Err(JiaClawError::ToolExecution(
                        "参数 'max_matches' 必须是整数（将钳制到 1..=200）".to_string(),
                    ));
                }
            }
        }
    };

    Ok(GrepArgs {
        pattern,
        path,
        glob,
        case_insensitive,
        max_matches,
    })
}

/// 解析 `glob` 参数：必填 `pattern`（glob 模式）；可选 `path`（默认 `.`）、`max_results`（默认 100）。
///
/// # Errors
///
/// `pattern` 缺失/为空/过长，`path` 不是字符串，或 `max_results` 不是整数时返回错误。
pub fn parse_glob_args(args: &Value) -> Result<GlobArgs, JiaClawError> {
    let pattern = match args.get("pattern") {
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Err(JiaClawError::ToolExecution(
                    "参数 'pattern' 不能为空".to_string(),
                ));
            }
            trimmed.to_string()
        }
        None | Some(Value::Null) => {
            return Err(JiaClawError::ToolExecution(
                "缺少参数 'pattern'（glob 模式，如 **/*.rs）".to_string(),
            ));
        }
        Some(_) => {
            return Err(JiaClawError::ToolExecution(
                "参数 'pattern' 必须是字符串".to_string(),
            ));
        }
    };
    if pattern.len() > GLOB_PATTERN_MAX_BYTES {
        return Err(JiaClawError::ToolExecution(format!(
            "参数 'pattern' 超过上限 {GLOB_PATTERN_MAX_BYTES} 字节"
        )));
    }

    let path = match args.get("path") {
        None | Some(Value::Null) => ".".to_string(),
        Some(value) => {
            let raw = value.as_str().ok_or_else(|| {
                JiaClawError::ToolExecution(
                    "参数 'path' 必须是字符串（工作区相对搜索根，默认 .）".to_string(),
                )
            })?;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                ".".to_string()
            } else {
                trimmed.to_string()
            }
        }
    };

    let max_results = match args.get("max_results") {
        None | Some(Value::Null) => GLOB_DEFAULT_MAX_RESULTS,
        Some(value) => {
            let raw = value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()));
            match raw {
                Some(n) => clamp_glob_max_results(n),
                None => {
                    return Err(JiaClawError::ToolExecution(
                        "参数 'max_results' 必须是整数（将钳制到 1..=500）".to_string(),
                    ));
                }
            }
        }
    };

    Ok(GlobArgs {
        pattern,
        path,
        max_results,
    })
}

/// 解析 `mkdir` 参数：必填 `path`；可选 `recursive` / `parents`（默认 `true`，等价 `mkdir -p`）。
///
/// `recursive` 与 `parents` 为别名；同时给出时必须一致。
///
/// # Errors
///
/// `path` 缺失/空白，`recursive`/`parents` 不是布尔值，或两者冲突时返回错误。
pub fn parse_mkdir_args(args: &Value) -> Result<MkdirArgs, JiaClawError> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            JiaClawError::ToolExecution("缺少参数 'path'（工作区相对路径）".to_string())
        })?;

    let recursive_opt = parse_optional_bool_arg(args, "recursive", "true")?;
    let parents_opt = parse_optional_bool_arg(args, "parents", "true")?;
    let recursive = match (recursive_opt, parents_opt) {
        (None, None) => true,
        (Some(flag), None) | (None, Some(flag)) => flag,
        (Some(left), Some(right)) if left == right => left,
        (Some(_), Some(_)) => {
            return Err(JiaClawError::ToolExecution(
                "参数 'recursive' 与 'parents' 必须一致（均为 mkdir -p 别名，默认 true）"
                    .to_string(),
            ));
        }
    };

    Ok(MkdirArgs {
        path: path.to_string(),
        recursive,
    })
}

fn parse_optional_trimmed_path_arg(
    args: &Value,
    name: &str,
) -> Result<Option<String>, JiaClawError> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Err(JiaClawError::ToolExecution(format!(
                    "参数 '{name}' 不能为空（工作区相对路径）"
                )))
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Some(_) => Err(JiaClawError::ToolExecution(format!(
            "参数 '{name}' 必须是字符串（工作区相对路径）"
        ))),
    }
}

fn parse_required_aliased_path(
    args: &Value,
    primary: &str,
    alias: &str,
) -> Result<String, JiaClawError> {
    let primary_val = parse_optional_trimmed_path_arg(args, primary)?;
    let alias_val = parse_optional_trimmed_path_arg(args, alias)?;
    match (primary_val, alias_val) {
        (None, None) => Err(JiaClawError::ToolExecution(format!(
            "缺少参数 '{primary}' 或 '{alias}'（工作区相对路径）"
        ))),
        (Some(value), None) | (None, Some(value)) => Ok(value),
        (Some(left), Some(right)) if left == right => Ok(left),
        (Some(_), Some(_)) => Err(JiaClawError::ToolExecution(format!(
            "参数 '{primary}' 与 '{alias}' 必须一致（均为路径别名）"
        ))),
    }
}

/// 解析 `move` 参数：必填 `from` / `source` 与 `to` / `destination`；可选 `overwrite`（默认 `false`）。
///
/// 文档主名为 `from` 与 `to`；`source` / `destination` 为别名。成对同时给出时必须一致。
///
/// # Errors
///
/// 路径缺失/空白、别名冲突，或 `overwrite` 不是布尔值时返回错误。
pub fn parse_move_args(args: &Value) -> Result<MoveArgs, JiaClawError> {
    let from = parse_required_aliased_path(args, "from", "source")?;
    let to = parse_required_aliased_path(args, "to", "destination")?;
    let overwrite = parse_optional_bool_arg(args, "overwrite", "false")?.unwrap_or(false);
    Ok(MoveArgs {
        from,
        to,
        overwrite,
    })
}

/// 解析工作区相对创建目标。
///
/// 已存在路径 canonicalize 后必须落在工作区内；不存在时沿已存在祖先
/// canonicalize，再拼回剩余组件（可随后创建中间目录）。
///
/// 返回 `(path, existed)`：`existed` 表示目标路径本身已存在。
fn prepare_workspace_create_path(
    workspace: &Path,
    rel_path: &str,
) -> Result<(PathBuf, bool), JiaClawError> {
    let joined = resolve_workspace_relative_path(workspace, rel_path)?;
    let ws = canonicalize_existing_or_clone(workspace);

    let mut ancestor = joined.clone();
    let mut suffix: Vec<OsString> = Vec::new();
    loop {
        if fs::symlink_metadata(&ancestor).is_ok() {
            break;
        }
        match ancestor.file_name() {
            Some(name) => {
                suffix.push(name.to_os_string());
                match ancestor.parent() {
                    Some(parent) => ancestor = parent.to_path_buf(),
                    None => break,
                }
            }
            None => break,
        }
        if ancestor == ws {
            break;
        }
    }

    if fs::symlink_metadata(&ancestor).is_err() {
        return Err(JiaClawError::ToolExecution(format!(
            "无法解析路径 {rel_path}"
        )));
    }
    if !ancestor.exists() {
        return Err(JiaClawError::ToolExecution(format!(
            "安全错误: 路径 {rel_path} 指向无效或损坏的链接"
        )));
    }

    ensure_existing_within_workspace(workspace, &ancestor)?;
    let mut current = ancestor
        .canonicalize()
        .map_err(|e| JiaClawError::ToolExecution(format!("无法解析路径 {rel_path}: {e}")))?;
    if !current.starts_with(&ws) {
        return Err(JiaClawError::ToolExecution(format!(
            "安全错误: 路径 {rel_path} 指向工作空间外部"
        )));
    }

    if suffix.is_empty() {
        return Ok((current, true));
    }

    if !current.is_dir() {
        return Err(JiaClawError::ToolExecution(format!(
            "路径的父级不是目录: {rel_path}"
        )));
    }

    for name in suffix.iter().rev() {
        current.push(name);
    }
    if !current.starts_with(&ws) {
        return Err(JiaClawError::ToolExecution(format!(
            "安全错误: 路径 {rel_path} 指向工作空间外部"
        )));
    }
    Ok((current, false))
}

/// 写入工作区相对路径下的常规文件（覆盖或追加）。可创建中间目录。
///
/// # Errors
///
/// 路径非法、越出工作空间、symlink 逃逸、不是常规文件、超过大小上限，或 IO 失败时返回错误。
/// 超限时不落盘。
pub fn write_workspace_regular_file(
    workspace: &Path,
    rel_path: &str,
    content: &str,
    mode: WriteFileMode,
    max_bytes: usize,
) -> Result<WriteFileOutput, JiaClawError> {
    let bytes_written = crate::memory_io::write_file_bytes(
        workspace,
        rel_path,
        content,
        mode == WriteFileMode::Append,
        max_bytes,
    )?;
    Ok(WriteFileOutput {
        path: rel_path.to_string(),
        mode,
        bytes_written,
    })
}

/// 删除工作区相对路径下的常规文件。不递归、不删除目录、不调用 shell。
///
/// # Errors
///
/// 路径非法、越出工作空间、symlink 逃逸、目标是目录、文件不存在，或 IO 失败时返回错误。
/// 缺文件时明确报错，不静默成功。
pub fn delete_workspace_regular_file(
    workspace: &Path,
    rel_path: &str,
) -> Result<DeleteFileOutput, JiaClawError> {
    let size_bytes = crate::memory_io::delete_file(workspace, rel_path)?;
    Ok(DeleteFileOutput {
        path: rel_path.to_string(),
        deleted: true,
        size_bytes,
    })
}

/// 在工作区相对路径的常规文本文件内做精确字符串替换。
///
/// `replace_all = false`（默认）时 `old_str` 必须恰好出现 1 次；`true` 时替换全部非重叠匹配。
/// 0 次匹配始终报错。读入或写出超过 `max_bytes` 时报错且不落盘。
///
/// # Errors
///
/// 路径非法、越出工作空间、symlink 逃逸、不是常规文本文件、二进制、匹配次数不符合、超过大小上限，或 IO 失败时返回错误。
pub fn str_replace_workspace_file(
    workspace: &Path,
    rel_path: &str,
    old_str: &str,
    new_str: &str,
    replace_all: bool,
    max_bytes: usize,
) -> Result<StrReplaceOutput, JiaClawError> {
    let (replacements, bytes_written) = crate::memory_io::replace_file_text(
        workspace,
        rel_path,
        old_str,
        new_str,
        replace_all,
        max_bytes,
    )?;
    Ok(StrReplaceOutput {
        path: rel_path.to_string(),
        replacements,
        bytes_written,
        replace_all,
    })
}

/// 简单 glob：`*` 匹配单段内任意字符，`?` 匹配单字符，`**` 匹配跨目录。
/// 不含 `/` 的模式只对文件名生效（`*.rs` 可匹配 `src/lib.rs`）。
#[must_use]
pub fn glob_matches(pattern: &str, rel_path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    let rel_path = rel_path.replace('\\', "/");
    if pattern.is_empty() {
        return true;
    }
    let pattern = pattern.trim_start_matches("./");
    let rel_path = rel_path.trim_start_matches("./");
    if !pattern.contains('/') {
        let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
        return glob_match_segment(pattern, name);
    }
    let glob_segs: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let file_segs: Vec<&str> = rel_path.split('/').filter(|s| !s.is_empty()).collect();
    glob_match_parts(&glob_segs, &file_segs)
}

fn glob_match_parts(glob_segs: &[&str], file_segs: &[&str]) -> bool {
    match (glob_segs.split_first(), file_segs.split_first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some((&"**", rest)), None) => glob_match_parts(rest, file_segs),
        (Some((&"**", rest)), Some((_, remaining))) => {
            glob_match_parts(rest, file_segs) || glob_match_parts(glob_segs, remaining)
        }
        (Some((segment, rest)), Some((name, remaining))) => {
            glob_match_segment(segment, name) && glob_match_parts(rest, remaining)
        }
        (Some((segment, rest)), None) => *segment == "**" && glob_match_parts(rest, file_segs),
    }
}

fn glob_match_segment(glob: &str, text: &str) -> bool {
    let glob_chars: Vec<char> = glob.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    let mut glob_idx = 0;
    let mut text_idx = 0;
    let mut star_glob: Option<usize> = None;
    let mut star_text = 0;
    while text_idx < text_chars.len() {
        if glob_idx < glob_chars.len()
            && glob_chars[glob_idx] != '*'
            && (glob_chars[glob_idx] == '?' || glob_chars[glob_idx] == text_chars[text_idx])
        {
            glob_idx += 1;
            text_idx += 1;
        } else if glob_idx < glob_chars.len() && glob_chars[glob_idx] == '*' {
            star_glob = Some(glob_idx);
            star_text = text_idx;
            glob_idx += 1;
        } else if let Some(star_at) = star_glob {
            glob_idx = star_at + 1;
            star_text += 1;
            text_idx = star_text;
        } else {
            return false;
        }
    }
    while glob_idx < glob_chars.len() && glob_chars[glob_idx] == '*' {
        glob_idx += 1;
    }
    glob_idx == glob_chars.len()
}

fn workspace_rel_display(workspace: &Path, abs: &Path) -> String {
    match abs.strip_prefix(workspace) {
        Ok(rel) => {
            let text = rel.to_string_lossy().replace('\\', "/");
            if text.is_empty() {
                ".".to_string()
            } else {
                text
            }
        }
        Err(_) => ".".to_string(),
    }
}

fn join_rel(prefix: &str, name: &str) -> String {
    if prefix.is_empty() || prefix == "." {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

fn truncate_snippet(line: &str) -> String {
    let trimmed = line.trim_end();
    let mut chars = trimmed.chars();
    let taken: String = chars.by_ref().take(GREP_SNIPPET_MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{taken}…")
    } else {
        taken
    }
}

fn line_contains_literal(line: &str, pattern: &str, case_insensitive: bool) -> bool {
    if case_insensitive {
        line.to_lowercase().contains(&pattern.to_lowercase())
    } else {
        line.contains(pattern)
    }
}

fn glob_allows(glob: Option<&str>, rel_path: &str) -> bool {
    glob.is_none_or(|pattern| glob_matches(pattern, rel_path))
}

fn read_text_file_for_grep(
    path: &Path,
    rel_path: &str,
    file_max_bytes: usize,
    fail_on_skip: bool,
) -> Result<Option<String>, JiaClawError> {
    let size_bytes = fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| JiaClawError::ToolExecution(format!("无法读取文件元数据 {rel_path}: {e}")))?;
    let limit_bytes = u64::try_from(file_max_bytes).unwrap_or(u64::MAX);
    if size_bytes > limit_bytes {
        if fail_on_skip {
            return Err(JiaClawError::ToolExecution(format!(
                "文件超过上限 {file_max_bytes} 字节（实际 {size_bytes} 字节）: {rel_path}"
            )));
        }
        return Ok(None);
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if fail_on_skip => {
            return Err(JiaClawError::ToolExecution(format!(
                "无法读取文件 {rel_path}: {err}"
            )));
        }
        Err(_) => return Ok(None),
    };
    match String::from_utf8(bytes) {
        Ok(text) if !text.contains('\0') => Ok(Some(text)),
        _ if fail_on_skip => Err(JiaClawError::ToolExecution(format!(
            "二进制文件，跳过搜索: {rel_path}（检测到 NUL 或非 UTF-8）"
        ))),
        _ => Ok(None),
    }
}

fn collect_line_matches(
    rel_path: &str,
    content: &str,
    pattern: &str,
    case_insensitive: bool,
    remaining: usize,
    out: &mut Vec<GrepMatch>,
) -> bool {
    if remaining == 0 {
        return true;
    }
    let mut added = 0;
    for (idx, line) in content.lines().enumerate() {
        if line_contains_literal(line, pattern, case_insensitive) {
            out.push(GrepMatch {
                path: rel_path.to_string(),
                line: idx + 1,
                snippet: truncate_snippet(line),
            });
            added += 1;
            if added >= remaining {
                return true;
            }
        }
    }
    false
}

fn grep_one_regular_file(
    path: &Path,
    rel_path: &str,
    args: &GrepArgs,
    file_max_bytes: usize,
    fail_on_skip: bool,
    files_scanned: &mut usize,
    out: &mut Vec<GrepMatch>,
) -> Result<bool, JiaClawError> {
    if *files_scanned >= GREP_MAX_FILES_SCANNED {
        return Ok(true);
    }
    *files_scanned += 1;
    if !glob_allows(args.glob.as_deref(), rel_path) {
        return Ok(false);
    }
    let Some(content) = read_text_file_for_grep(path, rel_path, file_max_bytes, fail_on_skip)?
    else {
        return Ok(false);
    };
    let remaining = args.max_matches.saturating_sub(out.len());
    Ok(collect_line_matches(
        rel_path,
        &content,
        &args.pattern,
        args.case_insensitive,
        remaining,
        out,
    ))
}

fn walk_grep_dir(
    dir: &Path,
    rel_prefix: &str,
    args: &GrepArgs,
    file_max_bytes: usize,
    files_scanned: &mut usize,
    out: &mut Vec<GrepMatch>,
) -> Result<bool, JiaClawError> {
    if out.len() >= args.max_matches || *files_scanned >= GREP_MAX_FILES_SCANNED {
        return Ok(true);
    }

    let mut children: Vec<(String, PathBuf, fs::Metadata)> = Vec::new();
    let iter = fs::read_dir(dir)
        .map_err(|e| JiaClawError::ToolExecution(format!("无法读取目录 {}: {e}", dir.display())))?;
    for entry in iter {
        let entry =
            entry.map_err(|e| JiaClawError::ToolExecution(format!("无法读取目录条目: {e}")))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "." || name == ".." || name == ".git" {
            continue;
        }
        let child_path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&child_path) else {
            continue;
        };
        children.push((name.into_owned(), child_path, meta));
    }
    children.sort_by(|a, b| a.0.cmp(&b.0));

    let mut truncated = false;
    for (name, child_path, meta) in children {
        if out.len() >= args.max_matches || *files_scanned >= GREP_MAX_FILES_SCANNED {
            truncated = true;
            break;
        }
        let rel_name = join_rel(rel_prefix, &name);
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if walk_grep_dir(
                &child_path,
                &rel_name,
                args,
                file_max_bytes,
                files_scanned,
                out,
            )? {
                truncated = true;
                break;
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if grep_one_regular_file(
            &child_path,
            &rel_name,
            args,
            file_max_bytes,
            false,
            files_scanned,
            out,
        )? {
            truncated = true;
            break;
        }
    }
    Ok(truncated || out.len() >= args.max_matches || *files_scanned >= GREP_MAX_FILES_SCANNED)
}

/// 在工作区内按**字面量**子串搜索文本（非正则，避免 `ReDoS`）。
///
/// `path` 可为相对目录或文件（默认 `.`）。目录默认递归；不跟随 symlink。
/// 二进制与超过 `file_max_bytes` 的文件在目录扫描时跳过；若 `path` 指向单个此类文件则报错。
///
/// # Errors
///
/// 路径非法、越出工作空间、symlink 逃逸、目标不存在，或无法读取搜索根时返回错误。
pub fn grep_workspace(
    workspace: &Path,
    args: &GrepArgs,
    file_max_bytes: usize,
) -> Result<GrepOutput, JiaClawError> {
    let path = resolve_workspace_relative_path(workspace, &args.path)?;
    if !path.exists() {
        return Err(JiaClawError::ToolExecution(format!(
            "路径不存在: {}",
            args.path
        )));
    }
    ensure_existing_within_workspace(workspace, &path)?;

    let canon = path
        .canonicalize()
        .map_err(|e| JiaClawError::ToolExecution(format!("无法解析路径 {}: {e}", args.path)))?;
    let ws = canonicalize_existing_or_clone(workspace);
    if !canon.starts_with(&ws) {
        return Err(JiaClawError::ToolExecution(format!(
            "安全错误: 路径 {} 指向工作空间外部",
            args.path
        )));
    }

    let mut matches = Vec::new();
    let mut files_scanned = 0;
    let truncated = if canon.is_file() {
        let rel = workspace_rel_display(&ws, &canon);
        grep_one_regular_file(
            &canon,
            &rel,
            args,
            file_max_bytes,
            true,
            &mut files_scanned,
            &mut matches,
        )?
    } else if canon.is_dir() {
        let rel = workspace_rel_display(&ws, &canon);
        walk_grep_dir(
            &canon,
            &rel,
            args,
            file_max_bytes,
            &mut files_scanned,
            &mut matches,
        )?
    } else {
        return Err(JiaClawError::ToolExecution(format!(
            "路径不是文件或目录: {}",
            args.path
        )));
    };

    Ok(GrepOutput {
        pattern: args.pattern.clone(),
        path: args.path.clone(),
        glob: args.glob.clone(),
        case_insensitive: args.case_insensitive,
        max_matches: args.max_matches,
        truncated,
        match_count: matches.len(),
        matches,
    })
}

fn glob_one_regular_file(rel_path: &str, pattern: &str, out: &mut Vec<String>) -> bool {
    if !glob_matches(pattern, rel_path) {
        return false;
    }
    if out.len() >= GLOB_MAX_RESULTS {
        return true;
    }
    out.push(rel_path.to_string());
    false
}

fn walk_glob_dir(
    dir: &Path,
    rel_prefix: &str,
    pattern: &str,
    files_scanned: &mut usize,
    out: &mut Vec<String>,
) -> Result<bool, JiaClawError> {
    if *files_scanned >= GLOB_MAX_FILES_SCANNED {
        return Ok(true);
    }

    let mut children: Vec<(String, PathBuf, fs::Metadata)> = Vec::new();
    let iter = fs::read_dir(dir)
        .map_err(|e| JiaClawError::ToolExecution(format!("无法读取目录 {}: {e}", dir.display())))?;
    for entry in iter {
        let entry =
            entry.map_err(|e| JiaClawError::ToolExecution(format!("无法读取目录条目: {e}")))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "." || name == ".." || name == ".git" {
            continue;
        }
        let child_path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&child_path) else {
            continue;
        };
        children.push((name.into_owned(), child_path, meta));
    }
    children.sort_by(|a, b| a.0.cmp(&b.0));

    let mut truncated = false;
    for (name, child_path, meta) in children {
        if *files_scanned >= GLOB_MAX_FILES_SCANNED {
            truncated = true;
            break;
        }
        let rel_name = join_rel(rel_prefix, &name);
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if walk_glob_dir(&child_path, &rel_name, pattern, files_scanned, out)? {
                truncated = true;
                break;
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        *files_scanned += 1;
        if glob_one_regular_file(&rel_name, pattern, out) {
            truncated = true;
            break;
        }
    }
    Ok(truncated)
}

/// 按 glob 模式列出工作区内匹配的**常规文件**路径（不含目录）。
///
/// `path` 可为相对目录或文件（默认 `.`）。目录默认递归；不跟随 symlink。
/// 结果按路径排序；超过 `max_results` 或扫描上限时截断并设置 `truncated`。
///
/// # Errors
///
/// 路径非法、越出工作空间、symlink 逃逸、目标不存在，或无法读取搜索根时返回错误。
pub fn glob_workspace(workspace: &Path, args: &GlobArgs) -> Result<GlobOutput, JiaClawError> {
    let path = resolve_workspace_relative_path(workspace, &args.path)?;
    if !path.exists() {
        return Err(JiaClawError::ToolExecution(format!(
            "路径不存在: {}",
            args.path
        )));
    }
    ensure_existing_within_workspace(workspace, &path)?;

    let canon = path
        .canonicalize()
        .map_err(|e| JiaClawError::ToolExecution(format!("无法解析路径 {}: {e}", args.path)))?;
    let ws = canonicalize_existing_or_clone(workspace);
    if !canon.starts_with(&ws) {
        return Err(JiaClawError::ToolExecution(format!(
            "安全错误: 路径 {} 指向工作空间外部",
            args.path
        )));
    }

    let mut matches = Vec::new();
    let mut files_scanned = 0;
    let scan_truncated = if canon.is_file() {
        let rel = workspace_rel_display(&ws, &canon);
        glob_one_regular_file(&rel, &args.pattern, &mut matches)
    } else if canon.is_dir() {
        let rel = workspace_rel_display(&ws, &canon);
        walk_glob_dir(
            &canon,
            &rel,
            &args.pattern,
            &mut files_scanned,
            &mut matches,
        )?
    } else {
        return Err(JiaClawError::ToolExecution(format!(
            "路径不是文件或目录: {}",
            args.path
        )));
    };

    matches.sort();
    let truncated = scan_truncated || matches.len() > args.max_results;
    if matches.len() > args.max_results {
        matches.truncate(args.max_results);
    }

    Ok(GlobOutput {
        pattern: args.pattern.clone(),
        path: args.path.clone(),
        max_results: args.max_results,
        truncated,
        match_count: matches.len(),
        matches,
    })
}

fn mkdir_existing_dir_result(
    workspace: &Path,
    dest: &Path,
    rel_path: &str,
    recursive: bool,
) -> Result<MkdirOutput, JiaClawError> {
    ensure_existing_within_workspace(workspace, dest)?;
    let canon = dest
        .canonicalize()
        .map_err(|e| JiaClawError::ToolExecution(format!("无法解析目录 {rel_path}: {e}")))?;
    let ws = canonicalize_existing_or_clone(workspace);
    if !canon.starts_with(&ws) {
        return Err(JiaClawError::ToolExecution(format!(
            "安全错误: 路径 {rel_path} 指向工作空间外部"
        )));
    }
    if !canon.is_dir() {
        return Err(JiaClawError::ToolExecution(format!(
            "路径已存在且不是目录: {rel_path}"
        )));
    }
    Ok(MkdirOutput {
        path: rel_path.to_string(),
        created: false,
        existed: true,
        recursive,
    })
}

/// 在工作区相对路径创建目录。默认 `recursive=true`（等价 `mkdir -p`）。
///
/// 目标已存在且为目录时幂等成功（`created=false`，`existed=true`）。
/// 已存在且为文件时报错。创建后 canonicalize 仍须落在工作区内。
///
/// # Errors
///
/// 路径非法、越出工作空间、symlink 逃逸、目标是文件、非递归时父目录不存在，或 IO 失败时返回错误。
pub fn mkdir_workspace(
    workspace: &Path,
    rel_path: &str,
    recursive: bool,
) -> Result<MkdirOutput, JiaClawError> {
    let (dest, existed) = prepare_workspace_create_path(workspace, rel_path)?;
    if existed {
        return mkdir_existing_dir_result(workspace, &dest, rel_path, recursive);
    }

    if recursive {
        fs::create_dir_all(&dest).map_err(|err| {
            if dest.exists() && !dest.is_dir() {
                JiaClawError::ToolExecution(format!("路径已存在且不是目录: {rel_path}"))
            } else {
                JiaClawError::ToolExecution(format!("无法创建目录 {rel_path}: {err}"))
            }
        })?;
    } else {
        let parent = dest
            .parent()
            .ok_or_else(|| JiaClawError::ToolExecution(format!("无效的目录路径: {rel_path}")))?;
        if !parent.exists() {
            return Err(JiaClawError::ToolExecution(format!(
                "父目录不存在（recursive/parents=false，需先创建上级或使用默认 mkdir -p）: {rel_path}"
            )));
        }
        if !parent.is_dir() {
            return Err(JiaClawError::ToolExecution(format!(
                "路径的父级不是目录: {rel_path}"
            )));
        }
        match fs::create_dir(&dest) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                return mkdir_existing_dir_result(workspace, &dest, rel_path, recursive);
            }
            Err(err) => {
                return Err(JiaClawError::ToolExecution(format!(
                    "无法创建目录 {rel_path}: {err}"
                )));
            }
        }
    }

    ensure_existing_within_workspace(workspace, &dest)?;
    let canon = dest
        .canonicalize()
        .map_err(|e| JiaClawError::ToolExecution(format!("无法解析目录 {rel_path}: {e}")))?;
    let ws = canonicalize_existing_or_clone(workspace);
    if !canon.starts_with(&ws) {
        return Err(JiaClawError::ToolExecution(format!(
            "安全错误: 路径 {rel_path} 指向工作空间外部"
        )));
    }
    if !canon.is_dir() {
        return Err(JiaClawError::ToolExecution(format!(
            "路径不是目录: {rel_path}"
        )));
    }

    Ok(MkdirOutput {
        path: rel_path.to_string(),
        created: true,
        existed: false,
        recursive,
    })
}

/// Unix `EXDEV`（跨设备）。
#[cfg(unix)]
const EXDEV_ERRNO: i32 = 18;
/// Windows `ERROR_NOT_SAME_DEVICE`。
#[cfg(windows)]
const ERROR_NOT_SAME_DEVICE: i32 = 17;

fn is_cross_device(err: &std::io::Error) -> bool {
    // `ErrorKind::CrossesDevices` 在当前 MSRV 上仍不稳定。
    match err.raw_os_error() {
        #[cfg(unix)]
        Some(EXDEV_ERRNO) => true,
        #[cfg(windows)]
        Some(ERROR_NOT_SAME_DEVICE) => true,
        _ => false,
    }
}

fn directory_is_empty(path: &Path) -> Result<bool, JiaClawError> {
    let mut entries = fs::read_dir(path).map_err(|err| {
        JiaClawError::ToolExecution(format!("无法读取目录 {}: {err}", path.display()))
    })?;
    match entries.next() {
        None => Ok(true),
        Some(Ok(_)) => Ok(false),
        Some(Err(err)) => Err(JiaClawError::ToolExecution(format!(
            "无法读取目录条目 {}: {err}",
            path.display()
        ))),
    }
}

fn resolve_move_source(
    workspace: &Path,
    from_rel: &str,
) -> Result<(PathBuf, MoveKind), JiaClawError> {
    let path = resolve_workspace_relative_path(workspace, from_rel)?;
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(JiaClawError::ToolExecution(format!(
                "源路径不存在: {from_rel}"
            )));
        }
        Err(err) => {
            return Err(JiaClawError::ToolExecution(format!(
                "无法读取源路径元数据 {from_rel}: {err}"
            )));
        }
    };

    let kind = if meta.file_type().is_dir() {
        MoveKind::Dir
    } else if meta.file_type().is_file() {
        MoveKind::File
    } else {
        return Err(JiaClawError::ToolExecution(format!(
            "源路径不是常规文件或目录: {from_rel}"
        )));
    };

    if !path.exists() {
        return Err(JiaClawError::ToolExecution(format!(
            "源路径不存在: {from_rel}"
        )));
    }
    ensure_existing_within_workspace(workspace, &path)?;
    let canon = path
        .canonicalize()
        .map_err(|e| JiaClawError::ToolExecution(format!("无法解析源路径 {from_rel}: {e}")))?;
    let ws = canonicalize_existing_or_clone(workspace);
    if !canon.starts_with(&ws) {
        return Err(JiaClawError::ToolExecution(format!(
            "安全错误: 源路径 {from_rel} 指向工作空间外部"
        )));
    }
    if canon == ws {
        return Err(JiaClawError::ToolExecution(
            "拒绝移动工作区根目录".to_string(),
        ));
    }
    match kind {
        MoveKind::Dir if !canon.is_dir() => {
            return Err(JiaClawError::ToolExecution(format!(
                "源路径不是目录: {from_rel}"
            )));
        }
        MoveKind::File if !canon.is_file() => {
            return Err(JiaClawError::ToolExecution(format!(
                "源路径不是文件: {from_rel}"
            )));
        }
        _ => {}
    }
    Ok((canon, kind))
}

fn resolve_move_destination(
    workspace: &Path,
    to_rel: &str,
    source_canon: &Path,
    kind: MoveKind,
    overwrite: bool,
) -> Result<(PathBuf, bool), JiaClawError> {
    let (dest, dest_existed) = prepare_workspace_create_path(workspace, to_rel)?;
    let ws = canonicalize_existing_or_clone(workspace);

    if dest_existed {
        ensure_existing_within_workspace(workspace, &dest)?;
        let dest_canon = dest
            .canonicalize()
            .map_err(|e| JiaClawError::ToolExecution(format!("无法解析目标路径 {to_rel}: {e}")))?;
        if !dest_canon.starts_with(&ws) {
            return Err(JiaClawError::ToolExecution(format!(
                "安全错误: 目标路径 {to_rel} 指向工作空间外部"
            )));
        }
        if dest_canon == source_canon {
            return Err(JiaClawError::ToolExecution(format!(
                "源与目标是同一路径: {to_rel}"
            )));
        }
        if !overwrite {
            return Err(JiaClawError::ToolExecution(format!(
                "目标已存在: {to_rel}（overwrite=false，拒绝覆盖）"
            )));
        }
        let dest_meta = fs::symlink_metadata(&dest_canon).map_err(|err| {
            JiaClawError::ToolExecution(format!("无法读取目标路径元数据 {to_rel}: {err}"))
        })?;
        match (
            kind,
            dest_meta.file_type().is_dir(),
            dest_meta.file_type().is_file(),
        ) {
            (MoveKind::File, false, true) => {
                fs::remove_file(&dest_canon).map_err(|err| {
                    JiaClawError::ToolExecution(format!("无法覆盖目标文件 {to_rel}: {err}"))
                })?;
            }
            (MoveKind::Dir, true, false) => {
                if !directory_is_empty(&dest_canon)? {
                    return Err(JiaClawError::ToolExecution(format!(
                        "拒绝覆盖非空目录: {to_rel}"
                    )));
                }
                fs::remove_dir(&dest_canon).map_err(|err| {
                    JiaClawError::ToolExecution(format!("无法覆盖目标目录 {to_rel}: {err}"))
                })?;
            }
            _ => {
                return Err(JiaClawError::ToolExecution(format!(
                    "源与目标类型不一致，拒绝覆盖: {to_rel}"
                )));
            }
        }
        return Ok((dest_canon, true));
    }

    let parent = dest
        .parent()
        .ok_or_else(|| JiaClawError::ToolExecution(format!("无效的目标路径: {to_rel}")))?;
    if !parent.exists() {
        return Err(JiaClawError::ToolExecution(format!(
            "目标父目录不存在: {to_rel}"
        )));
    }
    ensure_existing_within_workspace(workspace, parent)?;
    let parent_canon = parent
        .canonicalize()
        .map_err(|e| JiaClawError::ToolExecution(format!("无法解析目标父目录 {to_rel}: {e}")))?;
    if !parent_canon.starts_with(&ws) || !parent_canon.is_dir() {
        return Err(JiaClawError::ToolExecution(format!(
            "安全错误: 目标路径 {to_rel} 指向工作空间外部"
        )));
    }
    let file_name = dest
        .file_name()
        .ok_or_else(|| JiaClawError::ToolExecution(format!("无效的目标文件名: {to_rel}")))?;
    let dest_final = parent_canon.join(file_name);
    if !dest_final.starts_with(&ws) {
        return Err(JiaClawError::ToolExecution(format!(
            "安全错误: 目标路径 {to_rel} 指向工作空间外部"
        )));
    }
    Ok((dest_final, false))
}

fn copy_delete_across_device(
    source: &Path,
    dest: &Path,
    from_rel: &str,
    to_rel: &str,
    kind: MoveKind,
) -> Result<(), JiaClawError> {
    match kind {
        MoveKind::File => {
            fs::copy(source, dest).map_err(|err| {
                JiaClawError::ToolExecution(format!(
                    "跨文件系统复制 {from_rel} -> {to_rel} 失败: {err}"
                ))
            })?;
            fs::remove_file(source).map_err(|err| {
                JiaClawError::ToolExecution(format!(
                    "跨文件系统复制后无法删除源文件 {from_rel}: {err}"
                ))
            })?;
        }
        MoveKind::Dir => {
            if !directory_is_empty(source)? {
                return Err(JiaClawError::ToolExecution(format!(
                    "跨文件系统移动非空目录仅支持同卷 rename: {from_rel} -> {to_rel}"
                )));
            }
            fs::create_dir(dest).map_err(|err| {
                JiaClawError::ToolExecution(format!("跨文件系统创建目标目录 {to_rel} 失败: {err}"))
            })?;
            fs::remove_dir(source).map_err(|err| {
                JiaClawError::ToolExecution(format!(
                    "跨文件系统复制后无法删除源目录 {from_rel}: {err}"
                ))
            })?;
        }
    }
    Ok(())
}

fn rename_or_copy_delete(
    source: &Path,
    dest: &Path,
    from_rel: &str,
    to_rel: &str,
    kind: MoveKind,
) -> Result<(), JiaClawError> {
    match fs::rename(source, dest) {
        Ok(()) => Ok(()),
        Err(err) if is_cross_device(&err) => {
            copy_delete_across_device(source, dest, from_rel, to_rel, kind)
        }
        Err(err) => Err(JiaClawError::ToolExecution(format!(
            "无法移动 {from_rel} -> {to_rel}: {err}"
        ))),
    }
}

/// 在工作区内移动或重命名文件 / 目录。优先同卷 [`std::fs::rename`]；文件与空目录在跨文件系统时回退复制后删除。
///
/// `from` 必须存在。`to` 已存在且 `overwrite=false`（默认）时报错。不创建中间目录。
/// 非空目录仅同卷 rename；跨卷非空目录报错。
///
/// # Errors
///
/// 路径非法、越出工作空间、symlink 逃逸、源不存在、目标已存在且未覆盖、类型不匹配，或 IO 失败时返回错误。
pub fn move_workspace(
    workspace: &Path,
    from_rel: &str,
    to_rel: &str,
    overwrite: bool,
) -> Result<MoveOutput, JiaClawError> {
    let (source_canon, kind) = resolve_move_source(workspace, from_rel)?;
    let (dest_final, overwritten) =
        resolve_move_destination(workspace, to_rel, &source_canon, kind, overwrite)?;

    if dest_final == source_canon {
        return Err(JiaClawError::ToolExecution(format!(
            "源与目标是同一路径: {to_rel}"
        )));
    }
    if kind == MoveKind::Dir && dest_final.starts_with(&source_canon) {
        return Err(JiaClawError::ToolExecution(format!(
            "不能将目录移动到其自身或其子路径: {from_rel} -> {to_rel}"
        )));
    }

    rename_or_copy_delete(&source_canon, &dest_final, from_rel, to_rel, kind)?;

    if source_canon.exists() {
        return Err(JiaClawError::ToolExecution(format!(
            "移动后源路径仍存在: {from_rel}"
        )));
    }
    if !dest_final.exists() {
        return Err(JiaClawError::ToolExecution(format!(
            "移动后目标路径不存在: {to_rel}"
        )));
    }
    ensure_existing_within_workspace(workspace, &dest_final)?;
    let dest_canon = dest_final.canonicalize().map_err(|e| {
        JiaClawError::ToolExecution(format!("无法解析移动后的目标路径 {to_rel}: {e}"))
    })?;
    let ws = canonicalize_existing_or_clone(workspace);
    if !dest_canon.starts_with(&ws) {
        return Err(JiaClawError::ToolExecution(format!(
            "安全错误: 目标路径 {to_rel} 指向工作空间外部"
        )));
    }

    Ok(MoveOutput {
        from: from_rel.to_string(),
        to: to_rel.to_string(),
        overwrite,
        overwritten,
        kind,
    })
}

fn slice_lines(content: &str, offset: usize, limit: Option<usize>) -> (String, usize, usize, bool) {
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let start = offset.saturating_sub(1).min(total_lines);
    let end = match limit {
        Some(n) => start.saturating_add(n).min(total_lines),
        None => total_lines,
    };
    let returned = &lines[start..end];
    let truncated = start > 0 || end < total_lines;
    (returned.join("\n"), total_lines, returned.len(), truncated)
}

/// 读取工作区相对路径下的真实文本文件（按行切片）。
///
/// # Errors
///
/// 路径非法、越出工作空间、symlink 逃逸、不是文件、超过大小上限、或二进制时返回错误。
pub fn read_workspace_file(
    workspace: &Path,
    rel_path: &str,
    offset: usize,
    limit: Option<usize>,
    max_bytes: usize,
) -> Result<ReadFileOutput, JiaClawError> {
    let bytes = crate::memory_io::read_file_bytes(workspace, rel_path, max_bytes)?;
    let size_bytes = bytes.len() as u64;
    let content = match String::from_utf8(bytes) {
        Ok(text) if !text.contains('\0') => text,
        _ => {
            return Err(JiaClawError::ToolExecution(format!(
                "二进制文件，拒绝读取: {rel_path}（检测到 NUL 或非 UTF-8）"
            )))
        }
    };
    let (text, total_lines, returned_lines, truncated) = slice_lines(&content, offset, limit);
    Ok(ReadFileOutput {
        path: rel_path.to_string(),
        offset,
        limit,
        total_lines,
        returned_lines,
        truncated,
        size_bytes,
        content: text,
    })
}

/// 列出工作区相对目录（默认不递归，不跟随 symlink 目录）。
///
/// # Errors
///
/// 路径非法、越出工作空间、symlink 逃逸、或不是目录时返回错误。
pub fn list_workspace_dir(
    workspace: &Path,
    rel_path: &str,
    max_entries: usize,
    recursive: bool,
) -> Result<ListDirOutput, JiaClawError> {
    let (entries, truncated) =
        crate::memory_io::list_directory(workspace, rel_path, max_entries, recursive)?;
    Ok(ListDirOutput {
        path: rel_path.to_string(),
        recursive,
        truncated,
        entries,
    })
}

/// `read_file` 的 JSON 返回体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileOutput {
    /// 调用方传入的工作区相对路径
    pub path: String,
    /// 实际使用的起始行（从 1 起）
    pub offset: usize,
    /// 请求的最大行数（缺省为读到末尾）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// 文件总行数
    pub total_lines: usize,
    /// 本次返回的行数
    pub returned_lines: usize,
    /// 是否因 offset/limit 未返回全文
    pub truncated: bool,
    /// 文件字节数
    pub size_bytes: u64,
    /// 切片后的文本内容
    pub content: String,
}

/// 目录中的一条文件或子目录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntryInfo {
    /// 条目名称（递归时为相对所列目录的路径）
    pub name: String,
    /// `file`、`dir`、`symlink` 或 `unsupported`（不跟随链接）
    #[serde(rename = "type")]
    pub kind: String,
    /// 常规单链接文件大小（字节）；其他类型为 `None`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// `list_dir` 的 JSON 返回体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListDirOutput {
    /// 所列目录的工作区相对路径
    pub path: String,
    /// 是否递归
    pub recursive: bool,
    /// 是否因条目、扫描、深度、路径、时间或 JSON 字节预算截断
    pub truncated: bool,
    /// 目录条目
    pub entries: Vec<DirEntryInfo>,
}

/// `write_file` 的 JSON 返回体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteFileOutput {
    /// 调用方传入的工作区相对路径
    pub path: String,
    /// 实际使用的写入模式
    pub mode: WriteFileMode,
    /// 结果文件字节数
    pub bytes_written: usize,
}

/// `delete_file` 的 JSON 返回体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteFileOutput {
    /// 调用方传入的工作区相对路径
    pub path: String,
    /// 是否已删除（成功时为 `true`）
    pub deleted: bool,
    /// 删除前的文件字节数
    pub size_bytes: u64,
}

/// `str_replace` 的 JSON 返回体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrReplaceOutput {
    /// 调用方传入的工作区相对路径
    pub path: String,
    /// 实际替换次数
    pub replacements: usize,
    /// 是否按 `replace_all` 替换全部匹配
    pub replace_all: bool,
    /// 结果文件字节数
    pub bytes_written: usize,
}

/// `grep` 的一条匹配。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepMatch {
    /// 工作区相对路径
    pub path: String,
    /// 1-indexed 行号
    pub line: usize,
    /// 截断后的行文本
    pub snippet: String,
}

/// `grep` 的 JSON 返回体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepOutput {
    /// 字面量搜索串
    pub pattern: String,
    /// 调用方传入的工作区相对路径
    pub path: String,
    /// 可选 glob 过滤
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// 是否忽略大小写
    pub case_insensitive: bool,
    /// 生效的匹配上限
    pub max_matches: usize,
    /// 是否因 `max_matches` 或扫描文件数上限截断
    pub truncated: bool,
    /// 本次返回的匹配条数
    pub match_count: usize,
    /// 匹配列表
    pub matches: Vec<GrepMatch>,
}

/// `glob` 的 JSON 返回体。只含常规文件路径（不含目录）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobOutput {
    /// glob 模式
    pub pattern: String,
    /// 调用方传入的工作区相对搜索根
    pub path: String,
    /// 生效的结果上限
    pub max_results: usize,
    /// 是否因 `max_results` 或扫描文件数上限截断
    pub truncated: bool,
    /// 本次返回的路径条数
    pub match_count: usize,
    /// 工作区相对路径（已排序）
    pub matches: Vec<String>,
}

/// `mkdir` 的 JSON 返回体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MkdirOutput {
    /// 调用方传入的工作区相对路径
    pub path: String,
    /// 是否新创建了目录（已存在则为 `false`）
    pub created: bool,
    /// 调用前目标是否已作为目录存在
    pub existed: bool,
    /// 实际使用的 recursive/parents 值
    pub recursive: bool,
}

/// `move` 的 JSON 返回体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveOutput {
    /// 调用方传入的源路径（文档主名 `from`）
    pub from: String,
    /// 调用方传入的目标路径（文档主名 `to`）
    pub to: String,
    /// 实际使用的 overwrite 值
    pub overwrite: bool,
    /// 是否因 overwrite 删除了已存在的目标
    pub overwritten: bool,
    /// 移动的是文件还是目录
    pub kind: MoveKind,
}

/// `read_file` 工具：读取工作区相对路径下的文本文件（路径沙箱，不调用 LLM）。
pub struct WorkspaceReadFileTool {
    workspace_path: PathBuf,
    max_bytes: usize,
}

impl WorkspaceReadFileTool {
    /// 创建工具；读取上限为 [`READ_FILE_MAX_BYTES`]。
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            workspace_path: canonicalize_existing_or_clone(workspace_path),
            max_bytes: READ_FILE_MAX_BYTES,
        }
    }

    /// 测试或自定义上限。
    #[must_use]
    pub fn with_max_bytes(workspace_path: &Path, max_bytes: usize) -> Self {
        Self {
            workspace_path: canonicalize_existing_or_clone(workspace_path),
            max_bytes,
        }
    }
}

#[async_trait]
impl Tool for WorkspaceReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "读取工作区相对路径下的文本文件。path 相对于工作区根；禁止 .. / 绝对路径 / symlink 逃逸。可选 offset/limit 按行切片（1-indexed）。超过 256KiB 或二进制则报错。返回 {path, offset, limit, total_lines, returned_lines, truncated, size_bytes, content}。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "文件相对路径（相对于工作区根目录）"
                },
                "offset": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "起始行号（从 1 起，默认 1）",
                    "default": 1
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "最多返回的行数；缺省读到文件末尾（仍受 256KiB 上限约束）"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let parsed = parse_read_file_args(&args)?;
        let workspace = self.workspace_path.clone();
        let maximum = self.max_bytes;
        let output = crate::memory_io::run_blocking(move || {
            read_workspace_file(
                &workspace,
                &parsed.path,
                parsed.offset,
                parsed.limit,
                maximum,
            )
        })
        .await?;
        serde_json::to_string_pretty(&output)
            .map_err(|e| JiaClawError::ToolExecution(format!("序列化读取结果失败: {e}")))
    }
}

/// `list_dir` 工具：列出工作区相对目录（默认不递归，不调用 LLM）。
pub struct WorkspaceListDirTool {
    workspace_path: PathBuf,
}

impl WorkspaceListDirTool {
    /// 创建工具。
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            workspace_path: canonicalize_existing_or_clone(workspace_path),
        }
    }
}

#[async_trait]
impl Tool for WorkspaceListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "列出工作区相对目录中的条目。path 默认 . ；禁止 .. / 绝对路径 / symlink 逃逸。默认不递归（recursive=false）。可选 max_entries（默认 200，钳制 1..=1000）；另有扫描/深度/时间预算和 64KiB JSON 上限，超限 truncated=true。链接和特殊文件只报告类型，不跟随。返回 {path, recursive, truncated, entries:[{name, type, size?}]}。不执行 shell。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "目录相对路径（相对于工作区根目录，默认 .）",
                    "default": "."
                },
                "max_entries": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "最多返回的条目数（默认 200，钳制 1..=1000）",
                    "default": 200
                },
                "recursive": {
                    "type": "boolean",
                    "description": "是否递归列出子目录（默认 false，且不跟随 symlink 目录）",
                    "default": false
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let parsed = parse_list_dir_args(&args)?;
        let workspace = self.workspace_path.clone();
        let output = crate::memory_io::run_blocking(move || {
            list_workspace_dir(
                &workspace,
                &parsed.path,
                parsed.max_entries,
                parsed.recursive,
            )
        })
        .await?;
        serde_json::to_string_pretty(&output)
            .map_err(|e| JiaClawError::ToolExecution(format!("序列化目录列表失败: {e}")))
    }
}

/// `write_file` 工具：写入工作区相对路径下的常规文件（路径沙箱，不调用 LLM）。
pub struct WorkspaceWriteFileTool {
    workspace_path: PathBuf,
    max_bytes: usize,
}

impl WorkspaceWriteFileTool {
    /// 创建工具；写入上限为 [`WRITE_FILE_MAX_BYTES`]（与 read 对齐，256KiB）。
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            workspace_path: canonicalize_existing_or_clone(workspace_path),
            max_bytes: WRITE_FILE_MAX_BYTES,
        }
    }

    /// 测试或自定义上限。
    #[must_use]
    pub fn with_max_bytes(workspace_path: &Path, max_bytes: usize) -> Self {
        Self {
            workspace_path: canonicalize_existing_or_clone(workspace_path),
            max_bytes,
        }
    }
}

#[async_trait]
impl Tool for WorkspaceWriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "写入工作区相对路径下的常规文件。path / content 必填；可选 mode=overwrite|append（默认 overwrite）。禁止 .. / 绝对路径 / symlink 逃逸。可创建中间目录。结果文件超过 256KiB 则报错且不落盘。tmp + rename 原子写。返回 {path, mode, bytes_written}。不执行 shell，不调用 LLM。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "文件相对路径（相对于工作区根目录）"
                },
                "content": {
                    "type": "string",
                    "description": "要写入的文件内容"
                },
                "mode": {
                    "type": "string",
                    "enum": ["overwrite", "append"],
                    "description": "overwrite（默认）覆盖整个文件；append 追加",
                    "default": "overwrite"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let parsed = parse_write_file_args(&args)?;
        let workspace = self.workspace_path.clone();
        let maximum = self.max_bytes;
        let output = crate::memory_io::run_blocking(move || {
            write_workspace_regular_file(
                &workspace,
                &parsed.path,
                &parsed.content,
                parsed.mode,
                maximum,
            )
        })
        .await?;
        serde_json::to_string_pretty(&output)
            .map_err(|e| JiaClawError::ToolExecution(format!("序列化写入结果失败: {e}")))
    }
}

/// `delete_file` 工具：删除工作区相对路径下的常规文件（路径沙箱，不调用 LLM）。
pub struct WorkspaceDeleteFileTool {
    workspace_path: PathBuf,
}

impl WorkspaceDeleteFileTool {
    /// 创建工具。
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            workspace_path: canonicalize_existing_or_clone(workspace_path),
        }
    }
}

#[async_trait]
impl Tool for WorkspaceDeleteFileTool {
    fn name(&self) -> &str {
        "delete_file"
    }

    fn description(&self) -> &str {
        "删除工作区相对路径下的常规文件。path 必填且相对于工作区根。禁止 .. / 绝对路径 / symlink 逃逸。只删常规文件，拒绝目录；文件不存在时明确报错（不静默成功）。不递归、不执行 shell、不调用 LLM。返回 {path, deleted, size_bytes}。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "要删除的文件相对路径（相对于工作区根目录）"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let parsed = parse_delete_file_args(&args)?;
        let workspace = self.workspace_path.clone();
        let output = crate::memory_io::run_blocking(move || {
            delete_workspace_regular_file(&workspace, &parsed.path)
        })
        .await?;
        serde_json::to_string_pretty(&output)
            .map_err(|e| JiaClawError::ToolExecution(format!("序列化删除结果失败: {e}")))
    }
}

/// `str_replace` 工具：在工作区相对路径的常规文本文件内做精确字符串替换（路径沙箱，不调用 LLM）。
pub struct WorkspaceStrReplaceTool {
    workspace_path: PathBuf,
    max_bytes: usize,
}

impl WorkspaceStrReplaceTool {
    /// 创建工具；读入/写出上限为 [`STR_REPLACE_MAX_BYTES`]（与 read/write 对齐，256KiB）。
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            workspace_path: canonicalize_existing_or_clone(workspace_path),
            max_bytes: STR_REPLACE_MAX_BYTES,
        }
    }

    /// 测试或自定义上限。
    #[must_use]
    pub fn with_max_bytes(workspace_path: &Path, max_bytes: usize) -> Self {
        Self {
            workspace_path: canonicalize_existing_or_clone(workspace_path),
            max_bytes,
        }
    }
}

#[async_trait]
impl Tool for WorkspaceStrReplaceTool {
    fn name(&self) -> &str {
        "str_replace"
    }

    fn description(&self) -> &str {
        "在工作区相对路径的常规文本文件内做精确字符串替换。path / old_str / new_str 必填；可选 replace_all（默认 false：必须恰好匹配 1 次，否则报错）。禁止 .. / 绝对路径 / symlink 逃逸。只操作常规文本文件；拒绝二进制。读入或写出超过 256KiB 则报错且不落盘。tmp + rename 原子写。返回 {path, replacements, replace_all, bytes_written}。不执行 shell，不调用 LLM。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "文件相对路径（相对于工作区根目录）"
                },
                "old_str": {
                    "type": "string",
                    "description": "要查找的精确子串（必填，非空）"
                },
                "new_str": {
                    "type": "string",
                    "description": "替换后的文本（必填，可为空表示删除匹配）"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "是否替换全部非重叠匹配（默认 false：必须恰好 1 次）",
                    "default": false
                }
            },
            "required": ["path", "old_str", "new_str"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let parsed = parse_str_replace_args(&args)?;
        let workspace = self.workspace_path.clone();
        let maximum = self.max_bytes;
        let output = crate::memory_io::run_blocking(move || {
            str_replace_workspace_file(
                &workspace,
                &parsed.path,
                &parsed.old_str,
                &parsed.new_str,
                parsed.replace_all,
                maximum,
            )
        })
        .await?;
        serde_json::to_string_pretty(&output)
            .map_err(|e| JiaClawError::ToolExecution(format!("序列化替换结果失败: {e}")))
    }
}

/// `grep` 工具：在工作区内按字面量搜索文本（路径沙箱，不调用 LLM）。
pub struct WorkspaceGrepTool {
    workspace_path: PathBuf,
    file_max_bytes: usize,
}

impl WorkspaceGrepTool {
    /// 创建工具；单文件上限为 [`GREP_FILE_MAX_BYTES`]（与 read 对齐，256KiB）。
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            workspace_path: canonicalize_existing_or_clone(workspace_path),
            file_max_bytes: GREP_FILE_MAX_BYTES,
        }
    }

    /// 测试或自定义单文件上限。
    #[must_use]
    pub fn with_max_bytes(workspace_path: &Path, file_max_bytes: usize) -> Self {
        Self {
            workspace_path: canonicalize_existing_or_clone(workspace_path),
            file_max_bytes,
        }
    }
}

#[async_trait]
impl Tool for WorkspaceGrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "在工作区内按字面量搜索文本（非正则，避免 ReDoS）。pattern 必填；可选 path（相对目录或文件，默认 .）、glob（如 *.rs）、case_insensitive（默认 false）、max_matches（默认 50，钳制 1..=200）。禁止 .. / 绝对路径 / symlink 逃逸。不跟随 symlink；跳过二进制与超过 256KiB 的文件。返回 {path, line, snippet} 列表。不执行 shell，不调用 LLM。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "要查找的字面量子串（必填，非正则）"
                },
                "path": {
                    "type": "string",
                    "description": "相对目录或文件（相对于工作区根，默认 .）",
                    "default": "."
                },
                "glob": {
                    "type": "string",
                    "description": "可选文件名/路径 glob（如 *.rs；不含 / 时匹配文件名）"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "是否忽略大小写（默认 false）",
                    "default": false
                },
                "max_matches": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "最多返回的匹配条数（默认 50，钳制 1..=200）",
                    "default": 50
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let parsed = parse_grep_args(&args)?;
        let output = grep_workspace(&self.workspace_path, &parsed, self.file_max_bytes)?;
        serde_json::to_string_pretty(&output)
            .map_err(|e| JiaClawError::ToolExecution(format!("序列化搜索结果失败: {e}")))
    }
}

/// `glob` 工具：按 glob 模式列出工作区内匹配的常规文件（路径沙箱，不调用 LLM）。
pub struct WorkspaceGlobTool {
    workspace_path: PathBuf,
}

impl WorkspaceGlobTool {
    /// 创建工具。
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            workspace_path: canonicalize_existing_or_clone(workspace_path),
        }
    }
}

#[async_trait]
impl Tool for WorkspaceGlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "按 glob 模式列出工作区内匹配的常规文件路径（不含目录）。pattern 必填（如 **/*.rs、src/**/*.toml）；可选 path（相对搜索根，默认 .）、max_results（默认 100，钳制 1..=500）。结果按路径排序；超限截断并注明 truncated。禁止 .. / 绝对路径 / symlink 逃逸。不跟随 symlink。不执行 shell，不调用 LLM。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "glob 模式（必填，如 **/*.rs；不含 / 时匹配文件名）"
                },
                "path": {
                    "type": "string",
                    "description": "相对搜索根（相对于工作区根，默认 .）",
                    "default": "."
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "最多返回的文件路径数（默认 100，钳制 1..=500）",
                    "default": 100
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let parsed = parse_glob_args(&args)?;
        let output = glob_workspace(&self.workspace_path, &parsed)?;
        serde_json::to_string_pretty(&output)
            .map_err(|e| JiaClawError::ToolExecution(format!("序列化 glob 结果失败: {e}")))
    }
}

/// `mkdir` 工具：创建工作区相对路径下的目录（路径沙箱，不调用 LLM）。
pub struct WorkspaceMkdirTool {
    workspace_path: PathBuf,
}

impl WorkspaceMkdirTool {
    /// 创建工具。
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            workspace_path: canonicalize_existing_or_clone(workspace_path),
        }
    }
}

#[async_trait]
impl Tool for WorkspaceMkdirTool {
    fn name(&self) -> &str {
        "mkdir"
    }

    fn description(&self) -> &str {
        "创建工作区相对路径下的目录。path 必填；可选 recursive / parents（默认 true，等价 mkdir -p）。目录已存在则幂等成功（created=false, existed=true）。若路径已存在且为文件则报错。禁止 .. / 绝对路径 / symlink 逃逸。创建后 canonicalize 必须仍落在工作区。不执行 shell，不调用 LLM。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "要创建的目录相对路径（相对于工作区根目录）"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "是否创建中间目录（默认 true，等价 mkdir -p；与 parents 为别名）",
                    "default": true
                },
                "parents": {
                    "type": "boolean",
                    "description": "recursive 的别名（默认 true，等价 mkdir -p）",
                    "default": true
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let parsed = parse_mkdir_args(&args)?;
        let output = mkdir_workspace(&self.workspace_path, &parsed.path, parsed.recursive)?;
        serde_json::to_string_pretty(&output)
            .map_err(|e| JiaClawError::ToolExecution(format!("序列化 mkdir 结果失败: {e}")))
    }
}

/// `move` 工具：在工作区内移动或重命名文件 / 目录（路径沙箱，不调用 LLM）。
pub struct WorkspaceMoveTool {
    workspace_path: PathBuf,
}

impl WorkspaceMoveTool {
    /// 创建工具。
    #[must_use]
    pub fn new(workspace_path: &Path) -> Self {
        Self {
            workspace_path: canonicalize_existing_or_clone(workspace_path),
        }
    }
}

#[async_trait]
impl Tool for WorkspaceMoveTool {
    fn name(&self) -> &str {
        "move"
    }

    fn description(&self) -> &str {
        "在工作区内移动或重命名文件或目录。文档主名 from / to（source / destination 为别名，同时给出时必须一致）。from 必须存在；to 已存在且 overwrite=false（默认）则报错。支持常规文件、空目录与非空目录（非空目录优先同卷 rename；跨文件系统的非空目录报错，文件与空目录回退复制后删除）。禁止 .. / 绝对路径 / symlink 逃逸。canonicalize 后两端必须仍落在工作区。不创建中间目录。不执行 shell，不调用 LLM。"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "from": {
                    "type": "string",
                    "description": "源路径（工作区相对；与 source 为别名，文档主名 from）"
                },
                "source": {
                    "type": "string",
                    "description": "from 的别名（工作区相对源路径）"
                },
                "to": {
                    "type": "string",
                    "description": "目标路径（工作区相对；与 destination 为别名，文档主名 to）"
                },
                "destination": {
                    "type": "string",
                    "description": "to 的别名（工作区相对目标路径）"
                },
                "overwrite": {
                    "type": "boolean",
                    "description": "目标已存在时是否覆盖（默认 false，拒绝覆盖）",
                    "default": false
                }
            },
            "required": ["from", "to"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String, JiaClawError> {
        let parsed = parse_move_args(&args)?;
        let output = move_workspace(
            &self.workspace_path,
            &parsed.from,
            &parsed.to,
            parsed.overwrite,
        )?;
        serde_json::to_string_pretty(&output)
            .map_err(|e| JiaClawError::ToolExecution(format!("序列化 move 结果失败: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}_{}_{nanos}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parse_read_file_args_requires_path_and_defaults_offset() {
        let err = parse_read_file_args(&serde_json::json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("path"), "{err}");

        let err = parse_read_file_args(&serde_json::json!({"path": "   "}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("path"), "{err}");

        let parsed = parse_read_file_args(&serde_json::json!({"path": " notes/a.md "})).unwrap();
        assert_eq!(parsed.path, "notes/a.md");
        assert_eq!(parsed.offset, 1);
        assert!(parsed.limit.is_none());

        let parsed = parse_read_file_args(&serde_json::json!({
            "path": "a.md",
            "offset": 3,
            "limit": 2
        }))
        .unwrap();
        assert_eq!(parsed.offset, 3);
        assert_eq!(parsed.limit, Some(2));
    }

    #[test]
    fn parse_list_dir_args_defaults_and_clamps() {
        let parsed = parse_list_dir_args(&serde_json::json!({})).unwrap();
        assert_eq!(parsed.path, ".");
        assert_eq!(parsed.max_entries, LIST_DIR_DEFAULT_MAX_ENTRIES);
        assert!(!parsed.recursive);

        let parsed = parse_list_dir_args(&serde_json::json!({
            "path": " notes ",
            "max_entries": 0,
            "recursive": true
        }))
        .unwrap();
        assert_eq!(parsed.path, "notes");
        assert_eq!(parsed.max_entries, 1);
        assert!(parsed.recursive);

        let parsed = parse_list_dir_args(&serde_json::json!({"max_entries": 9999})).unwrap();
        assert_eq!(parsed.max_entries, LIST_DIR_MAX_ENTRIES);
    }

    #[test]
    fn clamp_list_dir_max_entries_bounds() {
        assert_eq!(clamp_list_dir_max_entries(0), 1);
        assert_eq!(clamp_list_dir_max_entries(200), 200);
        assert_eq!(clamp_list_dir_max_entries(1000), 1000);
        assert_eq!(clamp_list_dir_max_entries(1001), 1000);
    }

    #[tokio::test]
    async fn read_file_success_and_line_slice() {
        let ws = unique_temp("jiaclaw_read_file_ok");
        fs::write(ws.join("notes.md"), "alpha\nbeta\ngamma\n").unwrap();
        let tool = WorkspaceReadFileTool::new(&ws);
        assert_eq!(tool.name(), "read_file");

        let result = tool
            .execute(serde_json::json!({"path": "notes.md"}))
            .await
            .unwrap();
        assert!(result.contains("alpha"), "{result}");
        assert!(result.contains("gamma"), "{result}");
        assert!(result.contains("\"total_lines\": 3"), "{result}");

        let sliced = tool
            .execute(serde_json::json!({
                "path": "notes.md",
                "offset": 2,
                "limit": 1
            }))
            .await
            .unwrap();
        assert!(sliced.contains("beta"), "{sliced}");
        assert!(!sliced.contains("alpha"), "{sliced}");
        assert!(sliced.contains("\"returned_lines\": 1"), "{sliced}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn read_file_rejects_traversal_and_absolute() {
        let ws = unique_temp("jiaclaw_read_file_trav");
        fs::write(ws.join("ok.md"), "inside").unwrap();
        let tool = WorkspaceReadFileTool::new(&ws);

        let err = tool
            .execute(serde_json::json!({"path": "../secret.md"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("穿越") || err.contains("安全"), "{err}");

        let err = tool
            .execute(serde_json::json!({"path": "/etc/passwd"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("绝对路径"), "{err}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_file_rejects_symlink_escape() {
        let ws = unique_temp("jiaclaw_read_file_symlink");
        let outside = ws.parent().unwrap().join(format!(
            "jiaclaw_read_outside_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "secret-outside").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("leak.md")).unwrap();

        let tool = WorkspaceReadFileTool::new(&ws);
        let result = tool.execute(serde_json::json!({"path": "leak.md"})).await;
        assert!(result.is_err(), "symlink 逃逸应被拒绝: {result:?}");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("安全") || err.contains("工作空间"), "{err}");

        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn read_file_rejects_oversize_and_binary() {
        let ws = unique_temp("jiaclaw_read_file_limits");
        let tool = WorkspaceReadFileTool::with_max_bytes(&ws, 16);
        fs::write(ws.join("big.md"), "abcdefghijklmnopqrstuvwxyz").unwrap();
        let err = tool
            .execute(serde_json::json!({"path": "big.md"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("超过上限"), "{err}");
        assert!(err.contains("16"), "{err}");

        fs::write(ws.join("bin.dat"), [0_u8, 1, 2, 3, 255]).unwrap();
        let err = WorkspaceReadFileTool::new(&ws)
            .execute(serde_json::json!({"path": "bin.dat"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("二进制"), "{err}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn list_dir_basic_and_nested() {
        let ws = unique_temp("jiaclaw_list_dir_ok");
        fs::write(ws.join("MEMORY.md"), "mem").unwrap();
        fs::create_dir_all(ws.join("notes")).unwrap();
        fs::write(ws.join("notes").join("a.md"), "a").unwrap();
        let tool = WorkspaceListDirTool::new(&ws);
        assert_eq!(tool.name(), "list_dir");

        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(result.contains("MEMORY.md"), "{result}");
        assert!(result.contains("notes"), "{result}");
        assert!(result.contains("\"type\": \"file\""), "{result}");
        assert!(result.contains("\"type\": \"dir\""), "{result}");
        assert!(
            !result.contains("a.md"),
            "默认不递归不应列出子目录文件: {result}"
        );

        let nested = tool
            .execute(serde_json::json!({"path": "notes"}))
            .await
            .unwrap();
        assert!(nested.contains("a.md"), "{nested}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn list_dir_rejects_traversal() {
        let ws = unique_temp("jiaclaw_list_dir_trav");
        fs::create_dir_all(&ws).unwrap();
        let tool = WorkspaceListDirTool::new(&ws);
        let err = tool
            .execute(serde_json::json!({"path": "../"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("穿越") || err.contains("安全"), "{err}");

        let err = tool
            .execute(serde_json::json!({"path": "/tmp"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("绝对路径"), "{err}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn list_dir_rejects_symlink_escape() {
        let ws = unique_temp("jiaclaw_list_dir_symlink");
        let outside = ws.parent().unwrap().join(format!(
            "jiaclaw_list_outside_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "nope").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("escape")).unwrap();

        let tool = WorkspaceListDirTool::new(&ws);
        let result = tool.execute(serde_json::json!({"path": "escape"})).await;
        assert!(result.is_err(), "symlink 逃逸应被拒绝: {result:?}");
        let _ = fs::remove_dir_all(&outside);
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn list_dir_truncates_at_max_entries() {
        let ws = unique_temp("jiaclaw_list_dir_max");
        for i in 0..5 {
            fs::write(ws.join(format!("f{i}.txt")), "x").unwrap();
        }
        let tool = WorkspaceListDirTool::new(&ws);
        let result = tool
            .execute(serde_json::json!({"max_entries": 2}))
            .await
            .unwrap();
        assert!(result.contains("\"truncated\": true"), "{result}");
        let parsed: ListDirOutput = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.entries.len(), 2);
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn parse_write_file_args_requires_path_content_and_defaults_mode() {
        let err = parse_write_file_args(&serde_json::json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("path"), "{err}");

        let err = parse_write_file_args(&serde_json::json!({"path": "a.md"}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("content"), "{err}");

        let parsed = parse_write_file_args(&serde_json::json!({
            "path": " notes/a.md ",
            "content": "hello"
        }))
        .unwrap();
        assert_eq!(parsed.path, "notes/a.md");
        assert_eq!(parsed.content, "hello");
        assert_eq!(parsed.mode, WriteFileMode::Overwrite);

        let parsed = parse_write_file_args(&serde_json::json!({
            "path": "a.md",
            "content": "more",
            "mode": "APPEND"
        }))
        .unwrap();
        assert_eq!(parsed.mode, WriteFileMode::Append);

        let err = parse_write_file_args(&serde_json::json!({
            "path": "a.md",
            "content": "x",
            "mode": "delete"
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("mode"), "{err}");
    }

    #[tokio::test]
    async fn write_file_overwrite_and_append() {
        let ws = unique_temp("jiaclaw_write_file_ok");
        let tool = WorkspaceWriteFileTool::new(&ws);
        assert_eq!(tool.name(), "write_file");

        let created = tool
            .execute(serde_json::json!({
                "path": "notes/hello.md",
                "content": "alpha"
            }))
            .await
            .unwrap();
        assert!(created.contains("overwrite"), "{created}");
        assert!(created.contains("notes/hello.md"), "{created}");
        assert_eq!(
            fs::read_to_string(ws.join("notes").join("hello.md")).unwrap(),
            "alpha"
        );

        let overwritten = tool
            .execute(serde_json::json!({
                "path": "notes/hello.md",
                "content": "beta",
                "mode": "overwrite"
            }))
            .await
            .unwrap();
        assert!(overwritten.contains("overwrite"), "{overwritten}");
        assert_eq!(
            fs::read_to_string(ws.join("notes").join("hello.md")).unwrap(),
            "beta"
        );

        let appended = tool
            .execute(serde_json::json!({
                "path": "notes/hello.md",
                "content": "gamma",
                "mode": "append"
            }))
            .await
            .unwrap();
        assert!(appended.contains("append"), "{appended}");
        assert_eq!(
            fs::read_to_string(ws.join("notes").join("hello.md")).unwrap(),
            "betagamma"
        );
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn write_file_rejects_traversal_and_absolute() {
        let ws = unique_temp("jiaclaw_write_file_trav");
        let tool = WorkspaceWriteFileTool::new(&ws);

        let err = tool
            .execute(serde_json::json!({
                "path": "../secret.md",
                "content": "nope"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("穿越") || err.contains("安全"), "{err}");
        assert!(!ws.parent().unwrap().join("secret.md").exists());

        let err = tool
            .execute(serde_json::json!({
                "path": "/etc/passwd",
                "content": "nope"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("绝对路径"), "{err}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_file_rejects_symlink_escape() {
        let ws = unique_temp("jiaclaw_write_file_symlink");
        let outside = ws.parent().unwrap().join(format!(
            "jiaclaw_write_outside_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "secret-outside").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("leak.md")).unwrap();

        let tool = WorkspaceWriteFileTool::new(&ws);
        let result = tool
            .execute(serde_json::json!({
                "path": "leak.md",
                "content": "pwned"
            }))
            .await;
        assert!(result.is_err(), "symlink 逃逸应被拒绝: {result:?}");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("安全") || err.contains("工作空间"), "{err}");
        assert_eq!(fs::read_to_string(&outside).unwrap(), "secret-outside");

        let outside_dir = ws.parent().unwrap().join(format!(
            "jiaclaw_write_outside_dir_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::create_dir_all(&outside_dir).unwrap();
        std::os::unix::fs::symlink(&outside_dir, ws.join("escape")).unwrap();
        let result = tool
            .execute(serde_json::json!({
                "path": "escape/pwned.md",
                "content": "nope"
            }))
            .await;
        assert!(result.is_err(), "symlink 目录逃逸应被拒绝: {result:?}");
        assert!(!outside_dir.join("pwned.md").exists());

        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&outside_dir);
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn write_file_rejects_oversize_without_writing() {
        let ws = unique_temp("jiaclaw_write_file_limits");
        let tool = WorkspaceWriteFileTool::with_max_bytes(&ws, 8);
        fs::write(ws.join("keep.md"), "old").unwrap();

        let err = tool
            .execute(serde_json::json!({
                "path": "keep.md",
                "content": "abcdefghijk"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("超过上限"), "{err}");
        assert_eq!(fs::read_to_string(ws.join("keep.md")).unwrap(), "old");

        fs::write(ws.join("log.md"), "12345").unwrap();
        let err = tool
            .execute(serde_json::json!({
                "path": "log.md",
                "content": "67890",
                "mode": "append"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("超过上限"), "{err}");
        assert_eq!(fs::read_to_string(ws.join("log.md")).unwrap(), "12345");
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn parse_delete_file_args_requires_path() {
        let err = parse_delete_file_args(&serde_json::json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("path"), "{err}");

        let err = parse_delete_file_args(&serde_json::json!({"path": "   "}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("path"), "{err}");

        let parsed = parse_delete_file_args(&serde_json::json!({"path": " notes/a.md "})).unwrap();
        assert_eq!(parsed.path, "notes/a.md");
    }

    #[tokio::test]
    async fn delete_file_success_removes_regular_file() {
        let ws = unique_temp("jiaclaw_delete_file_ok");
        fs::write(ws.join("notes.md"), "delete-me").unwrap();
        let tool = WorkspaceDeleteFileTool::new(&ws);
        assert_eq!(tool.name(), "delete_file");

        let result = tool
            .execute(serde_json::json!({"path": "notes.md"}))
            .await
            .unwrap();
        assert!(result.contains("notes.md"), "{result}");
        assert!(result.contains("\"deleted\": true"), "{result}");
        assert!(!ws.join("notes.md").exists());
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn delete_file_rejects_traversal_and_absolute() {
        let ws = unique_temp("jiaclaw_delete_file_trav");
        let outside = ws.parent().unwrap().join(format!(
            "jiaclaw_delete_secret_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "keep-me").unwrap();
        let tool = WorkspaceDeleteFileTool::new(&ws);

        let err = tool
            .execute(serde_json::json!({"path": "../jiaclaw_should_not_delete.md"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("穿越") || err.contains("安全"), "{err}");

        let err = tool
            .execute(serde_json::json!({"path": "/etc/passwd"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("绝对路径"), "{err}");
        assert_eq!(fs::read_to_string(&outside).unwrap(), "keep-me");
        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delete_file_rejects_symlink_escape() {
        let ws = unique_temp("jiaclaw_delete_file_symlink");
        let outside = ws.parent().unwrap().join(format!(
            "jiaclaw_delete_outside_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "secret-outside").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("leak.md")).unwrap();

        let tool = WorkspaceDeleteFileTool::new(&ws);
        let result = tool.execute(serde_json::json!({"path": "leak.md"})).await;
        assert!(result.is_err(), "symlink 逃逸应被拒绝: {result:?}");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("安全") || err.contains("工作空间"), "{err}");
        assert_eq!(fs::read_to_string(&outside).unwrap(), "secret-outside");
        assert!(outside.exists(), "不得删除工作区外目标文件");

        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn delete_file_rejects_directory() {
        let ws = unique_temp("jiaclaw_delete_file_dir");
        fs::create_dir_all(ws.join("notes")).unwrap();
        fs::write(ws.join("notes").join("keep.md"), "stay").unwrap();
        let tool = WorkspaceDeleteFileTool::new(&ws);

        let err = tool
            .execute(serde_json::json!({"path": "notes"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("目录") || err.contains("不是文件"), "{err}");
        assert!(ws.join("notes").is_dir());
        assert_eq!(
            fs::read_to_string(ws.join("notes").join("keep.md")).unwrap(),
            "stay"
        );
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn delete_file_missing_file_is_explicit_error() {
        let ws = unique_temp("jiaclaw_delete_file_missing");
        let tool = WorkspaceDeleteFileTool::new(&ws);
        let result = tool
            .execute(serde_json::json!({"path": "no-such.md"}))
            .await;
        assert!(result.is_err(), "缺文件不得静默成功: {result:?}");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("不存在"), "{err}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn parse_str_replace_args_requires_path_old_and_new() {
        let err = parse_str_replace_args(&serde_json::json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("path"), "{err}");

        let err = parse_str_replace_args(&serde_json::json!({"path": "a.md"}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("old_str"), "{err}");

        let err = parse_str_replace_args(&serde_json::json!({
            "path": "a.md",
            "old_str": "foo"
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("new_str"), "{err}");

        let err = parse_str_replace_args(&serde_json::json!({
            "path": "a.md",
            "old_str": "",
            "new_str": "bar"
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("old_str"), "{err}");

        let parsed = parse_str_replace_args(&serde_json::json!({
            "path": " notes/a.md ",
            "old_str": "foo",
            "new_str": "bar"
        }))
        .unwrap();
        assert_eq!(parsed.path, "notes/a.md");
        assert_eq!(parsed.old_str, "foo");
        assert_eq!(parsed.new_str, "bar");
        assert!(!parsed.replace_all);

        let parsed = parse_str_replace_args(&serde_json::json!({
            "path": "a.md",
            "old_str": "foo",
            "new_str": "",
            "replace_all": true
        }))
        .unwrap();
        assert_eq!(parsed.new_str, "");
        assert!(parsed.replace_all);

        let err = parse_str_replace_args(&serde_json::json!({
            "path": "a.md",
            "old_str": "foo",
            "new_str": "bar",
            "replace_all": "yes"
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("replace_all"), "{err}");
    }

    #[tokio::test]
    async fn str_replace_single_occurrence() {
        let ws = unique_temp("jiaclaw_str_replace_once");
        fs::write(ws.join("notes.md"), "alpha beta gamma").unwrap();
        let tool = WorkspaceStrReplaceTool::new(&ws);
        assert_eq!(tool.name(), "str_replace");

        let result = tool
            .execute(serde_json::json!({
                "path": "notes.md",
                "old_str": "beta",
                "new_str": "BETA"
            }))
            .await
            .unwrap();
        assert!(result.contains("notes.md"), "{result}");
        assert!(result.contains("\"replacements\": 1"), "{result}");
        assert!(result.contains("\"replace_all\": false"), "{result}");
        assert_eq!(
            fs::read_to_string(ws.join("notes.md")).unwrap(),
            "alpha BETA gamma"
        );
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn str_replace_all_occurrences() {
        let ws = unique_temp("jiaclaw_str_replace_all");
        fs::write(ws.join("notes.md"), "foo bar foo baz foo").unwrap();
        let tool = WorkspaceStrReplaceTool::new(&ws);

        let result = tool
            .execute(serde_json::json!({
                "path": "notes.md",
                "old_str": "foo",
                "new_str": "qux",
                "replace_all": true
            }))
            .await
            .unwrap();
        assert!(result.contains("\"replacements\": 3"), "{result}");
        assert!(result.contains("\"replace_all\": true"), "{result}");
        assert_eq!(
            fs::read_to_string(ws.join("notes.md")).unwrap(),
            "qux bar qux baz qux"
        );
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn str_replace_zero_or_multiple_matches_fail_without_writing() {
        let ws = unique_temp("jiaclaw_str_replace_count");
        fs::write(ws.join("notes.md"), "one two two three").unwrap();
        let tool = WorkspaceStrReplaceTool::new(&ws);

        let err = tool
            .execute(serde_json::json!({
                "path": "notes.md",
                "old_str": "missing",
                "new_str": "x"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("0 次") || err.contains("未找到"), "{err}");
        assert_eq!(
            fs::read_to_string(ws.join("notes.md")).unwrap(),
            "one two two three"
        );

        let err = tool
            .execute(serde_json::json!({
                "path": "notes.md",
                "old_str": "two",
                "new_str": "TWO"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("2 次") || err.contains("恰好 1 次"), "{err}");
        assert_eq!(
            fs::read_to_string(ws.join("notes.md")).unwrap(),
            "one two two three"
        );
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn str_replace_rejects_traversal_and_absolute() {
        let ws = unique_temp("jiaclaw_str_replace_trav");
        let outside = ws.parent().unwrap().join(format!(
            "jiaclaw_str_replace_secret_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "keep-me").unwrap();
        fs::write(ws.join("ok.md"), "inside").unwrap();
        let tool = WorkspaceStrReplaceTool::new(&ws);

        let err = tool
            .execute(serde_json::json!({
                "path": "../secret.md",
                "old_str": "keep",
                "new_str": "pwned"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("穿越") || err.contains("安全"), "{err}");
        assert_eq!(fs::read_to_string(&outside).unwrap(), "keep-me");

        let err = tool
            .execute(serde_json::json!({
                "path": "/etc/passwd",
                "old_str": "root",
                "new_str": "pwned"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("绝对路径"), "{err}");
        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn str_replace_rejects_symlink_escape() {
        let ws = unique_temp("jiaclaw_str_replace_symlink");
        let outside = ws.parent().unwrap().join(format!(
            "jiaclaw_str_replace_outside_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "secret-outside").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("leak.md")).unwrap();

        let tool = WorkspaceStrReplaceTool::new(&ws);
        let result = tool
            .execute(serde_json::json!({
                "path": "leak.md",
                "old_str": "secret",
                "new_str": "pwned"
            }))
            .await;
        assert!(result.is_err(), "symlink 逃逸应被拒绝: {result:?}");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("安全") || err.contains("工作空间"), "{err}");
        assert_eq!(fs::read_to_string(&outside).unwrap(), "secret-outside");

        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn str_replace_rejects_oversize_read_and_write_without_writing() {
        let ws = unique_temp("jiaclaw_str_replace_limits");
        let tool = WorkspaceStrReplaceTool::with_max_bytes(&ws, 8);
        fs::write(ws.join("big.md"), "abcdefghijk").unwrap();
        let err = tool
            .execute(serde_json::json!({
                "path": "big.md",
                "old_str": "abc",
                "new_str": "xyz"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("超过上限"), "{err}");
        assert_eq!(
            fs::read_to_string(ws.join("big.md")).unwrap(),
            "abcdefghijk"
        );

        fs::write(ws.join("keep.md"), "ab").unwrap();
        let err = tool
            .execute(serde_json::json!({
                "path": "keep.md",
                "old_str": "ab",
                "new_str": "abcdefghijk"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("超过上限"), "{err}");
        assert_eq!(fs::read_to_string(ws.join("keep.md")).unwrap(), "ab");
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn str_replace_rejects_binary() {
        let ws = unique_temp("jiaclaw_str_replace_bin");
        fs::write(ws.join("bin.dat"), [0_u8, 1, 2, 3, 255]).unwrap();
        let err = WorkspaceStrReplaceTool::new(&ws)
            .execute(serde_json::json!({
                "path": "bin.dat",
                "old_str": "\u{0}",
                "new_str": "x"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("二进制"), "{err}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn glob_matches_filename_and_path() {
        assert!(glob_matches("*.rs", "src/lib.rs"));
        assert!(glob_matches("*.rs", "lib.rs"));
        assert!(!glob_matches("*.rs", "lib.toml"));
        assert!(glob_matches("src/*.rs", "src/lib.rs"));
        assert!(!glob_matches("src/*.rs", "src/foo/lib.rs"));
        assert!(glob_matches("**/*.rs", "src/foo/lib.rs"));
        assert!(glob_matches("**/*.rs", "lib.rs"));
        assert!(glob_matches("test_*.rs", "test_files.rs"));
        assert!(glob_matches("notes/*.md", "notes/a.md"));
        assert!(!glob_matches("notes/*.md", "src/a.md"));
        assert!(glob_matches("?", "a"));
        assert!(!glob_matches("?", "ab"));
    }

    #[test]
    fn clamp_grep_max_matches_bounds() {
        assert_eq!(clamp_grep_max_matches(0), 1);
        assert_eq!(clamp_grep_max_matches(50), 50);
        assert_eq!(clamp_grep_max_matches(200), 200);
        assert_eq!(clamp_grep_max_matches(201), 200);
    }

    #[test]
    fn parse_grep_args_requires_pattern_and_defaults() {
        let err = parse_grep_args(&serde_json::json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("pattern"), "{err}");

        let err = parse_grep_args(&serde_json::json!({"pattern": ""}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("pattern"), "{err}");

        let parsed = parse_grep_args(&serde_json::json!({"pattern": "foo"})).unwrap();
        assert_eq!(parsed.pattern, "foo");
        assert_eq!(parsed.path, ".");
        assert!(parsed.glob.is_none());
        assert!(!parsed.case_insensitive);
        assert_eq!(parsed.max_matches, GREP_DEFAULT_MAX_MATCHES);

        let parsed = parse_grep_args(&serde_json::json!({
            "pattern": "Foo",
            "path": " notes ",
            "glob": " *.md ",
            "case_insensitive": true,
            "max_matches": 0
        }))
        .unwrap();
        assert_eq!(parsed.path, "notes");
        assert_eq!(parsed.glob.as_deref(), Some("*.md"));
        assert!(parsed.case_insensitive);
        assert_eq!(parsed.max_matches, 1);

        let parsed = parse_grep_args(&serde_json::json!({
            "pattern": "x",
            "max_matches": 9999
        }))
        .unwrap();
        assert_eq!(parsed.max_matches, GREP_MAX_MATCHES);

        let err = parse_grep_args(&serde_json::json!({
            "pattern": "x",
            "case_insensitive": "yes"
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("case_insensitive"), "{err}");
    }

    #[tokio::test]
    async fn grep_finds_literal_matches_across_files() {
        let ws = unique_temp("jiaclaw_grep_ok");
        fs::write(ws.join("MEMORY.md"), "alpha beta\ngamma\n").unwrap();
        fs::create_dir_all(ws.join("notes")).unwrap();
        fs::write(ws.join("notes").join("a.md"), "hello beta world\n").unwrap();
        fs::write(ws.join("notes").join("b.rs"), "fn beta() {}\n").unwrap();
        let tool = WorkspaceGrepTool::new(&ws);
        assert_eq!(tool.name(), "grep");

        let result = tool
            .execute(serde_json::json!({"pattern": "beta"}))
            .await
            .unwrap();
        let parsed: GrepOutput = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.match_count, 3);
        assert!(!parsed.truncated);
        let paths: Vec<_> = parsed.matches.iter().map(|m| m.path.as_str()).collect();
        assert!(paths.contains(&"MEMORY.md"), "{paths:?}");
        assert!(paths.contains(&"notes/a.md"), "{paths:?}");
        assert!(paths.contains(&"notes/b.rs"), "{paths:?}");
        assert!(parsed.matches.iter().all(|m| m.line >= 1));
        assert!(parsed.matches.iter().any(|m| m.snippet.contains("beta")));
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn grep_glob_and_case_insensitive_and_max_matches() {
        let ws = unique_temp("jiaclaw_grep_filters");
        fs::write(ws.join("keep.md"), "Needle here\n").unwrap();
        fs::write(ws.join("skip.rs"), "Needle here too\n").unwrap();
        fs::write(ws.join("other.md"), "needle again\nsecond needle\n").unwrap();
        let tool = WorkspaceGrepTool::new(&ws);

        let globbed = tool
            .execute(serde_json::json!({
                "pattern": "Needle",
                "glob": "*.md"
            }))
            .await
            .unwrap();
        let parsed: GrepOutput = serde_json::from_str(&globbed).unwrap();
        assert_eq!(parsed.match_count, 1);
        assert_eq!(parsed.matches[0].path, "keep.md");

        let insensitive = tool
            .execute(serde_json::json!({
                "pattern": "needle",
                "case_insensitive": true,
                "glob": "*.md"
            }))
            .await
            .unwrap();
        let parsed: GrepOutput = serde_json::from_str(&insensitive).unwrap();
        assert_eq!(parsed.match_count, 3);

        let limited = tool
            .execute(serde_json::json!({
                "pattern": "needle",
                "case_insensitive": true,
                "max_matches": 1
            }))
            .await
            .unwrap();
        let parsed: GrepOutput = serde_json::from_str(&limited).unwrap();
        assert_eq!(parsed.match_count, 1);
        assert!(parsed.truncated);
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn grep_single_file_and_truncates_long_snippet() {
        let ws = unique_temp("jiaclaw_grep_file");
        let long = format!("{}FOUND{}", "x".repeat(180), "y".repeat(80));
        fs::write(ws.join("notes.md"), format!("{long}\nnope\n")).unwrap();
        let tool = WorkspaceGrepTool::new(&ws);
        let result = tool
            .execute(serde_json::json!({
                "pattern": "FOUND",
                "path": "notes.md"
            }))
            .await
            .unwrap();
        let parsed: GrepOutput = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.match_count, 1);
        assert_eq!(parsed.matches[0].line, 1);
        assert!(parsed.matches[0].snippet.contains("FOUND"));
        assert!(parsed.matches[0].snippet.ends_with('…'));
        assert!(parsed.matches[0].snippet.chars().count() <= GREP_SNIPPET_MAX_CHARS + 1);
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn grep_skips_binary_in_tree_and_errors_on_single_binary() {
        let ws = unique_temp("jiaclaw_grep_bin");
        fs::write(ws.join("ok.md"), "needle\n").unwrap();
        fs::write(ws.join("bin.dat"), [0_u8, 1, 2, 3, 255]).unwrap();
        let tool = WorkspaceGrepTool::new(&ws);

        let tree = tool
            .execute(serde_json::json!({"pattern": "needle"}))
            .await
            .unwrap();
        let parsed: GrepOutput = serde_json::from_str(&tree).unwrap();
        assert_eq!(parsed.match_count, 1);
        assert_eq!(parsed.matches[0].path, "ok.md");

        let err = tool
            .execute(serde_json::json!({
                "pattern": "needle",
                "path": "bin.dat"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("二进制"), "{err}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn grep_rejects_traversal_and_absolute() {
        let ws = unique_temp("jiaclaw_grep_trav");
        fs::write(ws.join("ok.md"), "inside").unwrap();
        let tool = WorkspaceGrepTool::new(&ws);

        let err = tool
            .execute(serde_json::json!({
                "pattern": "inside",
                "path": "../secret.md"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("穿越") || err.contains("安全"), "{err}");

        let err = tool
            .execute(serde_json::json!({
                "pattern": "root",
                "path": "/etc/passwd"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("绝对路径"), "{err}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn grep_rejects_symlink_escape_and_skips_symlink_files() {
        let ws = unique_temp("jiaclaw_grep_symlink");
        let outside = ws.parent().unwrap().join(format!(
            "jiaclaw_grep_outside_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "secret-outside needle").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("leak.md")).unwrap();
        fs::write(ws.join("ok.md"), "needle inside").unwrap();

        let tool = WorkspaceGrepTool::new(&ws);
        let result = tool
            .execute(serde_json::json!({
                "pattern": "needle",
                "path": "leak.md"
            }))
            .await;
        assert!(result.is_err(), "symlink 逃逸应被拒绝: {result:?}");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("安全") || err.contains("工作空间"), "{err}");

        let tree = tool
            .execute(serde_json::json!({"pattern": "needle"}))
            .await
            .unwrap();
        let parsed: GrepOutput = serde_json::from_str(&tree).unwrap();
        assert_eq!(parsed.match_count, 1);
        assert_eq!(parsed.matches[0].path, "ok.md");
        assert!(
            parsed.matches.iter().all(|m| m.path != "leak.md"),
            "不得跟随逃逸 symlink 文件: {parsed:?}"
        );

        let outside_dir = ws.parent().unwrap().join(format!(
            "jiaclaw_grep_outside_dir_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::create_dir_all(&outside_dir).unwrap();
        fs::write(outside_dir.join("secret.txt"), "needle-dir").unwrap();
        std::os::unix::fs::symlink(&outside_dir, ws.join("escape")).unwrap();
        let dir_result = tool
            .execute(serde_json::json!({
                "pattern": "needle",
                "path": "escape"
            }))
            .await;
        assert!(
            dir_result.is_err(),
            "symlink 目录逃逸应被拒绝: {dir_result:?}"
        );

        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&outside_dir);
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn grep_missing_path_is_explicit_error() {
        let ws = unique_temp("jiaclaw_grep_missing");
        let tool = WorkspaceGrepTool::new(&ws);
        let err = tool
            .execute(serde_json::json!({
                "pattern": "x",
                "path": "no-such.md"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("不存在"), "{err}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn clamp_glob_max_results_bounds() {
        assert_eq!(clamp_glob_max_results(0), 1);
        assert_eq!(clamp_glob_max_results(100), 100);
        assert_eq!(clamp_glob_max_results(500), 500);
        assert_eq!(clamp_glob_max_results(501), 500);
    }

    #[test]
    fn parse_glob_args_requires_pattern_and_defaults() {
        let err = parse_glob_args(&serde_json::json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("pattern"), "{err}");

        let err = parse_glob_args(&serde_json::json!({"pattern": ""}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("pattern"), "{err}");

        let err = parse_glob_args(&serde_json::json!({"pattern": "   "}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("pattern"), "{err}");

        let parsed = parse_glob_args(&serde_json::json!({"pattern": "**/*.rs"})).unwrap();
        assert_eq!(parsed.pattern, "**/*.rs");
        assert_eq!(parsed.path, ".");
        assert_eq!(parsed.max_results, GLOB_DEFAULT_MAX_RESULTS);

        let parsed = parse_glob_args(&serde_json::json!({
            "pattern": " src/**/*.toml ",
            "path": " crates ",
            "max_results": 0
        }))
        .unwrap();
        assert_eq!(parsed.pattern, "src/**/*.toml");
        assert_eq!(parsed.path, "crates");
        assert_eq!(parsed.max_results, 1);

        let parsed = parse_glob_args(&serde_json::json!({
            "pattern": "*.md",
            "max_results": 9999
        }))
        .unwrap();
        assert_eq!(parsed.max_results, GLOB_MAX_RESULTS);

        let err = parse_glob_args(&serde_json::json!({
            "pattern": "*.rs",
            "max_results": "many"
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("max_results"), "{err}");
    }

    #[tokio::test]
    async fn glob_lists_files_sorted_and_skips_directories() {
        let ws = unique_temp("jiaclaw_glob_ok");
        fs::write(ws.join("MEMORY.md"), "mem").unwrap();
        fs::create_dir_all(ws.join("src")).unwrap();
        fs::write(ws.join("src").join("lib.rs"), "fn x() {}").unwrap();
        fs::write(ws.join("src").join("main.rs"), "fn main() {}").unwrap();
        fs::create_dir_all(ws.join("src").join("nested")).unwrap();
        fs::write(ws.join("src").join("nested").join("mod.rs"), "").unwrap();
        fs::write(ws.join("Cargo.toml"), "[package]").unwrap();
        let tool = WorkspaceGlobTool::new(&ws);
        assert_eq!(tool.name(), "glob");

        let result = tool
            .execute(serde_json::json!({"pattern": "**/*.rs"}))
            .await
            .unwrap();
        let parsed: GlobOutput = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.match_count, 3);
        assert!(!parsed.truncated);
        assert_eq!(
            parsed.matches,
            vec![
                "src/lib.rs".to_string(),
                "src/main.rs".to_string(),
                "src/nested/mod.rs".to_string()
            ]
        );

        let nested_toml = tool
            .execute(serde_json::json!({"pattern": "src/**/*.toml"}))
            .await
            .unwrap();
        let parsed: GlobOutput = serde_json::from_str(&nested_toml).unwrap();
        assert_eq!(parsed.match_count, 0);

        let by_name = tool
            .execute(serde_json::json!({"pattern": "*.md"}))
            .await
            .unwrap();
        let parsed: GlobOutput = serde_json::from_str(&by_name).unwrap();
        assert_eq!(parsed.matches, vec!["MEMORY.md".to_string()]);
        assert!(
            parsed.matches.iter().all(|p| !p.ends_with('/')),
            "不得返回目录: {parsed:?}"
        );
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn glob_respects_path_and_truncates_max_results() {
        let ws = unique_temp("jiaclaw_glob_filters");
        fs::create_dir_all(ws.join("src")).unwrap();
        fs::create_dir_all(ws.join("notes")).unwrap();
        fs::write(ws.join("src").join("a.rs"), "a").unwrap();
        fs::write(ws.join("src").join("b.rs"), "b").unwrap();
        fs::write(ws.join("notes").join("c.rs"), "c").unwrap();
        let tool = WorkspaceGlobTool::new(&ws);

        let scoped = tool
            .execute(serde_json::json!({
                "pattern": "*.rs",
                "path": "src"
            }))
            .await
            .unwrap();
        let parsed: GlobOutput = serde_json::from_str(&scoped).unwrap();
        assert_eq!(parsed.match_count, 2);
        assert_eq!(
            parsed.matches,
            vec!["src/a.rs".to_string(), "src/b.rs".to_string()]
        );

        let limited = tool
            .execute(serde_json::json!({
                "pattern": "**/*.rs",
                "max_results": 1
            }))
            .await
            .unwrap();
        let parsed: GlobOutput = serde_json::from_str(&limited).unwrap();
        assert_eq!(parsed.match_count, 1);
        assert!(parsed.truncated);
        assert_eq!(parsed.matches[0], "notes/c.rs");
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn glob_single_file_and_filename_pattern() {
        let ws = unique_temp("jiaclaw_glob_file");
        fs::write(ws.join("notes.md"), "x").unwrap();
        fs::write(ws.join("skip.rs"), "y").unwrap();
        let tool = WorkspaceGlobTool::new(&ws);

        let hit = tool
            .execute(serde_json::json!({
                "pattern": "*.md",
                "path": "notes.md"
            }))
            .await
            .unwrap();
        let parsed: GlobOutput = serde_json::from_str(&hit).unwrap();
        assert_eq!(parsed.matches, vec!["notes.md".to_string()]);

        let miss = tool
            .execute(serde_json::json!({
                "pattern": "*.rs",
                "path": "notes.md"
            }))
            .await
            .unwrap();
        let parsed: GlobOutput = serde_json::from_str(&miss).unwrap();
        assert_eq!(parsed.match_count, 0);
        assert!(parsed.matches.is_empty());
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn glob_rejects_traversal_and_absolute() {
        let ws = unique_temp("jiaclaw_glob_trav");
        fs::write(ws.join("ok.md"), "inside").unwrap();
        let tool = WorkspaceGlobTool::new(&ws);

        let err = tool
            .execute(serde_json::json!({
                "pattern": "*.md",
                "path": "../secret.md"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("穿越") || err.contains("安全"), "{err}");

        let err = tool
            .execute(serde_json::json!({
                "pattern": "*",
                "path": "/etc"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("绝对路径"), "{err}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn glob_rejects_symlink_escape_and_skips_symlink_files() {
        let ws = unique_temp("jiaclaw_glob_symlink");
        let outside = ws.parent().unwrap().join(format!(
            "jiaclaw_glob_outside_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "secret-outside").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("leak.md")).unwrap();
        fs::write(ws.join("ok.md"), "inside").unwrap();

        let tool = WorkspaceGlobTool::new(&ws);
        let result = tool
            .execute(serde_json::json!({
                "pattern": "*.md",
                "path": "leak.md"
            }))
            .await;
        assert!(result.is_err(), "symlink 逃逸应被拒绝: {result:?}");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("安全") || err.contains("工作空间"), "{err}");

        let tree = tool
            .execute(serde_json::json!({"pattern": "*.md"}))
            .await
            .unwrap();
        let parsed: GlobOutput = serde_json::from_str(&tree).unwrap();
        assert_eq!(parsed.matches, vec!["ok.md".to_string()]);
        assert!(
            parsed.matches.iter().all(|p| p != "leak.md"),
            "不得跟随逃逸 symlink 文件: {parsed:?}"
        );

        let outside_dir = ws.parent().unwrap().join(format!(
            "jiaclaw_glob_outside_dir_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::create_dir_all(&outside_dir).unwrap();
        fs::write(outside_dir.join("secret.txt"), "nope").unwrap();
        std::os::unix::fs::symlink(&outside_dir, ws.join("escape")).unwrap();
        let dir_result = tool
            .execute(serde_json::json!({
                "pattern": "**/*",
                "path": "escape"
            }))
            .await;
        assert!(
            dir_result.is_err(),
            "symlink 目录逃逸应被拒绝: {dir_result:?}"
        );

        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&outside_dir);
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn glob_missing_path_is_explicit_error() {
        let ws = unique_temp("jiaclaw_glob_missing");
        let tool = WorkspaceGlobTool::new(&ws);
        let err = tool
            .execute(serde_json::json!({
                "pattern": "*.md",
                "path": "no-such"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("不存在"), "{err}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn parse_mkdir_args_requires_path_and_defaults_recursive() {
        let err = parse_mkdir_args(&serde_json::json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("path"), "{err}");

        let err = parse_mkdir_args(&serde_json::json!({"path": "   "}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("path"), "{err}");

        let parsed = parse_mkdir_args(&serde_json::json!({"path": " notes/deep "})).unwrap();
        assert_eq!(parsed.path, "notes/deep");
        assert!(parsed.recursive);

        let parsed = parse_mkdir_args(&serde_json::json!({
            "path": "a",
            "recursive": false
        }))
        .unwrap();
        assert!(!parsed.recursive);

        let parsed = parse_mkdir_args(&serde_json::json!({
            "path": "a",
            "parents": false
        }))
        .unwrap();
        assert!(!parsed.recursive);

        let parsed = parse_mkdir_args(&serde_json::json!({
            "path": "a",
            "recursive": true,
            "parents": true
        }))
        .unwrap();
        assert!(parsed.recursive);

        let err = parse_mkdir_args(&serde_json::json!({
            "path": "a",
            "recursive": true,
            "parents": false
        }))
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("recursive") && err.contains("parents"),
            "{err}"
        );

        let err = parse_mkdir_args(&serde_json::json!({
            "path": "a",
            "recursive": "yes"
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("recursive"), "{err}");
    }

    #[tokio::test]
    async fn mkdir_creates_nested_dirs_and_is_idempotent() {
        let ws = unique_temp("jiaclaw_mkdir_ok");
        let tool = WorkspaceMkdirTool::new(&ws);
        assert_eq!(tool.name(), "mkdir");

        let created = tool
            .execute(serde_json::json!({"path": "notes/deep"}))
            .await
            .unwrap();
        let parsed: MkdirOutput = serde_json::from_str(&created).unwrap();
        assert_eq!(parsed.path, "notes/deep");
        assert!(parsed.created);
        assert!(!parsed.existed);
        assert!(parsed.recursive);
        assert!(ws.join("notes").join("deep").is_dir());

        let again = tool
            .execute(serde_json::json!({"path": "notes/deep"}))
            .await
            .unwrap();
        let parsed: MkdirOutput = serde_json::from_str(&again).unwrap();
        assert!(!parsed.created);
        assert!(parsed.existed);
        assert!(ws.join("notes").join("deep").is_dir());
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn mkdir_rejects_existing_file() {
        let ws = unique_temp("jiaclaw_mkdir_file");
        fs::write(ws.join("notes.md"), "not-a-dir").unwrap();
        let tool = WorkspaceMkdirTool::new(&ws);
        let err = tool
            .execute(serde_json::json!({"path": "notes.md"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("不是目录") || err.contains("文件"), "{err}");
        assert!(ws.join("notes.md").is_file());
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn mkdir_non_recursive_requires_parent() {
        let ws = unique_temp("jiaclaw_mkdir_norecurse");
        let tool = WorkspaceMkdirTool::new(&ws);

        let err = tool
            .execute(serde_json::json!({
                "path": "missing/child",
                "recursive": false
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("父目录不存在"), "{err}");
        assert!(!ws.join("missing").exists());

        let created = tool
            .execute(serde_json::json!({
                "path": "notes",
                "parents": false
            }))
            .await
            .unwrap();
        let parsed: MkdirOutput = serde_json::from_str(&created).unwrap();
        assert!(parsed.created);
        assert!(!parsed.recursive);
        assert!(ws.join("notes").is_dir());

        let nested = tool
            .execute(serde_json::json!({
                "path": "notes/leaf",
                "recursive": false
            }))
            .await
            .unwrap();
        let parsed: MkdirOutput = serde_json::from_str(&nested).unwrap();
        assert!(parsed.created);
        assert!(ws.join("notes").join("leaf").is_dir());
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn mkdir_rejects_traversal_and_absolute() {
        let ws = unique_temp("jiaclaw_mkdir_trav");
        let tool = WorkspaceMkdirTool::new(&ws);

        let err = tool
            .execute(serde_json::json!({"path": "../secret-dir"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("穿越") || err.contains("安全"), "{err}");
        assert!(!ws.parent().unwrap().join("secret-dir").exists());

        let err = tool
            .execute(serde_json::json!({"path": "/tmp/jiaclaw-mkdir-escape"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("绝对路径"), "{err}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mkdir_rejects_symlink_escape() {
        let ws = unique_temp("jiaclaw_mkdir_symlink");
        let outside_dir = ws.parent().unwrap().join(format!(
            "jiaclaw_mkdir_outside_dir_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::create_dir_all(&outside_dir).unwrap();
        std::os::unix::fs::symlink(&outside_dir, ws.join("escape")).unwrap();

        let tool = WorkspaceMkdirTool::new(&ws);
        let result = tool.execute(serde_json::json!({"path": "escape"})).await;
        assert!(result.is_err(), "symlink 目录逃逸应被拒绝: {result:?}");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("安全") || err.contains("工作空间"), "{err}");

        let nested = tool
            .execute(serde_json::json!({"path": "escape/pwned"}))
            .await;
        assert!(nested.is_err(), "经 symlink 创建子目录应被拒绝: {nested:?}");
        assert!(!outside_dir.join("pwned").exists());

        let _ = fs::remove_dir_all(&outside_dir);
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn parse_move_args_requires_from_to_and_accepts_aliases() {
        let err = parse_move_args(&serde_json::json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("from") || err.contains("source"), "{err}");

        let err = parse_move_args(&serde_json::json!({"from": "a.md"}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("to") || err.contains("destination"), "{err}");

        let err = parse_move_args(&serde_json::json!({"from": "  ", "to": "b.md"}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("from"), "{err}");

        let parsed = parse_move_args(&serde_json::json!({
            "from": " notes/a.md ",
            "to": " notes/b.md "
        }))
        .unwrap();
        assert_eq!(parsed.from, "notes/a.md");
        assert_eq!(parsed.to, "notes/b.md");
        assert!(!parsed.overwrite);

        let parsed = parse_move_args(&serde_json::json!({
            "source": "a.md",
            "destination": "b.md",
            "overwrite": true
        }))
        .unwrap();
        assert_eq!(parsed.from, "a.md");
        assert_eq!(parsed.to, "b.md");
        assert!(parsed.overwrite);

        let parsed = parse_move_args(&serde_json::json!({
            "from": "a.md",
            "source": "a.md",
            "to": "b.md",
            "destination": "b.md"
        }))
        .unwrap();
        assert_eq!(parsed.from, "a.md");
        assert_eq!(parsed.to, "b.md");

        let err = parse_move_args(&serde_json::json!({
            "from": "a.md",
            "source": "other.md",
            "to": "b.md"
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("from") && err.contains("source"), "{err}");

        let err = parse_move_args(&serde_json::json!({
            "from": "a.md",
            "to": "b.md",
            "destination": "c.md"
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("to") && err.contains("destination"), "{err}");

        let err = parse_move_args(&serde_json::json!({
            "from": "a.md",
            "to": "b.md",
            "overwrite": "yes"
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("overwrite"), "{err}");
    }

    #[tokio::test]
    async fn move_renames_regular_file() {
        let ws = unique_temp("jiaclaw_move_file_ok");
        fs::write(ws.join("old.md"), "hello").unwrap();
        let tool = WorkspaceMoveTool::new(&ws);
        assert_eq!(tool.name(), "move");

        let result = tool
            .execute(serde_json::json!({
                "from": "old.md",
                "to": "new.md"
            }))
            .await
            .unwrap();
        let parsed: MoveOutput = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.from, "old.md");
        assert_eq!(parsed.to, "new.md");
        assert!(!parsed.overwrite);
        assert!(!parsed.overwritten);
        assert_eq!(parsed.kind, MoveKind::File);
        assert!(!ws.join("old.md").exists());
        assert_eq!(fs::read_to_string(ws.join("new.md")).unwrap(), "hello");
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn move_accepts_source_destination_aliases() {
        let ws = unique_temp("jiaclaw_move_alias");
        fs::create_dir_all(ws.join("notes")).unwrap();
        fs::write(ws.join("notes").join("a.md"), "x").unwrap();
        let tool = WorkspaceMoveTool::new(&ws);
        let result = tool
            .execute(serde_json::json!({
                "source": "notes/a.md",
                "destination": "notes/b.md"
            }))
            .await
            .unwrap();
        let parsed: MoveOutput = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.from, "notes/a.md");
        assert_eq!(parsed.to, "notes/b.md");
        assert!(!ws.join("notes").join("a.md").exists());
        assert!(ws.join("notes").join("b.md").is_file());
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn move_empty_and_nonempty_directories() {
        let ws = unique_temp("jiaclaw_move_dirs");
        fs::create_dir_all(ws.join("empty")).unwrap();
        fs::create_dir_all(ws.join("full")).unwrap();
        fs::write(ws.join("full").join("a.md"), "nested").unwrap();
        let tool = WorkspaceMoveTool::new(&ws);

        let empty = tool
            .execute(serde_json::json!({
                "from": "empty",
                "to": "empty-renamed"
            }))
            .await
            .unwrap();
        let parsed: MoveOutput = serde_json::from_str(&empty).unwrap();
        assert_eq!(parsed.kind, MoveKind::Dir);
        assert!(!ws.join("empty").exists());
        assert!(ws.join("empty-renamed").is_dir());

        let full = tool
            .execute(serde_json::json!({
                "from": "full",
                "to": "full-renamed"
            }))
            .await
            .unwrap();
        let parsed: MoveOutput = serde_json::from_str(&full).unwrap();
        assert_eq!(parsed.kind, MoveKind::Dir);
        assert!(!ws.join("full").exists());
        assert_eq!(
            fs::read_to_string(ws.join("full-renamed").join("a.md")).unwrap(),
            "nested"
        );
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn move_rejects_existing_dest_without_overwrite() {
        let ws = unique_temp("jiaclaw_move_no_overwrite");
        fs::write(ws.join("a.md"), "src").unwrap();
        fs::write(ws.join("b.md"), "dst").unwrap();
        let tool = WorkspaceMoveTool::new(&ws);
        let err = tool
            .execute(serde_json::json!({
                "from": "a.md",
                "to": "b.md"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("已存在") || err.contains("overwrite"), "{err}");
        assert_eq!(fs::read_to_string(ws.join("a.md")).unwrap(), "src");
        assert_eq!(fs::read_to_string(ws.join("b.md")).unwrap(), "dst");
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn move_overwrite_replaces_existing_file() {
        let ws = unique_temp("jiaclaw_move_overwrite");
        fs::write(ws.join("a.md"), "src").unwrap();
        fs::write(ws.join("b.md"), "dst").unwrap();
        let tool = WorkspaceMoveTool::new(&ws);
        let result = tool
            .execute(serde_json::json!({
                "from": "a.md",
                "to": "b.md",
                "overwrite": true
            }))
            .await
            .unwrap();
        let parsed: MoveOutput = serde_json::from_str(&result).unwrap();
        assert!(parsed.overwrite);
        assert!(parsed.overwritten);
        assert!(!ws.join("a.md").exists());
        assert_eq!(fs::read_to_string(ws.join("b.md")).unwrap(), "src");
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn move_missing_source_is_explicit_error() {
        let ws = unique_temp("jiaclaw_move_missing");
        let tool = WorkspaceMoveTool::new(&ws);
        let err = tool
            .execute(serde_json::json!({
                "from": "no-such.md",
                "to": "out.md"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("不存在"), "{err}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn move_missing_parent_is_explicit_error() {
        let ws = unique_temp("jiaclaw_move_noparent");
        fs::write(ws.join("a.md"), "x").unwrap();
        let tool = WorkspaceMoveTool::new(&ws);
        let err = tool
            .execute(serde_json::json!({
                "from": "a.md",
                "to": "missing/a.md"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("父目录不存在"), "{err}");
        assert!(ws.join("a.md").is_file());
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn move_rejects_dir_into_itself() {
        let ws = unique_temp("jiaclaw_move_into_self");
        fs::create_dir_all(ws.join("notes")).unwrap();
        fs::write(ws.join("notes").join("a.md"), "x").unwrap();
        let tool = WorkspaceMoveTool::new(&ws);
        let err = tool
            .execute(serde_json::json!({
                "from": "notes",
                "to": "notes/nested"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("自身") || err.contains("子路径"), "{err}");
        assert!(ws.join("notes").join("a.md").is_file());
        let _ = fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn move_rejects_traversal_and_absolute_on_both_ends() {
        let ws = unique_temp("jiaclaw_move_trav");
        fs::write(ws.join("ok.md"), "inside").unwrap();
        let tool = WorkspaceMoveTool::new(&ws);

        let err = tool
            .execute(serde_json::json!({
                "from": "../secret.md",
                "to": "ok.md"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("穿越") || err.contains("安全"), "{err}");

        let err = tool
            .execute(serde_json::json!({
                "from": "ok.md",
                "to": "../escaped.md"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("穿越") || err.contains("安全"), "{err}");
        assert!(ws.join("ok.md").is_file());

        let err = tool
            .execute(serde_json::json!({
                "from": "/etc/passwd",
                "to": "stolen.md"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("绝对路径"), "{err}");

        let err = tool
            .execute(serde_json::json!({
                "from": "ok.md",
                "to": "/tmp/jiaclaw-move-escape"
            }))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("绝对路径"), "{err}");
        let _ = fs::remove_dir_all(&ws);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn move_rejects_symlink_escape() {
        let ws = unique_temp("jiaclaw_move_symlink");
        let outside = ws.parent().unwrap().join(format!(
            "jiaclaw_move_outside_{}",
            ws.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&outside, "secret-outside").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("leak.md")).unwrap();
        fs::write(ws.join("ok.md"), "inside").unwrap();

        let tool = WorkspaceMoveTool::new(&ws);
        let result = tool
            .execute(serde_json::json!({
                "from": "leak.md",
                "to": "copied.md"
            }))
            .await;
        assert!(result.is_err(), "symlink 源逃逸应被拒绝: {result:?}");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("安全") || err.contains("工作空间") || err.contains("不是"),
            "{err}"
        );

        let dest_link = tool
            .execute(serde_json::json!({
                "from": "ok.md",
                "to": "leak.md",
                "overwrite": true
            }))
            .await;
        assert!(
            dest_link.is_err(),
            "经 symlink 覆盖逃逸应被拒绝: {dest_link:?}"
        );
        assert_eq!(fs::read_to_string(&outside).unwrap(), "secret-outside");

        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&ws);
    }
}
