fn main() {
    if let Some(status) = bridge_lib::dsc::run_probe_child_from_args(std::env::args().skip(1)) {
        std::process::exit(status);
    }

    bridge_lib::run()
}
