/*
 * Copyright (C) 2022-2026 rhctl Contributors
 *
 * SPDX-License-Identifier: Apache-2.0
 * **************************************************************************
 * File operation utils.
 *
 * Since: 1.0.0
 * Date: October 16, 2025
 */
use crate::domain::cmd_params::ServerMetadata;
use crate::domain::constants::REMOTE_TEMP_RHCTL_FOLDER;
use crate::domain::yml_config::YmlConfig;
use crate::log_local;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use dirs_next as dirs;

pub fn load_yaml_config(path: &str) -> Result<YmlConfig, String> {
    let mut file = File::open(path)
        .map_err(|e| format!("Failed to open config file {}. \n\t> {}", path, e))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|e| format!("Failed to read config file {}. \n\t> {}", path, e))?;
    // println!("contents: {:?}", contents);
    serde_yaml::from_str(&contents)
        .map_err(|e| format!("Failed to parse YAML config {}. \n\t> {}", path, e))
}

pub fn generate_remote_temp_dir(prefix: &str) -> String {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{}/{}_{:x}", REMOTE_TEMP_RHCTL_FOLDER, prefix, timestamp)
}
pub fn split_unix_path(path: &str) -> Result<(String, String), String> {
    let path: &str = path.trim_end_matches('/');

    match path.rsplit_once('/') {
        Some((parent, basename)) => Ok((basename.to_string(), parent.to_string())),
        None => Err(format!("Invalid unix path: {}", path)),
    }
}

pub fn log_local_file_info(
    server_metadata: &Arc<ServerMetadata>,
    task_name: &str,
    local_path: &Path,
) -> Result<(), String> {
    let ls_local = Command::new("ls")
        .arg("-al")
        .arg(local_path)
        .output()
        .map_err(|e| {
            format!(
                "Failed to execute ls -al on '{}'. \n\t> {}",
                local_path.display(),
                e
            )
        })?;

    if !ls_local.status.success() {
        return Err(format!(
            "Failed to get local file info for '{}'",
            local_path.display()
        ));
    }

    for line in String::from_utf8_lossy(&ls_local.stdout).lines() {
        // Replace `log_local` with your actual logging macro
        log_local!(server_metadata, task_name, "{}", line);
    }

    Ok(())
}

pub fn get_local_path_base_name(path: &Path) -> Result<String, String> {
    if !path.exists() {
        return Err(format!("Path does not exist: {}", path.display()));
    }

    let os_str: Option<&OsStr> = if path.is_dir() {
        path.file_name()
            .or_else(|| path.components().last().map(|c| c.as_os_str()))
    } else if path.is_file() {
        path.file_name()
    } else {
        None
    };

    os_str
        .and_then(|s| s.to_str())
        .map(|s| s.trim_end().to_string())
        .ok_or_else(|| format!("Could not get base name: {}", path.display()))
}
pub fn parse_mapping(
    raw: &str,
    vars: &HashMap<String, String>,
    expected: &str,
) -> Result<(String, String), String> {
    let line = raw.trim();
    let parts: Vec<&str> = line.splitn(2, '=').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid mapping '{}'; expected {}", raw, expected));
    }
    let source = substitute_vars(parts[0].trim(), vars)?;
    let target = substitute_vars(parts[1].trim(), vars)?;
    if source.is_empty() || target.is_empty() {
        return Err(format!("Empty source or target path in mapping '{}'", raw));
    }
    Ok((source, target))
}
pub fn load_mapping_file(
    file: &str,
    mappings: &mut HashMap<String, String>,
    vars: &HashMap<String, String>,
    expected: &str,
) -> Result<(), String> {
    let file = substitute_vars(file, vars)?;
    let f = File::open(&file).map_err(|e| format!("Error opening file '{}'. \n\t> {}", file, e))?;
    for (line_num, line) in BufReader::new(f).lines().enumerate() {
        let line = line.map_err(|e| format!("Error reading line {}. \n\t> {}", line_num + 1, e))?;
        let line = line.trim_end_matches(|c| c == '\n' || c == '\r' || c == ' ' || c == '\t');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let (local_file_or_dir, target_path) = parse_mapping(line, vars, expected)
            .map_err(|e| format!("Invalid format at line {}: {}", line_num + 1, e))?;
        mappings.insert(local_file_or_dir, target_path);
    }
    Ok(())
}
pub fn resolve_upload_mappings(
    upload_mapping: Option<&str>,
    uploads: &[String],
    vars: &HashMap<String, String>,
) -> Result<HashMap<String, String>, String> {
    let mut mappings = HashMap::new();
    if let Some(file) = upload_mapping {
        load_mapping_file(file, &mut mappings, vars, "local=remote-dir")?;
    }
    for raw in uploads {
        let (local, remote) = parse_mapping(raw, vars, "local=remote-dir")?;
        mappings.insert(local, remote);
    }
    Ok(mappings)
}

pub fn resolve_download_mappings(
    download_mapping: Option<&str>,
    downloads: &[String],
    vars: &HashMap<String, String>,
) -> Result<HashMap<String, String>, String> {
    let mut mappings = HashMap::new();
    if let Some(file) = download_mapping {
        load_mapping_file(file, &mut mappings, vars, "remote=local-dir")?;
    }
    for raw in downloads {
        let (remote, local) = parse_mapping(raw, vars, "remote=local-dir")?;
        mappings.insert(remote, local);
    }
    Ok(mappings)
}

pub fn resolve_patch_env_mappings(
    patch_envs: &[String],
    vars: &HashMap<String, String>,
) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let mut by_local: HashMap<String, usize> = HashMap::new();
    for raw in patch_envs {
        let (local, remote) = parse_mapping(raw, vars, "local=remote-env-file")?;
        if let Some(idx) = by_local.get(&local).copied() {
            out[idx] = (local.clone(), remote);
        } else {
            by_local.insert(local.clone(), out.len());
            out.push((local, remote));
        }
    }
    Ok(out)
}

/// Parse `.env` text into ordered (key, value) pairs. Skips blanks and `#` comments.
/// `export KEY=...` is accepted. First `=` separates key/value. Later duplicate keys win.
pub fn parse_env_file_contents(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut order: Vec<String> = Vec::new();
    let mut map: HashMap<String, String> = HashMap::new();

    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "Invalid .env line {}: expected KEY=VALUE, got '{}'",
                i + 1,
                raw
            ));
        };
        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .enumerate()
                .all(|(j, c)| {
                if j == 0 {
                    c.is_ascii_alphabetic() || c == '_'
                } else {
                    c.is_ascii_alphanumeric() || c == '_'
                }
            })
        {
            return Err(format!(
                "Invalid .env key on line {}: '{}'",
                i + 1,
                key
            ));
        }
        if !map.contains_key(key) {
            order.push(key.to_string());
        }
        map.insert(key.to_string(), value.to_string());
    }

    Ok(order
        .into_iter()
        .filter_map(|k| map.remove(&k).map(|v| (k, v)))
        .collect())
}

/// Merge local env over remote: remote-only keys kept; local keys overwrite; new local keys appended.
pub fn merge_env_entries(
    remote: &[(String, String)],
    local: &[(String, String)],
) -> Vec<(String, String)> {
    let local_map: HashMap<&str, &str> = local
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let mut seen: HashMap<String, ()> = HashMap::new();
    let mut out = Vec::new();

    for (k, v) in remote {
        let value = local_map.get(k.as_str()).copied().unwrap_or(v.as_str());
        out.push((k.clone(), value.to_string()));
        seen.insert(k.clone(), ());
    }
    for (k, v) in local {
        if seen.contains_key(k) {
            continue;
        }
        out.push((k.clone(), v.clone()));
    }
    out
}

pub fn format_env_file(entries: &[(String, String)]) -> String {
    let mut s = String::new();
    for (k, v) in entries {
        s.push_str(k);
        s.push('=');
        s.push_str(v);
        s.push('\n');
    }
    s
}

pub fn load_local_env_file(path: &str) -> Result<Vec<(String, String)>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read local env file '{}': {}", path, e))?;
    parse_env_file_contents(&text)
}

pub fn expand_vars(input: &str, overlays: &HashMap<String, String>) -> Result<String, String> {
    if input.trim().is_empty() {
        return Err("Input is empty".to_string());
    }
    let mut result = String::new();
    let mut rest = input;

    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);
        rest = &rest[start + 2..];

        if let Some(end) = rest.find('}') {
            let key = &rest[..end];
            let value = overlays
                .get(key)
                .cloned()
                .or_else(|| std::env::var(key).ok());
            match value {
                Some(value) if !value.is_empty() => result.push_str(&value),
                Some(_) => {
                    return Err(format!("Variable '{}' is empty in '{}'", key, input));
                }
                None => {
                    return Err(format!(
                        "Variable '{}' not found in environment (or var-map) in '{}'",
                        key, input
                    ));
                }
            }
            rest = &rest[end + 1..];
        } else {
            return Err(format!("Unclosed variable placeholder in '{}'", input));
        }
    }

    result.push_str(rest);
    Ok(result)
}

pub fn validate_path_chars(path: &str) -> Result<(), String> {
    // Invalid characters check (macOS / Linux only)
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let forbidden: &[char] = &['<', '>', ':', '"', '|', '?', '*'];
        if path.chars().any(|c| forbidden.contains(&c)) {
            return Err(format!("Path '{}' contains invalid characters.", path));
        }
    }
    let _ = path;
    Ok(())
}

pub fn substitute_vars(input_path: &str, vars: &HashMap<String, String>) -> Result<String, String> {
    let result = expand_vars(input_path, vars)?;
    validate_path_chars(&result)?;
    Ok(result)
}

/// Expand a leading `~/` to the user's home directory.
pub fn expand_local_user_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    } else if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().into_owned();
        }
    }
    path.to_string()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_transfer_mapping_ok() {
        let vars = HashMap::new();
        let (l, r) = parse_mapping("a/b.txt=~/out", &vars, "local=remote-dir").unwrap();
        assert_eq!(l, "a/b.txt");
        assert_eq!(r, "~/out");
    }

    #[test]
    fn resolve_upload_allows_empty() {
        let vars = HashMap::new();
        let m = resolve_upload_mappings(None, &[], &vars).unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn resolve_upload_inline_only() {
        let vars = HashMap::new();
        let uploads = vec!["local.txt=~/remote".to_string()];
        let m = resolve_upload_mappings(None, &uploads, &vars).unwrap();
        assert_eq!(m.get("local.txt").map(String::as_str), Some("~/remote"));
    }
    #[test]
    fn resolve_download_inline_only() {
        let vars = HashMap::new();
        let downloads = vec!["/remote/app=./backup".to_string()];
        let m = resolve_download_mappings(None, &downloads, &vars).unwrap();
        assert_eq!(m.get("/remote/app").map(String::as_str), Some("./backup"));
    }
    #[test]
    fn merge_env_keeps_remote_only_and_overwrites() {
        let remote = vec![
            ("A".into(), "1".into()),
            ("B".into(), "2".into()),
        ];
        let local = vec![
            ("B".into(), "9".into()),
            ("C".into(), "3".into()),
        ];
        let merged = merge_env_entries(&remote, &local);
        assert_eq!(
            merged,
            vec![
                ("A".into(), "1".into()),
                ("B".into(), "9".into()),
                ("C".into(), "3".into()),
            ]
        );
    }

    #[test]
    fn parse_env_skips_comments_and_export() {
        let text = "# comment\n\nFOO=bar\nexport BAZ=qux\n";
        let entries = parse_env_file_contents(text).unwrap();
        assert_eq!(
            entries,
            vec![("FOO".into(), "bar".into()), ("BAZ".into(), "qux".into())]
        );
    }
}




