---
name: visual-debug
description: "Triggers when user provides screenshots, videos, screen recordings, or mentions visual bugs, UI glitches, animation issues. Processes media files using `hoangsa-cli media analyze` to create annotated montage grids with diff overlays for visual debugging."
---

# Visual Debugging

## When to Use

- User shares screenshots, screen recordings, or video files
- "There's a UI glitch in my app"
- "This animation looks wrong"
- "Something changed visually between versions"
- Investigating layout shifts, rendering bugs, or visual regressions

## Workflow

### 1. Detect Media Files

Identify media files in user input by extension:

- **Video**: `.mp4`, `.mov`, `.webm`, `.avi`, `.mkv`
- **Image**: `.png`, `.jpg`, `.webp`, `.gif`

### 2. Handle Images

For image files, Claude reads them directly (native capability) — no processing needed. Analyze visually and report findings.

### 3. Handle Videos

```
1. Check ffmpeg availability:
   hoangsa-cli media check-ffmpeg

2. If not available:
   hoangsa-cli media install-ffmpeg
   (or show platform-specific install instructions if auto-install fails)

3. Determine output directory:
   - If inside a HOANGSA session (SESSION_DIR exists):
     OUTPUT_DIR=$SESSION_DIR/attachments/media-analysis
   - Otherwise:
     OUTPUT_DIR=/tmp/hoangsa-debug-<timestamp>

4. Analyze the video:
   hoangsa-cli media analyze <video_path> --output-dir $OUTPUT_DIR

5. Read output files:
   - $OUTPUT_DIR/montage.png       → annotated grid with timestamps
   - $OUTPUT_DIR/diff-montage.png  → red overlay showing frame-to-frame changes

6. Analyze both images to identify:
   - UI changes across frames
   - Visual regressions
   - Animation issues
   - Layout shifts
```

### 4. Report Findings

Report findings with timestamps referencing specific frames visible in the montage grid.

## Checklist

```
- [ ] Identified media file type (image vs video)
- [ ] For images: analyzed directly via Claude vision
- [ ] For videos: confirmed ffmpeg available
- [ ] For videos: ran hoangsa-cli media analyze
- [ ] Read montage.png (frame grid)
- [ ] Read diff-montage.png (change overlay)
- [ ] Reported findings with frame timestamps
```
