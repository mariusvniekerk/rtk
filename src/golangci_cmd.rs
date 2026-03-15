use crate::tracking;
use crate::utils::{resolved_command, truncate};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct Position {
    #[serde(rename = "Filename")]
    filename: String,
    #[serde(rename = "Line")]
    line: usize,
    #[serde(rename = "Column")]
    column: usize,
}

#[derive(Debug, Deserialize)]
struct Issue {
    #[serde(rename = "FromLinter")]
    from_linter: String,
    #[serde(rename = "Text")]
    text: String,
    #[serde(rename = "Pos")]
    pos: Position,
}

#[derive(Debug, Deserialize)]
struct GolangciOutput {
    #[serde(rename = "Issues")]
    issues: Vec<Issue>,
}

/// Known golangci-lint subcommands that are NOT "run"
/// v1: cache, completion, config, custom, help, linters, version
/// v2 adds: fmt, formatters, migrate
const NON_RUN_SUBCOMMANDS: &[&str] = &[
    "cache",
    "completion",
    "config",
    "custom",
    "fmt",
    "formatters",
    "help",
    "linters",
    "migrate",
    "version",
];

/// Determine if the args represent a "run" invocation.
/// Returns true if: no args, explicit "run", or first arg is a flag/path (implicit run).
fn is_run_subcommand(args: &[String]) -> bool {
    match args.first() {
        None => true, // no args = implicit "run"
        Some(first) => {
            if first == "run" {
                return true;
            }
            // If first arg is a known non-run subcommand, it's not a run
            if NON_RUN_SUBCOMMANDS.contains(&first.as_str()) {
                return false;
            }
            // Flags (--fix, -v) or paths (./...) imply "run"
            first.starts_with('-') || first.starts_with('.') || first.starts_with('/')
        }
    }
}

/// Detect golangci-lint major version by running `golangci-lint version`.
/// Parses "golangci-lint has version X.Y.Z ..." output (works for both v1 and v2).
/// Returns 1 or 2. Defaults to 2 (current mainstream) on failure.
fn detect_major_version_from_binary() -> u8 {
    let output = Command::new("golangci-lint").arg("version").output();

    match output {
        Ok(o) => {
            let out = String::from_utf8_lossy(&o.stdout);
            detect_major_version(&out)
        }
        Err(_) => 2,
    }
}

/// Parse major version from version output.
/// Handles both "2.11.3" and "golangci-lint has version 2.11.3 built with ...".
fn detect_major_version(output: &str) -> u8 {
    // Try to find "version X.Y.Z" pattern first (long format)
    let version_str = if let Some(pos) = output.find("version ") {
        output[pos + 8..].split_whitespace().next().unwrap_or("")
    } else {
        output.trim()
    };
    version_str
        .split('.')
        .next()
        .and_then(|v| v.parse::<u8>().ok())
        .unwrap_or(2)
}

/// Check if the user already specified an output format flag (v1 or v2 style).
fn has_user_format_flag(args: &[String]) -> bool {
    args.iter().any(|a| {
        // v1: --out-format or --out-format=...
        a == "--out-format" || a.starts_with("--out-format=") ||
        // v2: --output.json.path, --output.text.path, etc.
        a.starts_with("--output.")
    })
}

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let use_json = is_run_subcommand(args);

    let mut cmd = resolved_command("golangci-lint");
    let mut json_tmp_path: Option<std::path::PathBuf> = None;

    if use_json {
        let has_format = has_user_format_flag(args);
        let run_args: Vec<&String> = args.iter().filter(|a| a.as_str() != "run").collect();

        cmd.arg("run");

        if !has_format {
            let major = detect_major_version_from_binary();

            match major {
                1 => {
                    cmd.arg("--out-format=json");
                    if verbose > 0 {
                        eprintln!("Running: golangci-lint run --out-format=json (v1)");
                    }
                }
                _ => {
                    // v2: write JSON to temp file to avoid stdout mixing with text summary
                    let tmp = std::env::temp_dir()
                        .join(format!("rtk_golangci_{}.json", std::process::id()));
                    let path_str = tmp.to_string_lossy().to_string();
                    cmd.arg(format!("--output.json.path={}", path_str));
                    if verbose > 0 {
                        eprintln!(
                            "Running: golangci-lint run --output.json.path={} (v2)",
                            path_str
                        );
                    }
                    json_tmp_path = Some(tmp);
                }
            }
        } else if verbose > 0 {
            eprintln!("Running: golangci-lint run (user format flag detected)");
        }

        for arg in &run_args {
            cmd.arg(arg);
        }
    } else {
        // Non-run subcommand — pass through as-is
        for arg in args {
            cmd.arg(arg);
        }

        if verbose > 0 {
            eprintln!("Running: golangci-lint {}", args.join(" "));
        }
    }

    let output = cmd.output().context(
        "Failed to run golangci-lint. Is it installed? Try: go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest",
    )?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = if use_json {
        // If we wrote JSON to a temp file (v2), read from there; otherwise parse stdout (v1)
        let json_content = if let Some(ref tmp) = json_tmp_path {
            let content = std::fs::read_to_string(tmp).unwrap_or_default();
            let _ = std::fs::remove_file(tmp);
            content
        } else {
            stdout.to_string()
        };
        filter_golangci_json(&json_content)
    } else {
        stdout.trim().to_string()
    };

    println!("{}", filtered);

    if !stderr.trim().is_empty() && verbose > 0 {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("golangci-lint {}", args.join(" ")),
        &format!("rtk golangci-lint {}", args.join(" ")),
        &raw,
        &filtered,
    );

    // golangci-lint: exit 0 = clean, exit 1 = lint issues, exit 2+ = config/build error
    // None = killed by signal (OOM, SIGKILL) — always fatal
    match output.status.code() {
        Some(0) | Some(1) => Ok(()),
        Some(code) => {
            if !stderr.trim().is_empty() {
                eprintln!("{}", stderr.trim());
            }
            std::process::exit(code);
        }
        None => {
            eprintln!("golangci-lint: killed by signal");
            std::process::exit(130);
        }
    }
}

/// Filter golangci-lint JSON output - group by linter and file
fn filter_golangci_json(output: &str) -> String {
    let trimmed = output.trim();

    // Empty output means clean run (no issues, no JSON emitted)
    if trimmed.is_empty() {
        return "✓ golangci-lint: No issues found".to_string();
    }

    let result: Result<GolangciOutput, _> = serde_json::from_str(trimmed);

    let golangci_output = match result {
        Ok(o) => o,
        Err(_) => {
            // Non-JSON output — just pass through (don't scare users with parse errors)
            return truncate(trimmed, 2000);
        }
    };

    let issues = golangci_output.issues;

    if issues.is_empty() {
        return "✓ golangci-lint: No issues found".to_string();
    }

    let total_issues = issues.len();

    // Count unique files
    let unique_files: std::collections::HashSet<_> =
        issues.iter().map(|i| &i.pos.filename).collect();
    let total_files = unique_files.len();

    // Group by linter
    let mut by_linter: HashMap<String, usize> = HashMap::new();
    for issue in &issues {
        *by_linter.entry(issue.from_linter.clone()).or_insert(0) += 1;
    }

    // Group by file
    let mut by_file: HashMap<&str, usize> = HashMap::new();
    for issue in &issues {
        *by_file.entry(&issue.pos.filename).or_insert(0) += 1;
    }

    let mut file_counts: Vec<_> = by_file.iter().collect();
    file_counts.sort_by(|a, b| b.1.cmp(a.1));

    // Build output
    let mut result = String::new();
    result.push_str(&format!(
        "golangci-lint: {} issues in {} files\n",
        total_issues, total_files
    ));
    result.push_str("═══════════════════════════════════════\n");

    // Show top linters
    let mut linter_counts: Vec<_> = by_linter.iter().collect();
    linter_counts.sort_by(|a, b| b.1.cmp(a.1));

    if !linter_counts.is_empty() {
        result.push_str("Top linters:\n");
        for (linter, count) in linter_counts.iter().take(10) {
            result.push_str(&format!("  {} ({}x)\n", linter, count));
        }
        result.push('\n');
    }

    // Show top files
    result.push_str("Top files:\n");
    for (file, count) in file_counts.iter().take(10) {
        let short_path = compact_path(file);
        result.push_str(&format!("  {} ({} issues)\n", short_path, count));

        // Show top 3 linters in this file
        let mut file_linters: HashMap<String, usize> = HashMap::new();
        for issue in issues.iter().filter(|i| &i.pos.filename == *file) {
            *file_linters.entry(issue.from_linter.clone()).or_insert(0) += 1;
        }

        let mut file_linter_counts: Vec<_> = file_linters.iter().collect();
        file_linter_counts.sort_by(|a, b| b.1.cmp(a.1));

        for (linter, count) in file_linter_counts.iter().take(3) {
            result.push_str(&format!("    {} ({})\n", linter, count));
        }
    }

    if file_counts.len() > 10 {
        result.push_str(&format!("\n... +{} more files\n", file_counts.len() - 10));
    }

    result.trim().to_string()
}

/// Compact file path (remove common prefixes)
fn compact_path(path: &str) -> String {
    let path = path.replace('\\', "/");

    if let Some(pos) = path.rfind("/pkg/") {
        format!("pkg/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/cmd/") {
        format!("cmd/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/internal/") {
        format!("internal/{}", &path[pos + 10..])
    } else if let Some(pos) = path.rfind('/') {
        path[pos + 1..].to_string()
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Subcommand detection tests ---

    #[test]
    fn test_is_run_subcommand_explicit_run() {
        let args: Vec<String> = vec!["run".into(), "./...".into()];
        assert!(is_run_subcommand(&args));
    }

    #[test]
    fn test_is_run_subcommand_no_args_implies_run() {
        let args: Vec<String> = vec![];
        assert!(is_run_subcommand(&args));
    }

    #[test]
    fn test_is_run_subcommand_flags_only_implies_run() {
        let args: Vec<String> = vec!["--fix".into(), "--fast".into()];
        assert!(is_run_subcommand(&args));
    }

    #[test]
    fn test_is_run_subcommand_path_only_implies_run() {
        let args: Vec<String> = vec!["./...".into()];
        assert!(is_run_subcommand(&args));
    }

    #[test]
    fn test_is_run_subcommand_version() {
        let args: Vec<String> = vec!["version".into()];
        assert!(!is_run_subcommand(&args));
    }

    #[test]
    fn test_is_run_subcommand_linters() {
        let args: Vec<String> = vec!["linters".into()];
        assert!(!is_run_subcommand(&args));
    }

    #[test]
    fn test_is_run_subcommand_cache_clean() {
        let args: Vec<String> = vec!["cache".into(), "clean".into()];
        assert!(!is_run_subcommand(&args));
    }

    #[test]
    fn test_is_run_subcommand_help() {
        let args: Vec<String> = vec!["help".into()];
        assert!(!is_run_subcommand(&args));
    }

    #[test]
    fn test_is_run_subcommand_config() {
        let args: Vec<String> = vec!["config".into()];
        assert!(!is_run_subcommand(&args));
    }

    // v1+v2: custom
    #[test]
    fn test_is_run_subcommand_custom() {
        let args: Vec<String> = vec!["custom".into()];
        assert!(!is_run_subcommand(&args));
    }

    // v2-only subcommands
    #[test]
    fn test_is_run_subcommand_fmt() {
        let args: Vec<String> = vec!["fmt".into()];
        assert!(!is_run_subcommand(&args));
    }

    #[test]
    fn test_is_run_subcommand_formatters() {
        let args: Vec<String> = vec!["formatters".into()];
        assert!(!is_run_subcommand(&args));
    }

    #[test]
    fn test_is_run_subcommand_migrate() {
        let args: Vec<String> = vec!["migrate".into()];
        assert!(!is_run_subcommand(&args));
    }

    // --- Version detection tests ---

    #[test]
    fn test_detect_major_version_v1_short() {
        assert_eq!(detect_major_version("1.62.2"), 1);
    }

    #[test]
    fn test_detect_major_version_v2_short() {
        assert_eq!(detect_major_version("2.11.3"), 2);
    }

    #[test]
    fn test_detect_major_version_v1_long() {
        assert_eq!(
            detect_major_version(
                "golangci-lint has version 1.62.2 built with go1.23.3 from 89476e7a on 2024-11-25T14:16:01Z"
            ),
            1
        );
    }

    #[test]
    fn test_detect_major_version_v2_long() {
        assert_eq!(
            detect_major_version(
                "golangci-lint has version 2.11.3 built with go1.26.1 from 6008b81b on 2026-03-10T10:25:44Z"
            ),
            2
        );
    }

    #[test]
    fn test_detect_major_version_unknown_defaults_to_2() {
        assert_eq!(detect_major_version(""), 2);
        assert_eq!(detect_major_version("garbage"), 2);
    }

    #[test]
    fn test_has_user_format_flag_v1() {
        let args = vec!["--out-format=text".to_string()];
        assert!(has_user_format_flag(&args));
    }

    #[test]
    fn test_has_user_format_flag_v2_output_dot() {
        let args = vec!["--output.text.path=stdout".to_string()];
        assert!(has_user_format_flag(&args));
    }

    #[test]
    fn test_has_user_format_flag_none() {
        let args = vec!["--fix".to_string(), "./...".to_string()];
        assert!(!has_user_format_flag(&args));
    }

    // --- JSON filter tests ---

    #[test]
    fn test_filter_golangci_json_empty_string() {
        let result = filter_golangci_json("");
        assert!(result.contains("✓ golangci-lint"));
        assert!(result.contains("No issues found"));
    }

    #[test]
    fn test_filter_golangci_json_non_json_fallback() {
        let result = filter_golangci_json("golangci-lint has version 1.55.0");
        assert!(!result.contains("JSON parse failed"));
    }

    #[test]
    fn test_filter_golangci_no_issues() {
        let output = r#"{"Issues":[]}"#;
        let result = filter_golangci_json(output);
        assert!(result.contains("✓ golangci-lint"));
        assert!(result.contains("No issues found"));
    }

    #[test]
    fn test_filter_golangci_with_issues() {
        let output = r#"{
  "Issues": [
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Pos": {"Filename": "main.go", "Line": 42, "Column": 5}
    },
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Pos": {"Filename": "main.go", "Line": 50, "Column": 10}
    },
    {
      "FromLinter": "gosimple",
      "Text": "Should use strings.Contains",
      "Pos": {"Filename": "utils.go", "Line": 15, "Column": 2}
    }
  ]
}"#;

        let result = filter_golangci_json(output);
        assert!(result.contains("3 issues"));
        assert!(result.contains("2 files"));
        assert!(result.contains("errcheck"));
        assert!(result.contains("gosimple"));
        assert!(result.contains("main.go"));
        assert!(result.contains("utils.go"));
    }

    #[test]
    fn test_filter_golangci_with_v2_extra_fields() {
        // v2 JSON includes extra fields (Severity, SourceLines, Report) — serde ignores them
        let output = r#"{"Issues":[{"FromLinter":"errcheck","Text":"Error return value not checked","Severity":"","SourceLines":[],"Pos":{"Filename":"main.go","Offset":0,"Line":42,"Column":5},"ExpectNoLint":false,"ExpectedNoLintLinter":""}],"Report":{"Linters":[]}}"#;
        let result = filter_golangci_json(output);
        assert!(result.contains("1 issues"));
        assert!(result.contains("errcheck"));
    }

    // --- Real fixture tests (captured from golangci-lint v1.62.2 and v2.11.3) ---

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_real_v1_json() {
        let input = include_str!("../tests/fixtures/golangci_v1_json.txt");
        let result = filter_golangci_json(input);
        assert!(
            result.contains("2 issues"),
            "v1: expected 2 issues, got: {}",
            result
        );
        assert!(result.contains("errcheck"));
        assert!(result.contains("ineffassign"));
        // Filtered output must be shorter than raw JSON
        assert!(
            count_tokens(&result) < count_tokens(input),
            "v1: filtered ({}) should be shorter than raw ({})",
            count_tokens(&result),
            count_tokens(input)
        );
    }

    #[test]
    fn test_filter_real_v2_json() {
        let input = include_str!("../tests/fixtures/golangci_v2_json.txt");
        let result = filter_golangci_json(input);
        assert!(
            result.contains("2 issues"),
            "v2: expected 2 issues, got: {}",
            result
        );
        assert!(result.contains("errcheck"));
        assert!(result.contains("ineffassign"));
        // Filtered output must be shorter than raw JSON
        assert!(
            count_tokens(&result) < count_tokens(input),
            "v2: filtered ({}) should be shorter than raw ({})",
            count_tokens(&result),
            count_tokens(input)
        );
    }

    // --- Integration tests (run with: cargo test --ignored) ---

    /// Set up a temp Go project with intentional lint issues, return its path.
    fn setup_go_testapp(suffix: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rtk_golangci_test_{}_{}",
            std::process::id(),
            suffix
        ));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join("go.mod"),
            include_str!("../tests/fixtures/golangci_testapp_go.mod"),
        )
        .expect("write go.mod");
        std::fs::write(
            dir.join("main.go"),
            include_str!("../tests/fixtures/golangci_testapp.go"),
        )
        .expect("write main.go");
        dir
    }

    fn cleanup_testapp(dir: &std::path::Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[ignore]
    fn test_integration_v1_json_output() {
        // Set GOLANGCI_V1_BIN to point to a v1 binary (e.g. golangci-lint 1.x)
        let v1 = match std::env::var("GOLANGCI_V1_BIN") {
            Ok(p) if !p.is_empty() => std::path::PathBuf::from(p),
            _ => {
                eprintln!("Skipping: set GOLANGCI_V1_BIN to a golangci-lint v1.x binary");
                return;
            }
        };
        if !v1.exists() {
            eprintln!("Skipping: golangci-lint v1 not found at {:?}", v1);
            return;
        }

        let dir = setup_go_testapp("v1");
        let output = Command::new(&v1)
            .args(["run", "--out-format=json"])
            .current_dir(&dir)
            .output()
            .expect("run v1");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let result = filter_golangci_json(&stdout);
        assert!(
            result.contains("issues"),
            "v1 integration: expected issues in output, got: {}",
            result
        );
        assert!(!result.contains("JSON parse failed"));

        cleanup_testapp(&dir);
    }

    #[test]
    #[ignore]
    fn test_integration_v2_json_file() {
        // Set GOLANGCI_V2_BIN or falls back to `golangci-lint` on PATH
        let v2 = match std::env::var("GOLANGCI_V2_BIN") {
            Ok(p) if !p.is_empty() => std::path::PathBuf::from(p),
            _ => std::path::PathBuf::from("golangci-lint"),
        };

        let dir = setup_go_testapp("v2");
        let json_path = dir.join("output.json");
        let output = Command::new(v2.as_os_str())
            .args([
                "run",
                &format!("--output.json.path={}", json_path.to_string_lossy()),
            ])
            .current_dir(&dir)
            .output()
            .unwrap_or_else(|e| panic!("run v2 at {:?} in {:?}: {}", v2, dir, e));

        // v2 writes JSON to file, text summary to stdout
        assert!(
            json_path.exists(),
            "v2 should write JSON file, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json_content = std::fs::read_to_string(&json_path).expect("read json file");
        let result = filter_golangci_json(&json_content);
        assert!(
            result.contains("issues"),
            "v2 integration: expected issues in output, got: {}",
            result
        );
        assert!(!result.contains("JSON parse failed"));

        cleanup_testapp(&dir);
    }

    #[test]
    fn test_compact_path() {
        assert_eq!(
            compact_path("/Users/foo/project/pkg/handler/server.go"),
            "pkg/handler/server.go"
        );
        assert_eq!(
            compact_path("/home/user/app/cmd/main/main.go"),
            "cmd/main/main.go"
        );
        assert_eq!(
            compact_path("/project/internal/config/loader.go"),
            "internal/config/loader.go"
        );
        assert_eq!(compact_path("relative/file.go"), "file.go");
    }
}
