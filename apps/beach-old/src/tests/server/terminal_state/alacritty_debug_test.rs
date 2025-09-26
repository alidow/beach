#[cfg(feature = "alacritty-backend")]
use crate::server::terminal_state::AlacrittyTerminal;

#[cfg(feature = "alacritty-backend")]
#[test_timeout::timeout]
fn test_debug_alacritty_grid_direct() {
    use alacritty_terminal::event::{Event, EventListener};
    use alacritty_terminal::vte::ansi::{CharsetIndex, Handler, StandardCharset};
    use alacritty_terminal::{Term, grid::Dimensions, term::Config};

    struct TermDimensions {
        columns: usize,
        screen_lines: usize,
    }

    impl Dimensions for TermDimensions {
        fn total_lines(&self) -> usize {
            self.screen_lines
        }
        fn screen_lines(&self) -> usize {
            self.screen_lines
        }
        fn columns(&self) -> usize {
            self.columns
        }
    }

    #[derive(Clone)]
    struct EventProxy;
    impl EventListener for EventProxy {
        fn send_event(&self, _event: Event) {}
    }

    let dimensions = TermDimensions {
        columns: 80,
        screen_lines: 24,
    };

    let event_proxy = EventProxy;
    let config = Config::default();
    let mut term = Term::new(config, &dimensions, event_proxy);

    // Process "Line 1\nLine 2" directly using Handler trait
    term.input('L');
    term.input('i');
    term.input('n');
    term.input('e');
    term.input(' ');
    term.input('1');
    term.linefeed();
    term.input('L');
    term.input('i');
    term.input('n');
    term.input('e');
    term.input(' ');
    term.input('2');

    // Print grid state
    println!("Alacritty grid after 'Line 1\\nLine 2':");
    println!(
        "Grid dimensions: {} lines x {} columns",
        term.grid().screen_lines(),
        term.grid().columns()
    );
    println!(
        "Cursor: line={}, col={}",
        term.grid().cursor.point.line.0,
        term.grid().cursor.point.column.0
    );

    // Check what's in the grid
    for line_idx in 0..5 {
        print!("Line {}: ", line_idx);
        for col_idx in 0..15 {
            let point = alacritty_terminal::index::Point {
                line: alacritty_terminal::index::Line(line_idx),
                column: alacritty_terminal::index::Column(col_idx),
            };
            let cell = &term.grid()[point];
            if cell.c == '\0' || cell.c == ' ' {
                print!("_");
            } else {
                print!("{}", cell.c);
            }
        }
        println!();
    }

    // Also check with negative line indices (scrollback)
    println!("\nChecking with display_offset:");
    let display_offset = term.grid().display_offset();
    println!("Display offset: {}", display_offset);
}
