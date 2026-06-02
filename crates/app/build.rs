//! Build script — embeds the C0PL4ND sigil icon + a Windows version resource
//! into the compiled binaries.
//!
//! Why this exists: the Start-menu shortcut, taskbar button, Explorer listing,
//! and the Add/Remove-Programs entry all read their icon FROM the `.exe`
//! itself. Without an embedded icon resource the exe has none, so every one of
//! those surfaces renders the blank/default placeholder — which is exactly the
//! "no icon on shortcuts" symptom. Embedding `packaging/windows/c0pl4nd.ico`
//! (the sigil: CRT plate + cyan prompt chevron/cursor + violet keyline, derived
//! from `assets/svg/app-icon.svg`) fixes all of them at once.
//!
//! The version resource (ProductName / FileDescription / Comments) is what
//! Windows Search indexes for an executable, which also improves Start-menu
//! discoverability.
//!
//! Windows-only: the resource code is `#[cfg(windows)]`-gated so it is compiled
//! ONLY on a Windows host. `winresource` is a host-gated build-dependency
//! (`[target.'cfg(windows)'.build-dependencies]`), so it is absent on
//! Linux/macOS hosts — the cfg gate (not a runtime check) is what keeps the
//! build script compiling there.

fn main() {
    // Re-run if the icon changes.
    println!("cargo:rerun-if-changed=../../packaging/windows/c0pl4nd.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    embed_windows_resource();
}

#[cfg(windows)]
fn embed_windows_resource() {
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set by cargo");
    let icon = std::path::Path::new(&manifest_dir).join("../../packaging/windows/c0pl4nd.ico");
    assert!(
        icon.is_file(),
        "app icon not found at {} — packaging/windows/c0pl4nd.ico is required to embed the exe icon",
        icon.display()
    );

    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon.to_str().expect("icon path is valid UTF-8"));
    res.set("ProductName", "C0PL4ND");
    // FileDescription + Comments are indexed by Windows Search; include the
    // phonetic "Copland" so a search for that spelling also surfaces the app.
    res.set("FileDescription", "C0PL4ND terminal (Copland)");
    res.set(
        "Comments",
        "C0PL4ND — fast, local-first terminal (a.k.a. Copland).",
    );
    res.set("CompanyName", "Itasha.Corp");
    res.set("LegalCopyright", "© Itasha.Corp");
    res.set("OriginalFilename", "c0pl4nd.exe");
    res.set("InternalName", "c0pl4nd");

    // Fail loud on Windows: the release runner (windows-latest, MSVC) ships
    // `rc.exe`, so a failure here means we would otherwise ship a blank-icon
    // exe — exactly the regression this build script exists to prevent.
    res.compile().expect(
        "embed Windows icon + version resource (requires the MSVC resource compiler rc.exe)",
    );
}
