#![feature(fn_traits)]
#![feature(unix_socket_ancillary_data)]
#![feature(unix_socket_peek)]
#![feature(default_field_values)]
#![feature(peer_credentials_unix_socket)]
#![feature(sync_nonpoison)]
#![feature(nonpoison_mutex)]
#![feature(nonpoison_condvar)]
#![allow(clippy::missing_safety_doc)]

use crate::ffi::SuRequest;
use crate::socket::Encodable;
use base::derive::Decodable;
use daemon::{MagiskD, connect_daemon_for_cxx};
use logging::{android_logging};
use magisk::magisk_main;
use resetprop::{get_prop, resetprop_main};
use selinux::{lgetfilecon, setfilecon};
use socket::{recv_fd, recv_fds, send_fd};
use std::fs::File;
use std::mem::ManuallyDrop;
use std::ops::DerefMut;
use std::os::fd::FromRawFd;
use su::{get_pty_num, pump_tty};

mod bootstages;
#[path = "../include/consts.rs"]
mod consts;
mod daemon;
mod db;
mod logging;
mod magisk;
mod module;
mod mount;
mod package;
mod resetprop;
mod selinux;
mod socket;
mod su;
mod thread;

#[allow(clippy::needless_lifetimes)]
#[cxx::bridge]
pub mod ffi {
    #[repr(i32)]
    enum RequestCode {
        START_DAEMON,
        CHECK_VERSION,
        CHECK_VERSION_CODE,
        STOP_DAEMON,

        _SYNC_BARRIER_,

        SUPERUSER,
        SQLITE_CMD,
        REMOVE_MODULES,

        _STAGE_BARRIER_,

        POST_FS_DATA,
        LATE_START,
        BOOT_COMPLETE,

        END,
    }

    #[repr(i32)]
    enum RespondCode {
        ERROR = -1,
        OK = 0,
        ROOT_REQUIRED,
        ACCESS_DENIED,
        END,
    }

    enum DbEntryKey {
        RootAccess,
        SuMultiuserMode,
        SuMntNs,
        BootloopCount,
        SuManager,
    }

    #[repr(i32)]
    enum MntNsMode {
        Global,
        Requester,
        Isolate,
    }

    #[repr(i32)]
    enum SuPolicy {
        Query,
        Deny,
        Allow,
        Restrict,
    }

    struct ModuleInfo {
        name: String,
    }

    #[derive(Decodable)]
    struct SuRequest {
        target_uid: i32,
        target_pid: i32,
        login: bool,
        keep_env: bool,
        drop_cap: bool,
        shell: String,
        command: String,
        context: String,
        gids: Vec<u32>,
    }

    unsafe extern "C++" {
        #[cxx_name = "Utf8CStr"]
        type Utf8CStrRef<'a> = base::Utf8CStrRef<'a>;

        include!("include/core.hpp");

        #[cxx_name = "get_magisk_tmp_rs"]
        fn get_magisk_tmp() -> Utf8CStrRef<'static>;
        #[cxx_name = "resolve_preinit_dir_rs"]
        fn resolve_preinit_dir(base_dir: Utf8CStrRef) -> String;
        fn check_key_combo() -> bool;
        fn unlock_blocks();
        fn switch_mnt_ns(pid: i32) -> i32;
        fn exec_root_shell(client: i32, pid: i32, req: &mut SuRequest, mode: MntNsMode);

        // Scripting
        fn exec_script(script: Utf8CStrRef);
        fn exec_common_scripts(stage: Utf8CStrRef);
        fn exec_module_scripts(state: Utf8CStrRef, modules: &Vec<ModuleInfo>);
        fn install_apk(apk: Utf8CStrRef);
        fn uninstall_pkg(apk: Utf8CStrRef);
        fn install_module(zip: Utf8CStrRef);

        include!("include/sqlite.hpp");

        type sqlite3;
        type DbValues;
        type DbStatement;

        fn sqlite3_errstr(code: i32) -> *const c_char;
        fn open_and_init_db() -> *mut sqlite3;
        fn get_int(self: &DbValues, index: i32) -> i32;
        #[cxx_name = "get_str"]
        fn get_text(self: &DbValues, index: i32) -> &str;
        fn bind_text(self: Pin<&mut DbStatement>, index: i32, val: &str) -> i32;
        fn bind_int64(self: Pin<&mut DbStatement>, index: i32, val: i64) -> i32;
    }

    extern "Rust" {
        fn android_logging();
        fn send_fd(socket: i32, fd: i32) -> bool;
        fn recv_fd(socket: i32) -> i32;
        fn recv_fds(socket: i32) -> Vec<i32>;
        fn write_to_fd(self: &SuRequest, fd: i32);
        fn pump_tty(ptmx: i32, pump_stdin: bool);
        fn get_pty_num(fd: i32) -> i32;
        fn lgetfilecon(path: Utf8CStrRef, con: &mut [u8]) -> bool;
        fn setfilecon(path: Utf8CStrRef, con: Utf8CStrRef) -> bool;

        fn get_prop(name: Utf8CStrRef) -> String;
        unsafe fn resetprop_main(argc: i32, argv: *mut *mut c_char) -> i32;

        #[cxx_name = "connect_daemon"]
        fn connect_daemon_for_cxx(code: RequestCode, create: bool) -> i32;
        unsafe fn magisk_main(argc: i32, argv: *mut *mut c_char) -> i32;
    }

    // Default constructors
    extern "Rust" {
        #[Self = SuRequest]
        #[cxx_name = "New"]
        fn default() -> SuRequest;
    }

    // FFI for MagiskD
    extern "Rust" {
        type MagiskD;
        fn sdk_int(&self) -> i32;
        fn get_db_setting(&self, key: DbEntryKey) -> i32;
        #[cxx_name = "set_db_setting"]
        fn set_db_setting_for_cxx(&self, key: DbEntryKey, value: i32) -> bool;

        #[Self = MagiskD]
        #[cxx_name = "Get"]
        fn get() -> &'static MagiskD;
    }
}

impl SuRequest {
    fn write_to_fd(&self, fd: i32) {
        unsafe {
            let mut w = ManuallyDrop::new(File::from_raw_fd(fd));
            self.encode(w.deref_mut()).ok();
        }
    }
}
