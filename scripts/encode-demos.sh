#!/usr/bin/env bash
# Encode recorded demos (scripts/record-desktop-demo.sh) for the website.
#
#   scripts/encode-demos.sh [IN_DIR] [OUT_DIR]
#
# Reads demo-<name>.mp4 (dark) and demo-<name>-light.mp4 from IN_DIR (the repo
# root by default) and writes <name>-<mode>.mp4 (H.264), .webm (VP9), and .jpg
# (poster) to OUT_DIR (docs/public/demos by default). No audio, at most 1600px
# wide, 30 fps.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
in_dir="${1:-$repo_root}"
out_dir="${2:-$repo_root/docs/public/demos}"
command -v ffmpeg >/dev/null || {
    echo "ffmpeg is required (nix shell nixpkgs#ffmpeg)" >&2
    exit 1
}
mkdir -p "$out_dir"
scale="fps=30,scale='min(1600,iw)':-2:flags=lanczos"

shopt -s nullglob
found=0
for src in "$in_dir"/demo-*.mp4; do
    base="$(basename "$src" .mp4)"
    name="${base#demo-}"
    mode=dark
    if [[ "$name" == *-light ]]; then
        name="${name%-light}"
        mode=light
    fi
    out="$out_dir/$name-$mode"
    echo "Encoding $name ($mode)…"
    ffmpeg -loglevel error -y -i "$src" -an -vf "$scale" \
        -c:v libx264 -preset slow -crf 26 -pix_fmt yuv420p -movflags +faststart "$out.mp4"
    ffmpeg -loglevel error -y -i "$src" -an -vf "$scale" \
        -c:v libvpx-vp9 -crf 36 -b:v 0 -row-mt 1 -deadline good -cpu-used 2 "$out.webm"
    ffmpeg -loglevel error -y -ss 1.5 -i "$src" -frames:v 1 \
        -vf "scale='min(1600,iw)':-2:flags=lanczos" -q:v 3 "$out.jpg"
    found=1
done
[[ $found == 1 ]] || {
    echo "No demo-*.mp4 in $in_dir" >&2
    exit 1
}
ls -lh "$out_dir"
