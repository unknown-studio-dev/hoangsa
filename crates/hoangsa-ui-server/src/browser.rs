use std::process::Command;

/// Best-effort browser launch. We don't propagate errors — if the OS hook is
/// missing, the user can copy the URL printed on stdout. The caller has
/// already logged the URL by this point.
pub fn open(url: &str) {
    #[cfg(target_os = "macos")]
    let prog = "open";
    #[cfg(target_os = "linux")]
    let prog = "xdg-open";
    #[cfg(target_os = "windows")]
    let prog = "cmd";

    #[cfg(target_os = "windows")]
    let args: Vec<&str> = vec!["/C", "start", "", url];
    #[cfg(not(target_os = "windows"))]
    let args: Vec<&str> = vec![url];

    let _ = Command::new(prog).args(args).spawn();
}
