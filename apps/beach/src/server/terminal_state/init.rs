use std::env;
use crate::server::terminal_state::{Grid, Color, Cell};

/// Initialize terminal grid with appropriate defaults based on environment
pub struct TerminalInitializer;

impl TerminalInitializer {
    /// Create an initial grid that better matches the terminal environment
    /// 
    /// This function attempts to set reasonable defaults based on:
    /// - Terminal type (from TERM env var)
    /// - Color scheme hints (from COLORFGBG env var)
    /// - Terminal emulator detection
    /// 
    /// This ensures beach captures a more accurate initial state rather than
    /// assuming all terminals start with default colors.
    pub fn create_initial_grid(width: u16, height: u16) -> Grid {
        let mut grid = Grid::new(width, height);
        
        // Detect terminal background color from environment hints
        let bg_color = Self::detect_background_color();
        let fg_color = Self::detect_foreground_color();
        
        // If we detected non-default colors, apply them to all initial cells
        // This ensures that when beach starts in a terminal with a custom theme,
        // the initial state reflects that theme rather than assuming defaults
        if bg_color != Color::Default || fg_color != Color::Default {
            for row in 0..height {
                for col in 0..width {
                    if let Some(cell) = grid.get_cell_mut(row, col) {
                        cell.bg_color = bg_color.clone();
                        cell.fg_color = fg_color.clone();
                    }
                }
            }
        }
        
        grid
    }
    
    /// Detect background color from environment hints
    fn detect_background_color() -> Color {
        // Check COLORFGBG environment variable (used by some terminals)
        // Format is typically "foreground;background" where colors are palette indices
        if let Ok(colorfgbg) = env::var("COLORFGBG") {
            if let Some(bg_part) = colorfgbg.split(';').nth(1) {
                if let Ok(bg_idx) = bg_part.parse::<u8>() {
                    // Common mappings:
                    // 0 = black, 7 = white (light terminals)
                    // 15 = bright white (some light themes)
                    // Return as indexed color if valid
                    if bg_idx <= 15 {
                        return Color::Indexed(bg_idx);
                    }
                }
            }
        }
        
        // Check for dark mode hints from terminal emulator
        if let Ok(term_program) = env::var("TERM_PROGRAM") {
            // Terminal.app on macOS
            if term_program == "Apple_Terminal" {
                if let Ok(profile) = env::var("TERM_PROGRAM_VERSION") {
                    // Could check profile for theme hints
                }
            }
            
            // iTerm2
            if term_program == "iTerm.app" {
                // iTerm2 specific detection could go here
            }
        }
        
        // Check for theme hints in other env vars
        if let Ok(colorterm) = env::var("COLORTERM") {
            if colorterm == "truecolor" || colorterm == "24bit" {
                // Terminal supports true color, but we still don't know the theme
            }
        }
        
        // Default to terminal's default color
        Color::Default
    }
    
    /// Detect foreground color from environment hints
    fn detect_foreground_color() -> Color {
        // Check COLORFGBG environment variable
        if let Ok(colorfgbg) = env::var("COLORFGBG") {
            if let Some(fg_part) = colorfgbg.split(';').nth(0) {
                if let Ok(fg_idx) = fg_part.parse::<u8>() {
                    // Common mappings:
                    // 0 = black (light terminals)
                    // 7 = white (dark terminals)
                    // 15 = bright white
                    if fg_idx <= 15 {
                        return Color::Indexed(fg_idx);
                    }
                }
            }
        }
        
        // Default to terminal's default color
        Color::Default
    }
    
    /// Check if terminal appears to be using a dark theme
    /// This is a heuristic based on common patterns
    pub fn is_dark_theme() -> bool {
        // Check COLORFGBG - if background is black/dark (0-6), it's likely dark theme
        if let Ok(colorfgbg) = env::var("COLORFGBG") {
            if let Some(bg_part) = colorfgbg.split(';').nth(1) {
                if let Ok(bg_idx) = bg_part.parse::<u8>() {
                    // 0-6 are typically dark colors, 7-15 are light
                    return bg_idx <= 6;
                }
            }
        }
        
        // Check terminal emulator specific hints
        if let Ok(iterm_profile) = env::var("ITERM_PROFILE") {
            // Common dark theme names
            let dark_keywords = ["dark", "night", "black", "dracula", "monokai", "solarized dark"];
            let profile_lower = iterm_profile.to_lowercase();
            for keyword in &dark_keywords {
                if profile_lower.contains(keyword) {
                    return true;
                }
            }
        }
        
        // Default assumption is light theme (safer for visibility)
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_initial_grid_creation() {
        let grid = TerminalInitializer::create_initial_grid(80, 24);
        assert_eq!(grid.width, 80);
        assert_eq!(grid.height, 24);
        
        // Check that all cells are initialized
        for row in 0..24 {
            for col in 0..80 {
                assert!(grid.get_cell(row, col).is_some());
            }
        }
    }
    
    #[test]
    fn test_colorfgbg_parsing() {
        // This would need to mock environment variables in a real test
        // For now, just verify the grid creation doesn't panic
        let grid = TerminalInitializer::create_initial_grid(10, 10);
        assert_eq!(grid.width, 10);
    }
}