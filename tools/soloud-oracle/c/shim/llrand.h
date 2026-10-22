// Stand-in for the viewer's llrand.h: ll_frand() is the viewer's randomly
// seeded thread-local lagged Fibonacci generator; for a reproducible
// comparison the oracle and the Rust port share one deterministic generator
// (xorshift64*, 24-bit floats in [0, 1); vkstorm_audio::mixer::WindNoise).
#ifndef ORACLE_LLRAND_H
#define ORACLE_LLRAND_H

#include <stdint.h>

extern uint64_t g_oracle_noise_state;

inline void oracle_seed_noise(uint64_t seed)
{
    g_oracle_noise_state = seed ? seed : 0x9E3779B97F4A7C15ull;
}

inline float ll_frand()
{
    uint64_t x = g_oracle_noise_state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    g_oracle_noise_state = x;
    uint64_t r = x * 0x2545F4914F6CDD1Dull;
    return (float)(r >> 40) * (1.0f / 16777216.0f);
}

#endif
