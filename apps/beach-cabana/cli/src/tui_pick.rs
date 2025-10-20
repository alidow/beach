use anyhow::Result;
use crossterm::{event::{self, Event, KeyCode, KeyEventKind}, terminal::{disable_raw_mode, enable_raw_mode}};
use ratatui::{backend::CrosstermBackend, Terminal, widgets::{List, ListItem, Block, Borders, Paragraph}, layout::{Layout, Constraint, Direction}, style::{Style, Modifier}, text::Span};
use image::GenericImageView;

#[derive(Clone, Debug)]
struct Item { id: String, title: String, app: String, is_display: bool }

pub fn run_picker() -> Result<Option<String>> {
    let mut items = load_items()?;
    let mut filter = String::new();
    let mut show_displays = true; // Tab between Displays and Windows
    let mut last_preview_id: Option<String> = None;
    let mut preview_lines: Vec<String> = Vec::new();
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(&mut stdout, crossterm::terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut index: usize = 0;
    let res = loop {
        terminal.draw(|f| {
            let size = f.size();
            let vchunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(3), Constraint::Min(1)].as_ref()).split(size);
            let header = Paragraph::new(Span::raw(format!("Picker — Tab: {} | Filter: {}", if show_displays {"Displays"} else {"Windows"}, filter))).block(Block::default().title("beach-cabana").borders(Borders::ALL));
            f.render_widget(header, vchunks[0]);
            let filtered: Vec<&Item> = items.iter().filter(|it| it.is_display == show_displays && (filter.is_empty() || it.title.to_lowercase().contains(&filter.to_lowercase()) || it.app.to_lowercase().contains(&filter.to_lowercase()))).collect();
            let mut idx = index;
            if filtered.is_empty() { idx = 0; } else { idx = idx.min(filtered.len().saturating_sub(1)); }
            // Recompute preview if selection changed
            if let Some(sel) = filtered.get(idx) {
                let need = match &last_preview_id { Some(prev) => prev != &sel.id, None => true };
                if need {
                    preview_lines = build_preview_ascii(&sel.id).unwrap_or_else(|_| vec!["(preview unavailable)".to_string()]);
                    last_preview_id = Some(sel.id.clone());
                }
            } else {
                preview_lines.clear();
                last_preview_id = None;
            }
            let hchunks = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(60), Constraint::Percentage(40)].as_ref()).split(vchunks[1]);
            let items_view: Vec<ListItem> = filtered.iter().enumerate().map(|(i, it)| {
                let prefix = if it.is_display { "[D]" } else { "[W]" };
                let line = format!("{} {} — {}", prefix, it.title, it.app);
                let mut li = ListItem::new(line);
                if i == idx { li = li.style(Style::default().add_modifier(Modifier::REVERSED)); }
                li
            }).collect();
            let list = List::new(items_view).block(Block::default().title("Select target (Tab: switch, type: filter, Enter: confirm, r: refresh, q: quit)").borders(Borders::ALL));
            f.render_widget(list, hchunks[0]);
            let preview_text = preview_lines.join("\n");
            let preview = Paragraph::new(preview_text).block(Block::default().title("Preview").borders(Borders::ALL));
            f.render_widget(preview, hchunks[1]);
        })?;
        if event::poll(std::time::Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => { break Ok(None); }
                        KeyCode::Tab => { show_displays = !show_displays; }
                        KeyCode::Up => { if index > 0 { index -= 1; } }
                        KeyCode::Down => { index = index.saturating_add(1); }
                        KeyCode::Enter => {
                            let id = items.iter().filter(|it| it.is_display == show_displays && (filter.is_empty() || it.title.to_lowercase().contains(&filter.to_lowercase()) || it.app.to_lowercase().contains(&filter.to_lowercase()))).nth(index).map(|it| it.id.clone());
                            break Ok(id);
                        }
                        KeyCode::Char('r') => { items = load_items()?; index = 0; }
                        KeyCode::Backspace => { filter.pop(); }
                        KeyCode::Char(c) => { if !c.is_control() { filter.push(c); } }
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

fn build_preview_ascii(id: &str) -> Result<Vec<String>> {
    // Save a preview PNG via host and convert to small ASCII block
    let path = beach_cabana_host::platform::preview_window(id).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let img = image::open(&path)?;
    let (w, h) = img.dimensions();
    let target_w = 40u32;
    let scale = if w > 0 { (target_w as f32 / w as f32).min(1.0) } else { 1.0 };
    let out_w = (w as f32 * scale).max(1.0) as u32;
    let out_h = ((h as f32 * scale * 0.5).max(1.0)) as u32; // vertical scale correction
    let small = img.resize_exact(out_w.max(1), out_h.max(1), image::imageops::FilterType::Triangle).to_luma8();
    let ramp = [ ' ', '.', ':', '-', '=', '+', '*', '#', '%', '@' ];
    let mut lines = Vec::with_capacity(small.height() as usize);
    for y in 0..small.height() {
        let mut line = String::with_capacity(small.width() as usize);
        for x in 0..small.width() {
            let px = small.get_pixel(x, y)[0];
            let idx = ((px as f32 / 255.0) * ((ramp.len() - 1) as f32)).round() as usize;
            line.push(ramp[idx]);
        }
        lines.push(line);
    }
    Ok(lines)
}

fn load_items() -> Result<Vec<Item>> {
    let wins = beach_cabana_host::platform::enumerate_windows().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let mut items: Vec<Item> = wins.into_iter().map(|w| Item { id: w.identifier, title: w.title, app: w.application, is_display: matches!(w.kind, beach_cabana_host::platform::WindowKind::Display) }).collect();
    items.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    Ok(items)
}
