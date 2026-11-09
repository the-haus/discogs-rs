//! The splitter: decompressed bytes cut into blocks of whole records.
//!
//! A block is filled to about the block size, then cut right after the
//! last record end tag in it (`</release>` or `</master>`, found with
//! `memchr::memmem` from the end). The bytes after the cut start the next
//! block. A block with no end tag grows until it has one (a record bigger
//! than the block size), and the last block is whatever is left.
//!
//! Scanning the blocks one by one finds exactly the records a scan of the
//! whole input finds: a record runs from its start tag to the first end
//! tag after it, so no record spans a cut placed right after an end tag.

use std::io::{self, Read};

use memchr::memmem::FinderRev;

/// The default block size: 16 MB.
pub const DEFAULT_BLOCK_SIZE: usize = 16 << 20;

/// Cuts a reader's bytes into blocks of whole records.
pub struct Splitter<R> {
    reader: R,
    close: FinderRev<'static>,
    block_size: usize,
    carry: Vec<u8>,
    eof: bool,
    /// Bytes read from the reader.
    pub bytes: u64,
}

impl<R: Read> Splitter<R> {
    /// A splitter cutting after `close_tag`, in blocks of about `block_size`
    /// bytes (at least 1).
    pub fn new(reader: R, close_tag: &'static [u8], block_size: usize) -> Self {
        Self {
            reader,
            close: FinderRev::new(close_tag),
            block_size: block_size.max(1),
            carry: Vec::new(),
            eof: false,
            bytes: 0,
        }
    }

    /// The next block, built in `buf` (cleared first; pass a recycled one
    /// to save allocating). `None` at the end.
    pub fn next_block(&mut self, mut buf: Vec<u8>) -> io::Result<Option<Vec<u8>>> {
        buf.clear();
        buf.append(&mut self.carry);
        let mut want = self.block_size;
        loop {
            while buf.len() < want && !self.eof {
                let need = (want - buf.len()) as u64;
                let got = (&mut self.reader).take(need).read_to_end(&mut buf)? as u64;
                self.bytes += got;
                if got < need {
                    self.eof = true;
                }
            }
            if self.eof {
                return Ok((!buf.is_empty()).then_some(buf));
            }
            if let Some(p) = self.close.rfind(&buf) {
                let cut = p + self.close.needle().len();
                self.carry.extend_from_slice(&buf[cut..]);
                buf.truncate(cut);
                return Ok(Some(buf));
            }
            want += self.block_size;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(input: &[u8], size: usize) -> Vec<Vec<u8>> {
        let mut s = Splitter::new(input, b"</r>", size);
        let mut out = Vec::new();
        while let Some(b) = s.next_block(Vec::new()).unwrap() {
            out.push(b);
        }
        assert_eq!(s.bytes, input.len() as u64);
        out
    }

    #[test]
    fn cuts_after_end_tags() {
        let input = b"<x><r>1</r><r>22</r><r>333</r></x>";
        for size in 1..input.len() + 2 {
            let b = blocks(input, size);
            assert_eq!(b.concat(), input, "size {size}");
            for block in &b[..b.len() - 1] {
                assert!(block.ends_with(b"</r>"), "size {size}");
            }
        }
        assert_eq!(blocks(b"", 4).len(), 0);
        assert_eq!(blocks(b"no tags at all", 4), vec![b"no tags at all".to_vec()]);
    }
}
