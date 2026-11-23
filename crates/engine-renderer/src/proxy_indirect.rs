//! The deterministic indirect-draw command core (Cycle 6-E2 — the
//! ladder's stretch package; see `docs/AUTONOMOUS-CYCLE-PROTOCOL.md`
//! §2 item 4): the packed `VkDrawIndirectCommand` bytes the GPU-side
//! instance counts live in. ONE draw call per populated proxy kind
//! with the count read from GPU memory at draw time — the submission
//! no longer carries the instance count in the command stream, so a
//! LATER GPU pass (the deferred GPU-culling package) can rewrite the
//! counts without re-recording anything. This package's writer is
//! the CPU (the frame flow, after the slot's fence wait); the
//! mechanism — `vkCmdDrawIndirect` reading a buffer — is the
//! deliverable.
//!
//! THE DETERMINISTIC CORE (the `proxy_instances` pattern, §1 of the
//! protocol): pure integer packing, no floats, no libm, no
//! platform-varying arithmetic — `u32::to_le_bytes` in a FIXED field
//! order. The bytes are therefore byte-identical on every platform
//! for the same input, and the cross-platform FNV-1a pin below is
//! the proof CI asserts on Linux and Windows alike.
//!
//! WIRE LAYOUT (why manual packing is exact): `VkDrawIndirectCommand`
//! is the C struct `{ uint32_t vertexCount; uint32_t instanceCount;
//! uint32_t firstVertex; uint32_t firstInstance; }` — four u32
//! fields in that order, no padding. Every target this workspace
//! builds is little-endian (x86_64 and aarch64 Linux/Windows, and
//! aarch64 Apple — the same `to_le_bytes` discipline the 96-byte
//! instance packing documents), so the packed bytes are the
//! structure's memory image bit for bit.
//!
//! `firstVertex` and `firstInstance` are PINNED ZERO: the per-kind
//! mesh draws from its own bound VBO (offset 0), the per-kind
//! instance range arrives through the instance-rate vertex-binding
//! OFFSET (the 6-E mechanism, unchanged), and a nonzero firstInstance
//! would require the optional `drawIndirectFirstInstance` feature —
//! deliberately NOT required, so the path runs on every conformant
//! implementation including lavapipe (the CI gate's rasterizer).
//!
//! ZERO-PER-FRAME-ALLOCATION DISCIPLINE: the packed commands are a
//! FIXED 32-byte block per frame slot (two commands, npc then
//! vehicle — the repack's fixed kind order), written into a
//! persistently mapped host-visible buffer with one disjoint region
//! per frame slot (the instance buffer's exact discipline: slot S's
//! region is written only inside the frame flow that first waited
//! slot S's fence). The block is small enough to live in a stack
//! array — no per-frame Vec, no allocation at all.

use crate::populated_meshes::{humanoid_mesh, sedan_mesh};

/// One `VkDrawIndirectCommand`'s byte size: four u32 fields
/// (vertexCount, instanceCount, firstVertex, firstInstance).
pub const INDIRECT_COMMAND_BYTES: usize = 16;

/// The proxy kinds an indirect block carries commands for, in the
/// repack's FIXED order (npc then vehicle — the instance buffer's
/// own kind order; a kind's command sits at its index times
/// [`INDIRECT_COMMAND_BYTES`]).
pub const INDIRECT_KINDS: usize = 2;

/// One frame slot's indirect-command block: one command per kind.
pub const INDIRECT_SLOT_BYTES: usize = INDIRECT_COMMAND_BYTES * INDIRECT_KINDS;

/// The byte offset of `kind`'s command within a slot's block.
///
/// Pure — pinned by tests alongside the slot-region math.
pub const fn indirect_kind_offset(kind_index: usize) -> usize {
    kind_index * INDIRECT_COMMAND_BYTES
}

/// Packs one command's 16 bytes: `vertex_count`, `instance_count`,
/// then the pinned zero `firstVertex`/`firstInstance` words, all
/// little-endian, fixed field order (the wire layout documented
/// above). Pure — the deterministic core's single packing point.
pub fn append_indirect_command(bytes: &mut [u8], vertex_count: u32, instance_count: u32) {
    let mut write = |at: usize, word: u32| {
        bytes[at..at + 4].copy_from_slice(&word.to_le_bytes());
    };
    write(0, vertex_count);
    write(4, instance_count);
    write(8, 0);
    write(12, 0);
}

/// One frame slot's full command block for a population: the NPC
/// command at offset 0, the vehicle command at offset 16 — each
/// kind's mesh vertex count (the pinned proxy-mesh constants: NPC
/// 396, vehicle 552 — the Era-1 v2 silhouettes) paired with its
/// instance count. Empty kinds
/// pack `instanceCount = 0` commands (the draw loop never submits
/// them; the block's shape stays fixed regardless).
///
/// Pure — byte-identical on every platform; the FNV-1a pin below
/// pins the shipped population's actual block. The draw-CALL model
/// (one indirect draw per populated kind — the same count the
/// direct instanced mode records) stays with the submission model
/// it extends in the vulkan module's `proxy_draw_call_count`, the
/// single source of truth.
pub fn indirect_slot_commands(npc_count: u32, vehicle_count: u32) -> [u8; INDIRECT_SLOT_BYTES] {
    let mut block = [0u8; INDIRECT_SLOT_BYTES];
    append_indirect_command(
        &mut block[indirect_kind_offset(0)..],
        humanoid_mesh().len() as u32,
        npc_count,
    );
    append_indirect_command(
        &mut block[indirect_kind_offset(1)..],
        sedan_mesh().len() as u32,
        vehicle_count,
    );
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads one little-endian u32 word out of a packed block.
    fn word(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    /// The packing is deterministic with the pinned layout: the
    /// mesh vertex counts sit in the vertexCount words, the
    /// population counts in the instanceCount words, and the
    /// firstVertex/firstInstance words are pinned ZERO (the
    /// portability contract — no `drawIndirectFirstInstance`
    /// feature, the instance range arrives through the vertex
    /// binding offset as in 6-E). Double-packing reproduces the
    /// identical bytes.
    #[test]
    fn commands_packed_deterministically_with_pinned_layout() {
        let a = indirect_slot_commands(24, 2);
        let b = indirect_slot_commands(24, 2);
        assert_eq!(a, b, "double-packing reproduces the identical bytes");
        // The proxy-mesh vertex pins (populated_meshes's own tests
        // pin 396/552 — the Era-1 v2 silhouettes; the command bytes
        // carry the same numbers).
        assert_eq!(word(&a, 0), 396, "npc vertexCount = the mesh's 396");
        assert_eq!(word(&a, 4), 24, "npc instanceCount = the population");
        assert_eq!(word(&a, 16), 552, "vehicle vertexCount = the mesh's 552");
        assert_eq!(word(&a, 20), 2, "vehicle instanceCount = the population");
        // The pinned zero words: firstVertex and firstInstance on
        // BOTH commands.
        for base in [0, 16] {
            assert_eq!(word(&a, base + 8), 0, "firstVertex pinned zero");
            assert_eq!(word(&a, base + 12), 0, "firstInstance pinned zero");
        }
        // The semantic pin tying the packed command to the direct
        // draw it replaces: cmd_draw(vertex_count, instance_count,
        // 0, 0) and the packed words agree by construction —
        // asserted so a field swap cannot pass silently.
        assert_eq!(
            (word(&a, 0), word(&a, 4)),
            (humanoid_mesh().len() as u32, 24)
        );
        assert_eq!((word(&a, 16), word(&a, 20)), (sedan_mesh().len() as u32, 2));
    }

    /// THE CROSS-PLATFORM BYTE PIN (the fingerprint-pin discipline
    /// applied to the indirect commands): the shipped corner
    /// population's command block — 24 NPCs, 2 vehicles, the pinned
    /// mesh vertex counts — hashes to this FNV-1a u64. Pure integer
    /// packing: Linux and Windows CI asserting the same value IS
    /// the determinism proof.
    #[test]
    fn slot_commands_fnv_pin() {
        let bytes = indirect_slot_commands(24, 2);
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for &byte in &bytes {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        assert_eq!(
            hash, PINNED_FNV,
            "the command-bytes hash moved: a packing change — an \
             investigation, never a silent repin"
        );
    }

    /// The measured-actual FNV-1a of the shipped population's
    /// command block (24 npc + 2 vehicle over the Era-1 v2
    /// silhouettes' 396/552 vertex counts — the population-v2
    /// package's deliberate mesh swap, the documented cause of the
    /// repin from the 6-E2 value 5608403408697128027) — measured at
    /// this package's first test run on this pod (the
    /// placeholder-pin discipline: run once, read the actual, pin
    /// it; a later mismatch is an investigation, never a silent
    /// repin).
    const PINNED_FNV: u64 = 1080195817659645834;

    /// Empty kinds pack zero-instanceCount commands — the block's
    /// shape is fixed (two commands) regardless of which kinds are
    /// populated, and an empty population's block is all
    /// zero-instance commands (nothing draws; the bytes stay
    /// deterministic).
    #[test]
    fn empty_kinds_pack_zero_instance_counts() {
        let empty = indirect_slot_commands(0, 0);
        assert_eq!(word(&empty, 0), 396, "the npc command stays present");
        assert_eq!(word(&empty, 4), 0, "npc instanceCount zero");
        assert_eq!(word(&empty, 16), 552, "the vehicle command stays present");
        assert_eq!(word(&empty, 20), 0, "vehicle instanceCount zero");
        // Half-populated: the empty kind's command is zeroed, the
        // populated kind's carries its count.
        let npc_only = indirect_slot_commands(7, 0);
        assert_eq!(word(&npc_only, 4), 7);
        assert_eq!(word(&npc_only, 20), 0);
    }

    /// The slot-region math the buffer carve relies on: one 32-byte
    /// block per frame slot, kind commands at their fixed offsets.
    #[test]
    fn slot_region_and_kind_offsets() {
        assert_eq!(INDIRECT_COMMAND_BYTES, 16, "four u32 words");
        assert_eq!(INDIRECT_KINDS, 2, "npc then vehicle");
        assert_eq!(INDIRECT_SLOT_BYTES, 32, "one command per kind");
        assert_eq!(indirect_kind_offset(0), 0);
        assert_eq!(indirect_kind_offset(1), 16);
        // The per-slot region stride the GPU buffer uses: the block
        // times the frames-in-flight constant (the instance
        // buffer's own discipline — one disjoint region per slot).
        assert_eq!(
            INDIRECT_SLOT_BYTES * crate::config::FRAMES_IN_FLIGHT as usize,
            64,
            "two frame slots of 32 bytes"
        );
    }
}
