# soloud-oracle: mixer, WAV reader and asset decode parity harness

Compares `crates/vkstorm-audio` with the C/C++ the viewer used before rung
six:

- the mixer (`src/mixer`) with SoLoud e82fd32c1f plus the 3p-soloud patches,
  driven the way `indra/llaudio/llaudioengine_soloud.cpp` drives it;
- the WAV reader (`src/wav.rs`) with SoLoud's `Wav::load` (dr_wav 0.13.2);
- the asset decoder (`src/asset.rs`) with `LLVorbisDecodeState`
  (`indra/llaudio/llaudiodecodemgr.cpp`) over the viewer's libvorbis.

Nothing here is built by `cargo build` of the workspace except the Rust
driver; the C/C++ programs are compiled on demand.

## Layout

- `c/soloud_oracle.cpp`: the C++ side. `scene` runs a scene script against
  SoLoud's NULLDRIVER (`Soloud::mix` in chunks of the script's size, FTZ/DAZ
  set around each call as on the viewer's audio thread) and writes the
  interleaved float output and a query log; `bench` times the `mix` calls of
  a script; `wavdump` writes what `Wav::load` produced. The adapter's call
  sequences (startVoicePaused, play, playSynced, update3DPosition, declicked
  stops, the wind source, loadWAV's defaults and fade) are copied from
  `llaudioengine_soloud.cpp`. The wind is the viewer's unmodified
  `llwindgen.h`; `c/shim` supplies the few `llcommon` definitions it needs and
  a deterministic `ll_frand` (xorshift64*, the same generator as
  `vkstorm_audio::mixer::WindNoise`; the viewer's own is randomly seeded per
  thread, so there is nothing to reproduce there).
- `c/asset_oracle.c`: `initDecode`/`decodeSection`/`finishDecode` transcribed
  in C over libvorbisfile with LLFileSystem's read/seek/tell semantics; writes
  the `.dsf` bytes or the error name. `--bench <list> <reps>` times it.
- `src/main.rs`: the Rust side and the comparisons. The scene runner and the
  test material (fixtures, scenes) live in
  `crates/vkstorm-audio/tests/mixer_common`, shared with the crate's parity
  test; the generator uses no libm, so scripts and fixtures are the same
  bytes on every platform.
- `scripts/build_win.bat`, `scripts/build_linux.sh`: the builds below.

## Scenes

15 scenes x {44100, 48000} Hz x {1, 2, 4, 6, 8} channels: `moving` (sources
passing the listener, Doppler 1), `listener` (listener moving and rotating),
`attenuation` (0.5 to 600 m), `rolloff` (0.4/1/19, changed live), `doppler`
(source and listener velocities, factors 0/1/4), `wind` (changing targets),
`winddecay` (6 s at zero wind gain: the generator's gain underflows, which
SoLoud's flush-to-zero decides), `voices` (80 voices, the active-voice sort,
the protected wind), `declick` (fadeVolume + scheduleStop 20 ms), `synced`
(seek to fmod(master position, length) + 20 ms fade-in), `looping` (loop
counts, loop toggles), `ui2d` (2-D sounds: mono/stereo/4/6/8-channel, 8/16/24
bit, float, 11.025 to 96 kHz, the stereo-in-mono decoded-cache file),
`global` (global volume steps), `inaudible` (voices crossing the 0.001
threshold while ticking), `chunks` (mix sizes 37, 333, 512, 1000, 1023, 1024,
441, 7). Device periods are miniaudio's 40 ms, mixed in 1024-frame chunks as
the patched backend did.

## Reproduce

1. Oracles. Windows, from any shell (VS 2022, the viewer's configured
   `build-vc170-64` provides `soloud.lib` and the ogg/vorbis libraries):

   ```bat
   tools\soloud-oracle\scripts\build_win.bat C:\Dev\vkstorm-rust-audio <outdir>
   ```

   Linux (the patched SoLoud source is the 3p-soloud build's
   `_deps/soloud_source-src`; libogg 1.3.5 and libvorbis 1.3.7 sources):

   ```sh
   tools/soloud-oracle/scripts/build_linux.sh <soloud-src> <viewer>/indra/llaudio <ogg-src> <vorbis-src> <outdir> 14
   ```

2. Driver: `cargo build --release -p soloud-oracle --features link-curl`
   (libcurl as in `crates/vkstorm-http/README.md`).

3. Compare:

   ```sh
   soloud-oracle gen <dir>
   soloud-oracle compare <outdir>/soloud_oracle <dir>
   soloud-oracle wav-compare <outdir>/soloud_oracle <dir>
   soloud-oracle asset-compare <outdir>/asset_oracle crates/vkstorm-vorbis/tests/fixtures <work>
   soloud-oracle asset-compare <outdir>/asset_oracle <vorbis corpus> <work>
   ```

4. Committed expectations (checked by `cargo test -p vkstorm-audio
   --features link-curl --test mixer_parity --test asset_decode` with no C
   involved):

   ```sh
   soloud-oracle expected <outdir>/soloud_oracle <dir> crates/vkstorm-audio/tests/mixer_expected/scenes-windows-msvc.txt
   soloud-oracle expected <outdir>/soloud_oracle <dir> crates/vkstorm-audio/tests/mixer_expected/scenes-linux-gcc-glibc.txt
   soloud-oracle asset-expected <outdir>/asset_oracle crates/vkstorm-vorbis/tests/fixtures <work> crates/vkstorm-audio/tests/mixer_expected/asset-<platform>.txt
   ```

5. Timing: `SOLOUD_ORACLE=<outdir>/soloud_oracle cargo bench -p
   vkstorm-audio --features link-curl --bench mixer` (64 moving 3D voices,
   48 kHz, 2/6/8 channels, min of 3 interleaved runs); asset decode:
   `asset_oracle --bench <list> 1` and `soloud-oracle asset-bench <list> 1`
   alternately.

## Findings about SoLoud

- More than 64 voices with `INAUDIBLE_TICK` crash it: `calcActiveVoices`
  returns early without `mapResampleBuffers` when the ticking voices fill
  every active slot, and a newly active voice then reads through a null
  resample buffer. The adapter sets the tick flag on every channel voice, so
  65 coexisting channel voices (30 channels plus 20 ms declick tails) are
  enough. The port maps buffers only in that case, so every run SoLoud
  survives is unchanged. The `voices` scene turns the tick off on most of
  its voices to exercise the volume sort without the crash.
- `Soloud::mix_internal` sets FTZ/DAZ once per process (`static bool once`)
  on whichever thread mixes first and never restores it; after a device
  change the new audio thread mixes with denormals. The port sets FTZ/DAZ
  for each mix call and restores the caller's mode.
- dr_wav crashes on an ADPCM-tagged header with PCM-sized blocks
  (`wavs/adpcm_tag.wav`); the port rejects ADPCM.

Results: `results/`.
