//! MPEG-TS, PES and PSI framing, written rather than captured.

pub const TS_PACKET_SIZE: usize = 188;
pub const SYNC_BYTE: u8 = 0x47;

/// Accumulates a transport stream, keeping one continuity counter per PID.
///
/// The counter is the reason this is a struct and not a free function: a
/// stream whose continuity jumps makes `PesAssembler` count a discontinuity,
/// and every test that reads a fixture asserts that count is zero.
#[derive(Debug, Default)]
pub struct TsWriter {
    counters: Vec<(u16, u8)>,
    bytes: Vec<u8>,
}

impl TsWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }

    fn next_counter(&mut self, pid: u16) -> u8 {
        match self.counters.iter_mut().find(|(seen, _)| *seen == pid) {
            Some(entry) => {
                entry.1 = (entry.1 + 1) & 0x0F;
                entry.1
            }
            None => {
                self.counters.push((pid, 0));
                0
            }
        }
    }

    /// One 188-byte packet. A payload shorter than 184 bytes is stuffed with an
    /// adaptation field, which is what a multiplexer does and what keeps the
    /// payload boundary exact rather than trailing 0xFF into the PES.
    pub fn packet(&mut self, pid: u16, payload_unit_start: bool, payload: &[u8]) {
        assert!(
            !payload.is_empty() && payload.len() <= TS_PACKET_SIZE - 4,
            "a TS payload is 1..=184 bytes, got {}",
            payload.len()
        );
        let continuity_counter = self.next_counter(pid);
        let mut packet = [0xFFu8; TS_PACKET_SIZE];
        packet[0] = SYNC_BYTE;
        packet[1] = ((pid >> 8) as u8 & 0x1F) | if payload_unit_start { 0x40 } else { 0x00 };
        packet[2] = (pid & 0xFF) as u8;
        let payload_offset = if payload.len() == TS_PACKET_SIZE - 4 {
            packet[3] = 0x10 | continuity_counter;
            4
        } else {
            packet[3] = 0x30 | continuity_counter;
            let adaptation_length = TS_PACKET_SIZE - 5 - payload.len();
            packet[4] = adaptation_length as u8;
            if adaptation_length != 0 {
                packet[5] = 0x00; // no adaptation flags set
            }
            5 + adaptation_length
        };
        packet[payload_offset..].copy_from_slice(payload);
        self.bytes.extend_from_slice(&packet);
    }

    /// Split one PES or PSI payload across as many packets as it needs, with
    /// the unit-start indicator on the first.
    pub fn payload(&mut self, pid: u16, bytes: &[u8]) {
        let mut rest = bytes;
        let mut first = true;
        while !rest.is_empty() {
            let take = rest.len().min(TS_PACKET_SIZE - 4);
            self.packet(pid, first, &rest[..take]);
            rest = &rest[take..];
            first = false;
        }
    }

    /// A PSI section, prefixed with the `pointer_field` a section start carries.
    pub fn section(&mut self, pid: u16, section: &[u8]) {
        let mut payload = Vec::with_capacity(section.len() + 1);
        payload.push(0x00);
        payload.extend_from_slice(section);
        self.payload(pid, &payload);
    }

    /// Appends a null (stuffing) packet on the reserved PID `0x1FFF`.
    ///
    /// Real muxers pad with these constantly to hold a constant bitrate, so
    /// this is not filler invented for testing — it is the standard MPEG-TS
    /// mechanism (ISO/IEC 13818-1 §2.4.3.3) for a stream that has nothing
    /// else to send at that instant. It earns its place here for a sharper
    /// reason: `PacketSplitter`'s initial-sync check will not trust a sync
    /// byte until it also sees one exactly `TS_PACKET_SIZE` bytes later, so a
    /// synthesized stream that is genuinely one packet long can never be
    /// recovered — and a one-packet transport stream is not a shape any real
    /// broadcast produces anyway. Call this once after such a payload so the
    /// stream is at least two packets, the way it always is on the wire.
    ///
    /// This does not go through the per-PID continuity-counter map: PID
    /// `0x1FFF` carries no elementary stream for a continuity count to track,
    /// and ISO/IEC 13818-1 leaves a null packet's continuity_counter
    /// unspecified, so every null packet here is byte-for-byte identical and
    /// carries `continuity_counter = 0`. `PesAssembler` and `ServiceScanner`
    /// both filter by PID before looking at a packet's contents, so this can
    /// never be mistaken for PES payload or section bytes by anything that
    /// reads the stream afterward.
    pub fn null_packet(&mut self) {
        const NULL_PACKET_PID: u16 = 0x1FFF;
        let mut packet = [0xFFu8; TS_PACKET_SIZE];
        packet[0] = SYNC_BYTE;
        packet[1] = (NULL_PACKET_PID >> 8) as u8 & 0x1F;
        packet[2] = (NULL_PACKET_PID & 0xFF) as u8;
        packet[3] = 0x10; // no adaptation field, payload present, cc = 0
        self.bytes.extend_from_slice(&packet);
    }
}

/// A private_stream_1 PES (0xBD) carrying a PTS — how the caption PID is sent.
pub fn caption_pes(payload: &[u8], pts_90k: u64) -> Vec<u8> {
    let mut bytes = vec![0x00, 0x00, 0x01, 0xBD];
    // Three fixed header bytes, five of PTS, then the payload.
    let packet_length = 3 + 5 + payload.len();
    assert!(packet_length <= 0xFFFF, "PES_packet_length overflows");
    bytes.push((packet_length >> 8) as u8);
    bytes.push((packet_length & 0xFF) as u8);
    bytes.push(0x80); // marker bits '10'
    bytes.push(0x80); // PTS present, DTS absent
    bytes.push(0x05); // PES_header_data_length
    bytes.push(0x21 | (((pts_90k >> 30) as u8 & 0x07) << 1));
    bytes.push(((pts_90k >> 22) & 0xFF) as u8);
    bytes.push((((pts_90k >> 15) & 0x7F) as u8) << 1 | 0x01);
    bytes.push(((pts_90k >> 7) & 0xFF) as u8);
    bytes.push(((pts_90k & 0x7F) as u8) << 1 | 0x01);
    bytes.extend_from_slice(payload);
    bytes
}

/// A private_stream_2 PES (0xBF): no optional header, and therefore no PTS.
/// ARIB superimpose arrives this way, which is why it has no timing at all.
pub fn superimpose_pes(payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0x00, 0x00, 0x01, 0xBF];
    assert!(payload.len() <= 0xFFFF, "PES_packet_length overflows");
    bytes.push((payload.len() >> 8) as u8);
    bytes.push((payload.len() & 0xFF) as u8);
    bytes.extend_from_slice(payload);
    bytes
}

/// The MPEG-2 systems CRC-32 (polynomial 0x04C11DB7, MSB first, no final xor).
pub fn mpeg_crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in bytes {
        crc ^= (byte as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn with_crc(mut section: Vec<u8>) -> Vec<u8> {
    let crc = mpeg_crc32(&section);
    section.extend_from_slice(&crc.to_be_bytes());
    section
}

/// A one-program PAT.
pub fn pat_section(program_number: u16, pmt_pid: u16) -> Vec<u8> {
    // 5 bytes of table-specific header + one 4-byte entry + 4 bytes of CRC.
    let section_length: u16 = 13;
    with_crc(vec![
        0x00,
        0xB0 | ((section_length >> 8) as u8 & 0x0F),
        (section_length & 0xFF) as u8,
        0x00,
        0x01, // transport_stream_id
        0xC1, // version 0, current
        0x00, // section_number
        0x00, // last_section_number
        (program_number >> 8) as u8,
        (program_number & 0xFF) as u8,
        0xE0 | ((pmt_pid >> 8) as u8 & 0x1F),
        (pmt_pid & 0xFF) as u8,
    ])
}

/// One elementary stream in the PMT, with its stream_identifier_descriptor.
#[derive(Clone, Copy, Debug)]
pub struct ElementaryStream {
    pub stream_type: u8,
    pub pid: u16,
    pub component_tag: u8,
}

/// A PMT, padded by `program_info_filler` bytes of private descriptor.
///
/// The padding is the point: an ISDB PMT carrying the ARIB descriptors runs
/// past the 183 bytes a section start fits in one TS packet, and a scanner that
/// only reads single-packet sections finds nothing at all on a real broadcast.
/// A synthesized PMT that fitted in one packet would test the easy case only.
pub fn pmt_section(
    program_number: u16,
    pcr_pid: u16,
    program_info_filler: usize,
    streams: &[ElementaryStream],
) -> Vec<u8> {
    let mut program_info = Vec::with_capacity(program_info_filler);
    if program_info_filler > 0 {
        assert!(
            (2..=257).contains(&program_info_filler),
            "a descriptor is 2..=257 bytes"
        );
        program_info.push(0xC1); // user-private descriptor tag
        program_info.push((program_info_filler - 2) as u8);
        program_info.resize(program_info_filler, 0x00);
    }

    let mut elementary = Vec::new();
    for stream in streams {
        elementary.push(stream.stream_type);
        elementary.push(0xE0 | ((stream.pid >> 8) as u8 & 0x1F));
        elementary.push((stream.pid & 0xFF) as u8);
        elementary.push(0xF0);
        elementary.push(0x03); // ES_info_length
        elementary.push(0x52); // stream_identifier_descriptor
        elementary.push(0x01);
        elementary.push(stream.component_tag);
    }

    let section_length = 5 + 2 + 2 + program_info.len() + elementary.len() + 4;
    assert!(section_length <= 0x03FD, "section_length overflows");
    let mut section = vec![
        0x02,
        0xB0 | ((section_length >> 8) as u8 & 0x0F),
        (section_length & 0xFF) as u8,
        (program_number >> 8) as u8,
        (program_number & 0xFF) as u8,
        0xC1,
        0x00,
        0x00,
        0xE0 | ((pcr_pid >> 8) as u8 & 0x1F),
        (pcr_pid & 0xFF) as u8,
        0xF0 | ((program_info.len() >> 8) as u8 & 0x0F),
        (program_info.len() & 0xFF) as u8,
    ];
    section.extend_from_slice(&program_info);
    section.extend_from_slice(&elementary);
    with_crc(section)
}
