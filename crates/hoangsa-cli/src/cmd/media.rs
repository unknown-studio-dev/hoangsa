use crate::helpers::out;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::process::Command;
use image::imageops::FilterType;
use image::{Rgba, RgbaImage};
use imageproc::drawing::draw_filled_rect_mut;
use imageproc::rect::Rect;
use walkdir::WalkDir;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
pub struct VideoInfo {
    pub path: String,
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub codec: String,
    pub format: String,
}

#[derive(Serialize, Deserialize)]
pub struct FrameExtractResult {
    pub video: String,
    pub frames_dir: String,
    pub frame_count: u32,
    pub interval_secs: f64,
}

#[derive(Serialize, Deserialize)]
pub struct MontageResult {
    pub montage_path: String,
    pub diff_montage_path: String,
    pub grid_cols: u32,
    pub grid_rows: u32,
    pub frame_count: u32,
    pub metadata: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
pub struct FfmpegStatus {
    pub available: bool,
    pub path: String,
    pub version: String,
    pub source: String,
}

// ── Frame interval calculation ────────────────────────────────────────────────

pub fn calculate_frame_interval(duration_secs: f64) -> (f64, u32) {
    let (interval, max_frames) = if duration_secs <= 5.0 {
        (0.25, 20u32)
    } else if duration_secs <= 30.0 {
        (0.5, 40u32)
    } else {
        (1.0, 40u32)
    };
    let frame_count = ((duration_secs / interval).floor() as u32).min(max_frames);
    (interval, frame_count)
}

// ── Format detection ─────────────────────────────────────────────────────────

#[cfg(test)]
const VIDEO_FORMATS: &[&str] = &["mp4", "mov", "webm", "avi", "mkv"];
#[cfg(test)]
const IMAGE_FORMATS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif"];

#[cfg(test)]
pub fn detect_media_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    if VIDEO_FORMATS.contains(&ext.as_str()) {
        "video"
    } else if IMAGE_FORMATS.contains(&ext.as_str()) {
        "image"
    } else {
        "unknown"
    }
}

// ── Grid layout ───────────────────────────────────────────────────────────────

pub fn calculate_grid(frame_count: u32) -> (u32, u32) {
    let cols = if frame_count <= 12 {
        4
    } else {
        5
    };
    let rows = frame_count.div_ceil(cols);
    (cols, rows)
}

// ── FFmpeg detection ──────────────────────────────────────────────────────────

pub fn cmd_check_ffmpeg() {
    // Try system ffmpeg
    let which_cmd = if cfg!(target_os = "windows") {
        "where"
    } else {
        "which"
    };

    let system_result = Command::new(which_cmd)
        .arg("ffmpeg")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    if let Ok(output) = system_result {
        if output.status.success() {
            let ffmpeg_path = String::from_utf8_lossy(&output.stdout).trim().to_string();

            // Get version
            let version_str = Command::new("ffmpeg")
                .arg("-version")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .ok()
                .and_then(|o| {
                    String::from_utf8(o.stdout).ok().and_then(|s| {
                        s.lines()
                            .next()
                            .map(|l| l.trim().to_string())
                    })
                })
                .unwrap_or_default();

            out(&json!({
                "available": true,
                "path": ffmpeg_path,
                "version": version_str,
                "source": "system"
            }));
            return;
        }
    }

    // Try npm ffmpeg-static
    let npm_result = Command::new("node")
        .args(["-e", "console.log(require('ffmpeg-static'))"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    if let Ok(output) = npm_result {
        if output.status.success() {
            let ffmpeg_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !ffmpeg_path.is_empty() && ffmpeg_path != "null" {
                out(&json!({
                    "available": true,
                    "path": ffmpeg_path,
                    "version": "",
                    "source": "ffmpeg-static"
                }));
                return;
            }
        }
    }

    out(&json!({
        "available": false,
        "path": "",
        "version": "",
        "source": ""
    }));
}

// ── FFmpeg install ────────────────────────────────────────────────────────────

pub fn cmd_install_ffmpeg() {
    // Try npm install -g ffmpeg-static if node is available
    let node_check = Command::new("node")
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .status();

    if node_check.map(|s| s.success()).unwrap_or(false) {
        let result = Command::new("npm")
            .args(["install", "-g", "ffmpeg-static"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .status();

        match result {
            Ok(s) if s.success() => {
                out(&json!({ "success": true, "method": "npm", "package": "ffmpeg-static" }));
            }
            _ => {
                out(&json!({ "success": false, "error": "npm install -g ffmpeg-static failed" }));
            }
        }
        return;
    }

    // Print platform-specific instructions
    if cfg!(target_os = "macos") {
        out(&json!({
            "success": false,
            "instructions": "Install ffmpeg via Homebrew: brew install ffmpeg"
        }));
    } else if cfg!(target_os = "windows") {
        out(&json!({
            "success": false,
            "instructions": "Install ffmpeg via Chocolatey: choco install ffmpeg"
        }));
    } else {
        out(&json!({
            "success": false,
            "instructions": "Install ffmpeg via apt: sudo apt install ffmpeg"
        }));
    }
}

// ── Video probe ───────────────────────────────────────────────────────────────

pub fn cmd_probe(path: &str) {
    let output = Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
            path,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            out(&json!({ "error": format!("ffprobe failed: {}", e) }));
            return;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        out(&json!({ "error": format!("ffprobe error: {}", stderr.trim()) }));
        return;
    }

    let probe: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(e) => {
            out(&json!({ "error": format!("Failed to parse ffprobe output: {}", e) }));
            return;
        }
    };

    // Extract video stream
    let streams = probe["streams"].as_array();
    let video_stream = streams
        .and_then(|arr| arr.iter().find(|s| s["codec_type"].as_str() == Some("video")));

    let width = video_stream
        .and_then(|s| s["width"].as_u64())
        .unwrap_or(0) as u32;
    let height = video_stream
        .and_then(|s| s["height"].as_u64())
        .unwrap_or(0) as u32;
    let codec = video_stream
        .and_then(|s| s["codec_name"].as_str())
        .unwrap_or("")
        .to_string();

    // fps from r_frame_rate e.g. "30/1"
    let fps = video_stream
        .and_then(|s| s["r_frame_rate"].as_str())
        .map(|r| {
            let parts: Vec<&str> = r.split('/').collect();
            if parts.len() == 2 {
                let num: f64 = parts[0].parse().unwrap_or(0.0);
                let den: f64 = parts[1].parse().unwrap_or(1.0);
                if den != 0.0 { num / den } else { 0.0 }
            } else {
                r.parse().unwrap_or(0.0)
            }
        })
        .unwrap_or(0.0);

    let duration_secs = probe["format"]["duration"]
        .as_str()
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(0.0);

    let format = probe["format"]["format_name"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let info = VideoInfo {
        path: path.to_string(),
        duration_secs,
        width,
        height,
        fps,
        codec,
        format,
    };

    out(&serde_json::to_value(&info).unwrap_or(json!({ "error": "serialization failed" })));
}

// ── Frame extraction ──────────────────────────────────────────────────────────

pub fn cmd_frames(args: &[String]) {
    // Parse args: <video_path> [--interval <f>] [--max-frames <n>] [--output-dir <dir>]
    if args.is_empty() {
        out(&json!({ "error": "video path required" }));
        return;
    }

    let video_path = &args[0];
    let mut interval_arg: Option<f64> = None;
    let mut max_frames_arg: Option<u32> = None;
    let mut output_dir = String::from("frames");

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--interval" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    interval_arg = v.parse().ok();
                }
            }
            "--max-frames" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    max_frames_arg = v.parse().ok();
                }
            }
            "--output-dir" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    output_dir = v.clone();
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Get duration via ffprobe to calculate interval if not provided
    let duration_secs = if interval_arg.is_none() {
        let probe_out = Command::new("ffprobe")
            .args([
                "-v", "quiet",
                "-print_format", "json",
                "-show_format",
                video_path.as_str(),
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output();

        probe_out
            .ok()
            .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
            .and_then(|v| v["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok()))
            .unwrap_or(30.0)
    } else {
        0.0
    };

    let (interval, mut frame_count) = if let Some(iv) = interval_arg {
        (iv, max_frames_arg.unwrap_or(40))
    } else {
        let (iv, fc) = calculate_frame_interval(duration_secs);
        (iv, max_frames_arg.unwrap_or(fc))
    };

    // Create output dir
    if let Err(e) = std::fs::create_dir_all(&output_dir) {
        out(&json!({ "error": format!("Cannot create output dir: {}", e) }));
        return;
    }

    let fps_filter = format!("fps=1/{interval}");
    let frame_pattern = format!("{output_dir}/frame_%04d.png");

    let result = Command::new("ffmpeg")
        .args([
            "-i", video_path.as_str(),
            "-vf", fps_filter.as_str(),
            frame_pattern.as_str(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .status();

    match result {
        Ok(s) if s.success() => {
            // Count actual frames written
            let actual_count = std::fs::read_dir(&output_dir)
                .ok()
                .map(|rd| rd.filter_map(|e| e.ok()).filter(|e| {
                    e.path().extension().and_then(|x| x.to_str()) == Some("png")
                }).count() as u32)
                .unwrap_or(frame_count);
            frame_count = actual_count;

            out(&json!({
                "video": video_path,
                "frames_dir": output_dir,
                "frame_count": frame_count,
                "interval_secs": interval
            }));
        }
        Ok(_) => {
            out(&json!({ "error": "ffmpeg exited with non-zero status" }));
        }
        Err(e) => {
            out(&json!({ "error": format!("ffmpeg failed: {}", e) }));
        }
    }
}

// ── Montage creation ──────────────────────────────────────────────────────────

pub fn cmd_montage(args: &[&str]) {
    // Parse args: <frames_dir> [--cols <n>] [--timestamps] [--output <path>]
    if args.is_empty() {
        out(&json!({ "error": "frames_dir path required" }));
        return;
    }

    let frames_dir = args[0];
    let mut cols_arg: Option<u32> = None;
    let mut timestamps = false;
    let mut output_path: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i] {
            "--cols" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    cols_arg = v.parse().ok();
                }
            }
            "--timestamps" => {
                timestamps = true;
            }
            "--output" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    output_path = Some(v.to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Collect and sort .png frames from frames_dir
    let mut frame_paths: Vec<std::path::PathBuf> = WalkDir::new(frames_dir)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.into_path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("png"))
                    .unwrap_or(false)
        })
        .collect();

    frame_paths.sort();

    if frame_paths.is_empty() {
        out(&json!({ "error": "no PNG frames found in frames_dir" }));
        return;
    }

    let frame_count = frame_paths.len() as u32;

    // Determine grid dimensions
    let (cols, rows) = if let Some(c) = cols_arg {
        let r = frame_count.div_ceil(c);
        (c, r)
    } else {
        calculate_grid(frame_count)
    };

    // Cell dimensions: 320px wide, maintain aspect ratio from first frame
    const CELL_W: u32 = 320;
    const TIMESTAMP_BAR_H: u32 = 20;

    // Load first frame to determine aspect ratio
    let first_img = match image::open(&frame_paths[0]) {
        Ok(img) => img,
        Err(e) => {
            out(&json!({ "error": format!("failed to open first frame: {}", e) }));
            return;
        }
    };

    let orig_w = first_img.width();
    let orig_h = first_img.height();
    let cell_h = if orig_w > 0 {
        (CELL_W as f64 * orig_h as f64 / orig_w as f64).round() as u32
    } else {
        180u32
    };

    let canvas_w = cols * CELL_W;
    let canvas_h = rows * (cell_h + if timestamps { TIMESTAMP_BAR_H } else { 0 });

    // Create white background canvas
    let mut canvas = RgbaImage::from_pixel(canvas_w, canvas_h, Rgba([255u8, 255, 255, 255]));

    // Try to load a font for timestamp text (optional, falls back to black bar only)
    let font_data: Option<Vec<u8>> = load_system_font();
    let ab_font: Option<ab_glyph::FontVec> = font_data.and_then(|data| {
        ab_glyph::FontVec::try_from_vec(data).ok()
    });

    // Composite each frame into the grid
    for (idx, frame_path) in frame_paths.iter().enumerate() {
        let idx = idx as u32;
        let col = idx % cols;
        let row = idx / cols;

        let cell_row_h = cell_h + if timestamps { TIMESTAMP_BAR_H } else { 0 };
        let x_off = col * CELL_W;
        let y_off = row * cell_row_h;

        // Load and resize frame
        let frame_img = match image::open(frame_path) {
            Ok(img) => img,
            Err(_) => continue,
        };

        let resized_rgba = image::imageops::resize(&frame_img, CELL_W, cell_h, FilterType::Lanczos3);

        // Overlay frame onto canvas
        image::imageops::overlay(&mut canvas, &resized_rgba, x_off as i64, y_off as i64);

        // Draw timestamp bar if requested
        if timestamps {
            let bar_y = (y_off + cell_h) as i32;
            let bar_rect = Rect::at(x_off as i32, bar_y).of_size(CELL_W, TIMESTAMP_BAR_H);

            // Semi-transparent black bar (solid dark for simplicity on RGBA)
            draw_filled_rect_mut(&mut canvas, bar_rect, Rgba([0u8, 0, 0, 180]));

            // Draw timestamp text if font is available
            // Timestamp: extract from filename like frame_0042.png → index * interval
            // We don't know interval here, so use frame index as a counter (0.5s steps assumed)
            // Or try to parse from filename stem digits
            let ts_secs = extract_timestamp_from_path(frame_path, idx);
            let ts_str = format!("{ts_secs:.2}s");

            if let Some(ref font) = ab_font {
                let scale = ab_glyph::PxScale { x: 12.0, y: 12.0 };
                let text_x = x_off as i32 + 4;
                let text_y = bar_y + 3;
                imageproc::drawing::draw_text_mut(
                    &mut canvas,
                    Rgba([255u8, 255, 255, 255]),
                    text_x,
                    text_y,
                    scale,
                    font,
                    &ts_str,
                );
            }
        }
    }

    // Determine output path
    let out_path = output_path.unwrap_or_else(|| {
        let p = std::path::Path::new(frames_dir);
        p.join("montage.png").to_string_lossy().to_string()
    });

    // Save PNG
    if let Err(e) = canvas.save(&out_path) {
        out(&json!({ "error": format!("failed to save montage: {}", e) }));
        return;
    }

    let result = MontageResult {
        montage_path: out_path.clone(),
        diff_montage_path: String::new(),
        grid_cols: cols,
        grid_rows: rows,
        frame_count,
        metadata: json!({
            "cell_width": CELL_W,
            "cell_height": cell_h,
            "timestamps": timestamps,
        }),
    };

    out(&serde_json::to_value(&result).unwrap_or(json!({ "error": "serialization failed" })));
}

/// Attempt to load a system TTF/OTF font for timestamp rendering.
/// Returns None if no font is found — timestamps will be bar-only.
fn load_system_font() -> Option<Vec<u8>> {
    let candidates: &[&str] = &[
        // macOS
        "/System/Library/Fonts/Monaco.ttf",
        "/System/Library/Fonts/Supplemental/Courier New.ttf",
        // Linux
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        // Windows
        "C:\\Windows\\Fonts\\cour.ttf",
        "C:\\Windows\\Fonts\\arial.ttf",
    ];

    for path in candidates {
        if let Ok(data) = std::fs::read(path) {
            return Some(data);
        }
    }
    None
}

// ── Diff montage creation ─────────────────────────────────────────────────────

pub fn cmd_diff(args: &[&str]) {
    // Parse args: <frames_dir> [--cols <n>] [--output <path>]
    if args.is_empty() {
        out(&json!({ "error": "frames_dir path required" }));
        return;
    }

    let frames_dir = args[0];
    let mut cols_arg: Option<u32> = None;
    let mut output_path: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i] {
            "--cols" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    cols_arg = v.parse().ok();
                }
            }
            "--output" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    output_path = Some(v.to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Collect and sort .png frames from frames_dir
    let mut frame_paths: Vec<std::path::PathBuf> = WalkDir::new(frames_dir)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.into_path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("png"))
                    .unwrap_or(false)
        })
        .collect();

    frame_paths.sort();

    if frame_paths.is_empty() {
        out(&json!({ "error": "no PNG frames found in frames_dir" }));
        return;
    }

    let frame_count = frame_paths.len() as u32;

    // Determine grid dimensions
    let (cols, rows) = if let Some(c) = cols_arg {
        let r = frame_count.div_ceil(c);
        (c, r)
    } else {
        calculate_grid(frame_count)
    };

    // Cell dimensions: 320px wide, maintain aspect ratio from first frame
    const CELL_W: u32 = 320;

    // Load first frame to determine aspect ratio
    let first_img = match image::open(&frame_paths[0]) {
        Ok(img) => img,
        Err(e) => {
            out(&json!({ "error": format!("failed to open first frame: {}", e) }));
            return;
        }
    };

    let orig_w = first_img.width();
    let orig_h = first_img.height();
    let cell_h = if orig_w > 0 {
        (CELL_W as f64 * orig_h as f64 / orig_w as f64).round() as u32
    } else {
        180u32
    };

    let canvas_w = cols * CELL_W;
    let canvas_h = rows * cell_h;

    // Create white background canvas
    let mut canvas = RgbaImage::from_pixel(canvas_w, canvas_h, Rgba([255u8, 255, 255, 255]));

    // Load all frames as RGBA images resized to cell dimensions
    let mut frames_rgba: Vec<RgbaImage> = Vec::with_capacity(frame_paths.len());
    for frame_path in &frame_paths {
        let img = match image::open(frame_path) {
            Ok(img) => img,
            Err(_) => {
                // Use a blank gray image as placeholder
                frames_rgba.push(RgbaImage::from_pixel(CELL_W, cell_h, Rgba([128u8, 128, 128, 255])));
                continue;
            }
        };
        let rgba = image::imageops::resize(&img, CELL_W, cell_h, FilterType::Lanczos3);
        frames_rgba.push(rgba);
    }

    // Build diff overlay images
    let mut diff_overlays: Vec<RgbaImage> = Vec::with_capacity(frames_rgba.len());

    for idx in 0..frames_rgba.len() {
        if idx + 1 < frames_rgba.len() {
            // Compute pixel diff between frame[idx] and frame[idx+1]
            let a = &frames_rgba[idx];
            let b = &frames_rgba[idx + 1];

            // Ensure same dimensions (both were resized to CELL_W x cell_h, so they match)
            let w = a.width().min(b.width());
            let h = a.height().min(b.height());

            let mut overlay = RgbaImage::new(CELL_W, cell_h);

            for y in 0..h {
                for x in 0..w {
                    let pa = a.get_pixel(x, y);
                    let pb = b.get_pixel(x, y);

                    let r1 = pa[0];
                    let g1 = pa[1];
                    let b1 = pa[2];

                    let r2 = pb[0];
                    let g2 = pb[1];
                    let b2 = pb[2];

                    let diff = (r1.abs_diff(r2) as u16
                        + g1.abs_diff(g2) as u16
                        + b1.abs_diff(b2) as u16) as u32;

                    let pixel = if diff > 30 {
                        // Changed: red tint overlay
                        let r = ((r1 as f32 * 0.6) + (255.0 * 0.4)) as u8;
                        let g = (g1 as f32 * 0.6) as u8;
                        let b = (b1 as f32 * 0.6) as u8;
                        Rgba([r, g, b, 255])
                    } else {
                        // Unchanged: dim
                        let r = (r1 as f32 * 0.7) as u8;
                        let g = (g1 as f32 * 0.7) as u8;
                        let b = (b1 as f32 * 0.7) as u8;
                        Rgba([r, g, b, 255])
                    };

                    overlay.put_pixel(x, y, pixel);
                }
            }

            // Fill any out-of-bounds area (if dimensions differed) with gray
            for y in 0..cell_h {
                for x in 0..CELL_W {
                    if x >= w || y >= h {
                        overlay.put_pixel(x, y, Rgba([100u8, 100, 100, 255]));
                    }
                }
            }

            diff_overlays.push(overlay);
        } else {
            // Last frame — no next frame to diff against; show fully dimmed/grayed
            let a = &frames_rgba[idx];
            let mut dimmed = RgbaImage::new(CELL_W, cell_h);
            for y in 0..cell_h {
                for x in 0..CELL_W {
                    if x < a.width() && y < a.height() {
                        let pa = a.get_pixel(x, y);
                        let r = (pa[0] as f32 * 0.5) as u8;
                        let g = (pa[1] as f32 * 0.5) as u8;
                        let b = (pa[2] as f32 * 0.5) as u8;
                        dimmed.put_pixel(x, y, Rgba([r, g, b, 255]));
                    } else {
                        dimmed.put_pixel(x, y, Rgba([80u8, 80, 80, 255]));
                    }
                }
            }
            diff_overlays.push(dimmed);
        }
    }

    // Composite each diff overlay into the grid canvas
    for (idx, overlay) in diff_overlays.iter().enumerate() {
        let idx = idx as u32;
        let col = idx % cols;
        let row = idx / cols;

        let x_off = col * CELL_W;
        let y_off = row * cell_h;

        image::imageops::overlay(&mut canvas, overlay, x_off as i64, y_off as i64);
    }

    // Determine output path
    let out_path = output_path.unwrap_or_else(|| {
        let p = std::path::Path::new(frames_dir);
        p.join("diff-montage.png").to_string_lossy().to_string()
    });

    // Save PNG
    if let Err(e) = canvas.save(&out_path) {
        out(&json!({ "error": format!("failed to save diff montage: {}", e) }));
        return;
    }

    let result = MontageResult {
        montage_path: String::new(),
        diff_montage_path: out_path.clone(),
        grid_cols: cols,
        grid_rows: rows,
        frame_count,
        metadata: json!({
            "cell_width": CELL_W,
            "cell_height": cell_h,
            "diff_threshold": 30,
        }),
    };

    out(&serde_json::to_value(&result).unwrap_or(json!({ "error": "serialization failed" })));
}

/// Compute a diff overlay between two RGBA images.
/// Returns a new RgbaImage where changed pixels are red-tinted and unchanged pixels are dimmed.
#[allow(dead_code)]
pub(crate) fn compute_diff_overlay(a: &RgbaImage, b: &RgbaImage) -> RgbaImage {
    let w = a.width().min(b.width());
    let h = a.height().min(b.height());
    let out_w = a.width().max(b.width());
    let out_h = a.height().max(b.height());

    let mut overlay = RgbaImage::from_pixel(out_w, out_h, Rgba([100u8, 100, 100, 255]));

    for y in 0..h {
        for x in 0..w {
            let pa = a.get_pixel(x, y);
            let pb = b.get_pixel(x, y);

            let r1 = pa[0];
            let g1 = pa[1];
            let b1 = pa[2];

            let r2 = pb[0];
            let g2 = pb[1];
            let b2 = pb[2];

            let diff = (r1.abs_diff(r2) as u16
                + g1.abs_diff(g2) as u16
                + b1.abs_diff(b2) as u16) as u32;

            let pixel = if diff > 30 {
                // Changed: red tint overlay
                let r = ((r1 as f32 * 0.6) + (255.0 * 0.4)) as u8;
                let g = (g1 as f32 * 0.6) as u8;
                let b = (b1 as f32 * 0.6) as u8;
                Rgba([r, g, b, 255])
            } else {
                // Unchanged: dim
                let r = (r1 as f32 * 0.7) as u8;
                let g = (g1 as f32 * 0.7) as u8;
                let b = (b1 as f32 * 0.7) as u8;
                Rgba([r, g, b, 255])
            };

            overlay.put_pixel(x, y, pixel);
        }
    }

    overlay
}

/// Extract a timestamp in seconds from a frame path.
/// Tries to parse the trailing digits from the filename stem.
/// Falls back to frame_index as a whole number.
fn extract_timestamp_from_path(path: &std::path::Path, frame_index: u32) -> f64 {
    // Try to parse digits from stem like "frame_0042" → 42
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    let digits: String = stem.chars().rev().take_while(|c| c.is_ascii_digit()).collect();
    let digits: String = digits.chars().rev().collect();

    if let Ok(n) = digits.parse::<u32>() {
        // Assume 0.5s intervals (common default), frame n → n * 0.5s
        // Frame numbers are 1-based (frame_0001 = 0.5s)
        (n as f64) * 0.5
    } else {
        frame_index as f64
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    // ── Frame interval tests ──────────────────────────────────────────────────

    #[test]
    fn test_frame_interval_short() {
        let (interval, _frame_count) = calculate_frame_interval(3.0);
        assert_eq!(interval, 0.25);
        // max_frames for duration <= 5.0 is 20
        let (_iv, fc) = calculate_frame_interval(3.0);
        assert!(fc <= 20);
    }

    #[test]
    fn test_frame_interval_medium() {
        let (interval, frame_count) = calculate_frame_interval(15.0);
        assert_eq!(interval, 0.5);
        assert!(frame_count <= 40);
    }

    #[test]
    fn test_frame_interval_long() {
        let (interval, frame_count) = calculate_frame_interval(120.0);
        assert_eq!(interval, 1.0);
        assert!(frame_count <= 40);
    }

    #[test]
    fn test_frame_interval_cap() {
        let (_interval, frame_count) = calculate_frame_interval(600.0);
        assert!(frame_count <= 40);
    }

    // ── Grid layout tests ────────────────────────────────────────────────────

    #[test]
    fn test_grid_layout_small() {
        let (cols, rows) = calculate_grid(8);
        assert_eq!(cols, 4);
        assert_eq!(rows, 2);
    }

    #[test]
    fn test_grid_layout_medium() {
        let (cols, rows) = calculate_grid(20);
        assert_eq!(cols, 5);
        assert_eq!(rows, 4);
    }

    #[test]
    fn test_grid_layout_max() {
        let (cols, rows) = calculate_grid(40);
        assert_eq!(cols, 5);
        assert_eq!(rows, 8);
    }

    // ── Format detection tests ───────────────────────────────────────────────

    #[test]
    fn test_format_detection_video() {
        for ext in &["mp4", "mov", "webm", "avi", "mkv"] {
            let path = format!("test_file.{}", ext);
            assert_eq!(
                detect_media_type(&path),
                "video",
                "Expected 'video' for extension '{}'",
                ext
            );
        }
    }

    #[test]
    fn test_format_detection_image() {
        for ext in &["png", "jpg", "jpeg", "webp", "gif"] {
            let path = format!("test_file.{}", ext);
            assert_eq!(
                detect_media_type(&path),
                "image",
                "Expected 'image' for extension '{}'",
                ext
            );
        }
    }

    // ── Diff computation tests ───────────────────────────────────────────────

    #[test]
    fn test_diff_no_change() {
        // Two identical white 10x10 images → no red pixels (all pixels dimmed, not red-tinted)
        let white = RgbaImage::from_pixel(10, 10, Rgba([255u8, 255, 255, 255]));
        let result = compute_diff_overlay(&white, &white);

        // All pixels should be dimmed (unchanged path), not red-tinted.
        // Unchanged path: r = (255 * 0.7) = 178, g = 178, b = 178
        // A "red pixel" would have significantly higher R than G and B.
        for pixel in result.pixels() {
            let r = pixel[0] as i16;
            let g = pixel[1] as i16;
            // Red tint: R is boosted, G and B are dampened → R >> G
            assert!(
                (r - g).abs() < 50,
                "Unexpected red-tinted pixel in identical-image diff: {:?}",
                pixel
            );
        }
    }

    #[test]
    fn test_diff_full_change() {
        // White vs black 10x10 images → all pixels should have red tint
        let white = RgbaImage::from_pixel(10, 10, Rgba([255u8, 255, 255, 255]));
        let black = RgbaImage::from_pixel(10, 10, Rgba([0u8, 0, 0, 255]));
        let result = compute_diff_overlay(&white, &black);

        // Changed pixels: r = (255*0.6 + 255*0.4) = 255, g = 255*0.6 = 153, b = 153
        // All pixels differ by 255+255+255 = 765 > 30 threshold
        for pixel in result.pixels() {
            let r = pixel[0] as i16;
            let g = pixel[1] as i16;
            // Red channel should be notably higher than green in the red-tinted output
            assert!(
                r > g,
                "Expected red tint for changed pixel, got: {:?}",
                pixel
            );
        }
    }

    #[test]
    fn test_diff_partial_change() {
        // White image with one red pixel changed → only that pixel area shows diff
        let mut img_a = RgbaImage::from_pixel(10, 10, Rgba([255u8, 255, 255, 255]));
        let img_b = RgbaImage::from_pixel(10, 10, Rgba([255u8, 255, 255, 255]));

        // Make pixel (5, 5) fully red in img_a → diff at that location vs white in img_b
        img_a.put_pixel(5, 5, Rgba([255u8, 0, 0, 255]));

        let result = compute_diff_overlay(&img_a, &img_b);

        // Pixel (5,5): diff = |255-255| + |0-255| + |0-255| = 510 > 30 → red tint
        let changed = result.get_pixel(5, 5);
        // Unchanged pixel (0,0): diff = 0 → dimmed
        let unchanged = result.get_pixel(0, 0);

        // changed pixel: R is boosted above G
        assert!(
            changed[0] as i16 > changed[1] as i16,
            "Expected red tint at changed pixel (5,5), got: {:?}",
            changed
        );
        // unchanged pixel: R roughly equals G (both dimmed equally from white)
        assert!(
            (unchanged[0] as i16 - unchanged[1] as i16).abs() < 10,
            "Expected neutral dimmed pixel at (0,0), got: {:?}",
            unchanged
        );
    }

    // ── Montage creation test ────────────────────────────────────────────────

    #[test]
    fn test_montage_creation() {
        use std::env::temp_dir;

        // Create a unique temp directory for this test
        let temp_base = temp_dir();
        let test_dir = temp_base.join(format!("hoangsa_montage_test_{}", std::process::id()));
        std::fs::create_dir_all(&test_dir).expect("failed to create temp dir");

        // Create 4 solid-color 50x50 test images
        let colors = [
            Rgba([255u8, 0, 0, 255]),   // red
            Rgba([0u8, 255, 0, 255]),   // green
            Rgba([0u8, 0, 255, 255]),   // blue
            Rgba([255u8, 255, 0, 255]), // yellow
        ];

        for (i, color) in colors.iter().enumerate() {
            let img = RgbaImage::from_pixel(50, 50, *color);
            let path = test_dir.join(format!("frame_{:04}.png", i + 1));
            img.save(&path).expect("failed to save test frame");
        }

        // Run montage via cmd_montage
        let frames_dir_str = test_dir.to_string_lossy().to_string();
        let output_path = test_dir.join("montage_out.png");
        let output_str = output_path.to_string_lossy().to_string();

        cmd_montage(&[
            frames_dir_str.as_str(),
            "--output",
            output_str.as_str(),
        ]);

        // Verify the montage file was created
        assert!(output_path.exists(), "montage output file was not created");

        // Verify output dimensions: 4 frames → cols=4, rows=1, cell_w=320
        // cell_h = round(320 * 50/50) = 320
        // canvas_w = 4 * 320 = 1280, canvas_h = 1 * 320 = 320
        let montage_img = image::open(&output_path).expect("failed to open montage output");
        let expected_w = 4 * 320;
        let expected_h = 1 * 320;
        assert_eq!(
            montage_img.width(),
            expected_w,
            "montage width mismatch: expected {}, got {}",
            expected_w,
            montage_img.width()
        );
        assert_eq!(
            montage_img.height(),
            expected_h,
            "montage height mismatch: expected {}, got {}",
            expected_h,
            montage_img.height()
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&test_dir);
    }
}
