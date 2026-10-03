// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The file tools' small glob language, with bounded polynomial matching.
//! `**` branches over path components through a rolling dynamic program;
//! repeated stars never create a recursive backtracking tree.

/// Preserve the existing basename, slash normalization, and leading `./` rules.
/// File-tool callers separately bound their pattern and workspace-relative path.
/// Matching uses O(pattern characters * path characters) work and rolling
/// O(path characters) state. Normalized inputs additionally need O(pattern +
/// path characters) storage. `?` consumes one Unicode scalar.
pub(crate) fn matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    let path = path.replace('\\', "/");
    if pattern.is_empty() {
        return true;
    }
    let pattern = pattern.trim_start_matches("./");
    let path = path.trim_start_matches("./");
    if !pattern.contains('/') {
        let name = path.rsplit('/').next().unwrap_or(path);
        return segment_matches(pattern, name);
    }
    let components: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let mut reached = vec![false; components.len() + 1];
    reached[0] = true;
    for segment in pattern.split('/').filter(|part| !part.is_empty()) {
        if segment == "**" {
            // Zero components keep each state; consuming one extends the
            // preceding state. Left-to-right propagation covers any suffix.
            for index in 1..reached.len() {
                reached[index] |= reached[index - 1];
            }
        } else {
            let mut next = vec![false; reached.len()];
            for (index, name) in components.iter().enumerate() {
                next[index + 1] = reached[index] && segment_matches(segment, name);
            }
            reached = next;
        }
    }
    reached[components.len()]
}

fn segment_matches(pattern: &str, text: &str) -> bool {
    let text: Vec<char> = text.chars().collect();
    let mut reached = vec![false; text.len() + 1];
    reached[0] = true;
    for token in pattern.chars() {
        if token == '*' {
            for index in 1..reached.len() {
                reached[index] |= reached[index - 1];
            }
        } else {
            // Descending updates retain the preceding pattern's states.
            for index in (1..reached.len()).rev() {
                reached[index] = reached[index - 1] && (token == '?' || token == text[index - 1]);
            }
            reached[0] = false;
        }
    }
    reached[text.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_basename_component_and_normalization_semantics() {
        for (pattern, path, expected) in [
            ("*.rs", "src/deep/lib.rs", true),
            ("*.rs", "src/lib.toml", false),
            ("src/*.rs", "src/deep/lib.rs", false),
            ("src/**/*.rs", "src/lib.rs", true),
            ("src/**/*.rs", "src/deep/lib.rs", true),
            ("src/**", "src", true),
            ("**/*.rs", "lib.rs", true),
            ("a/***/c", "a/b/d/c", false),
            ("a/**/c", "a/b/d/c", true),
            ("a//b", "a/b", true),
            ("a/b", "a//b", true),
            ("././src\\**\\*.rs", "./src\\deep\\lib.rs", true),
            ("[ab]", "a", false),
            ("[ab]", "[ab]", true),
            ("a?c", "a中c", true),
            ("a?c", "a🙂c", true),
            ("?", "e\u{301}", false),
            ("??", "e\u{301}", true),
            ("*.RS", "src/lib.rs", false),
            ("", "anything", true),
            ("./", "anything", false),
            ("./", "", true),
            ("/", "", true),
            ("**/", "a/b/", true),
            ("*", "a/b/", true),
            ("?", "a/b/", false),
        ] {
            assert_eq!(matches(pattern, path), expected, "{pattern:?} vs {path:?}");
        }
    }

    #[test]
    fn repeated_recursive_stars_and_maximum_tool_inputs_terminate() {
        // Both patterns fit the public glob tool's 256-byte pattern budget.
        // A recursive matcher explores a combinatorial number of failures.
        let path = std::iter::repeat_n("component", 64)
            .collect::<Vec<_>>()
            .join("/");
        let misses = format!("{}missing", "**/".repeat(80));
        assert!(misses.len() <= 256 && path.len() <= 1024);
        assert!(!matches(&misses, &path));
        assert!(matches(&format!("{}component", "**/".repeat(80)), &path));
        assert!(!matches(
            &format!("{}z", "*a".repeat(127)),
            &"a".repeat(1024)
        ));
        assert!(matches(
            &format!("{}*", "*a".repeat(127)),
            &"a".repeat(1024)
        ));
        assert!(matches("**/**/**", ""));
        assert!(!matches("**/**/x/**", "a/b/c"));
    }

    // A deliberately tiny independent exhaustive reference, never exposed to
    // user-sized inputs. The production implementation contains no recursion.
    fn reference_segment(pattern: &[char], text: &[char]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some(('*', rest)) => {
                reference_segment(rest, text)
                    || (!text.is_empty() && reference_segment(pattern, &text[1..]))
            }
            Some((token, rest)) => {
                !text.is_empty()
                    && (*token == '?' || *token == text[0])
                    && reference_segment(rest, &text[1..])
            }
        }
    }
    fn reference_parts(pattern: &[&str], path: &[&str]) -> bool {
        match pattern.split_first() {
            None => path.is_empty(),
            Some((&"**", rest)) => {
                reference_parts(rest, path)
                    || (!path.is_empty() && reference_parts(pattern, &path[1..]))
            }
            Some((segment, rest)) => {
                !path.is_empty()
                    && reference_segment(
                        &segment.chars().collect::<Vec<_>>(),
                        &path[0].chars().collect::<Vec<_>>(),
                    )
                    && reference_parts(rest, &path[1..])
            }
        }
    }
    fn reference(pattern: &str, path: &str) -> bool {
        let pattern = pattern.replace('\\', "/");
        let path = path.replace('\\', "/");
        if pattern.is_empty() {
            return true;
        }
        let pattern = pattern.trim_start_matches("./");
        let path = path.trim_start_matches("./");
        if !pattern.contains('/') {
            return reference_segment(
                &pattern.chars().collect::<Vec<_>>(),
                &path
                    .rsplit('/')
                    .next()
                    .unwrap_or(path)
                    .chars()
                    .collect::<Vec<_>>(),
            );
        }
        reference_parts(
            &pattern
                .split('/')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>(),
            &path
                .split('/')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn randomized_small_inputs_match_independent_exhaustive_reference() {
        let alphabet = ['a', 'b', '中', '🙂', '*', '?', '/', '\\', '.'];
        let mut state = 0x3d62_a40b_u64;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            (state >> 32) as usize
        };
        for _ in 0..4096 {
            let pattern_len = next() % 9;
            let path_len = next() % 9;
            let pattern: String = (0..pattern_len)
                .map(|_| alphabet[next() % alphabet.len()])
                .collect();
            let path: String = (0..path_len)
                .map(|_| alphabet[next() % alphabet.len()])
                .collect();
            assert_eq!(
                matches(&pattern, &path),
                reference(&pattern, &path),
                "{pattern:?} vs {path:?}"
            );
        }
    }
}
