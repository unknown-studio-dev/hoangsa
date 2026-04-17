---
name: visual-debug
description: "This skill should be used when the user provides screenshots, videos, screen recordings, or mentions visual bugs, UI glitches, layout shifts, animation issues, or visual regressions. Analyzes media files to create annotated montage grids with diff overlays for visual debugging."
---

<objective>
Analyze screenshots, screen recordings, and video files to identify visual bugs, UI glitches, layout shifts, animation issues, and visual regressions. Produces annotated montage grids with diff overlays.
</objective>

<triggers>
- User shares screenshots, screen recordings, or video files
- "There's a UI glitch in my app"
- "This animation looks wrong"
- "Something changed visually between versions"
- "The layout shifts when I scroll"
- Investigating layout shifts, rendering bugs, or visual regressions
</triggers>

<flows>
<flow name="detect-media">
Identify media files in user input by extension. Classify as image or video.

Supported formats:
- **Video**: `.mp4`, `.mov`, `.webm`, `.avi`, `.mkv`
- **Image**: `.png`, `.jpg`, `.jpeg`, `.webp`, `.gif`

For detailed format info and ffmpeg requirements, see `references/supported-formats.md`.
</flow>

<flow name="handle-images">
Read image files directly using Claude's native vision capability — no processing needed. Analyze visually and report findings.
</flow>

<flow name="handle-videos">
1. Check ffmpeg availability: `hoangsa-cli media check-ffmpeg`
2. If not available, install: `hoangsa-cli media install-ffmpeg` (show platform-specific instructions if auto-install fails)
3. Determine output directory:
   - If the environment variable `SESSION_DIR` is set: `OUTPUT_DIR=$SESSION_DIR/attachments/media-analysis`
   - Otherwise: `OUTPUT_DIR=/tmp/hoangsa-debug-<timestamp>`
4. Analyze the video: `hoangsa-cli media analyze <video_path> --output-dir $OUTPUT_DIR`
5. Read output files:
   - `$OUTPUT_DIR/montage.png` — annotated grid with timestamps
   - `$OUTPUT_DIR/diff-montage.png` — red overlay showing frame-to-frame changes
6. Analyze both images to identify UI changes across frames, visual regressions, animation issues, and layout shifts
</flow>

<flow name="report-findings">
Report findings with timestamps referencing specific frames visible in the montage grid. Include:
- **What changed**: specific UI elements affected
- **Where**: frame numbers and timestamps from the montage
- **Severity**: cosmetic, functional, or blocking
- **Suggested fix**: if the root cause is identifiable from the visual evidence
</flow>
</flows>

<rules>
| Rule | Detail |
|------|--------|
| **Always classify first** | Identify media file type (image vs video) before processing |
| **ffmpeg required for video** | Confirm ffmpeg is available before video analysis |
| **Timestamp references** | Always reference specific frames and timestamps from montage |
| **Severity rating** | Every finding must include severity: cosmetic, functional, or blocking |
</rules>

<references>
- **`references/supported-formats.md`** — Supported video/image formats, ffmpeg requirements, output artifacts, and analysis capabilities
</references>
