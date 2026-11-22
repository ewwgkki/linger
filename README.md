# Linger · 余音

An ad-free local music player for Windows: play your own music from local disks or NAS, with playlists, favorites and synced lyrics.

无广告的本地音乐播放器，播放电脑或 NAS 上的音乐，支持歌单、收藏和滚动歌词。

> Status: preview. Windows 10/11. 预览版，支持 Windows 10/11。

## Download / 下载

**[Download Linger.exe / 下载 Linger.exe](https://github.com/ewwgkki/linger/releases/latest/download/Linger.exe)** · [All releases / 所有版本](https://github.com/ewwgkki/linger/releases)

No installation needed: download it and double-click. The app is not code-signed yet, so Windows may show "Windows protected your PC". Choose **More info**, then **Run anyway**.

无需安装，下载后双击运行。程序暂未签名，Windows 可能提示“已保护你的电脑”，点 **更多信息**，再点 **仍要运行**。

WMA, APE and Opus playback needs `ffmpeg.exe` in the same folder as `Linger.exe` ([FFmpeg builds](https://github.com/BtbN/FFmpeg-Builds/releases)). Other formats work without it.

播放 WMA、APE、Opus 需要把 `ffmpeg.exe` 放在 `Linger.exe` 同一文件夹（[FFmpeg 下载](https://github.com/BtbN/FFmpeg-Builds/releases)）。其他格式无需 FFmpeg。

## Features / 功能

- Play music from local disks, mapped drives and network (UNC) folders / 播放本地硬盘、映射盘和网络路径上的音乐
- MP3, FLAC, M4A, WAV, OGG, plus WMA / APE / Opus with FFmpeg; CUE sheets are split into tracks / 支持常见格式，CUE 整轨自动分轨
- Browse by song, artist, album, folder and quality / 按歌曲、歌手、专辑、文件夹和音质浏览
- Playlists, favorites and a play queue / 歌单、我喜欢的音乐、播放队列
- Import Spotify playlists from Exportify or Spotify data exports / 从 Exportify 或 Spotify 数据导出导入歌单
- Synced karaoke-style lyrics and a full-screen play page / 卡拉 OK 式滚动歌词和全屏播放页
- Lyrics from embedded tags, `.lrc` files, LRCLIB and NetEase / 歌词来自内嵌标签、`.lrc`、LRCLIB 和网易云
- No ads, no accounts, and your music files are never modified / 无广告、无需账号、绝不修改音乐文件

Online lyric lookups send only the song title, artist, album and duration. NetEase can be turned off in settings.

在线查歌词只发送歌名、歌手、专辑和时长。网易云来源可在设置中关闭。

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
cargo tauri build    # build the release
```

## License / 许可证

GPL-3.0-or-later — see [LICENSE](LICENSE) / 见 [LICENSE](LICENSE)
