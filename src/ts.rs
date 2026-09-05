//! Just enough MPEG-TS to find the caption PID and reassemble its PES packets.
//!
//! This exists because the caption stream reaches us as a transport stream and
//! nothing else in the pipeline will hand it over decoded: ferrite feeds a
//! service TS from its fanout, and a recording on disk is the same bytes. A
//! full demuxer is not needed — one PID, one PES per presentation, and the PTS
//! that goes with it.
//!
//! [`ServiceScanner`] answers "which PID carries the captions", which cannot be
//! assumed: it differs per service, and the superimpose stream sits on its own
//! PID with the same stream type. The PMT's component tag is what separates
//! them (ARIB TR-B14: 0x30..0x37 caption, 0x38..0x3F superimpose).

use tuner_codec::pes::transport_stream_packet_view;

pub const TS_PACKET_SIZE: usize = 188;
pub(crate) const SYNC_BYTE: u8 = 0x47;

pub use crate::shared_pes_adapter::{pes_pts_ms, PesAssembler, PesPacket};

/// A caption-carrying elementary stream found in the PMT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptionStream {
    pub pid: u16,
    pub service_id: u16,
    /// The stream_identifier_descriptor's component tag, which is what
    /// distinguishes captions (0x30..0x37) from superimpose (0x38..0x3F).
    pub component_tag: Option<u8>,
}

impl CaptionStream {
    pub fn is_superimpose(&self) -> bool {
        matches!(self.component_tag, Some(tag) if (0x38..=0x3f).contains(&tag))
    }
}

/// Finds caption PIDs by reading PAT and PMT off the stream.
///
/// Sections are reassembled across packets, which is not optional: an ISDB PMT
/// carrying the ARIB descriptors runs past 184 bytes routinely — NHK's is 210 —
/// so a scanner that only reads sections fitting in one packet finds nothing at
/// all on a real broadcast.
#[derive(Debug, Default)]
pub struct ServiceScanner {
    pmt_pids: Vec<(u16, u16)>, // (program_number, pmt_pid)
    readers: Vec<(u16, SectionReader)>,
    found: Vec<CaptionStream>,
}

impl ServiceScanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a TS packet. Returns the caption streams known so far.
    pub fn push(&mut self, packet: &[u8]) -> &[CaptionStream] {
        let Some(packet) = payload_of(packet) else {
            return &self.found;
        };
        let pid = packet.pid;
        let is_pat = pid == 0;
        let is_pmt = self.pmt_pids.iter().any(|&(_, p)| p == pid);
        if !is_pat && !is_pmt {
            return &self.found;
        }

        // Collect first, parse after: the reader borrow has to end before
        // parse_pat can touch self.pmt_pids.
        let sections = {
            let reader = match self.readers.iter().position(|&(p, _)| p == pid) {
                Some(i) => &mut self.readers[i].1,
                None => {
                    self.readers.push((pid, SectionReader::default()));
                    &mut self.readers.last_mut().unwrap().1
                }
            };
            let mut sections = Vec::new();
            reader.push(packet, &mut sections);
            sections
        };
        for section in sections {
            if is_pat {
                self.parse_pat(&section);
            } else {
                self.parse_pmt(&section);
            }
        }
        &self.found
    }

    pub fn streams(&self) -> &[CaptionStream] {
        &self.found
    }

    /// The caption (not superimpose) PID, if one has been seen.
    pub fn caption_pid(&self) -> Option<u16> {
        self.found
            .iter()
            .find(|s| !s.is_superimpose())
            .map(|s| s.pid)
    }

    /// The superimpose PID, if one has been seen.
    pub fn superimpose_pid(&self) -> Option<u16> {
        self.found
            .iter()
            .find(|s| s.is_superimpose())
            .map(|s| s.pid)
    }

    fn parse_pat(&mut self, section: &[u8]) {
        if section.is_empty() || section[0] != 0x00 {
            return;
        }
        let Some(body) = section_body(section) else {
            return;
        };
        // program_number(2) + PMT PID(2) per entry, after the 5 bytes of
        // table-specific header that section_body already skipped.
        for entry in body.chunks_exact(4) {
            let program = ((entry[0] as u16) << 8) | entry[1] as u16;
            let pid = (((entry[2] & 0x1f) as u16) << 8) | entry[3] as u16;
            if program != 0 && !self.pmt_pids.iter().any(|&(pr, _)| pr == program) {
                self.pmt_pids.push((program, pid));
            }
        }
    }

    fn parse_pmt(&mut self, section: &[u8]) {
        if section.is_empty() || section[0] != 0x02 {
            return;
        }
        // section_body() enforces the 8-byte minimum, so the service-id read
        // below must follow it: a section_length of 0 or 1 yields a 3- or
        // 4-byte section, and indexing [3] and [4] first panicked on it. The
        // bytes read here sit inside the header section_body has validated.
        let Some(body) = section_body(section) else {
            return;
        };
        let service_id = ((section[3] as u16) << 8) | section[4] as u16;
        if body.len() < 4 {
            return;
        }
        let program_info_length = (((body[2] & 0x0f) as usize) << 8) | body[3] as usize;
        let mut off = 4 + program_info_length;
        while off + 5 <= body.len() {
            let stream_type = body[off];
            let pid = (((body[off + 1] & 0x1f) as u16) << 8) | body[off + 2] as u16;
            let es_info_length = (((body[off + 3] & 0x0f) as usize) << 8) | body[off + 4] as usize;
            let desc_start = off + 5;
            let desc_end = desc_start + es_info_length;
            if desc_end > body.len() {
                break;
            }
            // 0x06 is PES carrying private data — captions, superimpose, and
            // the data broadcasting streams all share it, so the component tag
            // decides.
            if stream_type == 0x06 {
                let component_tag = component_tag(&body[desc_start..desc_end]);
                if matches!(component_tag, Some(tag) if (0x30..=0x3f).contains(&tag)) {
                    let stream = CaptionStream {
                        pid,
                        service_id,
                        component_tag,
                    };
                    if !self.found.contains(&stream) {
                        self.found.push(stream);
                    }
                }
            }
            off = desc_end;
        }
    }
}

/// Reassembles PSI sections arriving on one PID.
///
/// A section start carries a `pointer_field` saying how many bytes of the
/// *previous* section come first, which is how two tables share a packet.
#[derive(Debug, Default)]
struct SectionReader {
    buf: Vec<u8>,
    started: bool,
    awaiting_pusi: bool,
    last_continuity_counter: Option<u8>,
    last_packet: Option<[u8; TS_PACKET_SIZE]>,
}

impl SectionReader {
    fn push(&mut self, packet: PsiPayloadPacket<'_>, out: &mut Vec<Vec<u8>>) {
        if packet.transport_error_indicator || packet.scrambling_control != 0 {
            self.invalidate();
            return;
        }

        let exact_duplicate = self.last_continuity_counter == Some(packet.continuity_counter)
            && self
                .last_packet
                .as_ref()
                .is_some_and(|last| last.as_slice() == packet.raw);
        if exact_duplicate {
            return;
        }

        let continuity_gap = self
            .last_continuity_counter
            .is_some_and(|last| packet.continuity_counter != (last + 1) & 0x0f);
        if continuity_gap {
            self.invalidate();
        }
        self.last_continuity_counter = Some(packet.continuity_counter);
        let mut saved = [0u8; TS_PACKET_SIZE];
        saved.copy_from_slice(packet.raw);
        self.last_packet = Some(saved);

        if self.awaiting_pusi {
            if !packet.pusi {
                return;
            }
            self.awaiting_pusi = false;
        }

        self.push_payload(packet.payload, packet.pusi, out);
    }

    fn push_payload(&mut self, payload: &[u8], pusi: bool, out: &mut Vec<Vec<u8>>) {
        let mut rest = payload;
        if pusi {
            if rest.is_empty() {
                return;
            }
            let pointer = (rest[0] as usize).min(rest.len() - 1);
            rest = &rest[1..];
            let (tail, next) = rest.split_at(pointer);
            if self.started {
                self.buf.extend_from_slice(tail);
                self.drain(out);
            }
            // A section start abandons anything still incomplete: its missing
            // bytes are never coming.
            self.buf.clear();
            self.started = !next.is_empty() && next[0] != 0xff;
            if self.started {
                self.buf.extend_from_slice(next);
            }
        } else if self.started {
            self.buf.extend_from_slice(rest);
        }
        self.drain(out);
    }

    fn invalidate(&mut self) {
        self.buf.clear();
        self.started = false;
        self.awaiting_pusi = true;
    }

    fn drain(&mut self, out: &mut Vec<Vec<u8>>) {
        while self.started && self.buf.len() >= 3 {
            let length = (((self.buf[1] & 0x0f) as usize) << 8) | self.buf[2] as usize;
            let total = 3 + length;
            if self.buf.len() < total {
                return;
            }
            out.push(self.buf[..total].to_vec());
            self.buf.drain(..total);
            // What follows is another section or 0xFF stuffing.
            if self.buf.first().is_none_or(|&b| b == 0xff) {
                self.buf.clear();
                self.started = false;
            }
        }
    }
}

#[derive(Clone, Copy)]
struct PsiPayloadPacket<'a> {
    raw: &'a [u8],
    pid: u16,
    payload: &'a [u8],
    pusi: bool,
    continuity_counter: u8,
    transport_error_indicator: bool,
    scrambling_control: u8,
}

/// Borrowed fields of a valid TS packet carrying PSI payload bytes.
fn payload_of(packet: &[u8]) -> Option<PsiPayloadPacket<'_>> {
    let parsed = transport_stream_packet_view(packet)?;
    if !parsed.has_payload || parsed.payload.is_empty() {
        return None;
    }
    Some(PsiPayloadPacket {
        raw: packet,
        pid: parsed.packet_identifier,
        payload: parsed.payload,
        pusi: parsed.payload_unit_start_indicator,
        continuity_counter: parsed.continuity_counter,
        transport_error_indicator: parsed.transport_error_indicator,
        scrambling_control: parsed.scrambling_control,
    })
}

/// Contents of a section between its header and its CRC, or `None` if the
/// section does not fit in this packet.
fn section_body(section: &[u8]) -> Option<&[u8]> {
    if section.len() < 8 {
        return None;
    }
    let section_length = (((section[1] & 0x0f) as usize) << 8) | section[2] as usize;
    let end = 3 + section_length;
    if end > section.len() || section_length < 9 {
        return None;
    }
    // Skip the 5 bytes after section_length (table id extension, version,
    // section numbers) and stop before the 4-byte CRC.
    Some(&section[8..end - 4])
}

fn component_tag(descriptors: &[u8]) -> Option<u8> {
    let mut off = 0usize;
    while off + 2 <= descriptors.len() {
        let tag = descriptors[off];
        let len = descriptors[off + 1] as usize;
        let body = descriptors.get(off + 2..off + 2 + len)?;
        // stream_identifier_descriptor
        if tag == 0x52 && !body.is_empty() {
            return Some(body[0]);
        }
        off += 2 + len;
    }
    None
}

/// Splits a byte stream into TS packets, tolerating a stream that does not
/// start on a packet boundary.
#[derive(Debug, Default)]
pub struct PacketSplitter {
    buf: Vec<u8>,
    synced: bool,
}

impl PacketSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append bytes, then call [`PacketSplitter::next_packet`] until it is None.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn next_packet(&mut self) -> Option<[u8; TS_PACKET_SIZE]> {
        if !self.synced {
            // Resync: a sync byte that is followed by another one a packet
            // later. A single 0x47 inside video payload is common; two in step
            // is not.
            let mut i = 0usize;
            loop {
                if i + TS_PACKET_SIZE + 1 > self.buf.len() {
                    if i > 0 {
                        self.buf.drain(..i);
                    }
                    return None;
                }
                if self.buf[i] == SYNC_BYTE && self.buf[i + TS_PACKET_SIZE] == SYNC_BYTE {
                    self.buf.drain(..i);
                    self.synced = true;
                    break;
                }
                i += 1;
            }
        }
        if self.buf.len() < TS_PACKET_SIZE {
            return None;
        }
        if self.buf[0] != SYNC_BYTE {
            // Lost alignment — resync on the next call.
            self.synced = false;
            self.buf.drain(..1);
            return None;
        }
        let mut packet = [0u8; TS_PACKET_SIZE];
        packet.copy_from_slice(&self.buf[..TS_PACKET_SIZE]);
        self.buf.drain(..TS_PACKET_SIZE);
        Some(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts_packet(pid: u16, pusi: bool, cc: u8, payload: &[u8]) -> [u8; TS_PACKET_SIZE] {
        assert!(payload.len() <= TS_PACKET_SIZE - 4);
        let mut p = [0xffu8; TS_PACKET_SIZE];
        p[0] = SYNC_BYTE;
        p[1] = ((pid >> 8) as u8 & 0x1f) | if pusi { 0x40 } else { 0 };
        p[2] = (pid & 0xff) as u8;
        let payload_offset = if payload.len() == TS_PACKET_SIZE - 4 {
            p[3] = 0x10 | (cc & 0x0f);
            4
        } else {
            p[3] = 0x30 | (cc & 0x0f);
            let adaptation_length = TS_PACKET_SIZE - 5 - payload.len();
            p[4] = adaptation_length as u8;
            if adaptation_length != 0 {
                p[5] = 0;
            }
            5 + adaptation_length
        };
        p[payload_offset..].copy_from_slice(payload);
        p
    }

    fn pat_section(pmt_pid: u16) -> Vec<u8> {
        vec![
            0x00,
            0xB0,
            0x0D,
            0x00,
            0x01,
            0xC1,
            0x00,
            0x00,
            0x04,
            0x00,
            0xE0 | ((pmt_pid >> 8) as u8 & 0x1f),
            pmt_pid as u8,
            0x00,
            0x00,
            0x00,
            0x00,
        ]
    }

    fn pmt_section(caption_pid: u16) -> Vec<u8> {
        vec![
            0x02,
            0xB0,
            0x15,
            0x04,
            0x00,
            0xC1,
            0x00,
            0x00,
            0xE1,
            0x00,
            0xF0,
            0x00,
            0x06,
            0xE0 | ((caption_pid >> 8) as u8 & 0x1f),
            caption_pid as u8,
            0xF0,
            0x03,
            0x52,
            0x01,
            0x30,
            0x00,
            0x00,
            0x00,
            0x00,
        ]
    }

    fn section_start(section: &[u8]) -> Vec<u8> {
        let mut payload = Vec::with_capacity(section.len() + 1);
        payload.push(0);
        payload.extend_from_slice(section);
        payload
    }

    fn feed_pat(scanner: &mut ServiceScanner, pmt_pid: u16, cc: u8) {
        scanner.push(&ts_packet(
            0,
            true,
            cc,
            &section_start(&pat_section(pmt_pid)),
        ));
    }

    fn mark_transport_error(packet: &mut [u8; TS_PACKET_SIZE]) {
        packet[1] |= 0x80;
    }

    fn mark_scrambled(packet: &mut [u8; TS_PACKET_SIZE]) {
        packet[3] |= 0x80;
    }

    fn assert_psi_damage_is_rejected(mark_damage: fn(&mut [u8; TS_PACKET_SIZE])) {
        const PMT_PID: u16 = 0x0100;
        const CAPTION_PID: u16 = 0x0130;

        // A damaged PAT must not make its PMT PID watchable.
        let mut scanner = ServiceScanner::new();
        let mut pat = ts_packet(0, true, 0, &section_start(&pat_section(PMT_PID)));
        mark_damage(&mut pat);
        scanner.push(&pat);
        scanner.push(&ts_packet(
            PMT_PID,
            true,
            0,
            &section_start(&pmt_section(CAPTION_PID)),
        ));
        assert_eq!(scanner.caption_pid(), None);

        // Damage in a split PMT invalidates the first fragment. A later tail
        // remains unusable until a clean PUSI starts a new section.
        let mut scanner = ServiceScanner::new();
        feed_pat(&mut scanner, PMT_PID, 0);
        let pmt = pmt_section(CAPTION_PID);
        let mut first = vec![0];
        first.extend_from_slice(&pmt[..10]);
        scanner.push(&ts_packet(PMT_PID, true, 0, &first));
        let mut damaged_tail = ts_packet(PMT_PID, false, 1, &pmt[10..]);
        mark_damage(&mut damaged_tail);
        scanner.push(&damaged_tail);
        scanner.push(&ts_packet(PMT_PID, false, 2, &pmt[10..]));
        assert_eq!(scanner.caption_pid(), None);

        scanner.push(&ts_packet(PMT_PID, true, 3, &section_start(&pmt)));
        assert_eq!(scanner.caption_pid(), Some(CAPTION_PID));
    }

    #[test]
    fn service_scanner_rejects_transport_error_pat_and_pmt_packets() {
        assert_psi_damage_is_rejected(mark_transport_error);
    }

    #[test]
    fn service_scanner_rejects_scrambled_pat_and_pmt_packets() {
        assert_psi_damage_is_rejected(mark_scrambled);
    }

    #[test]
    fn gap_at_pusi_discards_the_damaged_tail_before_starting_clean_pmt() {
        const PMT_PID: u16 = 0x0100;
        let old_pmt = pmt_section(0x0130);
        let new_pmt = pmt_section(0x0131);
        let mut scanner = ServiceScanner::new();
        feed_pat(&mut scanner, PMT_PID, 0);

        let split = old_pmt.len() - 4;
        let mut first = vec![0];
        first.extend_from_slice(&old_pmt[..split]);
        scanner.push(&ts_packet(PMT_PID, true, 0, &first));

        let mut recovery = vec![4];
        recovery.extend_from_slice(&old_pmt[split..]);
        recovery.extend_from_slice(&new_pmt);
        scanner.push(&ts_packet(PMT_PID, true, 2, &recovery));

        assert_eq!(
            scanner.streams(),
            &[CaptionStream {
                pid: 0x0131,
                service_id: 1024,
                component_tag: Some(0x30),
            }]
        );
    }

    #[test]
    fn gap_rejects_non_pusi_recovery_until_a_clean_start() {
        const PMT_PID: u16 = 0x0100;
        let old_pmt = pmt_section(0x0130);
        let new_pmt = pmt_section(0x0131);
        let mut scanner = ServiceScanner::new();
        feed_pat(&mut scanner, PMT_PID, 0);

        let mut first = vec![0];
        first.extend_from_slice(&old_pmt[..10]);
        scanner.push(&ts_packet(PMT_PID, true, 0, &first));
        scanner.push(&ts_packet(PMT_PID, false, 2, &old_pmt[10..]));
        scanner.push(&ts_packet(PMT_PID, false, 3, &new_pmt));
        assert_eq!(scanner.caption_pid(), None);

        scanner.push(&ts_packet(PMT_PID, true, 4, &section_start(&new_pmt)));
        assert_eq!(scanner.caption_pid(), Some(0x0131));
    }

    #[test]
    fn exact_duplicate_payload_packet_is_ignored() {
        const PMT_PID: u16 = 0x0100;
        let pmt = pmt_section(0x0130);
        let mut scanner = ServiceScanner::new();
        feed_pat(&mut scanner, PMT_PID, 0);

        let mut first = vec![0];
        first.extend_from_slice(&pmt[..8]);
        scanner.push(&ts_packet(PMT_PID, true, 0, &first));
        let middle = ts_packet(PMT_PID, false, 1, &pmt[8..16]);
        scanner.push(&middle);
        scanner.push(&middle);
        scanner.push(&ts_packet(PMT_PID, false, 2, &pmt[16..]));

        assert_eq!(scanner.caption_pid(), Some(0x0130));
        assert_eq!(scanner.streams().len(), 1);
    }

    #[test]
    fn pat_and_pmt_track_payload_continuity_independently() {
        const PMT_PID: u16 = 0x0100;
        let pmt = pmt_section(0x0130);
        let mut scanner = ServiceScanner::new();
        feed_pat(&mut scanner, PMT_PID, 7);

        let mut first = vec![0];
        first.extend_from_slice(&pmt[..10]);
        scanner.push(&ts_packet(PMT_PID, true, 12, &first));
        feed_pat(&mut scanner, PMT_PID, 8);
        scanner.push(&ts_packet(PMT_PID, false, 13, &pmt[10..]));

        assert_eq!(scanner.caption_pid(), Some(0x0130));
    }

    #[test]
    fn splitter_resyncs_on_garbage_prefix() {
        let mut split = PacketSplitter::new();
        let a = ts_packet(0x130, true, 0, &[0x01]);
        let b = ts_packet(0x130, false, 1, &[0x02]);
        let mut stream = vec![0x00, 0x47, 0x11]; // junk, including a false sync
        stream.extend_from_slice(&a);
        stream.extend_from_slice(&b);
        split.feed(&stream);
        let first = split.next_packet().expect("first packet");
        assert_eq!(first[..4], a[..4]);
        let second = split.next_packet().expect("second packet");
        assert_eq!(second[..4], b[..4]);
    }

    #[test]
    fn a_pmt_too_short_to_hold_a_service_id_is_ignored_rather_than_panicking() {
        // parse_pmt read section[3..=4] for the service id before
        // section_body() validated the 8-byte minimum, so a section_length of
        // 0 or 1 produced a 3- or 4-byte "PMT" that indexed past its end.
        // Slice indexing is bounds-checked in every profile, so this panicked
        // in release too, not only in debug.
        const PMT_PID: u16 = 0x0100;

        for section_length in [0x00u8, 0x01] {
            let mut scanner = ServiceScanner::new();
            feed_pat(&mut scanner, PMT_PID, 0);
            let truncated = [0x02, 0xb0, section_length];
            scanner.push(&ts_packet(PMT_PID, true, 0, &section_start(&truncated)));
            assert_eq!(scanner.caption_pid(), None);
        }
    }
}
