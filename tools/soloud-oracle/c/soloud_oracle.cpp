// SoLoud oracle for vkstorm-audio's mixer (tools/soloud-oracle).
//
// Runs scene scripts against the viewer's SoLoud: the 3p-soloud package
// (SoLoud e82fd32c1f with the viewer's patches) on Windows, the same patched
// source built with gcc on Linux, through the NULLDRIVER backend and
// Soloud::mix, driven the way indra/llaudio/llaudioengine_soloud.cpp drives
// it (the adapter's call sequences are copied below, marked "adapter").
// The wind uses the viewer's unmodified indra/llaudio/llwindgen.h with a
// deterministic ll_frand (see shim/llrand.h).
//
// Commands:
//   soloud_oracle scene <scene.txt> <out.f32> <out.log>
//   soloud_oracle bench <scene.txt> <reps>        (prints min ns per mix call)
//   soloud_oracle wavdump <in.wav> <out.bin>      (Wav::load, planar dump)
//
// Scene script: one command per line (see src/scene.rs in the Rust driver
// for the grammar; both sides parse the same files).

#include <soloud.h>
#include <soloud_wav.h>

#include <atomic>
#include <array>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <map>
#include <memory>
#include <sstream>
#include <string>
#include <vector>

#include <xmmintrin.h>

#include "llcommon.h"
#include "llrand.h"
#include "llwindgen.h"

uint64_t g_oracle_noise_state = 1;

// ---------------------------------------------------------------- adapter
// From llaudioengine_soloud.cpp (wind source and buffer loading), verbatim
// apart from the logging.

static const F32 SOLOUD_MAX_AUDIBLE_DISTANCE = 512.f;

struct LLSoLoudWindState
{
    LLWindGen<F32> generator;
    std::atomic<F32> frequency{100.f};
    std::atomic<F32> gain{0.f};
    std::atomic<F32> pan{0.5f};
};

class LLSoLoudWindInstance : public SoLoud::AudioSourceInstance
{
public:
    LLSoLoudWindInstance(LLSoLoudWindState& state)
        : mState(state)
    {
    }

    virtual unsigned int getAudio(float *aBuffer, unsigned int aSamplesToRead, unsigned int aBufferSize)
    {
        mState.generator.mTargetFreq = mState.frequency.load(std::memory_order_relaxed);
        mState.generator.mTargetGain = mState.gain.load(std::memory_order_relaxed);
        mState.generator.mTargetPanGainR = mState.pan.load(std::memory_order_relaxed);
        for (unsigned int offset = 0; offset < aSamplesToRead;)
        {
            const unsigned int frames = llmin(aSamplesToRead - offset, 512u);
            mState.generator.windGenerate(mScratch.data(), frames);
            for (unsigned int frame = 0; frame < frames; ++frame)
            {
                aBuffer[offset + frame] = mScratch[frame * 2];
                aBuffer[aBufferSize + offset + frame] = mScratch[frame * 2 + 1];
            }
            offset += frames;
        }
        return aSamplesToRead;
    }

    virtual bool hasEnded()
    {
        return false;
    }

private:
    LLSoLoudWindState& mState;
    std::array<float, 1024> mScratch;
};

class LLSoLoudWindSource : public SoLoud::AudioSource
{
public:
    LLSoLoudWindSource()
    {
        mChannels = 2;
        mBaseSamplerate = (float)mState.generator.getInputSamplingRate();
        setSingleInstance(true);
    }

    virtual ~LLSoLoudWindSource()
    {
        stop();
    }

    virtual SoLoud::AudioSourceInstance* createInstance()
    {
        return new LLSoLoudWindInstance(mState);
    }

    void setTargets(F32 freq, F32 gain, F32 pan_gain_r)
    {
        mState.frequency.store(freq, std::memory_order_relaxed);
        mState.gain.store(gain, std::memory_order_relaxed);
        mState.pan.store(pan_gain_r, std::memory_order_relaxed);
    }

private:
    LLSoLoudWindState mState;
};

// LLAudioBufferSoLoud::loadWAV
static bool load_wav(SoLoud::Wav& mWav, const char* filename)
{
    SoLoud::result res = mWav.load(filename);
    if (res == SoLoud::SO_NO_ERROR)
    {
        mWav.set3dAttenuation(SoLoud::AudioSource::INVERSE_DISTANCE, 1.f);
        mWav.set3dMinMaxDistance(1.f, SOLOUD_MAX_AUDIBLE_DISTANCE);
        const unsigned int FADE_SAMPLES = 64;
        if (mWav.mData && mWav.mSampleCount > FADE_SAMPLES * 4)
        {
            for (unsigned int ch = 0; ch < mWav.mChannels; ++ch)
            {
                float* chan_data = mWav.mData + ch * mWav.mSampleCount;
                for (unsigned int i = 0; i < FADE_SAMPLES; ++i)
                {
                    float fade_gain = (float)i / (float)FADE_SAMPLES;
                    chan_data[i] *= fade_gain;
                    chan_data[mWav.mSampleCount - 1 - i] *= fade_gain;
                }
            }
        }
        return true;
    }
    return false;
}

// ---------------------------------------------------------------- runner

struct Voice
{
    SoLoud::handle handle = 0;
    int wav = -1;
};

struct Runner
{
    std::unique_ptr<SoLoud::Soloud> soloud;
    std::map<int, std::unique_ptr<SoLoud::Wav>> wavs;
    std::map<int, Voice> voices;
    std::unique_ptr<LLSoLoudWindSource> wind;
    SoLoud::handle windHandle = 0;
    unsigned int channels = 2;
    FILE* out = nullptr;
    FILE* log = nullptr;
    bool timing = false;
    double bestMixNs = 0;
    double totalMixNs = 0;
    uint64_t mixCalls = 0;
    std::vector<float> buffer;

    // LLAudioChannelSoLoud::startVoicePaused (adapter)
    SoLoud::handle startVoicePaused(int wav, float gain, bool loop, bool positional,
                                    const float* p, float doppler, float rolloff)
    {
        SoLoud::handle h;
        SoLoud::Wav& w = *wavs[wav];
        if (!positional)
        {
            h = soloud->play(w, gain, 0.f, true /*paused*/);
        }
        else
        {
            h = soloud->play3d(w, p[0], p[1], p[2], p[3], p[4], p[5], gain, true /*paused*/);
            soloud->set3dSourceDopplerFactor(h, doppler);
            if (rolloff != 1.f)
            {
                soloud->set3dSourceAttenuation(h, SoLoud::AudioSource::INVERSE_DISTANCE, rolloff);
            }
        }
        soloud->setLooping(h, loop);
        soloud->setInaudibleBehavior(h, true, false);
        if (positional)
        {
            soloud->update3dAudio();
        }
        return h;
    }

    void mix(unsigned int frames, unsigned int chunk)
    {
        buffer.resize((size_t)chunk * channels);
        unsigned int done = 0;
        while (done < frames)
        {
            const unsigned int n = (frames - done) < chunk ? (frames - done) : chunk;
            // The viewer's audio thread runs with FTZ/DAZ set by SoLoud; its
            // main thread does not. Set them for the mix only.
            const unsigned int csr = _mm_getcsr();
            _mm_setcsr(csr | 0x8040);
            auto t0 = std::chrono::steady_clock::now();
            soloud->mix(buffer.data(), n);
            auto t1 = std::chrono::steady_clock::now();
            _mm_setcsr(csr);
            if (timing)
            {
                totalMixNs += std::chrono::duration<double, std::nano>(t1 - t0).count();
                ++mixCalls;
            }
            if (out)
            {
                fwrite(buffer.data(), sizeof(float), (size_t)n * channels, out);
            }
            done += n;
        }
    }

    bool run(const std::string& path)
    {
        std::ifstream in(path);
        if (!in)
        {
            fprintf(stderr, "cannot open %s\n", path.c_str());
            return false;
        }
        std::string line;
        int lineno = 0;
        while (std::getline(in, line))
        {
            ++lineno;
            if (line.empty() || line[0] == '#')
            {
                continue;
            }
            std::istringstream s(line);
            std::string cmd;
            s >> cmd;
            if (cmd == "init")
            {
                unsigned int rate;
                s >> rate >> channels;
                voices.clear();
                wind.reset();
                soloud.reset(new SoLoud::Soloud());
                SoLoud::result res = soloud->init(SoLoud::Soloud::CLIP_ROUNDOFF, SoLoud::Soloud::NULLDRIVER,
                                                  rate, SoLoud::Soloud::AUTO, channels);
                if (res != SoLoud::SO_NO_ERROR)
                {
                    fprintf(stderr, "init failed %u\n", res);
                    return false;
                }
                // LLAudioEngine_SoLoud::startBackend (adapter)
                soloud->setMaxActiveVoiceCount(64);
                if (soloud->getBackendChannels() == 8)
                {
                    soloud->setSpeakerPosition(4, 2.f, 0.f, -1.f);
                    soloud->setSpeakerPosition(5, -2.f, 0.f, -1.f);
                    soloud->setSpeakerPosition(6, 2.f, 0.f, 0.f);
                    soloud->setSpeakerPosition(7, -2.f, 0.f, 0.f);
                }
            }
            else if (cmd == "global")
            {
                float g;
                s >> g;
                soloud->setGlobalVolume(g);
            }
            else if (cmd == "wav")
            {
                int id;
                std::string file;
                s >> id;
                std::getline(s >> std::ws, file);  // the rest of the line (paths may contain spaces)
                std::unique_ptr<SoLoud::Wav> w(new SoLoud::Wav());
                if (!load_wav(*w, file.c_str()))
                {
                    fprintf(stderr, "line %d: cannot load %s\n", lineno, file.c_str());
                    return false;
                }
                wavs[id] = std::move(w);
            }
            else if (cmd == "listener")
            {
                float v[12];
                for (float& x : v) s >> x;
                // LLListener_SoLoud::commitDeferredChanges (adapter)
                soloud->set3dListenerParameters(v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8], v[9], v[10], v[11]);
                soloud->update3dAudio();
            }
            else if (cmd == "play2d" || cmd == "play3d")
            {
                int id, wav, loop;
                float gain, p[6] = {0, 0, 0, 0, 0, 0}, doppler = 1.f, rolloff = 1.f;
                s >> id >> wav >> gain >> loop;
                const bool positional = cmd == "play3d";
                if (positional)
                {
                    for (float& x : p) s >> x;
                    s >> doppler >> rolloff;
                }
                // LLAudioChannelSoLoud::play (adapter)
                SoLoud::handle h = startVoicePaused(wav, gain, loop != 0, positional, p, doppler, rolloff);
                if (h)
                {
                    soloud->setPause(h, false);
                }
                voices[id] = Voice{h, wav};
            }
            else if (cmd == "sync")
            {
                int id, master, wav, loop;
                float gain, p[6], doppler, rolloff;
                s >> id >> master >> wav >> gain >> loop;
                for (float& x : p) s >> x;
                s >> doppler >> rolloff;
                // LLAudioChannelSoLoud::playSynced (adapter)
                const Voice& m = voices[master];
                SoLoud::handle h = 0;
                if (m.handle && soloud->isValidVoiceHandle(m.handle))
                {
                    double master_offset = soloud->getStreamPosition(m.handle);
                    double master_length = wavs[m.wav]->getLength();
                    if (master_length > 0.0)
                    {
                        master_offset = fmod(master_offset, master_length);
                    }
                    h = startVoicePaused(wav, gain, loop != 0, true, p, doppler, rolloff);
                    if (h)
                    {
                        float target_volume = gain;
                        soloud->setVolume(h, 0.f);
                        soloud->seek(h, master_offset);
                        soloud->fadeVolume(h, target_volume, 0.02);
                        soloud->setPause(h, false);
                    }
                }
                else
                {
                    h = startVoicePaused(wav, gain, loop != 0, true, p, doppler, rolloff);
                    if (h)
                    {
                        soloud->setPause(h, false);
                    }
                }
                voices[id] = Voice{h, wav};
            }
            else if (cmd == "pos")
            {
                int id;
                float p[6], doppler, rolloff;
                s >> id;
                for (float& x : p) s >> x;
                s >> doppler >> rolloff;
                // LLAudioChannelSoLoud::update3DPosition (adapter)
                SoLoud::handle h = voices[id].handle;
                soloud->set3dSourceParameters(h, p[0], p[1], p[2], p[3], p[4], p[5]);
                soloud->set3dSourceDopplerFactor(h, doppler);
                soloud->set3dSourceAttenuation(h, SoLoud::AudioSource::INVERSE_DISTANCE, rolloff);
            }
            else if (cmd == "gain")
            {
                int id;
                float g;
                s >> id >> g;
                soloud->setVolume(voices[id].handle, g);
            }
            else if (cmd == "loop")
            {
                int id, l;
                s >> id >> l;
                soloud->setLooping(voices[id].handle, l != 0);
            }
            else if (cmd == "inaudible")
            {
                // Not an adapter call: lets a scene exercise SoLoud's
                // volume sort, which the adapter's all-ticking voices never
                // reach without crashing SoLoud (see the README).
                int id, tick, kill;
                s >> id >> tick >> kill;
                soloud->setInaudibleBehavior(voices[id].handle, tick != 0, kill != 0);
            }
            else if (cmd == "commit")
            {
                soloud->update3dAudio();
            }
            else if (cmd == "declick")
            {
                int id;
                s >> id;
                // stop_voice_declicked (adapter)
                const double DECLICK_FADE_SEC = 0.02;
                soloud->fadeVolume(voices[id].handle, 0.f, DECLICK_FADE_SEC);
                soloud->scheduleStop(voices[id].handle, DECLICK_FADE_SEC);
            }
            else if (cmd == "stop")
            {
                int id;
                s >> id;
                soloud->stop(voices[id].handle);
            }
            else if (cmd == "wind_on")
            {
                unsigned long long seed;
                s >> seed;
                oracle_seed_noise(seed);
                // LLAudioEngine_SoLoud::initWind (adapter)
                wind.reset(new LLSoLoudWindSource());
                windHandle = soloud->play(*wind, 1.0f);
                soloud->setProtectVoice(windHandle, true);
            }
            else if (cmd == "wind")
            {
                float f, g, p;
                s >> f >> g >> p;
                wind->setTargets(f, g, p);
            }
            else if (cmd == "wind_off")
            {
                // LLAudioEngine_SoLoud::cleanupWind (adapter)
                if (windHandle) soloud->stop(windHandle);
                windHandle = 0;
                wind.reset();
            }
            else if (cmd == "mix")
            {
                unsigned int frames, chunk;
                s >> frames >> chunk;
                mix(frames, chunk);
            }
            else if (cmd == "query")
            {
                int id;
                s >> id;
                SoLoud::handle h = voices[id].handle;
                double pos = soloud->getStreamPosition(h);
                float vol = soloud->getVolume(h);
                float ovol = soloud->getOverallVolume(h);
                uint64_t posbits;
                uint32_t volbits, ovolbits;
                memcpy(&posbits, &pos, 8);
                memcpy(&volbits, &vol, 4);
                memcpy(&ovolbits, &ovol, 4);
                if (log)
                {
                    fprintf(log, "query %d valid=%d loops=%u pos=%016llx vol=%08x ovol=%08x\n", id,
                            soloud->isValidVoiceHandle(h) ? 1 : 0, soloud->getLoopCount(h),
                            (unsigned long long)posbits, volbits, ovolbits);
                }
            }
            else if (cmd == "count")
            {
                if (log)
                {
                    fprintf(log, "count voices=%u\n", soloud->getVoiceCount());
                }
            }
            else
            {
                fprintf(stderr, "line %d: unknown command %s\n", lineno, cmd.c_str());
                return false;
            }
        }
        return true;
    }
};

static int wavdump(const char* in, const char* outPath)
{
    SoLoud::Wav w;
    SoLoud::result res = w.load(in);
    FILE* f = fopen(outPath, "wb");
    if (!f) return 2;
    uint32_t hdr[4] = {res, 0, 0, 0};
    if (res == SoLoud::SO_NO_ERROR)
    {
        hdr[1] = w.mChannels;
        hdr[2] = w.mSampleCount;
        memcpy(&hdr[3], &w.mBaseSamplerate, 4);
    }
    fwrite(hdr, 4, 4, f);
    if (res == SoLoud::SO_NO_ERROR)
    {
        fwrite(w.mData, sizeof(float), (size_t)w.mSampleCount * w.mChannels, f);
    }
    fclose(f);
    return 0;
}

int main(int argc, char** argv)
{
    if (argc >= 5 && strcmp(argv[1], "scene") == 0)
    {
        Runner r;
        r.out = fopen(argv[3], "wb");
        r.log = fopen(argv[4], "wb");
        if (!r.out || !r.log) return 2;
        bool ok = r.run(argv[2]);
        fclose(r.out);
        fclose(r.log);
        return ok ? 0 : 1;
    }
    if (argc >= 4 && strcmp(argv[1], "bench") == 0)
    {
        int reps = atoi(argv[3]);
        double best = 1e300;
        for (int i = 0; i < reps; ++i)
        {
            Runner r;
            r.timing = true;
            if (!r.run(argv[2])) return 1;
            double per = r.totalMixNs / (double)(r.mixCalls ? r.mixCalls : 1);
            if (per < best) best = per;
        }
        printf("%.1f\n", best);
        return 0;
    }
    if (argc >= 4 && strcmp(argv[1], "wavdump") == 0)
    {
        return wavdump(argv[2], argv[3]);
    }
    fprintf(stderr, "usage: soloud_oracle scene <scene> <out.f32> <out.log> | bench <scene> <reps> | wavdump <in.wav> <out.bin>\n");
    return 2;
}
