//! The packet-level decoder (`vorbis_synthesis_headerin`,
//! `vorbis_synthesis_init`, `vorbis_synthesis` + `vorbis_synthesis_blockin`,
//! `vorbis_synthesis_pcmout`, `vorbis_synthesis_read`,
//! `vorbis_synthesis_restart`), for streams demuxed with [`crate::ogg`].

use crate::Error;
use crate::dsp::DspState;
use crate::info::{Comment, Info, synthesis_headerin};
use crate::ogg::Packet;

/// Decodes one logical Vorbis stream from its packets.
///
/// Feed the three header packets to [`Decoder::headerin`], then the audio
/// packets to [`Decoder::synthesis`]; after each, drain the samples with
/// [`Decoder::pcmout`] and [`Decoder::read`]. A chained stream (a new
/// serial number) needs a new `Decoder`.
#[derive(Debug)]
pub struct Decoder {
    info: Info,
    comment: Comment,
    dsp: Option<DspState>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

/// The samples ready after a [`Decoder::synthesis`]: one `f32` slice per
/// channel (`vorbis_synthesis_pcmout`).
#[derive(Debug)]
pub struct PcmOut<'a> {
    dsp: Option<&'a DspState>,
    len: usize,
}

impl<'a> PcmOut<'a> {
    /// Samples per channel.
    pub fn len(&self) -> usize {
        self.len
    }
    /// No samples ready.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Channel count.
    pub fn channels(&self) -> usize {
        self.dsp.map_or(0, |d| d.channels())
    }
    /// The samples of channel `ch`.
    pub fn channel(&self, ch: usize) -> &'a [f32] {
        self.dsp.map_or(&[], |d| d.pcm_channel(ch))
    }
}

impl Decoder {
    /// An empty decoder (`vorbis_info_init` + `vorbis_comment_init`).
    pub fn new() -> Self {
        Self {
            info: Info::new(),
            comment: Comment::new(),
            dsp: None,
        }
    }

    /// `vorbis_synthesis_headerin` for one header packet (`b_o_s`: the
    /// packet is the first of its logical stream).
    pub fn headerin(&mut self, packet: &[u8], b_o_s: bool) -> Result<(), Error> {
        Error::check(synthesis_headerin(
            &mut self.info,
            &mut self.comment,
            packet,
            b_o_s,
        ))
    }

    /// [`Decoder::headerin`] for a demuxed packet.
    pub fn headerin_packet(&mut self, packet: &Packet<'_>) -> Result<(), Error> {
        self.headerin(packet.data, packet.b_o_s)
    }

    /// The identification header's information.
    pub fn info(&self) -> &Info {
        &self.info
    }

    /// The comment header.
    pub fn comment(&self) -> &Comment {
        &self.comment
    }

    /// All three headers were accepted (the setup header is loaded).
    pub fn headers_complete(&self) -> bool {
        self.info
            .codec_setup
            .as_ref()
            .is_some_and(|ci| ci.books > 0)
    }

    /// `vorbis_synthesis_init`; done implicitly by the first
    /// [`Decoder::synthesis`]. `OV_EBADLINK` when the setup is unusable.
    pub fn synthesis_init(&mut self) -> Result<(), Error> {
        if self.dsp.is_none() {
            if !self.headers_complete() {
                return Err(Error::Inval);
            }
            self.dsp = Some(DspState::init(&mut self.info).ok_or(Error::BadLink)?);
        }
        Ok(())
    }

    /// Decodes an audio packet (`vorbis_synthesis`, then
    /// `vorbis_synthesis_blockin` when that succeeded). `granulepos`,
    /// `packetno` and `e_o_s` come from the Ogg packet. Fails with
    /// `OV_ENOTAUDIO`/`OV_EBADPACKET` for a packet the C rejects (the decoder
    /// state is unchanged, as in the C) and `OV_EINVAL` when samples of the
    /// previous packet are still unread.
    pub fn synthesis_raw(
        &mut self,
        packet: &[u8],
        granulepos: i64,
        packetno: i64,
        e_o_s: bool,
    ) -> Result<(), Error> {
        self.synthesis_init()?;
        let ci = self.info.codec_setup.as_deref().ok_or(Error::Fault)?;
        let dsp = self.dsp.as_mut().ok_or(Error::Fault)?;
        Error::check(dsp.synthesis(ci, packet, granulepos, packetno, e_o_s))?;
        Error::check(dsp.blockin(ci))
    }

    /// [`Decoder::synthesis_raw`] for a demuxed packet.
    pub fn synthesis(&mut self, packet: &Packet<'_>) -> Result<(), Error> {
        self.synthesis_raw(
            packet.data,
            packet.granulepos,
            packet.packetno,
            packet.e_o_s,
        )
    }

    /// `vorbis_synthesis_pcmout`: the decoded samples not yet consumed.
    pub fn pcmout(&self) -> PcmOut<'_> {
        match &self.dsp {
            Some(d) => PcmOut {
                dsp: Some(d),
                len: d.pcmout_count() as usize,
            },
            None => PcmOut { dsp: None, len: 0 },
        }
    }

    /// `vorbis_synthesis_read`: marks `n` samples per channel consumed.
    pub fn read(&mut self, n: usize) -> Result<(), Error> {
        match &mut self.dsp {
            Some(d) => Error::check(d.read(i32::try_from(n).map_err(|_| Error::Inval)?)),
            None if n == 0 => Ok(()),
            None => Err(Error::Inval),
        }
    }

    /// `vorbis_synthesis_restart`: forget the overlap and granule state
    /// (after a seek or a hole the application wants to treat as a cut).
    pub fn restart(&mut self) -> Result<(), Error> {
        let (Some(d), Some(ci)) = (&mut self.dsp, self.info.codec_setup.as_deref()) else {
            return Err(Error::Inval);
        };
        d.restart(ci);
        Ok(())
    }

    /// The MDCT trig table (0 short, 1 long) after `synthesis_init`, for
    /// parity tests against the C library.
    #[doc(hidden)]
    pub fn mdct_trig(&self, which: usize) -> Option<&[f32]> {
        self.dsp.as_ref().map(|d| d.mdct_trig(which))
    }
}
