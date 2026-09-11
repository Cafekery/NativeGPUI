use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};

use crate::plan::{Intent, PathTaken};
use crate::{Format, Job, convert, ffmpeg_available, plan, probe};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("ng-convert-tests");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

fn write_png(path: &Path, w: u32, h: u32) {
    let mut img = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            img.put_pixel(
                x,
                y,
                Rgba([
                    (x * 255 / w.max(1)) as u8,
                    (y * 255 / h.max(1)) as u8,
                    80,
                    255,
                ]),
            );
        }
    }
    let file = std::fs::File::create(path).unwrap();
    PngEncoder::new(file)
        .write_image(img.as_raw(), w, h, ExtendedColorType::Rgba8)
        .unwrap();
}

fn write_jpeg(path: &Path, w: u32, h: u32, quality: u8) {
    let mut rgb = vec![0u8; (w * h * 3) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 3) as usize;
            rgb[i] = (x * 255 / w.max(1)) as u8;
            rgb[i + 1] = (y * 255 / h.max(1)) as u8;
            rgb[i + 2] = 40;
        }
    }
    let file = std::fs::File::create(path).unwrap();
    let mut enc = JpegEncoder::new_with_quality(file, quality);
    enc.encode(&rgb, w, h, image::ColorType::Rgb8.into())
        .unwrap();
}

fn write_wav(path: &Path, samples: &[i16]) {
    let data_len = (samples.len() * 2) as u32;
    let mut buf = Vec::new();
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&44100u32.to_le_bytes());
    buf.extend_from_slice(&88200u32.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, buf).unwrap();
}

fn sine(n: usize) -> Vec<i16> {
    (0..n)
        .map(|i| {
            let t = i as f64 / 44100.0;
            (t * 440.0 * std::f64::consts::TAU).sin() * 16000.0
        })
        .map(|v| v as i16)
        .collect()
}

#[test]
fn png_to_webp_archive_is_pixel_identical() {
    let src = tmp("grad.png");
    let dest = tmp("grad.converted.webp");
    write_png(&src, 24, 16);
    let report = convert(&Job {
        source: src.clone(),
        dest: dest.clone(),
        target: Format::Webp,
        intent: Intent::Archive,
        max_edge: None,
        keep_metadata: true,
    })
    .unwrap();
    assert_eq!(report.path, PathTaken::Encode);
    let a = image::open(&src).unwrap().to_rgba8();
    let b = image::open(&dest).unwrap().to_rgba8();
    assert_eq!(a.dimensions(), b.dimensions());
    assert_eq!(a.as_raw(), b.as_raw());
    assert!(report.bytes_out > 0);
}

#[test]
fn jpeg_archive_same_format_copies() {
    let src = tmp("photo.jpg");
    let dest = tmp("photo.converted.jpg");
    write_jpeg(&src, 32, 24, 80);
    let probed = probe(&src).unwrap();
    let planned = plan(&probed, Format::Jpeg, Intent::Archive).unwrap();
    assert_eq!(planned.path, PathTaken::Copy);
    assert!(planned.notes.contains("lossy"));
    let report = convert(&Job {
        source: src.clone(),
        dest: dest.clone(),
        target: Format::Jpeg,
        intent: Intent::Archive,
        max_edge: None,
        keep_metadata: true,
    })
    .unwrap();
    assert_eq!(report.path, PathTaken::Copy);
    assert_eq!(std::fs::read(&src).unwrap(), std::fs::read(&dest).unwrap());
}

#[test]
fn jpeg_web_is_smaller_than_high() {
    let src = tmp("q.jpg");
    write_jpeg(&src, 64, 48, 95);
    let high = tmp("q.high.jpg");
    let web = tmp("q.web.jpg");
    let high_r = convert(&Job {
        source: src.clone(),
        dest: high,
        target: Format::Jpeg,
        intent: Intent::High,
        max_edge: None,
        keep_metadata: false,
    })
    .unwrap();
    let web_r = convert(&Job {
        source: src,
        dest: web,
        target: Format::Jpeg,
        intent: Intent::Web,
        max_edge: None,
        keep_metadata: false,
    })
    .unwrap();
    assert_eq!(high_r.path, PathTaken::Encode);
    assert_eq!(web_r.path, PathTaken::Encode);
    assert!(web_r.bytes_out < high_r.bytes_out);
}

#[test]
fn jpeg_cannot_be_planned_as_lossless_encode() {
    let src = tmp("no-lossless.jpg");
    write_jpeg(&src, 16, 16, 70);
    let probed = probe(&src).unwrap();
    let planned = plan(&probed, Format::Jpeg, Intent::Archive).unwrap();
    assert_eq!(planned.path, PathTaken::Copy);
    let to_png = plan(&probed, Format::Png, Intent::Archive).unwrap();
    assert_eq!(to_png.path, PathTaken::Encode);
    assert!(to_png.notes.contains("JPEG"));
}

#[test]
fn web_intent_resizes() {
    let src = tmp("wide.png");
    let dest = tmp("wide.web.webp");
    write_png(&src, 2400, 100);
    let report = convert(&Job {
        source: src,
        dest,
        target: Format::Webp,
        intent: Intent::Web,
        max_edge: None,
        keep_metadata: false,
    })
    .unwrap();
    assert_eq!(report.width_out, Some(1920));
    assert_eq!(report.height_out, Some(80));
}

#[test]
fn aac_m4a_to_mp4_is_remux() {
    let probed = crate::Probe {
        path: PathBuf::from("clip.m4a"),
        kind: crate::Kind::Audio,
        format: Format::M4a,
        bytes: 1000,
        width: None,
        height: None,
        duration: Some(1.0),
        audio_codec: Some("aac".into()),
        outline: None,
    };
    let planned = plan(&probed, Format::Mp4, Intent::Archive).unwrap();
    assert_eq!(planned.path, PathTaken::Remux);
    assert!(planned.notes.contains("copied") || planned.notes.contains("aac"));
}

#[test]
fn wav_to_flac_pcm_identical_if_ffmpeg() {
    if !ffmpeg_available() {
        return;
    }
    let src = tmp("tone.wav");
    let dest = tmp("tone.converted.flac");
    write_wav(&src, &sine(4410));
    let report = convert(&Job {
        source: src.clone(),
        dest: dest.clone(),
        target: Format::Flac,
        intent: Intent::Archive,
        max_edge: None,
        keep_metadata: true,
    })
    .unwrap();
    assert_eq!(report.path, PathTaken::Encode);
    let a = crate::audio::decode_pcm_s16(&src).unwrap();
    let b = crate::audio::decode_pcm_s16(&dest).unwrap();
    assert_eq!(a, b);
    if let (Some(d_in), Some(d_out)) = (report.duration_in, report.duration_out) {
        assert!((d_in - d_out).abs() < 0.05);
    }
}

#[test]
fn flac_to_mp3_keeps_duration_if_ffmpeg() {
    if !ffmpeg_available() {
        return;
    }
    let wav = tmp("song.wav");
    let flac = tmp("song.flac");
    let mp3 = tmp("song.converted.mp3");
    write_wav(&wav, &sine(8820));
    convert(&Job {
        source: wav,
        dest: flac.clone(),
        target: Format::Flac,
        intent: Intent::Archive,
        max_edge: None,
        keep_metadata: true,
    })
    .unwrap();
    let report = convert(&Job {
        source: flac,
        dest: mp3,
        target: Format::Mp3,
        intent: Intent::High,
        max_edge: None,
        keep_metadata: true,
    })
    .unwrap();
    assert_eq!(report.path, PathTaken::Encode);
    let d_in = report.duration_in.unwrap_or(0.0);
    let d_out = report.duration_out.unwrap_or(0.0);
    assert!((d_in - d_out).abs() < 0.15, "{d_in} vs {d_out}");
}

#[test]
fn remux_aac_m4a_to_mp4_if_ffmpeg() {
    if !ffmpeg_available() {
        return;
    }
    let wav = tmp("voice.wav");
    let m4a = tmp("voice.m4a");
    let mp4 = tmp("voice.converted.mp4");
    write_wav(&wav, &sine(4410));
    convert(&Job {
        source: wav,
        dest: m4a.clone(),
        target: Format::Aac,
        intent: Intent::High,
        max_edge: None,
        keep_metadata: true,
    })
    .unwrap();
    let probed = probe(&m4a).unwrap();
    assert_eq!(probed.audio_codec.as_deref(), Some("aac"));
    let planned = plan(&probed, Format::Mp4, Intent::Archive).unwrap();
    assert_eq!(planned.path, PathTaken::Remux);
    let report = convert(&Job {
        source: m4a,
        dest: mp4.clone(),
        target: Format::Mp4,
        intent: Intent::Archive,
        max_edge: None,
        keep_metadata: true,
    })
    .unwrap();
    assert_eq!(report.path, PathTaken::Remux);
    let again = probe(&mp4).unwrap();
    assert_eq!(again.audio_codec.as_deref(), Some("aac"));
}

fn sample_ttf() -> Option<PathBuf> {
    const CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/Geneva.ttf",
        "/System/Library/Fonts/Keyboard.ttf",
        "/Library/Fonts/Arial Unicode.ttf",
    ];
    CANDIDATES.iter().map(PathBuf::from).find(|p| p.is_file())
}

fn sample_cff_otf() -> Option<PathBuf> {
    const CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/LastResort.otf",
        "/Library/Fonts/NewYorkExtraLarge-Black.otf",
    ];
    CANDIDATES.iter().map(PathBuf::from).find(|p| {
        p.is_file()
            && probe(p)
                .ok()
                .is_some_and(|pr| pr.outline == Some(crate::Outline::Cff))
    })
}

#[test]
fn font_advice_says_which_is_better() {
    let ttf = Format::Ttf.advice().unwrap();
    assert!(ttf.contains("Mac") || ttf.contains("Word"));
    assert!(ttf.contains("website"));
    let otf = Format::Otf.advice().unwrap();
    assert!(otf.contains("design") || otf.contains("print"));
    let web = Format::Woff2.advice().unwrap();
    assert!(web.contains("website"));
    assert!(web.contains("Font Book") || web.contains("install"));
    let old = Format::Woff.advice().unwrap();
    assert!(old.contains("WOFF2"));
    assert!(Format::Webp.advice().is_none());
}

#[test]
fn ttf_to_woff2_is_smaller_and_round_trips() {
    let Some(src) = sample_ttf() else {
        return;
    };
    let dest = tmp("geneva.converted.woff2");
    let back = tmp("geneva.roundtrip.ttf");
    let report = convert(&Job {
        source: src.clone(),
        dest: dest.clone(),
        target: Format::Woff2,
        intent: Intent::High,
        max_edge: None,
        keep_metadata: true,
    })
    .unwrap();
    assert_eq!(report.path, PathTaken::Remux);
    assert!(report.bytes_out > 0);
    assert!(report.bytes_out < report.bytes_in);
    convert(&Job {
        source: dest,
        dest: back.clone(),
        target: Format::Ttf,
        intent: Intent::Archive,
        max_edge: None,
        keep_metadata: true,
    })
    .unwrap();
    let a = std::fs::read(&src).unwrap();
    let b = std::fs::read(&back).unwrap();
    assert_eq!(glyph_count(&a), glyph_count(&b));
    assert_eq!(
        crate::font::table_bytes(&a, b"name").unwrap(),
        crate::font::table_bytes(&b, b"name").unwrap()
    );
    let again = probe(&back).unwrap();
    assert_eq!(again.format, Format::Ttf);
    assert_eq!(again.outline, Some(crate::Outline::TrueType));
}

fn glyph_count(sfnt: &[u8]) -> u16 {
    let maxp = crate::font::table_bytes(sfnt, b"maxp").unwrap();
    u16::from_be_bytes([maxp[4], maxp[5]])
}

#[test]
fn ttf_to_otf_copies_bytes() {
    let Some(src) = sample_ttf() else {
        return;
    };
    let dest = tmp("geneva.converted.otf");
    let report = convert(&Job {
        source: src.clone(),
        dest: dest.clone(),
        target: Format::Otf,
        intent: Intent::High,
        max_edge: None,
        keep_metadata: true,
    })
    .unwrap();
    assert_eq!(report.path, PathTaken::Copy);
    assert_eq!(std::fs::read(&src).unwrap(), std::fs::read(&dest).unwrap());
}

#[test]
fn ttf_to_woff_round_trips() {
    let Some(src) = sample_ttf() else {
        return;
    };
    let dest = tmp("geneva.converted.woff");
    let back = tmp("geneva.woff.ttf");
    let report = convert(&Job {
        source: src.clone(),
        dest: dest.clone(),
        target: Format::Woff,
        intent: Intent::High,
        max_edge: None,
        keep_metadata: true,
    })
    .unwrap();
    assert_eq!(report.path, PathTaken::Remux);
    convert(&Job {
        source: dest,
        dest: back.clone(),
        target: Format::Ttf,
        intent: Intent::Archive,
        max_edge: None,
        keep_metadata: true,
    })
    .unwrap();
    let a = std::fs::read(&src).unwrap();
    let b = std::fs::read(&back).unwrap();
    let glyf_a = crate::font::table_bytes(&a, b"glyf").unwrap();
    let glyf_b = crate::font::table_bytes(&b, b"glyf").unwrap();
    assert_eq!(glyf_a, glyf_b);
}

#[test]
fn cff_otf_refuses_ttf() {
    let Some(src) = sample_cff_otf() else {
        return;
    };
    let probed = probe(&src).unwrap();
    assert!(plan(&probed, Format::Ttf, Intent::High).is_err());
    let dest = tmp("cff.converted.ttf");
    let err = convert(&Job {
        source: src,
        dest,
        target: Format::Ttf,
        intent: Intent::High,
        max_edge: None,
        keep_metadata: true,
    })
    .unwrap_err();
    assert!(err.to_string().contains("CFF") || err.to_string().contains("redraw"));
}

#[test]
fn ttc_is_refused() {
    let path = tmp("faces.ttc");
    std::fs::write(&path, b"ttcf").unwrap();
    let err = probe(&path).unwrap_err();
    assert!(err.to_string().contains("collections"));
}
