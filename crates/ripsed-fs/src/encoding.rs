//! Source-encoding detection and transcoding.
//!
//! ripsed's engine operates on `String`s; this module converts file bytes
//! to and from that representation while remembering enough to write the
//! file back in its original encoding. Detection is BOM-based only:
//! UTF-16 without a BOM is indistinguishable from binary data by a cheap
//! check and is out of scope (such files are skipped as binary).
//!
//! No external dependency: the WHATWG Encoding Standard (encoding_rs)
//! deliberately cannot *encode* to UTF-16, while `std` provides both
//! directions (`String::from_utf16`, `str::encode_utf16`).

use std::io;

/// UTF-8 byte-order mark.
pub const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
/// UTF-16 little-endian byte-order mark.
pub const UTF16_LE_BOM: [u8; 2] = [0xFF, 0xFE];
/// UTF-16 big-endian byte-order mark.
pub const UTF16_BE_BOM: [u8; 2] = [0xFE, 0xFF];

/// The detected on-disk encoding of a file.
///
/// Carried from read to write so a file keeps its encoding (and BOM)
/// across an edit, and stored as a tag in the undo log so undo restores
/// the original bytes exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceEncoding {
    /// Plain UTF-8, no byte-order mark (the default).
    #[default]
    Utf8,
    /// UTF-8 with a leading BOM (stripped on read, re-attached on write).
    Utf8Bom,
    /// UTF-16 little-endian with BOM.
    Utf16Le,
    /// UTF-16 big-endian with BOM.
    Utf16Be,
}

impl SourceEncoding {
    /// Stable string tag for serialization (undo log).
    pub fn tag(self) -> &'static str {
        match self {
            SourceEncoding::Utf8 => "utf-8",
            SourceEncoding::Utf8Bom => "utf-8-bom",
            SourceEncoding::Utf16Le => "utf-16le",
            SourceEncoding::Utf16Be => "utf-16be",
        }
    }

    /// Parse a serialized tag; `None` for unknown tags (treat as UTF-8).
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "utf-8" => Some(SourceEncoding::Utf8),
            "utf-8-bom" => Some(SourceEncoding::Utf8Bom),
            "utf-16le" => Some(SourceEncoding::Utf16Le),
            "utf-16be" => Some(SourceEncoding::Utf16Be),
            _ => None,
        }
    }
}

/// Whether the byte prefix carries a UTF-16 BOM.
///
/// Used by binary detection: UTF-16 text is full of NUL bytes and would
/// otherwise be misclassified as binary and skipped.
pub fn has_utf16_bom(prefix: &[u8]) -> bool {
    prefix.starts_with(&UTF16_LE_BOM) || prefix.starts_with(&UTF16_BE_BOM)
}

/// Decode file bytes into text, detecting the encoding from the BOM.
///
/// Errors with `InvalidData` on malformed input (invalid UTF-8, an odd
/// number of UTF-16 payload bytes, or unpaired surrogates) — never panics.
pub fn decode(bytes: Vec<u8>) -> io::Result<(String, SourceEncoding)> {
    if bytes.starts_with(&UTF8_BOM) {
        let text = String::from_utf8(bytes[UTF8_BOM.len()..].to_vec())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        return Ok((text, SourceEncoding::Utf8Bom));
    }
    if bytes.starts_with(&UTF16_LE_BOM) {
        let text = decode_utf16(&bytes[2..], u16::from_le_bytes)?;
        return Ok((text, SourceEncoding::Utf16Le));
    }
    if bytes.starts_with(&UTF16_BE_BOM) {
        let text = decode_utf16(&bytes[2..], u16::from_be_bytes)?;
        return Ok((text, SourceEncoding::Utf16Be));
    }
    let text =
        String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok((text, SourceEncoding::Utf8))
}

fn decode_utf16(payload: &[u8], read_u16: fn([u8; 2]) -> u16) -> io::Result<String> {
    if !payload.len().is_multiple_of(2) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated UTF-16: odd number of payload bytes",
        ));
    }
    let units: Vec<u16> = payload
        .chunks_exact(2)
        .map(|pair| read_u16([pair[0], pair[1]]))
        .collect();
    String::from_utf16(&units).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Encode text back to bytes in the given encoding, re-attaching the BOM
/// that was present on read.
pub fn encode(text: &str, encoding: SourceEncoding) -> Vec<u8> {
    match encoding {
        SourceEncoding::Utf8 => text.as_bytes().to_vec(),
        SourceEncoding::Utf8Bom => {
            let mut out = Vec::with_capacity(UTF8_BOM.len() + text.len());
            out.extend_from_slice(&UTF8_BOM);
            out.extend_from_slice(text.as_bytes());
            out
        }
        SourceEncoding::Utf16Le => encode_utf16(text, &UTF16_LE_BOM, u16::to_le_bytes),
        SourceEncoding::Utf16Be => encode_utf16(text, &UTF16_BE_BOM, u16::to_be_bytes),
    }
}

fn encode_utf16(text: &str, bom: &[u8; 2], write_u16: fn(u16) -> [u8; 2]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + text.len() * 2);
    out.extend_from_slice(bom);
    for unit in text.encode_utf16() {
        out.extend_from_slice(&write_u16(unit));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_plain_utf8() {
        let (text, enc) = decode(b"hello\n".to_vec()).unwrap();
        assert_eq!(text, "hello\n");
        assert_eq!(enc, SourceEncoding::Utf8);
    }

    #[test]
    fn decode_utf8_bom_strips_bom() {
        let mut bytes = UTF8_BOM.to_vec();
        bytes.extend_from_slice(b"hello\n");
        let (text, enc) = decode(bytes).unwrap();
        assert_eq!(text, "hello\n", "BOM must not appear as content");
        assert_eq!(enc, SourceEncoding::Utf8Bom);
    }

    #[test]
    fn utf16le_roundtrip() {
        let original = "héllo wörld\r\nsecond ünïcode line\n";
        let bytes = encode(original, SourceEncoding::Utf16Le);
        assert!(bytes.starts_with(&UTF16_LE_BOM));
        let (text, enc) = decode(bytes.clone()).unwrap();
        assert_eq!(text, original);
        assert_eq!(enc, SourceEncoding::Utf16Le);
        assert_eq!(encode(&text, enc), bytes, "byte-exact roundtrip");
    }

    #[test]
    fn utf16be_roundtrip() {
        let original = "emoji 🎉 and CJK 漢字\n";
        let bytes = encode(original, SourceEncoding::Utf16Be);
        assert!(bytes.starts_with(&UTF16_BE_BOM));
        let (text, enc) = decode(bytes.clone()).unwrap();
        assert_eq!(text, original);
        assert_eq!(enc, SourceEncoding::Utf16Be);
        assert_eq!(encode(&text, enc), bytes);
    }

    #[test]
    fn utf16_surrogate_pairs_survive() {
        // 🎉 encodes as a surrogate pair in UTF-16.
        let bytes = encode("🎉", SourceEncoding::Utf16Le);
        assert_eq!(bytes.len(), 2 + 4); // BOM + two u16 units
        let (text, _) = decode(bytes).unwrap();
        assert_eq!(text, "🎉");
    }

    #[test]
    fn truncated_utf16_is_clean_error() {
        let mut bytes = UTF16_LE_BOM.to_vec();
        bytes.extend_from_slice(&[0x68, 0x00, 0x65]); // "he" minus a byte
        let err = decode(bytes).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("truncated"));
    }

    #[test]
    fn unpaired_surrogate_is_clean_error() {
        let mut bytes = UTF16_LE_BOM.to_vec();
        bytes.extend_from_slice(&0xD800u16.to_le_bytes()); // lone high surrogate
        let err = decode(bytes).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn invalid_utf8_is_clean_error() {
        // FF FF is not a BOM (LE is FF FE, BE is FE FF), so this takes the
        // UTF-8 path and fails validation there.
        let err = decode(vec![0xFF, 0xFF, 0xFF]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn empty_inputs() {
        assert_eq!(
            decode(Vec::new()).unwrap(),
            (String::new(), SourceEncoding::Utf8)
        );
        // BOM-only files decode to empty text with the right encoding.
        let (text, enc) = decode(UTF16_LE_BOM.to_vec()).unwrap();
        assert_eq!(text, "");
        assert_eq!(enc, SourceEncoding::Utf16Le);
    }

    #[test]
    fn has_utf16_bom_detection() {
        assert!(has_utf16_bom(&[0xFF, 0xFE, 0x00]));
        assert!(has_utf16_bom(&[0xFE, 0xFF]));
        assert!(!has_utf16_bom(&UTF8_BOM));
        assert!(!has_utf16_bom(b"text"));
        assert!(!has_utf16_bom(&[]));
    }

    #[test]
    fn tags_roundtrip() {
        for enc in [
            SourceEncoding::Utf8,
            SourceEncoding::Utf8Bom,
            SourceEncoding::Utf16Le,
            SourceEncoding::Utf16Be,
        ] {
            assert_eq!(SourceEncoding::from_tag(enc.tag()), Some(enc));
        }
        assert_eq!(SourceEncoding::from_tag("latin-1"), None);
    }
}
