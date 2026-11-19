#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if !cc_router_lib::session_monitor::cli_entry() {
        cc_router_lib::run();
    }
}
