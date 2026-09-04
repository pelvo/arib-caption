//! Compatibility assertions for `arib-caption`'s shared PES adapter.

mod support;

use arib_caption::ts::{pes_pts_ms, PesAssembler, PesPacket, TS_PACKET_SIZE};
use support::fixtures;

const CAPTION_PID: u16 = 0x0130;

fn ts_packet(pid: u16, pusi: bool, cc: u8, payload: &[u8]) -> [u8; TS_PACKET_SIZE] {
    let mut packet = [0xFFu8; TS_PACKET_SIZE];
    packet[0] = 0x47;
    packet[1] = ((pid >> 8) as u8 & 0x1F) | if pusi { 0x40 } else { 0 };
    packet[2] = (pid & 0xFF) as u8;
    packet[3] = 0x10 | (cc & 0x0F);
    packet[4..4 + payload.len()].copy_from_slice(payload);
    packet
}

/// PES header for a private_stream_1 packet with a PTS, then `body`.
fn pes(body: &[u8], pts_90k: u64) -> Vec<u8> {
    let mut bytes = vec![0x00, 0x00, 0x01, 0xBD];
    let length = 3 + 5 + body.len();
    bytes.push((length >> 8) as u8);
    bytes.push((length & 0xFF) as u8);
    bytes.push(0x80);
    bytes.push(0x80); // PTS present
    bytes.push(5); // header length
    bytes.push(0x21 | (((pts_90k >> 30) as u8 & 0x07) << 1));
    bytes.push(((pts_90k >> 22) & 0xFF) as u8);
    bytes.push((((pts_90k >> 15) & 0x7F) as u8) << 1 | 1);
    bytes.push(((pts_90k >> 7) & 0xFF) as u8);
    bytes.push(((pts_90k & 0x7F) as u8) << 1 | 1);
    bytes.extend_from_slice(body);
    bytes
}

fn video_pes(body: &[u8], pts_90k: u64) -> Vec<u8> {
    let mut bytes = pes(body, pts_90k);
    bytes[3] = 0xE0;
    bytes
}

fn assert_transport_damage_drops_in_progress(mark_damage: fn(&mut [u8; TS_PACKET_SIZE])) {
    let old_body = vec![0x11; 300];
    let old_pes = pes(&old_body, 90_000);
    let new_body = [0x80, 0xFF, 0xF0, 0x22];
    let mut assembler = PesAssembler::new(CAPTION_PID);

    assert!(assembler
        .push(&ts_packet(CAPTION_PID, true, 0, &old_pes[..184]))
        .is_none());
    let mut damaged_tail = ts_packet(CAPTION_PID, false, 1, &old_pes[184..]);
    mark_damage(&mut damaged_tail);
    assert!(assembler.push(&damaged_tail).is_none());
    assert!(assembler
        .push(&ts_packet(CAPTION_PID, false, 2, &old_pes[184..]))
        .is_none());
    assert!(assembler.flush().is_none());

    let completed = assembler
        .push(&ts_packet(CAPTION_PID, true, 3, &pes(&new_body, 180_000)))
        .expect("clean PUSI recovers");
    assert_eq!(completed.payload, new_body);
    assert_eq!(assembler.discontinuities, 0);
}

fn mark_transport_error(packet: &mut [u8; TS_PACKET_SIZE]) {
    packet[1] |= 0x80;
}

fn mark_scrambled(packet: &mut [u8; TS_PACKET_SIZE]) {
    packet[3] |= 0x80;
}

#[test]
fn reassembles_one_pes_with_pts() {
    let body = [0x80u8, 0xFF, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00];
    let mut assembler = PesAssembler::new(0x130);
    // A PES that fits in one packet completes on PES_packet_length alone.
    let output = assembler.push(&ts_packet(0x130, true, 0, &pes(&body, 90_000)));
    let packet: PesPacket = output.expect("completed PES");
    assert_eq!(packet.pts_90k, Some(90_000));
    assert_eq!(packet.pts_ms(), Some(1000));
    assert_eq!(packet.payload, body);
}

#[test]
fn ignores_other_pids() {
    let mut assembler = PesAssembler::new(0x130);
    assert!(assembler
        .push(&ts_packet(0x100, true, 0, &pes(&[0x80, 0xFF, 0xF0], 0),))
        .is_none());
}

#[test]
fn rejects_transport_error_packets_without_rewriting_the_flag() {
    let body = [0x80, 0xFF, 0xF0, 0x00];
    assert_transport_damage_drops_in_progress(mark_transport_error);

    let mut video = ts_packet(0x0100, true, 0, &video_pes(&body, 90_000));
    assert_eq!(pes_pts_ms(&video), Some(1_000), "clean control");
    mark_transport_error(&mut video);
    assert_eq!(pes_pts_ms(&video), None);
}

#[test]
fn rejects_scrambled_packets_without_rewriting_the_flag() {
    let body = [0x80, 0xFF, 0xF0, 0x00];
    assert_transport_damage_drops_in_progress(mark_scrambled);

    let mut video = ts_packet(0x0100, true, 0, &video_pes(&body, 90_000));
    assert_eq!(pes_pts_ms(&video), Some(1_000), "clean control");
    mark_scrambled(&mut video);
    assert_eq!(pes_pts_ms(&video), None);
}

#[test]
fn a_gap_at_fresh_pusi_discards_the_truncated_prior_pes() {
    let old_body = vec![0x11; 300];
    let old_pes = pes(&old_body, 90_000);
    let new_body = [0x80, 0xFF, 0xF0, 0x22];
    let mut assembler = PesAssembler::new(CAPTION_PID);

    assert!(assembler
        .push(&ts_packet(CAPTION_PID, true, 0, &old_pes[..184]))
        .is_none());
    let completed = assembler
        .push(&ts_packet(CAPTION_PID, true, 2, &pes(&new_body, 180_000)))
        .expect("fresh PES completes");

    assert_eq!(completed.pts_90k, Some(180_000));
    assert_eq!(completed.payload, new_body);
    assert_eq!(assembler.discontinuities, 1);
    assert!(assembler.flush().is_none());
}

/// The same reassembly, over the stream `tests/support/` builds.
///
/// The golden it replaces was captured from the retired vendored PES engine
/// against a real recording, so it attested to parity with that engine. This
/// one is a self-golden of the current engine over bytes this repository can
/// publish: it catches drift from here on and claims nothing about the engine
/// it replaced. See README.md's Test coverage section.
#[test]
fn synthetic_caption_stream_matches_the_recorded_pes_sequence() {
    let bytes = fixtures::caption_ts();
    let mut assembler = PesAssembler::new(fixtures::CAPTION_PID);
    let mut packets = Vec::new();
    for chunk in bytes.chunks_exact(TS_PACKET_SIZE) {
        if let Some(packet) = assembler.push(chunk) {
            packets.push(packet);
        }
    }
    if let Some(packet) = assembler.flush() {
        packets.push(packet);
    }
    assert_eq!(assembler.discontinuities, 0, "fixture must be clean");

    let actual = packets
        .iter()
        .map(|packet| {
            (
                packet.pts_ms().expect("caption PES carries PTS"),
                packet.payload.len(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        actual,
        vec![
            (9_500, 20),
            (10_000, 82),
            (11_500, 20),
            (12_000, 73),
            (13_500, 20),
            (14_000, 79),
            (15_500, 20),
            (16_000, 87),
            (17_500, 20),
            (18_000, 76),
            (19_500, 20),
            (20_000, 80),
            (21_500, 20),
            (22_000, 72),
            (23_500, 20),
            (24_000, 76),
            (25_500, 20),
            (26_000, 78),
            (27_500, 20),
            (28_000, 70),
            (29_500, 20),
            (30_000, 78),
            (31_500, 20),
            (32_000, 72),
            (33_500, 20),
            (34_000, 74),
            (35_500, 20),
            (36_000, 78),
            (37_500, 20),
            (38_000, 72),
            (39_500, 20),
            (40_000, 75),
            (41_500, 20),
            (42_000, 78),
            (43_500, 20),
            (44_000, 70),
            (45_500, 20),
            (46_000, 70),
            (47_500, 20),
            (48_000, 78),
            (49_500, 20),
            (50_000, 74),
            (51_500, 20),
            (52_000, 70),
            (53_500, 20),
            (54_000, 78),
            (55_500, 20),
            (56_000, 70),
            (57_500, 20),
            (58_000, 70),
            (59_500, 20),
            (60_000, 78),
            (61_500, 20),
            (62_000, 72),
            (63_500, 20),
            (64_000, 70),
            (65_500, 20),
            (66_000, 70),
            (67_500, 20),
            (68_000, 70),
            (69_500, 20),
            (70_000, 70),
            (71_500, 20),
            (72_000, 70),
            (73_500, 20),
            (74_000, 74),
            (75_500, 20),
            (76_000, 84),
            (77_500, 20),
            (78_000, 80),
        ]
    );
}
