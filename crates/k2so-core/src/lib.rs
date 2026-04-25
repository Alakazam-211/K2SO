//! K2SO core library.
//!
//! Home of the device-local runtime that was previously embedded inside
//! `src-tauri/src/`: SQLite database, llama-cpp LLM integration, Alacritty
//! terminal backend, companion WebSocket server, heartbeat scheduler,
//! agent-lifecycle HTTP hooks, and the pluggable `PushTarget` interface for
//! notification delivery.
//!
//! Both the `k2so-daemon` binary and the `src-tauri` Tauri app link this
//! crate so the core logic executes in exactly one place — the daemon —
//! while the Tauri app stays a thin client that proxies state-mutating
//! commands over HTTP.
//!
//! Module migration from src-tauri lands incrementally.

/// Safe `eprintln!` replacement that silently ignores stderr write
/// failures. When K2SO is launched from Finder there's no tty attached and
/// the default `eprintln!` panics on broken-pipe, which then cascades into
/// abort(). This macro swallows the write result instead.
///
/// `#[macro_export]` so both k2so-core-internal modules and downstream
/// crates (src-tauri, k2so-daemon) can share one definition.
#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

pub mod agent_hooks;
pub mod agents;
pub mod chat_history;
pub mod companion;
pub mod db;
pub mod editors;
pub mod fs_abstract;
pub mod fs_atomic;
pub mod git;
pub mod hook_config;
pub mod llm;
pub mod perf;
pub mod project_config;
pub mod push;
pub mod scheduler;
pub mod terminal;
pub mod wake;

// 0.34.0 Session Stream primitives — gated so flag-off builds compile
// bit-for-bit like v0.33.0. See .k2so/prds/session-stream-and-awareness-bus.md
// for the phased rollout; plan at ~/.claude/plans/happy-hatching-locket.md.
#[cfg(feature = "session_stream")]
pub mod awareness;
#[cfg(feature = "session_stream")]
pub mod session;
#[cfg(feature = "session_stream")]
pub mod term;

/// Replace this process's `PATH` with the one a fresh login shell
/// would produce.
///
/// Why: macOS launchd does not source `.zshrc` / `.bash_profile` when
/// it starts jobs. Both K2SO's Tauri `.app` and `k2so-daemon` are
/// launchd children, so they inherit the kernel's sparse default
/// (`/usr/bin:/bin:/usr/sbin:/sbin`). User binaries installed under
/// `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, or any
/// language-runtime prefix (`~/.cargo/bin`, `~/.bun/bin`, npm globals,
/// etc.) live outside that and are unfindable by `posix_spawn` of a
/// bare command name.
///
/// The standard macOS-GUI-app remedy: ask the user's login shell to
/// print its `PATH` once at startup and adopt it. Children spawned
/// later inherit the rich PATH naturally — no per-spawn shell wrapper
/// or per-spawn lookup needed.
///
/// Costs ~30-100ms once at startup (one shell exec; the upper bound
/// covers users with heavy `.zshrc` plugins like oh-my-zsh / p10k).
/// Best-effort: silently leaves the existing PATH alone if the shell
/// exec fails or returns nothing useful, so we never block startup
/// on a misconfigured shell.
///
/// Why **`-ilc`** instead of just `-lc`: zsh sources `~/.zshrc` only
/// for *interactive* shells, not login-only ones. Many users (Rosson
/// included) set `export PATH="$HOME/.local/bin:$PATH"` in `~/.zshrc`,
/// not in `~/.zprofile`. Without `-i`, `~/.local/bin` and tool-manager
/// dirs (`~/.bun/bin`, `~/.cargo/bin`, npm globals, etc.) are missing
/// from the captured PATH — exactly the bug 0.35.1 shipped. We use
/// `-ilc` to source the full chain (zshenv → zprofile → zshrc → zlogin
/// for zsh). Stderr is dropped so noisy `.zshrc` plugins don't pollute
/// our capture, and we take only the last line of stdout in case the
/// rc files emit anything before our `printf`.
///
/// Call this at the top of `main()` / `run()` in every binary that
/// might `posix_spawn` user-installed tools (the daemon spawns them
/// directly via alacritty's `tty::new`; the Tauri process spawns them
/// via the legacy `TerminalManager` and assorted `Command::new` call
/// sites).
#[cfg(unix)]
pub fn enrich_path_from_login_shell() {
    use std::process::Stdio;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let output = std::process::Command::new(&shell)
        .args(["-ilc", "printf %s \"$PATH\""])
        .stderr(Stdio::null())
        .output();
    if let Ok(out) = output {
        if out.status.success() {
            // Take the last non-empty line of stdout. If the user's
            // rc files print anything before us, our `printf %s` (no
            // newline) lands at the end — split-rsplit-find-non-empty
            // recovers our payload regardless of preamble noise.
            let stdout = String::from_utf8_lossy(&out.stdout);
            let captured = stdout
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_string();
            if !captured.is_empty()
                && captured.contains('/')
                && captured != std::env::var("PATH").unwrap_or_default()
            {
                std::env::set_var("PATH", captured);
            }
        }
    }
}

#[doc(hidden)]
pub fn __scaffolding_marker() {}

#[cfg(all(test, unix))]
mod path_enrichment_tests {
    //! Regression tests for the launchd-PATH gap that broke v0.35.0
    //! production: launchd hands `/Applications/K2SO.app` and the daemon
    //! a sparse PATH (`/usr/bin:/bin:/usr/sbin:/sbin`). Spawns of user-
    //! installed tools like `claude` failed with ENOENT until the
    //! `enrich_path_from_login_shell` helper landed in 0.35.1.
    //!
    //! These tests run in `cargo test`'s shell-inherited (rich) PATH
    //! by default, so we can't rely on the ambient env to reproduce the
    //! bug — we deliberately pave PATH down to the launchd default
    //! before calling the helper, then assert it widens.
    use super::*;
    use std::sync::Mutex;

    /// `std::env::set_var` is not thread-safe and these tests mutate it
    /// directly. Serialize them so they don't race each other when the
    /// test runner uses multiple threads.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Lock + recover from poison so a panic in one test doesn't
    /// fail the next one with an opaque PoisonError.
    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn with_sparse_path<F: FnOnce()>(f: F) {
        let _g = lock_env();
        let original = std::env::var("PATH").ok();
        // Standard launchd default — what `/Applications/K2SO.app` and
        // the daemon actually see in production after a fresh install.
        std::env::set_var("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
        f();
        match original {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
    }

    #[test]
    fn enrich_path_widens_sparse_launchd_default() {
        with_sparse_path(|| {
            let before = std::env::var("PATH").unwrap();
            assert_eq!(before, "/usr/bin:/bin:/usr/sbin:/sbin");
            enrich_path_from_login_shell();
            let after = std::env::var("PATH").unwrap();
            // The login shell's PATH is set in the user's rc files. We
            // assert the helper produced *something different and longer*
            // — the exact contents are user-environment-specific.
            assert_ne!(
                before, after,
                "expected enrich_path_from_login_shell to widen PATH; got the same launchd-default value"
            );
            assert!(
                after.len() > before.len(),
                "expected widened PATH to be longer than launchd default; got before={before} after={after}"
            );
        });
    }

    #[test]
    fn enrich_path_safe_to_call_multiple_times() {
        // Production calls the helper exactly once at startup, so
        // strict idempotency isn't required — but it must be safe to
        // invoke repeatedly without crashing or producing an empty
        // PATH. (Some users' rc files reorder dirs across invocations,
        // so equality across calls is too strict an assertion.)
        let _g = lock_env();
        let before = std::env::var("PATH").unwrap_or_default();
        enrich_path_from_login_shell();
        let after_one = std::env::var("PATH").unwrap_or_default();
        enrich_path_from_login_shell();
        let after_two = std::env::var("PATH").unwrap_or_default();
        assert!(
            !after_one.is_empty(),
            "PATH must not become empty after first enrich (before={before})"
        );
        assert!(
            !after_two.is_empty(),
            "PATH must not become empty after second enrich"
        );
        // Bound any drift: a second call shouldn't massively grow the
        // string (e.g. by re-prepending user dirs over and over). 2x
        // headroom catches real runaway growth without flaking on
        // legitimate reordering.
        assert!(
            after_two.len() < after_one.len() * 2,
            "second enrich call doubled PATH length — likely a runaway prepend"
        );
    }
}
