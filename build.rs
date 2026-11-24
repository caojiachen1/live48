// Embeds the same icon the Tauri build shipped, so Explorer and the taskbar show the
// live48 artwork instead of the generic exe. The .res is prebuilt from
// `assets/icon.ico` (windres -O res) because an MSVC resource compiler is not
// guaranteed to exist on the build machine; link.exe accepts a .res directly.
fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icon.res");
        let res = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icon.res");
        // Paths are handed to the linker unquoted, so this relies on the crate living
        // somewhere without spaces; the workspace path is the normal case.
        println!("cargo:rustc-link-arg={}", res.display());
    }
}
