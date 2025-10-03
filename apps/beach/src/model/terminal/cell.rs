use super::error::TerminalStateError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cell {
    /// Unicode character (can be multi-byte)
    pub char: char,

    /// Foreground color (24-bit RGB or indexed)
    pub fg_color: Color,

    /// Background color (24-bit RGB or indexed)
    pub bg_color: Color,

    /// Text attributes
    pub attributes: CellAttributes,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Color {
    Default,
    Indexed(u8),     // 256-color palette
    Rgb(u8, u8, u8), // 24-bit true color
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CellAttributes {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub reverse: bool,
    pub blink: bool,
    pub dim: bool,
    pub hidden: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            char: ' ',
            fg_color: Color::Default,
            bg_color: Color::Default,
            attributes: CellAttributes::default(),
        }
    }
}

impl Cell {
    /// Check if cell is blank (space or null character)
    pub fn is_blank(&self) -> bool {
        self.char == ' ' || self.char == '\0'
    }
}

impl Default for CellAttributes {
    fn default() -> Self {
        CellAttributes {
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            reverse: false,
            blink: false,
            dim: false,
            hidden: false,
        }
    }
}

impl Cell {
    /// Memory-efficient serialization for storage
    pub fn to_bytes(&self) -> Vec<u8> {
        // Compact binary representation
        let mut bytes = Vec::with_capacity(13);

        // Encode char as UTF-8 (1-4 bytes)
        let char_bytes = self.char.to_string().into_bytes();
        bytes.push(char_bytes.len() as u8);
        bytes.extend_from_slice(&char_bytes);

        // Encode colors (3-4 bytes each)
        bytes.extend(self.fg_color.to_bytes());
        bytes.extend(self.bg_color.to_bytes());

        // Pack attributes into single byte
        bytes.push(self.attributes.to_byte());

        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TerminalStateError> {
        if bytes.is_empty() {
            return Err(TerminalStateError::SerializationError(
                "Empty byte array".to_string(),
            ));
        }

        let mut cursor = 0;

        // Read char length
        let char_len = bytes[cursor] as usize;
        cursor += 1;

        if cursor + char_len > bytes.len() {
            return Err(TerminalStateError::SerializationError(
                "Invalid char length".to_string(),
            ));
        }

        // Read char
        let char_str = String::from_utf8(bytes[cursor..cursor + char_len].to_vec())
            .map_err(|e| TerminalStateError::SerializationError(e.to_string()))?;
        let char = char_str.chars().next().ok_or_else(|| {
            TerminalStateError::SerializationError("Empty char string".to_string())
        })?;
        cursor += char_len;

        // Read colors
        let (fg_color, fg_size) = Color::from_bytes(&bytes[cursor..])
            .map_err(|e| TerminalStateError::SerializationError(e))?;
        cursor += fg_size;

        let (bg_color, bg_size) = Color::from_bytes(&bytes[cursor..])
            .map_err(|e| TerminalStateError::SerializationError(e))?;
        cursor += bg_size;

        // Read attributes
        if cursor >= bytes.len() {
            return Err(TerminalStateError::SerializationError(
                "Missing attributes byte".to_string(),
            ));
        }
        let attributes = CellAttributes::from_byte(bytes[cursor]);

        Ok(Cell {
            char,
            fg_color,
            bg_color,
            attributes,
        })
    }
}

impl Color {
    fn to_bytes(&self) -> Vec<u8> {
        match self {
            Color::Default => vec![0],
            Color::Indexed(idx) => vec![1, *idx],
            Color::Rgb(r, g, b) => vec![2, *r, *g, *b],
        }
    }

    fn from_bytes(bytes: &[u8]) -> Result<(Self, usize), String> {
        if bytes.is_empty() {
            return Err("Empty color bytes".to_string());
        }

        match bytes[0] {
            0 => Ok((Color::Default, 1)),
            1 => {
                if bytes.len() < 2 {
                    return Err("Invalid indexed color".to_string());
                }
                Ok((Color::Indexed(bytes[1]), 2))
            }
            2 => {
                if bytes.len() < 4 {
                    return Err("Invalid RGB color".to_string());
                }
                Ok((Color::Rgb(bytes[1], bytes[2], bytes[3]), 4))
            }
            _ => Err("Unknown color type".to_string()),
        }
    }
}

impl CellAttributes {
    fn to_byte(&self) -> u8 {
        let mut byte = 0u8;
        if self.bold {
            byte |= 1 << 0;
        }
        if self.italic {
            byte |= 1 << 1;
        }
        if self.underline {
            byte |= 1 << 2;
        }
        if self.strikethrough {
            byte |= 1 << 3;
        }
        if self.reverse {
            byte |= 1 << 4;
        }
        if self.blink {
            byte |= 1 << 5;
        }
        if self.dim {
            byte |= 1 << 6;
        }
        if self.hidden {
            byte |= 1 << 7;
        }
        byte
    }

    fn from_byte(byte: u8) -> Self {
        CellAttributes {
            bold: byte & (1 << 0) != 0,
            italic: byte & (1 << 1) != 0,
            underline: byte & (1 << 2) != 0,
            strikethrough: byte & (1 << 3) != 0,
            reverse: byte & (1 << 4) != 0,
            blink: byte & (1 << 5) != 0,
            dim: byte & (1 << 6) != 0,
            hidden: byte & (1 << 7) != 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_timeout::timeout]
    fn test_cell_serialization() {
        let cell = Cell {
            char: 'A',
            fg_color: Color::Rgb(255, 128, 64),
            bg_color: Color::Indexed(42),
            attributes: CellAttributes {
                bold: true,
                italic: false,
                underline: true,
                ..Default::default()
            },
        };

        let bytes = cell.to_bytes();
        let decoded = Cell::from_bytes(&bytes).unwrap();

        assert_eq!(cell, decoded);
    }

    #[test_timeout::timeout]
    fn test_emoji_serialization() {
        // Test with emoji character
        let cell = Cell {
            char: '🏖',
            fg_color: Color::Default,
            bg_color: Color::Default,
            attributes: CellAttributes::default(),
        };

        let bytes = cell.to_bytes();
        let decoded = Cell::from_bytes(&bytes).unwrap();

        assert_eq!(cell.char, decoded.char);
    }

    #[test_timeout::timeout]
    fn test_default_cell() {
        let cell = Cell::default();
        assert_eq!(cell.char, ' ');
        assert_eq!(cell.fg_color, Color::Default);
        assert_eq!(cell.bg_color, Color::Default);
        assert!(!cell.attributes.bold);
    }
}
