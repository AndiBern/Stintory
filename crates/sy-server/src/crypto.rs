// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Passwords, tokens, TOTP and encryption of stored secrets.
//!
//! - Passwords: argon2id with the crate's defaults (OWASP parameters).
//! - Tokens (session cookie, login challenge): 32 random bytes, base64url;
//!   the database stores only their SHA-256.
//! - TOTP (RFC 6238): HMAC-SHA1, 30 s steps, 6 digits, ±1 step tolerance,
//!   a step is accepted only once.
//! - Recovery codes: 50 random bits, stored as HMAC-SHA256 with the server key.
//! - TOTP secrets are encrypted with XChaCha20-Poly1305 and the server key;
//!   the user id is the associated data, so a secret can't be moved to
//!   another account.

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::ApiError;

/// Shortest accepted password (characters). Length beats complexity rules
/// (NIST SP 800-63B).
pub const MIN_PASSWORD_LEN: usize = 12;
/// Longest accepted password (bytes), bounds hashing cost.
pub const MAX_PASSWORD_BYTES: usize = 1024;

/// Random bytes from the operating system's CSPRNG (via `rand`).
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rand::fill(&mut b[..]);
    b
}

/// A new random token (base64url, no padding).
pub fn new_token() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes::<32>())
}

/// SHA-256 of a token, as stored in the database.
pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Checks the password policy. Returns a message for the user.
pub fn check_password_policy(password: &str) -> Result<(), ApiError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(ApiError::bad_request(format!(
            "the password must have at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(ApiError::bad_request("the password is too long"));
    }
    Ok(())
}

/// Hashes a password (slow on purpose: runs on a blocking thread).
pub async fn hash_password(password: String) -> Result<String, ApiError> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|h| h.to_string())
            .map_err(|e| ApiError::internal(format!("password hashing: {e}")))
    })
    .await?
}

/// Checks a password against a stored hash. Without a hash (unknown user)
/// a dummy hash is checked, so the answer takes as long either way.
pub async fn verify_password(password: String, hash: Option<String>) -> Result<bool, ApiError> {
    tokio::task::spawn_blocking(move || {
        let known = hash.is_some();
        let hash = hash.unwrap_or_else(|| dummy_hash().to_owned());
        let parsed = PasswordHash::new(&hash)
            .map_err(|e| ApiError::internal(format!("stored password hash: {e}")))?;
        let ok = Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();
        Ok(known && ok)
    })
    .await?
}

/// argon2id hash of a random password, for timing equalization.
fn dummy_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| {
        Argon2::default()
            .hash_password(&random_bytes::<16>())
            .map(|h| h.to_string())
            .expect("hashing random bytes")
    })
}

/// A random password for accounts created or reset by an admin.
pub fn temporary_password() -> String {
    // 16 characters from a 32-symbol alphabet: 80 bits
    base32(&random_bytes::<10>()).to_lowercase()
}

/// RFC 4648 base32 without padding.
pub fn base32(data: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in data {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// Encrypts and decrypts small secrets with the server key.
#[derive(Clone)]
pub struct SecretBox {
    cipher: XChaCha20Poly1305,
    key: [u8; 32],
}

impl std::fmt::Debug for SecretBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretBox(..)")
    }
}

impl SecretBox {
    /// Uses `key` (32 bytes).
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            cipher: XChaCha20Poly1305::new_from_slice(&key).expect("32-byte key"),
            key,
        }
    }

    /// `nonce || ciphertext`, bound to `owner`.
    pub fn seal(&self, owner: Uuid, plaintext: &[u8]) -> Result<Vec<u8>, ApiError> {
        let nonce_bytes = random_bytes::<24>();
        let nonce = XNonce::from(nonce_bytes);
        let sealed = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: owner.as_bytes(),
                },
            )
            .map_err(|_| ApiError::internal("encryption failed"))?;
        let mut out = nonce_bytes.to_vec();
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Reverses [`seal`](Self::seal).
    pub fn open(&self, owner: Uuid, sealed: &[u8]) -> Result<Vec<u8>, ApiError> {
        if sealed.len() < 24 {
            return Err(ApiError::internal("stored secret is damaged"));
        }
        let (nonce, ciphertext) = sealed.split_at(24);
        let nonce = XNonce::try_from(nonce).map_err(|_| ApiError::internal("bad nonce"))?;
        self.cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad: owner.as_bytes(),
                },
            )
            .map_err(|_| {
                ApiError::internal(
                    "stored secret can't be decrypted (was STINTORY_SECRET_KEY changed?)",
                )
            })
    }

    /// HMAC-SHA256 with the server key (recovery codes).
    pub fn mac(&self, data: &[u8]) -> Vec<u8> {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.key).expect("any key length");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    }
}

/// TOTP time step length (s).
pub const TOTP_STEP_S: u64 = 30;

/// The 6-digit TOTP code for a time step.
pub fn totp_code(secret: &[u8], step: u64) -> u32 {
    let mut mac = <Hmac<sha1::Sha1> as KeyInit>::new_from_slice(secret).expect("any key length");
    mac.update(&step.to_be_bytes());
    let hash = mac.finalize().into_bytes();
    let offset = (hash[19] & 0x0f) as usize;
    let bin = u32::from_be_bytes([
        hash[offset] & 0x7f,
        hash[offset + 1],
        hash[offset + 2],
        hash[offset + 3],
    ]);
    bin % 1_000_000
}

/// The current time step.
pub fn totp_step_now() -> u64 {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    u64::try_from(now).unwrap_or(0) / TOTP_STEP_S
}

/// Checks `code` against the steps around `now`. Steps at or before
/// `last_used` are rejected (a code works once). Returns the matching step.
pub fn totp_verify(secret: &[u8], code: &str, now: u64, last_used: Option<u64>) -> Option<u64> {
    let code = code.trim().replace(' ', "");
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let code: u32 = code.parse().ok()?;
    [now.saturating_sub(1), now, now + 1]
        .into_iter()
        .filter(|&s| last_used.is_none_or(|l| s > l))
        .find(|&s| totp_code(secret, s) == code)
}

/// `otpauth://` URI for authenticator apps.
pub fn totp_uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    let enc = |s: &str| -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits=6&period={TOTP_STEP_S}",
        enc(issuer),
        enc(account),
        base32(secret),
        enc(issuer)
    )
}

/// QR code of `text` as SVG.
pub fn qr_svg(text: &str) -> Result<String, ApiError> {
    let code = qrcode::QrCode::new(text.as_bytes())
        .map_err(|e| ApiError::internal(format!("QR code: {e}")))?;
    Ok(code
        .render::<qrcode::render::svg::Color<'_>>()
        .min_dimensions(200, 200)
        .quiet_zone(true)
        .build())
}

/// A new recovery code, `xxxxx-xxxxx` (50 bits).
pub fn new_recovery_code() -> String {
    let s = base32(&random_bytes::<7>()).to_lowercase();
    format!("{}-{}", &s[..5], &s[5..10])
}

/// Recovery code as compared: lowercase, without separators.
pub fn normalize_recovery_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_matches_rfc_6238_test_vectors() {
        // RFC 6238 appendix B, SHA1 secret "12345678901234567890"; the RFC
        // lists 8 digits, we use the last 6.
        let secret = b"12345678901234567890";
        for (time, code) in [
            (59u64, 94_287_082u32),
            (1_111_111_109, 7_081_804),
            (1_234_567_890, 89_005_924),
            (2_000_000_000, 69_279_037),
        ] {
            assert_eq!(totp_code(secret, time / 30), code % 1_000_000, "t={time}");
        }
    }

    #[test]
    fn totp_accepts_neighbour_steps_once() {
        let secret = b"12345678901234567890";
        let now = 1_000;
        let code = format!("{:06}", totp_code(secret, now - 1));
        assert_eq!(totp_verify(secret, &code, now, None), Some(now - 1));
        assert_eq!(
            totp_verify(secret, &code, now, Some(now - 1)),
            None,
            "replay"
        );
        let old = format!("{:06}", totp_code(secret, now - 2));
        assert_eq!(totp_verify(secret, &old, now, None), None, "too old");
        assert_eq!(totp_verify(secret, "12345", now, None), None);
        assert_eq!(totp_verify(secret, "abcdef", now, None), None);
    }

    #[test]
    fn base32_matches_rfc_4648() {
        assert_eq!(base32(b""), "");
        assert_eq!(base32(b"f"), "MY");
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
    }

    #[test]
    fn secret_box_roundtrip_is_bound_to_owner() {
        let b = SecretBox::new([1; 32]);
        let (alice, bob) = (Uuid::new_v4(), Uuid::new_v4());
        let sealed = b.seal(alice, b"secret").unwrap();
        assert_eq!(b.open(alice, &sealed).unwrap(), b"secret");
        assert!(b.open(bob, &sealed).is_err());
        assert!(SecretBox::new([2; 32]).open(alice, &sealed).is_err());
    }

    #[tokio::test]
    async fn passwords() {
        let hash = hash_password("correct horse battery".into()).await.unwrap();
        assert!(
            verify_password("correct horse battery".into(), Some(hash.clone()))
                .await
                .unwrap()
        );
        assert!(!verify_password("wrong".into(), Some(hash)).await.unwrap());
        assert!(!verify_password("x".into(), None).await.unwrap());
        assert!(check_password_policy("short").is_err());
        assert!(check_password_policy("long enough password").is_ok());
    }

    #[test]
    fn recovery_codes() {
        let c = new_recovery_code();
        assert_eq!(c.len(), 11);
        assert_eq!(normalize_recovery_code(&c.to_uppercase()).len(), 10);
        assert_eq!(normalize_recovery_code(" ab-CD 1 "), "abcd1");
    }
}
