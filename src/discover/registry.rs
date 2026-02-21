use lazy_static::lazy_static;
use regex::{Regex, RegexSet};

/// A rule mapping a shell command pattern to its RTK equivalent.
struct RtkRule {
    rtk_cmd: &'static str,
    /// Original command prefixes to replace with rtk_cmd (longest first for correct matching).
    rewrite_prefixes: &'static [&'static str],
    category: &'static str,
    savings_pct: f64,
    subcmd_savings: &'static [(&'static str, f64)],
    subcmd_status: &'static [(&'static str, super::report::RtkStatus)],
}

/// Result of classifying a command.
#[derive(Debug, PartialEq)]
pub enum Classification {
    Supported {
        rtk_equivalent: &'static str,
        category: &'static str,
        estimated_savings_pct: f64,
        status: super::report::RtkStatus,
    },
    Unsupported {
        base_command: String,
    },
    Ignored,
}

/// Average token counts per category for estimation when no output_len available.
pub fn category_avg_tokens(category: &str, subcmd: &str) -> usize {
    match category {
        "Git" => match subcmd {
            "log" | "diff" | "show" => 200,
            _ => 40,
        },
        "Cargo" => match subcmd {
            "test" => 500,
            _ => 150,
        },
        "Tests" => 800,
        "Files" => 100,
        "Build" => 300,
        "Infra" => 120,
        "Network" => 150,
        "GitHub" => 200,
        "PackageManager" => 150,
        _ => 150,
    }
}

// Patterns ordered to match RTK_RULES indices exactly.
const PATTERNS: &[&str] = &[
    r"^git\s+(status|log|diff|show|add|commit|push|pull|branch|fetch|stash|worktree)",
    r"^gh\s+(pr|issue|run|repo|api)",
    r"^cargo\s+(build|test|clippy|check|fmt)",
    r"^pnpm\s+(list|ls|outdated|install)",
    r"^npm\s+(run|exec)",
    r"^npx\s+",
    r"^(cat|head|tail)\s+",
    r"^(rg|grep)\s+",
    r"^ls(\s|$)",
    r"^find\s+",
    r"^(npx\s+|pnpm\s+)?tsc(\s|$)",
    r"^(npx\s+|pnpm\s+)?(eslint|biome|lint)(\s|$)",
    r"^(npx\s+|pnpm\s+)?prettier",
    r"^(npx\s+|pnpm\s+)?next\s+build",
    r"^(pnpm\s+|npx\s+)?(vitest|jest|test)(\s|$)",
    r"^(npx\s+|pnpm\s+)?playwright",
    r"^(npx\s+|pnpm\s+)?prisma",
    r"^docker\s+(ps|images|logs)",
    r"^kubectl\s+(get|logs)",
    r"^curl\s+",
    r"^wget\s+",
];

const RULES: &[RtkRule] = &[
    RtkRule {
        rtk_cmd: "rtk git",
        rewrite_prefixes: &["git"],
        category: "Git",
        savings_pct: 70.0,
        subcmd_savings: &[
            ("diff", 80.0),
            ("show", 80.0),
            ("add", 59.0),
            ("commit", 59.0),
        ],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk gh",
        rewrite_prefixes: &["gh"],
        category: "GitHub",
        savings_pct: 82.0,
        subcmd_savings: &[("pr", 87.0), ("run", 82.0), ("issue", 80.0)],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk cargo",
        rewrite_prefixes: &["cargo"],
        category: "Cargo",
        savings_pct: 80.0,
        subcmd_savings: &[("test", 90.0), ("check", 80.0)],
        subcmd_status: &[("fmt", super::report::RtkStatus::Passthrough)],
    },
    RtkRule {
        rtk_cmd: "rtk pnpm",
        rewrite_prefixes: &["pnpm"],
        category: "PackageManager",
        savings_pct: 80.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk npm",
        rewrite_prefixes: &["npm"],
        category: "PackageManager",
        savings_pct: 70.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk npx",
        rewrite_prefixes: &["npx"],
        category: "PackageManager",
        savings_pct: 70.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk read",
        rewrite_prefixes: &["cat", "head", "tail"],
        category: "Files",
        savings_pct: 60.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk grep",
        rewrite_prefixes: &["rg", "grep"],
        category: "Files",
        savings_pct: 75.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk ls",
        rewrite_prefixes: &["ls"],
        category: "Files",
        savings_pct: 65.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk find",
        rewrite_prefixes: &["find"],
        category: "Files",
        savings_pct: 70.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        // Longest prefixes first for correct matching
        rtk_cmd: "rtk tsc",
        rewrite_prefixes: &["pnpm tsc", "npx tsc", "tsc"],
        category: "Build",
        savings_pct: 83.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk lint",
        rewrite_prefixes: &[
            "npx eslint",
            "pnpm lint",
            "npx biome",
            "eslint",
            "biome",
            "lint",
        ],
        category: "Build",
        savings_pct: 84.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk prettier",
        rewrite_prefixes: &["npx prettier", "pnpm prettier", "prettier"],
        category: "Build",
        savings_pct: 70.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        // "next build" is stripped to "rtk next" — the build subcommand is internal
        rtk_cmd: "rtk next",
        rewrite_prefixes: &["npx next build", "pnpm next build", "next build"],
        category: "Build",
        savings_pct: 87.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk vitest",
        rewrite_prefixes: &["pnpm vitest", "npx vitest", "vitest", "jest"],
        category: "Tests",
        savings_pct: 99.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk playwright",
        rewrite_prefixes: &["npx playwright", "pnpm playwright", "playwright"],
        category: "Tests",
        savings_pct: 94.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk prisma",
        rewrite_prefixes: &["npx prisma", "pnpm prisma", "prisma"],
        category: "Build",
        savings_pct: 88.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk docker",
        rewrite_prefixes: &["docker"],
        category: "Infra",
        savings_pct: 85.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk kubectl",
        rewrite_prefixes: &["kubectl"],
        category: "Infra",
        savings_pct: 85.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk curl",
        rewrite_prefixes: &["curl"],
        category: "Network",
        savings_pct: 70.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
    RtkRule {
        rtk_cmd: "rtk wget",
        rewrite_prefixes: &["wget"],
        category: "Network",
        savings_pct: 65.0,
        subcmd_savings: &[],
        subcmd_status: &[],
    },
];

/// Commands to ignore (shell builtins, trivial, already rtk).
const IGNORED_PREFIXES: &[&str] = &[
    "cd ",
    "cd\t",
    "echo ",
    "printf ",
    "export ",
    "source ",
    "mkdir ",
    "rm ",
    "mv ",
    "cp ",
    "chmod ",
    "chown ",
    "touch ",
    "which ",
    "type ",
    "command ",
    "test ",
    "true",
    "false",
    "sleep ",
    "wait",
    "kill ",
    "set ",
    "unset ",
    "wc ",
    "sort ",
    "uniq ",
    "tr ",
    "cut ",
    "awk ",
    "sed ",
    "python3 -c",
    "python -c",
    "node -e",
    "ruby -e",
    "rtk ",
    "pwd",
    "bash ",
    "sh ",
    "then\n",
    "then ",
    "else\n",
    "else ",
    "fi",
    "do\n",
    "do ",
    "done",
    "for ",
    "while ",
    "if ",
    "case ",
];

const IGNORED_EXACT: &[&str] = &["cd", "echo", "true", "false", "wait", "pwd", "bash", "sh"];

lazy_static! {
    static ref REGEX_SET: RegexSet = RegexSet::new(PATTERNS).expect("invalid regex patterns");
    static ref COMPILED: Vec<Regex> = PATTERNS
        .iter()
        .map(|p| Regex::new(p).expect("invalid regex"))
        .collect();
    static ref ENV_PREFIX: Regex =
        Regex::new(r"^(?:sudo\s+|env\s+|[A-Z_][A-Z0-9_]*=[^\s]*\s+)+").unwrap();
}

/// Classify a single (already-split) command.
pub fn classify_command(cmd: &str) -> Classification {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return Classification::Ignored;
    }

    // Check ignored
    for exact in IGNORED_EXACT {
        if trimmed == *exact {
            return Classification::Ignored;
        }
    }
    for prefix in IGNORED_PREFIXES {
        if trimmed.starts_with(prefix) {
            return Classification::Ignored;
        }
    }

    // Strip env prefixes (sudo, env VAR=val, VAR=val)
    let stripped = ENV_PREFIX.replace(trimmed, "");
    let cmd_clean = stripped.trim();
    if cmd_clean.is_empty() {
        return Classification::Ignored;
    }

    // Fast check with RegexSet — take the last (most specific) match
    let matches: Vec<usize> = REGEX_SET.matches(cmd_clean).into_iter().collect();
    if let Some(&idx) = matches.last() {
        let rule = &RULES[idx];

        // Extract subcommand for savings override and status detection
        let (savings, status) = if let Some(caps) = COMPILED[idx].captures(cmd_clean) {
            if let Some(sub) = caps.get(1) {
                let subcmd = sub.as_str();
                // Check if this subcommand has a special status
                let status = rule
                    .subcmd_status
                    .iter()
                    .find(|(s, _)| *s == subcmd)
                    .map(|(_, st)| *st)
                    .unwrap_or(super::report::RtkStatus::Existing);

                // Check if this subcommand has custom savings
                let savings = rule
                    .subcmd_savings
                    .iter()
                    .find(|(s, _)| *s == subcmd)
                    .map(|(_, pct)| *pct)
                    .unwrap_or(rule.savings_pct);

                (savings, status)
            } else {
                (rule.savings_pct, super::report::RtkStatus::Existing)
            }
        } else {
            (rule.savings_pct, super::report::RtkStatus::Existing)
        };

        Classification::Supported {
            rtk_equivalent: rule.rtk_cmd,
            category: rule.category,
            estimated_savings_pct: savings,
            status,
        }
    } else {
        // Extract base command for unsupported
        let base = extract_base_command(cmd_clean);
        if base.is_empty() {
            Classification::Ignored
        } else {
            Classification::Unsupported {
                base_command: base.to_string(),
            }
        }
    }
}

/// Extract the base command (first word, or first two if it looks like a subcommand pattern).
fn extract_base_command(cmd: &str) -> &str {
    let parts: Vec<&str> = cmd.splitn(3, char::is_whitespace).collect();
    match parts.len() {
        0 => "",
        1 => parts[0],
        _ => {
            let second = parts[1];
            // If the second token looks like a subcommand (no leading -)
            if !second.starts_with('-') && !second.contains('/') && !second.contains('.') {
                // Return "cmd subcmd"
                let end = cmd
                    .find(char::is_whitespace)
                    .and_then(|i| {
                        let rest = &cmd[i..];
                        let trimmed = rest.trim_start();
                        trimmed
                            .find(char::is_whitespace)
                            .map(|j| i + (rest.len() - trimmed.len()) + j)
                    })
                    .unwrap_or(cmd.len());
                &cmd[..end]
            } else {
                parts[0]
            }
        }
    }
}

/// Split a command chain on `&&`, `||`, `;` outside quotes.
/// For pipes `|`, only keep the first command.
/// Lines with `<<` (heredoc) or `$((` are returned whole.
pub fn split_command_chain(cmd: &str) -> Vec<&str> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    // Heredoc or arithmetic expansion: treat as single command
    if trimmed.contains("<<") || trimmed.contains("$((") {
        return vec![trimmed];
    }

    let mut results = Vec::new();
    let mut start = 0;
    let bytes = trimmed.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut pipe_seen = false;

    while i < len {
        let b = bytes[i];
        match b {
            b'\'' if !in_double => {
                in_single = !in_single;
                i += 1;
            }
            b'"' if !in_single => {
                in_double = !in_double;
                i += 1;
            }
            b'|' if !in_single && !in_double => {
                if i + 1 < len && bytes[i + 1] == b'|' {
                    // ||
                    let segment = trimmed[start..i].trim();
                    if !segment.is_empty() {
                        results.push(segment);
                    }
                    i += 2;
                    start = i;
                } else {
                    // pipe: keep only first command
                    let segment = trimmed[start..i].trim();
                    if !segment.is_empty() {
                        results.push(segment);
                    }
                    pipe_seen = true;
                    break;
                }
            }
            b'&' if !in_single && !in_double && i + 1 < len && bytes[i + 1] == b'&' => {
                let segment = trimmed[start..i].trim();
                if !segment.is_empty() {
                    results.push(segment);
                }
                i += 2;
                start = i;
            }
            b';' if !in_single && !in_double => {
                let segment = trimmed[start..i].trim();
                if !segment.is_empty() {
                    results.push(segment);
                }
                i += 1;
                start = i;
            }
            _ => {
                i += 1;
            }
        }
    }

    if !pipe_seen && start < len {
        let segment = trimmed[start..].trim();
        if !segment.is_empty() {
            results.push(segment);
        }
    }

    results
}

/// Rewrite a raw command to its RTK equivalent.
///
/// Returns `Some(rewritten)` if the command has an RTK equivalent or is already RTK.
/// Returns `None` if the command is unsupported or ignored (hook should pass through).
///
/// Handles compound commands (`&&`, `||`, `;`) by rewriting each segment independently.
/// For pipes (`|`), only rewrites the first command (the filter stays raw).
pub fn rewrite_command(cmd: &str) -> Option<String> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Heredoc or arithmetic expansion — unsafe to split/rewrite
    if trimmed.contains("<<") || trimmed.contains("$((") {
        return None;
    }

    // Simple (non-compound) already-RTK command — return as-is.
    // For compound commands that start with "rtk" (e.g. "rtk git add . && cargo test"),
    // fall through to rewrite_compound so the remaining segments get rewritten.
    let has_compound = trimmed.contains("&&")
        || trimmed.contains("||")
        || trimmed.contains(';')
        || trimmed.contains('|');
    if !has_compound && (trimmed.starts_with("rtk ") || trimmed == "rtk") {
        return Some(trimmed.to_string());
    }

    rewrite_compound(trimmed)
}

/// Rewrite a compound command (with `&&`, `||`, `;`, `|`) by rewriting each segment.
fn rewrite_compound(cmd: &str) -> Option<String> {
    let bytes = cmd.as_bytes();
    let len = bytes.len();
    let mut result = String::with_capacity(len + 32);
    let mut any_changed = false;
    let mut seg_start = 0;
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;

    while i < len {
        let b = bytes[i];
        match b {
            b'\'' if !in_double => {
                in_single = !in_single;
                i += 1;
            }
            b'"' if !in_single => {
                in_double = !in_double;
                i += 1;
            }
            b'|' if !in_single && !in_double => {
                if i + 1 < len && bytes[i + 1] == b'|' {
                    // `||` operator — rewrite left, continue
                    let seg = cmd[seg_start..i].trim();
                    let rewritten = rewrite_segment(seg).unwrap_or_else(|| seg.to_string());
                    if rewritten != seg {
                        any_changed = true;
                    }
                    result.push_str(&rewritten);
                    result.push_str(" || ");
                    i += 2;
                    while i < len && bytes[i] == b' ' {
                        i += 1;
                    }
                    seg_start = i;
                } else {
                    // `|` pipe — rewrite first segment only, pass through the rest unchanged
                    let seg = cmd[seg_start..i].trim();
                    let rewritten = rewrite_segment(seg).unwrap_or_else(|| seg.to_string());
                    if rewritten != seg {
                        any_changed = true;
                    }
                    result.push_str(&rewritten);
                    // Preserve the space before the pipe that was lost by trim()
                    result.push(' ');
                    result.push_str(cmd[i..].trim_start());
                    return if any_changed { Some(result) } else { None };
                }
            }
            b'&' if !in_single && !in_double && i + 1 < len && bytes[i + 1] == b'&' => {
                // `&&` operator — rewrite left, continue
                let seg = cmd[seg_start..i].trim();
                let rewritten = rewrite_segment(seg).unwrap_or_else(|| seg.to_string());
                if rewritten != seg {
                    any_changed = true;
                }
                result.push_str(&rewritten);
                result.push_str(" && ");
                i += 2;
                while i < len && bytes[i] == b' ' {
                    i += 1;
                }
                seg_start = i;
            }
            b';' if !in_single && !in_double => {
                // `;` separator
                let seg = cmd[seg_start..i].trim();
                let rewritten = rewrite_segment(seg).unwrap_or_else(|| seg.to_string());
                if rewritten != seg {
                    any_changed = true;
                }
                result.push_str(&rewritten);
                result.push(';');
                i += 1;
                while i < len && bytes[i] == b' ' {
                    i += 1;
                }
                if i < len {
                    result.push(' ');
                }
                seg_start = i;
            }
            _ => {
                i += 1;
            }
        }
    }

    // Last (or only) segment
    let seg = cmd[seg_start..len].trim();
    let rewritten = rewrite_segment(seg).unwrap_or_else(|| seg.to_string());
    if rewritten != seg {
        any_changed = true;
    }
    result.push_str(&rewritten);

    if any_changed {
        Some(result)
    } else {
        None
    }
}

/// Rewrite a single (non-compound) command segment.
/// Returns `Some(rewritten)` if matched (including already-RTK pass-through).
/// Returns `None` if no match (caller uses original segment).
fn rewrite_segment(seg: &str) -> Option<String> {
    let trimmed = seg.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Already RTK — pass through unchanged
    if trimmed.starts_with("rtk ") || trimmed == "rtk" {
        return Some(trimmed.to_string());
    }

    // Use classify_command for correct ignore/prefix handling
    let rtk_equivalent = match classify_command(trimmed) {
        Classification::Supported { rtk_equivalent, .. } => rtk_equivalent,
        _ => return None,
    };

    // Find the matching rule (rtk_cmd values are unique across all rules)
    let rule = RULES.iter().find(|r| r.rtk_cmd == rtk_equivalent)?;

    // Extract env prefix (sudo, env VAR=val, etc.)
    let stripped_cow = ENV_PREFIX.replace(trimmed, "");
    let env_prefix_len = trimmed.len() - stripped_cow.len();
    let env_prefix = &trimmed[..env_prefix_len];
    let cmd_clean = stripped_cow.trim();

    // Try each rewrite prefix (longest first) with word-boundary check
    for &prefix in rule.rewrite_prefixes {
        if let Some(rest) = strip_word_prefix(cmd_clean, prefix) {
            let rewritten = if rest.is_empty() {
                format!("{}{}", env_prefix, rule.rtk_cmd)
            } else {
                format!("{}{} {}", env_prefix, rule.rtk_cmd, rest)
            };
            return Some(rewritten);
        }
    }

    None
}

/// Strip a command prefix with word-boundary check.
/// Returns the remainder of the command after the prefix, or `None` if no match.
fn strip_word_prefix<'a>(cmd: &'a str, prefix: &str) -> Option<&'a str> {
    if cmd == prefix {
        Some("")
    } else if cmd.len() > prefix.len()
        && cmd.starts_with(prefix)
        && cmd.as_bytes()[prefix.len()] == b' '
    {
        Some(cmd[prefix.len() + 1..].trim_start())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::report::RtkStatus;
    use super::*;

    #[test]
    fn test_classify_git_status() {
        assert_eq!(
            classify_command("git status"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_git_diff_cached() {
        assert_eq!(
            classify_command("git diff --cached"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cargo_test_filter() {
        assert_eq!(
            classify_command("cargo test filter::"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                category: "Cargo",
                estimated_savings_pct: 90.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_npx_tsc() {
        assert_eq!(
            classify_command("npx tsc --noEmit"),
            Classification::Supported {
                rtk_equivalent: "rtk tsc",
                category: "Build",
                estimated_savings_pct: 83.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cat_file() {
        assert_eq!(
            classify_command("cat src/main.rs"),
            Classification::Supported {
                rtk_equivalent: "rtk read",
                category: "Files",
                estimated_savings_pct: 60.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cd_ignored() {
        assert_eq!(classify_command("cd /tmp"), Classification::Ignored);
    }

    #[test]
    fn test_classify_rtk_already() {
        assert_eq!(classify_command("rtk git status"), Classification::Ignored);
    }

    #[test]
    fn test_classify_echo_ignored() {
        assert_eq!(
            classify_command("echo hello world"),
            Classification::Ignored
        );
    }

    #[test]
    fn test_classify_terraform_unsupported() {
        match classify_command("terraform plan -var-file=prod.tfvars") {
            Classification::Unsupported { base_command } => {
                assert_eq!(base_command, "terraform plan");
            }
            other => panic!("expected Unsupported, got {:?}", other),
        }
    }

    #[test]
    fn test_classify_env_prefix_stripped() {
        assert_eq!(
            classify_command("GIT_SSH_COMMAND=ssh git push"),
            Classification::Supported {
                rtk_equivalent: "rtk git",
                category: "Git",
                estimated_savings_pct: 70.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_sudo_stripped() {
        assert_eq!(
            classify_command("sudo docker ps"),
            Classification::Supported {
                rtk_equivalent: "rtk docker",
                category: "Infra",
                estimated_savings_pct: 85.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cargo_check() {
        assert_eq!(
            classify_command("cargo check"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                category: "Cargo",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cargo_check_all_targets() {
        assert_eq!(
            classify_command("cargo check --all-targets"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                category: "Cargo",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_classify_cargo_fmt_passthrough() {
        assert_eq!(
            classify_command("cargo fmt"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                category: "Cargo",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Passthrough,
            }
        );
    }

    #[test]
    fn test_classify_cargo_clippy_savings() {
        assert_eq!(
            classify_command("cargo clippy --all-targets"),
            Classification::Supported {
                rtk_equivalent: "rtk cargo",
                category: "Cargo",
                estimated_savings_pct: 80.0,
                status: RtkStatus::Existing,
            }
        );
    }

    #[test]
    fn test_patterns_rules_length_match() {
        assert_eq!(
            PATTERNS.len(),
            RULES.len(),
            "PATTERNS and RULES must be aligned"
        );
    }

    #[test]
    fn test_registry_covers_all_cargo_subcommands() {
        // Verify that every CargoCommand variant (Build, Test, Clippy, Check, Fmt)
        // except Other has a matching pattern in the registry
        for subcmd in ["build", "test", "clippy", "check", "fmt"] {
            let cmd = format!("cargo {subcmd}");
            match classify_command(&cmd) {
                Classification::Supported { .. } => {}
                other => panic!("cargo {subcmd} should be Supported, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_registry_covers_all_git_subcommands() {
        // Verify that every GitCommand subcommand has a matching pattern
        for subcmd in [
            "status", "log", "diff", "show", "add", "commit", "push", "pull", "branch", "fetch",
            "stash", "worktree",
        ] {
            let cmd = format!("git {subcmd}");
            match classify_command(&cmd) {
                Classification::Supported { .. } => {}
                other => panic!("git {subcmd} should be Supported, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_split_chain_and() {
        assert_eq!(split_command_chain("a && b"), vec!["a", "b"]);
    }

    #[test]
    fn test_split_chain_semicolon() {
        assert_eq!(split_command_chain("a ; b"), vec!["a", "b"]);
    }

    #[test]
    fn test_split_pipe_first_only() {
        assert_eq!(split_command_chain("a | b"), vec!["a"]);
    }

    #[test]
    fn test_split_single() {
        assert_eq!(split_command_chain("git status"), vec!["git status"]);
    }

    #[test]
    fn test_split_quoted_and() {
        assert_eq!(
            split_command_chain(r#"echo "a && b""#),
            vec![r#"echo "a && b""#]
        );
    }

    #[test]
    fn test_split_heredoc_no_split() {
        let cmd = "cat <<'EOF'\nhello && world\nEOF";
        assert_eq!(split_command_chain(cmd), vec![cmd]);
    }

    // --- rewrite_command tests ---

    #[test]
    fn test_rewrite_git_status() {
        assert_eq!(rewrite_command("git status"), Some("rtk git status".into()));
    }

    #[test]
    fn test_rewrite_git_log() {
        assert_eq!(
            rewrite_command("git log -10"),
            Some("rtk git log -10".into())
        );
    }

    #[test]
    fn test_rewrite_cargo_test() {
        assert_eq!(rewrite_command("cargo test"), Some("rtk cargo test".into()));
    }

    #[test]
    fn test_rewrite_compound_and() {
        assert_eq!(
            rewrite_command("git add . && cargo test"),
            Some("rtk git add . && rtk cargo test".into())
        );
    }

    #[test]
    fn test_rewrite_compound_three_segments() {
        assert_eq!(
            rewrite_command("cargo fmt --all && cargo clippy --all-targets && cargo test"),
            Some("rtk cargo fmt --all && rtk cargo clippy --all-targets && rtk cargo test".into())
        );
    }

    #[test]
    fn test_rewrite_already_rtk() {
        assert_eq!(
            rewrite_command("rtk git status"),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_rewrite_unsupported_returns_none() {
        assert_eq!(rewrite_command("terraform plan"), None);
    }

    #[test]
    fn test_rewrite_ignored_cd() {
        assert_eq!(rewrite_command("cd /tmp"), None);
    }

    #[test]
    fn test_rewrite_with_env_prefix() {
        assert_eq!(
            rewrite_command("GIT_SSH_COMMAND=ssh git push"),
            Some("GIT_SSH_COMMAND=ssh rtk git push".into())
        );
    }

    #[test]
    fn test_rewrite_npx_tsc() {
        assert_eq!(
            rewrite_command("npx tsc --noEmit"),
            Some("rtk tsc --noEmit".into())
        );
    }

    #[test]
    fn test_rewrite_pnpm_tsc() {
        assert_eq!(
            rewrite_command("pnpm tsc --noEmit"),
            Some("rtk tsc --noEmit".into())
        );
    }

    #[test]
    fn test_rewrite_cat_file() {
        assert_eq!(
            rewrite_command("cat src/main.rs"),
            Some("rtk read src/main.rs".into())
        );
    }

    #[test]
    fn test_rewrite_rg_pattern() {
        assert_eq!(
            rewrite_command("rg \"fn main\""),
            Some("rtk grep \"fn main\"".into())
        );
    }

    #[test]
    fn test_rewrite_npx_playwright() {
        assert_eq!(
            rewrite_command("npx playwright test"),
            Some("rtk playwright test".into())
        );
    }

    #[test]
    fn test_rewrite_next_build() {
        assert_eq!(
            rewrite_command("next build --turbo"),
            Some("rtk next --turbo".into())
        );
    }

    #[test]
    fn test_rewrite_pipe_first_only() {
        // After a pipe, the filter command stays raw
        assert_eq!(
            rewrite_command("git log -10 | grep feat"),
            Some("rtk git log -10 | grep feat".into())
        );
    }

    #[test]
    fn test_rewrite_heredoc_returns_none() {
        assert_eq!(rewrite_command("cat <<'EOF'\nfoo\nEOF"), None);
    }

    #[test]
    fn test_rewrite_empty_returns_none() {
        assert_eq!(rewrite_command(""), None);
        assert_eq!(rewrite_command("   "), None);
    }

    #[test]
    fn test_rewrite_mixed_compound_partial() {
        // First segment already RTK, second gets rewritten
        assert_eq!(
            rewrite_command("rtk git add . && cargo test"),
            Some("rtk git add . && rtk cargo test".into())
        );
    }
}
