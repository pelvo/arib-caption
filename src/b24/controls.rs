//! Control code values (ARIB STD-B24 part 1, tables 7-14 and 7-15).

/// C0 set — cursor movement, shifts, and the escape.
pub mod c0 {
    pub const NUL: u8 = 0x00;
    pub const BEL: u8 = 0x07;
    /// Active position backward.
    pub const APB: u8 = 0x08;
    /// Active position forward.
    pub const APF: u8 = 0x09;
    /// Active position down.
    pub const APD: u8 = 0x0a;
    /// Active position up.
    pub const APU: u8 = 0x0b;
    /// Clear screen.
    pub const CS: u8 = 0x0c;
    /// Active position return (newline).
    pub const APR: u8 = 0x0d;
    /// Locking shift 1 / 0.
    pub const LS1: u8 = 0x0e;
    pub const LS0: u8 = 0x0f;
    /// Parameterized active position forward.
    pub const PAPF: u8 = 0x16;
    pub const CAN: u8 = 0x18;
    /// Single shift 2 / 3.
    pub const SS2: u8 = 0x19;
    pub const ESC: u8 = 0x1b;
    /// Active position set.
    pub const APS: u8 = 0x1c;
    pub const SS3: u8 = 0x1d;
    pub const RS: u8 = 0x1e;
    pub const US: u8 = 0x1f;
    pub const SP: u8 = 0x20;
}

/// C1 set — colour, size, styling, timing.
pub mod c1 {
    pub const DEL: u8 = 0x7f;
    /// Foreground colour, black through white.
    pub const BKF: u8 = 0x80;
    pub const RDF: u8 = 0x81;
    pub const GRF: u8 = 0x82;
    pub const YLF: u8 = 0x83;
    pub const BLF: u8 = 0x84;
    pub const MGF: u8 = 0x85;
    pub const CNF: u8 = 0x86;
    pub const WHF: u8 = 0x87;
    /// Small / middle / normal size, and the parameterized form.
    pub const SSZ: u8 = 0x88;
    pub const MSZ: u8 = 0x89;
    pub const NSZ: u8 = 0x8a;
    pub const SZX: u8 = 0x8b;
    /// Colour controls.
    pub const COL: u8 = 0x90;
    /// Flashing control.
    pub const FLC: u8 = 0x91;
    /// Conceal display controls.
    pub const CDC: u8 = 0x92;
    /// Pattern polarity controls.
    pub const POL: u8 = 0x93;
    /// Foreground/background memory-write modification.
    pub const WMM: u8 = 0x94;
    pub const MACRO: u8 = 0x95;
    /// Highlighting character block (the enclosure).
    pub const HLC: u8 = 0x97;
    /// Repeat character.
    pub const RPC: u8 = 0x98;
    /// Stop / start lining (underline).
    pub const SPL: u8 = 0x99;
    pub const STL: u8 = 0x9a;
    /// Control sequence introducer.
    pub const CSI: u8 = 0x9b;
    /// Time controls.
    pub const TIME: u8 = 0x9d;
}

/// Locking shifts that follow an ESC.
pub mod esc {
    pub const LS2: u8 = 0x6e;
    pub const LS3: u8 = 0x6f;
    pub const LS1R: u8 = 0x7e;
    pub const LS2R: u8 = 0x7d;
    pub const LS3R: u8 = 0x7c;
}

/// CSI final bytes.
pub mod csi {
    /// Character deformation.
    pub const GSM: u8 = 0x42;
    /// Set writing format.
    pub const SWF: u8 = 0x53;
    /// Composite character composition.
    pub const CCC: u8 = 0x54;
    /// Set display format.
    pub const SDF: u8 = 0x56;
    /// Character composition dot designation.
    pub const SSM: u8 = 0x57;
    /// Set horizontal / vertical spacing.
    pub const SHS: u8 = 0x58;
    pub const SVS: u8 = 0x59;
    /// Partially line down / up.
    pub const PLD: u8 = 0x5b;
    pub const PLU: u8 = 0x5c;
    /// Colouring block.
    pub const GAA: u8 = 0x5d;
    /// Raster colour designation.
    pub const SRC: u8 = 0x5e;
    /// Set display position.
    pub const SDP: u8 = 0x5f;
    /// Active coordinate position set.
    pub const ACPS: u8 = 0x61;
    /// Switch control.
    pub const TCC: u8 = 0x62;
    /// Ornament control (the stroke).
    pub const ORN: u8 = 0x63;
    /// Font (bold / italic).
    pub const MDF: u8 = 0x64;
    /// Character font set.
    pub const CFS: u8 = 0x65;
    /// External character set.
    pub const XCS: u8 = 0x66;
    pub const SCR: u8 = 0x67;
    /// Built-in sound replay.
    pub const PRA: u8 = 0x68;
    /// Alternative character set.
    pub const ACS: u8 = 0x69;
    /// Invisible embedded data control.
    pub const UED: u8 = 0x6a;
    /// Raster colour command.
    pub const RCS: u8 = 0x6e;
    /// Skip character set.
    pub const SCS: u8 = 0x6f;
}
