//! Static bearer tokens, known only by their SHA-256 digests.

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::store::UserId;

/// SHA-256 digest of a bearer token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenDigest([u8; 32]);

impl TokenDigest {
    pub fn of(token: &str) -> Self {
        Self(Sha256::digest(token.as_bytes()).into())
    }

    /// Parses 64 hex digits, as printed by `shasum -a 256`.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.as_bytes();
        // Checked up front: `from_str_radix` would also accept a leading `+`.
        if hex.len() != 64 || !hex.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let mut digest = [0; 32];
        for (byte, pair) in digest.iter_mut().zip(hex.chunks_exact(2)) {
            *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        }
        Some(Self(digest))
    }
}

/// The accepted tokens. Every one currently maps to [`UserId::DEFAULT`].
#[derive(Debug, Clone)]
pub struct Tokens {
    digests: Vec<TokenDigest>,
}

impl Tokens {
    pub fn new(digests: Vec<TokenDigest>) -> Self {
        Self { digests }
    }

    /// The user `token` belongs to, if it is accepted.
    ///
    /// Compares against every digest in constant time, so timing reveals
    /// neither which digest matched nor how much of one did.
    pub fn verify(&self, token: &str) -> Option<UserId> {
        let TokenDigest(presented) = TokenDigest::of(token);
        let accepted = self
            .digests
            .iter()
            .fold(subtle::Choice::from(0), |found, TokenDigest(digest)| {
                found | digest.ct_eq(&presented)
            });
        bool::from(accepted).then_some(UserId::DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(accepted: &[&str]) -> Tokens {
        Tokens::new(
            accepted
                .iter()
                .map(|token| TokenDigest::of(token))
                .collect(),
        )
    }

    #[test]
    fn digest_is_sha256() {
        let expected = "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b";

        let TokenDigest(bytes) = TokenDigest::of("secret");

        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(hex, expected);
    }

    #[test]
    fn parses_a_hex_digest_in_either_case() {
        let lower = "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b";

        assert_eq!(
            TokenDigest::from_hex(lower),
            Some(TokenDigest::of("secret"))
        );
        assert_eq!(
            TokenDigest::from_hex(&lower.to_uppercase()),
            Some(TokenDigest::of("secret"))
        );
    }

    #[test]
    fn rejects_malformed_hex_digests() {
        let valid = "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b";

        assert_eq!(TokenDigest::from_hex(&valid[1..]), None);
        assert_eq!(TokenDigest::from_hex(&format!("{valid}0")), None);
        assert_eq!(TokenDigest::from_hex(&valid.replace('b', "g")), None);
        assert_eq!(TokenDigest::from_hex(&valid.replace("2b", "+b")), None);
        assert_eq!(
            TokenDigest::from_hex(&valid.replacen("2b", "\u{e9}", 1)),
            None
        );
    }

    #[test]
    fn accepts_a_configured_token() {
        assert_eq!(
            tokens(&["phone", "laptop"]).verify("laptop"),
            Some(UserId::DEFAULT)
        );
    }

    #[test]
    fn rejects_an_unknown_token() {
        assert_eq!(tokens(&["phone"]).verify("laptop"), None);
    }

    #[test]
    fn rejects_everything_without_tokens() {
        assert_eq!(tokens(&[]).verify(""), None);
    }
}
