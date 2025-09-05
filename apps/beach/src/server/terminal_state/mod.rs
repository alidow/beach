pub mod cell;
pub mod char_counter;
pub mod error;
pub mod grid;
pub mod grid_delta;
pub mod grid_history;
pub mod grid_view;
pub mod init;
pub mod line_counter;
pub mod tracker;
#[cfg(feature = "alacritty-backend")]
pub mod alacritty_backend;
pub mod backend;

pub use cell::{Cell, CellAttributes, Color};
pub use char_counter::CharCounter;
pub use error::TerminalStateError;
pub use grid::{CursorPosition, CursorShape, Grid};
pub use grid_delta::{CellChange, CursorChange, DimensionChange, GridDelta};
pub use grid_history::{GridHistory, HistoryConfig, HistoryStats};
pub use grid_view::GridView;
pub use init::TerminalInitializer;
pub use line_counter::LineCounter;
pub use tracker::TerminalStateTracker;
#[cfg(feature = "alacritty-backend")]
pub use alacritty_backend::AlacrittyTerminal;
pub use backend::{TerminalBackend, create_terminal_backend};
