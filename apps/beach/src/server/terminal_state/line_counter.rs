use num_bigint::BigUint;
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct LineCounter {
    /// Unwrapped line number (dimension-independent)
    pub value: BigUint,
}

impl LineCounter {
    pub fn new() -> Self {
        LineCounter { 
            value: BigUint::from(0u32) 
        }
    }
    
    pub fn from_u64(val: u64) -> Self {
        LineCounter { 
            value: BigUint::from(val) 
        }
    }
    
    pub fn increment(&mut self) {
        self.value += 1u32;
    }
    
    pub fn add(&mut self, lines: u64) {
        self.value += lines;
    }
    
    /// Calculate wrapped line for given width
    pub fn to_wrapped(&self, content: &str, width: u16) -> u64 {
        // Account for line wrapping at specific width
        let mut wrapped_lines = 0u64;
        for line in content.lines() {
            let line_width = UnicodeWidthStr::width(line);
            wrapped_lines += ((line_width as u64) + (width as u64) - 1) / (width as u64);
        }
        wrapped_lines
    }
    
    pub fn to_u64(&self) -> Option<u64> {
        // Try to convert to u64, returns None if value is too large
        self.value.to_u64_digits()
            .first()
            .copied()
    }
}

impl Default for LineCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_line_counter_basic() {
        let mut counter = LineCounter::new();
        assert_eq!(counter.value, BigUint::from(0u32));
        
        counter.increment();
        assert_eq!(counter.value, BigUint::from(1u32));
        
        counter.add(99);
        assert_eq!(counter.value, BigUint::from(100u32));
    }
    
    #[test]
    fn test_line_counter_wrapping() {
        let counter = LineCounter::from_u64(10);
        let content = "This is a very long line that will wrap when displayed in a narrow terminal";
        let wrapped = counter.to_wrapped(content, 20);
        assert_eq!(wrapped, 4); // Should wrap to 4 lines at width 20
    }
}