use crate::consts::{METAMODULE, MODULEROOT};
use base::{Directory, FsPathBuilder, LoggedResult, ResultExt, Utf8CStr, Utf8CStrBuf, Utf8CString, cstr, info};

pub fn is_metamodule(module_dir: &Utf8CStr) -> bool {
    let mut prop_path = cstr::buf::default().join_path(module_dir).join_path("module.prop");
    if let Ok(file) = prop_path.open(nix::fcntl::OFlag::O_RDONLY | nix::fcntl::OFlag::O_CLOEXEC) {
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
        let root = Directory::open(cstr!(MODULEROOT))?;
        while let Some(e) = root.read()? {
            if !e.is_dir() || e.name() == ".core" {
                continue;
            }
            let dir = e.open_as_dir()?;
            if dir.contains_path(cstr!("remove")) || dir.contains_path(cstr!("disable")) {
                continue;
            }
            let mut path = cstr::buf::default().join_path(MODULEROOT).join_path(e.name());
            if is_metamodule(&path) {
                result = Some(path.to_owned());
                return Ok(());
            }
        }
        Ok(())
    }();
    result
}

pub fn ensure_metamodule_symlink() {
    let symlink = cstr!(METAMODULE);
    symlink.remove().ok();
    if let Some(target) = find_metamodule() {
        symlink.create_symlink_to(&target).log_ok();
    }
}

pub fn get_metamodule_path() -> Option<Utf8CStrBuf> {
    let symlink = cstr!(METAMODULE);
    if symlink.exists() {
        let mut buf = cstr::buf::default();
        if symlink.read_link(&mut buf).is_ok() {
            return Some(buf);
        }
    }
    None
}

pub fn exec_metamodule_script(stage: &Utf8CStr) {
    if let Some(path) = get_metamodule_path() {
        let script = cstr::buf::default().join_path(&path).join_path(stage);
        if script.exists() {
            info!("metamodule: exec {stage}");
            crate::ffi::exec_script(&script);
        }
    }
}

pub fn exec_metamount() {
    exec_metamodule_script(cstr!("metamount.sh"));
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
