# Linger · 余音

An ad-free local music player for Windows: play your own music from local disks or NAS, with playlists, favorites and synced lyrics.

无广告的本地音乐播放器，播放电脑或 NAS 上的音乐，支持歌单、收藏和滚动歌词。

> Status: early development. 开发中。

## Features / 功能

- Play local and NAS music (in development) / 播放本地与 NAS 音乐（开发中）
- Playlists (in development) / 歌单（开发中）
- Favorites (in development) / 收藏（开发中）
- Synced, scrolling lyrics (in development) / 滚动歌词（开发中）
- No ads, no accounts / 无广告、无需账号

## Build from source / 从源码构建

Requirements / 依赖:

1. Rust (via [rustup](https://rustup.rs/)), MSVC toolchain: `rustup default stable-x86_64-pc-windows-msvc`
2. Visual Studio 2022 Build Tools with the "Desktop development with C++" workload
3. Tauri CLI: `cargo install tauri-cli --version "^2" --locked`
4. WebView2 (preinstalled on Windows 10/11)

```powershell
git clone https://github.com/ewwgkki/linger.git
cd linger
cargo tauri dev      # run in development
cargo tauri build    # build the NSIS installer
```

## License / 许可证

[GPL-3.0](LICENSE)
