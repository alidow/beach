use anyhow::Result;
use crossterm::{event::{self, Event, KeyCode, KeyEventKind}, terminal::{disable_raw_mode, enable_raw_mode}};
use ratatui::{backend::CrosstermBackend, Terminal, widgets::{List, ListItem, Block, Borders}, layout::{Layout, Constraint, Direction}, style::{Style, Modifier}};

#[derive(Clone, Debug)]
struct Item { id: String, title: String, app: String, is_display: bool }

pub fn run_picker() -> Result<Option<String>> {
    let mut items = load_items()?;
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(&mut stdout, crossterm::terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut index: usize = 0;
    let res = loop {
        terminal.draw(|f| {
            let size = f.size();
            let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(1)].as_ref()).split(size);
            let items_view: Vec<ListItem> = items.iter().enumerate().map(|(i, it)| {
                let prefix = if it.is_display { "[D]" } else { "[W]" };
                let line = format!("{} {} — {}", prefix, it.title, it.app);
                let mut li = ListItem::new(line);
                if i == index { li = li.style(Style::default().add_modifier(Modifier::REVERSED)); }
                li
            }).collect();
            let list = List::new(items_view).block(Block::default().title("Pick a window/display (Up/Down, Enter, r=refresh, q=quit)").borders(Borders::ALL));
            f.render_widget(list, chunks[0]);
        })?;
        if event::poll(std::time::Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => { break Ok(None); }
                        KeyCode::Up => { if index > 0 { index -= 1; } }
                        KeyCode::Down => { if !items.is_empty() { index = (index + 1).min(items.len().saturating_sub(1)); } }
                        KeyCode::Enter => {
                            let id = items.get(index).map(|it| it.id.clone());
                            break Ok(id);
                        }
                        KeyCode::Char('r') => { items = load_items()?; index = index.min(items.len().saturating_sub(1)); }
                        _ => {}
                    }
                }
            }
        }
    };
    disable_raw_mode()?;
    drop(terminal);
    let mut out = std::io::stdout();
    crossterm::execute!(&mut out, crossterm::terminal::LeaveAlternateScreen)?;
    res
}

fn load_items() -> Result<Vec<Item>> {
    let wins = beach_cabana_host::platform::enumerate_windows().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let mut items: Vec<Item> = wins.into_iter().map(|w| Item { id: w.identifier, title: w.title, app: w.application, is_display: matches!(w.kind, beach_cabana_host::platform::WindowKind::Display) }).collect();
    items.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    Ok(items)
}
