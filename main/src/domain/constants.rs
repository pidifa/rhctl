/*
 * Copyright (C) 2022-2026 rhctl Contributors
 *
 * SPDX-License-Identifier: Apache-2.0
 * **************************************************************************
 * Global constants.
 *
 * Since: 1.0.0
 * Date: October 16, 2025
 */
use std::{collections::HashSet, time::Duration};

pub static REMOTE_TEMP_RHCTL_FOLDER: &str = "/tmp/rhctl";
pub static USER_ABORTED_MESSAGE: &str = "Operation aborted by user";
pub const TRANSFER_TASK_NAME: &str = "TRANSFER";
pub const EXECUTE_TASK_NAME: &str = "EXECUTE";
pub const PATCH_TASK_NAME: &str = "PATCH";
pub const SYSTEM_TASK_NAME: &str = "SYSTEM";

// Log info
pub const LOG_INFO: &str = "INFO";
pub const LOG_ERROR: &str = "ERROR";
pub const LOG_WARN: &str = "WARN";
pub const LOG_DEBUG: &str = "DEBUG";
pub const LOG_REMOTE: &str = "REMOTE";
pub const LOG_LOCAL: &str = "LOCAL";
pub const LOG_ASK: &str = "ASK";
pub const LOG_SHUTDOWN: &str = "SHUTDOWN";

pub const LOG_TASK_NAME_WIDTH: usize = 7;
pub const LOG_LEVEL_WIDTH: usize = 6;

// Default Config
pub const DEFAULT_SSH_PORT: u16 = 22;
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_MAX_CHANNELS_PER_SESSION: usize = 200;
pub const DEFAULT_MAX_SESSIONS_PER_SERVER: usize = 200;
pub const DEFAULT_SESSION_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_MAX_SESSION_LIFETIME: Duration = Duration::from_secs(600);

/// Default maximum number of concurrent file-transfer tasks.
pub const DEFAULT_TRANSFER_CONCURRENCY: usize = 16;

pub const DEFAULT_EXECUTE_WORK_PATH: &str = "~";
pub const DEFAULT_EXECUTE_MODE: &str = "sync";

/// Min seconds between appended REMOTE lines for `\r` progress meters (e.g. curl).
pub const REMOTE_CR_PROGRESS_INTERVAL_SECS: u64 = 30;

// Connection Config
pub const DEFAULT_SSH_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

// Error Handle
pub const SUDO_ERR_MSG: &str = "sudo: a password is required";

pub static DANGEROUS_PATHS: once_cell::sync::Lazy<HashSet<&'static str>> =
    once_cell::sync::Lazy::new(|| {
        HashSet::from([
            // Root / users
            "",
            "/",
            "/home",
            "/root",

            // System configuration
            "/etc",
            "/etc/systemd",
            "/etc/systemd/system",
            "/etc/systemd/user",
            "/etc/ssh",
            "/etc/ssh/sshd_config",
            "/etc/sudoers",
            "/etc/sudoers.d",
            "/etc/cron.d",
            "/etc/cron.daily",
            "/etc/cron.hourly",
            "/etc/cron.monthly",
            "/etc/cron.weekly",
            
            // Network
            "/etc/fstab",     
            "/etc/crypttab",  
            "/etc/hostname",
            "/etc/hosts",
            "/etc/resolv.conf",

            // Executables / libraries
            "/usr",
            "/usr/bin",
            "/usr/sbin",
            "/usr/lib",
            "/usr/lib64",
            "/bin",
            "/sbin",
            "/lib",
            "/lib64",

            // Runtime / boot
            "/boot",
            "/boot/efi",
            "/dev",
            "/proc",
            "/sys",
            "/run",

            // System state / logs
            "/var",
            "/var/lib",
            "/var/lib/systemd",
            "/var/lib/dpkg",
            "/var/lib/rpm",
            "/var/log",

            // Temporary / mounts
            "/tmp",
            "/mnt",
            "/media",
            "/opt",

            // Network configuration
            "/etc/network",
            "/etc/netplan",
            "/etc/NetworkManager",

            // Package / service configuration
            "/etc/default",
            "/etc/init.d",

            // Shell / environment configuration
            "/etc/profile",
            "/etc/profile.d",

            // Wildcard / home shortcut
            "~",
        ])
    });