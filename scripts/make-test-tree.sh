#!/usr/bin/env bash
# Builds the dummy tree strata is tested against. Never point strata at real files.
#   scripts/make-test-tree.sh [root]     default: /tmp/claude-1000/strata-test
#   BIG=1 also creates big/ with 50 000 empty files
set -euo pipefail
ROOT="${1:-/tmp/claude-1000/strata-test}"
rm -rf "$ROOT"
mkdir -p "$ROOT"
cd "$ROOT"

mkdir -p Documents/notes Documents/reports/2025 Projects Media/photos Media/clips "Empty Folder" \
  "folder with spaces" "ünïcødé — ферзь 日本" .hidden-dir/inner

# text files, natural-sort bait
for i in 1 2 3 10 11 20 100; do printf 'note %s\n' "$i" > "Documents/notes/note$i.txt"; done
printf '# Readme\n\nSome *markdown* text.\n\n```rust\nfn main() {}\n```\n' > Documents/README.md
printf 'quarterly numbers\nrevenue: 42\nneedle in a haystack\n' > Documents/reports/2025/q1.txt
printf 'quarterly numbers\nrevenue: 43\n' > Documents/reports/2025/q2.txt
head -c 3000000 /dev/zero | tr '\0' 'a' > Documents/reports/big.log
cat > Projects/main.rs <<'RS'
use std::collections::HashMap;

/// A tiny example.
fn main() {
    let mut m: HashMap<&str, i32> = HashMap::new();
    m.insert("needle", 1);
    println!("{:?}", m.get("needle"));
}
RS
printf 'def hello(name):\n    return f"hi {name}"  # needle\n' > Projects/script.py
printf '{"a": 1, "b": [true, null]}\n' > Projects/data.json
printf 'secret\n' > .hidden-file
printf 'inner\n' > .hidden-dir/inner/x.txt
printf 'spaced\n' > "folder with spaces/file with spaces.txt"
printf 'uni\n' > "ünïcødé — ферзь 日本/文件.txt"
long=$(printf 'very_long_name_%.0s' {1..12})
printf 'long\n' > "$long.txt"
printf 'x' > "weird 'quotes' & \$dollar.txt"
printf '#!/bin/sh\necho hi\n' > Projects/run.sh && chmod +x Projects/run.sh
: > empty.txt

# media
ffmpeg -loglevel error -f lavfi -i testsrc2=size=640x480 -frames:v 1 Media/photos/test-pattern.png
ffmpeg -loglevel error -f lavfi -i testsrc2=size=1280x720 -frames:v 1 Media/photos/wide.jpg
ffmpeg -loglevel error -f lavfi -i "mandelbrot=size=400x400" -frames:v 1 Media/photos/fractal.png
ffmpeg -loglevel error -f lavfi -i testsrc2=size=640x360:rate=25 -f lavfi -i sine=frequency=440 -t 3 \
  -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest Media/clips/clip.mp4
ffmpeg -loglevel error -f lavfi -i sine=frequency=330 -t 2 Media/tone.ogg
# a one-page PDF, made by ffmpeg-free means if possible
if command -v magick >/dev/null; then magick Media/photos/test-pattern.png Media/sample.pdf 2>/dev/null || true; fi

# symlinks
ln -s Documents/README.md link-to-readme
ln -s Documents link-to-documents
ln -s does-not-exist broken-link

# fake git repo with modified / untracked / staged files
mkdir -p Projects/repo && cd Projects/repo
git init -q
git -c user.name=test -c user.email=test@example.invalid commit -q --allow-empty -m init
printf 'one\n' > tracked.txt
printf 'two\n' > will-modify.txt
git add . && git -c user.name=test -c user.email=test@example.invalid commit -q -m files
printf 'changed\n' >> will-modify.txt
printf 'new\n' > untracked.txt
printf 'staged\n' > staged.txt && git add staged.txt
cd "$ROOT"

if [ "${BIG:-0}" = 1 ]; then
  mkdir -p big
  ( cd big && seq -f 'file-%05g.txt' 1 50000 | xargs touch )
fi
echo "$ROOT"
