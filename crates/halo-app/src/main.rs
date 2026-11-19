//! Halo — Windows utility suite: notch, tray, settings hub and modules M1–M19.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod devcmd;
mod floating;
mod hub;
mod launcher;
mod overlay;
mod palette;
mod panels;
mod panels2;
mod panels3;
mod panels4;
mod search;
mod state;
mod tray;
mod ui;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    // Drop helper mode (`--drop-helper <halo pid> <ipc dir>`): a non-elevated companion
    // that receives files dragged onto the notch (see `halo_platform::drophelper`).
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.get(1).map(String::as_str) == Some("--drop-helper") {
            if let (Some(pid), Some(dir)) = (args.get(2).and_then(|p| p.parse().ok()), args.get(3)) {
                halo_platform::drophelper::run(dir.into(), pid);
            }
            return;
        }
    }
    // Release builds always run as administrator (network limiter, live per-app
    // speeds, elevated startup task). Started normally, Halo asks for elevation and
    // the non-elevated instance exits; started by the sign-in task it already is.
    #[cfg(all(windows, not(debug_assertions)))]
    if !halo_platform::is_elevated() {
        if !halo_platform::relaunch_elevated() {
            log::warn!("administrator rights declined: Halo does not start without them");
        }
        return;
    }
    log::info!("Halo {} starting", env!("CARGO_PKG_VERSION"));

    gpui_kit::application().with_assets(halo_ui::HaloAssets).run(|cx| {
        gpui_kit::init(cx);
        let typo = halo_ui::load_fonts(cx);
        let shared = state::AppState::load(typo);
        halo_ui::init_theme(cx, shared.borrow().mode);
        launcher::prewarm();
        if let Err(err) = overlay::open(cx, shared) {
            log::error!("cannot open the overlay window: {err:#}");
            cx.quit();
        }
    });
}
