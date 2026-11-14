use std::ffi::OsStr;

/// `NO_COLOR` disables colour when it is present and not empty.
pub fn color_enabled(no_color: bool, no_color_env: Option<&OsStr>, is_terminal: bool) -> bool {
    if no_color || !is_terminal {
        return false;
    }
    if no_color_env.is_some_and(|value| !value.is_empty()) {
        return false;
    }
    true
}

pub fn ignore_sigpipe() {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn signal(sig: i32, handler: usize) -> usize;
        }
        // SAFETY: `signal` is the libc function. On Linux and macOS, signal 13
        // is SIGPIPE and handler 1 is SIG_IGN. Ignoring it turns a closed pipe
        // into a normal write error instead of killing the process.
        unsafe {
            signal(13, 1);
        }
    }
}

/// Enable a UTF-8 console on Windows and turn on ANSI processing when colour
/// was requested. Returns whether colour should actually be emitted.
pub fn prepare_ansi(want_color: bool) -> bool {
    #[cfg(windows)]
    {
        enable_utf8_console();
        want_color && enable_virtual_terminal()
    }
    #[cfg(not(windows))]
    {
        want_color
    }
}

#[cfg(windows)]
fn enable_utf8_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetConsoleOutputCP(code_page: u32) -> i32;
        fn SetConsoleCP(code_page: u32) -> i32;
    }
    const CP_UTF8: u32 = 65001;
    // SAFETY: 65001 is the documented UTF-8 code page. A failed call leaves
    // the existing console code page in place.
    unsafe {
        SetConsoleOutputCP(CP_UTF8);
        SetConsoleCP(CP_UTF8);
    }
}

#[cfg(windows)]
fn enable_virtual_terminal() -> bool {
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleMode(handle: *mut std::ffi::c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut std::ffi::c_void, mode: u32) -> i32;
    }
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    let handle = std::io::stdout().as_raw_handle();
    let mut mode = 0;
    // SAFETY: `handle` comes from stdout. The console-mode calls are the
    // documented Windows APIs; a zero result means this is not a console that
    // can accept ANSI sequences.
    unsafe {
        if GetConsoleMode(handle, &mut mode) == 0 {
            return false;
        }
        SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn colour_requires_a_terminal_and_respects_opt_outs() {
        assert!(color_enabled(false, None, true));
        assert!(!color_enabled(true, None, true));
        assert!(!color_enabled(false, None, false));
        assert!(!color_enabled(false, Some(OsStr::new("1")), true));
        assert!(color_enabled(false, Some(OsStr::new("")), true));
    }
}
