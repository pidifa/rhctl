/*
 * Copyright (C) 2022-2026 rhctl Contributors
 *
 * SPDX-License-Identifier: Apache-2.0
 * **************************************************************************
 * Parse the original Cli paramters and YAML configuration file.
 *
 * Since: 1.0.0
 * Date: October 16, 2025
 */
use std::collections::{HashMap, HashSet};
use std::process::exit;
use std::time::Duration;

use crate::common::ssh_pool::ServerPool;
use crate::domain::cmd_params::{
    parse_script_invocation, ExecuteCmdConfig, PatchCmdConfig, ServerMetadata, TransferCmdConfig,
};
use crate::domain::constants::{
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_EXECUTE_MODE, DEFAULT_EXECUTE_WORK_PATH,
    DEFAULT_MAX_CHANNELS_PER_SESSION, DEFAULT_MAX_SESSIONS_PER_SERVER,
    DEFAULT_MAX_SESSION_LIFETIME, DEFAULT_SESSION_ACQUIRE_TIMEOUT, DEFAULT_SSH_PORT,
    EXECUTE_TASK_NAME, PATCH_TASK_NAME, TRANSFER_TASK_NAME,
};
use crate::domain::yml_config::{
    ExecuteStep, NamedConfig, PatchStep, RunStep, ServerConfig, TransferStep, YmlConfig,
};
use crate::utils::file_utils::{expand_vars, substitute_vars, validate_path_chars};
use crate::utils::log_utils::prompt_password_or_exit;
use crate::{log_error_direct, log_error_with_host_direct, log_warn_direct, log_warn_root};

fn resolve_script_invocation(
    raw: &str,
    vars: &HashMap<String, String>,
) -> Result<crate::domain::cmd_params::ScriptInvocation, String> {
    let expanded = expand_vars(raw, vars)?;
    let inv = parse_script_invocation(&expanded)?;
    validate_path_chars(&inv.path)?;
    Ok(inv)
}
fn resolve_auth(
    password: Option<String>,
    identity_file: Option<String>,
    certificate_file: Option<String>,
    user: &str,
    host: &str,
    task_name: &str,
) -> (String, Option<String>, Option<String>) {
    if certificate_file.is_some() && identity_file.is_none() {
        log_error_with_host_direct!(
            user,
            host,
            task_name,
            "OpenSSH certificate requires an identity file (--identity / identity-file)."
        );
        exit(1);
    }
    let password = if identity_file.is_some() {
        password.unwrap_or_default()
    } else {
        password.unwrap_or_else(|| prompt_password_or_exit(user, host, task_name))
    };
    (password, identity_file, certificate_file)
}
fn build_server_metadata(
    host: &str,
    user: &str,
    ssh_port: u16,
    password: String,
    identity_file: Option<String>,
    certificate_file: Option<String>,
    connect_timeout: Duration,
    max_channels_per_session: usize,
    max_sessions_per_server: usize,
    session_acquire_timeout: Duration,
    max_session_lifetime: Duration,
) -> ServerMetadata {
    ServerMetadata {
        host: host.to_string(),
        user: user.to_string(),
        ssh_port,
        password,
        identity_file,
        certificate_file,
        server_key: ServerPool::generate_server_key(host, ssh_port, user),
        connect_timeout,
        max_channels_per_session,
        max_sessions_per_server,
        session_acquire_timeout,
        max_session_lifetime,
    }
}
pub fn parse_patch_config_from_cmd(
    host: &str,
    user: &str,
    ssh_port: Option<u16>,
    password: Option<String>,
    identity_file: Option<String>,
    certificate_file: Option<String>,
    recover: bool,
    local_path: &str,
    remote_upload: &str,
    remote_path: &str,
    remote_backup: &str,
    use_sudo: bool,
    use_rsync: bool,
    silent: bool,
    connect_timeout: Option<Duration>,
    max_channels_per_session: Option<usize>,
    max_sessions_per_server: Option<usize>,
    session_acquire_timeout: Option<Duration>,
    max_session_lifetime: Option<Duration>,
    vars: &HashMap<String, String>,
) -> PatchCmdConfig {
    let ssh_port = ssh_port.unwrap_or(DEFAULT_SSH_PORT);
    let (password, identity_file, certificate_file) = resolve_auth(
        password,
        identity_file,
        certificate_file,
        user,
        host,
        PATCH_TASK_NAME,
    );
    PatchCmdConfig {
        server_metadata: build_server_metadata(
            host,
            user,
            ssh_port,
            password,
            identity_file,
            certificate_file,
            connect_timeout.unwrap_or(DEFAULT_CONNECT_TIMEOUT),
            max_channels_per_session.unwrap_or(DEFAULT_MAX_CHANNELS_PER_SESSION),
            max_sessions_per_server.unwrap_or(DEFAULT_MAX_SESSIONS_PER_SERVER),
            session_acquire_timeout.unwrap_or(DEFAULT_SESSION_ACQUIRE_TIMEOUT),
            max_session_lifetime.unwrap_or(DEFAULT_MAX_SESSION_LIFETIME),
        ),
        use_sudo,
        use_rsync,
        silent,
        recover,
        local_path: substitute_vars(&local_path, &vars).unwrap_or_else(|e| {
            log_error_with_host_direct!(&user, &host, PATCH_TASK_NAME, "{}", e);
            exit(1);
        }),
        remote_upload: substitute_vars(&remote_upload, &vars).unwrap_or_else(|e| {
            log_error_with_host_direct!(&user, &host, PATCH_TASK_NAME, "{}", e);
            exit(1);
        }),
        remote_path: substitute_vars(&remote_path, &vars).unwrap_or_else(|e| {
            log_error_with_host_direct!(&user, &host, PATCH_TASK_NAME, "{}", e);
            exit(1);
        }),
        remote_backup: substitute_vars(&remote_backup, &vars).unwrap_or_else(|e| {
            log_error_with_host_direct!(&user, &host, PATCH_TASK_NAME, "{}", e);
            exit(1);
        }),
    }
}
pub fn parse_execute_config_from_cmd(
    host: &str,
    user: &str,
    ssh_port: Option<u16>,
    password: Option<String>,
    identity_file: Option<String>,
    certificate_file: Option<String>,
    script: Vec<String>,
    cmds: Vec<String>,
    env_extract_regex: Vec<String>,
    env_name: Vec<String>,
    env_write_file: Option<String>,
    work_path: Option<String>,
    mode: Option<String>,
    use_sudo: bool,
    use_rsync: bool,
    silent: bool,
    connect_timeout: Option<Duration>,
    max_channels_per_session: Option<usize>,
    max_sessions_per_server: Option<usize>,
    session_acquire_timeout: Option<Duration>,
    max_session_lifetime: Option<Duration>,
    vars: &HashMap<String, String>,
) -> ExecuteCmdConfig {
    let ssh_port = ssh_port.unwrap_or(DEFAULT_SSH_PORT);
    let (password, identity_file, certificate_file) = resolve_auth(
        password,
        identity_file,
        certificate_file,
        user,
        host,
        EXECUTE_TASK_NAME,
    );
    if script.is_empty() && cmds.is_empty() {
        log_error_with_host_direct!(
            user,
            host,
            EXECUTE_TASK_NAME,
            "Provide at least one --script or --cmd"
        );
        exit(1);
    }
    let env_write_file = env_write_file
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .map(|p| substitute_vars(&p, vars).unwrap_or_else(|e| {
            log_error_with_host_direct!(user, host, EXECUTE_TASK_NAME, "{}", e);
            exit(1);
        }));
    if let Err(e) = crate::commands::execute::validate_env_extract_options(
        &env_extract_regex,
        &env_name,
        env_write_file.as_deref(),
    ) {
        log_error_with_host_direct!(user, host, EXECUTE_TASK_NAME, "{}", e);
        exit(1);
    }
    ExecuteCmdConfig {
        server_metadata: build_server_metadata(
            host,
            user,
            ssh_port,
            password,
            identity_file,
            certificate_file,
            connect_timeout.unwrap_or(DEFAULT_CONNECT_TIMEOUT),
            max_channels_per_session.unwrap_or(DEFAULT_MAX_CHANNELS_PER_SESSION),
            max_sessions_per_server.unwrap_or(DEFAULT_MAX_SESSIONS_PER_SERVER),
            session_acquire_timeout.unwrap_or(DEFAULT_SESSION_ACQUIRE_TIMEOUT),
            max_session_lifetime.unwrap_or(DEFAULT_MAX_SESSION_LIFETIME),
        ),
        use_sudo,
        use_rsync,
        silent,
        scripts: script
            .into_iter()
            .map(|s| {
                resolve_script_invocation(&s, vars).unwrap_or_else(|e| {
                    log_error_with_host_direct!(user, host, EXECUTE_TASK_NAME, "{}", e);
                    exit(1);
                })
            })
            .collect(),
        cmds,
        env_extract_regexes: env_extract_regex,
        env_names: env_name,
        env_write_file,
        mode: mode.unwrap_or(DEFAULT_EXECUTE_MODE.to_string()),
        work_path: substitute_vars(
            &work_path.unwrap_or_else(|| DEFAULT_EXECUTE_WORK_PATH.to_string()),
            vars,
        )
        .unwrap_or_else(|e| {
            log_error_with_host_direct!(user, host, EXECUTE_TASK_NAME, "{}", e);
            exit(1);
        }),
    }
}
pub fn parse_transfer_config_from_cmd(
    host: &str,
    user: &str,
    ssh_port: Option<u16>,
    password: Option<String>,
    identity_file: Option<String>,
    certificate_file: Option<String>,
    upload_mapping: Option<String>,
    uploads: Vec<String>,
    download_mapping: Option<String>,
    downloads: Vec<String>,
    patch_envs: Vec<String>,
    use_sudo: bool,
    use_rsync: bool,
    silent: bool,
    connect_timeout: Option<Duration>,
    max_channels_per_session: Option<usize>,
    max_sessions_per_server: Option<usize>,
    session_acquire_timeout: Option<Duration>,
    max_session_lifetime: Option<Duration>,
    vars: &HashMap<String, String>,
) -> TransferCmdConfig {
    let ssh_port = ssh_port.unwrap_or(DEFAULT_SSH_PORT);
    let (password, identity_file, certificate_file) = resolve_auth(
        password,
        identity_file,
        certificate_file,
        user,
        host,
        TRANSFER_TASK_NAME,
    );
    let has_upload = upload_mapping.is_some() || !uploads.is_empty();
    let has_download = download_mapping.is_some() || !downloads.is_empty();
    if !has_upload && !has_download && patch_envs.is_empty() {
        log_error_with_host_direct!(
            user,
            host,
            TRANSFER_TASK_NAME,
            "Provide at least one of --upload or --download or --patch-env"
        );
        exit(1);
    }
    TransferCmdConfig {
        server_metadata: build_server_metadata(
            host,
            user,
            ssh_port,
            password,
            identity_file,
            certificate_file,
            connect_timeout.unwrap_or(DEFAULT_CONNECT_TIMEOUT),
            max_channels_per_session.unwrap_or(DEFAULT_MAX_CHANNELS_PER_SESSION),
            max_sessions_per_server.unwrap_or(DEFAULT_MAX_SESSIONS_PER_SERVER),
            session_acquire_timeout.unwrap_or(DEFAULT_SESSION_ACQUIRE_TIMEOUT),
            max_session_lifetime.unwrap_or(DEFAULT_MAX_SESSION_LIFETIME),
        ),
        use_sudo,
        use_rsync,
        silent,
        upload_mapping: upload_mapping.map(|f| {
            substitute_vars(&f, vars).unwrap_or_else(|e| {
                log_error_with_host_direct!(user, host, TRANSFER_TASK_NAME, "{}", e);
                exit(1);
            })
        }),
        uploads,
        download_mapping: download_mapping.map(|f| {
            substitute_vars(&f, vars).unwrap_or_else(|e| {
                log_error_with_host_direct!(user, host, TRANSFER_TASK_NAME, "{}", e);
                exit(1);
            })
        }),
        downloads,
        patch_envs,
    }
}
fn server_metadata_from_yml(
    server: &ServerConfig,
    common: &Option<crate::domain::yml_config::CommonConfig>,
    task_name: &str,
) -> ServerMetadata {
    let ssh_port = server.ssh_port.unwrap_or(DEFAULT_SSH_PORT);
    let (password, identity_file, certificate_file) = resolve_auth(
        server.password.clone(),
        server.identity_file.clone(),
        server.certificate_file.clone(),
        &server.user,
        &server.host,
        task_name,
    );
    build_server_metadata(
        &server.host,
        &server.user,
        ssh_port,
        password,
        identity_file,
        certificate_file,
        server
            .connect_timeout
            .or_else(|| common.as_ref()?.server.as_ref()?.connect_timeout)
            .unwrap_or(DEFAULT_CONNECT_TIMEOUT),
        server
            .max_channels_per_session
            .or_else(|| common.as_ref()?.server.as_ref()?.max_channels_per_session)
            .unwrap_or(DEFAULT_MAX_CHANNELS_PER_SESSION),
        server
            .max_sessions_per_server
            .or_else(|| common.as_ref()?.server.as_ref()?.max_sessions_per_server)
            .unwrap_or(DEFAULT_MAX_SESSIONS_PER_SERVER),
        server
            .session_acquire_timeout
            .or_else(|| common.as_ref()?.server.as_ref()?.session_acquire_timeout)
            .unwrap_or(DEFAULT_SESSION_ACQUIRE_TIMEOUT),
        server
            .max_session_lifetime
            .or_else(|| common.as_ref()?.server.as_ref()?.max_session_lifetime)
            .unwrap_or(DEFAULT_MAX_SESSION_LIFETIME),
    )
}
pub enum ParsedRunStep {
    Transfer(Vec<(TransferCmdConfig, HashMap<String, String>)>),
    Execute(Vec<(ExecuteCmdConfig, HashMap<String, String>)>),
    Patch(Vec<(PatchCmdConfig, HashMap<String, String>)>),
}
fn merge_var_maps(
    global: &HashMap<String, String>,
    config: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut merged = global.clone();
    for (k, v) in config {
        merged.insert(k.clone(), v.clone());
    }
    merged
}
fn effective_targets<'a>(
    named_config: &'a NamedConfig,
    step_servers: &'a [String],
    step_groups: &'a [String],
) -> (&'a [String], &'a [String]) {
    let servers = if step_servers.is_empty() {
        named_config.target_servers.as_slice()
    } else {
        step_servers
    };
    let groups = if step_groups.is_empty() {
        named_config.target_groups.as_slice()
    } else {
        step_groups
    };
    (servers, groups)
}
pub fn parse_run_steps(
    named_config: &NamedConfig,
    yml_config: &YmlConfig,
    failed_servers: &HashSet<String>,
    server_config_map: &HashMap<String, ServerConfig>,
) -> Vec<ParsedRunStep> {
    if named_config.runs.is_empty() {
        log_error_direct!(
            "Config '{}' has no runs. Add ordered transfer/execute/patch steps under `runs:`.",
            named_config.name
        );
        exit(1);
    }
    let var_map = merge_var_maps(&yml_config.var_map, &named_config.var_map);
    let common = &yml_config.common;
    let mut steps = Vec::with_capacity(named_config.runs.len());
    for (idx, run) in named_config.runs.iter().enumerate() {
        let step_no = idx + 1;
        match run {
            RunStep::Transfer(transfer) => {
                let servers = collect_servers_for_step(
                    server_config_map,
                    yml_config,
                    failed_servers,
                    named_config,
                    &transfer.target_servers,
                    &transfer.target_groups,
                    TRANSFER_TASK_NAME,
                    step_no,
                );
                steps.push(ParsedRunStep::Transfer(build_transfer_step_configs(
                    named_config,
                    transfer,
                    &servers,
                    common,
                    &var_map,
                )));
            }
            RunStep::Execute(execute) => {
                let servers = collect_servers_for_step(
                    server_config_map,
                    yml_config,
                    failed_servers,
                    named_config,
                    &execute.target_servers,
                    &execute.target_groups,
                    EXECUTE_TASK_NAME,
                    step_no,
                );
                steps.push(ParsedRunStep::Execute(build_execute_step_configs(
                    named_config,
                    execute,
                    &servers,
                    common,
                    &var_map,
                )));
            }
            RunStep::Patch(patch) => {
                let servers = collect_servers_for_step(
                    server_config_map,
                    yml_config,
                    failed_servers,
                    named_config,
                    &patch.target_servers,
                    &patch.target_groups,
                    PATCH_TASK_NAME,
                    step_no,
                );
                steps.push(ParsedRunStep::Patch(build_patch_step_configs(
                    named_config,
                    patch,
                    &servers,
                    common,
                    &var_map,
                )));
            }
        }
    }
    steps
}
fn build_transfer_step_configs(
    named_config: &NamedConfig,
    transfer: &TransferStep,
    servers: &[ServerConfig],
    common: &Option<crate::domain::yml_config::CommonConfig>,
    var_map: &HashMap<String, String>,
) -> Vec<(TransferCmdConfig, HashMap<String, String>)> {
    let mut configs = Vec::new();
    for server in servers {
        let has_upload = transfer.upload_mapping.is_some() || !transfer.uploads.is_empty();
        let has_download = transfer.download_mapping.is_some() || !transfer.downloads.is_empty();
        if !has_upload && !has_download {
            log_error_with_host_direct!(
                &server.user,
                &server.host,
                TRANSFER_TASK_NAME,
                "Transfer step requires upload and/or download mappings"
            );
            exit(1);
        }
        if !transfer.patch_envs.is_empty() && !has_upload {
            log_error_with_host_direct!(
                &server.user,
                &server.host,
                TRANSFER_TASK_NAME,
                "--patch-env can only be used with upload mappings"
            );
            exit(1);
        }
        let upload_mapping = transfer.upload_mapping.as_ref().map(|f| {
            substitute_vars(f, var_map).unwrap_or_else(|e| {
                log_error_with_host_direct!(
                    &server.user,
                    &server.host,
                    TRANSFER_TASK_NAME,
                    "{}",
                    e
                );
                exit(1);
            })
        });
        let download_mapping = transfer.download_mapping.as_ref().map(|f| {
            substitute_vars(f, var_map).unwrap_or_else(|e| {
                log_error_with_host_direct!(
                    &server.user,
                    &server.host,
                    TRANSFER_TASK_NAME,
                    "{}",
                    e
                );
                exit(1);
            })
        });
        configs.push((
            TransferCmdConfig {
                server_metadata: server_metadata_from_yml(server, common, TRANSFER_TASK_NAME),
                use_sudo: transfer.use_sudo.or(named_config.use_sudo).unwrap_or(false),
                use_rsync: transfer
                    .use_rsync
                    .or(named_config.use_rsync)
                    .unwrap_or(false),
                silent: transfer.silent.or(named_config.silent).unwrap_or(false),
                upload_mapping,
                uploads: transfer.uploads.clone(),
                download_mapping,
                downloads: transfer.downloads.clone(),
                patch_envs: transfer.patch_envs.clone(),
            },
            var_map.clone(),
        ));
    }
    configs
}

/// Resolve `env-write-file` for a YAML execute step: trim it, substitute
/// `${VAR}` placeholders, then validate the whole env-extract option set.
///
/// Validation runs *after* substitution so a placeholder like `${ENV_FILE}`
/// can expand to an absolute path before it is checked.
/// Returns the resolved path (`None` when the step has no `env-write-file`).
fn resolve_step_env_options(
    execute: &ExecuteStep,
    var_map: &HashMap<String, String>,
) -> Result<Option<String>, String> {
    let write_file = execute
        .env_write_file
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| substitute_vars(p, var_map))
        .transpose()?;
    crate::commands::execute::validate_env_extract_options(
        &execute.env_extract_regex,
        &execute.env_name,
        write_file.as_deref(),
    )?;
    Ok(write_file)
}

fn build_execute_step_configs(
    named_config: &NamedConfig,
    execute: &ExecuteStep,
    servers: &[ServerConfig],
    common: &Option<crate::domain::yml_config::CommonConfig>,
    var_map: &HashMap<String, String>,
) -> Vec<(ExecuteCmdConfig, HashMap<String, String>)> {
    if execute.scripts.is_empty() && execute.cmds.is_empty() {
        log_error_direct!(
            "Execute step in config '{}' requires scripts and/or cmds",
            named_config.name
        );
        exit(1);
    }
    let env_write_file = resolve_step_env_options(execute, var_map).unwrap_or_else(|e| {
        log_error_direct!("Execute step in config '{}': {}", named_config.name, e);
        exit(1);
    });
    let mut configs = Vec::new();
    for server in servers {
        configs.push((
            ExecuteCmdConfig {
                server_metadata: server_metadata_from_yml(server, common, EXECUTE_TASK_NAME),
                use_sudo: execute.use_sudo.or(named_config.use_sudo).unwrap_or(false),
                use_rsync: execute
                    .use_rsync
                    .or(named_config.use_rsync)
                    .unwrap_or(false),
                silent: execute.silent.or(named_config.silent).unwrap_or(false),
                scripts: execute
                    .scripts
                    .clone()
                    .into_iter()
                    .map(|s| {
                        resolve_script_invocation(&s, var_map).unwrap_or_else(|e| {
                            log_error_with_host_direct!(
                                &server.user,
                                &server.host,
                                EXECUTE_TASK_NAME,
                                "{}",
                                e
                            );
                            exit(1);
                        })
                    })
                    .collect(),
                cmds: execute.cmds.clone(),
                env_extract_regexes: execute.env_extract_regex.clone(),
                env_names: execute.env_name.clone(),
                env_write_file: env_write_file.clone(),
                mode: execute
                    .mode
                    .clone()
                    .unwrap_or(DEFAULT_EXECUTE_MODE.to_string()),
                work_path: substitute_vars(
                    &execute
                        .work_path
                        .clone()
                        .unwrap_or_else(|| DEFAULT_EXECUTE_WORK_PATH.to_string()),
                    var_map,
                )
                .unwrap_or_else(|e| {
                    log_error_with_host_direct!(
                        &server.user,
                        &server.host,
                        EXECUTE_TASK_NAME,
                        "{}",
                        e
                    );
                    exit(1);
                }),
            },
            var_map.clone(),
        ));
    }
    configs
}
fn build_patch_step_configs(
    named_config: &NamedConfig,
    patch: &PatchStep,
    servers: &[ServerConfig],
    common: &Option<crate::domain::yml_config::CommonConfig>,
    var_map: &HashMap<String, String>,
) -> Vec<(PatchCmdConfig, HashMap<String, String>)> {
    let mut configs = Vec::new();
    for server in servers {
        configs.push((
            PatchCmdConfig {
                server_metadata: server_metadata_from_yml(server, common, PATCH_TASK_NAME),
                use_sudo: patch.use_sudo.or(named_config.use_sudo).unwrap_or(false),
                use_rsync: patch.use_rsync.or(named_config.use_rsync).unwrap_or(false),
                silent: patch.silent.or(named_config.silent).unwrap_or(false),
                recover: patch.recover,
                local_path: substitute_vars(&patch.local_path, var_map).unwrap_or_else(|e| {
                    log_error_with_host_direct!(
                        &server.user,
                        &server.host,
                        PATCH_TASK_NAME,
                        "{}",
                        e
                    );
                    exit(1);
                }),
                remote_upload: substitute_vars(&patch.remote_upload, var_map).unwrap_or_else(|e| {
                    log_error_with_host_direct!(
                        &server.user,
                        &server.host,
                        PATCH_TASK_NAME,
                        "{}",
                        e
                    );
                    exit(1);
                }),
                remote_path: substitute_vars(&patch.remote_path, var_map).unwrap_or_else(|e| {
                    log_error_with_host_direct!(
                        &server.user,
                        &server.host,
                        PATCH_TASK_NAME,
                        "{}",
                        e
                    );
                    exit(1);
                }),
                remote_backup: substitute_vars(&patch.remote_backup, var_map).unwrap_or_else(|e| {
                    log_error_with_host_direct!(
                        &server.user,
                        &server.host,
                        PATCH_TASK_NAME,
                        "{}",
                        e
                    );
                    exit(1);
                }),
            },
            var_map.clone(),
        ));
    }
    configs
}
fn collect_servers_for_step(
    server_map: &HashMap<String, ServerConfig>,
    yml_config: &YmlConfig,
    failed_servers: &HashSet<String>,
    named_config: &NamedConfig,
    step_servers: &[String],
    step_groups: &[String],
    task_name: &str,
    step_no: usize,
) -> Vec<ServerConfig> {
    let (target_servers, target_groups) =
        effective_targets(named_config, step_servers, step_groups);
    if target_servers.is_empty() && target_groups.is_empty() {
        log_error_direct!(
            "Config '{}' step {} ({}) has no target-servers or target-groups",
            named_config.name,
            step_no,
            task_name
        );
        exit(1);
    }
    let mut servers: HashSet<ServerConfig> = HashSet::new();
    for server_name in target_servers {
        if failed_servers.contains(server_name) {
            log_warn_root!(
                "Skip failed server '{}' for {} step {} in config '{}'",
                server_name,
                task_name,
                step_no,
                named_config.name
            );
            continue;
        }
        match server_map.get(server_name) {
            Some(server) => {
                servers.insert(server.clone());
            }
            None => {
                log_error_direct!("Server '{}' not found in servers list", server_name);
                exit(1);
            }
        }
    }
    if let Some(group_map) = &yml_config.group_map {
        for group_name in target_groups {
            match group_map.get(group_name) {
                Some(group_servers) => {
                    for server_name in group_servers {
                        if failed_servers.contains(server_name) {
                            log_warn_direct!(
                                "Skip failed server '{}' for {} step {} in config '{}'",
                                server_name,
                                task_name,
                                step_no,
                                named_config.name
                            );
                            continue;
                        }
                        match server_map.get(server_name) {
                            Some(server) => {
                                servers.insert(server.clone());
                            }
                            None => {
                                log_error_direct!(
                                    "Server '{}' in group '{}' not found in servers list",
                                    server_name,
                                    group_name
                                );
                                exit(1);
                            }
                        }
                    }
                }
                None => {
                    log_error_direct!("Group '{}' not found in group_map list", group_name);
                    exit(1);
                }
            }
        }
    }
    if servers.is_empty() {
        log_error_direct!(
            "Config '{}' step {} ({}) resolved to no reachable servers",
            named_config.name,
            step_no,
            task_name
        );
        exit(1);
    }
    servers.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(
        regexes: &[&str],
        names: &[&str],
        write_file: Option<&str>,
    ) -> crate::domain::yml_config::ExecuteStep {
        crate::domain::yml_config::ExecuteStep {
            env_extract_regex: regexes.iter().map(|s| s.to_string()).collect(),
            env_name: names.iter().map(|s| s.to_string()).collect(),
            env_write_file: write_file.map(|s| s.to_string()),
            ..Default::default()
        }
    }

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn step_env_options_ok_when_nothing_set() {
        let res = resolve_step_env_options(&step(&[], &[], None), &vars(&[]));
        assert!(res.is_ok(), "{:?}", res);
        assert!(res.unwrap().is_none());
    }

    #[test]
    fn step_env_options_require_write_file_with_regex() {
        let res = resolve_step_env_options(&step(&["URL=(.*)"], &["DB_URL"], None), &vars(&[]));
        assert!(res.is_err(), "expected error, got {:?}", res);
    }

    #[test]
    fn step_env_options_treat_blank_write_file_as_missing() {
        let res = resolve_step_env_options(
            &step(&["URL=(.*)"], &["DB_URL"], Some("   ")),
            &vars(&[]),
        );
        assert!(res.is_err(), "expected error, got {:?}", res);
    }

    #[test]
    fn step_env_options_write_file_requires_regex() {
        let res =
            resolve_step_env_options(&step(&[], &[], Some("/etc/myapp/.env")), &vars(&[]));
        assert!(res.is_err(), "expected error, got {:?}", res);
    }

    #[test]
    fn step_env_options_substitute_before_validate() {
        let res = resolve_step_env_options(
            &step(&["URL=(.*)"], &["DB_URL"], Some("${ENV_FILE}")),
            &vars(&[("ENV_FILE", "/etc/myapp/.env")]),
        );
        assert_eq!(res.unwrap().unwrap(), "/etc/myapp/.env");
    }

    #[test]
    fn step_env_options_reject_relative_after_substitution() {
        let res = resolve_step_env_options(
            &step(&["URL=(.*)"], &["DB_URL"], Some("${ENV_FILE}")),
            &vars(&[("ENV_FILE", "app/.env")]),
        );
        assert!(res.is_err(), "expected error, got {:?}", res);
    }
}

