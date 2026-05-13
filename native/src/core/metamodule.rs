/*
 * Modified from KernelSU (https://github.com/tiann/KernelSU)
 * KernelSU is licensed under GPL-3.0
 */

#![allow(dead_code)]

use crate::consts::{MAGISK_VER_CODE, MAGISK_VERSION, METAMODULE, MODULEROOT, MODULEUPGRADE};
use crate::module_config;
use base::{Directory, FsPathBuilder, LoggedResult, ResultExt, Utf8CStr, Utf8CString, cstr, info};
use nix::fcntl::OFlag;
use std::process::{Command, Stdio};

const METAMOUNT_SH: &str = "metamount.sh";
const METAINSTALL_SH: &str = "metainstall.sh";
const METAUNINSTALL_SH: &str = "metauninstall.sh";
const POST_FS_DATA_SH: &str = "post-fs-data.sh";
const SERVICE_SH: &str = "service.sh";
const POST_MOUNT_SH: &str = "post-mount.sh";
const BOOT_COMPLETED_SH: &str = "boot-completed.sh";
const UPDATE_MARKER: &str = "update";
const REMOVE_MARKER: &str = "remove";
const DISABLE_MARKER: &str = "disable";

fn get_magisk_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64-v8a",
        "arm" => "armeabi-v7a",
        "x86_64" => "x86_64",
        "x86" => "x86",
        other => other,
    }
}

fn get_common_envs() -> Vec<(&'static str, String)> {
    vec![
        ("MAGISK", "true".to_string()),
        ("MAGISK_VER", MAGISK_VERSION.to_string()),
        ("MAGISK_VER_CODE", MAGISK_VER_CODE.to_string()),
        ("MAGISK_ARCH", get_magisk_arch().to_string()),
        // KSU compatibility mappings
        ("KSU", "true".to_string()),
        ("KSU_VER", MAGISK_VERSION.to_string()),
        ("KSU_VER_CODE", MAGISK_VER_CODE.to_string()),
        ("KSU_ARCH", get_magisk_arch().to_string()),
        ("KSU_KERNEL_VER_CODE", "0".to_string()),
        ("BOOTMODE", "true".to_string()),
    ]
}

pub fn is_metamodule(module_dir: &Utf8CStr) -> bool {
    let prop_path = cstr::buf::default().join_path(module_dir).join_path("module.prop");
    if let Ok(file) = prop_path.open(OFlag::O_RDONLY | OFlag::O_CLOEXEC) {
        let reader = std::io::BufReader::new(file);
        for line in std::io::BufRead::lines(reader) {
            if let Ok(l) = line {
                let trimmed = l.trim();
                if trimmed == "metamodule=1" || trimmed.eq_ignore_ascii_case("metamodule=true") {
                    return true;
                }
            }
        }
    }
    false
}

pub fn find_metamodule() -> Option<Utf8CString> {
    let mut result = None;
    let _ = || -> LoggedResult<()> {
        let mut root = Directory::open(cstr!(MODULEROOT))?;
        while let Some(e) = root.read()? {
            if !e.is_dir() || e.name() == ".core" {
                continue;
            }
            let dir = e.open_as_dir()?;
            if dir.contains_path(cstr!(REMOVE_MARKER)) || dir.contains_path(cstr!(DISABLE_MARKER)) {
                continue;
            }
            let path = cstr::buf::default().join_path(MODULEROOT).join_path(e.name());
            if is_metamodule(&path) {
                result = Some(path.to_owned());
                return Ok(());
            }
        }
        Ok(())
    }();
    result
}

pub fn has_metamodule() -> bool {
    find_metamodule().is_some()
}

pub fn ensure_metamodule_symlink() {
    let symlink = cstr!(METAMODULE);
    symlink.remove().ok();
    if let Some(target) = find_metamodule() {
        symlink.create_symlink_to(&target).log_ok();
    }
}

pub fn remove_symlink() {
    let symlink = cstr!(METAMODULE);
    if symlink.exists() {
        symlink.remove().ok();
    }
}

pub fn get_metamodule_path() -> Option<Utf8CString> {
    let symlink = cstr!(METAMODULE);
    if symlink.exists() {
        let mut buf = cstr::buf::default();
        if symlink.read_link(&mut buf).is_ok() {
            return Some(buf.to_owned());
        }
    }
    None
}

pub fn get_metamodule_id() -> Option<String> {
    get_metamodule_path()
        .and_then(|p| p.file_name().map(|s| s.to_string()))
}

fn is_metamodule_stable(path: &Utf8CStr) -> bool {
    let dir = match Directory::open(path) {
        Ok(d) => d,
        Err(_) => return false,
    };
    !dir.contains_path(cstr!(UPDATE_MARKER))
        && !dir.contains_path(cstr!(REMOVE_MARKER))
        && !dir.contains_path(cstr!(DISABLE_MARKER))
}

pub fn check_install_safety() -> Result<(), bool> {
    let Some(path) = get_metamodule_path() else {
        return Ok(());
    };

    let has_metainstall = {
        let script = cstr::buf::default().join_path(&path).join_path(METAINSTALL_SH);
        if script.exists() {
            true
        } else {
            // Check staged update dir
            let update_script = cstr::buf::default()
                .join_path(MODULEUPGRADE)
                .join_path(path.file_name().unwrap_or(""))
                .join_path(METAINSTALL_SH);
            update_script.exists()
        }
    };

    if !has_metainstall {
        return Ok(());
    }

    if is_metamodule_stable(&path) {
        return Ok(());
    }

    let dir = Directory::open(&path).ok();
    let is_disabled = dir.as_ref().map_or(false, |d| d.contains_path(cstr!(DISABLE_MARKER)));
    Err(is_disabled)
}

fn check_metamodule_script(name: &str) -> Option<Utf8CString> {
    let path = get_metamodule_path()?;
    let dir = Directory::open(&path).ok()?;
    if dir.contains_path(cstr!(DISABLE_MARKER)) {
        return None;
    }
    let script = cstr::buf::default().join_path(&path).join_path(name);
    if script.exists() {
        Some(script.to_owned())
    } else {
        None
    }
}

// Execute metamodule script with optional environment variables
fn exec_metamodule_script_with_env(stage: &str, extra_envs: Vec<(&str, String)>, block: bool) {
    let Some(script) = check_metamodule_script(stage) else {
        return;
    };

    info!("metamodule: exec {stage}");

    let busybox = cstr!("/data/adb/magisk/busybox");
    if !busybox.exists() {
        // Fallback to exec_script (non-blocking, no env)
        crate::ffi::exec_script(&script);
        return;
    }

    let mut cmd = Command::new(busybox.as_str());
    cmd.arg("sh").arg(script.as_str());

    // Common Magisk / KSU-compatible envs
    for (key, value) in get_common_envs() {
        cmd.env(key, value);
    }

    // Extra per-call envs
    for (key, value) in extra_envs {
        cmd.env(key, value);
    }

    // Metamodule paths
    if let Some(metamodule_path) = get_metamodule_path() {
        cmd.env("MODDIR", metamodule_path.as_str());
    }
    if let Some(module_id) = get_metamodule_id() {
        cmd.env("KSU_MODULE", &module_id);
        cmd.env("MAGISK_MODULE", &module_id);
    }

    if block {
        let _ = cmd.stdout(Stdio::null()).stderr(Stdio::null()).status();
    } else {
        let _ = cmd.stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    }
}

pub fn exec_metamodule_script(stage: &Utf8CStr) {
    exec_metamodule_script_with_env(stage.as_str(), vec![], false);
}

// KSU-compatible: execute metamodule stage script with block option
pub fn exec_stage_script(stage: &str, block: bool) {
    let script_name = format!("{stage}.sh");
    exec_metamodule_script_with_env(&script_name, vec![], block);
}

pub fn exec_metamount() {
    let mut envs = vec![];
    
    // Pass MODULE_DIR to metamount.sh (KSU compatible)
    envs.push(("MODULE_DIR", MODULEROOT.to_string()));
    
    // Pass metamodule directory if available
    if let Some(path) = get_metamodule_path() {
        envs.push(("METAMODULE_DIR", path.to_string()));
    }
    
    exec_metamodule_script_with_env(METAMOUNT_SH, envs, true);
}

pub fn exec_metauninstall_script(module_id: &Utf8CStr) {
    let Some(script) = check_metamodule_script(METAUNINSTALL_SH) else {
        return;
    };

    info!("metamodule: exec metauninstall for {module_id}");

    let busybox = cstr!("/data/adb/magisk/busybox");
    if !busybox.exists() {
        crate::ffi::exec_script(&script);
        return;
    }

    let mut cmd = Command::new(busybox.as_str());
    // Pass module_id as $1 (KSU-compatible) + env
    cmd.arg("sh")
        .arg(script.as_str())
        .arg(module_id.as_str())
        .env("MODULE_ID", module_id.as_str());

    // Common envs
    for (key, value) in get_common_envs() {
        cmd.env(key, value);
    }

    if let Some(metamodule_path) = get_metamodule_path() {
        cmd.env("MODDIR", metamodule_path.as_str());
    }
    if let Some(id) = get_metamodule_id() {
        cmd.env("KSU_MODULE", &id);
        cmd.env("MAGISK_MODULE", &id);
    }

    let _ = cmd.stdout(Stdio::null()).stderr(Stdio::null()).status();
}

pub fn check_metamodule_for_install(module_path: &Utf8CStr) -> bool {
    if !is_metamodule(module_path) {
        return true;
    }
    // Only allow one metamodule
    if let Some(existing) = find_metamodule() {
        if existing != module_path.to_owned() {
            info!("metamodule: already exists, block install");
            return false;
        }
    }
    true
}

pub fn get_metainstall_script() -> Option<Utf8CString> {
    check_metamodule_script(METAINSTALL_SH)
}

// Load metamodule configs into environment
pub fn load_metamodule_config_envs() -> Vec<(&'static str, String)> {
    let mut envs = vec![];
    
    if let Some(module_id) = get_metamodule_id() {
        // Check if metamodule has config for su_compat
        if let Some(value) = module_config::get_config(&module_id, "manage.su_compat", false) {
            envs.push(("KSU_SU_COMPAT", value));
        }
        if let Some(value) = module_config::get_config(&module_id, "manage.kernel_umount", false) {
            envs.push(("KSU_KERNEL_UMOUNT", value));
        }
    }
    
    envs
}
