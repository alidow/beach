use anyhow::Result;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use beach_cabana_host::desktop::{ScreenCaptureDescriptor, SelectionEvent, publish_selection};
use beach_client_core::{
    auth::{self, AuthError, access_token_is_valid, credentials::StoredProfile},
    session::{SessionConfig, SessionError, SessionManager},
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::{
    NativeOptions,
    egui::{self, Align, Color32, ComboBox, Layout, RichText, ScrollArea, Vec2, ViewportBuilder},
};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::{
    collections::VecDeque,
    env,
    fmt::Write as FmtWrite,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::runtime::Runtime;

#[cfg(any(feature = "picker-mock", feature = "picker-native"))]
mod native_picker;

#[cfg(any(feature = "picker-mock", feature = "picker-native"))]
use cabana_macos_picker::{PickerItemKind, PickerResult};
#[cfg(any(feature = "picker-mock", feature = "picker-native"))]
use native_picker::{NativePickerClient, NativePickerMessage};

#[cfg(not(any(feature = "picker-mock", feature = "picker-native")))]
#[derive(Copy, Clone)]
enum PickerItemKind {
    Window,
    Display,
    Application,
    Unknown,
}

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

#[derive(Debug, Clone, Deserialize)]
struct PrivateBeachSummary {
    id: String,
    name: String,
    slug: String,
    #[serde(default)]
    created_at: i64,
}

#[derive(Debug, Clone)]
struct PublicSessionInfo {
    session_id: String,
    join_code: String,
    session_url: String,
}

#[derive(Debug, Clone)]
struct AuthPrompt {
    profile: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: String,
    expires_at: SystemTime,
}

#[derive(Debug, Clone)]
enum AuthStatus {
    LoggedOut,
    Starting,
    Pending(AuthPrompt),
    LoggedIn {
        profile: String,
        email: Option<String>,
        tier: Option<String>,
    },
    Error(String),
}

#[derive(Debug)]
enum AppMessage {
    AuthPrompt(AuthPrompt),
    AuthSuccess {
        profile: String,
        email: Option<String>,
        tier: Option<String>,
    },
    AuthError {
        message: String,
    },
    AccessToken {
        profile: String,
        token: String,
        expires_at: Option<SystemTime>,
    },
    BeachesLoaded(Vec<PrivateBeachSummary>),
    BeachesError(String),
    PublicSessionCreated(PublicSessionInfo),
    PublicSessionError(String),
    PrivateAttachOk {
        beach_id: String,
        beach_name: String,
        session_id: String,
    },
    PrivateAttachError(String),
}

struct PickerApp {
    #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
    picker: Option<NativePickerClient>,
    tiles: Vec<PickerTile>,
    selected_id: Option<String>,
    status_message: Option<String>,
    telemetry_log: VecDeque<String>,
    session_sheet: SessionSheetState,
    #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
    picker_bootstrapped: bool,
    event_tx: Sender<AppMessage>,
    event_rx: Receiver<AppMessage>,
    auth_status: AuthStatus,
    auth_profile: Option<String>,
    access_token: Option<String>,
    access_token_expiry: Option<SystemTime>,
    manager_base: String,
    road_base: String,
    beaches: Vec<PrivateBeachSummary>,
    selected_beach_id: Option<String>,
    session_updates: VecDeque<String>,
    public_session: Option<PublicSessionInfo>,
    auth_inflight: bool,
    fetching_beaches: bool,
    creating_public_session: bool,
    attaching_private_beach: bool,
}

impl PickerApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
        let picker = match NativePickerClient::new() {
            Ok(client) => Some(client),
            Err(err) => {
                eprintln!("Native picker unavailable: {err}");
                None
            }
        };

        #[cfg(not(any(feature = "picker-mock", feature = "picker-native")))]
        let picker: Option<NativePickerClient> = None;

        let (event_tx, event_rx) = unbounded();

        let mut app = Self {
            #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
            picker,
            tiles: Vec::new(),
            selected_id: None,
            status_message: None,
            telemetry_log: VecDeque::new(),
            session_sheet: SessionSheetState::new(),
            #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
            picker_bootstrapped: false,
            event_tx,
            event_rx,
            auth_status: AuthStatus::LoggedOut,
            auth_profile: None,
            access_token: None,
            access_token_expiry: None,
            manager_base: default_manager_base(),
            road_base: default_road_base(),
            beaches: Vec::new(),
            selected_beach_id: None,
            session_updates: VecDeque::new(),
            public_session: None,
            auth_inflight: false,
            fetching_beaches: false,
            creating_public_session: false,
            attaching_private_beach: false,
        };

        #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
        app.bootstrap_picker();
        app.refresh_auth_state();

        app
    }

    #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
    fn bootstrap_picker(&mut self) {
        if self.picker_bootstrapped {
            return;
        }
        if let Some(picker) = &self.picker {
            match picker.launch() {
                Ok(()) => {
                    self.status_message = Some("Native picker ready.".to_string());
                    self.record_telemetry("picker_open", "auto");
                    self.picker_bootstrapped = true;
                }
                Err(err) => {
                    self.status_message = Some(format!("Failed to launch native picker: {err}"));
                    self.picker_bootstrapped = true;
                }
            }
        } else {
            self.status_message =
                Some("Native picker is not available on this platform.".to_string());
            self.picker_bootstrapped = true;
        }
    }

    fn refresh_auth_state(&mut self) {
        match auth::load_store() {
            Ok(store) => {
                let profile_name = store
                    .current_profile
                    .clone()
                    .or_else(|| store.profile_names().into_iter().next());

                if let Some(name) = profile_name {
                    if let Some(profile) = store.profile(&name) {
                        self.auth_profile = Some(name.clone());
                        self.auth_status = AuthStatus::LoggedIn {
                            profile: name.clone(),
                            email: profile.email.clone(),
                            tier: profile.tier.clone(),
                        };
                        if let Some(cache) = profile
                            .access_token
                            .as_ref()
                            .filter(|entry| access_token_is_valid(entry))
                        {
                            self.access_token = Some(cache.token.clone());
                            self.access_token_expiry = Some(cache.expires_at.into());
                        }
                    } else {
                        self.auth_status = AuthStatus::LoggedOut;
                        self.auth_profile = None;
                        self.access_token = None;
                        self.access_token_expiry = None;
                    }
                } else {
                    self.auth_status = AuthStatus::LoggedOut;
                    self.auth_profile = None;
                    self.access_token = None;
                    self.access_token_expiry = None;
                }
            }
            Err(err) => {
                self.auth_status = AuthStatus::Error(err.to_string());
                self.auth_profile = None;
                self.access_token = None;
                self.access_token_expiry = None;
            }
        }
    }

    #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
    fn reopen_picker(&mut self) {
        if let Some(picker) = &self.picker {
            match picker.launch() {
                Ok(()) => {
                    self.status_message = Some("Picker relaunched.".to_string());
                    self.record_telemetry("picker_open", "manual");
                }
                Err(err) => {
                    self.status_message = Some(format!("Failed to relaunch picker: {err}"));
                }
            }
        } else {
            self.status_message =
                Some("Cannot relaunch picker without native support.".to_string());
        }
    }

    #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
    fn poll_picker_events(&mut self) {
        if let Some(picker) = &self.picker {
            for message in picker.poll() {
                match message {
                    NativePickerMessage::Selection(result) => {
                        self.apply_selection(result, SelectionSource::Stream);
                    }
                    NativePickerMessage::Cancelled => {
                        self.status_message = Some("Picker closed without selection.".to_string());
                    }
                    NativePickerMessage::Error(message) => {
                        self.status_message = Some(format!("Picker error: {message}"));
                    }
                }
            }
        }
    }

    #[cfg(not(any(feature = "picker-mock", feature = "picker-native")))]
    fn poll_picker_events(&mut self) {}

    fn poll_app_events(&mut self) {
        while let Ok(message) = self.event_rx.try_recv() {
            match message {
                AppMessage::AuthPrompt(prompt) => {
                    self.auth_status = AuthStatus::Pending(prompt.clone());
                    self.auth_inflight = true;
                    self.push_session_update(format!("Enter Beach Auth code {}", prompt.user_code));
                }
                AppMessage::AuthSuccess {
                    profile,
                    email,
                    tier,
                } => {
                    self.auth_status = AuthStatus::LoggedIn {
                        profile: profile.clone(),
                        email,
                        tier,
                    };
                    self.auth_profile = Some(profile.clone());
                    self.auth_inflight = false;
                    self.push_session_update(format!("Signed in as {profile}."));
                    self.request_access_token();
                }
                AppMessage::AuthError { message } => {
                    self.auth_status = AuthStatus::Error(message.clone());
                    self.auth_inflight = false;
                    self.push_session_update(format!("Auth error: {message}"));
                }
                AppMessage::AccessToken {
                    profile,
                    token,
                    expires_at,
                } => {
                    self.access_token = Some(token);
                    self.access_token_expiry = expires_at;
                    self.auth_profile = Some(profile);
                    self.auth_inflight = false;
                    self.push_session_update("Fetched Beach Auth token.");
                }
                AppMessage::BeachesLoaded(list) => {
                    self.fetching_beaches = false;
                    self.beaches = list;
                    if let Some(selected) = &self.selected_beach_id {
                        if !self.beaches.iter().any(|b| &b.id == selected) {
                            self.selected_beach_id = self.beaches.first().map(|b| b.id.clone());
                        }
                    } else {
                        self.selected_beach_id = self.beaches.first().map(|b| b.id.clone());
                    }
                    if self.beaches.is_empty() {
                        self.push_session_update("No private beaches registered.");
                    } else {
                        self.push_session_update(format!(
                            "Loaded {} private beach{}.",
                            self.beaches.len(),
                            if self.beaches.len() == 1 { "" } else { "es" }
                        ));
                    }
                }
                AppMessage::BeachesError(error) => {
                    self.fetching_beaches = false;
                    self.push_session_update(format!("Failed to load beaches: {error}"));
                }
                AppMessage::PublicSessionCreated(info) => {
                    self.creating_public_session = false;
                    self.public_session = Some(info.clone());
                    self.push_session_update(format!("Created session {}.", info.session_id));
                }
                AppMessage::PublicSessionError(error) => {
                    self.creating_public_session = false;
                    self.push_session_update(format!("Session creation failed: {error}"));
                }
                AppMessage::PrivateAttachOk {
                    beach_name,
                    session_id,
                    ..
                } => {
                    self.attaching_private_beach = false;
                    self.push_session_update(format!(
                        "Attached session {} to {}.",
                        session_id, beach_name
                    ));
                }
                AppMessage::PrivateAttachError(error) => {
                    self.attaching_private_beach = false;
                    self.push_session_update(format!("Attach failed: {error}"));
                }
            }
        }
    }

    #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
    fn apply_selection(&mut self, result: PickerResult, source: SelectionSource) {
        let is_new_tile = self.upsert_tile(result.clone());

        if is_new_tile {
            self.record_telemetry("picker_discovered", &result.id);
            if matches!(source, SelectionSource::Stream) && self.selected_id.is_some() {
                self.status_message = Some(format!("Discovered {}", result.label));
                return;
            }
        }

        if matches!(source, SelectionSource::Stream)
            && self.selected_id.is_some()
            && self.selected_id.as_deref() != Some(result.id.as_str())
        {
            return;
        }

        self.selected_id = Some(result.id.clone());

        let descriptor = ScreenCaptureDescriptor::new(
            result.id.clone(),
            result.filter_blob.clone(),
            result.stream_config_blob.clone(),
            result.metadata_json.clone(),
        );

        let metadata = decode_metadata(&result.metadata_json);
        self.session_sheet.update_selection(
            descriptor.clone(),
            result.label.clone(),
            result.application.clone(),
            result.kind,
            metadata,
        );

        publish_selection(SelectionEvent::new(
            descriptor,
            result.label.clone(),
            result.application.clone(),
            None,
        ));

        let detail = match source {
            SelectionSource::Stream => {
                if is_new_tile {
                    "stream-initial"
                } else {
                    "stream-refresh"
                }
            }
            SelectionSource::Tile => "tile",
        };
        self.record_telemetry("picker_selection", &format!("{} ({})", result.id, detail));
        self.status_message = Some(format!("Selected {}", result.label));
    }

    #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
    fn upsert_tile(&mut self, result: PickerResult) -> bool {
        let metadata = decode_metadata(&result.metadata_json);

        if let Some(tile) = self
            .tiles
            .iter_mut()
            .find(|tile| tile.result.id == result.id)
        {
            tile.result = result;
            tile.metadata = metadata;
            return false;
        }

        self.tiles.push(PickerTile { result, metadata });
        self.tiles.sort_by(|a, b| {
            a.result
                .label
                .to_lowercase()
                .cmp(&b.result.label.to_lowercase())
        });
        true
    }

    fn record_telemetry(&mut self, event: &str, detail: &str) {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|dur| dur.as_millis())
            .unwrap_or_default();
        let mut entry = String::new();
        let _ = write!(&mut entry, "[{}] {} {}", timestamp_ms, event, detail);
        self.telemetry_log.push_back(entry.clone());
        while self.telemetry_log.len() > 16 {
            self.telemetry_log.pop_front();
        }
        println!("[telemetry] {entry}");
    }

    fn begin_auth_login(&mut self) {
        if self.auth_inflight {
            return;
        }
        self.auth_inflight = true;
        self.auth_status = AuthStatus::Starting;
        let profile = self
            .auth_profile
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let sender = self.event_tx.clone();
        thread::spawn(move || {
            let runtime = Runtime::new().expect("failed to create tokio runtime for auth login");
            let result: Result<(), AuthError> = runtime.block_on(async {
                let config = auth::BeachAuthConfig::from_env()?;
                let (start, client) = auth::perform_device_login(&profile, config.clone()).await?;
                let prompt = AuthPrompt {
                    profile: profile.clone(),
                    user_code: start.user_code.clone(),
                    verification_uri: start.verification_uri.clone(),
                    verification_uri_complete: start.verification_uri_complete.clone(),
                    expires_at: SystemTime::now() + Duration::from_secs(start.expires_in.max(60)),
                };
                let _ = sender.send(AppMessage::AuthPrompt(prompt));

                loop {
                    tokio::time::sleep(Duration::from_secs(start.interval.max(3))).await;
                    match auth::complete_device_login(&profile, &client, &start.device_code).await {
                        Ok(tokens) => {
                            let _ = auth::set_current_profile(Some(profile.clone()));
                            let _ = sender.send(AppMessage::AuthSuccess {
                                profile: profile.clone(),
                                email: tokens.email.clone(),
                                tier: tokens.tier.clone(),
                            });

                            if let Ok(Some(token)) =
                                auth::maybe_access_token(Some(&profile), true).await
                            {
                                let expiry = auth::load_store()
                                    .ok()
                                    .and_then(|store| profile_expiry(&store, &profile));
                                let _ = sender.send(AppMessage::AccessToken {
                                    profile: profile.clone(),
                                    token,
                                    expires_at: expiry,
                                });
                            }
                            break;
                        }
                        Err(AuthError::AuthorizationPending) => continue,
                        Err(AuthError::AuthorizationDenied) => {
                            let _ = sender.send(AppMessage::AuthError {
                                message: "Authorization denied".to_string(),
                            });
                            break;
                        }
                        Err(err) => {
                            let _ = sender.send(AppMessage::AuthError {
                                message: err.to_string(),
                            });
                            break;
                        }
                    }
                }
                Ok(())
            });

            if let Err(err) = result {
                let _ = sender.send(AppMessage::AuthError {
                    message: err.to_string(),
                });
            }
        });
    }

    fn request_access_token(&mut self) {
        if self.auth_inflight {
            return;
        }
        let profile = match self.auth_profile.clone() {
            Some(name) => name,
            None => {
                self.push_session_update("Sign in to request a Beach Auth token.");
                return;
            }
        };
        self.auth_inflight = true;
        let sender = self.event_tx.clone();
        thread::spawn(move || {
            let runtime = Runtime::new().expect("failed to create tokio runtime for access token");
            let result = runtime.block_on(async {
                match auth::maybe_access_token(Some(&profile), true).await {
                    Ok(Some(token)) => {
                        let expiry = auth::load_store()
                            .ok()
                            .and_then(|store| profile_expiry(&store, &profile));
                        sender
                            .send(AppMessage::AccessToken {
                                profile: profile.clone(),
                                token,
                                expires_at: expiry,
                            })
                            .ok();
                        Ok(())
                    }
                    Ok(None) => Err(AuthError::ProfileNotFound(profile.clone())),
                    Err(err) => Err(err),
                }
            });
            if let Err(err) = result {
                sender
                    .send(AppMessage::AuthError {
                        message: err.to_string(),
                    })
                    .ok();
            }
        });
    }

    fn fetch_private_beaches(&mut self) {
        if self.fetching_beaches {
            return;
        }
        let token = match self.access_token.clone() {
            Some(token) => token,
            None => {
                self.push_session_update("Sign in to list private beaches.");
                return;
            }
        };
        self.fetching_beaches = true;
        let base = self.manager_base.clone();
        let sender = self.event_tx.clone();
        thread::spawn(move || {
            let runtime = Runtime::new().expect("failed to create tokio runtime for beaches");
            let result = runtime.block_on(async move {
                let client = reqwest::Client::new();
                let url = format!("{}/private-beaches", base.trim_end_matches('/'));
                let response = client
                    .get(url)
                    .bearer_auth(&token)
                    .send()
                    .await
                    .map_err(|err| err.to_string())?;
                if !response.status().is_success() {
                    return Err(format!("HTTP {}", response.status()));
                }
                response
                    .json::<Vec<PrivateBeachSummary>>()
                    .await
                    .map_err(|err| err.to_string())
            });
            match result {
                Ok(list) => {
                    sender.send(AppMessage::BeachesLoaded(list)).ok();
                }
                Err(err) => {
                    sender.send(AppMessage::BeachesError(err)).ok();
                }
            }
        });
    }

    fn begin_create_public_session(&mut self) {
        if self.creating_public_session {
            return;
        }
        if self.session_sheet.descriptor().is_none() {
            self.push_session_update("Select a target before creating a session.");
            return;
        }
        self.creating_public_session = true;
        let road = self.road_base.clone();
        let sender = self.event_tx.clone();
        thread::spawn(move || {
            let runtime =
                Runtime::new().expect("failed to create tokio runtime for session creation");
            let result = runtime.block_on(async move {
                let config = SessionConfig::new(&road).map_err(|err| err.to_string())?;
                let manager = SessionManager::new(config).map_err(|err| err.to_string())?;
                let host_session = manager.host().await.map_err(|err| match err {
                    SessionError::HttpStatus(status) => {
                        format!("Beach Road returned {}", status)
                    }
                    other => other.to_string(),
                })?;
                Ok(PublicSessionInfo {
                    session_id: host_session.session_id().to_string(),
                    join_code: host_session.join_code().to_string(),
                    session_url: host_session.handle().session_url().to_string(),
                })
            });
            match result {
                Ok(info) => {
                    sender.send(AppMessage::PublicSessionCreated(info)).ok();
                }
                Err(err) => {
                    sender.send(AppMessage::PublicSessionError(err)).ok();
                }
            }
        });
    }

    fn begin_attach_private_beach(&mut self) {
        if self.attaching_private_beach {
            return;
        }
        let token = match self.access_token.clone() {
            Some(token) => token,
            None => {
                self.push_session_update("Sign in to attach sessions to a private beach.");
                return;
            }
        };
        let descriptor = match self.session_sheet.descriptor().cloned() {
            Some(descriptor) => descriptor,
            None => {
                self.push_session_update("Select a capture target before attaching.");
                return;
            }
        };
        let public = match self.public_session.clone() {
            Some(info) => info,
            None => {
                self.push_session_update("Create a public session before attaching.");
                return;
            }
        };
        let beach_id = match self.selected_beach_id.clone() {
            Some(id) => id,
            None => {
                self.push_session_update("Choose a private beach to attach to.");
                return;
            }
        };
        let beach_name = self
            .beaches
            .iter()
            .find(|beach| beach.id == beach_id)
            .map(|b| b.name.clone())
            .unwrap_or_else(|| "selected private beach".to_string());
        self.attaching_private_beach = true;
        let manager = self.manager_base.clone();
        let session_label = self.session_sheet.session_name().to_string();
        let picker_label = self.session_sheet.label().map(|s| s.to_string());
        let application = self.session_sheet.application().map(|s| s.to_string());
        let kind = self.session_sheet.kind();
        let picker_metadata = self.session_sheet.metadata().cloned();
        let sender = self.event_tx.clone();
        thread::spawn(move || {
            let runtime =
                Runtime::new().expect("failed to create tokio runtime for private attach");
            let result = runtime.block_on(async move {
                attach_private_beach(
                    &manager,
                    &token,
                    &beach_id,
                    &beach_name,
                    &public,
                    &descriptor,
                    picker_label,
                    application,
                    kind,
                    picker_metadata,
                    session_label,
                )
                .await
            });
            match result {
                Ok(_) => {
                    sender
                        .send(AppMessage::PrivateAttachOk {
                            beach_id,
                            beach_name,
                            session_id: public.session_id,
                        })
                        .ok();
                }
                Err(err) => {
                    sender.send(AppMessage::PrivateAttachError(err)).ok();
                }
            }
        });
    }

    fn push_session_update(&mut self, message: impl Into<String>) {
        const MAX: usize = 12;
        self.session_updates.push_back(message.into());
        while self.session_updates.len() > MAX {
            self.session_updates.pop_front();
        }
    }

    fn render_session_actions(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            ui.label(RichText::new("Beach Auth").strong());
            ui.add_space(4.0);
            match &self.auth_status {
                AuthStatus::LoggedOut => {
                    ui.label("Not signed in.");
                }
                AuthStatus::Starting => {
                    ui.label("Starting login flow…");
                }
                AuthStatus::Pending(prompt) => {
                    ui.label("Complete login in your browser:");
                    ui.monospace(format!("Code: {}", prompt.user_code));
                    ui.hyperlink(&prompt.verification_uri_complete);
                    if let Ok(remaining) = prompt.expires_at.duration_since(SystemTime::now()) {
                        ui.small(format!("Expires in {} seconds.", remaining.as_secs()));
                    }
                }
                AuthStatus::LoggedIn {
                    profile,
                    email,
                    tier,
                } => {
                    ui.label(format!("Profile: {}", profile));
                    if let Some(email) = email {
                        ui.label(format!("Email: {}", email));
                    }
                    if let Some(tier) = tier {
                        ui.label(format!("Tier: {}", tier));
                    }
                    if let Some(expiry) = self.access_token_expiry {
                        if let Ok(remaining) = expiry.duration_since(SystemTime::now()) {
                            ui.small(format!(
                                "Access token expires in ~{} minute(s).",
                                (remaining.as_secs() / 60).max(1)
                            ));
                        }
                    }
                }
                AuthStatus::Error(message) => {
                    ui.colored_label(Color32::from_rgb(220, 70, 70), message);
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !self.auth_inflight,
                        egui::Button::new("Sign in with Beach Auth"),
                    )
                    .clicked()
                {
                    self.begin_auth_login();
                }
                if ui
                    .add_enabled(
                        !self.auth_inflight
                            && matches!(self.auth_status, AuthStatus::LoggedIn { .. }),
                        egui::Button::new("Refresh token"),
                    )
                    .clicked()
                {
                    self.request_access_token();
                }
            });
        });

        ui.add_space(12.0);
        ui.group(|ui| {
            ui.label(RichText::new("Public session").strong());
            ui.add_space(4.0);
            ui.small(format!("Beach Road: {}", self.road_base));
            let can_create =
                self.session_sheet.descriptor().is_some() && !self.creating_public_session;
            if ui
                .add_enabled(can_create, egui::Button::new("Create public session"))
                .clicked()
            {
                self.begin_create_public_session();
            }
            if self.creating_public_session {
                ui.label("Creating session…");
            }
            if let Some(session) = &self.public_session {
                ui.add_space(6.0);
                ui.label(format!("Session ID: {}", session.session_id));
                ui.horizontal(|ui| {
                    ui.label("Join code:");
                    ui.monospace(&session.join_code);
                    if ui.button("Copy").clicked() {
                        ui.output_mut(|o| o.copied_text = session.join_code.clone());
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Session URL:");
                    ui.hyperlink(&session.session_url);
                    if ui.button("Open").clicked() {
                        if let Err(err) = open::that(&session.session_url) {
                            self.push_session_update(format!("Failed to open URL: {err}"));
                        }
                    }
                });
            } else {
                ui.add_space(4.0);
                ui.small("No session created yet.");
            }
        });

        ui.add_space(12.0);
        ui.group(|ui| {
            ui.label(RichText::new("Private beach").strong());
            ui.add_space(4.0);
            ui.small(format!("Manager: {}", self.manager_base));
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !self.fetching_beaches
                            && self.access_token.is_some()
                            && matches!(self.auth_status, AuthStatus::LoggedIn { .. }),
                        egui::Button::new("Refresh beaches"),
                    )
                    .clicked()
                {
                    self.fetch_private_beaches();
                }
                if self.fetching_beaches {
                    ui.label("Loading…");
                }
            });
            if self.beaches.is_empty() {
                ui.label("No beaches loaded.");
            } else {
                let selected_name = self
                    .selected_beach_id
                    .as_ref()
                    .and_then(|id| {
                        self.beaches
                            .iter()
                            .find(|b| &b.id == id)
                            .map(|b| format!("{} ({})", b.name, b.slug))
                    })
                    .unwrap_or_else(|| "Select private beach".to_string());
                ComboBox::from_label("Private beach")
                    .selected_text(selected_name)
                    .show_ui(ui, |cb| {
                        for beach in &self.beaches {
                            cb.selectable_value(
                                &mut self.selected_beach_id,
                                Some(beach.id.clone()),
                                format!("{} ({})", beach.name, beach.slug),
                            );
                        }
                    });
            }
            let can_attach = self.access_token.is_some()
                && self.public_session.is_some()
                && self.session_sheet.descriptor().is_some()
                && self.selected_beach_id.is_some()
                && !self.attaching_private_beach;
            if ui
                .add_enabled(can_attach, egui::Button::new("Attach session"))
                .clicked()
            {
                self.begin_attach_private_beach();
            }
            if self.attaching_private_beach {
                ui.label("Attaching session…");
            }
        });

        ui.add_space(12.0);
        ui.group(|ui| {
            ui.label(RichText::new("Session log").strong());
            if self.session_updates.is_empty() {
                ui.label("No host session events yet.");
            } else {
                for entry in self.session_updates.iter().rev() {
                    ui.label(entry);
                }
            }
        });
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(100));

        self.poll_app_events();
        self.poll_picker_events();

        egui::TopBottomPanel::top("cabana-top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Cabana macOS Picker");
                ui.add_space(12.0);
                #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
                {
                    if ui.button("Reopen picker").clicked() {
                        self.reopen_picker();
                    }
                }
                if let Some(status) = &self.status_message {
                    ui.add_space(16.0);
                    ui.label(status.clone());
                }
            });
        });

        #[cfg(any(feature = "picker-mock", feature = "picker-native"))]
        {
            egui::SidePanel::left("cabana-tile-panel")
                .min_width(320.0)
                .resizable(true)
                .show(ctx, |ui| {
                    ui.heading("Capture targets");
                    ui.add_space(6.0);
                    ScrollArea::vertical().show(ui, |ui| {
                        if self.tiles.is_empty() {
                            ui.label("Waiting for picker events…");
                        } else {
                            ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
                            ui.horizontal_wrapped(|ui| {
                                for tile in &self.tiles {
                                    let is_selected = self
                                        .selected_id
                                        .as_deref()
                                        .map(|id| id == tile.result.id)
                                        .unwrap_or(false);
                                    let response = render_tile(ui, tile, is_selected);
                                    if response.clicked() {
                                        self.apply_selection(
                                            tile.result.clone(),
                                            SelectionSource::Tile,
                                        );
                                    }
                                }
                            });
                        }
                    });
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                ui.heading("Session sheet");
                ui.add_space(6.0);
                self.session_sheet.render(ui);
                ui.add_space(12.0);
                self.render_session_actions(ui);
            });
        });

        egui::TopBottomPanel::bottom("cabana-telemetry")
            .resizable(false)
            .show(ctx, |ui| {
                ui.heading("Telemetry");
                ui.add_space(4.0);
                if self.telemetry_log.is_empty() {
                    ui.label("No telemetry events recorded yet.");
                } else {
                    for entry in self.telemetry_log.iter().rev() {
                        ui.label(entry);
                    }
                }
            });
    }
}

struct SessionSheetState {
    session_name: String,
    descriptor: Option<ScreenCaptureDescriptor>,
    label: Option<String>,
    application: Option<String>,
    kind: Option<PickerItemKind>,
    metadata: Option<JsonValue>,
    confirmed_at: Option<SystemTime>,
}

impl SessionSheetState {
    fn new() -> Self {
        Self {
            session_name: String::new(),
            descriptor: None,
            label: None,
            application: None,
            kind: None,
            metadata: None,
            confirmed_at: None,
        }
    }

    fn update_selection(
        &mut self,
        descriptor: ScreenCaptureDescriptor,
        label: String,
        application: Option<String>,
        kind: PickerItemKind,
        metadata: Option<JsonValue>,
    ) {
        self.descriptor = Some(descriptor);
        self.label = Some(label);
        self.application = application;
        self.kind = Some(kind);
        self.metadata = metadata;
        self.confirmed_at = Some(SystemTime::now());
    }

    fn render(&mut self, ui: &mut egui::Ui) {
        if self.descriptor.is_none() {
            ui.label("Pick a window or display to configure a session.");
            return;
        }

        if let Some(label) = &self.label {
            ui.label(RichText::new(label).strong());
        }
        if let Some(app) = &self.application {
            ui.label(app.clone());
        }
        if let Some(kind) = self.kind {
            ui.label(format!("Capture kind: {}", kind_label(kind)));
        }
        if let Some(descriptor) = &self.descriptor {
            ui.add_space(4.0);
            ui.label(format!("Target identifier: {}", descriptor.target_id));
            if let Some(ts) = self.confirmed_at {
                if let Ok(ms) = ts.duration_since(UNIX_EPOCH) {
                    ui.label(format!("Selected at: {} ms", ms.as_millis()));
                }
            }
        }

        ui.add_space(6.0);
        ui.label("Session nickname (optional)");
        ui.text_edit_singleline(&mut self.session_name)
            .hint_text("Displayed to viewers and in Private Beach");

        if let Some(metadata) = &self.metadata {
            ui.add_space(6.0);
            ui.label("Picker metadata");
            for (key, value) in metadata_entries(metadata).into_iter().take(6) {
                ui.label(format!("• {}: {}", key, value));
            }
        }

        ui.add_space(8.0);
        ui.small("Use the actions below to create a session or attach to a private beach.");
    }

    fn descriptor(&self) -> Option<&ScreenCaptureDescriptor> {
        self.descriptor.as_ref()
    }

    fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    fn application(&self) -> Option<&str> {
        self.application.as_deref()
    }

    fn kind(&self) -> Option<PickerItemKind> {
        self.kind
    }

    fn metadata(&self) -> Option<&JsonValue> {
        self.metadata.as_ref()
    }

    fn session_name(&self) -> &str {
        self.session_name.trim()
    }
}

#[cfg(any(feature = "picker-mock", feature = "picker-native"))]
struct PickerTile {
    result: PickerResult,
    metadata: Option<JsonValue>,
}

#[cfg(not(any(feature = "picker-mock", feature = "picker-native")))]
type PickerTile = ();

enum SelectionSource {
    Stream,
    Tile,
}

#[cfg(any(feature = "picker-mock", feature = "picker-native"))]
fn render_tile(ui: &mut egui::Ui, tile: &PickerTile, selected: bool) -> egui::Response {
    let stroke_color = if selected {
        Color32::from_rgb(64, 145, 255)
    } else {
        Color32::from_rgb(90, 90, 90)
    };
    let fill_color = if selected {
        Color32::from_rgb(30, 40, 70)
    } else {
        Color32::from_rgb(24, 24, 24)
    };

    let frame = egui::Frame::group(ui.style())
        .fill(fill_color)
        .stroke(egui::Stroke::new(
            if selected { 2.0 } else { 1.0 },
            stroke_color,
        ))
        .rounding(egui::Rounding::same(10.0));

    frame
        .show(ui, |ui| {
            ui.set_width(220.0);
            ui.set_min_height(140.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(tile.result.label.clone()).strong());
                if let Some(app) = &tile.result.application {
                    ui.label(app.clone());
                }
                ui.label(kind_label(tile.result.kind));
                if let Some(meta) = &tile.metadata {
                    for (key, value) in metadata_entries(meta).into_iter().take(2) {
                        ui.small(format!("{}: {}", key, value));
                    }
                }
            });
        })
        .response
}

#[cfg(any(feature = "picker-mock", feature = "picker-native"))]
fn kind_label(kind: PickerItemKind) -> &'static str {
    match kind {
        PickerItemKind::Window => "Window",
        PickerItemKind::Display => "Display",
        PickerItemKind::Application => "Application",
        PickerItemKind::Unknown => "Unknown",
    }
}

#[cfg(not(any(feature = "picker-mock", feature = "picker-native")))]
fn kind_label(_kind: PickerItemKind) -> &'static str {
    "Unknown"
}

fn decode_metadata(raw: &Option<String>) -> Option<JsonValue> {
    raw.as_ref()
        .and_then(|json| serde_json::from_str::<JsonValue>(json).ok())
}

fn metadata_entries(metadata: &JsonValue) -> Vec<(String, String)> {
    match metadata {
        JsonValue::Object(map) => map
            .iter()
            .map(|(k, v)| (k.clone(), summarize_json_value(v)))
            .collect(),
        other => vec![("value".to_string(), summarize_json_value(other))],
    }
}

fn summarize_json_value(value: &JsonValue) -> String {
    match value {
        JsonValue::String(s) => s.clone(),
        JsonValue::Number(num) => num.to_string(),
        JsonValue::Bool(flag) => flag.to_string(),
        JsonValue::Null => "null".to_string(),
        JsonValue::Array(items) => {
            if items.is_empty() {
                "[]".to_string()
            } else {
                format!("{} item(s)", items.len())
            }
        }
        JsonValue::Object(_) => "object".to_string(),
    }
}

fn default_road_base() -> String {
    env::var("CABANA_ROAD_URL")
        .or_else(|_| env::var("BEACH_SESSION_SERVER"))
        .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string())
}

fn default_manager_base() -> String {
    env::var("CABANA_MANAGER_URL")
        .or_else(|_| env::var("NEXT_PUBLIC_MANAGER_URL"))
        .unwrap_or_else(|_| "http://localhost:8080".to_string())
}

fn profile_expiry(store: &auth::credentials::CredentialsStore, name: &str) -> Option<SystemTime> {
    store
        .profile(name)
        .and_then(|profile: &StoredProfile| profile.access_token.as_ref())
        .map(|cache| cache.expires_at.into())
}

async fn attach_private_beach(
    manager_base: &str,
    token: &str,
    beach_id: &str,
    beach_name: &str,
    session: &PublicSessionInfo,
    descriptor: &ScreenCaptureDescriptor,
    picker_label: Option<String>,
    application: Option<String>,
    kind: Option<PickerItemKind>,
    picker_metadata: Option<JsonValue>,
    session_name: String,
) -> Result<(), String> {
    #[derive(Serialize)]
    struct AttachBody<'a> {
        session_id: &'a str,
        code: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_descriptor: Option<JsonValue>,
    }

    #[derive(Serialize)]
    struct SessionUpdate<'a> {
        metadata: Option<JsonValue>,
        #[serde(skip_serializing_if = "Option::is_none")]
        location_hint: Option<&'a str>,
    }

    let descriptor_json = json!({
        "target_id": descriptor.target_id,
        "filter_base64": BASE64_STANDARD.encode(&descriptor.filter_blob),
        "stream_config_base64": descriptor
            .stream_config_blob
            .as_ref()
            .map(|blob| BASE64_STANDARD.encode(blob)),
    });

    let mut session_metadata = json!({
        "cabana": {
            "label": picker_label,
            "application": application,
            "kind": kind.map(kind_label),
            "descriptor": descriptor_json,
            "picker_metadata": picker_metadata,
        }
    });

    if !session_name.trim().is_empty() {
        session_metadata
            .as_object_mut()
            .and_then(|obj| obj.get_mut("cabana"))
            .and_then(JsonValue::as_object_mut)
            .map(|cabana| {
                cabana.insert("session_name".to_string(), JsonValue::String(session_name));
            });
    }

    let client = reqwest::Client::new();
    let base = manager_base.trim_end_matches('/');

    let attach_url = format!(
        "{}/private-beaches/{}/sessions/attach-by-code",
        base, beach_id
    );
    let attach_body = AttachBody {
        session_id: &session.session_id,
        code: &session.join_code,
        capture_descriptor: Some(descriptor_json.clone()),
    };

    let response = client
        .post(&attach_url)
        .bearer_auth(token)
        .json(&attach_body)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Attach failed ({} {}): {}",
            response.status(),
            beach_name,
            response.text().await.unwrap_or_default()
        ));
    }

    let update_url = format!("{}/sessions/{}", base, session.session_id);
    let update_body = SessionUpdate {
        metadata: Some(session_metadata),
        location_hint: None,
    };

    let response = client
        .patch(&update_url)
        .bearer_auth(token)
        .json(&update_body)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() && response.status() != StatusCode::NO_CONTENT {
        return Err(format!(
            "Metadata update failed ({}): {}",
            response.status(),
            response.text().await.unwrap_or_default()
        ));
    }

    Ok(())
}
