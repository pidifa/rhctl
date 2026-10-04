/*
 * Copyright (C) 2022-2026 rhctl Contributors
 *
 * SPDX-License-Identifier: Apache-2.0
 * **************************************************************************
 * Upload a file or directory contents into a remote server directory,
 * and optionally merge local `.env` files into remote env files.
 *
 * Since: 1.0.0
 * Date: October 16, 2025
 */

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use futures::future::join_all;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::common::ssh_pool::ServerPool;
use crate::domain::cmd_params::{ServerMetadata, UploadCmdConfig};
use crate::domain::constants::UPLOAD_TASK_NAME;
use crate::utils::file_utils::{
    format_env_file, load_local_env_file, merge_env_entries, parse_env_file_contents,
};
use crate::{log_info, log_warn};

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn python_str_literal(s: &str) -> String {
    format!("'{s}'", s = s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Write full env file contents via base64 to avoid shell/quoting issues.
fn build_write_env_file_cmd(remote_path: &str, content: &str) -> String {
    let b64 = B64.encode(content.as_bytes());
    let parent = Path::new(remote_path)
        .parent()
        .and_then(|p| p.to_str())
        .filter(|p| !p.is_empty())
        .unwrap_or("/");
    // No indented blocks: Rust `\n\` line-continuations strip leading spaces.
    let py = format!(
        "from pathlib import Path\n\
import base64\n\
parent = Path({parent})\n\
parent.mkdir(parents=True, exist_ok=True)\n\
(parent.chmod(0o700) if str(parent) == '/etc/rhctl' else None)\n\
path = Path({path})\n\
path.write_bytes(base64.b64decode({b64}))\n\
path.chmod(0o600)\n",
        parent = python_str_literal(parent),
        path = python_str_literal(remote_path),
        b64 = python_str_literal(&b64),
    );
    format!("python3 -c {}", shell_quote(&py))
}

async fn patch_one_env(
    config: &UploadCmdConfig,
    server_metadata: &Arc<ServerMetadata>,
    global_server_pool: &Arc<ServerPool>,
    local_path: &str,
    remote_path_raw: &str,
) -> Result<(), String> {
    let local = PathBuf::from(local_path);
    if !local.is_file() {
        return Err(format!(
            "Local env file '{}' does not exist or is not a file",
            local_path
        ));
    }

    let remote_path = global_server_pool
        .resolve_remote_path(
            server_metadata,
            UPLOAD_TASK_NAME,
            config.use_sudo,
            remote_path_raw,
        )
        .await
        .map_err(|e| {
            format!(
                "Failed to resolve remote env path '{}': {}",
                remote_path_raw, e
            )
        })?;
    if remote_path.is_empty() {
        return Err(format!(
            "Failed to resolve remote env path '{}'",
            remote_path_raw
        ));
    }

    let local_entries = load_local_env_file(local_path)?;

    let read_cmd = format!(
        "if [ -f {p} ]; then cat {p}; fi",
        p = shell_quote(&remote_path)
    );
    let remote_text = global_server_pool
        .exec(
            server_metadata,
            UPLOAD_TASK_NAME,
            &read_cmd,
            config.use_sudo,
        )
        .await
        .unwrap_or_default();
    let remote_entries = if remote_text.trim().is_empty() {
        Vec::new()
    } else {
        parse_env_file_contents(&remote_text).map_err(|e| {
            format!(
                "Failed to parse remote env file '{}': {}",
                remote_path, e
            )
        })?
    };

    let merged = merge_env_entries(&remote_entries, &local_entries);
    let content = format_env_file(&merged);
    let write_cmd = build_write_env_file_cmd(&remote_path, &content);

    log_info!(
        server_metadata,
        UPLOAD_TASK_NAME,
        "Patching env {} → {} (local={}, remote-only kept={}, total={})",
        local_path,
        remote_path,
        local_entries.len(),
        remote_entries
            .iter()
            .filter(|(k, _)| !local_entries.iter().any(|(lk, _)| lk == k))
            .count(),
        merged.len()
    );

    global_server_pool
        .exec_with_log(
            server_metadata,
            UPLOAD_TASK_NAME,
            &write_cmd,
            config.use_sudo,
        )
        .await?;

    Ok(())
}

pub async fn run(
    config: &UploadCmdConfig,
    mappings: &HashMap<String, String>,
    patch_envs: &[(String, String)],
    server_metadata: &Arc<ServerMetadata>,
    global_server_pool: Arc<ServerPool>,
) -> Result<(), String> {
    if mappings.is_empty() && patch_envs.is_empty() {
        return Err(
            "Provide at least one of --transfer, --transfer-file, or --patch-env".to_string(),
        );
    }

    let mut tasks = Vec::new();

    for (local_item, remote_dir) in mappings {
        let local_file_or_dir = Path::new(&local_item).to_path_buf();
        if !local_file_or_dir.exists() {
            log_warn!(
                server_metadata,
                UPLOAD_TASK_NAME,
                "Local item '{}' not found. Skipping.",
                local_item
            );
            continue;
        }

        let remote_dir_resolved = global_server_pool
            .resolve_remote_path(
                server_metadata,
                UPLOAD_TASK_NAME,
                config.use_sudo,
                &remote_dir,
            )
            .await
            .map_err(|e| {
                format!(
                    "Child Upload Task failed: {} --> {} \n\t> {}",
                    local_item, remote_dir, e
                )
            })?;
        if remote_dir_resolved.is_empty() {
            return Err(format!(
                "Failed to resolve remote directory '{}'",
                remote_dir
            ));
        }

        global_server_pool
            .validate_remote_dir(
                server_metadata,
                UPLOAD_TASK_NAME,
                &remote_dir_resolved,
                config.use_sudo,
            )
            .await
            .map_err(|e| {
                format!(
                    "Child Upload Task failed: {} --> {} \n\t> {}",
                    local_item, remote_dir, e
                )
            })?;

        let local_file_or_dir_clone = local_file_or_dir.clone();
        let remote_dir_clone = remote_dir_resolved.clone();
        let config_clone = config.clone();
        let global_server_pool_clone = global_server_pool.clone();
        let server_metadata_clone = server_metadata.clone();

        let task = tokio::spawn(async move {
            global_server_pool_clone
                .upload_file_or_dir_contents_into_dir(
                    &server_metadata_clone,
                    UPLOAD_TASK_NAME,
                    &local_file_or_dir_clone,
                    &remote_dir_clone,
                    None,
                    config_clone.use_sudo,
                    config_clone.use_rsync,
                    config_clone.silent,
                    false,
                    true,
                )
                .await
                .map_err(|e| {
                    format!(
                        "Child Upload Task failed: {} --> {} \n\t> {}",
                        local_file_or_dir_clone.display(),
                        remote_dir_clone,
                        e
                    )
                })
        });

        tasks.push(task);
    }

    let results = join_all(tasks).await;
    let mut errors = Vec::new();
    for r in results {
        if let Err(e) = r {
            errors.push(format!("Task join error: {:?}", e));
        } else if let Ok(Err(e)) = r {
            errors.push(e);
        }
    }

    // Env patches run sequentially after file transfers (need stable remote state).
    for (local_path, remote_path) in patch_envs {
        if let Err(e) = patch_one_env(
            config,
            server_metadata,
            &global_server_pool,
            local_path,
            remote_path,
        )
        .await
        {
            errors.push(format!(
                "Patch-env failed: {} --> {} \n\t> {}",
                local_path, remote_path, e
            ));
        }
    }

    if !errors.is_empty() {
        return Err(errors.join("\n\n\t"));
    }

    log_info!(server_metadata, UPLOAD_TASK_NAME, "Upload complete.");
    Ok(())
}
