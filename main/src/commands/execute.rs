/*
 * Copyright (C) 2022-2026 rhctl Contributors
 *
 * SPDX-License-Identifier: Apache-2.0
 * **************************************************************************
 * Execute a bash file or command on a remote server.
 *
 * Since: 1.0.0
 * Date: October 16, 2025
 */
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use regex::Regex;

use crate::common::ssh_pool::ServerPool;
use crate::domain::cmd_params::{ExecuteCmdConfig, ScriptInvocation, ServerMetadata};
use crate::domain::constants::{DANGEROUS_PATHS, EXECUTE_TASK_NAME};
use crate::{log_debug, log_info, log_warn};
use futures::future::join_all;

/// Single-quote escape for remote bash (safe inside `sudo bash -c '…'`).
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn build_remote_script_command(work_path: &str, remote_script: &str, args: &[String]) -> String {
    let mut cmd = format!("cd {} && bash {}", work_path, shell_quote(remote_script));
    for arg in args {
        cmd.push(' ');
        cmd.push_str(&shell_quote(arg));
    }
    cmd
}

fn build_remote_cmd_command(work_path: &str, cmd: &str) -> String {
    format!("cd {} && {}", work_path, cmd)
}

/// Outcome of a local `bash -n` syntax check.
#[derive(Debug, PartialEq, Eq)]
pub enum ScriptSyntaxStatus {
    /// `bash -n` reported a syntax error; string holds bash's diagnostic.
    Error(String),
    /// The check could not run (no local `bash`, unreadable file, ...).
    Unavailable(String),
    /// The script passed the syntax check.
    Ok,
}

/// Run `bash -n` on a local script, before anything is uploaded to the remote host.
///
/// The file content is piped to `bash -n` through stdin instead of passing the
/// path as an argument, so the check also works when the local `bash` is a
/// Windows-side launcher (WSL / Git Bash) that cannot resolve local paths.
pub fn check_script_syntax(script_path: &Path) -> ScriptSyntaxStatus {
    let content = match std::fs::read(script_path) {
        Ok(content) => content,
        Err(e) => {
            return ScriptSyntaxStatus::Unavailable(format!(
                "failed to read '{}': {}",
                script_path.display(),
                e
            ));
        }
    };
    let mut child = match Command::new("bash")
        .arg("-n")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            return ScriptSyntaxStatus::Unavailable(format!(
                "local 'bash' is not available: {}",
                e
            ));
        }
    };
    // Feed the exact file bytes (LF endings preserved); a broken pipe only means
    // bash stopped reading, which is surfaced through its exit status below.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&content);
    }
    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(e) => {
            return ScriptSyntaxStatus::Unavailable(format!("failed to run 'bash -n': {}", e));
        }
    };
    if output.status.success() {
        return ScriptSyntaxStatus::Ok;
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let diagnostics: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| l.to_ascii_lowercase().contains("syntax error"))
        .collect();
    if !diagnostics.is_empty() {
        return ScriptSyntaxStatus::Error(diagnostics.join("\n\t> "));
    }
    // No parse failure in the output: treat it as an infrastructure problem
    // (launcher/WSL warnings, ...) rather than a broken script, so we never
    // reject a script for reasons unrelated to its content.
    let stderr = stderr.trim().to_string();
    ScriptSyntaxStatus::Unavailable(format!(
        "'bash -n' exited with {}{}",
        output.status,
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {}", stderr)
        }
    ))
}

/// Scripts whose local syntax check could not be performed, as `(path, reason)`.
#[derive(Debug)]
pub struct LocalSyntaxCheckReport {
    pub skipped: Vec<(String, String)>,
}

/// Verify every script locally (exists + `bash -n`) before any remote work, so a
/// broken script is never uploaded or executed on the remote host.
///
/// Returns `Err` on the first script that fails its checks; scripts whose
/// check could not run are reported in [`LocalSyntaxCheckReport::skipped`].
pub fn validate_scripts_locally(
    scripts: &[ScriptInvocation],
) -> Result<LocalSyntaxCheckReport, String> {
    let mut skipped = Vec::new();
    for script in scripts {
        let path = Path::new(&script.path);
        if !path.exists() || !path.is_file() {
            return Err(format!(
                "Script file '{}' does not exist or is not a file",
                script.path
            ));
        }
        match check_script_syntax(path) {
            ScriptSyntaxStatus::Ok => {}
            ScriptSyntaxStatus::Unavailable(reason) => {
                skipped.push((script.path.clone(), reason));
            }
            ScriptSyntaxStatus::Error(detail) => {
                return Err(format!(
                    "Local syntax check failed for '{}' (bash -n)\n\t> {}",
                    script.path, detail
                ));
            }
        }
    }
    Ok(LocalSyntaxCheckReport { skipped })
}

/// Remote env file target: absolute (`/etc/myapp/.env`) or home-relative (`~/app/.env`).
/// Relative paths are rejected because the remote shell working directory is not predictable.
pub fn validate_env_write_file_path(path: &str) -> Result<(), String> {
    if path.ends_with('/') {
        return Err(format!(
            "Invalid --env-write-file '{}': must be a file path, not a directory",
            path
        ));
    }
    if !path.starts_with('/') && !path.starts_with("~/") {
        return Err(format!(
            "Invalid --env-write-file '{}': use an absolute remote path (e.g. /etc/myapp/.env) or ~/app/.env",
            path
        ));
    }
    if DANGEROUS_PATHS.contains(path.trim_end_matches('/')) {
        return Err(format!(
            "Refusing to use critical system path '{}' as --env-write-file (a file path is required)",
            path
        ));
    }
    Ok(())
}

/// Validate `--env-extract-regex` / `--env-name` / `--env-write-file`.
///
/// There is no default env file: `--env-extract-regex` requires both `--env-name`
/// and `--env-write-file`, and neither of those may be used on its own.
pub fn validate_env_extract_options(
    regexes: &[String],
    names: &[String],
    env_write_file: Option<&str>,
) -> Result<(), String> {
    let write_file = env_write_file.map(str::trim).filter(|p| !p.is_empty());
    if regexes.is_empty() {
        if names.is_empty() && write_file.is_none() {
            return Ok(());
        }
        return Err("--env-name and --env-write-file require --env-extract-regex".to_string());
    }
    if names.is_empty() {
        return Err("--env-name is required when --env-extract-regex is set".to_string());
    }
    let write_file = write_file.ok_or_else(|| {
        "--env-write-file is required when --env-extract-regex is set (there is no default env file)"
            .to_string()
    })?;
    if regexes.len() != names.len() {
        return Err(format!(
            "--env-extract-regex count ({}) must match --env-name count ({})",
            regexes.len(),
            names.len()
        ));
    }
    validate_env_write_file_path(write_file)
}

/// Last capture-group match on lines that do not contain `\\r` (progress output).
pub fn extract_env_value(output: &str, pattern: &str) -> Result<String, String> {
    let re = Regex::new(pattern).map_err(|e| format!("Invalid --env-extract-regex: {}", e))?;
    if re.captures_len() < 2 {
        return Err(
            "--env-extract-regex must contain at least one capture group, e.g. 'URL=(.*)'"
                .to_string(),
        );
    }

    let mut last: Option<String> = None;
    for line in output.split('\n') {
        // PTY often uses CRLF; strip trailing CR before matching.
        let line = line.trim_end_matches('\r');
        // Skip progress/spinner lines that rewrite mid-line with CR.
        if line.contains('\r') {
            continue;
        }
        if let Some(caps) = re.captures(line) {
            if let Some(m) = caps.get(1) {
                last = Some(m.as_str().to_string());
            }
        }
    }

    last.ok_or_else(|| {
        let sample: Vec<&str> = output
            .lines()
            .map(|l| l.trim_end_matches('\r'))
            .filter(|l| !l.contains('\r') && !l.trim().is_empty())
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let sample_txt = if sample.is_empty() {
            "(no clean stdout lines collected)".to_string()
        } else {
            sample.join("\n\t")
        };
        let mut msg = format!(
            "No match for --env-extract-regex in remote output (skipped mid-line '\\r' progress)\n\
             \tPattern: {}\n\
             \tRecent clean lines:\n\t{}",
            pattern, sample_txt
        );
        // After shell double-quotes, `\[INFO\]` often becomes `[INFO]` (a character class).
        if pattern.contains('[') && !pattern.contains("\\[") {
            msg.push_str(
                "\n\tHint: use single quotes so brackets stay escaped, e.g. \
                 --env-extract-regex '\\[INFO\\]\\s+DATABASE_URL=(.*)'",
            );
        }
        msg
    })
}

fn validate_env_name(name: &str) -> Result<(), String> {
    let ok = name
        .chars()
        .enumerate()
        .all(|(i, c)| {
            if i == 0 {
                c.is_ascii_alphabetic() || c == '_'
            } else {
                c.is_ascii_alphanumeric() || c == '_'
            }
        })
        && !name.is_empty();
    if ok {
        Ok(())
    } else {
        Err(format!(
            "Invalid --env-name '{}': use [A-Za-z_][A-Za-z0-9_]*",
            name
        ))
    }
}

fn python_str_literal(s: &str) -> String {
    format!("'{s}'", s = s.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn build_upsert_env_command(env_write_file: &str, env_name: &str, value: &str) -> String {
    // Build the script line by line: Rust's `\`-continuations strip the leading
    // whitespace of the following source line, which silently removed the Python
    // block indentation and caused `IndentationError: expected an indented block`
    // under `python3 -c`. Keeping indentation inside quoted strings avoids that.
    let py = [
        "from pathlib import Path".to_string(),
        format!("file_path = Path({})", python_str_literal(env_write_file)),
        "missing_dirs = []".to_string(),
        "scan_dir = file_path.parent".to_string(),
        "while not scan_dir.exists() and scan_dir.parent != scan_dir:".to_string(),
        "    missing_dirs.append(scan_dir)".to_string(),
        "    scan_dir = scan_dir.parent".to_string(),
        "for new_dir in reversed(missing_dirs):".to_string(),
        "    new_dir.mkdir(mode=0o700, exist_ok=True)".to_string(),
        "    new_dir.chmod(0o700)".to_string(),
        format!("key = {}", python_str_literal(env_name)),
        format!("value = {}", python_str_literal(value)),
        "lines = [ln for ln in (file_path.read_text(encoding='utf-8').splitlines() if file_path.exists() else []) if not ln.startswith(key + '=')]".to_string(),
        "lines.append(key + '=' + value)".to_string(),
        r"file_path.write_text('\n'.join(lines) + '\n', encoding='utf-8')".to_string(),
        "file_path.chmod(0o600)".to_string(),
    ]
    .join("\n");
    format!("python3 -c {}", shell_quote(&py))
}

async fn persist_env_value(
    server_metadata: &Arc<ServerMetadata>,
    global_server_pool: &Arc<ServerPool>,
    use_sudo: bool,
    env_write_file: &str,
    env_name: &str,
    value: &str,
) -> Result<(), String> {
    validate_env_name(env_name)?;
    let remote_cmd = build_upsert_env_command(env_write_file, env_name, value);
    log_info!(
        server_metadata,
        EXECUTE_TASK_NAME,
        "Writing {} to {}",
        env_name,
        env_write_file
    );
    global_server_pool
        .exec_with_log(
            server_metadata,
            EXECUTE_TASK_NAME,
            &remote_cmd,
            use_sudo,
        )
        .await?;
    log_info!(
        server_metadata,
        EXECUTE_TASK_NAME,
        "Saved {}={} in {}",
        env_name,
        value,
        env_write_file
    );
    Ok(())
}

/// Resolve the remote env file target once (expands a leading `~` on the remote host).
async fn resolve_env_write_file(
    server_metadata: &Arc<ServerMetadata>,
    global_server_pool: &Arc<ServerPool>,
    use_sudo: bool,
    env_write_file: &str,
) -> Result<String, String> {
    let resolved = global_server_pool
        .resolve_remote_path(
            server_metadata,
            EXECUTE_TASK_NAME,
            use_sudo,
            env_write_file,
        )
        .await?
        .trim()
        .to_string();
    if resolved.is_empty() {
        return Err(format!(
            "Failed to resolve remote env file '{}'",
            env_write_file
        ));
    }
    validate_env_write_file_path(&resolved)?;
    Ok(resolved)
}

pub async fn run(
    config: &ExecuteCmdConfig,
    server_metadata: &Arc<ServerMetadata>,
    global_server_pool: Arc<ServerPool>,
) -> Result<(), String> {
    if config.scripts.is_empty() && config.cmds.is_empty() {
        return Err("Provide at least one --script or --cmd".to_string());
    }

    validate_env_extract_options(
        &config.env_extract_regexes,
        &config.env_names,
        config.env_write_file.as_deref(),
    )?;

    // Local pre-flight: reject broken scripts before any remote connection work,
    // upload, or execution.
    let syntax_report = validate_scripts_locally(&config.scripts)?;
    if !config.scripts.is_empty() {
        let passed = config.scripts.len() - syntax_report.skipped.len();
        if passed > 0 {
            log_debug!(
                server_metadata,
                EXECUTE_TASK_NAME,
                "Local 'bash -n' syntax check passed for {} script(s)",
                passed
            );
        }
    }
    for (path, reason) in &syntax_report.skipped {
        log_warn!(
            server_metadata,
            EXECUTE_TASK_NAME,
            "Skipped local 'bash -n' check for '{}': {}",
            path,
            reason
        );
    }

    let mut all_output = String::new();

    let execute_script = |script: ScriptInvocation,
                          server_metadata: Arc<ServerMetadata>,
                          global_server_pool: Arc<ServerPool>| async move {
        let script_path = Path::new(&script.path);
        if !script_path.exists() || !script_path.is_file() {
            return Err(format!(
                "Script file '{}' does not exist or is not a file",
                script.path
            ));
        }

        let script_name = script_path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("Failed to get basename for '{}'", &script.path))?;

        let temp_remote_dir = global_server_pool
            .create_remote_temp_dir(
                &server_metadata.clone(),
                EXECUTE_TASK_NAME,
                "exec",
                config.use_sudo,
            )
            .await?;

        log_debug!(
            &server_metadata,
            EXECUTE_TASK_NAME,
            "Uploading script '{}' to temporary path '{}'",
            script.path,
            temp_remote_dir
        );

        global_server_pool
            .upload_file_or_dir_contents_into_dir(
                &server_metadata,
                EXECUTE_TASK_NAME,
                script_path,
                &temp_remote_dir,
                None,
                config.use_sudo,
                config.use_rsync,
                config.silent,
                true,
                false,
            )
            .await?;

        let remote_script = format!("{}/{}", temp_remote_dir, script_name);
        let remote_cmd =
            build_remote_script_command(&config.work_path, &remote_script, &script.args);

        if script.args.is_empty() {
            log_info!(
                &server_metadata,
                EXECUTE_TASK_NAME,
                "Executing script {} in '{}'",
                script_name,
                config.work_path
            );
        } else {
            log_info!(
                &server_metadata,
                EXECUTE_TASK_NAME,
                "Executing script {} in '{}' with args {:?}",
                script_name,
                config.work_path,
                script.args
            );
        }

        let out = global_server_pool
            .exec_with_log(
                &server_metadata,
                EXECUTE_TASK_NAME,
                &remote_cmd,
                config.use_sudo,
            )
            .await?;

        Ok(out)
    };

    if config.mode == "async" && !config.scripts.is_empty() {
        let futures = config
            .scripts
            .clone()
            .into_iter()
            .map(|s| execute_script(s, server_metadata.clone(), global_server_pool.clone()));
        let results = join_all(futures).await;

        for result in results {
            match result {
                Ok(out) => {
                    if !all_output.is_empty() {
                        all_output.push('\n');
                    }
                    all_output.push_str(&out);
                }
                Err(e) => return Err(e),
            }
        }
    } else {
        for script in &config.scripts {
            let out = execute_script(
                script.clone(),
                server_metadata.clone(),
                global_server_pool.clone(),
            )
            .await?;
            if !all_output.is_empty() {
                all_output.push('\n');
            }
            all_output.push_str(&out);
        }
    }

    for cmd in &config.cmds {
        log_info!(
            server_metadata,
            EXECUTE_TASK_NAME,
            "Executing command in '{}': {}",
            config.work_path,
            cmd
        );
        let remote_cmd = build_remote_cmd_command(&config.work_path, cmd);
        let out = global_server_pool
            .exec_with_log(
                server_metadata,
                EXECUTE_TASK_NAME,
                &remote_cmd,
                config.use_sudo,
            )
            .await?;
        if !all_output.is_empty() {
            all_output.push('\n');
        }
        all_output.push_str(&out);
    }

    // No default env file: the target is always the user-supplied --env-write-file.
    if !config.env_extract_regexes.is_empty() {
        let env_write_file = resolve_env_write_file(
            server_metadata,
            &global_server_pool,
            config.use_sudo,
            config.env_write_file.as_deref().unwrap_or_default(),
        )
        .await?;

        for (pattern, env_name) in config
            .env_extract_regexes
            .iter()
            .zip(config.env_names.iter())
        {
            let value = extract_env_value(&all_output, pattern)?;
            persist_env_value(
                server_metadata,
                &global_server_pool,
                config.use_sudo,
                &env_write_file,
                env_name,
                &value,
            )
            .await?;
        }
    }

    if !config.scripts.is_empty() && config.cmds.is_empty() {
        log_info!(
            server_metadata,
            EXECUTE_TASK_NAME,
            "All scripts executed successfully."
        );
    } else if config.scripts.is_empty() && !config.cmds.is_empty() {
        log_info!(
            server_metadata,
            EXECUTE_TASK_NAME,
            "All commands executed successfully."
        );
    } else {
        log_info!(
            server_metadata,
            EXECUTE_TASK_NAME,
            "All scripts and commands executed successfully."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_skips_cr_progress_and_takes_last() {
        let out = "[INFO] DATABASE_URL=first\nprogress 10%\rprogress 50%\r\n[INFO] DATABASE_URL=second\n";
        let v = extract_env_value(out, r"\[INFO\] DATABASE_URL=(.*)").unwrap();
        assert_eq!(v, "second");
    }

    #[test]
    fn extract_accepts_trailing_cr_from_pty() {
        let out = "[INFO]   DATABASE_URL=postgres://u:p@127.0.0.1:5432/db\r\n";
        let v = extract_env_value(out, r"\[INFO\]\s+DATABASE_URL=(.*)").unwrap();
        assert_eq!(v, "postgres://u:p@127.0.0.1:5432/db");
    }

    #[test]
    fn extract_requires_capture_group() {
        assert!(extract_env_value("a=1", r"a=1").is_err());
    }

    fn vec_of(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn env_options_ok_when_nothing_set() {
        assert!(validate_env_extract_options(&[], &[], None).is_ok());
    }

    #[test]
    fn env_options_ok_with_regex_name_and_write_file() {
        let regexes = vec_of(&[r"URL=(.*)", r"USER=(.*)"]);
        let names = vec_of(&["DB_URL", "DB_USER"]);
        assert!(validate_env_extract_options(&regexes, &names, Some("/etc/myapp/.env")).is_ok());
        assert!(validate_env_extract_options(&regexes, &names, Some("~/myapp/.env")).is_ok());
        assert!(validate_env_extract_options(&regexes, &names, Some(" /etc/myapp/.env ")).is_ok());
    }

    #[test]
    fn env_options_require_write_file_with_regex() {
        let regexes = vec_of(&[r"URL=(.*)"]);
        let names = vec_of(&["DB_URL"]);
        let err = validate_env_extract_options(&regexes, &names, None).unwrap_err();
        assert!(err.contains("--env-write-file is required"), "{}", err);
        // An empty value behaves like "not provided".
        assert!(validate_env_extract_options(&regexes, &names, Some("  ")).is_err());
    }

    #[test]
    fn env_options_require_name_with_regex() {
        let regexes = vec_of(&[r"URL=(.*)"]);
        let err =
            validate_env_extract_options(&regexes, &[], Some("/etc/myapp/.env")).unwrap_err();
        assert!(err.contains("--env-name is required"), "{}", err);
    }

    #[test]
    fn env_name_and_write_file_require_regex() {
        let names = vec_of(&["DB_URL"]);
        let err = validate_env_extract_options(&[], &names, Some("/etc/myapp/.env")).unwrap_err();
        assert!(err.contains("require --env-extract-regex"), "{}", err);

        let regexes = vec_of(&[r"URL=(.*)"]);
        let err = validate_env_extract_options(&[], &[], Some("/etc/myapp/.env")).unwrap_err();
        assert!(err.contains("require --env-extract-regex"), "{}", err);
        let _ = regexes;
    }

    #[test]
    fn env_options_check_matching_counts() {
        let regexes = vec_of(&[r"URL=(.*)"]);
        let names = vec_of(&["DB_URL", "DB_USER"]);
        let err =
            validate_env_extract_options(&regexes, &names, Some("/etc/myapp/.env")).unwrap_err();
        assert!(err.contains("count"), "{}", err);
    }

    #[test]
    fn env_write_file_path_must_be_absolute_file() {
        assert!(validate_env_write_file_path("/etc/myapp/.env").is_ok());
        assert!(validate_env_write_file_path("~/myapp/.env").is_ok());
        // Relative paths are ambiguous on the remote host.
        assert!(validate_env_write_file_path("myapp/.env").is_err());
        // Directories are not env files.
        assert!(validate_env_write_file_path("/etc/myapp/").is_err());
        // Critical system directories are never valid targets.
        assert!(validate_env_write_file_path("/etc").is_err());
        assert!(validate_env_write_file_path("/").is_err());
        assert!(validate_env_write_file_path("/etc/").is_err());
    }

    #[test]
    fn upsert_command_targets_given_env_file() {
        let cmd = build_upsert_env_command("/etc/myapp/.env", "DB_URL", "postgres://db");
        assert!(cmd.starts_with("python3 -c '"), "{}", cmd);
        // Path/key/value are single-quote shell-escaped (`'` -> `'"'"'`), so assert
        // on the raw substrings instead of the unescaped python source.
        assert!(cmd.contains("/etc/myapp/.env"), "{}", cmd);
        assert!(cmd.contains("DB_URL"), "{}", cmd);
        assert!(cmd.contains("postgres://db"), "{}", cmd);
        assert!(cmd.contains("file_path.chmod(0o600)"), "{}", cmd);
        assert!(cmd.contains("mkdir(mode=0o700"), "{}", cmd);
        // Regression: block bodies must keep their indentation (`\`-continuations
        // used to strip it, causing IndentationError on the remote host).
        assert!(
            cmd.contains("\nwhile not scan_dir.exists()"),
            "loop header must be at column 0:\n{}",
            cmd
        );
        assert!(
            cmd.contains("\n    missing_dirs.append(scan_dir)"),
            "while body must be indented:\n{}",
            cmd
        );
        assert!(
            cmd.contains("\n    scan_dir = scan_dir.parent"),
            "while body must be indented:\n{}",
            cmd
        );
        assert!(
            cmd.contains("\n    new_dir.mkdir(mode=0o700, exist_ok=True)"),
            "for body must be indented:\n{}",
            cmd
        );
        assert!(
            cmd.contains("\n    new_dir.chmod(0o700)"),
            "for body must be indented:\n{}",
            cmd
        );
        // The old hard-coded default must be gone.
        assert!(!cmd.contains("/etc/rhctl"), "{}", cmd);

        // A home-relative target is kept as-is (resolved remotely before this call).
        let cmd = build_upsert_env_command("~/myapp/.env", "DB_URL", "1");
        assert!(cmd.contains("~/myapp/.env"), "{}", cmd);
    }

    /// The sudo path in `ssh_pool` wraps the command with `shlex::try_quote`
    /// before it reaches the remote shell. Prove that round-trip is lossless,
    /// including the block indentation the fix depends on.
    #[test]
    fn upsert_command_round_trips_shell_quoting() {
        let cmd = build_upsert_env_command(
            "/etc/pidifa/.env",
            "DATABASE_URL_1",
            "postgres://user:pass@127.0.0.1:23759/db",
        );
        let quoted = shlex::try_quote(&cmd).unwrap().to_string();
        let parts = shlex::split(&quoted).unwrap();
        assert_eq!(parts, vec![cmd.clone()], "quoting must be lossless");
        assert!(
            parts[0].contains("\n    missing_dirs.append(scan_dir)"),
            "indentation must survive shell quoting:\n{}",
            parts[0]
        );
    }

    /// Full remote simulation: run the generated command through a real
    /// `bash -c <cmd>` — the exact layering `ssh_pool` uses after `shlex`
    /// unwrapping — and verify the env file is created and written.
    ///
    /// Skips when this machine has no usable `bash` + `python3` pair.
    #[test]
    fn upsert_command_executes_through_bash() {
        let probe = std::process::Command::new("bash")
            .args(["-c", "command -v python3 >/dev/null 2>&1"])
            .output();
        if !matches!(&probe, Ok(o) if o.status.success()) {
            eprintln!("skipped: no local bash with python3");
            return;
        }
        // Make the local temp dir addressable inside that bash
        // (WSL: C:/Users/... -> /mnt/c/Users/...; Git Bash -> /c/Users/...).
        let temp = std::env::temp_dir().to_string_lossy().replace('\\', "/");
        let candidates: Vec<String> = if temp.len() >= 2 && temp.as_bytes()[1] == b':' {
            let drive = temp[..1].to_ascii_lowercase();
            let rest = temp[2..].to_string();
            vec![format!("/mnt{}{}", drive, rest), format!("/{}{}", drive, rest)]
        } else {
            vec![temp.clone()]
        };
        let mut probe_log = String::new();
        let mut bash_temp = None;
        for candidate in &candidates {
            let out = std::process::Command::new("bash")
                .args(["-c", &format!("test -d '{}'", candidate)])
                .output();
            match &out {
                Ok(o) if o.status.success() => {
                    bash_temp = Some(candidate.clone());
                    break;
                }
                other => probe_log.push_str(&format!(
                    "\n\tprobe '{}': {:?}",
                    candidate,
                    other.as_ref().map(|o| (
                        o.status.code(),
                        String::from_utf8_lossy(&o.stderr).to_string()
                    ))
                )),
            }
        }
        let bash_temp = match bash_temp {
            Some(dir) => dir,
            None => {
                eprintln!(
                    "skipped: temp dir not visible to local bash (temp={}){}",
                    temp, probe_log
                );
                return;
            }
        };

        let local_file = std::env::temp_dir()
            .join("rhctl_bash_e2e")
            .join("nested")
            .join("app.env");
        let _ = std::fs::remove_file(&local_file);
        let bash_file = format!("{}/rhctl_bash_e2e/nested/app.env", bash_temp);
        let expected_value = "postgres://test:test@127.0.0.1:23759/db1";
        let cmd = build_upsert_env_command(&bash_file, "DATABASE_URL_1", expected_value);

        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(&cmd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "bash -c round-trip failed:\n---\n{}\n---\nstdout:\n{}\nstderr:\n{}",
            cmd,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let content = std::fs::read_to_string(&local_file)
            .expect("env file was not written")
            .replace("\r\n", "\n");
        assert_eq!(
            content,
            format!("DATABASE_URL_1={}\n", expected_value)
        );
        let _ = std::fs::remove_file(&local_file);
    }

    /// Reverse `shell_quote`'s `'` -> `'\''` escaping to recover the python source.
    fn extract_python_source(cmd: &str) -> String {
        let body = cmd
            .strip_prefix("python3 -c '")
            .and_then(|b| b.strip_suffix('\''))
            .expect("expected a `python3 -c '<quoted>'` command");
        body.replace("'\\''", "'")
    }

    fn local_python() -> Option<&'static str> {
        for candidate in ["python3", "python"] {
            if let Ok(out) = std::process::Command::new(candidate)
                .arg("--version")
                .output()
            {
                if out.status.success() {
                    return Some(candidate);
                }
            }
        }
        None
    }

    /// End-to-end regression test for the remote `IndentationError`: execute the
    /// *generated* python source with a real interpreter, create the missing
    /// parent dirs, then upsert the same key a second time.
    #[test]
    fn upsert_command_python_executes_and_upserts() {
        let python = match local_python() {
            Some(python) => python,
            None => {
                eprintln!("skipped: no local python interpreter");
                return;
            }
        };
        let base = std::env::temp_dir().join("rhctl_upsert_e2e");
        let env_file = base.join("nested").join("app.env");
        let py_file = std::env::temp_dir().join("rhctl_upsert_e2e_check.py");
        let _ = std::fs::remove_file(&env_file);
        // Forward slashes (also what absolute/~ remote paths use).
        let file_arg = env_file.to_string_lossy().replace('\\', "/");
        // Mirrors the real-world value from the bug report (URL-encoded payload).
        let value1 = "postgres://test:test@127.0.0.1:23759/db1";

        for (value, expected) in [
            (value1, format!("DATABASE_URL_1={}\n", value1)),
            ("second-value", "DATABASE_URL_1=second-value\n".to_string()),
        ] {
            let cmd = build_upsert_env_command(&file_arg, "DATABASE_URL_1", value);
            let source = extract_python_source(&cmd);
            std::fs::write(&py_file, &source).unwrap();
            let out = std::process::Command::new(python)
                .arg(&py_file)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "generated python failed:\n---\n{}\n---\nstderr:\n{}",
                source,
                String::from_utf8_lossy(&out.stderr)
            );
            // Windows text-mode writes `\r\n`; normalize before comparing
            // (the Linux remote target always writes plain `\n`).
            let content = std::fs::read_to_string(&env_file)
                .unwrap()
                .replace("\r\n", "\n");
            assert_eq!(content, expected, "unexpected env file content");
        }

        let _ = std::fs::remove_file(&env_file);
        let _ = std::fs::remove_file(&py_file);
    }

    fn write_temp_script(name: &str, content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn bash_n_accepts_valid_script() {
        let path = write_temp_script(
            "rhctl_bash_n_valid.sh",
            "#!/bin/bash\necho hi\nif true; then\n  echo yes\nfi\n",
        );
        match check_script_syntax(&path) {
            ScriptSyntaxStatus::Ok => {}
            // Local `bash` missing/unusable: nothing to verify on this host.
            ScriptSyntaxStatus::Unavailable(reason) => eprintln!("skipped: {}", reason),
            other => panic!("expected Ok, got {:?}", other),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn bash_n_rejects_broken_script() {
        let path = write_temp_script("rhctl_bash_n_broken.sh", "#!/bin/bash\nif true; then\necho hi\n");
        match check_script_syntax(&path) {
            ScriptSyntaxStatus::Error(detail) => {
                assert!(detail.to_lowercase().contains("syntax error"), "{}", detail)
            }
            // Local `bash` missing/unusable: cannot assert on the diagnostic.
            ScriptSyntaxStatus::Unavailable(reason) => eprintln!("skipped: {}", reason),
            other => panic!("expected Error, got {:?}", other),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn validate_scripts_locally_fails_on_syntax_error() {
        let path = write_temp_script("rhctl_bash_n_validate.sh", "if true; then\necho hi\n");
        let scripts = vec![ScriptInvocation {
            path: path.to_string_lossy().into_owned(),
            args: vec![],
        }];
        match validate_scripts_locally(&scripts) {
            // Local `bash` missing/unusable: the check is reported as skipped.
            Ok(report) => assert!(!report.skipped.is_empty()),
            Err(e) => {
                assert!(e.contains("bash -n"), "{}", e);
                assert!(e.contains("syntax error"), "{}", e);
                assert!(e.contains("rhctl_bash_n_validate.sh"), "{}", e);
            }
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn validate_scripts_locally_rejects_missing_file() {
        let scripts = vec![ScriptInvocation {
            path: "/nonexistent/rhctl_missing_script.sh".to_string(),
            args: vec![],
        }];
        let err = validate_scripts_locally(&scripts).unwrap_err();
        assert!(err.contains("does not exist"), "{}", err);
    }

    #[test]
    fn check_script_syntax_reports_unavailable_instead_of_false_error() {
        // Reading a directory fails, so `bash -n` never runs: this must be
        // reported as "unavailable" (skip), never as a syntax error.
        match check_script_syntax(&std::env::temp_dir()) {
            ScriptSyntaxStatus::Unavailable(reason) => {
                assert!(reason.contains("failed to read"), "{}", reason)
            }
            other => panic!("expected Unavailable, got {:?}", other),
        }
    }

    #[test]
    fn validate_scripts_locally_accepts_valid_and_empty_lists() {
        assert!(validate_scripts_locally(&[]).is_ok());

        let path = write_temp_script(
            "rhctl_bash_n_ok_list.sh",
            "#!/bin/bash\nset -e\necho 'all good'\n",
        );
        let scripts = vec![ScriptInvocation {
            path: path.to_string_lossy().into_owned(),
            args: vec!["--flag".to_string()],
        }];
        let report = validate_scripts_locally(&scripts).unwrap();
        // Either the script passed, or `bash` is unavailable here (skipped).
        if !report.skipped.is_empty() {
            assert_eq!(report.skipped.len(), 1);
        }
        let _ = std::fs::remove_file(&path);
    }
}
