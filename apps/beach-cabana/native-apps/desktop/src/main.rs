use anyhow::Result;
use beach_cabana_host::{self as cabana, desktop::{publish_selection, SelectionEvent}};
use eframe::{
    egui::{
        self, Align, Color32, Direction, Image, Layout, RichText, ScrollArea, TextEdit,
        TextureHandle, TextureOptions, Vec2, ViewportBuilder, ViewportCommand,
    },
    NativeOptions,
};
use image::imageops::FilterType;
use std::{fs, path::{Path, PathBuf}};

fn main() -> Result<()> {
    let options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size(Vec2::new(960.0, 600.0))
            .with_min_inner_size(Vec2::new(720.0, 480.0)),
        ..Default::default()
    };

    eframe::run_native(
        "Beach Cabana Picker",
        options,
        Box::new(|cc| Box::new(PickerApp::new(cc))),
    )
    .map_err(|err| anyhow::anyhow!(err))?;
    Ok(())
}

#[derive(Clone)]
struct Item {
    id: String,
    title: String,
    title_lower: String,
    application: String,
    application_lower: String,
    is_display: bool,
}

impl Item {
    fn from_window(window: cabana::platform::WindowInfo) -> Self {
        let title = if window.title.is_empty() {
            "(Untitled)".to_string()
        } else {
            window.title
        };
        let application = window.application;
        Self {
            id: window.identifier,
            title_lower: title.to_lowercase(),
            application_lower: application.to_lowercase(),
            title,
            application,
            is_display: matches!(window.kind, cabana::platform::WindowKind::Display),
        }
    }

    fn matches_filter(&self, filter: &str) -> bool {
        filter.is_empty()
            || self.title_lower.contains(filter)
            || self.application_lower.contains(filter)
    }

    fn prefix(&self) -> &'static str {
        if self.is_display { "🖥 " } else { "🪟 " }
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum PickerTab {
    Displays,
    Windows,
}

impl PickerTab {
    fn matches(self, item: &Item) -> bool {
        match self {
            PickerTab::Displays => item.is_display,
            PickerTab::Windows => !item.is_display,
        }
    }

    fn label(self) -> &'static str {
        match self {
            PickerTab::Displays => "Displays",
            PickerTab::Windows => "Windows",
        }
    }
}

struct PickerApp {
    items: Vec<Item>,
    filter: String,
    tab: PickerTab,
    selected_id: Option<String>,
    pending_preview: Option<String>,
    preview_texture: Option<TextureHandle>,
    preview_error: Option<String>,
    status_message: Option<String>,
    preview_path: Option<PathBuf>,
}

impl PickerApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut app = Self {
            items: Vec::new(),
            filter: String::new(),
            tab: PickerTab::Displays,
            selected_id: None,
            pending_preview: None,
            preview_texture: None,
            preview_error: None,
            status_message: None,
            preview_path: None,
        };
        app.refresh_items();
        app
    }

    fn refresh_items(&mut self) {
        let previous_selection = self.selected_id.clone();
        match cabana::platform::enumerate_windows() {
            Ok(windows) => {
                self.items = windows
                    .into_iter()
                    .map(Item::from_window)
                    .collect::<Vec<_>>();
                self.items.sort_by(|a, b| a.title_lower.cmp(&b.title_lower));
                self.status_message = None;
            }
            Err(err) => {
                self.items.clear();
                self.status_message =
                    Some(format!("Failed to enumerate windows: {}", err.to_string()));
            }
        }

        if let Some(sel) = previous_selection {
            if self.items.iter().any(|item| item.id == sel) {
                self.selected_id = Some(sel.clone());
                self.pending_preview = Some(sel);
            } else {
                self.selected_id = None;
                self.preview_texture = None;
                if let Some(path) = self.preview_path.take() {
                    let _ = fs::remove_file(path);
                }
            }
        }

        if self.selected_id.is_none() {
            if let Some(first) = self
                .filtered_items()
                .first()
                .map(|item| item.id.clone())
            {
                self.selected_id = Some(first.clone());
                self.pending_preview = Some(first);
            }
        }
    }

    fn filtered_items(&self) -> Vec<Item> {
        let filter = self.filter.trim().to_lowercase();
        self.items
            .iter()
            .filter(|item| self.tab.matches(item) && item.matches_filter(&filter))
            .cloned()
            .collect()
    }

    fn select_id(&mut self, id: &str) {
        if self.selected_id.as_deref() != Some(id) {
            self.selected_id = Some(id.to_string());
            self.pending_preview = Some(id.to_string());
            self.preview_error = None;
        }
    }

    fn load_preview(&mut self, ctx: &egui::Context, id: &str) {
        self.preview_error = None;
        if let Some(old) = self.preview_path.take() {
            let _ = fs::remove_file(old);
        }
        match cabana::platform::preview_window(id) {
            Ok(path) => {
                let result = load_texture_from_path(ctx, &path);
                match result {
                    Ok(texture) => {
                        self.preview_texture = Some(texture);
                        self.preview_path = Some(path);
                    }
                    Err(err) => {
                        self.preview_texture = None;
                        self.preview_error = Some(err);
                        let _ = fs::remove_file(path);
                    }
                }
            }
            Err(err) => {
                self.preview_texture = None;
                self.preview_error = Some(format!("Preview unavailable: {}", err));
            }
        }
    }

    fn selected_item(&self) -> Option<&Item> {
        let id = self.selected_id.as_ref()?;
        self.items.iter().find(|item| &item.id == id)
    }
}

impl eframe::App for PickerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(id) = self.pending_preview.take() {
            self.load_preview(ctx, &id);
        }
        let ctx_clone = ctx.clone();

        egui::TopBottomPanel::top("cabana-top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Beach Cabana – Select a window or display to share");
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, PickerTab::Displays, PickerTab::Displays.label());
                ui.selectable_value(&mut self.tab, PickerTab::Windows, PickerTab::Windows.label());
                ui.separator();
                let edit = TextEdit::singleline(&mut self.filter)
                    .hint_text("Filter by name or application");
                if ui.add(edit).changed() {
                    if let Some(selected) = self.selected_item() {
                        if !self.tab.matches(selected)
                            || !selected.matches_filter(&self.filter.to_lowercase())
                        {
                            self.selected_id = None;
                            self.preview_texture = None;
                        }
                    }
                }
                if ui.button("Refresh").clicked() {
                    self.refresh_items();
                }
            });
            show_permission_banner(ui);
            if let Some(msg) = &self.status_message {
                ui.colored_label(Color32::from_rgb(220, 95, 78), msg);
            }
            if let Some(preview_err) = &self.preview_error {
                ui.colored_label(Color32::from_rgb(220, 95, 78), preview_err);
            }
        });

        let filtered = self.filtered_items();
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |columns| {
                let list_ui = &mut columns[0];
                list_ui.heading(self.tab.label());
                list_ui.separator();
                if filtered.is_empty() {
                    list_ui.label("Nothing matches your filter.");
                } else {
                    ScrollArea::vertical().show(list_ui, |ui| {
                        for item in filtered {
                            let selected = self.selected_id.as_deref() == Some(&item.id);
                            let label = if item.application.is_empty() {
                                format!("{}{}", item.prefix(), item.title)
                            } else {
                                format!(
                                    "{}{}\n    {}",
                                    item.prefix(),
                                    item.title,
                                    item.application
                                )
                            };
                            if ui
                                .add(egui::SelectableLabel::new(selected, label))
                                .clicked()
                            {
                                self.select_id(&item.id);
                            }
                            ui.separator();
                        }
                    });
                }

                let preview_ui = &mut columns[1];
                preview_ui.heading("Preview");
                preview_ui.separator();
                preview_ui.with_layout(
                    Layout::centered_and_justified(Direction::TopDown),
                    |ui| {
                        if let Some(texture) = &self.preview_texture {
                            let size = texture.size_vec2();
                            let available = ui.available_size();
                            let scale = (available.x / size.x)
                                .min(available.y / size.y)
                                .clamp(0.0, 1.0);
                            let desired = if scale > 0.0 { size * scale } else { size };
                            ui.add(Image::new(texture).max_size(desired));
                        } else {
                            ui.label(
                                RichText::new("Select a target to see a live preview")
                                    .color(Color32::from_gray(150)),
                            );
                        }
                    },
                );
            });
        });

        egui::TopBottomPanel::bottom("cabana-actions").show(ctx, |ui| {
            ui.separator();
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let selected_id = self.selected_id.clone();
                let selected_info = selected_id
                    .as_ref()
                    .and_then(|id| self.items.iter().find(|item| &item.id == id))
                    .cloned();
                if ui.button("Confirm Selection").clicked() {
                    if let Some(item) = selected_info.clone() {
                        if self.preview_path.is_none() {
                            self.load_preview(&ctx_clone, &item.id);
                        }
                        println!("{}", item.id);
                        let delivered = publish_selection(SelectionEvent::new(
                            item.id.clone(),
                            self.preview_path.clone(),
                        ));
                        self.status_message = Some(if delivered == 0 {
                            "Selection ready (no active listeners detected yet).".to_string()
                        } else if delivered == 1 {
                            "Selection sent to 1 listener.".to_string()
                        } else {
                            format!("Selection sent to {} listeners.", delivered)
                        });
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    } else {
                        self.status_message =
                            Some("Pick a window or display before confirming.".to_string());
                    }
                }
                if ui.button("Cancel").clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                if let Some(item) = selected_info {
                    ui.separator();
                    ui.label(format!(
                        "Selected: {} — {}",
                        item.title,
                        if item.application.is_empty() {
                            "System".to_string()
                        } else {
                            item.application.clone()
                        }
                    ));
                    if ui.button("Copy identifier").clicked() {
                        ctx_clone.output_mut(|output| {
                            output.copied_text = item.id.clone();
                        });
                        self.status_message =
                            Some("Identifier copied to clipboard.".to_string());
                    }
                    let preview_enabled = self.preview_path.as_ref().map(|p| p.exists()).unwrap_or(false);
                    ui.add_enabled_ui(preview_enabled, |ui| {
                        if ui.button("Open preview file").clicked() {
                            match self
                                .preview_path
                                .as_ref()
                                .map(|path| open::that(path))
                                .transpose()
                            {
                                Ok(Some(_)) | Ok(None) => {
                                    self.status_message =
                                        Some("Opening preview with the system viewer…".to_string());
                                }
                                Err(err) => {
                                    self.status_message =
                                        Some(format!("Failed to open preview: {}", err));
                                }
                            }
                        }
                    });
                }
            });
        });
    }
}

fn load_texture_from_path(ctx: &egui::Context, path: &Path) -> Result<TextureHandle, String> {
    let bytes = fs::read(path).map_err(|err| err.to_string())?;
    let mut image = image::load_from_memory(&bytes)
        .map_err(|err| err.to_string())?
        .to_rgba8();
    let mut width = image.width();
    let mut height = image.height();
    if width > 1024 {
        let scale = 1024.0 / width as f32;
        let new_width = 1024;
        let new_height = (height as f32 * scale).round().max(1.0) as u32;
        let resized = image::imageops::resize(&image, new_width, new_height, FilterType::Triangle);
        image = resized;
        width = new_width;
        height = new_height;
    }
    let pixels = image.into_raw();
    let color_image =
        egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &pixels);
    Ok(ctx.load_texture(
        format!("preview-{}", path.display()),
        color_image,
        TextureOptions::LINEAR,
    ))
}

#[cfg(target_os = "macos")]
fn show_permission_banner(ui: &mut egui::Ui) {
    use beach_cabana_host::platform::macos::permissions::{
        request_access, status, ScreenRecordingStatus,
    };

    if status() != ScreenRecordingStatus::Granted {
        egui::Frame::group(ui.style())
            .fill(Color32::from_rgb(255, 248, 235))
            .stroke(egui::Stroke::new(1.0, Color32::from_rgb(247, 192, 120)))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("Screen Recording permission required")
                            .color(Color32::from_rgb(161, 92, 16))
                            .strong(),
                    );
                    ui.label(
                        "Grant access in System Settings → Privacy & Security → Screen Recording.",
                    );
                    if ui.button("Request Access").clicked() {
                        let _ = request_access();
                    }
                });
            });
    }
}

#[cfg(not(target_os = "macos"))]
fn show_permission_banner(ui: &mut egui::Ui) {
    #[cfg(target_os = "windows")]
    {
        egui::Frame::group(ui.style())
            .fill(Color32::from_rgb(240, 247, 255))
            .stroke(egui::Stroke::new(1.0, Color32::from_rgb(111, 176, 255)))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("Windows screen capture access")
                            .color(Color32::from_rgb(16, 87, 158))
                            .strong(),
                    );
                    ui.label(
                        "Windows shows a \"Screen capture\" toast the first time Cabana shares a window. Approve it, or enable access via Settings → Privacy & security → Screen capture.",
                    );
                });
            });
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        egui::Frame::group(ui.style())
            .fill(Color32::from_rgb(242, 255, 244))
            .stroke(egui::Stroke::new(1.0, Color32::from_rgb(120, 201, 138)))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("Portal/compositor permission expected")
                            .color(Color32::from_rgb(17, 99, 45))
                            .strong(),
                    );
                    ui.label(
                        "Your desktop environment will prompt for screen sharing via PipeWire or X11. Accept the prompt when it appears to continue.",
                    );
                });
            });
    }
}
