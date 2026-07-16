# Supported Media Formats

## Video Formats

| Format | Extension | Notes |
|--------|-----------|-------|
| MP4 | `.mp4` | Most common, H.264/H.265 codec |
| QuickTime | `.mov` | Common on macOS/iOS screen recordings |
| WebM | `.webm` | Web-optimized, VP8/VP9 codec |
| AVI | `.avi` | Legacy format, larger file sizes |
| Matroska | `.mkv` | Container format, multiple codecs |

## Image Formats

| Format | Extension | Notes |
|--------|-----------|-------|
| PNG | `.png` | Lossless, best for UI screenshots |
| JPEG | `.jpg`, `.jpeg` | Lossy, photos and gradients |
| WebP | `.webp` | Modern web format, both lossy/lossless |
| GIF | `.gif` | Animated sequences, limited colors |

## ffmpeg Requirements

- **Minimum version**: 4.0+
- **Required codecs**: libx264 (decode), png (encode)
- **Install methods**:
  - macOS: `brew install ffmpeg`
  - Ubuntu/Debian: `apt install ffmpeg`
  - Windows: `choco install ffmpeg`
  - Auto: `hoangsa-cli media install-ffmpeg`

## Output Artifacts

| File | Description | Use |
|------|-------------|-----|
| `montage.png` | Grid of key frames with timestamps | Identify UI states across time |
| `diff-montage.png` | Red overlay showing pixel differences | Spot visual regressions between frames |

## Analysis Capabilities

- **Layout shifts**: Detect elements that moved between frames
- **Visual regressions**: Pixel-level diff between states
- **Animation issues**: Frame-by-frame timing analysis
- **Rendering bugs**: Identify artifacts, clipping, z-index problems
- **Responsive breakpoints**: Compare layouts at different viewport sizes
