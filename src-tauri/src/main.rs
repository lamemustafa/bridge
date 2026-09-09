fn main() {
    if let Some(status) =
        bridge_lib::run_journal_confirmation_child_from_args(std::env::args().skip(1))
    {
        std::process::exit(status);
    }

    if let Some(status) = bridge_lib::dsc::run_probe_child_from_args(std::env::args().skip(1)) {
        std::process::exit(status);
    }

    bridge_lib::run(compiler_cache_context)
}

// Generate embedded configuration and assets in the executable, outside the
// cached library. The factory preserves the context's original startup timing.
fn compiler_cache_context() -> tauri::Context<tauri::Wry> {
    tauri::generate_context!()
}
