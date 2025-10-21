use anyhow::Result;
use beach_cabana_host::{self as cabana, desktop::{publish_selection, SelectionEvent}};
use crossbeam_channel::{self, Receiver, Sender};
use eframe::{
    egui::{self, Align, Color32, Layout, RichText, ScrollArea, TextEdit, Vec2, ViewportBuilder},
    NativeOptions,
};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};
use tokio::runtime::Runtime;

#[cfg(any(feature = "picker-mock", feature = "picker-native"))]
mod native_picker;

fn main() -> Result<()> {
    let options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size(Vec2::new(1100.0, 720.0))
            .with_min_inner_size(Vec2::new(820.0, 560.0)),
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

#[derive(Copy, Clone, PartialEq, Eq)]
enum CodecChoice {
    Gif,
    H264,
}

impl CodecChoice {
    fn label(self) -> &'static str {
        match self {
            CodecChoice::Gif => "PNG transport (debug)",
            CodecChoice::H264 => "H.264 (fragmented MP4)",
        }
    }

    fn to_host(self) -> cabana::webrtc::EncodeCodec {
        match self {
            CodecChoice::Gif => cabana::webrtc::EncodeCodec::Gif,
            CodecChoice::H264 => cabana::webrtc::EncodeCodec::H264,
        }
    }
}

struct ShareForm {
    session_id: String,
    passcode: String,
    road_url: String,
    fixture_url: String,
    fixture_dir: String,
    from_peer: String,
    to_peer: String,
    max_width: String,
    interval_ms: String,
    chunk_frames: String,
    codec: CodecChoice,
}

impl Default for ShareForm {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            passcode: String::new(),
            road_url: "http://127.0.0.1:8080".to_string(),
            fixture_url: String::new(),
            fixture_dir: String::new(),
            from_peer: "host".to_string(),
            to_peer: "viewer".to_string(),
            max_width: "1280".to_string(),
            interval_ms: "33".to_string(),
            chunk_frames: "120".to_string(),
            codec: CodecChoice::H264,
        }
    }
}

#[derive(Clone)]
struct ShareConfig {
    session_id: String,
    passcode: String,
    road_url: Option<String>,
    fixture_url: Option<String>,
    fixture_dir: Option<PathBuf>,
    from_peer: String,
    to_peer: String,
    target_id: String,
    codec: cabana::webrtc::EncodeCodec,
    interval_ms: u64,
    max_width: Option<u32>,
    chunk_frames: u32,
}

enum ShareEvent {
    Status(String),
    Verification(String),
    Started,
    Finished,
    Error(String),
}

struct ShareWorker {
    handle: Option<thread::JoinHandle<()>>,
    events: Receiver<ShareEvent>,
    stop: Arc<AtomicBool>,
}

impl ShareWorker {
    fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum SharingState {
    Idle,
    Starting,
    Streaming,
    Stopping,
}

#[cfg(any(feature = "picker-mock", feature = "picker-native"))]
use cabana_macos_picker::PickerResult;
#[cfg(any(feature = "picker-mock", feature = "picker-native"))]
use native_picker::{NativePickerClient, NativePickerMessage};

struct PickerApp {
    #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
    picker: Option<NativePickerClient>,
    #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
    picker_error: Option<String>,
    #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
    selection: Option<PickerResult>,
    status_message: Option<String>,
    share_form: ShareForm,
    share_state: SharingState,
    share_worker: Option<ShareWorker>,
    share_status_log: VecDeque<String>,
    share_error: Option<String>,
    share_verification: Option<String>,
}

impl PickerApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
        let (picker, picker_error) = match NativePickerClient::new() {
            Ok(client) => (Some(client), None),
            Err(err) => (None, Some(format!("Native picker unavailable: {}", err))),
        };

        #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
        let initial_status = picker_error.clone();

        #[cfg(not(any(feature = "picker-mock", feature = "picker-native")))]
        let initial_status: Option<String> = None;

        #[cfg(not(any(feature = "picker-mock", feature = "picker-native")))]
        let picker_error: Option<String> = None;

        Self {
            #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
            picker,
            #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
            picker_error,
            #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
            selection: None,
            status_message: initial_status,
            share_form: ShareForm::default(),
            share_state: SharingState::Idle,
            share_worker: None,
            share_status_log: VecDeque::new(),
            share_error: None,
            share_verification: None,
        }
    }

    fn append_share_status(&mut self, message: impl Into<String>) {
        const MAX_LOG: usize = 10;
        self.share_status_log.push_back(message.into());
        while self.share_status_log.len() > MAX_LOG {
            self.share_status_log.pop_front();
        }
    }

    fn poll_share_events(&mut self) {
        let mut should_cleanup = false;
        let mut encountered_error = false;
        let mut events = Vec::new();
        if let Some(worker) = self.share_worker.as_ref() {
            while let Ok(event) = worker.events.try_recv() {
                events.push(event);
            }
        }
        for event in events {
            match event {
                ShareEvent::Status(msg) => self.append_share_status(msg),
                ShareEvent::Verification(code) => {
                    self.share_verification = Some(code.clone());
                    self.append_share_status(format!("Verification code: {}", code));
                }
                ShareEvent::Started => {
                    self.share_state = SharingState::Streaming;
                    self.append_share_status("Streaming started.");
                }
                ShareEvent::Finished => {
                    should_cleanup = true;
                }
                ShareEvent::Error(err) => {
                    self.share_error = Some(err.clone());
                    self.append_share_status(format!("Error: {}", err));
                    encountered_error = true;
                    should_cleanup = true;
                }
            }
        }
        if should_cleanup {
            if let Some(mut worker) = self.share_worker.take() {
                worker.request_stop();
                worker.join();
            }
            if !encountered_error {
                self.append_share_status("Streaming stopped.");
            }
            self.share_state = SharingState::Idle;
        }
    }

    fn optional_trimmed(value: &str) -> Option<String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    fn build_share_config(&self) -> Result<ShareConfig, String> {
        let session_id = self.share_form.session_id.trim();
        if session_id.is_empty() {
            return Err("Enter a session ID.".to_string());
        }
        let passcode = self.share_form.passcode.trim();
        if passcode.is_empty() {
            return Err("Enter the session passcode.".to_string());
        }

        let interval_ms = self
            .share_form
            .interval_ms
            .trim()
            .parse::<u64>()
            .map_err(|_| "Interval must be a positive integer (ms).".to_string())?
            .max(1);
        let chunk_frames = self
            .share_form
            .chunk_frames
            .trim()
            .parse::<u32>()
            .map_err(|_| "Chunk size must be a positive integer (frames).".to_string())?
            .max(1);

        let max_width = {
            let trimmed = self.share_form.max_width.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(
                    trimmed
                        .parse::<u32>()
                        .map_err(|_| "Max width must be a positive integer.".to_string())?,
                )
            }
        };

        let fixture_dir =
            Self::optional_trimmed(&self.share_form.fixture_dir).map(PathBuf::from);

        let target_id = self
            .current_target_id()
            .ok_or_else(|| "Select a window or display using the native picker.".to_string())?;

        Ok(ShareConfig {
            session_id: session_id.to_string(),
            passcode: passcode.to_string(),
            road_url: Self::optional_trimmed(&self.share_form.road_url),
            fixture_url: Self::optional_trimmed(&self.share_form.fixture_url),
            fixture_dir,
            from_peer: self.share_form.from_peer.trim().to_string(),
            to_peer: self.share_form.to_peer.trim().to_string(),
            target_id,
            codec: self.share_form.codec.to_host(),
            interval_ms,
            max_width,
            chunk_frames,
        })
    }

    fn can_start_sharing(&self) -> bool {
        matches!(self.share_state, SharingState::Idle)
            && self.current_target_id().is_some()
            && !self.share_form.session_id.trim().is_empty()
            && !self.share_form.passcode.trim().is_empty()
    }

    fn start_sharing(&mut self) {
        if !self.can_start_sharing() {
            return;
        }
        if let Some(worker) = self.share_worker.as_ref() {
            worker.request_stop();
        }
        if let Some(mut worker) = self.share_worker.take() {
            worker.join();
        }
        if self.current_target_id().is_none() {
            self.share_error = Some("Pick a window or display before starting.".to_string());
            return;
        }
        let config = match self.build_share_config() {
            Ok(cfg) => cfg,
            Err(err) => {
                self.share_error = Some(err.clone());
                self.append_share_status(format!("Cannot start: {}", err));
                return;
            }
        };
        #[cfg(target_os = "macos")]
        {
            Self::ensure_screen_recording_permission();
        }

        let (tx, rx) = crossbeam_channel::unbounded();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let handle = spawn_share_worker(config, tx, stop_flag.clone());
        self.share_worker = Some(ShareWorker {
            handle: Some(handle),
            events: rx,
            stop: stop_flag,
        });
        self.share_state = SharingState::Starting;
        self.share_status_log.clear();
        self.share_error = None;
        self.share_verification = None;
        self.append_share_status("Launching Cabana host…");
    }

    fn stop_sharing(&mut self) {
        if let Some(worker) = self.share_worker.as_ref() {
            worker.request_stop();
            if !matches!(self.share_state, SharingState::Stopping) {
                self.append_share_status("Stop requested…");
            }
            self.share_state = SharingState::Stopping;
        }
    }

    fn force_stop_sharing(&mut self) {
        if let Some(worker) = self.share_worker.as_ref() {
            worker.request_stop();
        }
        if let Some(mut worker) = self.share_worker.take() {
            worker.join();
        }
        self.share_state = SharingState::Idle;
    }

    #[cfg(target_os = "macos")]
    fn ensure_screen_recording_permission() {
        use beach_cabana_host::platform::macos::permissions::{
            request_access, status, ScreenRecordingStatus,
        };
        if status() != ScreenRecordingStatus::Granted {
            let _ = request_access();
        }
    }

    fn render_share_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("WebRTC session").strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Session ID");
            ui.add(
                TextEdit::singleline(&mut self.share_form.session_id)
                    .desired_width(160.0)
                    .hint_text("road session id"),
            );
            ui.label("Passcode");
            ui.add(
                TextEdit::singleline(&mut self.share_form.passcode)
                    .desired_width(140.0)
                    .password(true)
                    .hint_text("shared secret"),
            );
            ui.label("Road URL");
            ui.add(
                TextEdit::singleline(&mut self.share_form.road_url)
                    .desired_width(220.0)
                    .hint_text("http://127.0.0.1:8080"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Fixture URL");
            ui.add(
                TextEdit::singleline(&mut self.share_form.fixture_url)
                    .desired_width(220.0)
                    .hint_text("optional"),
            );
            ui.label("Fixture dir");
            ui.add(
                TextEdit::singleline(&mut self.share_form.fixture_dir)
                    .desired_width(200.0)
                    .hint_text("optional"),
            );
            ui.label("From/To");
            ui.add(
                TextEdit::singleline(&mut self.share_form.from_peer)
                    .desired_width(80.0)
                    .hint_text("host id"),
            );
            ui.add(
                TextEdit::singleline(&mut self.share_form.to_peer)
                    .desired_width(80.0)
                    .hint_text("viewer id"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Codec");
            ui.selectable_value(
                &mut self.share_form.codec,
                CodecChoice::H264,
                CodecChoice::H264.label(),
            );
            ui.selectable_value(
                &mut self.share_form.codec,
                CodecChoice::Gif,
                CodecChoice::Gif.label(),
            );
            ui.separator();
            ui.label("Max width");
            ui.add(
                TextEdit::singleline(&mut self.share_form.max_width)
                    .desired_width(70.0)
                    .hint_text("1280"),
            );
            ui.label("Interval ms");
            ui.add(
                TextEdit::singleline(&mut self.share_form.interval_ms)
                    .desired_width(60.0)
                    .hint_text("33"),
            );
            ui.label("Chunk frames");
            ui.add(
                TextEdit::singleline(&mut self.share_form.chunk_frames)
                    .desired_width(70.0)
                    .hint_text("120"),
            );
        });
        ui.horizontal(|ui| {
            let start_label = match self.share_state {
                SharingState::Idle => "Start sharing",
                SharingState::Starting => "Starting…",
                SharingState::Streaming => "Streaming…",
                SharingState::Stopping => "Stopping…",
            };
            if ui
                .add_enabled(self.can_start_sharing(), egui::Button::new(start_label))
                .clicked()
            {
                self.start_sharing();
            }
            if ui
                .add_enabled(
                    matches!(
                        self.share_state,
                        SharingState::Starting | SharingState::Streaming | SharingState::Stopping
                    ),
                    egui::Button::new("Stop sharing"),
                )
                .clicked()
            {
                self.stop_sharing();
            }
            if let Some(code) = &self.share_verification {
                ui.label(RichText::new(format!("Verification: {}", code)).strong());
            }
            if let Some(err) = &self.share_error {
                ui.colored_label(Color32::from_rgb(220, 95, 78), err);
            }
        });
        if !self.share_status_log.is_empty() {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);
                for entry in &self.share_status_log {
                    ui.label(RichText::new(entry).color(Color32::from_gray(170)));
                }
            });
        }
    }

}

impl eframe::App for PickerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_share_events();

        egui::TopBottomPanel::top("cabana-top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Beach Cabana — select a window or display to share");
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut self.tab,
                    PickerTab::Displays,
                    PickerTab::Displays.label(),
                );
                ui.selectable_value(
                    &mut self.tab,
                    PickerTab::Windows,
                    PickerTab::Windows.label(),
                );
                ui.separator();
                let filter_changed = ui
                    .add(
                        TextEdit::singleline(&mut self.filter)
                            .hint_text("Filter by name or application"),
                    )
                    .changed();
                if filter_changed {
                    if let Some(selected) = self.selected_item() {
                        if !self.tab.matches(selected)
                            || !selected.matches_filter(&self.filter.to_lowercase())
                        {
                            self.selected_id = None;
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
        });

        let filtered = self.filtered_items();
        egui::CentralPanel::default().show(ctx, |ui| {
            if filtered.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(48.0);
                    ui.label(
                        RichText::new("No windows or displays match your filter.")
                            .color(Color32::from_gray(160)),
                    );
                });
                return;
            }

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(18.0, 18.0);
                        for item in filtered {
                            let selected = self.selected_id.as_deref() == Some(&item.id);
                            let preview_entry = if selected {
                                Some(self.ensure_preview(ctx, &item.id).clone())
                            } else {
                                self.preview_cache.get(&item.id).cloned()
                            };
                            let texture = preview_entry
                                .as_ref()
                                .and_then(|entry| entry.texture.clone());
                            let error = preview_entry
                                .as_ref()
                                .and_then(|entry| entry.error.clone());
                            let selected = self.selected_id.as_deref() == Some(&item.id);

                            let frame = egui::Frame::group(ui.style())
                                .fill(if selected {
                                    Color32::from_rgb(34, 52, 80)
                                } else {
                                    Color32::from_rgb(26, 28, 33)
                                })
                                .stroke(egui::Stroke::new(
                                    if selected { 2.0 } else { 1.0 },
                                    if selected {
                                        Color32::from_rgb(68, 120, 196)
                                    } else {
                                        Color32::from_rgb(58, 62, 70)
                                    },
                                ))
                                .inner_margin(Margin {
                                    left: 12.0,
                                    right: 12.0,
                                    top: 12.0,
                                    bottom: 12.0,
                                })
                                .rounding(egui::Rounding::same(10.0));

                            let response = frame
                                .show(ui, |ui| {
                                    ui.set_min_width(260.0);
                                    ui.set_max_width(260.0);
                                    ui.set_min_height(220.0);
                                    ui.vertical(|ui| {
                                        let preview_size = Vec2::new(
                                            ui.available_width(),
                                            150.0,
                                        );
                                        if let Some(texture) = texture {
                                            let texture_size = texture.size_vec2();
                                            let scale = (preview_size.x / texture_size.x)
                                                .min(preview_size.y / texture_size.y)
                                                .clamp(0.0, 1.0);
                                            let render_size = if scale > 0.0 {
                                                texture_size * scale
                                            } else {
                                                texture_size
                                            };
                                            ui.add(
                                                Image::new(&texture)
                                                    .max_size(preview_size)
                                                    .fit_to_exact_size(render_size),
                                            );
                                        } else if let Some(err) = error.clone() {
                                            ui.vertical_centered(|ui| {
                                                ui.label(
                                                    RichText::new("Preview unavailable")
                                                        .color(Color32::from_rgb(220, 95, 78))
                                                        .strong(),
                                                );
                                                ui.label(
                                                    RichText::new(err)
                                                        .color(Color32::from_gray(160)),
                                                );
                                            });
                                        } else {
                                            ui.vertical_centered(|ui| {
                                                ui.add_space(32.0);
                                                ui.label(
                                                    RichText::new(if selected {
                                                        "Preparing preview…"
                                                    } else {
                                                        "Select to generate preview"
                                                    })
                                                    .color(Color32::from_gray(160)),
                                                );
                                            });
                                        }
                                        ui.add_space(12.0);
                                        ui.label(
                                            RichText::new(format!(
                                                "{}{}",
                                                item.prefix(),
                                                item.title
                                            ))
                                            .strong()
                                            .color(Color32::from_rgb(224, 234, 255)),
                                        );
                                        if !item.application.is_empty() {
                                            ui.vertical_centered(|ui| {
                                                ui.label(
                                                    RichText::new(&item.application)
                                                        .color(Color32::from_rgb(170, 184, 210)),
                                                );
                                            });
                                        }
                                        if item.is_display {
                                            ui.vertical_centered(|ui| {
                                                ui.label(
                                                    RichText::new("Display")
                                                        .color(Color32::from_gray(150)),
                                                );
                                            });
                                        }
                                    });
                                })
                                .response
                                .interact(egui::Sense::click());

                            if response.clicked() {
                                self.select_id(&item.id);
                            }
                        }
                    });
                });
        });

        egui::TopBottomPanel::bottom("cabana-actions").show(ctx, |ui| {
            ui.separator();
            ui.vertical(|ui| {
                self.render_share_controls(ui);
                ui.separator();
                self.render_selection_controls(ui, ctx);
            });
        });
    }
}

impl Drop for PickerApp {
    fn drop(&mut self) {
        self.force_stop_sharing();
        self.reset_preview_cache();
    }
}

fn spawn_share_worker(
    config: ShareConfig,
    sender: Sender<ShareEvent>,
    stop_flag: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        sender
            .send(ShareEvent::Status(
                "Bootstrapping Cabana host…".to_string(),
            ))
            .ok();
        let runtime = match Runtime::new() {
            Ok(rt) => rt,
            Err(err) => {
                sender
                    .send(ShareEvent::Error(format!(
                        "Runtime init failed: {}",
                        err
                    )))
                    .ok();
                sender.send(ShareEvent::Finished).ok();
                return;
            }
        };

        let bootstrap_result = runtime.block_on(cabana::webrtc::host_bootstrap(
            config.session_id.clone(),
            config.passcode.clone(),
            config.road_url.clone(),
            config.fixture_url.clone(),
            config.fixture_dir.clone(),
            Vec::new(),
            config.from_peer.clone(),
            config.to_peer.clone(),
        ));

        let (transport, verification) = match bootstrap_result {
            Ok(pair) => pair,
            Err(err) => {
                sender
                    .send(ShareEvent::Error(format!(
                        "Bootstrap failed: {}",
                        err
                    )))
                    .ok();
                sender.send(ShareEvent::Finished).ok();
                return;
            }
        };

        sender
            .send(ShareEvent::Verification(verification))
            .ok();
        sender
            .send(ShareEvent::Status(
                "Handshake complete. Confirm verification with the viewer."
                    .to_string(),
            ))
            .ok();
        sender.send(ShareEvent::Started).ok();

        while !stop_flag.load(Ordering::SeqCst) {
            let chunk_result = runtime.block_on(cabana::webrtc::host_stream(
                &transport,
                config.codec,
                &config.window_id,
                config.chunk_frames,
                config.interval_ms,
                config.max_width,
            ));
            match chunk_result {
                Ok(_) => {
                    sender
                        .send(ShareEvent::Status(format!(
                            "Sent {} frames.",
                            config.chunk_frames
                        )))
                        .ok();
                }
                Err(err) => {
                    sender
                        .send(ShareEvent::Error(format!(
                            "Streaming failed: {}",
                            err
                        )))
                        .ok();
                    break;
                }
            }
        }

        sender.send(ShareEvent::Finished).ok();
    })
}

fn load_texture_from_path(ctx: &egui::Context, path: &Path) -> Result<TextureHandle, String> {
    let bytes = fs::read(path).map_err(|err| err.to_string())?;
    let mut image = image::load_from_memory(&bytes)
        .map_err(|err| err.to_string())?
        .to_rgba8();
    let mut width = image.width();
    let mut height = image.height();
    if width > 1400 {
        let scale = 1400.0 / width as f32;
        let new_width = 1400;
        let new_height = (height as f32 * scale).round().max(1.0) as u32;
        let resized =
            image::imageops::resize(&image, new_width, new_height, FilterType::Triangle);
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
                    if ui.button("Request access").clicked() {
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
