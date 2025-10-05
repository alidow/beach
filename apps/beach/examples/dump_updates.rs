use beach_human::cache::terminal::TerminalGrid;
use beach_human::server::terminal::{AlacrittyEmulator, TerminalEmulator};

fn main() {
    let grid = TerminalGrid::new(24, 80);
    let mut emulator = AlacrittyEmulator::new(&grid, true);
    for i in 1..=40 {
        let line = format!("Line {i}: Test\n");
        let updates = emulator.handle_output(line.as_bytes(), &grid);
        for update in updates {
            println!("{update:?}");
        }
    }
}
