//! Drive every advertised format as source and target. Prints JSONL to stdout.

use std::path::{Path, PathBuf};
use std::time::Instant;

use convert::{Format, Intent, Job, PathTaken, convert, ffmpeg_available, plan, probe};
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};

fn main() {
    let root = std::env::temp_dir().join("ng-convert-matrix");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    println!(
        "{{\"kind\":\"meta\",\"ffmpeg\":{},\"avifenc\":{},\"sips\":{}}}",
        ffmpeg_available(),
        which("avifenc"),
        std::path::Path::new("/usr/bin/sips").exists()
    );

    let images = make_image_sources(&root);
    let audio = make_audio_sources(&root);
    let fonts = make_font_sources(&root);

    for (fmt, path) in &images {
        match probe(path) {
            Ok(p) => row(
                "probe",
                "ok",
                *fmt,
                *fmt,
                Intent::High,
                PathTaken::Copy,
                p.bytes,
                p.bytes,
                0.0,
                "",
            ),
            Err(e) => row(
                "probe",
                "fail",
                *fmt,
                *fmt,
                Intent::High,
                PathTaken::Copy,
                0,
                0,
                0.0,
                &e.to_string(),
            ),
        }
    }
    for (fmt, path) in &audio {
        match probe(path) {
            Ok(p) => row(
                "probe",
                "ok",
                *fmt,
                *fmt,
                Intent::High,
                PathTaken::Copy,
                p.bytes,
                p.bytes,
                0.0,
                p.audio_codec.as_deref().unwrap_or(""),
            ),
            Err(e) => row(
                "probe",
                "fail",
                *fmt,
                *fmt,
                Intent::High,
                PathTaken::Copy,
                0,
                0,
                0.0,
                &e.to_string(),
            ),
        }
    }
    for (fmt, path) in &fonts {
        match probe(path) {
            Ok(p) => row(
                "probe",
                "ok",
                *fmt,
                *fmt,
                Intent::High,
                PathTaken::Copy,
                p.bytes,
                p.bytes,
                0.0,
                match p.outline {
                    Some(convert::Outline::Cff) => "cff",
                    Some(convert::Outline::TrueType) => "glyf",
                    None => "",
                },
            ),
            Err(e) => row(
                "probe",
                "fail",
                *fmt,
                *fmt,
                Intent::High,
                PathTaken::Copy,
                0,
                0,
                0.0,
                &e.to_string(),
            ),
        }
    }

    let intents = [Intent::Archive, Intent::High, Intent::Web];
    for (src_fmt, src) in &images {
        for target in convert::relevant_targets(*src_fmt) {
            for intent in intents {
                run_one(&root, "image", src, *src_fmt, *target, intent);
            }
        }
    }
    if ffmpeg_available() {
        for (src_fmt, src) in &audio {
            for target in convert::relevant_targets(*src_fmt) {
                for intent in intents {
                    run_one(&root, "audio", src, *src_fmt, *target, intent);
                }
            }
        }
    } else {
        row(
            "audio",
            "skip",
            Format::Wav,
            Format::Flac,
            Intent::Archive,
            PathTaken::Encode,
            0,
            0,
            0.0,
            "ffmpeg missing",
        );
    }
    for (src_fmt, src) in &fonts {
        for target in convert::relevant_targets(*src_fmt) {
            for intent in intents {
                run_one(&root, "font", src, *src_fmt, *target, intent);
            }
        }
    }

    quality_checks(&root, &images, &audio, &fonts);
    benches(&root, &images, &audio, &fonts);
}

fn run_one(root: &Path, kind: &str, src: &Path, src_fmt: Format, target: Format, intent: Intent) {
    let dest = root.join(format!(
        "out-{}-{}-{}-{}.{}",
        src_fmt.label(),
        target.label(),
        intent.label().replace(' ', "_"),
        kind,
        target.ext()
    ));
    let job = Job {
        source: src.to_path_buf(),
        dest: dest.clone(),
        target,
        intent,
        max_edge: None,
        keep_metadata: true,
    };
    let t0 = Instant::now();
    match convert(&job) {
        Ok(report) => {
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            row(
                kind,
                "ok",
                src_fmt,
                target,
                intent,
                report.path,
                report.bytes_in,
                report.bytes_out,
                ms,
                &report.notes,
            );
        }
        Err(e) => {
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            row(
                kind,
                "fail",
                src_fmt,
                target,
                intent,
                PathTaken::Encode,
                0,
                0,
                ms,
                &e.to_string(),
            );
        }
    }
}

fn quality_checks(
    root: &Path,
    images: &[(Format, PathBuf)],
    audio: &[(Format, PathBuf)],
    fonts: &[(Format, PathBuf)],
) {
    if let Some((_, png)) = images.iter().find(|(f, _)| *f == Format::Png) {
        let dest = root.join("qa-png-webp.webp");
        let report = convert(&Job {
            source: png.clone(),
            dest: dest.clone(),
            target: Format::Webp,
            intent: Intent::Archive,
            max_edge: None,
            keep_metadata: true,
        });
        match report {
            Ok(r) => {
                let same = match (image::open(png), image::open(&dest)) {
                    (Ok(a), Ok(b)) => a.to_rgba8().as_raw() == b.to_rgba8().as_raw(),
                    _ => false,
                };
                row(
                    "quality",
                    if same { "ok" } else { "fail" },
                    Format::Png,
                    Format::Webp,
                    Intent::Archive,
                    r.path,
                    r.bytes_in,
                    r.bytes_out,
                    0.0,
                    if same {
                        "pixels identical"
                    } else {
                        "pixels differ"
                    },
                );
            }
            Err(e) => row(
                "quality",
                "fail",
                Format::Png,
                Format::Webp,
                Intent::Archive,
                PathTaken::Encode,
                0,
                0,
                0.0,
                &e.to_string(),
            ),
        }

        if let Some((_, jpeg)) = images.iter().find(|(f, _)| *f == Format::Jpeg) {
            let probed = probe(jpeg).unwrap();
            let planned = plan(&probed, Format::Jpeg, Intent::Archive).unwrap();
            row(
                "quality",
                if planned.path == PathTaken::Copy {
                    "ok"
                } else {
                    "fail"
                },
                Format::Jpeg,
                Format::Jpeg,
                Intent::Archive,
                planned.path,
                probed.bytes,
                probed.bytes,
                0.0,
                &planned.notes,
            );
        }
    }

    if ffmpeg_available() {
        if let Some((_, wav)) = audio.iter().find(|(f, _)| *f == Format::Wav) {
            let dest = root.join("qa-wav.flac");
            let report = convert(&Job {
                source: wav.clone(),
                dest: dest.clone(),
                target: Format::Flac,
                intent: Intent::Archive,
                max_edge: None,
                keep_metadata: true,
            });
            match report {
                Ok(r) => {
                    let same = pcm_eq(wav, &dest);
                    row(
                        "quality",
                        if same { "ok" } else { "fail" },
                        Format::Wav,
                        Format::Flac,
                        Intent::Archive,
                        r.path,
                        r.bytes_in,
                        r.bytes_out,
                        0.0,
                        if same { "pcm identical" } else { "pcm differs" },
                    );
                }
                Err(e) => row(
                    "quality",
                    "fail",
                    Format::Wav,
                    Format::Flac,
                    Intent::Archive,
                    PathTaken::Encode,
                    0,
                    0,
                    0.0,
                    &e.to_string(),
                ),
            }
        }
        if let Some((_, m4a)) = audio.iter().find(|(f, _)| *f == Format::M4a) {
            let dest = root.join("qa-m4a.mp4");
            let report = convert(&Job {
                source: m4a.clone(),
                dest,
                target: Format::Mp4,
                intent: Intent::Archive,
                max_edge: None,
                keep_metadata: true,
            });
            match report {
                Ok(r) => row(
                    "quality",
                    if r.path == PathTaken::Remux {
                        "ok"
                    } else {
                        "fail"
                    },
                    Format::M4a,
                    Format::Mp4,
                    Intent::Archive,
                    r.path,
                    r.bytes_in,
                    r.bytes_out,
                    0.0,
                    "want remux",
                ),
                Err(e) => row(
                    "quality",
                    "fail",
                    Format::M4a,
                    Format::Mp4,
                    Intent::Archive,
                    PathTaken::Encode,
                    0,
                    0,
                    0.0,
                    &e.to_string(),
                ),
            }
        }
    }

    if let Some((_, ttf)) = fonts.iter().find(|(f, _)| *f == Format::Ttf) {
        let dest = root.join("qa-font.woff2");
        match convert(&Job {
            source: ttf.clone(),
            dest: dest.clone(),
            target: Format::Woff2,
            intent: Intent::High,
            max_edge: None,
            keep_metadata: true,
        }) {
            Ok(r) => {
                let smaller = r.bytes_out > 0 && r.bytes_out < r.bytes_in;
                row(
                    "quality",
                    if smaller && r.path == PathTaken::Remux {
                        "ok"
                    } else {
                        "fail"
                    },
                    Format::Ttf,
                    Format::Woff2,
                    Intent::High,
                    r.path,
                    r.bytes_in,
                    r.bytes_out,
                    0.0,
                    if smaller {
                        "woff2 smaller"
                    } else {
                        "woff2 not smaller"
                    },
                );
            }
            Err(e) => row(
                "quality",
                "fail",
                Format::Ttf,
                Format::Woff2,
                Intent::High,
                PathTaken::Remux,
                0,
                0,
                0.0,
                &e.to_string(),
            ),
        }
    }
}

fn benches(
    root: &Path,
    images: &[(Format, PathBuf)],
    audio: &[(Format, PathBuf)],
    fonts: &[(Format, PathBuf)],
) {
    let png = write_png(&root.join("bench.png"), 1280, 720);
    for target in convert::relevant_targets(Format::Png) {
        median_runs("bench-image", &png, Format::Png, *target, Intent::High, 3);
    }
    if let Some((_, jpeg)) = images.iter().find(|(f, _)| *f == Format::Jpeg) {
        for intent in [Intent::VisuallyLossless, Intent::High, Intent::Web] {
            median_runs("bench-intent", jpeg, Format::Jpeg, Format::Jpeg, intent, 3);
        }
    }
    if ffmpeg_available() {
        let wav = write_wav(&root.join("bench.wav"), 44100 * 2);
        for target in convert::relevant_targets(Format::Wav) {
            median_runs("bench-audio", &wav, Format::Wav, *target, Intent::High, 3);
        }
        if let Some((_, m4a)) = audio.iter().find(|(f, _)| *f == Format::M4a) {
            median_runs(
                "bench-remux",
                m4a,
                Format::M4a,
                Format::Mp4,
                Intent::Archive,
                3,
            );
        }
    }
    if let Some((_, ttf)) = fonts.iter().find(|(f, _)| *f == Format::Ttf) {
        for target in convert::relevant_targets(Format::Ttf) {
            median_runs("bench-font", ttf, Format::Ttf, *target, Intent::High, 3);
        }
    }
}

fn median_runs(kind: &str, src: &Path, src_fmt: Format, target: Format, intent: Intent, n: usize) {
    let mut times = Vec::new();
    let mut last_ok = None;
    let mut last_err = None;
    for i in 0..n {
        let dest = src.with_file_name(format!(
            "bench-{}-{}-{i}.{}",
            src_fmt.label(),
            target.label(),
            target.ext()
        ));
        let job = Job {
            source: src.to_path_buf(),
            dest,
            target,
            intent,
            max_edge: None,
            keep_metadata: true,
        };
        let t0 = Instant::now();
        match convert(&job) {
            Ok(r) => {
                times.push(t0.elapsed().as_secs_f64() * 1000.0);
                last_ok = Some(r);
            }
            Err(e) => {
                last_err = Some(e.to_string());
                times.push(t0.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = times[times.len() / 2];
    match (last_ok, last_err) {
        (Some(r), _) => row(
            kind,
            "ok",
            src_fmt,
            target,
            intent,
            r.path,
            r.bytes_in,
            r.bytes_out,
            median,
            &format!("median_of_{n}"),
        ),
        (None, Some(e)) => row(
            kind,
            "fail",
            src_fmt,
            target,
            intent,
            PathTaken::Encode,
            0,
            0,
            median,
            &e,
        ),
        _ => {}
    }
}

fn make_image_sources(root: &Path) -> Vec<(Format, PathBuf)> {
    let png = write_png(&root.join("src.png"), 256, 192);
    let mut out = vec![(Format::Png, png.clone())];
    for (fmt, name) in [
        (Format::Jpeg, "src.jpg"),
        (Format::Webp, "src.webp"),
        (Format::Tiff, "src.tiff"),
        (Format::Gif, "src.gif"),
        (Format::Heic, "src.heic"),
        (Format::Avif, "src.avif"),
    ] {
        let dest = root.join(name);
        match convert(&Job {
            source: png.clone(),
            dest: dest.clone(),
            target: fmt,
            intent: Intent::High,
            max_edge: None,
            keep_metadata: true,
        }) {
            Ok(_) => out.push((fmt, dest)),
            Err(e) => row(
                "source",
                "skip",
                Format::Png,
                fmt,
                Intent::High,
                PathTaken::Encode,
                0,
                0,
                0.0,
                &e.to_string(),
            ),
        }
    }
    out
}

fn make_audio_sources(root: &Path) -> Vec<(Format, PathBuf)> {
    if !ffmpeg_available() {
        return Vec::new();
    }
    let wav = write_wav(&root.join("src.wav"), 44100);
    let mut out = vec![(Format::Wav, wav.clone())];
    for (fmt, name) in [
        (Format::Flac, "src.flac"),
        (Format::Alac, "src-alac.m4a"),
        (Format::Aac, "src-aac.m4a"),
        (Format::M4a, "src.m4a"),
        (Format::Mp4, "src.mp4"),
        (Format::Mp3, "src.mp3"),
        (Format::Opus, "src.opus"),
    ] {
        let dest = root.join(name);
        match convert(&Job {
            source: wav.clone(),
            dest: dest.clone(),
            target: fmt,
            intent: Intent::High,
            max_edge: None,
            keep_metadata: true,
        }) {
            Ok(_) => out.push((fmt, dest)),
            Err(e) => row(
                "source",
                "skip",
                Format::Wav,
                fmt,
                Intent::High,
                PathTaken::Encode,
                0,
                0,
                0.0,
                &e.to_string(),
            ),
        }
    }
    out
}

fn make_font_sources(root: &Path) -> Vec<(Format, PathBuf)> {
    let Some(src) = [
        "/System/Library/Fonts/Geneva.ttf",
        "/System/Library/Fonts/Keyboard.ttf",
        "/Library/Fonts/Arial Unicode.ttf",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.is_file()) else {
        row(
            "source",
            "skip",
            Format::Ttf,
            Format::Ttf,
            Intent::High,
            PathTaken::Copy,
            0,
            0,
            0.0,
            "no system ttf",
        );
        return Vec::new();
    };
    let ttf = root.join("src.ttf");
    if std::fs::copy(&src, &ttf).is_err() {
        return Vec::new();
    }
    let mut out = vec![(Format::Ttf, ttf.clone())];
    for (fmt, name) in [
        (Format::Otf, "src.otf"),
        (Format::Woff, "src.woff"),
        (Format::Woff2, "src.woff2"),
    ] {
        let dest = root.join(name);
        match convert(&Job {
            source: ttf.clone(),
            dest: dest.clone(),
            target: fmt,
            intent: Intent::High,
            max_edge: None,
            keep_metadata: true,
        }) {
            Ok(_) => out.push((fmt, dest)),
            Err(e) => row(
                "source",
                "skip",
                Format::Ttf,
                fmt,
                Intent::High,
                PathTaken::Remux,
                0,
                0,
                0.0,
                &e.to_string(),
            ),
        }
    }
    out
}

fn write_png(path: &Path, w: u32, h: u32) -> PathBuf {
    let mut img = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            img.put_pixel(
                x,
                y,
                Rgba([
                    (x * 255 / w.max(1)) as u8,
                    (y * 255 / h.max(1)) as u8,
                    ((x + y) % 256) as u8,
                    255,
                ]),
            );
        }
    }
    let file = std::fs::File::create(path).unwrap();
    PngEncoder::new(file)
        .write_image(img.as_raw(), w, h, ExtendedColorType::Rgba8)
        .unwrap();
    path.to_path_buf()
}

fn write_wav(path: &Path, samples: usize) -> PathBuf {
    let mut buf = Vec::new();
    let data_len = (samples * 2) as u32;
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
    for i in 0..samples {
        let t = i as f64 / 44100.0;
        let s = ((t * 440.0 * std::f64::consts::TAU).sin() * 12000.0) as i16;
        buf.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, buf).unwrap();
    path.to_path_buf()
}

fn pcm_eq(a: &Path, b: &Path) -> bool {
    fn decode(path: &Path) -> Option<Vec<u8>> {
        let out = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(path)
            .args([
                "-f",
                "s16le",
                "-acodec",
                "pcm_s16le",
                "-ac",
                "1",
                "-ar",
                "44100",
                "-",
            ])
            .output()
            .ok()?;
        out.status.success().then_some(out.stdout)
    }
    match (decode(a), decode(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn which(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
fn row(
    kind: &str,
    status: &str,
    src: Format,
    dst: Format,
    intent: Intent,
    path: PathTaken,
    bytes_in: u64,
    bytes_out: u64,
    ms: f64,
    notes: &str,
) {
    let notes = notes.replace('\\', "\\\\").replace('"', "\\\"");
    println!(
        "{{\"kind\":\"{kind}\",\"status\":\"{status}\",\"src\":\"{}\",\"dst\":\"{}\",\"intent\":\"{}\",\"path\":\"{}\",\"bytes_in\":{bytes_in},\"bytes_out\":{bytes_out},\"ms\":{ms:.3},\"notes\":\"{notes}\"}}",
        src.label(),
        dst.label(),
        intent.label(),
        path.label()
    );
}
