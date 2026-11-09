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
        // Bytes of `buf` already known to hold no end tag.
        let mut searched: usize = 0;
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
            // Only the new bytes (and a tag's length before them) can hold
            // an end tag not searched yet.
            let from = searched.saturating_sub(self.close.needle().len() - 1);
            if let Some(p) = self.close.rfind(&buf[from..]) {
                let cut = from + p + self.close.needle().len();
                self.carry.extend_from_slice(&buf[cut..]);
                buf.truncate(cut);
                return Ok(Some(buf));
            }
            searched = buf.len();
            // No end tag yet (a record bigger than a block, or no records at
            // all): grow geometrically, so a long run without one costs
            // linear time, not quadratic.
            want += self.block_size.max(buf.len());
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

    #[test]
    fn long_runs_without_a_tag_stay_linear() {
        // 8 MB with one tag at the end, at block size 1: quadratic growth
        // would take minutes.
        let mut input = vec![b'x'; 8 << 20];
        input.extend_from_slice(b"</r>tail");
        let t = std::time::Instant::now();
        input.extend(std::iter::repeat_n(b'y', 32 << 20));
        let b = blocks(&input, 1);
        assert_eq!(b.concat(), input);
        assert!(b[0].ends_with(b"</r>"), "the first block ends at the tag");
        assert!(t.elapsed().as_secs() < 5);
    }
}
