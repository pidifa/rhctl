use crate::common::ssh_pool::ServerPool;
use crate::domain::cmd_params::{ServerMetadata, TransferCmdConfig};
use crate::domain::constants::TRANSFER_TASK_NAME;
use crate::utils::file_utils::{
    expand_local_user_path, format_env_file, load_local_env_file, merge_env_entries, parse_env_file_contents,
};
use crate::{log_info, log_warn};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use futures::future::join_all;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
fn python_str_literal(s: &str) -> String {
    format!("'{s}'", s = s.replace('\\', "\\\\").replace('\'', "\\'"))
}
fn build_write_env_file_cmd(remote_path: &str, content: &str) -> String {
    let b64 = B64.encode(content.as_bytes());
    let parent = Path::new(remote_path)
        .parent()
        .and_then(|p| p.to_str())
        .filter(|p| !p.is_empty())
        .unwrap_or("/");
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
    config: &TransferCmdConfig,
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
            TRANSFER_TASK_NAME,
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
            TRANSFER_TASK_NAME,
            &read_cmd,
            config.use_sudo,
        )
        .await
        .unwrap_or_default();
    let remote_entries = if remote_text.trim().is_empty() {
        Vec::new()
    } else {
        parse_env_file_contents(&remote_text)
            .map_err(|e| format!("Failed to parse remote env file '{}': {}", remote_path, e))?
    };
    let merged = merge_env_entries(&remote_entries, &local_entries);
    let content = format_env_file(&merged);
    let write_cmd = build_write_env_file_cmd(&remote_path, &content);
    log_info!(
        server_metadata,
        TRANSFER_TASK_NAME,
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
            TRANSFER_TASK_NAME,
            &write_cmd,
            config.use_sudo,
        )
        .await?;
    Ok(())
}
pub async fn run(
    config: &TransferCmdConfig,
    uploads: &HashMap<String, String>,
    downloads: &HashMap<String, String>,
    patch_envs: &[(String, String)],
    server_metadata: &Arc<ServerMetadata>,
    global_server_pool: Arc<ServerPool>,
) -> Result<(), String> {
    if uploads.is_empty() && downloads.is_empty() {
        return Err("Provide at least one of --upload or --download".to_string());
    }
    if !patch_envs.is_empty() && uploads.is_empty() {
        return Err("--patch-env can only be used with upload mappings".to_string());
    }

    let mut tasks = Vec::new();

    for (local_item, remote_dir) in uploads {
        let local_file_or_dir = Path::new(local_item).to_path_buf();
        if !local_file_or_dir.exists() {
            log_warn!(
                server_metadata,
                TRANSFER_TASK_NAME,
                "Local item '{}' not found. Skipping.",
                local_item
            );
            continue;
        }

        let remote_dir_resolved = global_server_pool
            .resolve_remote_path(
                server_metadata,
                TRANSFER_TASK_NAME,
                config.use_sudo,
                remote_dir,
            )
            .await
            .map_err(|e| {
                format!(
                    "Child upload task failed: {} --> {} \n\t> {}",
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
                TRANSFER_TASK_NAME,
                &remote_dir_resolved,
                config.use_sudo,
            )
            .await
            .map_err(|e| {
                format!(
                    "Child upload task failed: {} --> {} \n\t> {}",
                    local_item, remote_dir, e
                )
            })?;

        let local_clone = local_file_or_dir.clone();
        let remote_clone = remote_dir_resolved.clone();
        let config_clone = config.clone();
        let global_server_pool_clone = global_server_pool.clone();
        let server_metadata_clone = server_metadata.clone();
        tasks.push(tokio::spawn(async move {
            global_server_pool_clone
                .upload_file_or_dir_contents_into_dir(
                    &server_metadata_clone,
                    TRANSFER_TASK_NAME,
                    &local_clone,
                    &remote_clone,
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
                        "Child upload task failed: {} --> {} \n\t> {}",
                        local_clone.display(),
                        remote_clone,
                        e
                    )
                })
        }));
    }

    for (remote_item, local_dir) in downloads {
        let local_dir = expand_local_user_path(local_dir);

        let remote_resolved = global_server_pool
            .resolve_remote_path(
                server_metadata,
                TRANSFER_TASK_NAME,
                config.use_sudo,
                remote_item,
            )
            .await
            .map_err(|e| {
                format!(
                    "Child download task failed: {} --> {} \n\t> {}",
                    remote_item, local_dir, e
                )
            })?;
        let remote_resolved = if remote_resolved != "/" {
            remote_resolved.trim_end_matches('/').to_string()
        } else {
            remote_resolved
        };
        if remote_resolved.is_empty() {
            return Err(format!("Failed to resolve remote path '{}'", remote_item));
        }
        if !global_server_pool
            .file_or_dir_exists(
                server_metadata,
                TRANSFER_TASK_NAME,
                &remote_resolved,
                config.use_sudo,
            )
            .await?
        {
            return Err(format!("Remote path '{}' does not exist", remote_resolved));
        }

        let local_dir_path = PathBuf::from(&local_dir);
        if local_dir_path.exists() && !local_dir_path.is_dir() {
            return Err(format!(
                "Local download target '{}' is not a directory",
                local_dir
            ));
        }
        std::fs::create_dir_all(&local_dir_path).map_err(|e| {
            format!(
                "Failed to create local download directory '{}': {}",
                local_dir, e
            )
        })?;

        let remote_clone = remote_resolved.clone();
        let local_clone = local_dir_path.clone();
        let config_clone = config.clone();
        let global_server_pool_clone = global_server_pool.clone();
        let server_metadata_clone = server_metadata.clone();
        tasks.push(tokio::spawn(async move {
            global_server_pool_clone
                .download_file_or_dir_contents_into_dir(
                    &server_metadata_clone,
                    TRANSFER_TASK_NAME,
                    &remote_clone,
                    &local_clone,
                    config_clone.use_sudo,
                    config_clone.use_rsync,
                    config_clone.silent,
                    true,
                )
                .await
                .map_err(|e| {
                    format!(
                        "Child download task failed: {} --> {} \n\t> {}",
                        remote_clone,
                        local_clone.display(),
                        e
                    )
                })
        }));
    }

    let results = join_all(tasks).await;
    let mut errors = Vec::new();
    for r in results {
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(format!("Task join error: {:?}", e)),
        }
    }

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
    log_info!(server_metadata, TRANSFER_TASK_NAME, "Transfer complete.");
    Ok(())
}
