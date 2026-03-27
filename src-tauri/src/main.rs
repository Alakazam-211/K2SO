// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Check if invoked as an LLM worker subprocess
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--llm-worker" {
        k2so_lib::llm_worker_main(&args[2]);
        return;
    }

    k2so_lib::run()
}
