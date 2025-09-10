/// Predictive echo module for instant local feedback
/// Implements Mosh-inspired predictive echo with underline rendering
use crate::protocol::control_messages::ControlMessage;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Tracks predicted input and acknowledgments
pub struct PredictiveEcho {
    /// Client ID for this session
    client_id: String,
    
    /// Next sequence number to use
    next_seq: u64,
    
    /// Map of sequence number to predicted character
    predictions: HashMap<u64, PredictedChar>,
    
    /// Timeout for predictions (fallback if no ack)
    prediction_timeout: Duration,
}

/// A predicted character with metadata
#[derive(Debug, Clone)]
pub struct PredictedChar {
    /// The character(s) predicted
    pub chars: Vec<u8>,
    
    /// Position where it was inserted (cursor position)
    pub position: (u16, u16),
    
    /// When the prediction was made
    pub timestamp: Instant,
    
    /// Whether this has been acknowledged
    pub acknowledged: bool,
}

impl PredictiveEcho {
    /// Create a new predictive echo tracker
    pub fn new(client_id: String) -> Self {
        Self {
            client_id,
            next_seq: 1,
            predictions: HashMap::new(),
            prediction_timeout: Duration::from_millis(500), // 500ms fallback
        }
    }
    
    /// Record a prediction for typed input
    pub fn predict_input(&mut self, bytes: Vec<u8>, cursor_pos: (u16, u16)) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        
        let prediction = PredictedChar {
            chars: bytes.clone(),
            position: cursor_pos,
            timestamp: Instant::now(),
            acknowledged: false,
        };
        
        self.predictions.insert(seq, prediction);
        seq
    }
    
    /// Create a control message for the input
    pub fn create_input_message(&self, seq: u64, bytes: Vec<u8>) -> ControlMessage {
        ControlMessage::Input {
            client_id: self.client_id.clone(),
            client_seq: seq,
            bytes,
        }
    }
    
    /// Process an acknowledgment from the server
    pub fn acknowledge(&mut self, client_seq: u64, _apply_seq: u64, _version: u64) {
        if let Some(prediction) = self.predictions.get_mut(&client_seq) {
            prediction.acknowledged = true;
        }
    }
    
    /// Remove acknowledged predictions
    pub fn remove_acknowledged(&mut self) -> Vec<u64> {
        let mut removed = Vec::new();
        self.predictions.retain(|seq, pred| {
            if pred.acknowledged {
                removed.push(*seq);
                false
            } else {
                true
            }
        });
        removed
    }
    
    /// Remove timed-out predictions
    pub fn remove_timed_out(&mut self) -> Vec<u64> {
        let now = Instant::now();
        let mut removed = Vec::new();
        
        self.predictions.retain(|seq, pred| {
            if now.duration_since(pred.timestamp) > self.prediction_timeout {
                removed.push(*seq);
                false
            } else {
                true
            }
        });
        removed
    }
    
    /// Get all active predictions (not yet acknowledged)
    pub fn active_predictions(&self) -> Vec<(u64, &PredictedChar)> {
        self.predictions
            .iter()
            .filter(|(_, pred)| !pred.acknowledged)
            .map(|(seq, pred)| (*seq, pred))
            .collect()
    }
    
    /// Check if a character at position should be underlined
    pub fn should_underline(&self, pos: (u16, u16)) -> bool {
        self.predictions
            .values()
            .any(|pred| !pred.acknowledged && pred.position == pos)
    }
    
    /// Clear all predictions (e.g., on disconnect)
    pub fn clear(&mut self) {
        self.predictions.clear();
    }
    
    /// Queue predictions during disconnect
    pub fn queue_for_replay(&mut self) -> Vec<(u64, Vec<u8>)> {
        self.predictions
            .iter()
            .filter(|(_, pred)| !pred.acknowledged)
            .map(|(seq, pred)| (*seq, pred.chars.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_prediction_tracking() {
        let mut echo = PredictiveEcho::new("client-1".to_string());
        
        // Predict some input
        let seq1 = echo.predict_input(vec![b'h'], (0, 0));
        let seq2 = echo.predict_input(vec![b'i'], (1, 0));
        
        assert_eq!(seq1, 1);
        assert_eq!(seq2, 2);
        assert_eq!(echo.active_predictions().len(), 2);
        
        // Acknowledge first prediction
        echo.acknowledge(seq1, 1, 1);
        let removed = echo.remove_acknowledged();
        
        assert_eq!(removed, vec![seq1]);
        assert_eq!(echo.active_predictions().len(), 1);
    }
    
    #[test]
    fn test_underline_check() {
        let mut echo = PredictiveEcho::new("client-1".to_string());
        
        // Predict at position (5, 0)
        echo.predict_input(vec![b'x'], (5, 0));
        
        assert!(echo.should_underline((5, 0)));
        assert!(!echo.should_underline((6, 0)));
    }
    
    #[test]
    fn test_timeout_removal() {
        let mut echo = PredictiveEcho::new("client-1".to_string());
        echo.prediction_timeout = Duration::from_millis(0); // Immediate timeout
        
        let seq = echo.predict_input(vec![b'a'], (0, 0));
        std::thread::sleep(Duration::from_millis(1));
        
        let removed = echo.remove_timed_out();
        assert_eq!(removed, vec![seq]);
        assert_eq!(echo.active_predictions().len(), 0);
    }
}