//! Where randomness for secrets comes from, so that a test can say what it is.

pub trait Entropy: Send + Sync {
    /// Fills `bytes` with random bytes fit for a secret.
    fn fill(&self, bytes: &mut [u8]);
}
