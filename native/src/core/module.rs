use crate::consts::{MODULEROOT, MODULEUPGRADE};
use crate::daemon::MagiskD;
use crate::ffi::{ModuleInfo, exec_module_scripts, exec_script};
use crate::metamodule::{exec_metauninstall_script, remove_symlink};
use crate::resetprop::load_prop_file;
use base::{
    DirEntry, Directory, FsPathBuilder, LoggedResult, ResultExt, SilentLogExt, Utf8CStr,
    cstr, info,
};
use nix::fcntl::OFlag;
use nix::unistd::UnlinkatFlags;

fn upgrade_modules() -> LoggedResult<()> {
    let mut upgrade = Directory::open(cstr!(MODULEUPGRADE)).silent()?;
    let root = Directory::open(cstr!(MODULEROOT))?;
    while let Some(e) = upgrade.read()? {
        if !e.is_dir() {
            continue;
        }
        let module_name = e.name();
        let mut disable = false;
        // Cleanup old module if exists
        if root.contains_path(module_name) {
            let module = root.open_as_dir_at(module_name)?;
            // If the old module is disabled, we need to also disable the new one
            disable = module.contains_path(cstr!("disable"));
            module.remove_all()?;
            root.unlink_at(module_name, UnlinkatFlags::RemoveDir)?;
        }
        info!("Upgrade / New module: {module_name}");
        e.rename_to(&root, module_name)?;
        if disable {
            let path = cstr::buf::default()
                .join_path(module_name)
                .join_path("disable");
            let _ = root.open_as_file_at(
                &path,
                OFlag::O_RDONLY | OFlag::O_CREAT | OFlag::O_CLOEXEC,
                0,
            )?;
        }
    }
    upgrade.remove_all()?;
    cstr!(MODULEUPGRADE).remove()?;
    Ok(())
}

fn for_each_module(mut func: impl FnMut(&DirEntry) -> LoggedResult<()>) -> LoggedResult<()> {
    let mut root = Directory::open(cstr!(MODULEROOT))?;
    while let Some(ref e) = root.read()? {
        if e.is_dir() && e.name() != ".core" {
            func(e)?;
        }
    }
    Ok(())
}

pub fn disable_modules() {
    for_each_module(|e| {
        let dir = e.open_as_dir()?;
        dir.open_as_file_at(
            cstr!("disable"),
            OFlag::O_RDONLY | OFlag::O_CREAT | OFlag::O_CLOEXEC,
            0,
        )?;
        Ok(())
    })
    .log_ok();
}

fn run_uninstall_script(module_name: &Utf8CStr) {
    let script = cstr::buf::default()
        .join_path(MODULEROOT)
        .join_path(module_name)
        .join_path("uninstall.sh");
    exec_script(&script);
}

fn is_metamodule_path(path: &Utf8CStr) -> bool {
    crate::metamodule::is_metamodule(path)
}

pub fn remove_modules() {
    for_each_module(|e| {
        let dir = e.open_as_dir()?;
        let name = e.name();
        if dir.contains_path(cstr!("uninstall.sh")) {
            run_uninstall_script(name);
        }
        // If this is the metamodule, remove symlink
        let path = cstr::buf::default().join_path(MODULEROOT).join_path(name);
        if is_metamodule_path(&path) {
            remove_symlink();
        }
        Ok(())
    })
    .log_ok();
    cstr!(MODULEROOT).remove_all().log_ok();
}

fn collect_modules() -> Vec<ModuleInfo> {
    let mut modules = Vec::new();

    for_each_module(|e| {
        let name = e.name();
        let dir = e.open_as_dir()?;
        if dir.contains_path(cstr!("remove")) {
            info!("{name}: remove");
            exec_metauninstall_script(name);
            if dir.contains_path(cstr!("uninstall.sh")) {
                run_uninstall_script(name);
            }
        let path = cstr::buf::default().join_path(MODULEROOT).join_path(name);
            if is_metamodule_path(&path) {
                remove_symlink();
            }
            dir.remove_all()?;
            e.unlink()?;
            return Ok(());
        }
        dir.unlink_at(cstr!("update"), UnlinkatFlags::NoRemoveDir)
            .ok();
        if dir.contains_path(cstr!("disable")) {
            return Ok(());
        }

        modules.push(ModuleInfo {
            name: name.to_string(),
        });
        Ok(())
    })
    .log_ok();

    modules
}

impl MagiskD {
    pub fn handle_modules(&self) {
        upgrade_modules().ok();

        let modules = collect_modules();

        // Load module props before executing scripts
        for info in &modules {
            let prop_path = cstr::buf::default()
                .join_path(MODULEROOT)
                .join_path(&info.name)
                .join_path("system.prop");
            if prop_path.exists() {
                load_prop_file(&prop_path);
            }
        }

        exec_module_scripts(cstr!("post-fs-data"), &modules);

        // Recollect modules (module scripts could remove itself)
        let modules = collect_modules();

        self.module_list.set(modules).ok();
    }
}
