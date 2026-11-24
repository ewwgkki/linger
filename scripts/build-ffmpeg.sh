#!/usr/bin/env bash
# Builds the small, audio-only, LGPL ffmpeg that Linger ships.
# Usage: scripts/build-ffmpeg.sh <ffmpeg-9.1 source dir> <output file>
# On macOS it builds for Apple silicon with the Xcode tools.
# Elsewhere it needs make and a Windows cross compiler, for example x86_64-w64-mingw32-gcc.
set -euo pipefail
src=$(cd "$1" && pwd)
out=$2
if [[ "$(uname -s)" == Darwin ]]; then
  min=-mmacosx-version-min=13.0
  platform=(--arch=arm64 --target-os=darwin --cc=clang --extra-cflags="$min" --extra-ldflags="$min")
  product=ffmpeg
  jobs=$(sysctl -n hw.ncpu)
else
  cross=${CROSS_PREFIX:-x86_64-w64-mingw32-}
  platform=(--enable-cross-compile --cross-prefix="$cross" --arch=x86_64 --target-os=mingw32 --extra-ldflags='-static -s')
  product=ffmpeg.exe
  jobs=$(nproc)
fi
build=$(mktemp -d)
trap 'rm -rf "$build"' EXIT
cd "$build"
"$src/configure" \
  "${platform[@]}" \
  --disable-everything --disable-autodetect --disable-doc --disable-debug --disable-network \
  --disable-avdevice --disable-swscale --disable-ffplay --disable-ffprobe --disable-x86asm \
  --enable-small --enable-static --disable-shared --disable-stripping \
  --enable-protocol=file,pipe \
  --enable-demuxer=asf,ape,ogg,mp3,flac,mov,aac,wav \
  --enable-decoder=wmav1,wmav2,wmapro,wmalossless,wmavoice,ape,opus,vorbis,mp3float,mp2float,flac,aac,alac,pcm_s16le,pcm_s24le,pcm_s32le,pcm_u8,pcm_f32le \
  --enable-parser=mpegaudio,flac,aac,opus,vorbis \
  --enable-filter=aresample,aformat,anull,atrim \
  --enable-encoder=pcm_f32le --enable-muxer=pcm_f32le
make -j"$jobs" "$product"
[[ "$product" == ffmpeg ]] && strip -x ffmpeg
cp "$product" "$out"
