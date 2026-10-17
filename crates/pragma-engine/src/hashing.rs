//! Content hashes and text normalization shared by anchors, ids and keys.

/// A short, stable content hash: the first 16 hex characters of BLAKE3.
pub fn short_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex()[..16].to_string()
}

/// Collapses whitespace runs to one space and trims, so reformatting changes no key.
pub fn normalize_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_hash_is_stable_and_short() {
        assert_eq!(short_hash(b"x"), short_hash(b"x"));
        assert_ne!(short_hash(b"x"), short_hash(b"y"));
        assert_eq!(short_hash(b"x").len(), 16);
    }

    #[test]
    fn normalize_ws_collapses_whitespace() {
        assert_eq!(normalize_ws("  a\n\t b  c "), "a b c");
    }
}
