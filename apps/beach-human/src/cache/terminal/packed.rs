//! Helpers for packing terminal cells and deduplicating styles before storing
//! them in the cache.
//!
//! ```rust
//! # use beach_human::cache::terminal::packed::{StyleTable, Style, pack_cell, unpack_cell};
//! let table = StyleTable::new();
//! let id = table.ensure_id(Style::default());
//! let packed = pack_cell('x', id);
//! let (ch, resolved_id) = unpack_cell(packed);
//! assert_eq!(ch, 'x');
//! assert_eq!(resolved_id, id);
//! ```

use std::collections::HashMap;
use std::sync::RwLock;

use crate::model::terminal::cell::{Cell as HeavyCell, CellAttributes, Color as HeavyColor};

/// Packed cell layout: high 32 bits = char codepoint, low 32 bits = [`StyleId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackedCell(pub u64);

impl PackedCell {
    #[inline]
    pub fn from_raw(raw: u64) -> Self {
        PackedCell(raw)
    }

    #[inline]
    pub fn into_raw(self) -> u64 {
        self.0
    }
}

impl From<u64> for PackedCell {
    #[inline]
    fn from(value: u64) -> Self {
        PackedCell(value)
    }
}

impl From<PackedCell> for u64 {
    #[inline]
    fn from(value: PackedCell) -> Self {
        value.0
    }
}

/// Stable identifier for entries stored in a [`StyleTable`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StyleId(pub u32);

impl StyleId {
    pub const DEFAULT: StyleId = StyleId(0);

    #[inline]
    pub fn idx(self) -> usize {
        self.0 as usize
    }
}

#[inline]
pub fn pack_cell(ch: char, style_id: StyleId) -> PackedCell {
    let code = ch as u32 as u64;
    PackedCell::from_raw((code << 32) | (style_id.0 as u64))
}

#[inline]
pub fn unpack_cell(packed: PackedCell) -> (char, StyleId) {
    let code = (packed.0 >> 32) as u32;
    let style_id = (packed.0 & 0xFFFF_FFFF) as u32;
    (
        core::char::from_u32(code).unwrap_or('\u{FFFD}'),
        StyleId(style_id),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Style {
    /// PackedColor for foreground
    pub fg: u32,
    /// PackedColor for background
    pub bg: u32,
    /// Bitflags for CellAttributes (same layout as heavy attrs)
    pub attrs: u8,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            fg: pack_color_default(),
            bg: pack_color_default(),
            attrs: 0,
        }
    }
}

struct StyleTableInner {
    vec: Vec<Style>,
    map: HashMap<Style, StyleId>,
}

impl StyleTableInner {
    #[inline]
    fn get_id(&self, style: &Style) -> Option<StyleId> {
        self.map.get(style).copied()
    }
}

/// A simple style table that deduplicates styles and provides stable IDs.
/// All locking is handled internally via a single `RwLock`, so callers can
/// freely clone `Arc<StyleTable>` and share it between threads.
pub struct StyleTable {
    inner: RwLock<StyleTableInner>,
}

impl StyleTable {
    pub fn new() -> Self {
        let default_style = Style::default();
        let mut vec = Vec::with_capacity(16);
        vec.push(default_style); // id 0 is the default
        let mut map = HashMap::with_capacity(16);
        map.insert(default_style, StyleId::DEFAULT);
        StyleTable {
            inner: RwLock::new(StyleTableInner { vec, map }),
        }
    }

    /// Returns an existing ID for `style` or inserts it.
    pub fn ensure_id(&self, style: Style) -> StyleId {
        self.ensure_id_with_new(style).0
    }

    /// Returns the ID for `style` and whether the table inserted a new entry.
    pub fn ensure_id_with_new(&self, style: Style) -> (StyleId, bool) {
        if let Some(id) = self.inner.read().unwrap().get_id(&style) {
            return (id, false);
        }
        let mut inner = self.inner.write().unwrap();
        if let Some(id) = inner.get_id(&style) {
            return (id, false);
        }
        let id = StyleId(inner.vec.len() as u32);
        inner.vec.push(style);
        inner.map.insert(style, id);
        (id, true)
    }

    pub fn get(&self, id: StyleId) -> Option<Style> {
        self.inner.read().unwrap().vec.get(id.idx()).copied()
    }

    /// Replace the style stored at `id` and update the reverse lookup map.
    pub fn set(&self, id: StyleId, style: Style) -> bool {
        let mut inner = self.inner.write().unwrap();
        if let Some(slot) = inner.vec.get_mut(id.idx()) {
            let old_style = *slot;
            *slot = style;
            inner.map.remove(&old_style);
            inner.map.insert(style, id);
            true
        } else {
            false
        }
    }

    pub fn len(&self) -> usize {
        self.inner.read().unwrap().vec.len()
    }
}

// ---- Heavy <-> Packed conversions ----

#[inline]
pub fn pack_color_default() -> u32 {
    0 << 24
}

#[inline]
pub fn pack_color_indexed(idx: u8) -> u32 {
    (1u32 << 24) | (idx as u32)
}

#[inline]
pub fn pack_color_rgb(r: u8, g: u8, b: u8) -> u32 {
    (2u32 << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

#[inline]
pub fn pack_color_from_heavy(color: &HeavyColor) -> u32 {
    match color {
        HeavyColor::Default => pack_color_default(),
        HeavyColor::Indexed(i) => pack_color_indexed(*i),
        HeavyColor::Rgb(r, g, b) => pack_color_rgb(*r, *g, *b),
    }
}

#[inline]
pub fn unpack_color_to_heavy(packed: u32) -> HeavyColor {
    let kind = (packed >> 24) as u8;
    match kind {
        0 => HeavyColor::Default,
        1 => HeavyColor::Indexed((packed & 0xFF) as u8),
        2 => HeavyColor::Rgb(
            ((packed >> 16) & 0xFF) as u8,
            ((packed >> 8) & 0xFF) as u8,
            (packed & 0xFF) as u8,
        ),
        _ => HeavyColor::Default,
    }
}

#[inline]
pub fn attrs_to_byte(attrs: &CellAttributes) -> u8 {
    let mut b = 0u8;
    if attrs.bold {
        b |= 1 << 0;
    }
    if attrs.italic {
        b |= 1 << 1;
    }
    if attrs.underline {
        b |= 1 << 2;
    }
    if attrs.strikethrough {
        b |= 1 << 3;
    }
    if attrs.reverse {
        b |= 1 << 4;
    }
    if attrs.blink {
        b |= 1 << 5;
    }
    if attrs.dim {
        b |= 1 << 6;
    }
    if attrs.hidden {
        b |= 1 << 7;
    }
    b
}

#[inline]
pub fn attrs_from_byte(b: u8) -> CellAttributes {
    CellAttributes {
        bold: b & (1 << 0) != 0,
        italic: b & (1 << 1) != 0,
        underline: b & (1 << 2) != 0,
        strikethrough: b & (1 << 3) != 0,
        reverse: b & (1 << 4) != 0,
        blink: b & (1 << 5) != 0,
        dim: b & (1 << 6) != 0,
        hidden: b & (1 << 7) != 0,
    }
}

/// Convert a heavy Cell into a packed payload using `style_table`.
pub fn pack_from_heavy(cell: &HeavyCell, style_table: &StyleTable) -> PackedCell {
    let style = Style {
        fg: pack_color_from_heavy(&cell.fg_color),
        bg: pack_color_from_heavy(&cell.bg_color),
        attrs: attrs_to_byte(&cell.attributes),
    };
    let style_id = style_table.ensure_id(style);
    pack_cell(cell.char, style_id)
}

/// Convert a packed payload back into a heavy Cell via `style_table`.
pub fn unpack_to_heavy(packed: PackedCell, style_table: &StyleTable) -> HeavyCell {
    let (ch, style_id) = unpack_cell(packed);
    let s = style_table.get(style_id).unwrap_or_default();
    HeavyCell {
        char: ch,
        fg_color: unpack_color_to_heavy(s.fg),
        bg_color: unpack_color_to_heavy(s.bg),
        attributes: attrs_from_byte(s.attrs),
    }
}
