use crate::consts::SECURE_DIR;
use base::const_format::concatcp;
use base::{Directory, FsPathBuilder, LoggedResult, ResultExt, Utf8CStr, cstr};
use nix::fcntl::OFlag;
use std::collections::HashMap;
use std::io::{BufRead, Write};

const MODULE_CONFIG_DIR: &str = concatcp!(SECURE_DIR, "/module_configs");
const PERSIST_CONFIG: &str = "persist.config";
const TEMP_CONFIG: &str = "tmp.config";

pub fn ensure_config_dir() {
    cstr!(MODULE_CONFIG_DIR).mkdir(0o700).log_ok();
}

fn get_module_config_dir(module_id: &str) -> String {
    format!("{}/{}", MODULE_CONFIG_DIR, module_id)
}

fn read_config_file(path: &Utf8CStr) -> HashMap<String, String> {
    let mut configs = HashMap::new();
    let file = match path.open(OFlag::O_RDONLY | OFlag::O_CLOEXEC) {
        Ok(f) => f,
        Err(_) => return configs,
    };
    let reader = std::io::BufReader::new(file);
    for line in reader.lines() {
        if let Ok(l) = line {
            let trimmed = l.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = trimmed.split_once('=') {
                configs.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
    }
    configs
}

fn write_config_file(path: &Utf8CStr, configs: &HashMap<String, String>) -> LoggedResult<()> {
    let mut file = path.open(
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_TRUNC | OFlag::O_CLOEXEC,
        0o600,
    )?;
    for (key, value) in configs {
        writeln!(file, "{}={}", key, value)?;
    }
    Ok(())
}

pub fn get_config(module_id: &str, key: &str, temp: bool) -> Option<String> {
    let config_file = if temp { TEMP_CONFIG } else { PERSIST_CONFIG };
    let path = cstr::buf::default()
        .join_path(&get_module_config_dir(module_id))
        .join_path(config_file);
    let configs = read_config_file(&path);
    configs.get(key).cloned()
}

pub fn set_config(module_id: &str, key: &str, value: &str, temp: bool) {
    let module_dir = get_module_config_dir(module_id);
    let dir_path = cstr::buf::default().join_path(&module_dir);
    dir_path.mkdir(0o700).log_ok();
    
    let config_file = if temp { TEMP_CONFIG } else { PERSIST_CONFIG };
    let path = cstr::buf::default()
        .join_path(&module_dir)
        .join_path(config_file);
    
    let mut configs = read_config_file(&path);
    configs.insert(key.to_string(), value.to_string());
    write_config_file(&path, &configs).log_ok();
}

pub fn delete_config(module_id: &str, key: &str, temp: bool) {
    let config_file = if temp { TEMP_CONFIG } else { PERSIST_CONFIG };
    let path = cstr::buf::default()
        .join_path(&get_module_config_dir(module_id))
        .join_path(config_file);
    
    let mut configs = read_config_file(&path);
    configs.remove(key);
    write_config_file(&path, &configs).log_ok();
}

pub fn clear_temp_configs() {
    let _ = || -> LoggedResult<()> {
        let dir = Directory::open(cstr!(MODULE_CONFIG_DIR))?;
        while let Some(e) = dir.read()? {
            if !e.is_dir() {
                continue;
            }
            let temp_path = cstr::buf::default()
                .join_path(MODULE_CONFIG_DIR)
                .join_path(e.name())
                .join_path(TEMP_CONFIG);
            temp_path.remove().ok();
        }
        Ok(())
    }();
}

pub fn list_configs(module_id: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();
    let module_dir = get_module_config_dir(module_id);
    
    // Read persistent configs
    let persist_path = cstr::buf::default()
        .join_path(&module_dir)
        .join_path(PERSIST_CONFIG);
    result.extend(read_config_file(&persist_path));
    
    // Read temp configs (temp overrides persist)
    let temp_path = cstr::buf::default()
        .join_path(&module_dir)
        .join_path(TEMP_CONFIG);
    result.extend(read_config_file(&temp_path));
    
    result
}
