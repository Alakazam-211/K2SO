//! Kessel spawn + shell-exit throughput bench.
//!
//! Directly exercises `spawn_session_stream` — the exact code path
//! a Kessel tab takes — and times how long it takes a spawned
//! `zsh -ilc 'exit'` to run to completion. Compares the two modes:
//!
//!   - `track_alacritty_term: false` — production default (fast
//!     path after the 2026-04-21 PTY-drain fix)
//!   - `track_alacritty_term: true`  — test mode with dual-parse
//!     (the old production behavior that Rosson benchmarked as
//!     4.6x slower than Alacritty)
//!
//! Also drops through the shell-without-rc path (`zsh -c 'exit'`)
//! to show the PTY-overhead floor.
//!
//! Run with:
//!
//! ```bash
//! cargo run --release --example kessel_spawn_bench --features session_stream
//! ```
//!
//! Interpretation: the gap between the two `zsh -ilc 'exit'` modes
//! is the CPU cost of the alacritty Term dual-parse, measured in
//! wall-clock terms as seen by the child. If the fix landed the gap
//! should be several hundred ms per run.
//!
//! This bench is NOT a criterion benchmark — it's intentionally
//! simple wall-clock Instant::now() math so the signal is obvious.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k2so_core::session::SessionId;
use k2so_core::terminal::event_sink::TerminalEventSink;
use k2so_core::terminal::grid_types::GridUpdate;
use k2so_core::terminal::{spawn_session_stream, SpawnConfig, TerminalManager};

/// Minimal event sink for the Alacritty-path bench. Captures the
/// exit event so the bench can wait for it.
struct ExitWaiter {
    exited: Arc<Mutex<Option<i32>>>,
}

impl TerminalEventSink for ExitWaiter {
    fn on_title(&self, _: &str, _: &str) {}
    fn on_bell(&self, _: &str) {}
    fn on_exit(&self, _: &str, exit_code: i32) {
        *self.exited.lock().unwrap() = Some(exit_code);
    }
    fn on_grid_update(&self, _: &str, _: &GridUpdate) {}
}

fn run_alacritty(label: &str, command: &str) -> Duration {
    let started = Instant::now();
    let mut mgr = TerminalManager::new();
    let exited = Arc::new(Mutex::new(None));
    let sink = Arc::new(ExitWaiter {
        exited: Arc::clone(&exited),
    });
    let id = format!("alabench-{}", SessionId::new());
    mgr.create(
        id.clone(),
        "/tmp".into(),
        Some(command.to_string()),
        None,
        Some(80),
        Some(24),
        sink,
    )
    .expect("alacritty create");

    // Poll for exit. Capped at 10s.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if exited.lock().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            panic!("{label}: alacritty terminal did not exit in 10s");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let dur = started.elapsed();
    println!("{label:<60}  {:>7.1}ms", dur.as_secs_f64() * 1000.0);
    dur
}

fn run_one(
    label: &str,
    command: Option<String>,
    args: Option<Vec<String>>,
    track_alacritty_term: bool,
) -> Duration {
    let started = Instant::now();
    let cfg = SpawnConfig {
        session_id: SessionId::new(),
        cwd: "/tmp".into(),
        command,
        args,
        cols: 80,
        rows: 24,
        track_alacritty_term,
    };
    let mut session =
        spawn_session_stream(cfg).expect("spawn_session_stream should succeed");

    // Wait for the child to exit. `zsh -ilc 'exit'` should exit
    // nearly immediately after rc files are loaded + the final
    // `exit` runs. 10s is a generous safety ceiling.
    let exited = session.wait_for_exit(Duration::from_secs(10));
    assert!(exited, "{label}: child did not exit in 10s");

    // Also drain the reader so the measurement includes full
    // cleanup — same ordering the daemon's Drop path uses.
    session.wait_for_reader_drain(Duration::from_secs(5));

    let dur = started.elapsed();
    println!("{label:<60}  {:>7.1}ms", dur.as_secs_f64() * 1000.0);
    dur
}

fn main() {
    println!("Kessel spawn bench — times wall-clock from spawn_session_stream()");
    println!("{}", "-".repeat(80));

    // 1. Floor: shell with no rc files. This is the pure PTY +
    //    process-startup overhead. Should be near-identical between
    //    the two modes.
    println!("\n[zsh -c 'exit'] — no rc files (PTY floor)");
    run_one(
        "  track_alacritty_term: false  (production fast path)",
        Some("zsh".into()),
        Some(vec!["-c".into(), "exit".into()]),
        false,
    );
    run_one(
        "  track_alacritty_term: true   (old / test mode)",
        Some("zsh".into()),
        Some(vec!["-c".into(), "exit".into()]),
        true,
    );

    // 2. Rc-loaded shell. This is Rosson's actual benchmark case.
    //    The ~4.6x system-time gap lives here — rc files print
    //    output, which fills the PTY faster than the reader can
    //    drain, causing write() back-pressure on the child.
    println!("\n[zsh -ilc 'exit'] — full rc files (Rosson's benchmark)");
    let fast = run_one(
        "  track_alacritty_term: false  (production fast path)",
        Some("zsh".into()),
        Some(vec!["-ilc".into(), "exit".into()]),
        false,
    );
    let slow = run_one(
        "  track_alacritty_term: true   (old / test mode)",
        Some("zsh".into()),
        Some(vec!["-ilc".into(), "exit".into()]),
        true,
    );

    // 3. Claude version, inside a shell. Same comparison.
    println!("\n[zsh -ilc 'claude --version'] — includes Claude bootstrap");
    run_one(
        "  track_alacritty_term: false  (production fast path)",
        Some("zsh".into()),
        Some(vec!["-ilc".into(), "claude --version".into()]),
        false,
    );
    run_one(
        "  track_alacritty_term: true   (old / test mode)",
        Some("zsh".into()),
        Some(vec!["-ilc".into(), "claude --version".into()]),
        true,
    );

    // 4. Head-to-head: Alacritty backend vs Kessel fast path.
    //    Same command, same machine, same shell — different PTY
    //    reader implementation. This is the actual goalpost.
    println!("\n[Alacritty backend, head-to-head] — zsh -ilc 'exit'");
    let alac1 = run_alacritty("  Alacritty run #1 (cold)", "zsh -ilc exit");
    let alac2 = run_alacritty("  Alacritty run #2 (warm)", "zsh -ilc exit");

    println!("\n[Kessel, head-to-head] — zsh -ilc 'exit'");
    let kes1 = run_one(
        "  Kessel run #1 (cold)",
        Some("zsh".into()),
        Some(vec!["-ilc".into(), "exit".into()]),
        false,
    );
    let kes2 = run_one(
        "  Kessel run #2 (warm)",
        Some("zsh".into()),
        Some(vec!["-ilc".into(), "exit".into()]),
        false,
    );

    // Summary
    println!("\n{}", "-".repeat(80));
    let ratio = slow.as_secs_f64() / fast.as_secs_f64().max(0.001);
    println!(
        "Double-parse cost:        {:.2}x  (dual-parse={:.0}ms, skip-term={:.0}ms)",
        ratio,
        slow.as_secs_f64() * 1000.0,
        fast.as_secs_f64() * 1000.0,
    );
    let alac_avg = (alac1.as_secs_f64() + alac2.as_secs_f64()) / 2.0 * 1000.0;
    let kes_avg = (kes1.as_secs_f64() + kes2.as_secs_f64()) / 2.0 * 1000.0;
    let ratio_vs_alac = kes_avg / alac_avg.max(1.0);
    println!(
        "Kessel vs Alacritty:      {:.2}x  (Alacritty avg={:.0}ms, Kessel avg={:.0}ms)",
        ratio_vs_alac, alac_avg, kes_avg,
    );
    if ratio_vs_alac <= 1.2 {
        println!("✅ Kessel is within 20% of Alacritty — parity achieved.");
    } else if ratio_vs_alac <= 1.5 {
        println!("⚠️  Kessel is 20-50% slower than Alacritty — close.");
    } else {
        println!("❌ Kessel is >1.5x slower than Alacritty — more optimization needed.");
    }
}
