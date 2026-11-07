use crate::common::ssh_pool::{RemotePathKind, ServerPool};
use crate::domain::cmd_params::{ServerMetadata, TransferCmdConfig};
use crate::domain::constants::{DEFAULT_TRANSFER_CONCURRENCY, TRANSFER_TASK_NAME};
use crate::utils::file_utils::{
    expand_local_user_path, format_env_file, load_local_env_file, local_path_kind,
    merge_env_entries, parse_env_file_contents, LocalPathKind, TransferMapping,
};
use crate::{log_info};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use futures::future::join_all;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
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
    uploads: &[TransferMapping],
    downloads: &[TransferMapping],
    patch_envs: &[(String, String)],
    server_metadata: &Arc<ServerMetadata>,
    global_server_pool: Arc<ServerPool>,
) -> Result<(), String> {
    if uploads.is_empty() && downloads.is_empty() && patch_envs.is_empty() {
        return Err("Provide at least one of --upload or --download or --patch-env".to_string());
    }
    let mut tasks = Vec::new();
    // Limit concurrent transfers to avoid exhausting SSH sessions/file descriptors.
    let semaphore = Arc::new(Semaphore::new(DEFAULT_TRANSFER_CONCURRENCY));

    for mapping in uploads {
        let source_kind = local_path_kind(&mapping.source).map_err(|e| {
            format!(
                "Upload mapping failed: {} --> {} \n\t> {}",
                mapping.source, mapping.target, e
            )
        })?;
        if source_kind == LocalPathKind::Missing {
            return Err(format!(
                "Upload mapping failed: {} --> {} \n\t> Local source '{}' does not exist",
                mapping.source, mapping.target, mapping.source
            ));
        }

        if mapping.contents_only && source_kind != LocalPathKind::Dir {
            return Err(format!(
                "Upload mapping failed: {} => {} \n\t> '=>' (contents transfer) requires a directory source",
                mapping.source, mapping.target
            ));
        }
        let local_path = expand_local_user_path(&mapping.source);
        let remote_target_raw = global_server_pool
            .resolve_remote_path(
                server_metadata,
                TRANSFER_TASK_NAME,
                config.use_sudo,
                &mapping.target,
            )
            .await
            .map_err(|e| {
                format!(
                    "Upload mapping failed: {} --> {} \n\t> {}",
                    mapping.source, mapping.target, e
                )
            })?;
        let remote_target = if remote_target_raw != "/" {
            remote_target_raw.trim_end_matches('/').to_string()
        } else {
            remote_target_raw
        };
        if remote_target.is_empty() {
            return Err(format!(
                "Upload mapping failed: {} --> {} \n\t> Failed to resolve remote path '{}'",
                mapping.source, mapping.target, mapping.target
            ));
        }
        let remote_kind = global_server_pool
            .remote_path_kind(
                server_metadata,
                TRANSFER_TASK_NAME,
                &remote_target,
                config.use_sudo,
            )
            .await
            .map_err(|e| {
                format!(
                    "Upload mapping failed: {} --> {} \n\t> {}",
                    mapping.source, mapping.target, e
                )
            })?;
        // 0=file->file, 1=file->dir, 2=file->missing,
        // 3=dir->dir, 4=dir->missing, 5=contents, 99=error handled above.
        let op: u8 = match (source_kind, remote_kind, mapping.contents_only) {
            (_, Some(RemotePathKind::File), true) => {
                return Err(format!(
                    "Upload mapping failed: {} => {} \n\t> Cannot transfer directory contents into a file",
                    mapping.source, mapping.target
                ));
            }
            (_, _, true) => 5,
            (LocalPathKind::File, Some(RemotePathKind::File), false) => 0,
            (LocalPathKind::File, Some(RemotePathKind::Dir), false) => 1,
            (LocalPathKind::File, None, false) => {
                if mapping.target.ends_with('/') {
                    1
                } else {
                    2
                }
            }
            (LocalPathKind::Dir, Some(RemotePathKind::Dir), false) => 3,
            (LocalPathKind::Dir, Some(RemotePathKind::File), false) => {
                return Err(format!(
                    "Upload mapping failed: {} --> {} \n\t> Cannot transfer a directory onto a file",
                    mapping.source, mapping.target
                ));
            }
            (LocalPathKind::Dir, None, false) => 4,
            (LocalPathKind::Missing, _, _) => {
                return Err(format!(
                    "Upload mapping failed: {} --> {} \n\t> Local source '{}' does not exist",
                    mapping.source, mapping.target, mapping.source
                ));
            }
        };

        let local_clone = local_path.clone();
        let remote_clone = remote_target.clone();
        let config_clone = config.clone();
        let global_server_pool_clone = global_server_pool.clone();
        let server_metadata_clone = server_metadata.clone();
        let source_raw = mapping.source.clone();
        let target_raw = mapping.target.clone();
        let semaphore_clone = semaphore.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = semaphore_clone
                .acquire()
                .await
                .map_err(|e| format!("Transfer semaphore closed: {}", e))?;
            let res = if op == 0 || op == 2 {
                global_server_pool_clone
                    .upload_file_to_file(
                        &server_metadata_clone,
                        TRANSFER_TASK_NAME,
                        Path::new(&local_clone),
                        &remote_clone,
                        config_clone.use_sudo,
                        config_clone.use_rsync,
                        config_clone.silent,
                    )
                    .await
            } else if op == 1 || op == 5 {
                global_server_pool_clone
                    .upload_file_or_dir_contents_into_dir(
                        &server_metadata_clone,
                        TRANSFER_TASK_NAME,
                        Path::new(&local_clone),
                        &remote_clone,
                        None,
                        config_clone.use_sudo,
                        config_clone.use_rsync,
                        config_clone.silent,
                        false,
                        true,
                    )
                    .await
            } else if op == 3 {
                global_server_pool_clone
                    .upload_dir_replace_dir(
                        &server_metadata_clone,
                        TRANSFER_TASK_NAME,
                        Path::new(&local_clone),
                        &remote_clone,
                        config_clone.use_sudo,
                        config_clone.use_rsync,
                        config_clone.silent,
                    )
                    .await
            } else {
                global_server_pool_clone
                    .upload_dir_to_new_path(
                        &server_metadata_clone,
                        TRANSFER_TASK_NAME,
                        Path::new(&local_clone),
                        &remote_clone,
                        config_clone.use_sudo,
                        config_clone.use_rsync,
                        config_clone.silent,
                    )
                    .await
            };
            res.map_err(|e| {
                format!(
                    "Child upload task failed: {} --> {} \n\t> {}",
                    source_raw, target_raw, e
                )
            })
        }));
    }

    for mapping in downloads {
        let remote_raw = global_server_pool
            .resolve_remote_path(
                server_metadata,
                TRANSFER_TASK_NAME,
                config.use_sudo,
                &mapping.source,
            )
            .await
            .map_err(|e| {
                format!(
                    "Download mapping failed: {} --> {} \n\t> {}",
                    mapping.source, mapping.target, e
                )
            })?;
        let remote_source = if remote_raw != "/" {
            remote_raw.trim_end_matches('/').to_string()
        } else {
            remote_raw
        };
        if remote_source.is_empty() {
            return Err(format!(
                "Download mapping failed: {} --> {} \n\t> Failed to resolve remote path",
                mapping.source, mapping.target
            ));
        }
        let remote_kind_opt = global_server_pool
            .remote_path_kind(
                server_metadata,
                TRANSFER_TASK_NAME,
                &remote_source,
                config.use_sudo,
            )
            .await
            .map_err(|e| {
                format!(
                    "Download mapping failed: {} --> {} \n\t> {}",
                    mapping.source, mapping.target, e
                )
            })?;
        let Some(remote_kind) = remote_kind_opt else {
            return Err(format!(
                "Download mapping failed: {} --> {} \n\t> Remote source '{}' does not exist",
                mapping.source, mapping.target, mapping.source
            ));
        };
        if mapping.contents_only && remote_kind != RemotePathKind::Dir {
            return Err(format!(
                "Download mapping failed: {} => {} \n\t> '=>' requires a directory source",
                mapping.source, mapping.target
            ));
        }
        let local_kind = local_path_kind(&mapping.target).map_err(|e| {
            format!(
                "Download mapping failed: {} --> {} \n\t> {}",
                mapping.source, mapping.target, e
            )
        })?;
        let local_target = expand_local_user_path(&mapping.target);
        let dop: u8 = match (remote_kind, local_kind, mapping.contents_only) {
            (_, _, true) if remote_kind == RemotePathKind::File => {
                return Err(format!(
                    "Download mapping failed: {} => {} \n\t> Cannot transfer contents from a file",
                    mapping.source, mapping.target
                ));
            }
            (_, LocalPathKind::File, true) => {
                return Err(format!(
                    "Download mapping failed: {} => {} \n\t> Cannot transfer contents into a file",
                    mapping.source, mapping.target
                ));
            }
            (_, _, true) => 5,
            (RemotePathKind::File, LocalPathKind::File, false) => 0,
            (RemotePathKind::File, LocalPathKind::Dir, false) => 1,
            (RemotePathKind::File, LocalPathKind::Missing, false) => {
                if mapping.target.ends_with('/') || mapping.target.ends_with('\\') {
                    1
                } else {
                    2
                }
            }
            (RemotePathKind::Dir, LocalPathKind::Dir, false) => 3,
            (RemotePathKind::Dir, LocalPathKind::File, false) => {
                return Err(format!(
                    "Download mapping failed: {} --> {} \n\t> Cannot transfer a directory onto a file",
                    mapping.source, mapping.target
                ));
            }
            (RemotePathKind::Dir, LocalPathKind::Missing, false) => 4,
        };

        let remote_clone = remote_source.clone();
        let local_clone = local_target.clone();
        let config_clone = config.clone();
        let global_server_pool_clone = global_server_pool.clone();
        let server_metadata_clone = server_metadata.clone();
        let source_raw = mapping.source.clone();
        let target_raw = mapping.target.clone();
        let semaphore_clone = semaphore.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = semaphore_clone
                .acquire()
                .await
                .map_err(|e| format!("Transfer semaphore closed: {}", e))?;
            let res = if dop == 0 || dop == 2 {
                global_server_pool_clone
                    .download_file_to_file(
                        &server_metadata_clone,
                        TRANSFER_TASK_NAME,
                        &remote_clone,
                        Path::new(&local_clone),
                        config_clone.use_sudo,
                        config_clone.use_rsync,
                        config_clone.silent,
                    )
                    .await
            } else if dop == 1 || dop == 5 {
                global_server_pool_clone
                    .download_file_into_dir(
                        &server_metadata_clone,
                        TRANSFER_TASK_NAME,
                        &remote_clone,
                        Path::new(&local_clone),
                        config_clone.use_sudo,
                        config_clone.use_rsync,
                        config_clone.silent,
                    )
                    .await
            } else if dop == 3 {
                global_server_pool_clone
                    .download_dir_replace_dir(
                        &server_metadata_clone,
                        TRANSFER_TASK_NAME,
                        &remote_clone,
                        Path::new(&local_clone),
                        config_clone.use_sudo,
                        config_clone.use_rsync,
                        config_clone.silent,
                    )
                    .await
            } else {
                global_server_pool_clone
                    .download_dir_to_new_path(
                        &server_metadata_clone,
                        TRANSFER_TASK_NAME,
                        &remote_clone,
                        Path::new(&local_clone),
                        config_clone.use_sudo,
                        config_clone.use_rsync,
                        config_clone.silent,
                    )
                    .await
            };
            res.map_err(|e| {
                format!(
                    "Child download task failed: {} --> {} \n\t> {}",
                    source_raw, target_raw, e
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
