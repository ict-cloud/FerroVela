fn main() {
    // Embed Info.plist into the binary so macOS recognises the bundle identifier
    // even when the app is run as a bare binary (outside a .app bundle).
    // This suppresses the "missing main bundle identifier" / ViewBridge / task-port errors.
    #[cfg(target_os = "macos")]
    {
        let version = std::env::var("CARGO_PKG_VERSION").unwrap();
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let plist_path = format!("{out_dir}/Info.plist");

        std::fs::write(
            &plist_path,
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple Computer//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>English</string>
  <key>CFBundleDisplayName</key>
  <string>FerroVela</string>
  <key>CFBundleExecutable</key>
  <string>ferrovela</string>
  <key>CFBundleIdentifier</key>
  <string>com.ictcloud.ferrovela</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>FerroVela</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>{version}</string>
  <key>CFBundleVersion</key>
  <string>{version}</string>
  <key>CSResourcesFileMapped</key>
  <true/>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.utilities</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSHumanReadableCopyright</key>
  <string>Copyright (c) 2026 Pascal Rudnik</string>
</dict>
</plist>
"#
            ),
        )
        .unwrap();

        println!("cargo:rustc-link-arg=-sectcreate");
        println!("cargo:rustc-link-arg=__TEXT");
        println!("cargo:rustc-link-arg=__info_plist");
        println!("cargo:rustc-link-arg={plist_path}");
        println!("cargo:rerun-if-changed=Cargo.toml");
    }
}
