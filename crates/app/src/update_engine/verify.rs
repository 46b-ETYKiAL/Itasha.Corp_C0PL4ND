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

/// Full gate (fails closed): an artifact is acceptable IFF its checksum matches
/// (SHA-256 first — catches corruption) AND its signature verifies against the
/// embedded public key (then minisign — catches tampering). Returns `Err` the
/// moment either check fails; the caller NEVER returns an unverified binary.
pub fn verify_artifact(
    bytes: &[u8],
    expected_sha256: &str,
    sig_str: &str,
    public_key_box: &str,
) -> Result<(), String> {
    if !verify_checksum(bytes, expected_sha256) {
        return Err("checksum mismatch".to_string());
    }
    verify_signature(bytes, sig_str, public_key_box)
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
        // accepts; a wrong sha rejects BEFORE the signature is even checked.
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

        assert!(verify_artifact(data, &sha, &sig_str, &pk_box).is_ok());
        // Wrong checksum fails closed even with a valid signature.
        assert_eq!(
            verify_artifact(data, "deadbeef", &sig_str, &pk_box).unwrap_err(),
            "checksum mismatch"
        );
        // Right checksum but a bogus signature also fails closed.
        assert!(verify_artifact(data, &sha, "untrusted comment: x\nbogus", &pk_box).is_err());
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
