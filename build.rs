fn main() {
    // Embed Info.plist into the binary so macOS recognises the bundle identifier
    // even when the app is run as a bare binary (outside a .app bundle).
    // This suppresses the "missing main bundle identifier" / ViewBridge / task-port errors.
    #[cfg(target_os = "macos")]
    {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo:rustc-link-arg=-sectcreate");
        println!("cargo:rustc-link-arg=__TEXT");
        println!("cargo:rustc-link-arg=__info_plist");
        println!("cargo:rustc-link-arg={manifest_dir}/Info.plist");
        println!("cargo:rerun-if-changed=Info.plist");
    }
}
