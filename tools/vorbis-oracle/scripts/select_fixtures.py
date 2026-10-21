"""Copies the committed fixture subset of the corpus into the crate's tests.

usage: python -I tools/vorbis-oracle/scripts/select_fixtures.py <corpus> <crates/vkstorm-vorbis/tests/fixtures>

The subset keeps every class of input the full corpus has (the viewer's
encoder at the lengths that matter, other encoder settings, chains, unusual
page layouts, every damage group, the synthetic setups) while staying a
few MB: explicit picks plus a deterministic sample of each damaged group,
preferring small files. Afterwards regenerate the expected traces with
`vorbis-oracle expected` on Windows and on Linux (see the README).
"""
import os
import shutil
import sys

PICKS = [
    # the viewer's encoder (LLVorbisEncode): lengths around block and buffer sizes
    "clean/viewer/v16m_mix_0000000.ogg",
    "clean/viewer/v16m_mix_0000001.ogg",
    "clean/viewer/v16m_mix_0000063.ogg",
    "clean/viewer/v16m_mix_0000064.ogg",
    "clean/viewer/v16m_mix_0000065.ogg",
    "clean/viewer/v16m_mix_0000255.ogg",
    "clean/viewer/v16m_mix_0000257.ogg",
    "clean/viewer/v16m_mix_0001000.ogg",
    "clean/viewer/v16m_mix_0004096.ogg",
    "clean/viewer/v16m_mix_0044100.ogg",
    "clean/viewer/v16m_mix_0441000.ogg",
    "clean/viewer/v16m_mix_1323000.ogg",
    "clean/viewer/v16m_clip_0044100.ogg",
    "clean/viewer/v16m_silence_0044100.ogg",
    "clean/viewer/v16m_transient_0044100.ogg",
    "clean/viewer/v16m_impulse_0044100.ogg",
    "clean/viewer/v16m_noise_0044100.ogg",
    "clean/viewer/v16m_sine_0004096.ogg",
    "clean/viewer/v16m_gaps_0441000.ogg",
    "clean/viewer/v16s_mix_0044100.ogg",
    "clean/viewer/v16s_clip_0001025.ogg",
    "clean/viewer/v8m_mix_0044100.ogg",
    "clean/viewer/v8s_mix_0000100.ogg",
    # other encoder settings
    "clean/quality/q-01_1ch_mix.ogg",
    "clean/quality/q+05_2ch_mix.ogg",
    "clean/quality/q+10_1ch_mix.ogg",
    "clean/quality/q0.1_2ch_uncoupled.ogg",
    "clean/rates/r008000_1ch_q0.0.ogg",
    "clean/rates/r011025_2ch_q0.6.ogg",
    "clean/rates/r022050_1ch_q0.6.ogg",
    "clean/rates/r048000_2ch_q0.0.ogg",
    "clean/rates/r096000_1ch_q0.0.ogg",
    "clean/rates/r012345_2ch_q0.0.ogg",
    "clean/channels/c003_managed_96k.ogg",
    "clean/channels/c006_managed_96k.ogg",
    "clean/channels/c008_managed_96k.ogg",
    "clean/bitrate/managed_32k_mono.ogg",
    "clean/bitrate/managed_cbr32k_mono.ogg",
    "clean/bitrate/vbr_45k.ogg",
    "clean/comments/several.ogg",
    "clean/comments/mixed_case.ogg",
    "clean/comments/utf8.ogg",
    "clean/comments/odd_fields.ogg",
    "clean/comments/vendor_empty.ogg",
    "clean/comments/none.ogg",
    "clean/lengths/l00000_2ch_48k.ogg",
    "clean/lengths/l00001_1ch_chunk1.ogg",
    "clean/lengths/l00129_2ch_48k.ogg",
    "clean/lengths/l02049_2ch_48k.ogg",
    "clean/chained/chain_44m_22s_8m.ogg",
    "clean/chained/chain_5links.ogg",
    "clean/chained/chain_8m_6ch_44m.ogg",
    "clean/chained/chain_with_empty_link.ogg",
    "odd/pages/viewer_max.ogg",
    "odd/pages/viewer_seg1.ogg",
    "odd/pages/viewer_pkt1.ogg",
    # behave differently with the 32-bit C long of Windows
    "damaged/granule/stereo_p9_huge.ogg",
    "damaged/idhdr/viewer_rate_max.ogg",
]

# damaged groups: up to this many files each, smallest first, then evenly spaced
SAMPLE = {
    "damaged/files": 12,
    "damaged/granule": 14,
    "damaged/idhdr": 10,
    "damaged/setuphdr": 14,
    "damaged/commenthdr": 8,
    "damaged/pages": 12,
    "damaged/truncate": 16,
    "damaged/corrupt": 14,
    "damaged/pagehdr": 10,
    "damaged/garbage": 3,
    "damaged/chain": 4,
    "damaged/audiopkt": 3,
    "synthetic": 60,
}
MAX_SAMPLED_SIZE = 10_000


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    corpus, out = sys.argv[1], sys.argv[2]
    chosen = list(PICKS)
    for group, count in SAMPLE.items():
        files = []
        base = os.path.join(corpus, *group.split("/"))
        for root, _, names in os.walk(base):
            for n in names:
                if n.endswith(".ogg"):
                    p = os.path.join(root, n)
                    rel = os.path.relpath(p, corpus).replace(os.sep, "/")
                    if os.path.getsize(p) <= MAX_SAMPLED_SIZE and rel not in chosen:
                        files.append(rel)
        files.sort()
        if len(files) > count:
            step = len(files) / count
            files = [files[int(i * step)] for i in range(count)]
        chosen += files
    total = 0
    for rel in chosen:
        src = os.path.join(corpus, *rel.split("/"))
        dst = os.path.join(out, *rel.split("/"))
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copyfile(src, dst)
        total += os.path.getsize(src)
    print(f"{len(chosen)} fixtures, {total} bytes")


if __name__ == "__main__":
    main()
