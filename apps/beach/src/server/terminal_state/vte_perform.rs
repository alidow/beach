use vte::{Params, Perform};
use unicode_width::UnicodeWidthChar;
use crate::server::terminal_state::{Cell, Color, CellAttributes, Grid};

pub struct GridUpdater<'a> {
    pub grid: &'a mut Grid,
    pub current_attrs: CellAttributes,
    pub current_fg: Color,
    pub current_bg: Color,
}

impl<'a> GridUpdater<'a> {
    pub fn new(grid: &'a mut Grid) -> Self {
        GridUpdater {
            grid,
            current_attrs: CellAttributes::default(),
            current_fg: Color::Default,
            current_bg: Color::Default,
        }
    }
    
    pub fn new_with_state(grid: &'a mut Grid, fg: Color, bg: Color, attrs: CellAttributes) -> Self {
        GridUpdater {
            grid,
            current_attrs: attrs,
            current_fg: fg,
            current_bg: bg,
        }
    }
}

impl<'a> Perform for GridUpdater<'a> {
    fn print(&mut self, c: char) {
        // Handle character width for proper cursor movement
        let width = c.width().unwrap_or(1);
        
        // Ensure we don't go out of bounds
        if self.grid.cursor.col + width as u16 > self.grid.width {
            // Wrap to next line if character doesn't fit
            self.grid.cursor.col = 0;
            self.grid.cursor.row += 1;
            if self.grid.cursor.row >= self.grid.height {
                // Scroll up (simplified - just stay at bottom)
                self.grid.cursor.row = if self.grid.height > 0 { self.grid.height - 1 } else { 0 };
            }
        }
        
        let cell = Cell {
            char: c,
            fg_color: self.current_fg.clone(),
            bg_color: self.current_bg.clone(),
            attributes: self.current_attrs.clone(),
        };
        
        self.grid.set_cell(self.grid.cursor.row, self.grid.cursor.col, cell.clone());
        
        // For wide characters, fill next cell(s) with placeholder
        if width > 1 {
            for i in 1..width {
                if self.grid.cursor.col + (i as u16) < self.grid.width {
                    // Use a special marker for continuation cells
                    let continuation_cell = Cell {
                        char: '\0', // Null char indicates continuation
                        fg_color: self.current_fg.clone(),
                        bg_color: self.current_bg.clone(),
                        attributes: self.current_attrs.clone(),
                    };
                    self.grid.set_cell(self.grid.cursor.row, self.grid.cursor.col + (i as u16), continuation_cell);
                }
            }
        }
        
        // Move cursor by character width
        self.grid.cursor.col += width as u16;
        if self.grid.cursor.col >= self.grid.width {
            self.grid.cursor.col = 0;
            self.grid.cursor.row += 1;
            if self.grid.cursor.row >= self.grid.height {
                self.grid.cursor.row = if self.grid.height > 0 { self.grid.height - 1 } else { 0 };
            }
        }
    }
    
    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                // Line feed - move to next line AND return to beginning of line
                // This is the Unix convention (LF does both)
                self.grid.cursor.row += 1;
                if self.grid.cursor.row >= self.grid.height {
                    self.grid.cursor.row = if self.grid.height > 0 { self.grid.height - 1 } else { 0 };
                }
                self.grid.cursor.col = 0; // Also move to start of line
            }
            b'\r' => {
                // Carriage return - just move to beginning of line
                self.grid.cursor.col = 0;
            }
            b'\t' => {
                // Move to next tab stop (every 8 columns)
                let next_tab = ((self.grid.cursor.col / 8) + 1) * 8;
                self.grid.cursor.col = next_tab.min(self.grid.width - 1);
            }
            b'\x08' => {
                // Backspace
                if self.grid.cursor.col > 0 {
                    self.grid.cursor.col -= 1;
                }
            }
            _ => {}
        }
    }
    
    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _c: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}
    fn csi_dispatch(&mut self, params: &Params, _intermediates: &[u8], _ignore: bool, action: char) {
        match action {
            'H' | 'f' => {
                // Cursor position
                let row = params.iter().next().map(|p| p[0] as u16).unwrap_or(1).saturating_sub(1);
                let col = params.iter().nth(1).map(|p| p[0] as u16).unwrap_or(1).saturating_sub(1);
                self.grid.cursor.row = if self.grid.height > 0 { row.min(self.grid.height - 1) } else { 0 };
                self.grid.cursor.col = col.min(self.grid.width - 1);
            }
            'J' => {
                // Clear screen
                let mode = params.iter().next().map(|p| p[0]).unwrap_or(0);
                match mode {
                    2 => {
                        // Clear entire screen
                        // Create a blank cell with current background color
                        let blank_cell = Cell {
                            char: ' ',
                            fg_color: self.current_fg.clone(),
                            bg_color: self.current_bg.clone(),
                            attributes: self.current_attrs.clone(),
                        };
                        
                        for row in 0..self.grid.height {
                            for col in 0..self.grid.width {
                                self.grid.set_cell(row, col, blank_cell.clone());
                            }
                        }
                    }
                    1 => {
                        // Clear from cursor to beginning of screen
                        let blank_cell = Cell {
                            char: ' ',
                            fg_color: self.current_fg.clone(),
                            bg_color: self.current_bg.clone(),
                            attributes: self.current_attrs.clone(),
                        };
                        
                        // Clear from start to cursor position
                        for row in 0..=self.grid.cursor.row {
                            let end_col = if row == self.grid.cursor.row {
                                self.grid.cursor.col
                            } else {
                                self.grid.width - 1
                            };
                            for col in 0..=end_col {
                                self.grid.set_cell(row, col, blank_cell.clone());
                            }
                        }
                    }
                    0 | _ => {
                        // Clear from cursor to end of screen (default)
                        let blank_cell = Cell {
                            char: ' ',
                            fg_color: self.current_fg.clone(),
                            bg_color: self.current_bg.clone(),
                            attributes: self.current_attrs.clone(),
                        };
                        
                        // Clear from cursor position to end
                        for row in self.grid.cursor.row..self.grid.height {
                            let start_col = if row == self.grid.cursor.row {
                                self.grid.cursor.col
                            } else {
                                0
                            };
                            for col in start_col..self.grid.width {
                                self.grid.set_cell(row, col, blank_cell.clone());
                            }
                        }
                    }
                }
            }
            'K' => {
                // Erase line
                let mode = params.iter().next().map(|p| p[0]).unwrap_or(0);
                let blank_cell = Cell {
                    char: ' ',
                    fg_color: self.current_fg.clone(),
                    bg_color: self.current_bg.clone(),
                    attributes: self.current_attrs.clone(),
                };
                
                match mode {
                    2 => {
                        // Clear entire line
                        for col in 0..self.grid.width {
                            self.grid.set_cell(self.grid.cursor.row, col, blank_cell.clone());
                        }
                    }
                    1 => {
                        // Clear from cursor to beginning of line
                        for col in 0..=self.grid.cursor.col {
                            self.grid.set_cell(self.grid.cursor.row, col, blank_cell.clone());
                        }
                    }
                    0 | _ => {
                        // Clear from cursor to end of line (default)
                        for col in self.grid.cursor.col..self.grid.width {
                            self.grid.set_cell(self.grid.cursor.row, col, blank_cell.clone());
                        }
                    }
                }
            }
            'm' => {
                // SGR - Select Graphic Rendition
                for param in params.iter() {
                    match param[0] {
                        0 => {
                            // Reset
                            self.current_attrs = CellAttributes::default();
                            self.current_fg = Color::Default;
                            self.current_bg = Color::Default;
                        }
                        1 => self.current_attrs.bold = true,
                        3 => self.current_attrs.italic = true,
                        4 => self.current_attrs.underline = true,
                        7 => self.current_attrs.reverse = true,
                        30..=37 => self.current_fg = Color::Indexed(param[0] as u8 - 30),
                        40..=47 => self.current_bg = Color::Indexed(param[0] as u8 - 40),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    
    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {}
}
