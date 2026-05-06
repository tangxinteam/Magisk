#![allow(dead_code)]

use crate::consts::{METAMODULE, MODULEROOT, MODULEUPGRADE};
use base::{Directory, FsPathBuilder, LoggedResult, ResultExt, Utf8CStr, Utf8CString, cstr, info};
use nix::fcntl::OFlag;

const METAMOUNT_SH: &str = "metamount.sh";
const METAINSTALL_SH: &str = "metainstall.sh";
const METAUNINSTALL_SH: &str = "metauninstall.sh";
const UPDATE_MARKER: &str = "update";
const REMOVE_MARKER: &str = "remove";
const DISABLE_MARKER: &str = "disable";

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

pub fn exec_metamodule_script(stage: &Utf8CStr) {
    if let Some(script) = check_metamodule_script(stage.as_str()) {
        info!("metamodule: exec {stage}");
        crate::ffi::exec_script(&script);
    }
}

pub fn exec_metamount() {
    if let Some(script) = check_metamodule_script(METAMOUNT_SH) {
        info!("metamodule: exec metamount.sh");
        crate::ffi::exec_script(&script);
    }
}

pub fn exec_metauninstall_script(module_id: &Utf8CStr) {
    if let Some(script) = check_metamodule_script(METAUNINSTALL_SH) {
        info!("metamodule: exec metauninstall.sh for {module_id}");
        let mut cmd = cstr::buf::default().join_path(cstr!("/data/adb/magisk/busybox"));
        if !cmd.exists() {
            cmd = cstr::buf::default().join_path(cstr!("/data/adb/magisk/busybox"));
        }
        let _ = std::process::Command::new(cmd.as_str())
            .args(["sh", script.as_str()])
            .env("MODULE_ID", module_id.as_str())
            .status();
    }
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
