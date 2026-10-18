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
