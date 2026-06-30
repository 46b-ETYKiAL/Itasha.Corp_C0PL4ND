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

    emit_windows_exploit_mitigations();

    #[cfg(windows)]
    embed_windows_resource();
}

/// Opt the shipped Windows binaries into the modern exploit-mitigation set
/// (roadmap S-5). These are LINKER flags, so they are emitted via
/// `cargo:rustc-link-arg-bins` (binaries only — never the lib/test crates,
/// which would otherwise inherit MSVC-only flags the test harness rejects) and
/// gated to the MSVC target by `TARGET`. On any non-MSVC target this is a
/// no-op, so a Linux/macOS or `*-windows-gnu` build is unaffected.
///
/// Flags (Microsoft linker docs + BinSkim policy):
/// - `/GUARD:CF`     — Control Flow Guard (indirect-call target validation)
/// - `/CETCOMPAT`    — Intel CET shadow-stack compatibility (ROP defense)
/// - `/HIGHENTROPYVA`— 64-bit high-entropy ASLR
/// - `/DYNAMICBASE`  — ASLR (relocatable image; default, asserted explicitly)
/// - `/NXCOMPAT`     — DEP (data-execution prevention; default, asserted)
fn emit_windows_exploit_mitigations() {
    // `TARGET` is the cross-compilation target triple cargo always sets for a
    // build script — the correct signal even when cross-compiling from Linux.
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.ends_with("-windows-msvc") {
        return;
    }
    // `rustc-link-arg-bins=<arg>` passes `<arg>` straight to the linker for
    // binary targets only — exactly the MSVC `link.exe` switches we want.
    // These four are valid on every MSVC architecture (x86 and ARM64).
    let mut flags = vec!["/GUARD:CF", "/HIGHENTROPYVA", "/DYNAMICBASE", "/NXCOMPAT"];
    // `/CETCOMPAT` (Intel CET shadow-stack) is an x86-ONLY feature; the ARM64
    // linker rejects it with `LNK1246: '/CETCOMPAT' not compatible with 'ARM64'
    // target machine`. ARM64 Windows has its own ROP defense (pointer
    // authentication / the platform's hardware CFI), so emit `/CETCOMPAT` only
    // for the x86 MSVC targets and skip it on `aarch64-pc-windows-msvc`.
    if target.starts_with("x86_64") || target.starts_with("i686") {
        flags.push("/CETCOMPAT");
    }
    for flag in flags {
        println!("cargo:rustc-link-arg-bins={flag}");
    }
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
