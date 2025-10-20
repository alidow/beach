mod cli;
mod tui_pick;

use anyhow::{anyhow, Result};
use clap::Parser;
use rand::{rngs::OsRng, RngCore};
use std::fs;
use std::io::Write as _;
use std::time::{Duration, SystemTime};
use tracing_subscriber::{fmt, EnvFilter};

use beach_cabana_host as cabana;

#[cfg(target_os = "macos")]
use cabana::encoder::GifVideoEncoder;
#[cfg(all(target_os = "macos", feature = "cabana_sck"))]
use cabana::encoder::VideoToolboxEncoder;

fn main() -> Result<()> {
    init_tracing()?;
    let cli = cli::Cli::parse();
    run(cli)
}

fn init_tracing() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    match fmt().with_env_filter(filter).try_init() {
        Ok(()) => Ok(()),
        Err(err)
            if err
                .to_string()
                .contains("attempted to set a global default subscriber more than once") =>
        {
            Ok(())
        }
        Err(err) => Err(anyhow!(err)),
    }
}

fn run(cli: cli::Cli) -> Result<()> {
    match cli.command {
        cli::Commands::ListWindows { json } => {
            let windows = cabana::platform::enumerate_windows().map_err(|e| anyhow!(e.to_string()))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&windows)?);
            } else if windows.is_empty() {
                println!("No windows detected yet. Capture adapters are not implemented for this platform.");
            } else {
                for win in windows {
                    println!(
                        "[{}] {} ({}) — kind: {:?}",
                        win.identifier, win.title, win.application, win.kind
                    );
                }
            }
        }
        cli::Commands::Preview { window_id } => {
            let Some(window_id) = resolve_window_id(window_id)? else {
                eprintln!("Selection canceled");
                return Ok(());
            };
            prompt_screen_recording_permission();
            match cabana::platform::preview_window(&window_id) {
                Ok(path) => {
                    println!("Saved preview frame to {}", path.display());
                }
                Err(cabana::platform::WindowApiError::PreviewNotImplemented) => {
                    println!("Preview not yet implemented; window id: {}", window_id);
                }
                Err(cabana::platform::WindowApiError::EnumerationFailed(reason)) => {
                    println!(
                        "Preview unavailable because enumeration failed: {}",
                        reason
                    );
                }
                Err(cabana::platform::WindowApiError::InvalidIdentifier(reason)) => {
                    println!("Invalid target: {}", reason);
                }
                Err(cabana::platform::WindowApiError::CaptureFailed(reason)) => {
                    println!("Failed to capture preview: {}", reason);
                }
            }
        }
        cli::Commands::Pick {} => {
            match tui_pick::run_picker()? {
                Some(id) => {
                    println!("{}", id);
                }
                None => {
                    eprintln!("Selection canceled");
                }
            }
        }
        cli::Commands::Stream { window_id, frames, interval_ms, output_dir } => {
            #[cfg(target_os = "macos")]
            {
                use cabana::capture::create_producer;
                use image::{DynamicImage, ImageBuffer};

                if frames == 0 {
                    println!("Streaming requires at least one frame.");
                    return Ok(());
                }

                let window_id = match resolve_window_id(window_id)? {
                    Some(id) => id,
                    None => {
                        eprintln!("Selection canceled");
                        return Ok(());
                    }
                };
                prompt_screen_recording_permission();

                let mut producer = match create_producer(&window_id) {
                    Ok(p) => p,
                    Err(err) => {
                        println!("Failed to create capture producer: {}", err);
                        return Ok(());
                    }
                };

                if let Err(err) = producer.start() {
                    println!("Failed to start capture: {}", err);
                    return Ok(());
                }

                let base_dir = output_dir.unwrap_or_else(|| {
                    let ts = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    std::env::temp_dir().join(format!("beach-cabana-stream-{}", ts))
                });

                if let Err(err) = fs::create_dir_all(&base_dir) {
                    println!("Failed to create output directory {}: {}", base_dir.display(), err);
                    producer.stop();
                    return Ok(());
                }

                let mut captured_paths = Vec::new();
                let interval = Duration::from_millis(interval_ms);
                let mut capture_durations = Vec::with_capacity(frames as usize);
                let mut total_data_bytes: usize = 0;

                for index in 0..frames {
                    let started = std::time::Instant::now();
                    match producer.next_frame() {
                        Ok(mut frame) => {
                            let elapsed = started.elapsed();
                            capture_durations.push(elapsed);
                            total_data_bytes += frame.data.len();

                            if let Err(err) = adjust_frame_size(&mut frame, None) {
                                println!("Failed to process frame {}: {}", index, err);
                                continue;
                            }

                            let Some(buffer) = ImageBuffer::<image::Rgba<u8>, _>::from_vec(
                                frame.width,
                                frame.height,
                                frame.data,
                            ) else {
                                println!("Failed to build image buffer for frame {}", index);
                                continue;
                            };

                            let frame_path = base_dir.join(format!("frame_{:03}.png", index));
                            if let Err(err) = DynamicImage::ImageRgba8(buffer).save(&frame_path) {
                                println!("Failed to write frame {}: {}", index, err);
                                continue;
                            }

                            captured_paths.push(frame_path);
                        }
                        Err(err) => {
                            println!("Failed to capture frame {}: {}", index, err);
                            break;
                        }
                    }

                    if index + 1 < frames {
                        std::thread::sleep(interval);
                    }
                }

                producer.stop();

                if captured_paths.is_empty() {
                    println!("No frames captured.");
                } else {
                    println!(
                        "Captured {} frame(s) into {}",
                        captured_paths.len(),
                        base_dir.display()
                    );
                    for path in captured_paths {
                        println!("  - {}", path.display());
                    }
                    if !capture_durations.is_empty() {
                        let total_micros: u128 = capture_durations
                            .iter()
                            .map(|d| d.as_micros() as u128)
                            .sum();
                        let avg_ms = (total_micros as f64 / capture_durations.len() as f64) / 1000.0;
                        let max_ms = capture_durations
                            .iter()
                            .map(|d| d.as_secs_f64() * 1000.0)
                            .fold(0.0, f64::max);
                        let min_ms = capture_durations
                            .iter()
                            .map(|d| d.as_secs_f64() * 1000.0)
                            .fold(f64::INFINITY, f64::min);
                        println!("Capture metrics:");
                        println!(
                            "  - Avg frame latency: {:.2} ms (min {:.2} ms, max {:.2} ms)",
                            avg_ms, min_ms, max_ms
                        );
                        println!(
                            "  - Approx total frame bytes: {} ({} bytes/frame avg)",
                            total_data_bytes,
                            if capture_durations.is_empty() {
                                0
                            } else {
                                total_data_bytes / capture_durations.len()
                            }
                        );
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                println!("Streaming is not implemented for this platform yet.");
            }
        }
        cli::Commands::Encode { window_id, duration_secs, fps, max_width, output, codec } => {
            #[cfg(target_os = "macos")]
            {
                use cabana::capture::create_producer;
                use cli::EncodeCodec;

                let fps = fps.max(1);
                let total_frames = duration_secs.saturating_mul(fps);
                if total_frames == 0 { println!("Encoding requires a positive duration and fps."); return Ok(()); }

                let window_id = match resolve_window_id(window_id)? {
                    Some(id) => id,
                    None => {
                        eprintln!("Selection canceled");
                        return Ok(());
                    }
                };
                prompt_screen_recording_permission();

                let mut producer = match create_producer(&window_id) { Ok(p) => p, Err(err) => { println!("Failed to create capture producer: {}", err); return Ok(()); } };
                producer.start()?;

                let mut encoder: Option<ActiveEncoder> = None;
                let mut active_codec = codec;
                let mut frames_written = 0u32;
                let frame_interval = Duration::from_secs_f32(1.0 / fps as f32);

                for index in 0..total_frames {
                    let mut frame = match producer.next_frame() { Ok(frame) => frame, Err(err) => { println!("Failed to capture frame {}: {}", index, err); break; } };
                    if let Err(err) = adjust_frame_size(&mut frame, max_width) { println!("Failed to process frame {}: {}", index, err); continue; }
                    if frame.width == 0 || frame.height == 0 { println!("Frame {} has invalid dimensions; skipping.", index); continue; }

                    if encoder.is_none() {
                        let new_encoder = match active_codec {
                            EncodeCodec::Gif => match GifVideoEncoder::new(&output, frame.width, frame.height, fps) { Ok(enc) => Some(ActiveEncoder::Gif(enc)), Err(err) => { println!("Failed to initialize GIF encoder: {}", err); None } },
                            EncodeCodec::H264 => {
                                #[cfg(feature = "cabana_sck")]
                                { match VideoToolboxEncoder::new(&output, frame.width, frame.height, fps) { Ok(enc) => Some(ActiveEncoder::H264(enc)), Err(err) => { println!("VideoToolbox encoder failed to initialize ({}); falling back to GIF.", err); active_codec = EncodeCodec::Gif; match GifVideoEncoder::new(&output, frame.width, frame.height, fps) { Ok(enc) => Some(ActiveEncoder::Gif(enc)), Err(err) => { println!("Failed to initialize GIF encoder: {}", err); None } } } } }
                                #[cfg(not(feature = "cabana_sck"))]
                                { println!("VideoToolbox encoder requires the cabana_sck feature; falling back to GIF."); active_codec = EncodeCodec::Gif; match GifVideoEncoder::new(&output, frame.width, frame.height, fps) { Ok(enc) => Some(ActiveEncoder::Gif(enc)), Err(err) => { println!("Failed to initialize GIF encoder: {}", err); None } } }
                            }
                        };
                        if let Some(enc) = new_encoder { encoder = Some(enc); } else { break; }
                    }

                    if let Some(enc) = encoder.as_mut() { if let Err(err) = enc.write_frame(&frame) { println!("Failed to encode frame {}: {}", index, err); continue; } frames_written += 1; }
                    if index + 1 < total_frames { std::thread::sleep(frame_interval); }
                }

                producer.stop();

                if let Some(enc) = encoder { if let Err(err) = enc.finish() { println!("Failed to finalize encoder: {}", err); } }

                if frames_written == 0 { println!("No frames encoded; removing {}", output.display()); let _ = std::fs::remove_file(&output); } else { let codec_label = match active_codec { EncodeCodec::Gif => "GIF", EncodeCodec::H264 => "H264 (Annex B)" }; println!("Encoded {} frame(s) using {} to {}", frames_written, codec_label, output.display()); }
            }
            #[cfg(not(target_os = "macos"))]
            { println!("Encoding is not implemented for this platform yet (requested codec: {:?}).", codec); }
        }
        cli::Commands::Start { session_url, session_id, passcode, window_id, handshake_id, payload_file, fixture_url } => {
            let session_identifier = session_id.or_else(|| session_url.as_deref().and_then(extract_session_id)).ok_or_else(|| anyhow!("provide either --session-id or --session-url"))?;
            let material = cabana::security::SessionMaterial::derive(&session_identifier, &passcode)?;
            let handshake = match handshake_id { Some(id) => cabana::security::HandshakeId::from_base64(&id)?, None => cabana::security::HandshakeId::random(), };

            let payload = if let Some(ref path) = payload_file {
                let data = fs::read(path).map_err(|e| anyhow!("failed to read payload file {}: {}", path.display(), e))?;
                println!("Loaded payload file ({} bytes): {}", data.len(), path.display());
                data
            } else {
                let mut probe_payload = [0u8; 32]; OsRng.fill_bytes(&mut probe_payload); probe_payload.to_vec()
            };

            let envelope = cabana::security::seal_signaling_payload(&material, &handshake, &payload)?;
            println!("Session ID     : {}", session_identifier);
            if let Some(url) = session_url { println!("Session URL    : {}", url); }
            println!("Window target  : {}", window_id.unwrap_or_else(|| "interactive".into()));
            println!("Handshake ID   : {}", handshake.to_base64());
            println!("Passcode SHA256: {}", material.passcode_fingerprint());
            println!("Sealed envelope: {}", envelope.compact_encoding());
            println!("Derived preview: {}", hex::encode(material.preview_signaling_key(&handshake)?));
            if payload_file.is_none() { println!("Note: payload not provided, emitted random probe for testing."); }
            if let Some(url) = fixture_url { println!("Posting envelope to fixture {}", url); beach_cabana_host::fixture::client::post_envelope(&url, beach_cabana_host::fixture::client::FixtureEnvelope { session_id: &session_identifier, handshake_b64: &handshake.to_base64(), envelope: &envelope.compact_encoding(), }).map_err(|e| anyhow!("failed to post envelope to fixture: {}", e))?; println!("Fixture accepted envelope."); }
            println!("\nNext steps:\n  - Implement capture adapters per OS (see docs/beach-cabana/phase0.md).\n  - Replace probe sealing with real offer/answer payloads.\n  - Use `beach-cabana fixture-serve` with `--fixture-url` to rehearse sealed signaling end-to-end.");
        }
        cli::Commands::SealProbe { session_id, passcode, payload, handshake_id } => { let material = cabana::security::SessionMaterial::derive(&session_id, &passcode)?; let handshake = match handshake_id { Some(id) => cabana::security::HandshakeId::from_base64(&id)?, None => cabana::security::HandshakeId::random(), }; let envelope = cabana::security::seal_signaling_payload(&material, &handshake, payload.as_bytes())?; println!("{}", envelope.compact_encoding()); }
        cli::Commands::OpenProbe { session_id, passcode, envelope } => { let material = cabana::security::SessionMaterial::derive(&session_id, &passcode)?; let envelope = cabana::security::SealedEnvelope::from_compact(&envelope)?; let plaintext = cabana::security::open_signaling_payload(&material, &envelope)?; println!("{}", String::from_utf8_lossy(&plaintext)); }
        cli::Commands::FixtureServe { listen, storage_dir } => { beach_cabana_host::fixture::server::serve(listen, storage_dir)?; }
        cli::Commands::NoiseDiag { .. } => {
            println!("Noise diagnostic flow is moving under the host crate; not wired in this refactor step.");
        }
        #[cfg(feature = "webrtc")]
        cli::Commands::WebRtcHostRun { session_id, passcode, codec, road_url, fixture_url, fixture_dir, prologue, mut window_id, frames, interval_ms, max_width, from_id, to_id } => {
            // Optional interactive picker if no window selected and streaming requested
            if window_id.is_none() && frames > 0 {
                if let Some(id) = crate::tui_pick::run_picker()? { window_id = Some(id); } else { println!("Canceled."); return Ok(()); }
            }
            let rt = tokio::runtime::Runtime::new()?;
            let (transport, code, selected, codec_host) = rt.block_on(async move {
                let (t, c) = cabana::webrtc::host_bootstrap(
                    session_id.clone(),
                    passcode.clone(),
                    road_url.clone(),
                    fixture_url.clone(),
                    fixture_dir.clone(),
                    prologue.clone().into_bytes(),
                    from_id.clone(),
                    to_id.clone(),
                ).await?;
                Ok::<_, anyhow::Error>((t, c, window_id, match codec { cli::EncodeCodec::Gif => cabana::webrtc::EncodeCodec::Gif, cli::EncodeCodec::H264 => cabana::webrtc::EncodeCodec::H264 }))
            })?;
            println!("Host secured channel");
            println!("  Verification   : {}", code);
            // Gate: require confirmation before streaming
            if let Some(id) = selected {
                println!("Start streaming '{}' now? [y/N]", id);
                let mut input = String::new();
                std::io::stdout().flush().ok();
                std::io::stdin().read_line(&mut input).ok();
                if matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
                    // macOS permission guidance (best-effort) before starting
                    prompt_screen_recording_permission();
                    rt.block_on(async move {
                        #[cfg(target_os = "macos")]
                        {
                            cabana::webrtc::host_stream(&transport, codec_host, &id, frames, interval_ms, max_width).await.map_err(|e| anyhow::anyhow!(e.to_string()))
                        }
                        #[cfg(not(target_os = "macos"))]
                        { Ok::<_, anyhow::Error>(()) }
                    })?;
                } else {
                    println!("Streaming skipped by user.");
                }
            }
        }
        #[cfg(feature = "webrtc")]
        cli::Commands::WebRtcViewerAnswer { session_id, passcode, host_envelope, host_envelope_file, fixture_url, prologue } => {
            let env_str = if let Some(s) = host_envelope { s } else if let Some(path) = host_envelope_file { std::fs::read_to_string(path)? } else { return Err(anyhow!("provide --host-envelope or --host-envelope-file")); };
            let rt = tokio::runtime::Runtime::new()?;
            let result = rt.block_on(async move { cabana::webrtc::viewer_answer(session_id, passcode, env_str, fixture_url, prologue.into_bytes()).await })?;
            println!("Viewer answer result: {}", result);
        }
        #[cfg(feature = "webrtc")]
        cli::Commands::WebRtcViewerRun { session_id, passcode, host_envelope, host_envelope_file, fixture_url, prologue, recv_frames, output_dir, road_url, from_id, to_id } => {
            let env_str = if let Some(s) = host_envelope { s } else if let Some(path) = host_envelope_file { std::fs::read_to_string(path)? } else { String::new() };
            let rt = tokio::runtime::Runtime::new()?;
            let outcome = rt.block_on(async move {
                cabana::webrtc::viewer_run(
                    session_id,
                    passcode,
                    env_str,
                    fixture_url,
                    prologue.into_bytes(),
                    recv_frames,
                    output_dir,
                    road_url,
                    from_id,
                    to_id,
                    |code| {
                        println!("Viewer verification code: {}", code);
                        print!("Does the code match the host? [y/N]: ");
                        let _ = std::io::stdout().flush();
                        let mut input = String::new();
                        if std::io::stdin().read_line(&mut input).is_ok() {
                            matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
                        } else {
                            false
                        }
                    },
                ).await
            });
            match outcome {
                Ok(out_dir) => {
                    println!("Viewer received {} frame(s) into {}", recv_frames, out_dir.display());
                }
                Err(cabana::webrtc::NoiseDriverError::UserAborted) => {
                    println!("Viewer canceled playback before receiving frames.");
                }
                Err(err) => return Err(anyhow!(err)),
            }
        }
        #[cfg(feature = "webrtc")]
        cli::Commands::WebRtcLocal { session_id, passcode, host_id, viewer_id, prologue } => {
            let rt = tokio::runtime::Runtime::new()?;
            let material = cabana::security::SessionMaterial::derive(&session_id, &passcode)?;
            let handshake = cabana::security::HandshakeId::random();
            let code = rt.block_on(async move {
                cabana::webrtc::run_local_webrtc_noise_demo(material, handshake, host_id, viewer_id, prologue.into_bytes()).await
            })?;
            println!("Local WebRTC + Noise established\n  Verification   : {}", code);
        }
        #[cfg(feature = "webrtc")]
        _ => unreachable!(),
    }
    Ok(())
}

fn resolve_window_id(window_id: Option<String>) -> Result<Option<String>> {
    if let Some(id) = window_id {
        return Ok(Some(id));
    }

    if let Some(event) = cabana::desktop::last_selection() {
        println!("Using desktop picker selection '{}'", event.target_id);
        return Ok(Some(event.target_id));
    }

    let wait_ms_env = std::env::var("CABANA_PICKER_WAIT_MS")
        .ok()
        .and_then(|val| val.parse::<u64>().ok());
    let relay_enabled = std::env::var("CABANA_PICKER_RELAY")
        .ok()
        .map(|val| matches!(val.as_str(), "1" | "true" | "TRUE" | "True"))
        .unwrap_or(false);
    let wait_ms = wait_ms_env.or_else(|| if relay_enabled { Some(1500) } else { None });

    if let Some(ms) = wait_ms {
        if ms > 0 {
            println!(
                "Waiting up to {} ms for desktop picker selection…",
                ms
            );
            if let Some(event) =
                cabana::desktop::wait_for_selection(Some(Duration::from_millis(ms)))
            {
                println!("Received desktop picker selection '{}'", event.target_id);
                return Ok(Some(event.target_id));
            } else {
                println!("No desktop picker selection received; falling back to TUI picker.");
            }
        }
    }

    tui_pick::run_picker()
}

#[cfg(target_os = "macos")]
fn prompt_screen_recording_permission() {
    use beach_cabana_host::platform::macos::permissions::{
        request_access, status, ScreenRecordingStatus,
    };
    if status() != ScreenRecordingStatus::Granted {
        println!("Screen Recording permission not granted. If macOS prompts, approve access. If previously denied, enable it via System Settings ▸ Privacy & Security ▸ Screen Recording.");
        let _ = request_access();
    }
}

#[cfg(target_os = "windows")]
fn prompt_screen_recording_permission() {
    println!("Windows will show a one-time \"Screen capture\" toast the first time Cabana shares a window. If you dismissed it previously, enable access via Settings ▸ Privacy & security ▸ Screen capture.");
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn prompt_screen_recording_permission() {
    println!("This platform relies on compositor/portal prompts for capture permissions. Accept the request from your desktop environment when it appears.");
}

fn extract_session_id(url: &str) -> Option<String> { let trimmed = url.trim_end_matches('/'); trimmed.rsplit('/').next().map(|segment| segment.to_string()) }

#[cfg(target_os = "macos")]
use cabana::encoder::VideoEncoder;
#[cfg(target_os = "macos")]
enum ActiveEncoder { Gif(GifVideoEncoder), #[cfg(feature = "cabana_sck")] H264(VideoToolboxEncoder) }
#[cfg(target_os = "macos")]
impl ActiveEncoder {
    fn write_frame(&mut self, frame: &cabana::capture::Frame) -> Result<()> { match self { ActiveEncoder::Gif(enc) => enc.write_frame(frame), #[cfg(feature = "cabana_sck")] ActiveEncoder::H264(enc) => enc.write_frame(frame), } }
    fn finish(self) -> Result<()> { match self { ActiveEncoder::Gif(enc) => enc.finish(), #[cfg(feature = "cabana_sck")] ActiveEncoder::H264(enc) => enc.finish(), } }
}

#[cfg(target_os = "macos")]
fn adjust_frame_size(frame: &mut cabana::capture::Frame, max_width: Option<u32>) -> anyhow::Result<()> {
    if frame.width == 0 || frame.height == 0 { return Ok(()); }
    if let cabana::capture::PixelFormat::Bgra8888 = frame.pixel_format { for chunk in frame.data.chunks_mut(4) { chunk.swap(0, 2); } frame.pixel_format = cabana::capture::PixelFormat::Rgba8888; }
    if let Some(max_width) = max_width { if max_width > 0 && frame.width > max_width { let scale = max_width as f32 / frame.width as f32; let new_height = ((frame.height as f32 * scale).round() as u32).max(1); let Some(buffer) = image::ImageBuffer::<image::Rgba<u8>, _>::from_vec(frame.width, frame.height, frame.data.clone()) else { anyhow::bail!("failed to create image buffer for resize"); }; let resized = image::DynamicImage::ImageRgba8(buffer).resize(max_width, new_height, image::imageops::FilterType::Lanczos3).to_rgba8(); frame.width = resized.width(); frame.height = resized.height(); frame.data = resized.into_raw(); } }
    Ok(())
}
