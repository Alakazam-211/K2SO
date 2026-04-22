// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Check if invoked as an LLM worker subprocess
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--llm-worker" {
        k2so_lib::llm_worker_main(&args[2]);
        return;
    }

    // Fire the reqwest pool warmup IMMEDIATELY — before Tauri even
    // starts parsing the window config. reqwest::blocking's tokio
    // runtime takes ~500-800ms to materialize on first send(). By
    // spawning this thread at the very top of main(), it has a
    // head start on daemon startup + window hydration. Restored
    // terminals that spawn during React rehydration then hit an
    // already-warm pool instead of paying 600ms of first-call cost.
    k2so_lib::warm_http_pool_async();

    k2so_lib::run()
}
