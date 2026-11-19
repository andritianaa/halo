//! Probe: prints the brightness level and radio states, then re-applies the
//! *current* brightness (no visible change) to check that setting works.
fn main() {
    env_logger::Builder::new().filter_level(log::LevelFilter::Info).init();
    let b = halo_platform::controls::Brightness::spawn();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let level = b.level();
    println!("brightness: {level:?}");
    if let Some(l) = level {
        b.set(l);
        std::thread::sleep(std::time::Duration::from_millis(1200));
        println!("after re-applying {l}: {:?}", b.level());
    }
    println!("volume: {:?}", halo_platform::sys::volume_fast());
    for k in [halo_platform::controls::RadioType::Wifi, halo_platform::controls::RadioType::Bluetooth] {
        println!("{k:?} on: {:?}", halo_platform::controls::radio_on(k));
    }
}
