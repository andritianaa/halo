fn main() {
    // GPUI builds deep element trees; Windows' default 1 MB main-thread stack
    // overflows in debug builds when the hub renders. Reserve 16 MB.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg-bin=halo=/STACK:16777216");
        // App icon (taskbar, Explorer, installer). GPUI embeds the app manifest
        // (asInvoker); elevation is requested at startup instead (see `main`).
        println!("cargo:rerun-if-changed=halo.rc");
        println!("cargo:rerun-if-changed=../../assets/icons/halo.ico");
        let _ = embed_resource::compile("halo.rc", embed_resource::NONE);
    }
}
