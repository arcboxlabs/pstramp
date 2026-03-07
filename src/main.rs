//! pstramp — process spawn trampoline (no_std, macOS-only).
//!
//! Lightweight helper for process isolation. Performs optional session and
//! controlling-terminal setup, then exec()s into the requested command.
//!
//! # Usage
//!
//! ```text
//! pstramp [-setctty] [-disclaim] -- command [args...]
//! ```
//!
//! # Flags
//!
//! - `-setctty`  — Create a new session (`setsid`) and claim stdin as the
//!   controlling terminal (`TIOCSCTTY`).
//! - `-disclaim` — Use `posix_spawn` with `POSIX_SPAWN_DISCLAIM` to relinquish
//!   the parent's responsibility claims.

#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[cfg(not(target_os = "macos"))]
compile_error!("pstramp is macOS-only");

use core::ffi::{c_char, c_int, c_short, c_void};
use core::ptr;

// macOS ioctl request: _IO('t', 97) = 0x20007461
const TIOCSCTTY: u64 = 0x2000_7461;

// posix_spawn flags.
const POSIX_SPAWN_SETEXEC: c_short = 0x0040;
const POSIX_SPAWN_DISCLAIM: c_short = 0x2000;

type SpawnAttr = *mut c_void;

#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    fn setsid() -> c_int;
    fn ioctl(fd: c_int, request: u64, ...) -> c_int;
    fn execvp(file: *const c_char, argv: *const *const c_char) -> c_int;
    fn write(fd: c_int, buf: *const u8, count: usize) -> isize;
    fn strcmp(s1: *const c_char, s2: *const c_char) -> c_int;
    fn strlen(s: *const c_char) -> usize;
    fn exit(status: c_int) -> !;

    fn posix_spawnattr_init(attr: *mut SpawnAttr) -> c_int;
    fn posix_spawnattr_setflags(attr: *mut SpawnAttr, flags: c_short) -> c_int;
    fn posix_spawnattr_destroy(attr: *mut SpawnAttr) -> c_int;
    fn posix_spawn(
        pid: *mut c_int,
        path: *const c_char,
        actions: *const c_void,
        attr: *const SpawnAttr,
        argv: *const *const c_char,
        envp: *const *const c_char,
    ) -> c_int;

    fn _NSGetEnviron() -> *const *const *const c_char;
}

unsafe fn eputs(s: &[u8]) {
    write(2, s.as_ptr(), s.len());
}

unsafe fn eput_cstr(s: *const c_char) {
    write(2, s.cast(), strlen(s));
}

unsafe fn usage(argv0: *const c_char) {
    eputs(b"usage: ");
    eput_cstr(argv0);
    eputs(b" [-setctty] [-disclaim] -- command [args...]\n");
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn main(argc: c_int, argv: *const *const c_char) -> c_int {
    let mut do_setctty = false;
    let mut do_disclaim = false;
    let mut cmd_start: c_int = 0;

    // Parse flags up to `--` separator.
    let mut i: c_int = 1;
    while i < argc {
        let arg = *argv.offset(i as isize);
        if strcmp(arg, c"--".as_ptr()) == 0 {
            cmd_start = i + 1;
            break;
        } else if strcmp(arg, c"-setctty".as_ptr()) == 0 {
            do_setctty = true;
        } else if strcmp(arg, c"-disclaim".as_ptr()) == 0 {
            do_disclaim = true;
        } else {
            eputs(b"pstramp: unknown flag: ");
            eput_cstr(arg);
            eputs(b"\n");
            usage(*argv);
            return 2;
        }
        i += 1;
    }

    if cmd_start == 0 || cmd_start >= argc {
        usage(*argv);
        return 2;
    }

    let cmd_argv = argv.offset(cmd_start as isize);

    // -setctty: new session + claim stdin as controlling terminal.
    if do_setctty {
        if setsid() == -1 {
            eputs(b"pstramp: setsid failed\n");
            return 1;
        }
        if ioctl(0, TIOCSCTTY, 0 as c_int) == -1 {
            eputs(b"pstramp: ioctl TIOCSCTTY failed\n");
            return 1;
        }
    }

    // -disclaim: posix_spawn with DISCLAIM | SETEXEC replaces current process.
    if do_disclaim {
        let mut attr: SpawnAttr = ptr::null_mut();
        if posix_spawnattr_init(&mut attr) != 0 {
            eputs(b"pstramp: posix_spawnattr_init failed\n");
            return 1;
        }
        if posix_spawnattr_setflags(&mut attr, POSIX_SPAWN_DISCLAIM | POSIX_SPAWN_SETEXEC) != 0 {
            posix_spawnattr_destroy(&mut attr);
            eputs(b"pstramp: posix_spawnattr_setflags failed\n");
            return 1;
        }
        // SETEXEC replaces the current process; should not return on success.
        posix_spawn(
            ptr::null_mut(),
            *cmd_argv,
            ptr::null(),
            &attr,
            cmd_argv,
            *_NSGetEnviron(),
        );
        posix_spawnattr_destroy(&mut attr);
        eputs(b"pstramp: posix_spawn failed\n");
        return 1;
    }

    // Default: plain execvp.
    execvp(*cmd_argv, cmd_argv);
    eputs(b"pstramp: exec failed\n");
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { exit(1) }
}

// Stub for precompiled core's unwind references (dead-stripped in release via LTO).
#[unsafe(no_mangle)]
pub extern "C" fn rust_eh_personality() {}
