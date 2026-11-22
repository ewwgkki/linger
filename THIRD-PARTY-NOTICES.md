# Third-party notices

The Windows installer includes `ffmpeg.exe`, an audio-only build of FFmpeg 9.1.

- Project: https://ffmpeg.org/
- Source: https://ffmpeg.org/releases/ffmpeg-9.1.tar.xz
- SHA-256: `1a4d394ae63d190a526cd052eb4415c156b5c5e2df37efab41a7de0b8fde2433`
- License: GNU LGPL version 2.1 or later, see `licenses/FFmpeg-LGPL-2.1.txt`
- Build options: see `scripts/build-ffmpeg.sh` in the Linger repository

Linger runs `ffmpeg.exe` as a separate program. You may replace it with your own build.
