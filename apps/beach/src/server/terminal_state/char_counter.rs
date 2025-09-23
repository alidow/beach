use num_bigint::BigUint;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct CharCounter {
    /// Total character count including control chars
    pub value: BigUint,
}

impl CharCounter {
    pub fn new() -> Self {
        CharCounter {
            value: BigUint::from(0u32),
        }
    }

    pub fn increment(&mut self, count: usize) {
        self.value += count;
    }

    pub fn get(&self) -> &BigUint {
        &self.value
    }

    pub fn to_u64(&self) -> Option<u64> {
        // Try to convert to u64, returns None if value is too large
        self.value.to_u64_digits().first().copied()
    }
}

impl Default for CharCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_timeout::timeout]
    fn test_char_counter() {
        let mut counter = CharCounter::new();
        assert_eq!(counter.value, BigUint::from(0u32));

        counter.increment(5);
        assert_eq!(counter.value, BigUint::from(5u32));

        counter.increment(95);
        assert_eq!(counter.value, BigUint::from(100u32));
    }
}
