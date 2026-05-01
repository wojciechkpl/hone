//! Human-readable identifiers for memory entries.
//!
//! `friendly_memory_key` returns the description when present and non-blank,
//! otherwise a synthetic `<kind>-<HHMM>-<XX>` label derived from the memory's
//! kind, created_at timestamp, and id. The format matches `agent_name`'s
//! suffix convention so memory recall lines look at home next to session/
//! agent labels.

use hone_memory::{Memory, MemoryKind};

pub fn friendly_memory_key(memory: &Memory) -> String {
    if let Some(desc) = &memory.description {
        let trimmed = desc.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let secs_of_day = memory.created_at % 86_400;
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;
    let kind = kind_label_lower(memory.kind);
    let suffix = hash_id_to_byte(&memory.id);
    format!("{kind}-{hh:02}{mm:02}-{suffix:02x}")
}

fn kind_label_lower(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Conversation => "conversation",
        MemoryKind::User => "user",
        MemoryKind::Feedback => "feedback",
        MemoryKind::Project => "project",
        MemoryKind::Reference => "reference",
        MemoryKind::Outcome => "outcome",
    }
}

fn hash_id_to_byte(id: &str) -> u8 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a basis
    for b in id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mixed = h.rotate_left(13).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    (mixed >> 56) as u8
}
