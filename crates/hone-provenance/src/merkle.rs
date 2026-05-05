use sha2::{Digest, Sha256};

/// SHA-256 Merkle root over a list of hex-encoded leaf hashes.
///
/// Construction: leaves are sorted alphabetically for determinism (so the
/// same set of leaves always produces the same root regardless of insertion
/// order). At each level, pairs are concatenated (left + right) and SHA-256'd.
/// If the level has an odd number of nodes, the last node is duplicated
/// (Bitcoin convention).
///
/// Edge cases:
/// - empty leaves -> returns the genesis hash ("0".repeat(64))
/// - single leaf -> that leaf is the root (returned unchanged)
pub fn merkle_root(leaves: &[String]) -> String {
    if leaves.is_empty() {
        return "0".repeat(64);
    }

    let mut level: Vec<String> = leaves.to_vec();
    level.sort();

    if level.len() == 1 {
        return level.into_iter().next().unwrap();
    }

    while level.len() > 1 {
        // Duplicate last node when count is odd (Bitcoin convention).
        if level.len() % 2 == 1 {
            let last = level.last().unwrap().clone();
            level.push(last);
        }

        let mut next = Vec::with_capacity(level.len() / 2);
        for chunk in level.chunks(2) {
            let left = hex::decode(&chunk[0]).unwrap_or_default();
            let right = hex::decode(&chunk[1]).unwrap_or_default();
            let mut combined = Vec::with_capacity(left.len() + right.len());
            combined.extend_from_slice(&left);
            combined.extend_from_slice(&right);
            let hash = Sha256::digest(&combined);
            next.push(hex::encode(hash));
        }
        level = next;
    }

    level.into_iter().next().unwrap()
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn hash_concat(a: &str, b: &str) -> String {
        let la = hex::decode(a).unwrap();
        let lb = hex::decode(b).unwrap();
        let mut combined = Vec::with_capacity(la.len() + lb.len());
        combined.extend_from_slice(&la);
        combined.extend_from_slice(&lb);
        hex::encode(Sha256::digest(&combined))
    }

    fn leaf(s: &str) -> String {
        s.to_owned()
    }

    #[test]
    fn merkle_root_empty_returns_genesis() {
        let root = merkle_root(&[]);
        assert_eq!(root, "0".repeat(64));
        assert_eq!(root.len(), 64);
    }

    #[test]
    fn merkle_root_single_leaf_returns_leaf() {
        let h = "a".repeat(64);
        let root = merkle_root(&[h.clone()]);
        assert_eq!(root, h);
    }

    #[test]
    fn merkle_root_two_leaves_returns_hash_of_concat() {
        // Leaves are sorted before combining; "aa..." < "bb..."
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let expected = hash_concat(&a, &b);
        let root = merkle_root(&[a.clone(), b.clone()]);
        assert_eq!(root, expected, "two-leaf root must equal SHA256(a||b)");
    }

    #[test]
    fn merkle_root_three_leaves_duplicates_last() {
        // After sorting: a, b, c -> odd count -> duplicate c -> pairs: (a,b), (c,c)
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let c = "c".repeat(64);
        let h_ab = hash_concat(&a, &b);
        let h_cc = hash_concat(&c, &c);
        let expected = hash_concat(&h_ab, &h_cc);
        let root = merkle_root(&[c.clone(), a.clone(), b.clone()]);
        assert_eq!(root, expected);
    }

    #[test]
    fn merkle_root_four_leaves_balances_correctly() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let c = "c".repeat(64);
        let d = "d".repeat(64);
        // sorted: a, b, c, d -> pairs (a,b) and (c,d) -> hash those two
        let h_ab = hash_concat(&a, &b);
        let h_cd = hash_concat(&c, &d);
        let expected = hash_concat(&h_ab, &h_cd);
        let root = merkle_root(&[d.clone(), b.clone(), c.clone(), a.clone()]);
        assert_eq!(root, expected);
    }

    #[test]
    fn merkle_root_is_deterministic_across_orderings() {
        let a = "1".repeat(64);
        let b = "2".repeat(64);
        let c = "3".repeat(64);

        let r1 = merkle_root(&[a.clone(), b.clone(), c.clone()]);
        let r2 = merkle_root(&[c.clone(), a.clone(), b.clone()]);
        let r3 = merkle_root(&[b.clone(), c.clone(), a.clone()]);

        assert_eq!(r1, r2, "different orderings must produce the same root (r1 vs r2)");
        assert_eq!(r1, r3, "different orderings must produce the same root (r1 vs r3)");
    }

    #[test]
    fn merkle_root_changes_when_any_leaf_changes() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let c = "c".repeat(64);

        let root_orig = merkle_root(&[a.clone(), b.clone(), c.clone()]);
        // Change last leaf to "d" repeated
        let d = "d".repeat(64);
        let root_changed = merkle_root(&[a.clone(), b.clone(), d.clone()]);

        assert_ne!(
            root_orig, root_changed,
            "changing one leaf must change the Merkle root"
        );
    }

    #[test]
    fn merkle_root_returns_64_char_hex() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let root = merkle_root(&[a, b]);
        assert_eq!(root.len(), 64, "Merkle root must always be 64 hex chars");
        assert!(
            root.chars().all(|c| c.is_ascii_hexdigit()),
            "Merkle root must be all hex digits"
        );
    }
}
