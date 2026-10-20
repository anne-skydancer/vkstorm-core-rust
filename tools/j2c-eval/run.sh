#!/usr/bin/env bash
# Reproducible J2C decoder comparison. Requires: opj_bench_{avx2,sse2}.exe in c/ (see README),
# the workspace-root target/release/j2c-eval.exe, and a corpus directory of .j2c files.
# usage: ./run.sh <corpus_dir> <results.csv> [iters]
set -u
CORPUS=$1; OUT=$2; ITERS=${3:-5}
HERE=$(cd "$(dirname "$0")" && pwd); RS=$HERE/../../target/release/j2c-eval.exe
TMP=$(mktemp -d)
echo "file,bytes,bytes_pct,reduce,decoder,w,h,c,best_ms,mean_ms,max_abs_vs_opj,psnr_vs_opj" > "$OUT"
# scenarios: reduce:pct-of-bytes
SCEN="0:100 1:100 2:100 3:100 0:25 1:50 2:25 3:12"
for f in "$CORPUS"/*.j2c; do
  name=$(basename "$f" .j2c); size=$(stat -c %s "$f")
  for sc in $SCEN; do
    r=${sc%%:*}; pct=${sc##*:}; bytes=$(( size * pct / 100 )); [ "$pct" = 100 ] && bytes=0
    ref="$TMP/${name}_${r}_${pct}_opj.raw"
    for dec in opj_avx2 opj_sse2 hayro j2k j2k-auto; do
      o="$TMP/${name}_${r}_${pct}_${dec}.raw"
      case $dec in
        opj_avx2) line=$("$HERE/c/opj_bench_avx2.exe" "$f" $r $bytes $ITERS "$o" 2>/dev/null);;
        opj_sse2) line=$("$HERE/c/opj_bench_sse2.exe" "$f" $r $bytes $ITERS "$o" 2>/dev/null);;
        *)        line=$("$RS" $dec "$f" $r $bytes $ITERS "$o" 2>/dev/null);;
      esac
      [ "$dec" = opj_avx2 ] && cp "$o" "$ref" 2>/dev/null
      if [[ "$line" == FAIL* ]] || [ -z "$line" ]; then
        echo "$name,$bytes,$pct,$r,$dec,,,,,,," >> "$OUT"; echo "$name r=$r pct=$pct $dec: ${line:-FAIL}"; continue
      fi
      set -- $line; w=$1; h=$2; c=$3; best=$4; mean=$5
      cmp=""; if [ -f "$ref" ] && [ -f "$o" ]; then cmp=$("$RS" compare "$ref" "$o"); fi
      if [[ "$cmp" == max_abs* ]]; then set -- $cmp; maxd=$2; psnr=$6; else maxd="${cmp:-}"; psnr=""; fi
      echo "$name,$bytes,$pct,$r,$dec,$w,$h,$c,$best,$mean,$maxd,$psnr" >> "$OUT"
      echo "$name r=$r pct=$pct $dec: ${w}x${h}x$c best=${best}ms psnr=${psnr:-ref} max=${maxd:-0}"
    done
  done
done
rm -rf "$TMP"
