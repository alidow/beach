use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    Realtime,
    Historical,
    Anchored,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ViewPosition {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompressionType {
    None,
    Gzip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorCode(pub u16);

impl ErrorCode {
    pub const INVALID_MESSAGE_FORMAT: Self = Self(1001);
    pub const UNKNOWN_SUBSCRIPTION: Self = Self(1002);
    pub const SEQUENCE_MISMATCH: Self = Self(1003);
    
    pub const CHECKSUM_MISMATCH: Self = Self(2001);
    pub const INVALID_DIMENSIONS: Self = Self(2002);
    pub const HISTORY_NOT_AVAILABLE: Self = Self(2003);
    
    pub const INPUT_NOT_ALLOWED: Self = Self(3001);
    pub const VIEW_NOT_PERMITTED: Self = Self(3002);
    
    pub const TOO_MANY_SUBSCRIPTIONS: Self = Self(4001);
    pub const RATE_LIMIT_EXCEEDED: Self = Self(4002);
    
    pub const INTERNAL_ERROR: Self = Self(5001);
    pub const SESSION_ENDING: Self = Self(5002);
}

impl Serialize for ErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let code = u16::deserialize(deserializer)?;
        Ok(ErrorCode(code))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Dimensions {
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")] 
pub enum SubscriptionStatus {
    Active,
    Pending,
    Shared,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationType {
    DimensionChange,
    ModeChange,
    SessionEnding,
}