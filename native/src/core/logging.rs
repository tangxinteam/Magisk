use crate::consts::{LOG_PIPE, LOGFILE};
use crate::ffi::get_magisk_tmp;
use crate::logging::LogFile::{Actual, Buffer};
use base::const_format::concatcp;
use base::{
    FsPathBuilder, LogLevel, LoggedResult, ReadExt, ResultExt, Utf8CStr, Utf8CStrBuf, WriteExt,
    cstr, libc, new_daemon_thread, raw_cstr, update_logger,
};
use bytemuck::{Pod, Zeroable, bytes_of, write_zeroes};
use libc::{PIPE_BUF, c_char, localtime_r, sigtimedwait, time_t, timespec, tm};
use nix::fcntl::OFlag;
use nix::sys::signal::{SigSet, SigmaskHow, Signal};
use nix::unistd::{Gid, Uid, chown, getpid, gettid};
use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::FromPrimitive;
use std::cmp::min;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{IoSlice, Read, Write};
use std::mem::ManuallyDrop;
use std::os::fd::{FromRawFd, IntoRawFd, RawFd};
use std::ptr::null_mut;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::nonpoison::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{fs, io};

#[allow(dead_code, non_camel_case_types)]
#[derive(FromPrimitive, ToPrimitive)]
#[repr(i32)]
enum ALogPriority {
    ANDROID_LOG_UNKNOWN = 0,
    ANDROID_LOG_DEFAULT,
    ANDROID_LOG_VERBOSE,
    ANDROID_LOG_DEBUG,
    ANDROID_LOG_INFO,
    ANDROID_LOG_WARN,
    ANDROID_LOG_ERROR,
    ANDROID_LOG_FATAL,
    ANDROID_LOG_SILENT,
}

unsafe extern "C" {
    fn __android_log_write(prio: i32, tag: *const c_char, msg: *const c_char);
    fn strftime(buf: *mut c_char, len: usize, fmt: *const c_char, tm: *const tm) -> usize;
}

fn level_to_prio(level: LogLevel) -> i32 {
    match level {
        LogLevel::Error => ALogPriority::ANDROID_LOG_ERROR as i32,
        LogLevel::Warn => ALogPriority::ANDROID_LOG_WARN as i32,
        LogLevel::Info => ALogPriority::ANDROID_LOG_INFO as i32,
        LogLevel::Debug => ALogPriority::ANDROID_LOG_DEBUG as i32,
    }
}

fn android_log_write(level: LogLevel, msg: &Utf8CStr) {
    unsafe {
        __android_log_write(level_to_prio(level), raw_cstr!("Magisk"), msg.as_ptr());
    }
}

pub fn android_logging() {
    update_logger(|logger| logger.write = android_log_write);
}

pub fn magisk_logging() {
    fn magisk_log_write(level: LogLevel, msg: &Utf8CStr) {
        android_log_write(level, msg);
        magisk_log_to_pipe(level_to_prio(level), msg);
    }
    update_logger(|logger| logger.write = magisk_log_write);
}

#[derive(Copy, Clone, Pod, Zeroable)]
#[repr(C)]
struct LogMeta {
    prio: i32,
    len: i32,
    pid: i32,
    tid: i32,
}

const MAX_MSG_LEN: usize = PIPE_BUF - size_of::<LogMeta>();

fn write_log_to_pipe(mut logd: &File, prio: i32, msg: &Utf8CStr) -> io::Result<usize> {
    // Truncate message if needed
    let len = min(MAX_MSG_LEN, msg.len());
    let msg = &msg.as_bytes()[..len];

    let meta = LogMeta {
        prio,
        len: len as i32,
        pid: getpid().as_raw(),
        tid: gettid().as_raw(),
    };

    let io1 = IoSlice::new(bytes_of(&meta));
    let io2 = IoSlice::new(msg);
    let result = logd.write_vectored(&[io1, io2]);
    if let Err(ref e) = result {
        let mut buf = cstr::buf::new::<256>();
        write!(buf, "Cannot write_log_to_pipe: {e}").ok();
        android_log_write(LogLevel::Error, &buf);
    }
    result
}

static MAGISK_LOGD_FD: Mutex<Option<Arc<File>>> = Mutex::new(None);

fn with_logd_fd<R, F: FnOnce(&File) -> io::Result<R>>(f: F) {
    let fd = MAGISK_LOGD_FD.lock().clone();
    if let Some(logd) = fd
        && f(&logd).is_err()
    {
        // If any error occurs, shut down the logd pipe
        *MAGISK_LOGD_FD.lock() = None;
    }
}

fn magisk_log_to_pipe(prio: i32, msg: &Utf8CStr) {
    with_logd_fd(|logd| write_log_to_pipe(logd, prio, msg));
}

// The following is implementation for the logging daemon

enum LogFile {
    Buffer(Vec<u8>),
    Actual(File),
}

impl LogFile {
    fn as_write(&mut self) -> &mut dyn Write {
        match self {
            Buffer(e) => e,
            Actual(e) => e,
        }
    }
}

fn logfile_write_loop(mut pipe: File) -> io::Result<()> {
    let mut logfile: LogFile = Buffer(Vec::new());

    let mut meta = LogMeta::zeroed();
    let mut msg_buf = [0u8; MAX_MSG_LEN];
    let mut aux = cstr::buf::new::<64>();

    loop {
        // Read request
        write_zeroes(&mut meta);
        pipe.read_pod(&mut meta)?;

        if meta.prio < 0 {
            if let Buffer(ref mut buf) = logfile {
                fs::rename(LOGFILE, concatcp!(LOGFILE, ".bak")).ok();
                let mut out = File::create(LOGFILE)?;
                out.write_all(buf.as_slice())?;
                logfile = Actual(out);
            }
            continue;
        }

        if meta.len < 0 || meta.len > MAX_MSG_LEN as i32 {
            continue;
        }

        // Read the rest of the message
        let msg = &mut msg_buf[..(meta.len as usize)];
        pipe.read_exact(msg)?;

        // Start building the log string
        aux.clear();
        let prio = ALogPriority::from_i32(meta.prio).unwrap_or(ALogPriority::ANDROID_LOG_UNKNOWN);
        let prio = match prio {
            ALogPriority::ANDROID_LOG_VERBOSE => 'V',
            ALogPriority::ANDROID_LOG_DEBUG => 'D',
            ALogPriority::ANDROID_LOG_INFO => 'I',
            ALogPriority::ANDROID_LOG_WARN => 'W',
            ALogPriority::ANDROID_LOG_ERROR => 'E',
            // Unsupported values, skip
            _ => continue,
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);

        // Note: the obvious better implementation is to use the rust chrono crate, however
        // the crate cannot fetch the proper local timezone without pulling in a bunch of
        // timezone handling code. To reduce binary size, fallback to use localtime_r in libc.
        unsafe {
            let secs = now.as_secs() as time_t;
            let mut tm: tm = std::mem::zeroed();
            if localtime_r(&secs, &mut tm).is_null() {
                continue;
            }
            strftime(aux.as_mut_ptr(), aux.capacity(), raw_cstr!("%m-%d %T"), &tm);
        }

        if aux.rebuild().is_ok() {
            write!(
                aux,
                ".{:03} {:5} {:5} {} : ",
                now.subsec_millis(),
                meta.pid,
                meta.tid,
                prio
            )
            .ok();
        } else {
            continue;
        }

        let io1 = IoSlice::new(aux.as_bytes());
        let io2 = IoSlice::new(msg);
        // We don't need to care the written len because we are writing less than PIPE_BUF
        // It's guaranteed to always write the whole thing atomically
        let _ = logfile.as_write().write_vectored(&[io1, io2])?;
    }
}

pub fn setup_logfile() {
    with_logd_fd(|mut logd| {
        let meta = LogMeta {
            prio: -1,
            len: 0,
            pid: 0,
            tid: 0,
        };
        (&mut logd).write_pod(&meta)
    });
}

pub fn start_log_daemon() {
    let path = cstr::buf::default()
        .join_path(get_magisk_tmp())
        .join_path(LOG_PIPE);

    extern "C" fn logfile_writer_thread(arg: usize) -> usize {
        let file = unsafe { File::from_raw_fd(arg as RawFd) };
        logfile_write_loop(file).ok();
        // If any error occurs, shut down the logd pipe
        *MAGISK_LOGD_FD.lock() = None;
        0
    }

    let _ = || -> LoggedResult<()> {
        path.mkfifo(0o666).log_ok();
        chown(path.as_utf8_cstr(), Some(Uid::from(0)), Some(Gid::from(0)))?;
        let read = path.open(OFlag::O_RDWR | OFlag::O_CLOEXEC)?;
        let write = path.open(OFlag::O_WRONLY | OFlag::O_CLOEXEC)?;
        *MAGISK_LOGD_FD.lock() = Some(Arc::new(write));
        unsafe {
            new_daemon_thread(logfile_writer_thread, read.into_raw_fd() as usize);
        }
        Ok(())
    }();
}
