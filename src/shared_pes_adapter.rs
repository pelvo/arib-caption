//! Public compatibility adapter over the workspace's shared PES engine.

use std::collections::VecDeque;

use tuner_codec::pes::{
    parse_packetized_elementary_stream, ticks_to_ms, transport_stream_packet_view,
    PacketizedElementaryStream, PacketizedElementaryStreamAssembler,
};
use tuner_codec::ts::TransportStreamPacket;

/// A reassembled PES packet from one PID.
#[derive(Clone, Debug)]
pub struct PesPacket {
    /// Presentation timestamp in 90 kHz units, if the PES carried one.
    pub pts_90k: Option<u64>,
    /// The PES payload — for caption streams this starts at `data_identifier`,
    /// which is what [`crate::pes::parse`] wants.
    pub payload: Vec<u8>,
}

impl PesPacket {
    /// PTS in milliseconds.
    pub fn pts_ms(&self) -> Option<i64> {
        self.pts_90k.map(|pts| ticks_to_ms(pts) as i64)
    }
}

/// Reassembles PES packets for a single PID.
///
/// This preserves `arib-caption`'s original public API while delegating PES
/// parsing and buffering to [`PacketizedElementaryStreamAssembler`].
#[derive(Debug)]
pub struct PesAssembler {
    pid: u16,
    inner: PacketizedElementaryStreamAssembler,
    pending: VecDeque<PesPacket>,
    /// Packets dropped because the continuity counter jumped.
    pub discontinuities: u64,
}

impl PesAssembler {
    pub fn new(pid: u16) -> Self {
        Self {
            pid,
            inner: PacketizedElementaryStreamAssembler::new(pid),
            pending: VecDeque::new(),
            discontinuities: 0,
        }
    }

    pub fn pid(&self) -> u16 {
        self.pid
    }

    /// Push one TS packet. Packets for other PIDs are ignored.
    pub fn push(&mut self, packet: &[u8]) -> Option<PesPacket> {
        let packet = TransportStreamPacket::parse(packet).ok()?;
        if packet.packet_identifier != self.pid || !packet.has_payload || packet.payload.is_empty()
        {
            return None;
        }

        if let Ok(completed) = self.inner.append(&packet) {
            self.pending
                .extend(completed.into_iter().filter_map(into_public));
        }
        self.discontinuities = self.inner.continuity_drop_count();
        self.pending.pop_front()
    }

    /// Finish the PES currently under assembly, if any.
    pub fn flush(&mut self) -> Option<PesPacket> {
        self.queue_flushed();
        self.pending.pop_front()
    }

    fn queue_flushed(&mut self) {
        if let Ok(Some(completed)) = self.inner.flush() {
            if let Some(packet) = into_public(completed) {
                self.pending.push_back(packet);
            }
        }
    }
}

fn into_public(packet: PacketizedElementaryStream) -> Option<PesPacket> {
    matches!(packet.stream_identifier, 0xBD | 0xBF).then_some(PesPacket {
        pts_90k: packet.presentation_timestamp_90khz,
        payload: packet.elementary_bytes,
    })
}

/// The presentation timestamp of an audio or video PES unit start, in
/// milliseconds.
pub fn pes_pts_ms(packet: &[u8]) -> Option<i64> {
    let packet = transport_stream_packet_view(packet)?;
    if packet.transport_error_indicator
        || packet.scrambling_control != 0
        || !packet.payload_unit_start_indicator
    {
        return None;
    }
    let parsed = parse_packetized_elementary_stream(packet.payload).ok()?;
    if !(0xC0..=0xEF).contains(&parsed.stream_identifier) {
        return None;
    }
    parsed
        .presentation_timestamp_90khz
        .map(|pts| ticks_to_ms(pts) as i64)
}
