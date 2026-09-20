//! Ogg Opus (RFC 7845): the `OpusHead`/`OpusTags` header packets and a writer
//! that tracks the 48 kHz granule position, ends a page every second so a dead
//! daemon leaves playable files (53 §0.12), and ends the stream with EOS.

use std::io::{self, Write};

use ::ogg::{PacketWriteEndInfo, PacketWriter};

/// Encoder sample rate.
pub const RATE: u32 = 24_000;
/// Samples per Opus frame: 20 ms at [`RATE`].
pub const FRAME: usize = RATE as usize / 50;
/// Granule units per frame: 20 ms at the fixed 48 kHz granule rate (RFC 7845 §4).
pub const GRANULE_PER_FRAME: u64 = 960;
/// Frames per forced page end: 1 s.
const FRAMES_PER_PAGE: u32 = 50;

/// `OpusHead` (RFC 7845 §5.1): version 1, mono, `pre_skip` at 48 kHz, the
/// original input rate, 0 dB gain, mapping family 0.
pub fn opus_head(pre_skip: u16, input_rate: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(19);
    v.extend_from_slice(b"OpusHead");
    v.push(1);
    v.push(1);
    v.extend_from_slice(&pre_skip.to_le_bytes());
    v.extend_from_slice(&input_rate.to_le_bytes());
    v.extend_from_slice(&0i16.to_le_bytes());
    v.push(0);
    v
}

/// `OpusTags` (RFC 7845 §5.2): vendor string and `KEY=value` comments.
pub fn opus_tags(vendor: &str, comments: &[(&str, &str)]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"OpusTags");
    v.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    v.extend_from_slice(vendor.as_bytes());
    v.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for (k, val) in comments {
        let c = format!("{k}={val}");
        v.extend_from_slice(&(c.len() as u32).to_le_bytes());
        v.extend_from_slice(c.as_bytes());
    }
    v
}

/// One logical Opus stream in an Ogg container.
pub struct Writer<W: Write> {
    pw: PacketWriter<'static, W>,
    serial: u32,
    granule: u64,
    in_page: u32,
}

impl<W: Write> Writer<W> {
    /// Write both header pages. `comments` become `OpusTags`.
    pub fn new(w: W, serial: u32, pre_skip: u16, comments: &[(&str, &str)]) -> io::Result<Self> {
        let mut pw = PacketWriter::new(w);
        // Input rate is informational (decoders output 48 kHz regardless); it is what we fed libopus.
        pw.write_packet(
            opus_head(pre_skip, RATE),
            serial,
            PacketWriteEndInfo::EndPage,
            0,
        )?;
        pw.write_packet(
            opus_tags("theuth", comments),
            serial,
            PacketWriteEndInfo::EndPage,
            0,
        )?;
        Ok(Self {
            pw,
            serial,
            granule: 0,
            in_page: 0,
        })
    }

    /// One 20 ms Opus packet; `last` ends the stream (EOS).
    pub fn packet(&mut self, data: Vec<u8>, last: bool) -> io::Result<()> {
        self.granule += GRANULE_PER_FRAME;
        self.in_page += 1;
        let end = if last {
            PacketWriteEndInfo::EndStream
        } else if self.in_page == FRAMES_PER_PAGE {
            self.in_page = 0;
            PacketWriteEndInfo::EndPage
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        self.pw.write_packet(data, self.serial, end, self.granule)?;
        if !matches!(end, PacketWriteEndInfo::NormalPacket) {
            self.pw.inner_mut().flush()?;
        }
        Ok(())
    }

    /// Granule position after the last packet.
    pub fn granule(&self) -> u64 {
        self.granule
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::ogg::PacketReader;
    use std::io::Cursor;

    #[test]
    fn opus_head_bytes() {
        let h = opus_head(312, 48_000);
        assert_eq!(h.len(), 19);
        assert_eq!(&h[..8], b"OpusHead");
        assert_eq!(h[8], 1, "version");
        assert_eq!(h[9], 1, "channels");
        assert_eq!(u16::from_le_bytes([h[10], h[11]]), 312, "pre-skip");
        assert_eq!(
            u32::from_le_bytes([h[12], h[13], h[14], h[15]]),
            48_000,
            "input rate"
        );
        assert_eq!(&h[16..18], &[0, 0], "gain");
        assert_eq!(h[18], 0, "mapping family");
    }

    #[test]
    fn opus_tags_bytes() {
        let t = opus_tags("theuth", &[("TITLE", "Meeting"), ("TRACK", "mic")]);
        assert_eq!(&t[..8], b"OpusTags");
        assert_eq!(&t[8..12], &6u32.to_le_bytes());
        assert_eq!(&t[12..18], b"theuth");
        assert_eq!(&t[18..22], &2u32.to_le_bytes());
        assert_eq!(&t[22..26], &13u32.to_le_bytes());
        assert_eq!(&t[26..39], b"TITLE=Meeting");
        assert_eq!(&t[39..43], &9u32.to_le_bytes());
        assert_eq!(&t[43..], b"TRACK=mic");
    }

    #[test]
    fn granules_pages_and_eos() {
        let mut w = Writer::new(Vec::new(), 7, 312, &[("TRACK", "mix")]).unwrap();
        for _ in 0..120 {
            w.packet(vec![0xfc, 0xff, 0xfe], false).unwrap();
        }
        w.packet(vec![0xfc, 0xff, 0xfe], true).unwrap();
        assert_eq!(w.granule(), 121 * 960);
        let bytes = w.pw.into_inner();

        let mut r = PacketReader::new(Cursor::new(bytes));
        let head = r.read_packet_expected().unwrap();
        assert_eq!(&head.data[..8], b"OpusHead");
        assert_eq!(head.absgp_page(), 0);
        assert!(head.last_in_page());
        let tags = r.read_packet_expected().unwrap();
        assert_eq!(&tags.data[..8], b"OpusTags");
        assert!(tags.last_in_page());

        let mut n = 0u64;
        let mut page_ends = Vec::new();
        let mut last = None;
        while let Some(p) = r.read_packet().unwrap() {
            n += 1;
            assert_eq!(p.stream_serial(), 7);
            if p.last_in_page() {
                // The page granule is the sample count through its last packet.
                assert_eq!(p.absgp_page(), n * 960);
                page_ends.push(n);
            }
            last = Some(p);
        }
        assert_eq!(n, 121);
        // A page every 50 frames (1 s), then the EOS page with the tail.
        assert_eq!(page_ends, vec![50, 100, 121]);
        let last = last.unwrap();
        assert!(last.last_in_stream(), "EOS flag");
        assert_eq!(last.absgp_page(), 121 * 960);
    }
}
