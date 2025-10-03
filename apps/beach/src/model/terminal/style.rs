use super::cell::{Cell, CellAttributes, Color};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedStyle {
    pub fg: Color,
    pub bg: Color,
    pub attributes: CellAttributes,
}

impl ResolvedStyle {
    pub fn new(fg: Color, bg: Color, attributes: CellAttributes) -> Self {
        Self { fg, bg, attributes }
    }
}

impl From<&Cell> for ResolvedStyle {
    fn from(cell: &Cell) -> Self {
        ResolvedStyle {
            fg: cell.fg_color.clone(),
            bg: cell.bg_color.clone(),
            attributes: cell.attributes.clone(),
        }
    }
}
