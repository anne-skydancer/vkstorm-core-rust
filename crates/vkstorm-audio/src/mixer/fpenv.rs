//! Flush-to-zero and denormals-are-zero around the effects mix.
//!
//! SoLoud's `mix_internal` sets MXCSR's FTZ and DAZ bits (`| 0x8040`) on the
//! thread that mixes, once, and never clears them; every float operation of
//! its mix (and of the wind generator it calls) therefore treats denormal
//! inputs and results as zero. Denormals do occur there (the wind gain
//! decays geometrically to zero, ramps end near zero), so bit-identical
//! output needs the same mode. [`DenormalsAreZero`] sets the bits for the
//! lifetime of the guard and restores the previous MXCSR on drop, so the
//! caller's thread is left as it was.
//!
//! This is the module's only `unsafe`: `stmxcsr`/`ldmxcsr` through inline
//! assembly (the `_mm_setcsr` intrinsic is deprecated in favour of it). The
//! compiler does not model the floating-point environment, so the guard is
//! only sound around code that does not depend on compile-time folding of
//! denormal arithmetic: the mix computes on runtime data only.

#[cfg(target_arch = "x86_64")]
pub(crate) struct DenormalsAreZero {
    saved: u32,
}

#[cfg(target_arch = "x86_64")]
impl DenormalsAreZero {
    const FTZ_DAZ: u32 = 0x8040;

    #[inline]
    pub(crate) fn new() -> Self {
        let mut saved: u32 = 0;
        // SAFETY: stmxcsr stores the 32-bit MXCSR into the valid, aligned
        // local; ldmxcsr loads a value that only differs from the current one
        // in the FTZ and DAZ bits, which are always writable.
        unsafe {
            std::arch::asm!("stmxcsr [{}]", in(reg) &mut saved, options(nostack, preserves_flags));
            let wanted = saved | Self::FTZ_DAZ;
            if wanted != saved {
                std::arch::asm!("ldmxcsr [{}]", in(reg) &wanted, options(nostack, preserves_flags));
            }
        }
        DenormalsAreZero { saved }
    }
}

#[cfg(target_arch = "x86_64")]
impl Drop for DenormalsAreZero {
    #[inline]
    fn drop(&mut self) {
        if self.saved & Self::FTZ_DAZ != Self::FTZ_DAZ {
            // SAFETY: restores the MXCSR value read in `new`.
            unsafe {
                std::arch::asm!("ldmxcsr [{}]", in(reg) &self.saved, options(nostack, preserves_flags));
            }
        }
    }
}

/// Other architectures: no-op (SoLoud only changes the mode on x86 and
/// 32-bit ARM).
#[cfg(not(target_arch = "x86_64"))]
pub(crate) struct DenormalsAreZero;

#[cfg(not(target_arch = "x86_64"))]
impl DenormalsAreZero {
    #[inline]
    pub(crate) fn new() -> Self {
        DenormalsAreZero
    }
}
