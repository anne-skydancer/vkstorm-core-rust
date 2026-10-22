// Minimal stand-in for the viewer's llcommon.h, enough to compile the
// viewer's unmodified indra/llaudio/llwindgen.h in the SoLoud oracle.
// Types and helpers are the viewer's definitions (stdtypes.h, llmath.h).
#ifndef ORACLE_LLCOMMON_H
#define ORACLE_LLCOMMON_H

#include <math.h>
#include <stdlib.h>
#include <stdint.h>

typedef float F32;
typedef double F64;
typedef int32_t S32;
typedef uint32_t U32;
typedef uint8_t U8;
typedef uint16_t U16;

#define U16_MAX 0xFFFF
#define S16_MIN (-32768)
#define S16_MAX 32767

// llmath.h
constexpr F32 F_TWO_PI = 6.283185307179586476925286766559f;
inline F32 llabs(const F32 a) { return F32(fabsf(a)); }
inline F64 llabs(const F64 a) { return F64(fabs(a)); }
inline S32 llabs(const S32 a) { return S32(labs(a)); }
template <class LLDATATYPE>
inline LLDATATYPE llclamp(const LLDATATYPE& a, const LLDATATYPE& minval, const LLDATATYPE& maxval)
{
    if (a < minval) return minval;
    if (a > maxval) return maxval;
    return a;
}
template <typename T>
inline T llmin(T a, T b) { return a < b ? a : b; }

#endif
