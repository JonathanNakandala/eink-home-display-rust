//! The operating system's randomness.

use crate::domain::services::entropy::Entropy;

pub struct OsEntropy;

impl Entropy for OsEntropy {
    /// A host with no randomness can't make a secret that can be trusted, so this stops the program rather than hand out
    /// one that can be guessed.
    fn fill(&self, bytes: &mut [u8]) {
        getrandom::fill(bytes).expect("the operating system has no randomness for a secret");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_it_gives_differs_from_one_time_to_the_next() {
        let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
        OsEntropy.fill(&mut a);
        OsEntropy.fill(&mut b);
        assert_ne!(a, b);
        assert_ne!(a, [0u8; 32]);
    }
}
