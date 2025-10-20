use anyhow::{anyhow, Result};
use std::time::{SystemTime, UNIX_EPOCH};

/// Utility for packaging Annex-B H.264 samples into fragmented MP4 segments (init + moof/mdat).
/// Designed for low-latency browser playback via MediaSource (single-sample fragments).
pub struct Fmp4Writer {
    width: u32,
    height: u32,
    timescale: u32,
    sample_duration: u32,
    sequence_number: u32,
    decode_time: u64,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    codec_string: Option<String>,
    init_emitted: bool,
}

pub struct SampleSegments {
    pub init_segment: Option<Vec<u8>>,
    pub media_segment: Vec<u8>,
}

impl Fmp4Writer {
    pub fn new(width: u32, height: u32, fps: u32) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(anyhow!("invalid dimensions"));
        }
        if fps == 0 {
            return Err(anyhow!("fps must be greater than zero"));
        }
        let timescale = 90_000u32;
        let sample_duration = timescale / fps.max(1);
        Ok(Self {
            width,
            height,
            timescale,
            sample_duration,
            sequence_number: 1,
            decode_time: 0,
            sps: None,
            pps: None,
            codec_string: None,
            init_emitted: false,
        })
    }

    /// Converts a raw Annex-B chunk into MP4 segments.
    pub fn push_annexb_sample(&mut self, chunk: &[u8]) -> Result<SampleSegments> {
        let mut nals = split_annex_b(chunk);
        if nals.is_empty() {
            return Err(anyhow!("empty annex-b chunk"));
        }

        let mut is_keyframe = false;
        let mut filtered_nals = Vec::with_capacity(nals.len());
        for nal in nals.drain(..) {
            if nal.is_empty() {
                continue;
            }
            let nal_type = nal[0] & 0x1F;
            match nal_type {
                5 => {
                    is_keyframe = true;
                    filtered_nals.push(nal);
                }
                7 => {
                    self.sps = Some(nal.to_vec());
                    self.codec_string = Some(codec_string_from_sps(nal));
                }
                8 => {
                    self.pps = Some(nal.to_vec());
                }
                _ => {
                    filtered_nals.push(nal);
                }
            }
        }

        if self.sps.is_none() || self.pps.is_none() {
            return Err(anyhow!("missing SPS/PPS before emitting samples"));
        }

        let mut segments = SampleSegments {
            init_segment: None,
            media_segment: Vec::new(),
        };

        if !self.init_emitted {
            let init = build_init_segment(
                self.width,
                self.height,
                self.timescale,
                self.sample_duration,
                self.sps.as_ref().unwrap(),
                self.pps.as_ref().unwrap(),
                self.codec_string.as_deref().unwrap_or("avc1.42E01E"),
            )?;
            segments.init_segment = Some(init);
            self.init_emitted = true;
        }

        let sample_bytes = build_length_prefixed_sample(&filtered_nals)?;
        let fragment = build_fragment(
            self.sequence_number,
            self.decode_time,
            self.sample_duration,
            &sample_bytes,
            is_keyframe,
        );
        self.sequence_number = self.sequence_number.wrapping_add(1);
        self.decode_time = self
            .decode_time
            .saturating_add(self.sample_duration as u64);
        segments.media_segment = fragment;
        Ok(segments)
    }
}

fn split_annex_b(chunk: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut i = 0usize;
    while i + 3 < chunk.len() {
        if chunk[i] == 0 && chunk[i + 1] == 0 && chunk[i + 2] == 1 {
            let start = i + 3;
            i = start;
            while i + 3 < chunk.len()
                && !(chunk[i] == 0 && chunk[i + 1] == 0 && (chunk[i + 2] == 1 || (chunk[i + 2] == 0 && chunk[i + 3] == 1)))
            {
                i += 1;
            }
            let end = if i + 3 < chunk.len() && chunk[i] == 0 && chunk[i + 1] == 0 && chunk[i + 2] == 0 && chunk[i + 3] == 1 {
                i
            } else if i + 2 < chunk.len() && chunk[i] == 0 && chunk[i + 1] == 0 && chunk[i + 2] == 1 {
                i
            } else {
                chunk.len()
            };
            nals.push(&chunk[start..end]);
        } else if i + 4 < chunk.len() && chunk[i] == 0 && chunk[i + 1] == 0 && chunk[i + 2] == 0 && chunk[i + 3] == 1 {
            let start = i + 4;
            i = start;
            while i + 4 < chunk.len()
                && !(chunk[i] == 0 && chunk[i + 1] == 0 && (chunk[i + 2] == 1 || (chunk[i + 2] == 0 && chunk[i + 3] == 1)))
            {
                i += 1;
            }
            let end = if i + 4 < chunk.len() && chunk[i] == 0 && chunk[i + 1] == 0 && chunk[i + 2] == 0 && chunk[i + 3] == 1 {
                i
            } else if i + 3 < chunk.len() && chunk[i] == 0 && chunk[i + 1] == 0 && chunk[i + 2] == 1 {
                i
            } else {
                chunk.len()
            };
            nals.push(&chunk[start..end]);
        } else {
            i += 1;
        }
    }
    nals
}

fn codec_string_from_sps(sps: &[u8]) -> String {
    if sps.len() < 4 {
        return "avc1.42E01E".into();
    }
    let profile = sps[1];
    let compat = sps[2];
    let level = sps[3];
    format!("avc1.{profile:02X}{compat:02X}{level:02X}")
}

fn build_init_segment(
    width: u32,
    height: u32,
    timescale: u32,
    sample_duration: u32,
    sps: &[u8],
    pps: &[u8],
    codec: &str,
) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    write_box(&mut buf, b"ftyp", |ftyp| {
        ftyp.extend_from_slice(b"isom");
        ftyp.extend_from_slice(&0x00000200u32.to_be_bytes());
        ftyp.extend_from_slice(b"isomiso6avc1mp41");
    });

    write_box(&mut buf, b"moov", |moov| {
        write_box(moov, b"mvhd", |mvhd| {
            mvhd.push(0); // version
            mvhd.extend_from_slice(&[0, 0, 0]); // flags
            mvhd.extend_from_slice(&0u32.to_be_bytes()); // creation time
            mvhd.extend_from_slice(&0u32.to_be_bytes()); // modification time
            mvhd.extend_from_slice(&timescale.to_be_bytes());
            mvhd.extend_from_slice(&0u32.to_be_bytes()); // duration
            mvhd.extend_from_slice(&0x00010000u32.to_be_bytes()); // rate 1.0
            mvhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
            mvhd.extend_from_slice(&0u16.to_be_bytes()); // reserved
            mvhd.extend_from_slice(&[0u8; 8]); // reserved
            mvhd.extend_from_slice(&IDENTITY_MATRIX);
            mvhd.extend_from_slice(&[0u8; 24]); // pre-defined
            mvhd.extend_from_slice(&1u32.to_be_bytes()); // next track id
        });

        write_box(moov, b"trak", |trak| {
            write_box(trak, b"tkhd", |tkhd| {
                tkhd.push(0); // version
                tkhd.extend_from_slice(&[0x00, 0x00, 0x07]); // flags (track enabled + in movie + in preview)
                tkhd.extend_from_slice(&0u32.to_be_bytes()); // creation time
                tkhd.extend_from_slice(&0u32.to_be_bytes()); // modification time
                tkhd.extend_from_slice(&1u32.to_be_bytes()); // track id
                tkhd.extend_from_slice(&0u32.to_be_bytes()); // reserved
                tkhd.extend_from_slice(&0u32.to_be_bytes()); // duration
                tkhd.extend_from_slice(&[0u8; 8]); // reserved
                tkhd.extend_from_slice(&0u16.to_be_bytes()); // layer
                tkhd.extend_from_slice(&0u16.to_be_bytes()); // alternate group
                tkhd.extend_from_slice(&0u16.to_be_bytes()); // volume
                tkhd.extend_from_slice(&0u16.to_be_bytes());
                tkhd.extend_from_slice(&IDENTITY_MATRIX);
                tkhd.extend_from_slice(&((width as u32) << 16).to_be_bytes());
                tkhd.extend_from_slice(&((height as u32) << 16).to_be_bytes());
            });

            write_box(trak, b"mdia", |mdia| {
                write_box(mdia, b"mdhd", |mdhd| {
                    mdhd.push(0); // version
                    mdhd.extend_from_slice(&[0, 0, 0]); // flags
                    mdhd.extend_from_slice(&0u32.to_be_bytes()); // creation time
                    mdhd.extend_from_slice(&0u32.to_be_bytes()); // modification time
                    mdhd.extend_from_slice(&timescale.to_be_bytes());
                    mdhd.extend_from_slice(&0u32.to_be_bytes()); // duration
                    mdhd.extend_from_slice(&0x55C4u16.to_be_bytes()); // language 'und'
                    mdhd.extend_from_slice(&0u16.to_be_bytes());
                });

                write_box(mdia, b"hdlr", |hdlr| {
                    hdlr.push(0); // version
                    hdlr.extend_from_slice(&[0, 0, 0]); // flags
                    hdlr.extend_from_slice(&0u32.to_be_bytes()); // pre_defined
                    hdlr.extend_from_slice(b"vide");
                    hdlr.extend_from_slice(&[0u8; 12]); // reserved
                    hdlr.extend_from_slice(b"VideoHandler\0");
                });

                write_box(mdia, b"minf", |minf| {
                    write_box(minf, b"vmhd", |vmhd| {
                        vmhd.push(0); // version
                        vmhd.extend_from_slice(&[0, 0, 1]); // flags (graphicsmode + opcolor)
                        vmhd.extend_from_slice(&[0u8; 8]);
                    });

                    write_box(minf, b"dinf", |dinf| {
                        write_box(dinf, b"dref", |dref| {
                            dref.push(0); // version
                            dref.extend_from_slice(&[0, 0, 0]); // flags
                            dref.extend_from_slice(&1u32.to_be_bytes()); // entry count
                            write_box(dref, b"url ", |url| {
                                url.push(0); // version
                                url.extend_from_slice(&[0, 0, 1]); // flags (self-contained)
                            });
                        });
                    });

                    write_box(minf, b"stbl", |stbl| {
                        write_box(stbl, b"stsd", |stsd| {
                            stsd.push(0); // version
                            stsd.extend_from_slice(&[0, 0, 0]); // flags
                            stsd.extend_from_slice(&1u32.to_be_bytes()); // entry count
                            write_box(stsd, b"avc1", |avc1| {
                                avc1.extend_from_slice(&[0u8; 6]); // reserved
                                avc1.extend_from_slice(&1u16.to_be_bytes()); // data reference index
                                avc1.extend_from_slice(&0u16.to_be_bytes()); // pre-defined
                                avc1.extend_from_slice(&0u16.to_be_bytes()); // reserved
                                avc1.extend_from_slice(&[0u8; 12]); // pre-defined[3]
                                avc1.extend_from_slice(&(width as u16).to_be_bytes());
                                avc1.extend_from_slice(&(height as u16).to_be_bytes());
                                avc1.extend_from_slice(&0x00480000u32.to_be_bytes()); // horizresolution 72 dpi
                                avc1.extend_from_slice(&0x00480000u32.to_be_bytes()); // vertresolution
                                avc1.extend_from_slice(&0u32.to_be_bytes()); // reserved
                                avc1.extend_from_slice(&1u16.to_be_bytes()); // frame count
                                let mut compressor = [0u8; 32];
                                let codec_bytes = codec.as_bytes();
                                let name_len = codec_bytes.len().min(31);
                                compressor[0] = name_len as u8;
                                compressor[1..=name_len].copy_from_slice(&codec_bytes[..name_len]);
                                avc1.extend_from_slice(&compressor);
                                avc1.extend_from_slice(&0x0018u16.to_be_bytes()); // depth
                                avc1.extend_from_slice(&0xFFFFu16.to_be_bytes()); // pre-defined
                                write_box(avc1, b"avcC", |avcc| {
                                    avcc.push(1); // configuration version
                                    avcc.push(sps.get(1).copied().unwrap_or(0x4D));
                                    avcc.push(sps.get(2).copied().unwrap_or(0));
                                    avcc.push(sps.get(3).copied().unwrap_or(0x1E));
                                    avcc.push(0xFF); // lengthSizeMinusOne = 3
                                    avcc.push(0xE1); // numOfSequenceParameterSets
                                    avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
                                    avcc.extend_from_slice(sps);
                                    avcc.push(1); // num of PPS
                                    avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
                                    avcc.extend_from_slice(pps);
                                });
                                write_box(avc1, b"btrt", |btrt| {
                                    btrt.extend_from_slice(&0u32.to_be_bytes()); // buffer size
                                    btrt.extend_from_slice(&0u32.to_be_bytes()); // max bitrate
                                    btrt.extend_from_slice(&0u32.to_be_bytes()); // avg bitrate
                                });
                            });
                        });

                        write_box(stbl, b"stts", |stts| {
                            stts.push(0);
                            stts.extend_from_slice(&[0, 0, 0]);
                            stts.extend_from_slice(&0u32.to_be_bytes());
                        });
                        write_box(stbl, b"stsc", |stsc| {
                            stsc.push(0);
                            stsc.extend_from_slice(&[0, 0, 0]);
                            stsc.extend_from_slice(&0u32.to_be_bytes());
                        });
                        write_box(stbl, b"stsz", |stsz| {
                            stsz.push(0);
                            stsz.extend_from_slice(&[0, 0, 0]);
                            stsz.extend_from_slice(&0u32.to_be_bytes()); // sample size
                            stsz.extend_from_slice(&0u32.to_be_bytes()); // sample count
                        });
                        write_box(stbl, b"stco", |stco| {
                            stco.push(0);
                            stco.extend_from_slice(&[0, 0, 0]);
                            stco.extend_from_slice(&0u32.to_be_bytes());
                        });
                    });
                });
            });
        });

        write_box(moov, b"mvex", |mvex| {
            write_box(mvex, b"trex", |trex| {
                trex.push(0);
                trex.extend_from_slice(&[0, 0, 0]);
                trex.extend_from_slice(&1u32.to_be_bytes()); // track_id
                trex.extend_from_slice(&1u32.to_be_bytes()); // default sample description index
                trex.extend_from_slice(&sample_duration.to_be_bytes());
                trex.extend_from_slice(&0u32.to_be_bytes()); // default sample size
                trex.extend_from_slice(&0u32.to_be_bytes()); // default sample flags
            });
        });
    });

    Ok(buf)
}

fn build_fragment(
    sequence_number: u32,
    decode_time: u64,
    sample_duration: u32,
    sample_data: &[u8],
    keyframe: bool,
) -> Vec<u8> {
    let mfhd_box = build_box_bytes(b"mfhd", |mfhd| {
        mfhd.push(0);
        mfhd.extend_from_slice(&[0, 0, 0]);
        mfhd.extend_from_slice(&sequence_number.to_be_bytes());
    });

    let tfhd_box = build_box_bytes(b"tfhd", |tfhd| {
        tfhd.push(0);
        tfhd.extend_from_slice(&[0x00, 0x00, 0x08]); // default-sample-duration present
        tfhd.extend_from_slice(&1u32.to_be_bytes()); // track id
        tfhd.extend_from_slice(&sample_duration.to_be_bytes());
    });

    let tfdt_box = build_box_bytes(b"tfdt", |tfdt| {
        tfdt.push(1); // version 1 (64-bit)
        tfdt.extend_from_slice(&[0, 0, 0]);
        tfdt.extend_from_slice(&decode_time.to_be_bytes());
    });

    let mut trun_payload = Vec::new();
    trun_payload.push(0); // version
    let flags: u32 = 0x000001 | 0x000100 | 0x000200 | 0x000400;
    trun_payload.extend_from_slice(&flags.to_be_bytes()[1..4]); // lower 24 bits
    trun_payload.extend_from_slice(&1u32.to_be_bytes()); // sample count
    let data_offset_pos = trun_payload.len();
    trun_payload.extend_from_slice(&0i32.to_be_bytes()); // placeholder
    trun_payload.extend_from_slice(&sample_duration.to_be_bytes());
    trun_payload.extend_from_slice(&(sample_data.len() as u32).to_be_bytes());
    let sample_flags: u32 = if keyframe { 0x0200_0000 } else { 0x0101_0000 };
    trun_payload.extend_from_slice(&sample_flags.to_be_bytes());

    let trun_box_len = 8 + trun_payload.len();
    let traf_content_len = tfhd_box.len() + tfdt_box.len() + trun_box_len;
    let traf_box_len = 8 + traf_content_len;
    let moof_size = 8 + mfhd_box.len() + traf_box_len;
    let data_offset = (moof_size + 8) as i32; // include upcoming mdat header
    trun_payload[data_offset_pos..data_offset_pos + 4].copy_from_slice(&data_offset.to_be_bytes());

    let trun_box = build_box_bytes_from_payload(b"trun", trun_payload);
    let traf_box = build_box_bytes(b"traf", |traf| {
        traf.extend_from_slice(&tfhd_box);
        traf.extend_from_slice(&tfdt_box);
        traf.extend_from_slice(&trun_box);
    });

    let moof = build_box_bytes(b"moof", |moof_body| {
        moof_body.extend_from_slice(&mfhd_box);
        moof_body.extend_from_slice(&traf_box);
    });

    let mut mdat = Vec::new();
    write_box(&mut mdat, b"mdat", |mdat_body| {
        mdat_body.extend_from_slice(sample_data);
    });

    let mut out = Vec::with_capacity(moof.len() + mdat.len());
    out.extend_from_slice(&moof);
    out.extend_from_slice(&mdat);
    out
}

fn build_length_prefixed_sample(nals: &[&[u8]]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for nal in nals {
        let len = u32::try_from(nal.len()).map_err(|_| anyhow!("nal too large"))?;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(nal);
    }
    Ok(out)
}

fn write_box<F>(buf: &mut Vec<u8>, kind: &[u8; 4], build: F)
where
    F: FnOnce(&mut Vec<u8>),
{
    let mut content = Vec::new();
    build(&mut content);
    let size = (content.len() + 8) as u32;
    buf.extend_from_slice(&size.to_be_bytes());
    buf.extend_from_slice(kind);
    buf.extend_from_slice(&content);
}

fn build_box_bytes<F>(kind: &[u8; 4], build: F) -> Vec<u8>
where
    F: FnOnce(&mut Vec<u8>),
{
    let mut out = Vec::new();
    write_box(&mut out, kind, build);
    out
}

fn build_box_bytes_from_payload(kind: &[u8; 4], payload: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(&payload);
    out
}

const IDENTITY_MATRIX: [u8; 36] = [
    0x00, 0x01, 0x00, 0x00, // a
    0x00, 0x00, 0x00, 0x00, // b
    0x00, 0x00, 0x00, 0x00, // u
    0x00, 0x00, 0x00, 0x00, // c
    0x00, 0x01, 0x00, 0x00, // d
    0x00, 0x00, 0x00, 0x00, // v
    0x00, 0x00, 0x00, 0x00, // x
    0x00, 0x00, 0x00, 0x00, // y
    0x40, 0x00, 0x00, 0x00, // w
];

#[allow(dead_code)]
fn now_seconds() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .try_into()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmp4_writer_emits_init_and_fragment() {
        let mut writer = Fmp4Writer::new(1280, 720, 30).expect("writer");
        let annexb = [
            0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xC0, 0x1E, 0x9A, 0x74, 0x05, 0x01, 0xE0, 0x08,
            0x9F, 0x97, 0x01, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x04, 0x00, 0x00,
            0x03, 0x00, 0xF1, 0x83, 0x19, 0x60, //
            0x00, 0x00, 0x00, 0x01, 0x68, 0xCE, 0x38, 0x80, //
            0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x80, 0x20, 0x11, 0x99, 0x88,
        ];
        let segments = writer.push_annexb_sample(&annexb).expect("segments");
        assert!(segments.init_segment.is_some());
        assert!(!segments.media_segment.is_empty());
        // second sample should not emit init segment
        let annexb2 = [
            0x00, 0x00, 0x00, 0x01, 0x06, 0x05, 0xFF, 0xFF, 0x10, //
            0x00, 0x00, 0x00, 0x01, 0x61, 0x9A, 0x20, 0x11, 0x11,
        ];
        let segments2 = writer.push_annexb_sample(&annexb2).expect("segments2");
        assert!(segments2.init_segment.is_none());
        assert!(!segments2.media_segment.is_empty());
    }
}
