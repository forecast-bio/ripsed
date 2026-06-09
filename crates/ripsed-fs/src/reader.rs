use crate::encoding::{self, SourceEncoding};
use memmap2::Mmap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Threshold for using memory-mapped I/O (1 MB).
const MMAP_THRESHOLD: u64 = 1024 * 1024;

/// Size of the sample checked by `is_binary` (8 KB).
const BINARY_CHECK_SIZE: usize = 8192;

/// Read a file's contents as a string, dropping the detected encoding.
///
/// Prefer [`read_file_with_encoding`] when the content will be written
/// back — writing text read from a UTF-16 file as UTF-8 mangles it.
pub fn read_file(path: &Path) -> std::io::Result<String> {
    read_file_with_encoding(path).map(|(text, _)| text)
}

/// Read a file's contents as a string along with its detected
/// [`SourceEncoding`] (BOM-based: UTF-8 with/without BOM, UTF-16 LE/BE).
///
/// Uses memory-mapped I/O for large files, regular reads for small ones.
pub fn read_file_with_encoding(path: &Path) -> std::io::Result<(String, SourceEncoding)> {
    let metadata = std::fs::metadata(path)?;
    let size = metadata.len();

    let bytes = if size >= MMAP_THRESHOLD {
        read_mmap_bytes(path)?
    } else {
        let mut file = File::open(path)?;
        let mut content = Vec::with_capacity(size as usize);
        file.read_to_end(&mut content)?;
        content
    };
    encoding::decode(bytes)
}

fn read_mmap_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = File::open(path)?;
    // SAFETY: The mapping is immediately copied into an owned Vec via `.to_vec()`,
    // so the mmap is live for only the duration of that copy. A concurrent writer
    // using rename-based atomic writes (as ripsed does) cannot affect the mapped
    // inode during this window. The resulting Vec is a stable snapshot.
    let mmap = unsafe { Mmap::map(&file)? };
    Ok(mmap.to_vec())
}

/// Check if a file appears to be binary by looking for null bytes in the
/// first 8 KB of the file.
///
/// Files starting with a UTF-16 BOM are exempt: UTF-16 text is full of
/// NUL bytes but is decodable text, not binary.
pub fn is_binary(path: &Path) -> std::io::Result<bool> {
    let mut file = File::open(path)?;
    let mut buffer = [0u8; BINARY_CHECK_SIZE];
    let bytes_read = file.read(&mut buffer)?;
    let prefix = &buffer[..bytes_read];
    if encoding::has_utf16_bom(prefix) {
        return Ok(false);
    }
    Ok(prefix.contains(&0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ---- Binary detection tests ----

    #[test]
    fn is_binary_detects_null_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bin.dat");
        fs::write(&path, b"\x00\x01\x02\x03").unwrap();

        assert!(is_binary(&path).unwrap());
    }

    #[test]
    fn is_binary_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text.txt");
        fs::write(&path, "just text\n").unwrap();

        assert!(!is_binary(&path).unwrap());
    }

    #[test]
    fn is_binary_null_at_position_after_512() {
        // Ensures we check beyond the old 512-byte boundary
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sneaky.bin");
        let mut data = vec![b'A'; 1000];
        data[999] = 0; // null byte at position 999
        fs::write(&path, &data).unwrap();

        assert!(is_binary(&path).unwrap());
    }

    #[test]
    fn is_binary_null_just_inside_8kb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("edge.bin");
        let mut data = vec![b'X'; BINARY_CHECK_SIZE];
        data[BINARY_CHECK_SIZE - 1] = 0;
        fs::write(&path, &data).unwrap();

        assert!(is_binary(&path).unwrap());
    }

    #[test]
    fn is_binary_null_just_past_8kb() {
        // Null byte at position 8192 is beyond our check window
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("past.bin");
        let mut data = vec![b'X'; BINARY_CHECK_SIZE + 1];
        data[BINARY_CHECK_SIZE] = 0;
        fs::write(&path, &data).unwrap();

        assert!(!is_binary(&path).unwrap());
    }

    #[test]
    fn is_binary_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.bin");
        fs::write(&path, "").unwrap();

        assert!(!is_binary(&path).unwrap());
    }

    // ---- read_file tests ----

    #[test]
    fn read_file_small() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.txt");
        fs::write(&path, "hello").unwrap();

        assert_eq!(read_file(&path).unwrap(), "hello");
    }

    #[test]
    fn read_file_large_uses_mmap_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        // Create a file larger than MMAP_THRESHOLD (1 MB + 1 byte)
        let content = "x".repeat(MMAP_THRESHOLD as usize + 1);
        fs::write(&path, &content).unwrap();

        let result = read_file(&path).unwrap();
        assert_eq!(result.len(), MMAP_THRESHOLD as usize + 1);
        assert_eq!(result, content);
    }
}
