//! Floors: type 0 (LSP, `floor0.c` + `lsp.c`) and type 1 (piecewise
//! linear, `floor1.c`), unpack, look-up setup and inverse, ported from
//! libvorbis 1.3.7.
//!
//! Floor 0 builds its Bark map and evaluates the LSP curve with libm
//! (`atan`, `cos`, `exp`, `sqrt` in double) exactly as the C does, so its
//! output follows the platform's C library; with MSVC `M_PI` is the float
//! constant `3.1415926536f` from `os.h` (MSVC's `math.h` defines no `M_PI`
//! without `_USE_MATH_DEFINES`), elsewhere the double from `math.h`.
//!
//! Original code: Copyright (c) 1994-2015 Xiph.Org Foundation, BSD-3-Clause
//! (see `LICENSE-xiph`). Rust port: Vulkanstorm.

use crate::bitpack::BitReader;
use crate::codebook::{Codebook, StaticCodebook, ov_ilog};

const VIF_POSIT: usize = 63;
const VIF_CLASS: usize = 16;
const VIF_PARTS: usize = 31;

/// `vorbis_info_floor0`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Floor0Info {
    pub order: i32,
    pub rate: i64,
    pub barkmap: i64,
    pub ampbits: i32,
    pub amp_db: i32,
    pub numbooks: i32,
    pub books: [i32; 16],
}

/// `vorbis_info_floor1`.
#[derive(Clone, Debug)]
pub(crate) struct Floor1Info {
    pub partitions: i32,
    pub partitionclass: [i32; VIF_PARTS],
    pub class_dim: [i32; VIF_CLASS],
    pub class_subs: [i32; VIF_CLASS],
    pub class_book: [i32; VIF_CLASS],
    pub class_subbook: [[i32; 8]; VIF_CLASS],
    pub mult: i32,
    pub postlist: [i32; VIF_POSIT + 2],
}

impl Default for Floor1Info {
    fn default() -> Self {
        Self {
            partitions: 0,
            partitionclass: [0; VIF_PARTS],
            class_dim: [0; VIF_CLASS],
            class_subs: [0; VIF_CLASS],
            class_book: [0; VIF_CLASS],
            class_subbook: [[0; 8]; VIF_CLASS],
            mult: 0,
            postlist: [0; VIF_POSIT + 2],
        }
    }
}

/// A floor configuration from the setup header.
#[derive(Clone, Debug)]
pub(crate) enum FloorInfo {
    Zero(Floor0Info),
    One(Box<Floor1Info>),
}

/// `floor0_unpack`.
pub(crate) fn floor0_unpack(
    books: &[Option<StaticCodebook>],
    nbooks: i32,
    opb: &mut BitReader,
) -> Option<Floor0Info> {
    let mut info = Floor0Info {
        order: opb.read(8) as i32,
        rate: opb.read(16),
        barkmap: opb.read(16),
        ampbits: opb.read(6) as i32,
        amp_db: opb.read(8) as i32,
        numbooks: (opb.read(4) + 1) as i32,
        books: [0; 16],
    };
    if info.order < 1 || info.rate < 1 || info.barkmap < 1 || info.numbooks < 1 {
        return None;
    }
    for j in 0..info.numbooks as usize {
        info.books[j] = opb.read(8) as i32;
        if info.books[j] < 0 || info.books[j] >= nbooks {
            return None;
        }
        let b = books[info.books[j] as usize].as_ref()?;
        if b.maptype == 0 || b.dim < 1 {
            return None;
        }
    }
    Some(info)
}

/// `floor1_unpack`.
pub(crate) fn floor1_unpack(nbooks: i32, opb: &mut BitReader) -> Option<Box<Floor1Info>> {
    let mut info = Box::<Floor1Info>::default();
    let mut maxclass: i32 = -1;

    info.partitions = opb.read(5) as i32;
    for j in 0..info.partitions.max(0) as usize {
        info.partitionclass[j] = opb.read(4) as i32;
        if info.partitionclass[j] < 0 {
            return None;
        }
        if maxclass < info.partitionclass[j] {
            maxclass = info.partitionclass[j];
        }
    }

    for j in 0..(maxclass + 1).max(0) as usize {
        info.class_dim[j] = (opb.read(3) + 1) as i32;
        info.class_subs[j] = opb.read(2) as i32;
        if info.class_subs[j] < 0 {
            return None;
        }
        if info.class_subs[j] != 0 {
            info.class_book[j] = opb.read(8) as i32;
        }
        if info.class_book[j] < 0 || info.class_book[j] >= nbooks {
            return None;
        }
        for k in 0..(1usize << info.class_subs[j]) {
            info.class_subbook[j][k] = (opb.read(8) - 1) as i32;
            if info.class_subbook[j][k] < -1 || info.class_subbook[j][k] >= nbooks {
                return None;
            }
        }
    }

    info.mult = (opb.read(2) + 1) as i32;
    let rangebits = opb.read(4) as i32;
    if rangebits < 0 {
        return None;
    }

    let mut count: usize = 0;
    let mut k: usize = 0;
    for j in 0..info.partitions.max(0) as usize {
        count += info.class_dim[info.partitionclass[j] as usize] as usize;
        if count > VIF_POSIT {
            return None;
        }
        while k < count {
            let t = opb.read(rangebits) as i32;
            info.postlist[k + 2] = t;
            if t < 0 || t >= (1 << rangebits) {
                return None;
            }
            k += 1;
        }
    }
    info.postlist[0] = 0;
    info.postlist[1] = 1 << rangebits;

    // don't allow repeated values in post list
    let mut sorted: Vec<i32> = info.postlist[..count + 2].to_vec();
    sorted.sort_unstable();
    if sorted.windows(2).any(|w| w[0] == w[1]) {
        return None;
    }
    Some(info)
}

/// `vorbis_look_floor0`, with the lazily built Bark maps.
#[derive(Clone, Debug)]
pub(crate) struct Floor0Look {
    ln: i32,
    m: i32,
    linearmap: [Option<Vec<i32>>; 2],
    n: [i32; 2],
}

/// `vorbis_look_floor1`.
#[derive(Clone, Debug)]
pub(crate) struct Floor1Look {
    forward_index: [i32; VIF_POSIT + 2],
    hineighbor: [i32; VIF_POSIT],
    loneighbor: [i32; VIF_POSIT],
    posts: i32,
    quant_q: i32,
}

/// A floor's decode-time look-up.
#[derive(Clone, Debug)]
pub(crate) enum FloorLook {
    Zero(Floor0Look),
    One(Box<Floor1Look>),
}

/// `floor0_look` / `floor1_look`.
pub(crate) fn floor_look(info: &FloorInfo) -> FloorLook {
    match info {
        FloorInfo::Zero(i) => FloorLook::Zero(Floor0Look {
            ln: i.barkmap as i32,
            m: i.order,
            linearmap: [None, None],
            n: [0, 0],
        }),
        FloorInfo::One(info) => {
            let mut look = Box::new(Floor1Look {
                forward_index: [0; VIF_POSIT + 2],
                hineighbor: [0; VIF_POSIT],
                loneighbor: [0; VIF_POSIT],
                posts: 0,
                quant_q: 0,
            });
            let mut n: usize = 0;
            for i in 0..info.partitions.max(0) as usize {
                n += info.class_dim[info.partitionclass[i] as usize] as usize;
            }
            n += 2;
            look.posts = n as i32;
            // sorted position index (the posts are distinct)
            let mut order: Vec<usize> = (0..n).collect();
            order.sort_by_key(|&i| info.postlist[i]);
            for (i, &o) in order.iter().enumerate() {
                look.forward_index[i] = o as i32;
            }
            look.quant_q = match info.mult {
                1 => 256,
                2 => 128,
                3 => 86,
                4 => 64,
                _ => 0,
            };
            for i in 0..n - 2 {
                let mut lo = 0;
                let mut hi = 1;
                let mut lx = 0;
                let mut hx = info.postlist[1];
                let currentx = info.postlist[i + 2];
                for j in 0..i + 2 {
                    let x = info.postlist[j];
                    if x > lx && x < currentx {
                        lo = j;
                        lx = x;
                    }
                    if x < hx && x > currentx {
                        hi = j;
                        hx = x;
                    }
                }
                look.loneighbor[i] = lo as i32;
                look.hineighbor[i] = hi as i32;
            }
            FloorLook::One(look)
        }
    }
}

/// Per-channel result of the first floor pass (`floormemo`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FloorMemo {
    /// No floor for this channel in this packet (NULL memo).
    Unused,
    /// The memo is in the channel's scratch vector.
    Used,
}

/// Scratch space for the floor memos of one channel.
#[derive(Clone, Debug, Default)]
pub(crate) struct FloorScratch {
    pub fit: Vec<i32>,
    pub lsp: Vec<f32>,
}

fn render_point(x0: i32, x1: i32, y0: i32, y1: i32, x: i32) -> i32 {
    let y0 = y0 & 0x7fff;
    let y1 = y1 & 0x7fff;
    let dy = y1 - y0;
    let adx = x1 - x0;
    let ady = dy.abs();
    let err = ady.wrapping_mul(x - x0);
    let off = if adx != 0 { err / adx } else { 0 };
    if dy < 0 { y0 - off } else { y0 + off }
}

/// `floor1_inverse1`: unpacks the post values; `Unused` on end of packet.
pub(crate) fn floor1_inverse1(
    info: &Floor1Info,
    look: &Floor1Look,
    books: &[Codebook],
    opb: &mut BitReader,
    fit_value: &mut Vec<i32>,
) -> FloorMemo {
    if opb.read(1) != 1 {
        return FloorMemo::Unused;
    }
    let posts = look.posts as usize;
    fit_value.clear();
    fit_value.resize(posts, 0);
    let bits = ov_ilog((look.quant_q - 1) as u32);
    fit_value[0] = opb.read(bits) as i32;
    fit_value[1] = opb.read(bits) as i32;

    let mut j = 2usize;
    for i in 0..info.partitions as usize {
        let class = info.partitionclass[i] as usize;
        let cdim = info.class_dim[class] as usize;
        let csubbits = info.class_subs[class];
        let csub = 1i32 << csubbits;
        let mut cval: i32 = 0;
        if csubbits != 0 {
            cval = books[info.class_book[class] as usize].decode(opb) as i32;
            if cval == -1 {
                return FloorMemo::Unused;
            }
        }
        for k in 0..cdim {
            let book = info.class_subbook[class][(cval & (csub - 1)) as usize];
            cval >>= csubbits;
            if book >= 0 {
                let v = books[book as usize].decode(opb) as i32;
                fit_value[j + k] = v;
                if v == -1 {
                    return FloorMemo::Unused;
                }
            } else {
                fit_value[j + k] = 0;
            }
        }
        j += cdim;
    }

    for i in 2..posts {
        let lo = look.loneighbor[i - 2] as usize;
        let hi = look.hineighbor[i - 2] as usize;
        let predicted = render_point(
            info.postlist[lo],
            info.postlist[hi],
            fit_value[lo],
            fit_value[hi],
            info.postlist[i],
        );
        let hiroom = look.quant_q - predicted;
        let loroom = predicted;
        let room = (if hiroom < loroom { hiroom } else { loroom }) << 1;
        let mut val = fit_value[i];
        if val != 0 {
            if val >= room {
                if hiroom > loroom {
                    val -= loroom;
                } else {
                    val = -1 - (val - hiroom);
                }
            } else if val & 1 != 0 {
                val = -((val + 1) >> 1);
            } else {
                val >>= 1;
            }
            fit_value[i] = (val + predicted) & 0x7fff;
            fit_value[lo] &= 0x7fff;
            fit_value[hi] &= 0x7fff;
        } else {
            fit_value[i] = predicted | 0x8000;
        }
    }
    FloorMemo::Used
}

// The literals of floor1.c, verbatim.
#[rustfmt::skip]
static FLOOR1_FROMDB_LOOKUP: [f32; 256] = [
  1.0649863e-07, 1.1341951e-07, 1.2079015e-07, 1.2863978e-07,
  1.3699951e-07, 1.4590251e-07, 1.5538408e-07, 1.6548181e-07,
  1.7623575e-07, 1.8768855e-07, 1.9988561e-07, 2.128753e-07,
  2.2670913e-07, 2.4144197e-07, 2.5713223e-07, 2.7384213e-07,
  2.9163793e-07, 3.1059021e-07, 3.3077411e-07, 3.5226968e-07,
  3.7516214e-07, 3.9954229e-07, 4.2550680e-07, 4.5315863e-07,
  4.8260743e-07, 5.1396998e-07, 5.4737065e-07, 5.8294187e-07,
  6.2082472e-07, 6.6116941e-07, 7.0413592e-07, 7.4989464e-07,
  7.9862701e-07, 8.5052630e-07, 9.0579828e-07, 9.6466216e-07,
  1.0273513e-06, 1.0941144e-06, 1.1652161e-06, 1.2409384e-06,
  1.3215816e-06, 1.4074654e-06, 1.4989305e-06, 1.5963394e-06,
  1.7000785e-06, 1.8105592e-06, 1.9282195e-06, 2.0535261e-06,
  2.1869758e-06, 2.3290978e-06, 2.4804557e-06, 2.6416497e-06,
  2.8133190e-06, 2.9961443e-06, 3.1908506e-06, 3.3982101e-06,
  3.6190449e-06, 3.8542308e-06, 4.1047004e-06, 4.3714470e-06,
  4.6555282e-06, 4.9580707e-06, 5.2802740e-06, 5.6234160e-06,
  5.9888572e-06, 6.3780469e-06, 6.7925283e-06, 7.2339451e-06,
  7.7040476e-06, 8.2047000e-06, 8.7378876e-06, 9.3057248e-06,
  9.9104632e-06, 1.0554501e-05, 1.1240392e-05, 1.1970856e-05,
  1.2748789e-05, 1.3577278e-05, 1.4459606e-05, 1.5399272e-05,
  1.6400004e-05, 1.7465768e-05, 1.8600792e-05, 1.9809576e-05,
  2.1096914e-05, 2.2467911e-05, 2.3928002e-05, 2.5482978e-05,
  2.7139006e-05, 2.8902651e-05, 3.0780908e-05, 3.2781225e-05,
  3.4911534e-05, 3.7180282e-05, 3.9596466e-05, 4.2169667e-05,
  4.4910090e-05, 4.7828601e-05, 5.0936773e-05, 5.4246931e-05,
  5.7772202e-05, 6.1526565e-05, 6.5524908e-05, 6.9783085e-05,
  7.4317983e-05, 7.9147585e-05, 8.4291040e-05, 8.9768747e-05,
  9.5602426e-05, 0.00010181521, 0.00010843174, 0.00011547824,
  0.00012298267, 0.00013097477, 0.00013948625, 0.00014855085,
  0.00015820453, 0.00016848555, 0.00017943469, 0.00019109536,
  0.00020351382, 0.00021673929, 0.00023082423, 0.00024582449,
  0.00026179955, 0.00027881276, 0.00029693158, 0.00031622787,
  0.00033677814, 0.00035866388, 0.00038197188, 0.00040679456,
  0.00043323036, 0.00046138411, 0.00049136745, 0.00052329927,
  0.00055730621, 0.00059352311, 0.00063209358, 0.00067317058,
  0.00071691700, 0.00076350630, 0.00081312324, 0.00086596457,
  0.00092223983, 0.00098217216, 0.0010459992, 0.0011139742,
  0.0011863665, 0.0012634633, 0.0013455702, 0.0014330129,
  0.0015261382, 0.0016253153, 0.0017309374, 0.0018434235,
  0.0019632195, 0.0020908006, 0.0022266726, 0.0023713743,
  0.0025254795, 0.0026895994, 0.0028643847, 0.0030505286,
  0.0032487691, 0.0034598925, 0.0036847358, 0.0039241906,
  0.0041792066, 0.0044507950, 0.0047400328, 0.0050480668,
  0.0053761186, 0.0057254891, 0.0060975636, 0.0064938176,
  0.0069158225, 0.0073652516, 0.0078438871, 0.0083536271,
  0.0088964928, 0.009474637, 0.010090352, 0.010746080,
  0.011444421, 0.012188144, 0.012980198, 0.013823725,
  0.014722068, 0.015678791, 0.016697687, 0.017782797,
  0.018938423, 0.020169149, 0.021479854, 0.022875735,
  0.024362330, 0.025945531, 0.027631618, 0.029427276,
  0.031339626, 0.033376252, 0.035545228, 0.037855157,
  0.040315199, 0.042935108, 0.045725273, 0.048696758,
  0.051861348, 0.055231591, 0.058820850, 0.062643361,
  0.066714279, 0.071049749, 0.075666962, 0.080584227,
  0.085821044, 0.091398179, 0.097337747, 0.10366330,
  0.11039993, 0.11757434, 0.12521498, 0.13335215,
  0.14201813, 0.15124727, 0.16107617, 0.17154380,
  0.18269168, 0.19456402, 0.20720788, 0.22067342,
  0.23501402, 0.25028656, 0.26655159, 0.28387361,
  0.30232132, 0.32196786, 0.34289114, 0.36517414,
  0.38890521, 0.41417847, 0.44109412, 0.46975890,
  0.50028648, 0.53279791, 0.56742212, 0.60429640,
  0.64356699, 0.68538959, 0.72993007, 0.77736504,
  0.82788260, 0.88168307, 0.9389798, 1.,
];

/// `render_line`: multiplies `d[x0..min(n,x1))` by the dB curve.
#[inline]
fn render_line(n: i32, x0: i32, x1: i32, y0: i32, y1: i32, d: &mut [f32]) {
    let dy = y1 - y0;
    let adx = x1 - x0;
    let mut ady = dy.abs();
    let base = dy / adx;
    let sy = if dy < 0 { base - 1 } else { base + 1 };
    let mut x = x0;
    let mut y = y0;
    let mut err = 0;
    ady -= (base * adx).abs();
    let n = n.min(x1);
    if x < n {
        d[x as usize] *= FLOOR1_FROMDB_LOOKUP[y as usize];
    }
    loop {
        x += 1;
        if x >= n {
            break;
        }
        err += ady;
        if err >= adx {
            err -= adx;
            y += sy;
        } else {
            y += base;
        }
        d[x as usize] *= FLOOR1_FROMDB_LOOKUP[y as usize];
    }
}

/// `floor1_inverse2`: applies the curve to the residue spectrum
/// (`n` = half the block size).
pub(crate) fn floor1_inverse2(
    info: &Floor1Info,
    look: &Floor1Look,
    memo: FloorMemo,
    fit_value: &[i32],
    n: i32,
    out: &mut [f32],
) -> i32 {
    if memo == FloorMemo::Used {
        let mut hx = 0;
        let mut lx = 0;
        let ly0 = fit_value[0].wrapping_mul(info.mult);
        let mut ly = ly0.clamp(0, 255);
        for j in 1..look.posts as usize {
            let current = look.forward_index[j] as usize;
            let mut hy = fit_value[current] & 0x7fff;
            if hy == fit_value[current] {
                hx = info.postlist[current];
                hy = hy.wrapping_mul(info.mult).clamp(0, 255);
                render_line(n, lx, hx, ly, hy, out);
                lx = hx;
                ly = hy;
            }
        }
        let curve = FLOOR1_FROMDB_LOOKUP[ly as usize];
        if hx < n {
            for v in out[hx as usize..n as usize].iter_mut() {
                *v *= curve;
            }
        }
        return 1;
    }
    for v in out[..n as usize].iter_mut() {
        *v = 0.0;
    }
    0
}

/// `floor0_inverse1`: the LSP coefficients and amplitude; `Unused` when the
/// channel has no floor or the packet ends.
pub(crate) fn floor0_inverse1(
    info: &Floor0Info,
    look: &Floor0Look,
    books: &[Codebook],
    opb: &mut BitReader,
    lsp: &mut Vec<f32>,
) -> FloorMemo {
    let ampraw = opb.read(info.ampbits) as i32;
    if ampraw > 0 {
        // `1<<ampbits` on x86 shifts by the count modulo 32.
        let maxval: i64 = (1i32.wrapping_shl(info.ampbits as u32) - 1) as i64;
        let amp = (ampraw as f32) / (maxval as f32) * (info.amp_db as f32);
        let booknum = opb.read(ov_ilog(info.numbooks as u32)) as i32;
        if booknum != -1 && booknum < info.numbooks {
            let b = &books[info.books[booknum as usize] as usize];
            let m = look.m as usize;
            lsp.clear();
            lsp.resize(m + b.dim.max(0) as usize + 1, 0.0);
            if b.decodev_set(lsp, opb, look.m) == -1 {
                return FloorMemo::Unused;
            }
            let dim = b.dim.max(0) as usize;
            let mut last = 0f32;
            let mut j = 0usize;
            while j < m {
                let mut k = 0usize;
                while j < m && k < dim {
                    lsp[j] += last;
                    k += 1;
                    j += 1;
                }
                if dim == 0 {
                    // the C loops forever here; a zero-dimension book is
                    // rejected by floor0_unpack, so this is unreachable
                    return FloorMemo::Unused;
                }
                last = lsp[j - 1];
            }
            lsp[m] = amp;
            return FloorMemo::Used;
        }
    }
    FloorMemo::Unused
}

/// `M_PI` as the C sees it (see the module docs).
#[cfg(all(windows, target_env = "msvc"))]
#[inline]
fn m_pi_over(den: i32) -> f32 {
    3.141_592_653_6_f32 / den as f32
}
#[cfg(not(all(windows, target_env = "msvc")))]
#[inline]
fn m_pi_over(den: i32) -> f32 {
    (std::f64::consts::PI / den as f64) as f32
}

/// `toBARK(n)` for a float argument (the macro's mixed float/double
/// evaluation).
#[inline]
fn to_bark(n: f32) -> f64 {
    13.1f32 as f64 * ((0.00074f32 * n) as f64).atan()
        + 2.24f32 as f64 * ((n * n * 1.85e-8f32) as f64).atan()
        + (1e-4f32 * n) as f64
}

/// `floor0_map_lazy_init`.
fn floor0_map_lazy_init(info: &Floor0Info, look: &mut Floor0Look, blocksize: i64, w: usize) {
    if look.linearmap[w].is_none() {
        let n = (blocksize / 2) as i32;
        let half_rate = info.rate as f32 / 2.0f32;
        let scale = (look.ln as f64 / to_bark(half_rate)) as f32;
        let mut map = vec![0i32; n as usize + 1];
        for j in 0..n {
            let x = half_rate / n as f32 * j as f32;
            let mut val = (to_bark(x) * scale as f64).floor() as i32;
            if val >= look.ln {
                val = look.ln - 1;
            }
            map[j as usize] = val;
        }
        map[n as usize] = -1;
        look.linearmap[w] = Some(map);
        look.n[w] = n;
    }
}

/// `vorbis_lsp_to_curve` (the plain float version libvorbis builds).
fn lsp_to_curve(
    curve: &mut [f32],
    map: &[i32],
    n: i32,
    ln: i32,
    lsp: &mut [f32],
    m: i32,
    amp: f32,
    ampoffset: f32,
) {
    let wdel = m_pi_over(ln);
    let m = m as usize;
    for v in lsp[..m].iter_mut() {
        *v = (2.0f32 as f64 * (*v as f64).cos()) as f32;
    }
    let mut i = 0usize;
    let n = n as usize;
    while i < n {
        let k = map[i];
        let mut p = 0.5f32;
        let mut q = 0.5f32;
        let w = (2.0f32 as f64 * ((wdel * k as f32) as f64).cos()) as f32;
        let mut j = 1usize;
        while j < m {
            q *= w - lsp[j - 1];
            p *= w - lsp[j];
            j += 2;
        }
        if j == m {
            q *= w - lsp[j - 1];
            p *= p * (4.0f32 - w * w);
            q *= q;
        } else {
            p *= p * (2.0f32 - w);
            q *= q * (2.0f32 + w);
        }
        // fromdB(amp/sqrt(p+q)-ampoffset) = exp((x)*.11512925f), in double
        let x = amp as f64 / ((p + q) as f64).sqrt() - ampoffset as f64;
        let q = (x * 0.115_129_25_f32 as f64).exp() as f32;

        curve[i] *= q;
        i += 1;
        while map[i] == k {
            curve[i] *= q;
            i += 1;
        }
    }
}

/// `floor0_inverse2`.
pub(crate) fn floor0_inverse2(
    info: &Floor0Info,
    look: &mut Floor0Look,
    blocksize: i64,
    w: usize,
    memo: FloorMemo,
    lsp: &mut [f32],
    out: &mut [f32],
) -> i32 {
    floor0_map_lazy_init(info, look, blocksize, w);
    let n = look.n[w];
    if memo == FloorMemo::Used {
        let amp = lsp[look.m as usize];
        let map = look.linearmap[w].as_deref().unwrap_or(&[]);
        lsp_to_curve(out, map, n, look.ln, lsp, look.m, amp, info.amp_db as f32);
        return 1;
    }
    for v in out[..n as usize].iter_mut() {
        *v = 0.0;
    }
    0
}
