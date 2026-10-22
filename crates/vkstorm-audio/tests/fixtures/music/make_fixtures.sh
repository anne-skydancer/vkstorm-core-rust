#!/usr/bin/env bash
# Regenerates the parcel music test fixtures (vkstorm-audio src/stream tests
# and the viewer's vksaudio_mock_stream_server.py). Run on Linux (Ubuntu
# 26.04 was used) from any directory: ./make_fixtures.sh OUTDIR
#
# Tools (Ubuntu packages, versions used for the committed files):
#   sox 14.7.0.9, lame 3.101, vorbis-tools 1.4.3 (oggenc), opus-tools 0.2
#   (opusenc), flac 1.5.0, ffmpeg 8.0.1 (mp2, mpeg4, libtheora, aac
#   encoders); and the aac-enc example program of fdk-aac v3.0.0 built from
#   source (Ubuntu's libfdk-aac2 is fdk-aac-free, which has no SBR/PS):
#     git clone --depth 1 --branch v3.0.0 https://github.com/mstorsjo/fdk-aac
#     cmake -S fdk-aac -B fdk-aac/build -DBUILD_PROGRAMS=ON -DCMAKE_BUILD_TYPE=Release
#     cmake --build fdk-aac/build
#   and FDK_AAC_ENC=fdk-aac/build/aac-enc in the environment. aac-enc writes
#   ADTS.
#
# Every tone is a click-free continuous sine with a whole number of cycles
# over the file, so a looped file (the mock server's live streams) has no
# discontinuity in the PCM. Channel 1 is 440 Hz at 0.5 (-6 dBFS), channel 2
# is 660 Hz at 0.25 (-12 dBFS): the tests identify channels, levels and the
# playback rate from them. HE-AAC files put 9 kHz on channel 2, above the SBR
# crossover, so that the SBR (and, for v2, PS) tools carry real content.
# The 5.1 file has 300, 400, 500, 50 (LFE: libvorbis low-passes it), 700
# and 800 Hz on its six channels in WAV order.
#
# sox runs without dither (-D). The serial numbers, tags and encoder
# settings are fixed, and ffmpeg runs
# with +bitexact, so a rerun with the same tool versions gives identical
# files (sha256 in manifest.json).
set -euo pipefail
OUT=${1:?usage: make_fixtures.sh OUTDIR}
ENC=${FDK_AAC_ENC:?set FDK_AAC_ENC to the aac-enc of fdk-aac v3.0.0}
mkdir -p "$OUT"
cd "$OUT"
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT

tone() { # rate channels seconds file [freq1 amp1 freq2 amp2]
    local rate=$1 ch=$2 secs=$3 file=$4 f1=${5:-440} a1=${6:-0.5} f2=${7:-660} a2=${8:-0.25}
    if [ "$ch" = 1 ]; then
        sox -D -n -r "$rate" -c 1 -b 16 "$file" synth "$secs" sine "$f1" vol "$a1"
    else
        sox -D -n -r "$rate" -c 2 -b 16 "$file" synth "$secs" sine "$f1" sine "$f2" remix 1v"$a1" 2v"$a2"
    fi
}

tone 44100 2 4 "$W/t44.wav"
tone 48000 2 4 "$W/t48.wav"
tone 44100 2 2 "$W/t44_2s.wav"
tone 44100 2 1 "$W/t44_1s.wav"
tone 22050 1 2 "$W/m22.wav"
tone 48000 2 3 "$W/b48.wav"
tone 32000 1 2 "$W/c32.wav"
tone 48000 2 4 "$W/he48.wav" 440 0.5 9000 0.25
tone 44100 2 4 "$W/he44.wav" 440 0.5 9000 0.25
sox -D -n -r 48000 -c 6 -b 16 "$W/s51.wav" synth 1 sine 300 sine 400 sine 500 sine 50 sine 700 sine 800 \
    remix 1v0.3 2v0.3 3v0.3 4v0.3 5v0.3 6v0.3

# MPEG audio. -t: no Xing/Info frame (live streams carry none).
lame --quiet --nohist -t -b 128 "$W/t44.wav" tone.mp3
lame --quiet --nohist -t -m m -b 64 "$W/m22.wav" tone_mono22.mp3
lame --quiet --nohist -t -b 128 --id3v2-only --tt "Tag Title" --ta "Tag Artist" "$W/t44_2s.wav" tone_id3.mp3
ffmpeg -nostdin -loglevel error -y -i "$W/t48.wav" -c:a mp2 -b:a 192k -fflags +bitexact -flags:a +bitexact -map_metadata -1 -f mp2 tone.mp2

# Ogg Vorbis: a plain file and a three-link chain whose links change the
# rate, the channel count and the comments.
oggenc --quiet -q 3 --serial 1001 -t "Plain Title" -a "Plain Artist" -o tone.ogg "$W/t44.wav"
oggenc --quiet -q 3 --serial 2001 -t "First Title" -a "First Artist" -o "$W/l1.ogg" "$W/t44_2s.wav"
oggenc --quiet -q 3 --serial 2002 -t "Second Title" -a "Second Artist" -o "$W/l2.ogg" "$W/b48.wav"
oggenc --quiet -q 3 --serial 2003 -t "Third Title" -a "Third Artist" -o "$W/l3.ogg" "$W/c32.wav"
cat "$W/l1.ogg" "$W/l2.ogg" "$W/l3.ogg" > chain.ogg
oggenc --quiet -q 2 --serial 3001 -o tone51.ogg "$W/s51.wav"

# AAC in ADTS: LC, HE-AAC v1 (SBR) and v2 (SBR + PS), implicit signalling
# as ADTS requires (the header says LC at the core rate).
"$ENC" -t 2 -r 128000 "$W/t44.wav" tone_lc.aac
"$ENC" -t 5 -r 64000 "$W/he48.wav" tone_he1.aac
"$ENC" -t 29 -r 32000 "$W/he44.wav" tone_he2.aac

# Opus (48 kHz input: the decoded length must equal the input exactly).
opusenc --quiet --bitrate 96 --serial 4001 --title "Opus Title" --artist "Opus Artist" "$W/t48.wav" tone.opus

# FLAC, native and in Ogg.
flac --silent --best -T "TITLE=Flac Title" -T "ARTIST=Flac Artist" -o tone.flac "$W/t44_2s.wav"
flac --silent --best --ogg --serial-number=5001 -T "TITLE=OggFlac Title" -o tone_flac.oga "$W/t44_1s.wav"

# Not playable as music: video, an audio-only MP4, an HTML page.
ffmpeg -nostdin -loglevel error -y -f lavfi -i testsrc=size=64x64:rate=10 -f lavfi -i sine=frequency=440 -t 1 \
    -c:v mpeg4 -c:a aac -b:a 32k -movflags +faststart -fflags +bitexact -flags:v +bitexact -flags:a +bitexact \
    -map_metadata -1 video.mp4
ffmpeg -nostdin -loglevel error -y -f lavfi -i testsrc=size=64x64:rate=10 -f lavfi -i sine=frequency=440 -t 1 \
    -c:v libtheora -c:a libvorbis -fflags +bitexact -flags:v +bitexact -flags:a +bitexact -map_metadata -1 video.ogv
ffmpeg -nostdin -loglevel error -y -i "$W/t44_1s.wav" -c:a aac -b:a 64k -movflags +faststart -fflags +bitexact \
    -flags:a +bitexact -map_metadata -1 tone.m4a
cat > page.html <<'EOF'
<!DOCTYPE html>
<html><head><title>Not a stream</title></head>
<body><p>This is a web page, not an audio stream.</p></body></html>
EOF

# manifest.json: what each file holds, for the tests and the mock server's
# real-time pacing.
python3 - <<'EOF'
import hashlib, json, os
files = {
    "tone.mp3":        dict(kind="mp3",    rate=44100, channels=2, seconds=4.0, freqs=[440, 660]),
    "tone_mono22.mp3": dict(kind="mp3",    rate=22050, channels=1, seconds=2.0, freqs=[440]),
    "tone_id3.mp3":    dict(kind="mp3",    rate=44100, channels=2, seconds=2.0, freqs=[440, 660],
                            artist="Tag Artist", title="Tag Title"),
    "tone.mp2":        dict(kind="mp2",    rate=48000, channels=2, seconds=4.0, freqs=[440, 660]),
    "tone.ogg":        dict(kind="vorbis", rate=44100, channels=2, seconds=4.0, freqs=[440, 660],
                            artist="Plain Artist", title="Plain Title"),
    "chain.ogg":       dict(kind="vorbis", rate=44100, channels=2, seconds=7.0, freqs=[440, 660],
                            links=[dict(rate=44100, channels=2, seconds=2.0, artist="First Artist", title="First Title"),
                                   dict(rate=48000, channels=2, seconds=3.0, artist="Second Artist", title="Second Title"),
                                   dict(rate=32000, channels=1, seconds=2.0, artist="Third Artist", title="Third Title")]),
    "tone51.ogg":      dict(kind="vorbis", rate=48000, channels=6, seconds=1.0, freqs=[300, 400, 500, 50, 700, 800]),
    "tone_lc.aac":     dict(kind="aac-lc", rate=44100, channels=2, seconds=4.0, freqs=[440, 660]),
    "tone_he1.aac":    dict(kind="he-aac", rate=48000, channels=2, seconds=4.0, freqs=[440, 9000]),
    "tone_he2.aac":    dict(kind="he-aac-v2", rate=44100, channels=2, seconds=4.0, freqs=[440, 9000]),
    "tone.opus":       dict(kind="opus",   rate=48000, channels=2, seconds=4.0, freqs=[440, 660],
                            artist="Opus Artist", title="Opus Title"),
    "tone.flac":       dict(kind="flac",   rate=44100, channels=2, seconds=2.0, freqs=[440, 660],
                            artist="Flac Artist", title="Flac Title"),
    "tone_flac.oga":   dict(kind="ogg-flac", rate=44100, channels=2, seconds=1.0, freqs=[440, 660],
                            title="OggFlac Title"),
    "video.mp4":       dict(kind="video",  seconds=1.0),
    "video.ogv":       dict(kind="video",  seconds=1.0),
    "tone.m4a":        dict(kind="mp4-audio", seconds=1.0),
    "page.html":       dict(kind="html",   seconds=1.0),
}
for name, info in files.items():
    data = open(name, "rb").read()
    info["bytes"] = len(data)
    info["sha256"] = hashlib.sha256(data).hexdigest()
with open("manifest.json", "w") as f:
    json.dump(files, f, indent=1, sort_keys=True)
    f.write("\n")
EOF
ls -la
du -cb ./* | tail -1
