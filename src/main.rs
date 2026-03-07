//! pstramp — process spawn trampoline.
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
//!   the parent's responsibility claims (macOS only).

use std::ffi::CString;
use std::process::ExitCode;

/// macOS `POSIX_SPAWN_DISCLAIM` — relinquish parent responsibility claims.
/// Available since macOS 13, not yet in the `libc` crate.
#[cfg(target_os = "macos")]
const POSIX_SPAWN_DISCLAIM: libc::c_short = 0x2000;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();

    let mut do_setctty = false;
    let mut do_disclaim = false;
    let mut cmd_start: Option<usize> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--" => {
                cmd_start = Some(i + 1);
                break;
            }
            "-setctty" => do_setctty = true,
            "-disclaim" => do_disclaim = true,
            other => {
                eprintln!("pstramp: unknown flag: {other}");
                usage(&args[0]);
                return ExitCode::from(2);
            }
        }
        i += 1;
    }

    let cmd_start = match cmd_start {
        Some(idx) if idx < args.len() => idx,
        _ => {
            usage(&args[0]);
            return ExitCode::from(2);
        }
    };

    // -setctty: new session + controlling terminal.
    if do_setctty {
        if let Err(e) = setctty() {
            eprintln!("pstramp: {e}");
            return ExitCode::FAILURE;
        }
    }

    // -disclaim: posix_spawn with POSIX_SPAWN_DISCLAIM + POSIX_SPAWN_SETEXEC.
    #[cfg(target_os = "macos")]
    if do_disclaim {
        let err = disclaim_exec(&args[cmd_start..]);
        eprintln!("pstramp: {err}");
        return ExitCode::FAILURE;
    }

    #[cfg(not(target_os = "macos"))]
    if do_disclaim {
        eprintln!("pstramp: -disclaim is only supported on macOS");
        return ExitCode::FAILURE;
    }

    // Default path: plain exec.
    let err = exec(&args[cmd_start..]);
    eprintln!("pstramp: exec {}: {err}", args[cmd_start]);
    ExitCode::FAILURE
}

fn usage(argv0: &str) {
    eprintln!("usage: {argv0} [-setctty] [-disclaim] -- command [args...]");
}

/// Create a new session and claim stdin as the controlling terminal.
fn setctty() -> Result<(), String> {
    // SAFETY: setsid is always safe; it only fails if already a session leader.
    let ret = unsafe { libc::setsid() };
    if ret == -1 {
        return Err(format!("setsid: {}", std::io::Error::last_os_error()));
    }

    // SAFETY: TIOCSCTTY with arg 0 claims the terminal attached to stdin.
    let ret =
        unsafe { libc::ioctl(libc::STDIN_FILENO, u64::from(libc::TIOCSCTTY), 0) };
    if ret == -1 {
        return Err(format!(
            "ioctl TIOCSCTTY: {}",
            std::io::Error::last_os_error()
        ));
    }

    Ok(())
}

/// Use `posix_spawn` with `POSIX_SPAWN_DISCLAIM | POSIX_SPAWN_SETEXEC` to
/// replace the current process while relinquishing responsibility claims.
#[cfg(target_os = "macos")]
fn disclaim_exec(argv: &[String]) -> String {
    let Ok(program) = to_cstring(&argv[0]) else {
        return format!("program name contains nul: {}", argv[0]);
    };
    let c_args: Vec<CString> = match argv.iter().map(|a| to_cstring(a)).collect() {
        Ok(v) => v,
        Err(e) => return e,
    };
    let c_argv: Vec<*mut libc::c_char> = c_args
        .iter()
        .map(|s| s.as_ptr().cast_mut())
        .chain(std::iter::once(std::ptr::null_mut()))
        .collect();

    let mut attr: libc::posix_spawnattr_t = std::ptr::null_mut();

    // SAFETY: posix_spawnattr_init initializes the attribute object.
    let ret = unsafe { libc::posix_spawnattr_init(&mut attr) };
    if ret != 0 {
        return format!("posix_spawnattr_init: {}", os_error(ret));
    }

    #[allow(clippy::cast_possible_truncation)]
    let flags: libc::c_short = POSIX_SPAWN_DISCLAIM | libc::POSIX_SPAWN_SETEXEC as libc::c_short;

    // SAFETY: Setting flags on a properly initialized attribute.
    let ret = unsafe { libc::posix_spawnattr_setflags(&mut attr, flags) };
    if ret != 0 {
        unsafe { libc::posix_spawnattr_destroy(&mut attr) };
        return format!("posix_spawnattr_setflags: {}", os_error(ret));
    }

    // SAFETY: posix_spawn with SETEXEC replaces the current process.
    // _NSGetEnviron() returns the process environment pointer.
    let ret = unsafe {
        libc::posix_spawn(
            std::ptr::null_mut(),
            program.as_ptr(),
            std::ptr::null(),
            &attr,
            c_argv.as_ptr(),
            *libc::_NSGetEnviron(),
        )
    };

    // posix_spawn with SETEXEC should not return on success.
    unsafe { libc::posix_spawnattr_destroy(&mut attr) };
    format!("posix_spawn: {}", os_error(ret))
}

/// Plain execvp — replaces the current process.
fn exec(argv: &[String]) -> std::io::Error {
    let program = CString::new(argv[0].as_bytes()).expect("program name contains nul");
    let c_args: Vec<CString> = argv
        .iter()
        .map(|a| CString::new(a.as_bytes()).expect("argument contains nul"))
        .collect();
    let c_argv: Vec<*const libc::c_char> = c_args
        .iter()
        .map(|s| s.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();

    // SAFETY: execvp replaces the process; only returns on error.
    unsafe { libc::execvp(program.as_ptr(), c_argv.as_ptr()) };
    std::io::Error::last_os_error()
}

fn to_cstring(s: &str) -> Result<CString, String> {
    CString::new(s.as_bytes()).map_err(|_| format!("argument contains nul byte: {s}"))
}

fn os_error(errno: libc::c_int) -> String {
    std::io::Error::from_raw_os_error(errno).to_string()
}
