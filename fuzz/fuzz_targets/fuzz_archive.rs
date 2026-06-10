#![no_main]
//! Fuzz target for the update-engine archive-extraction SHAPE.
//!
//! The update engine downloads a release archive (`.zip` on Windows, `.tar.gz`
//! on unix) and extracts the single `c0pl4nd` / `c0pl4nd.exe` binary from it.
//! Even though the production path only reaches extraction AFTER a SHA-256 +
//! minisign verify (`extract_binary` is documented as "NEVER reached without a
//! passing `verify_artifact`"), the decompression code itself walks
//! attacker-shaped archive structure — zip central-directory / tar headers /
//! flate2 inflate — and is exactly the kind of byte-parsing surface a fuzzer
//! should cover (zip-bombs, malformed headers, path-traversal entry names,
//! truncated streams).
//!
//! ## Why a SHIM, not the real function
//!
//! The real `extract_binary_zip` / `extract_binary_targz` live in the app crate
//! (`crates/app/src/update_engine/net.rs`). That crate is a BINARY crate — it
//! has only `[[bin]]` targets and NO `[lib]`, so its private functions are not
//! importable from this (or any) external crate. Rather than add a `[lib]`
//! target or make the functions `pub` purely to fuzz them (a wider visibility
//! change owned by another stream, and explicitly out of this target's scope),
//! this fuzz target drives a SELF-CONTAINED shim below that faithfully mirrors
//! the production bounded-extraction logic against the SAME exact-pinned
//! `zip` / `tar` / `flate2` crate versions:
//!
//!   * entry-count cap  (`MAX_ARCHIVE_ENTRIES = 64`)            — decompress-bomb
//!   * copy-capped inflate (`MAX_EXTRACTED_BYTES`, saturating)  — decompress-bomb
//!   * `enclosed_name()` traversal-skip on the zip path         — path-traversal
//!   * `path().file_name()` selection on the tar path           — entry selection
//!   * the binary-name allow-list (`c0pl4nd` / `c0pl4nd.exe`)
//!
//! This is the "thin wrapper into a fuzzable location" the R2 finding calls for:
//! it fuzzes the SHAPE of the extraction logic. If `net.rs` later grows a `[lib]`
//! target, this shim should be swapped for a direct call to the real function.

use std::io::{Cursor, Read, Write};

use libfuzzer_sys::fuzz_target;

// Mirrors the production constants in crates/app/src/update_engine/net.rs.
const MAX_EXTRACTED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 64;

fn binary_file_names() -> [&'static str; 2] {
    ["c0pl4nd", "c0pl4nd.exe"]
}

/// A `Write` sink that mirrors `copy_capped`'s caller without touching the
/// filesystem: it counts bytes and discards them, so the fuzzer exercises the
/// inflate path at full speed and is bounded by the same cap logic.
struct CountingSink {
    written: u64,
}

impl Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.written = self.written.saturating_add(buf.len() as u64);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Faithful mirror of `copy_capped` from net.rs: bounded read→write loop that
/// aborts (returns Err) past `limit`, using saturating arithmetic so a hostile
/// stream can never overflow the byte counter.
fn copy_capped<R: Read, W: Write>(reader: &mut R, writer: &mut W, limit: u64) -> Result<u64, ()> {
    let mut written: u64 = 0;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => return Err(()),
        };
        written = written.saturating_add(n as u64);
        if written > limit {
            return Err(()); // decompression-bomb cap hit
        }
        if writer.write_all(&buf[..n]).is_err() {
            return Err(());
        }
    }
    Ok(written)
}

/// Mirror of `extract_binary_zip`: bounded zip walk with entry-count cap,
/// `enclosed_name()` path-traversal skip, name allow-list, and copy-capped
/// inflate into an in-memory counting sink. Returns whether the binary was
/// found; never panics on arbitrary bytes.
fn extract_zip_shim(archive_bytes: &[u8]) -> Result<bool, ()> {
    let reader = Cursor::new(archive_bytes);
    let mut zip = zip::ZipArchive::new(reader).map_err(|_| ())?;
    // Decompression-bomb guard: refuse an unreasonable entry count up front.
    if zip.len() > MAX_ARCHIVE_ENTRIES {
        return Err(());
    }
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|_| ())?;
        let entry_name = match file.enclosed_name() {
            Some(p) => p.to_path_buf(),
            None => continue, // skip path-traversal entries
        };
        let file_name = match entry_name.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if binary_file_names().contains(&file_name.as_str()) {
            let mut sink = CountingSink { written: 0 };
            copy_capped(&mut file, &mut sink, MAX_EXTRACTED_BYTES)?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Mirror of `extract_binary_targz`: gunzip → tar walk with entry-count cap,
/// `file_name()` selection, name allow-list, and copy-capped inflate into an
/// in-memory counting sink. Never panics on arbitrary bytes.
fn extract_targz_shim(archive_bytes: &[u8]) -> Result<bool, ()> {
    let gz = flate2::read::GzDecoder::new(archive_bytes);
    let mut archive = tar::Archive::new(gz);
    let entries = archive.entries().map_err(|_| ())?;
    let mut entry_count: usize = 0;
    for entry in entries {
        entry_count += 1;
        if entry_count > MAX_ARCHIVE_ENTRIES {
            return Err(()); // decompression-bomb guard
        }
        let mut entry = entry.map_err(|_| ())?;
        let path = entry.path().map_err(|_| ())?;
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if binary_file_names().contains(&file_name.as_str()) {
            let mut sink = CountingSink { written: 0 };
            copy_capped(&mut entry, &mut sink, MAX_EXTRACTED_BYTES)?;
            return Ok(true);
        }
    }
    Ok(false)
}

fuzz_target!(|data: &[u8]| {
    // Feed the SAME arbitrary bytes to BOTH archive shapes — the production
    // `extract_binary` selects by asset-name suffix, but the byte parser is what
    // we fuzz, so we drive both unconditionally. Each shim returns a `Result`
    // (Err on malformed/oversized/over-count input); we never unwrap into a
    // panic. A crash here is a real defect in the extraction SHAPE.
    let _ = extract_zip_shim(data);
    let _ = extract_targz_shim(data);
});
