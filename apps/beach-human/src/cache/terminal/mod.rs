pub mod cache;
pub mod packed;

pub use cache::{TerminalCellSnapshot, TerminalGrid};
pub use packed::{
    PackedCell, Style, StyleId, StyleTable, attrs_from_byte, attrs_to_byte, pack_cell,
    pack_color_from_heavy, pack_from_heavy, unpack_cell, unpack_to_heavy,
};
