//! Headless-friendly abstractions for daemon lifecycle wiring.
//!
//! **K2 Connect prep**: this module owns the *logic* of "where does the
//! daemon binary live?" and "what arguments would launchctl need to
//! load / kick / unload the daemon plist?" — independent of any actual
//! filesystem-touching or process-spawning. K2 Connect's headless flow
//! (Tauri-less daemons running on remote hosts) calls into the same
//! plist generator + arg builders here so a remote-daemon scenario
//! can produce a launchd plist + bootstrap script without dragging in
//! Tauri.
//!
//! `crate::wake` owns the FS write + actual `launchctl load/unload/list`
//! invocations. This module owns the **shape** of those calls: the
//! canonical label, the relative path where K2SO.app keeps the daemon
//! binary, the launchctl kickstart target string. The Tauri commands
//! in `src-tauri/src/commands/daemon.rs` are now thin wrappers around
//! these helpers + a plain `Command::new("launchctl")` invocation.
//!
//! Why a separate module from `wake`?
//! - `wake` is the implementation surface for "install + unload + check
//!   loaded state." Heavy because it writes files and shells out.
//! - `daemon_lifecycle` is the **pure** surface: no FS, no subprocess.
//!   Pure data + arg-vec construction. Cheap to test in isolation and
//!   trivial to call from K2 Connect orchestration code that doesn't
//!   want to accidentally trigger an actual install.

use std::path::{Path, PathBuf};

/// Reverse-DNS launch agent label used by the daemon plist on macOS.
///
/// Mirrors `crate::wake::DaemonPlist::canonical().label`. Re-exported
/// here so callers building `launchctl` arg vectors (kickstart target,
/// `list <label>` probes) don't have to import `wake::DaemonPlist`
/// just for the string.
pub const DAEMON_LAUNCH_AGENT_LABEL: &str = "com.k2so.k2so-daemon";

/// Filename of the daemon binary as bundled inside `K2SO.app`.
///
/// Used by `bundled_daemon_path` to resolve `K2SO.app/Contents/MacOS/k2so-daemon`
/// from the current Tauri binary path. Centralized so the build script,
/// the Tauri "Install daemon" command, and K2 Connect's "where's the
/// daemon binary?" probe all agree.
pub const DAEMON_BINARY_NAME: &str = "k2so-daemon";

/// Pure-logic resolution: given the path of a Tauri executable
/// (typically `K2SO.app/Contents/MacOS/k2so`), return the expected
/// path of the bundled `k2so-daemon` binary alongside it. This does
/// **not** touch the filesystem — callers (Tauri `daemon_install` or
/// K2 Connect's headless bootstrap) check `.exists()` themselves.
///
/// Returns `None` when `tauri_exe` has no parent directory (only
/// happens for pathological inputs like `/` or relative-empty paths).
///
/// Why split path-resolution from the FS check?
/// - Tests can drive this against synthetic paths without needing a
///   real binary on disk.
/// - K2 Connect can compute the "if I were to install a daemon plist
///   pointing at this Tauri install, this is the binary path it would
///   reference" without actually performing the install.
pub fn bundled_daemon_path(tauri_exe: &Path) -> Option<PathBuf> {
    tauri_exe.parent().map(|d| d.join(DAEMON_BINARY_NAME))
}

/// Build the `launchctl kickstart -k <target>` argument vector that
/// SIGTERMs the running daemon and lets launchd respawn it (because
/// `KeepAlive: true`). `uid` is the Unix UID the daemon plist is
/// loaded into — typically `unsafe { libc::getuid() }` from the caller.
///
/// Pure: returns the arg vector; the caller invokes `Command::new`.
/// Separating this out means K2 Connect can compose the same exact
/// command for a remote-machine kickstart via SSH without dragging in
/// `std::process::Command`.
pub fn launchctl_kickstart_args(uid: u32) -> Vec<String> {
    vec![
        "kickstart".to_string(),
        "-k".to_string(),
        format!("gui/{}/{}", uid, DAEMON_LAUNCH_AGENT_LABEL),
    ]
}

/// Build the `launchctl list <label>` argument vector used to probe
/// whether the daemon plist is currently loaded. Exit-code zero means
/// loaded. Mirror of [`launchctl_kickstart_args`] for symmetry — no FS,
/// no spawning, just the arg vec.
pub fn launchctl_list_args() -> Vec<String> {
    vec!["list".to_string(), DAEMON_LAUNCH_AGENT_LABEL.to_string()]
}

/// Generate the plist XML body the canonical daemon launch agent would
/// install. Thin re-export of `crate::wake::DaemonPlist::canonical(...).to_xml()`
/// — exposed at this path so K2 Connect callers can grab "what plist
/// would K2SO write for this daemon binary?" without going through the
/// `wake` module's install-shaped surface.
///
/// `program` is the path the plist's `ProgramArguments` will point at.
/// On a local install that's `~/Applications/K2SO.app/Contents/MacOS/k2so-daemon`;
/// on a K2 Connect remote install it's whatever path the remote-side
/// daemon binary lives at.
pub fn generate_plist_content(program: PathBuf) -> String {
    crate::wake::DaemonPlist::canonical(program).to_xml()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_plist_content_includes_correct_label() {
        let xml = generate_plist_content(PathBuf::from("/opt/k2so-daemon"));
        assert!(
            xml.contains(&format!("<string>{DAEMON_LAUNCH_AGENT_LABEL}</string>")),
            "plist missing daemon label: {xml}"
        );
    }

    #[test]
    fn generate_plist_content_points_to_daemon_binary() {
        let xml = generate_plist_content(PathBuf::from("/Applications/K2SO.app/Contents/MacOS/k2so-daemon"));
        assert!(
            xml.contains("<string>/Applications/K2SO.app/Contents/MacOS/k2so-daemon</string>"),
            "plist missing daemon binary path: {xml}"
        );
    }

    #[test]
    fn launchctl_kickstart_arg_format() {
        let args = launchctl_kickstart_args(501);
        assert_eq!(
            args,
            vec![
                "kickstart".to_string(),
                "-k".to_string(),
                "gui/501/com.k2so.k2so-daemon".to_string(),
            ],
            "kickstart args shape changed; bumping launchctl is wire-incompatible"
        );
    }

    #[test]
    fn launchctl_list_arg_format() {
        let args = launchctl_list_args();
        assert_eq!(
            args,
            vec!["list".to_string(), "com.k2so.k2so-daemon".to_string()],
        );
    }

    #[test]
    fn bundled_daemon_path_resolves_next_to_tauri_exe() {
        let path = bundled_daemon_path(Path::new(
            "/Applications/K2SO.app/Contents/MacOS/k2so",
        ));
        assert_eq!(
            path,
            Some(PathBuf::from("/Applications/K2SO.app/Contents/MacOS/k2so-daemon")),
        );
    }

    #[test]
    fn bundled_daemon_path_returns_none_for_pathological_input() {
        // `Path::new("").parent()` returns None — `bundled_daemon_path`
        // must surface that as None instead of panicking.
        let path = bundled_daemon_path(Path::new(""));
        assert!(path.is_none(), "empty path should resolve to None, got {path:?}");
    }

    #[test]
    fn daemon_label_constant_matches_wake_canonical() {
        // Guards against the constant drifting from
        // `DaemonPlist::canonical`'s label. Keeps the two sources
        // synchronized without runtime coupling.
        let p = crate::wake::DaemonPlist::canonical(PathBuf::from("/opt/k2so-daemon"));
        assert_eq!(p.label, DAEMON_LAUNCH_AGENT_LABEL);
    }
}
