mod capture;
mod cli;
mod encoder;
mod fixture;
mod platform;
mod noise;
mod security;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use rand::{rngs::OsRng, RngCore};
use std::fs;
use std::time::{Duration, SystemTime};
use tracing_subscriber::{fmt, EnvFilter};

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
            let windows = platform::enumerate_windows().context("window enumeration failed")?;
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
            match platform::preview_window(&window_id) {
                Ok(path) => {
                    println!("Saved preview frame to {}", path.display());
                }
                Err(platform::WindowApiError::PreviewNotImplemented) => {
                    println!("Preview not yet implemented; window id: {}", window_id);
                }
                Err(platform::WindowApiError::EnumerationFailed(reason)) => {
                    println!(
                        "Preview unavailable because enumeration failed: {}",
                        reason
                    );
                }
                Err(platform::WindowApiError::InvalidIdentifier(reason)) => {
                    println!("Invalid target: {}", reason);
                }
                Err(platform::WindowApiError::CaptureFailed(reason)) => {
                    println!("Failed to capture preview: {}", reason);
                }
            }
        }
        cli::Commands::Stream {
            window_id,
            frames,
            interval_ms,
            output_dir,
        } => {
            #[cfg(target_os = "macos")]
            {
                use crate::capture::create_producer;
                use image::{DynamicImage, ImageBuffer};

                if frames == 0 {
                    println!("Streaming requires at least one frame.");
                    return Ok(());
                }

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

                for index in 0..frames {
                    match producer.next_frame() {
                        Ok(mut frame) => {
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
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                println!("Streaming is not implemented for this platform yet.");
            }
        }
        cli::Commands::Encode {
            window_id,
            duration_secs,
            fps,
            max_width,
            output,
        } => {
            #[cfg(target_os = "macos")]
            {
                use crate::capture::create_producer;
                use crate::encoder::{GifVideoEncoder, VideoEncoder};

                let fps = fps.max(1);
                let total_frames = duration_secs.saturating_mul(fps);
                if total_frames == 0 {
                    println!("Encoding requires a positive duration and fps.");
                    return Ok(());
                }

                let mut producer = match create_producer(&window_id) {
                    Ok(p) => p,
                    Err(err) => {
                        println!("Failed to create capture producer: {}", err);
                        return Ok(());
                    }
                };

                producer.start()?;

                let mut encoder: Option<GifVideoEncoder> = None;
                let mut frames_written = 0u32;
                let frame_interval = Duration::from_secs_f32(1.0 / fps as f32);

                for index in 0..total_frames {
                    let mut frame = match producer.next_frame() {
                        Ok(frame) => frame,
                        Err(err) => {
                            println!("Failed to capture frame {}: {}", index, err);
                            break;
                        }
                    };

                    if let Err(err) = adjust_frame_size(&mut frame, max_width) {
                        println!("Failed to process frame {}: {}", index, err);
                        continue;
                    }

                    if frame.width == 0 || frame.height == 0 {
                        println!("Frame {} has invalid dimensions; skipping.", index);
                        continue;
                    }

                    if encoder.is_none() {
                        match GifVideoEncoder::new(&output, frame.width, frame.height, fps) {
                            Ok(enc) => encoder = Some(enc),
                            Err(err) => {
                                println!("Failed to initialize encoder: {}", err);
                                break;
                            }
                        }
                    }

                    if let Some(enc) = encoder.as_mut() {
                        if let Err(err) = enc.write_frame(&frame) {
                            println!("Failed to encode frame {}: {}", index, err);
                            continue;
                        }
                        frames_written += 1;
                    }

                    if index + 1 < total_frames {
                        std::thread::sleep(frame_interval);
                    }
                }

                producer.stop();

                if let Some(enc) = encoder {
                    if let Err(err) = enc.finish() {
                        println!("Failed to finalize GIF: {}", err);
                    }
                }

                if frames_written == 0 {
                    println!("No frames encoded; removing {}", output.display());
                    let _ = std::fs::remove_file(&output);
                } else {
                    println!(
                        "Encoded {} frame(s) to {}",
                        frames_written,
                        output.display()
                    );
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                println!("Encoding is not implemented for this platform yet.");
            }
        }
        cli::Commands::Start {
            session_url,
            session_id,
            passcode,
            window_id,
            handshake_id,
            payload_file,
            fixture_url,
        } => {
            let session_identifier = session_id
                .or_else(|| session_url.as_deref().and_then(extract_session_id))
                .ok_or_else(|| anyhow!("provide either --session-id or --session-url"))?;

            let material =
                security::SessionMaterial::derive(&session_identifier, &passcode)?;

            let handshake = match handshake_id {
                Some(id) => security::HandshakeId::from_base64(&id)?,
                None => security::HandshakeId::random(),
            };

            let payload = if let Some(ref path) = payload_file {
                let data = fs::read(path)
                    .with_context(|| format!("failed to read payload file {}", path.display()))?;
                println!(
                    "Loaded payload file ({} bytes): {}",
                    data.len(),
                    path.display()
                );
                data
            } else {
                let mut probe_payload = [0u8; 32];
                OsRng.fill_bytes(&mut probe_payload);
                probe_payload.to_vec()
            };

            let envelope =
                security::seal_signaling_payload(&material, &handshake, &payload)?;

            println!("Session ID     : {}", session_identifier);
            if let Some(url) = session_url {
                println!("Session URL    : {}", url);
            }
            println!("Window target  : {}", window_id.unwrap_or_else(|| "interactive".into()));
            println!("Handshake ID   : {}", handshake.to_base64());
            println!("Passcode SHA256: {}", material.passcode_fingerprint());
            println!(
                "Sealed envelope: {}",
                envelope.compact_encoding()
            );
            println!(
                "Derived preview: {}",
                hex::encode(material.preview_signaling_key(&handshake)?)
            );
            if payload_file.is_none() {
                println!("Note: payload not provided, emitted random probe for testing.");
            }
            if let Some(url) = fixture_url {
                println!("Posting envelope to fixture {}", url);
                fixture::client::post_envelope(
                    &url,
                    fixture::client::FixtureEnvelope {
                        session_id: &session_identifier,
                        handshake_b64: &handshake.to_base64(),
                        envelope: &envelope.compact_encoding(),
                    },
                )
                .context("failed to post envelope to fixture")?;
                println!("Fixture accepted envelope.");
            }
            println!();
            println!("Next steps:");
            println!("  - Implement capture adapters per OS (see docs/beach-cabana/phase0.md).");
            println!("  - Replace probe sealing with real offer/answer payloads.");
            println!("  - Use `beach-cabana fixture-serve` with `--fixture-url` to rehearse sealed signaling end-to-end.");
        }
        cli::Commands::SealProbe {
            session_id,
            passcode,
            payload,
            handshake_id,
        } => {
            let material = security::SessionMaterial::derive(&session_id, &passcode)?;
            let handshake = match handshake_id {
                Some(id) => security::HandshakeId::from_base64(&id)?,
                None => security::HandshakeId::random(),
            };
            let envelope =
                security::seal_signaling_payload(&material, &handshake, payload.as_bytes())?;
            println!("{}", envelope.compact_encoding());
        }
        cli::Commands::OpenProbe {
            session_id,
            passcode,
            envelope,
        } => {
            let material = security::SessionMaterial::derive(&session_id, &passcode)?;
            let envelope = security::SealedEnvelope::from_compact(&envelope)?;
            let plaintext = security::open_signaling_payload(&material, &envelope)?;
            println!("{}", String::from_utf8_lossy(&plaintext));
        }
        cli::Commands::FixtureServe {
            listen,
            storage_dir,
        } => {
            fixture::server::serve(listen, storage_dir)?;
        }
        cli::Commands::NoiseDiag {
            session_id,
            passcode,
            handshake_id,
            host_id,
            viewer_id,
            prologue,
        } => {
            let material = security::SessionMaterial::derive(&session_id, &passcode)?;
            let handshake = match handshake_id {
                Some(value) => security::HandshakeId::from_base64(&value)?,
                None => security::HandshakeId::random(),
            };
            let context_bytes = prologue.into_bytes();

            let mut host_handshake = noise::NoiseHandshake::new(noise::HandshakeConfig {
                material: &material,
                handshake_id: &handshake,
                role: noise::HandshakeRole::Initiator,
                local_id: &host_id,
                remote_id: &viewer_id,
                prologue_context: &context_bytes,
            })?;
            let mut viewer_handshake = noise::NoiseHandshake::new(noise::HandshakeConfig {
                material: &material,
                handshake_id: &handshake,
                role: noise::HandshakeRole::Responder,
                local_id: &viewer_id,
                remote_id: &host_id,
                prologue_context: &context_bytes,
            })?;

            // Exchange the three Noise XXpsk2 handshake messages locally.
            let msg1 = host_handshake.write_message(&[])?;
            viewer_handshake.read_message(&msg1)?;
            let msg2 = viewer_handshake.write_message(&[])?;
            host_handshake.read_message(&msg2)?;
            let msg3 = host_handshake.write_message(&[])?;
            viewer_handshake.read_message(&msg3)?;

            if !(host_handshake.is_finished() && viewer_handshake.is_finished()) {
                return Err(anyhow!("noise handshake did not complete"));
            }

            let host_session = host_handshake.finalize()?;
            let viewer_session = viewer_handshake.finalize()?;

            println!("Noise handshake diagnostic");
            println!("  Session ID     : {}", session_id);
            println!("  Handshake ID   : {}", handshake.to_base64());
            println!("  Verification   : {}", host_session.verification_code());
            println!("  Host send key  : {}", hex::encode(host_session.keys.send_key));
            println!("  Host recv key  : {}", hex::encode(host_session.keys.recv_key));
            println!(
                "  Viewer send key: {}",
                hex::encode(viewer_session.keys.send_key)
            );
            println!(
                "  Viewer recv key: {}",
                hex::encode(viewer_session.keys.recv_key)
            );
            println!(
                "  Handshake hash : {}",
                hex::encode(&host_session.handshake_hash)
            );
            if host_session.handshake_hash != viewer_session.handshake_hash {
                println!("  Warning: handshake hash mismatch between peers!");
            }

            let mut encryptor =
                host_session.keys.encryptor(&host_session.handshake_hash);
            let mut decryptor =
                viewer_session.keys.decryptor(&viewer_session.handshake_hash);

            let demo_frame = encryptor.seal(b"cabana-demo-frame")?;
            let demo_plaintext = decryptor.open(&demo_frame)?;
            println!(
                "  Demo nonce     : {}",
                hex::encode(demo_frame.nonce)
            );
            println!(
                "  Demo ciphertext: {}",
                hex::encode(&demo_frame.ciphertext)
            );
            println!(
                "  Demo plaintext : {}",
                String::from_utf8_lossy(&demo_plaintext)
            );
        }
    }

    Ok(())
}

fn extract_session_id(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/');
    trimmed.rsplit('/').next().map(|segment| segment.to_string())
}

#[cfg(target_os = "macos")]
fn adjust_frame_size(frame: &mut capture::Frame, max_width: Option<u32>) -> anyhow::Result<()> {
    if frame.width == 0 || frame.height == 0 {
        return Ok(());
    }

    if let capture::PixelFormat::Bgra8888 = frame.pixel_format {
        for chunk in frame.data.chunks_mut(4) {
            chunk.swap(0, 2);
        }
        frame.pixel_format = capture::PixelFormat::Rgba8888;
    }

    if let Some(max_width) = max_width {
        if max_width > 0 && frame.width > max_width {
            let scale = max_width as f32 / frame.width as f32;
            let new_height = ((frame.height as f32 * scale).round() as u32).max(1);
            let Some(buffer) = image::ImageBuffer::<image::Rgba<u8>, _>::from_vec(
                frame.width,
                frame.height,
                frame.data.clone(),
            ) else {
                anyhow::bail!("failed to create image buffer for resize");
            };
            let resized = image::DynamicImage::ImageRgba8(buffer)
                .resize(max_width, new_height, image::imageops::FilterType::Lanczos3)
                .to_rgba8();
            frame.width = resized.width();
            frame.height = resized.height();
            frame.data = resized.into_raw();
        }
    }

    Ok(())
}
