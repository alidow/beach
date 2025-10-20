use anyhow::Result;
use beach_cabana_host as cabana;
use crossterm::{terminal::{enable_raw_mode, disable_raw_mode}, event::{self, Event, KeyCode, KeyEventKind}};
use ratatui::{backend::CrosstermBackend, Terminal, layout::{Layout, Direction, Constraint}, widgets::{Block, Borders, List, ListItem, Paragraph}, text::Span, style::{Style, Modifier}};

#[derive(Clone, Debug)]
struct Item { id: String, title: String, app: String, is_display: bool }

fn load_items() -> Result<Vec<Item>> {
    let wins = cabana::platform::enumerate_windows().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let mut items: Vec<Item> = wins.into_iter().map(|w| Item { id: w.identifier, title: w.title, app: w.application, is_display: matches!(w.kind, cabana::platform::WindowKind::Display) }).collect();
    items.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    Ok(items)
}

fn main() -> Result<()> {
    // Minimal desktop picker scaffold (terminal UI placeholder). Future: replace with Tauri UI.
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(&mut stdout, crossterm::terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut items = load_items()?;
    let mut filter = String::new();
    let mut index: usize = 0;
    let mut show_displays = true;

    loop {
        let filtered: Vec<&Item> = items.iter().filter(|it| it.is_display == show_displays && (filter.is_empty() || it.title.to_lowercase().contains(&filter.to_lowercase()) || it.app.to_lowercase().contains(&filter.to_lowercase()))).collect();
        if filtered.is_empty() { index = 0; } else { index = index.min(filtered.len().saturating_sub(1)); }
        terminal.draw(|f| {
            let size = f.size();
            let layout = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(3), Constraint::Min(1)].as_ref()).split(size);
            let header = Paragraph::new(Span::raw(format!("Desktop Picker — Tab: {} | Filter: {}", if show_displays {"Displays"} else {"Windows"}, filter))).block(Block::default().title("beach-cabana-desktop").borders(Borders::ALL));
            f.render_widget(header, layout[0]);
            let items_view: Vec<ListItem> = filtered.iter().enumerate().map(|(i, it)| {
                let prefix = if it.is_display { "[D]" } else { "[W]" };
                let line = format!("{} {} — {}", prefix, it.title, it.app);
                let mut li = ListItem::new(line);
                if i == index { li = li.style(Style::default().add_modifier(Modifier::REVERSED)); }
                li
            }).collect();
            let list = List::new(items_view).block(Block::default().title("Select target (Tab: switch, type: filter, Enter: confirm, r: refresh, q: quit)").borders(Borders::ALL));
            f.render_widget(list, layout[1]);
        })?;

        if event::poll(std::time::Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Tab => { show_displays = !show_displays; }
                        KeyCode::Up => { if index > 0 { index -= 1; } }
                        KeyCode::Down => { index = index.saturating_add(1); }
                        KeyCode::Enter => {
                            if let Some(sel) = items.iter().filter(|it| it.is_display == show_displays && (filter.is_empty() || it.title.to_lowercase().contains(&filter.to_lowercase()) || it.app.to_lowercase().contains(&filter.to_lowercase()))).nth(index) {
                                println!("Selected: {}", sel.id);
                            }
                            break;
                        }
                        KeyCode::Char('r') => { items = load_items()?; }
                        KeyCode::Backspace => { filter.pop(); }
                        KeyCode::Char(c) => { if !c.is_control() { filter.push(c); } }
                        _ => {}
                    }
                }
            }
        }
    }

    disable_raw_mode()?;
    drop(terminal);
    let mut out = std::io::stdout();
    crossterm::execute!(&mut out, crossterm::terminal::LeaveAlternateScreen)?;
    Ok(())
}

