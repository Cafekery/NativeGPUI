use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::time::Instant;

use convert::{
    Format, Intent, Job, Outline, PathTaken, Report, convert, ffmpeg_available, plan, probe,
    relevant_targets,
};
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};

type Sources = Vec<(Format, PathBuf)>;

fn main() {
    let root = std::env::temp_dir().join("ng-convert-matrix");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    println!(
        "{{\"kind\":\"meta\",\"ffmpeg\":{},\"avifenc\":{},\"sips\":{}}}",
        ffmpeg_available(),
        which("avifenc"),
        Path::new("/usr/bin/sips").exists()
    );

    let images = make_image_sources(&root);
    let audio = make_audio_sources(&root);
    let fonts = make_font_sources(&root);

    for (fmt, path) in images.iter().chain(&audio).chain(&fonts) {
        let row = Row::new("probe", *fmt, *fmt, Intent::High);
        match probe(path) {
            Ok(p) => {
                let outline = match p.outline {
                    Some(Outline::Cff) => "cff",
                    Some(Outline::TrueType) => "glyf",
                    None => "",
                };
                let notes = p.audio_codec.as_deref().unwrap_or(outline);
                row.print("ok", PathTaken::Copy, p.bytes, p.bytes, 0.0, notes);
            }
            Err(e) => row.error("fail", PathTaken::Copy, 0.0, e),
        }
    }

    run_all(&root, "image", &images);
    if ffmpeg_available() {
        run_all(&root, "audio", &audio);
    } else {
        Row::new("audio", Format::Wav, Format::Flac, Intent::Archive).print(
            "skip",
            PathTaken::Encode,
            0,
            0,
            0.0,
            "ffmpeg missing",
        );
    }
    run_all(&root, "font", &fonts);

    quality_checks(&root, &images, &audio, &fonts);
    benches(&root, &images, &audio, &fonts);
}

fn job(source: &Path, dest: PathBuf, target: Format, intent: Intent) -> Job {
    Job {
        source: source.to_path_buf(),
        dest,
        target,
        intent,
        max_edge: None,
        keep_metadata: true,
    }
}

fn find(sources: &[(Format, PathBuf)], format: Format) -> Option<&Path> {
    sources
        .iter()
        .find(|(f, _)| *f == format)
        .map(|(_, path)| path.as_path())
}

fn ms(t0: Instant) -> f64 {
    t0.elapsed().as_secs_f64() * 1000.0
}

fn verdict(ok: bool) -> &'static str {
    if ok { "ok" } else { "fail" }
}

fn run_all(root: &Path, kind: &str, sources: &[(Format, PathBuf)]) {
    for (src_fmt, src) in sources {
        for &target in relevant_targets(*src_fmt) {
            for intent in [Intent::Archive, Intent::High, Intent::Web] {
                run_one(root, kind, src, *src_fmt, target, intent);
            }
        }
    }
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
    let row = Row::new(kind, src_fmt, target, intent);
    let t0 = Instant::now();
    match convert(&job(src, dest, target, intent)) {
        Ok(r) => row.report("ok", &r, ms(t0), &r.notes),
        Err(e) => row.error("fail", PathTaken::Encode, ms(t0), e),
    }
}

fn quality_checks(
    root: &Path,
    images: &[(Format, PathBuf)],
    audio: &[(Format, PathBuf)],
    fonts: &[(Format, PathBuf)],
) {
    if let Some(png) = find(images, Format::Png) {
        let dest = root.join("qa-png-webp.webp");
        quality(
            Format::Png,
            png,
            dest,
            Format::Webp,
            Intent::Archive,
            PathTaken::Encode,
            |_, dest| {
                let same = match (image::open(png), image::open(dest)) {
                    (Ok(a), Ok(b)) => a.to_rgba8().as_raw() == b.to_rgba8().as_raw(),
                    _ => false,
                };
                (
                    same,
                    if same {
                        "pixels identical"
                    } else {
                        "pixels differ"
                    },
                )
            },
        );

        if let Some(jpeg) = find(images, Format::Jpeg) {
            let probed = probe(jpeg).unwrap();
            let planned = plan(&probed, Format::Jpeg, Intent::Archive).unwrap();
            Row::new("quality", Format::Jpeg, Format::Jpeg, Intent::Archive).print(
                verdict(planned.path == PathTaken::Copy),
                planned.path,
                probed.bytes,
                probed.bytes,
                0.0,
                &planned.notes,
            );
        }
    }

    if ffmpeg_available() {
        if let Some(wav) = find(audio, Format::Wav) {
            let dest = root.join("qa-wav.flac");
            quality(
                Format::Wav,
                wav,
                dest,
                Format::Flac,
                Intent::Archive,
                PathTaken::Encode,
                |_, dest| {
                    let same = pcm_eq(wav, dest);
                    (same, if same { "pcm identical" } else { "pcm differs" })
                },
            );
        }
        if let Some(m4a) = find(audio, Format::M4a) {
            let dest = root.join("qa-m4a.mp4");
            quality(
                Format::M4a,
                m4a,
                dest,
                Format::Mp4,
                Intent::Archive,
                PathTaken::Encode,
                |r, _| (r.path == PathTaken::Remux, "want remux"),
            );
        }
    }

    if let Some(ttf) = find(fonts, Format::Ttf) {
        let dest = root.join("qa-font.woff2");
        quality(
            Format::Ttf,
            ttf,
            dest,
            Format::Woff2,
            Intent::High,
            PathTaken::Remux,
            |r, _| {
                let smaller = r.bytes_out > 0 && r.bytes_out < r.bytes_in;
                let notes = if smaller {
                    "woff2 smaller"
                } else {
                    "woff2 not smaller"
                };
                (smaller && r.path == PathTaken::Remux, notes)
            },
        );
    }
}

fn quality(
    src_fmt: Format,
    src: &Path,
    dest: PathBuf,
    target: Format,
    intent: Intent,
    failed: PathTaken,
    judge: impl FnOnce(&Report, &Path) -> (bool, &'static str),
) {
    let row = Row::new("quality", src_fmt, target, intent);
    match convert(&job(src, dest.clone(), target, intent)) {
        Ok(r) => {
            let (ok, notes) = judge(&r, &dest);
            row.report(verdict(ok), &r, 0.0, notes);
        }
        Err(e) => row.error("fail", failed, 0.0, e),
    }
}

fn benches(
    root: &Path,
    images: &[(Format, PathBuf)],
    audio: &[(Format, PathBuf)],
    fonts: &[(Format, PathBuf)],
) {
    let png = write_png(&root.join("bench.png"), 1280, 720);
    for &target in relevant_targets(Format::Png) {
        median_runs("bench-image", &png, Format::Png, target, Intent::High, 3);
    }
    if let Some(jpeg) = find(images, Format::Jpeg) {
        for intent in [Intent::VisuallyLossless, Intent::High, Intent::Web] {
            median_runs("bench-intent", jpeg, Format::Jpeg, Format::Jpeg, intent, 3);
        }
    }
    if ffmpeg_available() {
        let wav = write_wav(&root.join("bench.wav"), 44100 * 2);
        for &target in relevant_targets(Format::Wav) {
            median_runs("bench-audio", &wav, Format::Wav, target, Intent::High, 3);
        }
        if let Some(m4a) = find(audio, Format::M4a) {
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
    if let Some(ttf) = find(fonts, Format::Ttf) {
        for &target in relevant_targets(Format::Ttf) {
            median_runs("bench-font", ttf, Format::Ttf, target, Intent::High, 3);
        }
    }
}

fn median_runs(kind: &str, src: &Path, src_fmt: Format, target: Format, intent: Intent, n: usize) {
    let mut times = Vec::with_capacity(n);
    let mut last_ok = None;
    let mut last_err = None;
    for i in 0..n {
        let dest = src.with_file_name(format!(
            "bench-{}-{}-{i}.{}",
            src_fmt.label(),
            target.label(),
            target.ext()
        ));
        let t0 = Instant::now();
        let result = convert(&job(src, dest, target, intent));
        times.push(ms(t0));
        match result {
            Ok(r) => last_ok = Some(r),
            Err(e) => last_err = Some(e),
        }
    }
    times.sort_by(f64::total_cmp);
    let median = times[times.len() / 2];
    let row = Row::new(kind, src_fmt, target, intent);
    match (last_ok, last_err) {
        (Some(r), _) => row.report("ok", &r, median, &format!("median_of_{n}")),
        (None, Some(e)) => row.error("fail", PathTaken::Encode, median, e),
        (None, None) => {}
    }
}

fn make_image_sources(root: &Path) -> Sources {
    let png = write_png(&root.join("src.png"), 256, 192);
    derive(
        root,
        Format::Png,
        png,
        &[
            (Format::Jpeg, "src.jpg"),
            (Format::Webp, "src.webp"),
            (Format::Tiff, "src.tiff"),
            (Format::Gif, "src.gif"),
            (Format::Heic, "src.heic"),
            (Format::Avif, "src.avif"),
        ],
        PathTaken::Encode,
    )
}

fn make_audio_sources(root: &Path) -> Sources {
    if !ffmpeg_available() {
        return Vec::new();
    }
    let wav = write_wav(&root.join("src.wav"), 44100);
    derive(
        root,
        Format::Wav,
        wav,
        &[
            (Format::Flac, "src.flac"),
            (Format::Alac, "src-alac.m4a"),
            (Format::Aac, "src-aac.m4a"),
            (Format::M4a, "src.m4a"),
            (Format::Mp4, "src.mp4"),
            (Format::Mp3, "src.mp3"),
            (Format::Opus, "src.opus"),
        ],
        PathTaken::Encode,
    )
}

fn make_font_sources(root: &Path) -> Sources {
    let Some(src) = [
        "/System/Library/Fonts/Geneva.ttf",
        "/System/Library/Fonts/Keyboard.ttf",
        "/Library/Fonts/Arial Unicode.ttf",
    ]
    .into_iter()
    .map(Path::new)
    .find(|p| p.is_file()) else {
        Row::new("source", Format::Ttf, Format::Ttf, Intent::High).print(
            "skip",
            PathTaken::Copy,
            0,
            0,
            0.0,
            "no system ttf",
        );
        return Vec::new();
    };
    let ttf = root.join("src.ttf");
    if std::fs::copy(src, &ttf).is_err() {
        return Vec::new();
    }
    derive(
        root,
        Format::Ttf,
        ttf,
        &[
            (Format::Otf, "src.otf"),
            (Format::Woff, "src.woff"),
            (Format::Woff2, "src.woff2"),
        ],
        PathTaken::Remux,
    )
}

fn derive(
    root: &Path,
    base_fmt: Format,
    base: PathBuf,
    targets: &[(Format, &str)],
    failed: PathTaken,
) -> Sources {
    let mut out = vec![(base_fmt, base.clone())];
    for &(fmt, name) in targets {
        let dest = root.join(name);
        match convert(&job(&base, dest.clone(), fmt, Intent::High)) {
            Ok(_) => out.push((fmt, dest)),
            Err(e) => Row::new("source", base_fmt, fmt, Intent::High).error("skip", failed, 0.0, e),
        }
    }
    out
}

fn write_png(path: &Path, w: u32, h: u32) -> PathBuf {
    let img = RgbaImage::from_fn(w, h, |x, y| {
        Rgba([
            (x * 255 / w.max(1)) as u8,
            (y * 255 / h.max(1)) as u8,
            ((x + y) % 256) as u8,
            255,
        ])
    });
    let file = std::fs::File::create(path).unwrap();
    PngEncoder::new(file)
        .write_image(img.as_raw(), w, h, ExtendedColorType::Rgba8)
        .unwrap();
    path.to_path_buf()
}

fn write_wav(path: &Path, samples: usize) -> PathBuf {
    let data_len = (samples * 2) as u32;
    let mut buf = Vec::with_capacity(44 + samples * 2);
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
    matches!((decode(a), decode(b)), (Some(x), Some(y)) if x == y)
}

fn which(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

struct Row<'a> {
    kind: &'a str,
    src: Format,
    dst: Format,
    intent: Intent,
}

impl<'a> Row<'a> {
    fn new(kind: &'a str, src: Format, dst: Format, intent: Intent) -> Self {
        Self {
            kind,
            src,
            dst,
            intent,
        }
    }

    fn report(&self, status: &str, r: &Report, ms: f64, notes: &str) {
        self.print(status, r.path, r.bytes_in, r.bytes_out, ms, notes);
    }

    fn error(&self, status: &str, path: PathTaken, ms: f64, e: impl Display) {
        self.print(status, path, 0, 0, ms, &e.to_string());
    }

    fn print(
        &self,
        status: &str,
        path: PathTaken,
        bytes_in: u64,
        bytes_out: u64,
        ms: f64,
        notes: &str,
    ) {
        let kind = self.kind;
        let notes = notes.replace('\\', "\\\\").replace('"', "\\\"");
        println!(
            "{{\"kind\":\"{kind}\",\"status\":\"{status}\",\"src\":\"{}\",\"dst\":\"{}\",\"intent\":\"{}\",\"path\":\"{}\",\"bytes_in\":{bytes_in},\"bytes_out\":{bytes_out},\"ms\":{ms:.3},\"notes\":\"{notes}\"}}",
            self.src.label(),
            self.dst.label(),
            self.intent.label(),
            path.label()
        );
    }
}
