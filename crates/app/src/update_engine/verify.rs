//! Update artifact verification: SHA-256 checksum + minisign (ed25519)
//! signature. Defense in depth — the checksum catches corruption, the
//! signature catches tampering. An update is applied ONLY if BOTH pass.
//!
//! The public key is embedded in the binary at build time (a PUBLIC value,
//! safe to commit); the matching secret key lives OUTSIDE the repo as a CI
//! secret (`MINISIGN_SECRET_KEY`), never committed. Only the holder of that
//! secret can produce a signature this gate accepts — so a downloaded binary
//! a MITM or a compromised release asset cannot forge will fail closed.

use sha2::{Digest, Sha256};

/// The embedded minisign public key (full box form) for the C0PL4ND release
/// signing key. PUBLIC — safe to commit. The in-app updater verifies every
/// downloaded artifact against it; only the holder of the matching secret key
/// (the `MINISIGN_SECRET_KEY` GitHub Actions secret, never committed) can
/// produce an accepted signature.
///
/// Generated 2026-06-04 with `rsign generate -W` (rsign2 — the SAME tool the
/// release workflow signs with, mirroring the sibling SCR1B3 editor's
/// `packaging/signing.md`). The passwordless secret key was written to a
/// gitignored path OUTSIDE the repo and is added to CI as the
/// `MINISIGN_SECRET_KEY` repository secret. Until a signed release is cut,
/// verification has no signed artifact to accept and the UI reports "no
/// verified update available" — it NEVER installs an unsigned binary.
pub const EMBEDDED_PUBLIC_KEY: &str = "untrusted comment: minisign public key: A8D869E2B4DD3FD9\nRWTZP9204mnYqKT/TK6OfYG70QwFoHF5WuuxODg8tgPU+WdLRJYt6iNN";

/// Hex-encoded SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Case-insensitive hex comparison of the SHA-256 of `bytes` against the
/// expected digest (the `.sha256` sidecar's first whitespace token).
pub fn verify_checksum(bytes: &[u8], expected_hex: &str) -> bool {
    sha256_hex(bytes).eq_ignore_ascii_case(expected_hex.trim())
}

/// Verify a minisign signature (`sig_str` = the `.minisig` file contents)
/// against `bytes` using the given public-key box string.
pub fn verify_signature(bytes: &[u8], sig_str: &str, public_key_box: &str) -> Result<(), String> {
    let pk = minisign_verify::PublicKey::decode(public_key_box)
        .map_err(|e| format!("bad public key: {e}"))?;
    let sig =
        minisign_verify::Signature::decode(sig_str).map_err(|e| format!("bad signature: {e}"))?;
    pk.verify(bytes, &sig, false)
        .map_err(|e| format!("signature verification failed: {e}"))
}

/// Defense-in-depth (audit P3-#1): assert the signature's *trusted comment* binds
/// to the asset we believe we downloaded. The trusted comment is part of the
/// cryptographically-signed blob (minisign signs `signature || trusted_comment`),
/// so a same-key signature produced over a DIFFERENT asset (e.g. signing the
/// Windows archive and serving it as the Linux one) carries that asset's name in
/// its trusted comment and is caught here — a layer above the `.sha256` sidecar +
/// target-triple asset matching that already constrain this.
///
/// `rsign2`/`minisign` write a default trusted comment of the form
/// `timestamp:<unix>\tfile:<basename>[\tprehashed]` when no explicit `-t` is
/// given (the C0PL4ND release workflow signs exactly this way). We bind on the
/// `file:` token: if present, its value MUST equal `expected_asset` (the asset
/// basename the updater resolved from the target-triple-named release asset).
///
/// Conservative-by-construction so a legitimate update is never broken: if the
/// trusted comment carries NO `file:` token (a hand-rolled custom comment), the
/// binding is skipped rather than failed — the cryptographic signature + checksum
/// remain the load-bearing gates and this is purely additive. A `file:` token
/// that is PRESENT but MISMATCHED is a hard failure (fail-closed).
pub fn verify_signature_bound(
    bytes: &[u8],
    sig_str: &str,
    public_key_box: &str,
    expected_asset: &str,
) -> Result<(), String> {
    // Cryptographic check first — a bad signature is rejected before we even
    // look at the (now-trusted) comment.
    verify_signature(bytes, sig_str, public_key_box)?;

    let sig =
        minisign_verify::Signature::decode(sig_str).map_err(|e| format!("bad signature: {e}"))?;
    trusted_comment_binds_asset(sig.trusted_comment(), expected_asset)
}

/// The basename of a path-or-filename: the final component after any `/` or `\`
/// separator (so `release/foo.zip` and `foo.zip` both yield `foo.zip`).
fn basename(s: &str) -> &str {
    let s = s.trim();
    s.rsplit(['/', '\\']).next().unwrap_or(s).trim()
}

/// Bind a minisign trusted comment to the asset the updater resolved. The
/// comment is `\t`-separated `key:value` fields; if a `file:` token is present
/// its BASENAME must equal the BASENAME of `expected_asset` (case-insensitive).
///
/// Both sides are reduced to their basename so a signer invoked with a path
/// argument (e.g. `rsign sign release/foo.zip`, which writes
/// `file:release/foo.zip`) still matches the bare downloaded `foo.zip`. Without
/// this, a correctly-signed artifact was rejected with "signature trusted-comment
/// file mismatch" — the `release/`-prefix in-app-updater failure this fixes
/// (alongside the release workflow now signing bare filenames).
///
/// Conservative-by-construction: a comment with NO `file:` token returns `Ok`
/// (the binding is purely additive over the cryptographic signature + checksum);
/// a `file:` token PRESENT but with a mismatched basename is a hard failure.
fn trusted_comment_binds_asset(trusted: &str, expected_asset: &str) -> Result<(), String> {
    if let Some(signed_file) = trusted
        .split('\t')
        .find_map(|field| field.trim().strip_prefix("file:"))
    {
        let signed = basename(signed_file);
        let expected = basename(expected_asset);
        if !signed.eq_ignore_ascii_case(expected) {
            return Err(format!(
                "signature trusted-comment file mismatch: signed for {signed:?}, \
                 expected {expected:?}"
            ));
        }
    }
    Ok(())
}

/// Full gate (fails closed): an artifact is acceptable IFF its checksum matches
/// (SHA-256 first — catches corruption) AND its signature verifies against the
/// embedded public key with the signed trusted-comment `file:` token bound to
/// `expected_asset` (then minisign — catches tampering + same-key wrong-artifact
/// substitution). Returns `Err` the moment any check fails; the caller NEVER
/// returns an unverified binary. The `_bound` suffix denotes the trusted-comment
/// asset binding added in audit P3-#1 (see [`verify_signature_bound`]); the
/// updater always knows the asset name it resolved, so this is the only gate it
/// needs.
pub fn verify_artifact_bound(
    bytes: &[u8],
    expected_sha256: &str,
    sig_str: &str,
    public_key_box: &str,
    expected_asset: &str,
) -> Result<(), String> {
    if !verify_checksum(bytes, expected_sha256) {
        return Err("checksum mismatch".to_string());
    }
    verify_signature_bound(bytes, sig_str, public_key_box, expected_asset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        // SHA-256("abc")
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn trusted_comment_binding_compares_basenames() {
        let asset = "c0pl4nd-v0.4.8-x86_64-pc-windows-msvc.zip";

        // Bare filename in the trusted comment (the intended form) → matches.
        assert!(
            trusted_comment_binds_asset(&format!("timestamp:123\tfile:{asset}"), asset).is_ok()
        );

        // REGRESSION (the in-app-update failure): the signer was invoked with a
        // `release/` PATH prefix, so the trusted comment carries
        // `file:release/<asset>`. The basename comparison must still accept it.
        assert!(
            trusted_comment_binds_asset(&format!("timestamp:123\tfile:release/{asset}"), asset)
                .is_ok(),
            "a release/-prefixed trusted comment must bind to the bare asset"
        );
        // Windows-style separator too.
        assert!(trusted_comment_binds_asset(
            &format!("timestamp:123\tfile:release\\{asset}"),
            asset
        )
        .is_ok());

        // Case-insensitive on the filename.
        assert!(trusted_comment_binds_asset(
            &format!("timestamp:123\tfile:{}", asset.to_uppercase()),
            asset
        )
        .is_ok());

        // A genuinely DIFFERENT asset (wrong-artifact substitution) → hard fail.
        let err = trusted_comment_binds_asset(
            "timestamp:123\tfile:c0pl4nd-v0.4.8-x86_64-unknown-linux-gnu.tar.gz",
            asset,
        )
        .unwrap_err();
        assert!(
            err.contains("mismatch"),
            "wrong asset must be rejected: {err}"
        );

        // No `file:` token (hand-rolled comment) → binding skipped (additive).
        assert!(trusted_comment_binds_asset("timestamp:123", asset).is_ok());
    }

    #[test]
    fn checksum_match_and_mismatch() {
        let data = b"c0pl4nd release artifact";
        let good = sha256_hex(data);
        assert!(verify_checksum(data, &good));
        assert!(verify_checksum(data, &good.to_uppercase())); // case-insensitive
        assert!(verify_checksum(data, &format!("  {good}  "))); // trims whitespace
        assert!(!verify_checksum(data, "deadbeef"));
    }

    #[test]
    fn signature_roundtrip_accepts_valid_rejects_tampered() {
        // Sign with the dev-only `minisign` crate, verify with the production
        // `minisign-verify` path. Proves the verify path accepts a real sig and
        // rejects tampered data — the verify-before-swap contract.
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pk_box = kp.pk.to_box().unwrap().to_string();
        let data = b"the new c0pl4nd binary bytes";
        let sig_box = minisign::sign(
            Some(&kp.pk),
            &kp.sk,
            std::io::Cursor::new(&data[..]),
            Some("c0pl4nd v9.9.9"),
            Some("comment"),
        )
        .unwrap();
        let sig_str = sig_box.to_string();

        // Valid signature over the exact bytes -> accepted.
        assert!(verify_signature(data, &sig_str, &pk_box).is_ok());

        // Tampered bytes -> rejected.
        let tampered = b"the new c0pl4nd binary bytez";
        assert!(verify_signature(tampered, &sig_str, &pk_box).is_err());
    }

    #[test]
    fn verify_artifact_requires_both_checksum_and_signature() {
        // A full round-trip through the combined gate: correct sha + valid sig
        // accepts; a wrong sha rejects BEFORE the signature is even checked. Uses
        // a trusted comment with no `file:` token, so the asset binding is a
        // no-op here and this exercises the checksum+signature path in isolation.
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pk_box = kp.pk.to_box().unwrap().to_string();
        let data = b"c0pl4nd-x86_64-pc-windows-msvc.tar.gz bytes";
        let sig_box = minisign::sign(
            Some(&kp.pk),
            &kp.sk,
            std::io::Cursor::new(&data[..]),
            Some("c0pl4nd"),
            Some("c"),
        )
        .unwrap();
        let sig_str = sig_box.to_string();
        let sha = sha256_hex(data);
        let asset = "c0pl4nd-x86_64-pc-windows-msvc.tar.gz";

        assert!(verify_artifact_bound(data, &sha, &sig_str, &pk_box, asset).is_ok());
        // Wrong checksum fails closed even with a valid signature.
        assert_eq!(
            verify_artifact_bound(data, "deadbeef", &sig_str, &pk_box, asset).unwrap_err(),
            "checksum mismatch"
        );
        // Right checksum but a bogus signature also fails closed.
        assert!(
            verify_artifact_bound(data, &sha, "untrusted comment: x\nbogus", &pk_box, asset)
                .is_err()
        );
    }

    #[test]
    fn trusted_comment_binding_accepts_match_rejects_mismatch() {
        // The release workflow signs with rsign2's DEFAULT trusted comment,
        // which embeds `file:<basename>`. We bind on that token: a signature
        // whose trusted comment names a DIFFERENT asset is rejected even though
        // it verifies cryptographically against the same key (the same-key
        // wrong-artifact substitution this layer defends against).
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pk_box = kp.pk.to_box().unwrap().to_string();
        let data = b"the linux archive bytes";

        // Sign with a trusted comment naming the LINUX asset (mirrors the
        // rsign2 default `file:<basename>` shape).
        let sig_linux = minisign::sign(
            Some(&kp.pk),
            &kp.sk,
            std::io::Cursor::new(&data[..]),
            Some("timestamp:1700000000\tfile:c0pl4nd-v1.0.0-x86_64-unknown-linux-gnu.tar.gz"),
            Some("comment"),
        )
        .unwrap()
        .to_string();

        // Matching asset name -> accepted.
        assert!(verify_signature_bound(
            data,
            &sig_linux,
            &pk_box,
            "c0pl4nd-v1.0.0-x86_64-unknown-linux-gnu.tar.gz",
        )
        .is_ok());

        // Same key, same bytes, but we expected the WINDOWS asset -> the
        // trusted-comment `file:` token mismatches -> rejected.
        let err = verify_signature_bound(
            data,
            &sig_linux,
            &pk_box,
            "c0pl4nd-v1.0.0-x86_64-pc-windows-msvc.zip",
        )
        .unwrap_err();
        assert!(err.contains("trusted-comment file mismatch"), "{err}");

        // A staging-dir-prefixed expected name still binds on the basename.
        assert!(verify_signature_bound(
            data,
            &sig_linux,
            &pk_box,
            "/tmp/staging/c0pl4nd-v1.0.0-x86_64-unknown-linux-gnu.tar.gz",
        )
        .is_ok());
    }

    #[test]
    fn trusted_comment_binding_is_skipped_when_no_file_token() {
        // Conservative-by-construction: a custom trusted comment with NO `file:`
        // token does NOT break verification — the binding is purely additive on
        // top of the load-bearing checksum + signature gates.
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pk_box = kp.pk.to_box().unwrap().to_string();
        let data = b"archive bytes with custom comment";
        let sig = minisign::sign(
            Some(&kp.pk),
            &kp.sk,
            std::io::Cursor::new(&data[..]),
            Some("c0pl4nd v1.2.3"), // no `file:` token
            Some("comment"),
        )
        .unwrap()
        .to_string();
        // Binding skipped -> any expected asset name passes (crypto still gates).
        assert!(verify_signature_bound(data, &sig, &pk_box, "anything.tar.gz").is_ok());
        // But tampered bytes still fail closed.
        assert!(verify_signature_bound(b"tampered", &sig, &pk_box, "anything.tar.gz").is_err());
    }

    #[test]
    fn verify_artifact_bound_requires_checksum_signature_and_asset_binding() {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pk_box = kp.pk.to_box().unwrap().to_string();
        let data = b"c0pl4nd-v2.0.0-x86_64-pc-windows-msvc.zip bytes";
        let sig = minisign::sign(
            Some(&kp.pk),
            &kp.sk,
            std::io::Cursor::new(&data[..]),
            Some("timestamp:1700000001\tfile:c0pl4nd-v2.0.0-x86_64-pc-windows-msvc.zip"),
            Some("c"),
        )
        .unwrap()
        .to_string();
        let sha = sha256_hex(data);
        let asset = "c0pl4nd-v2.0.0-x86_64-pc-windows-msvc.zip";

        // Correct sha + valid sig + matching asset name -> accepted.
        assert!(verify_artifact_bound(data, &sha, &sig, &pk_box, asset).is_ok());
        // Wrong checksum fails closed BEFORE the signature is even checked.
        assert_eq!(
            verify_artifact_bound(data, "deadbeef", &sig, &pk_box, asset).unwrap_err(),
            "checksum mismatch"
        );
        // Right sha + valid sig but the WRONG expected asset -> rejected.
        assert!(verify_artifact_bound(data, &sha, &sig, &pk_box, "wrong-asset.tar.gz").is_err());
    }

    #[test]
    fn embedded_key_decodes() {
        // The committed embedded key must be a well-formed minisign public key
        // (so a real release signature CAN verify against it).
        assert!(minisign_verify::PublicKey::decode(EMBEDDED_PUBLIC_KEY).is_ok());
    }

    #[test]
    fn embedded_key_rejects_bogus_signatures() {
        // The embedded release key must reject a malformed / forged signature
        // (it only accepts artifacts signed by the matching secret key) — this
        // is the "fails closed until a signed release exists" guarantee.
        assert!(
            verify_signature(b"x", "untrusted comment: x\nbogus", EMBEDDED_PUBLIC_KEY).is_err()
        );
    }
}
