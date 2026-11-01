//! Fixed-output compression: as much of the input as fits in the output.
//!
//! Filesystems that compress into fixed-size physical clusters, such as EROFS,
//! need this mode. The parsers are left as they are. A window of the input is
//! compressed, and the block is then cut at the last sequence that fits, so
//! the output is what `compress_hc_to_vec` chose, up to that point.

use super::compress_hc_to_vec;

const MIN_MATCH: usize = 4;
const LAST_LITERALS: usize = 5;
/// The last match starts at least this far before the end of the block.
const MATCH_START_MARGIN: usize = 12;
/// The smallest window tried, however small the output.
const MIN_WINDOW: usize = 1 << 16;

/// Extra bytes a literal or match count needs beyond its token nibble.
fn extra_bytes(count: usize) -> usize {
    if count < 15 {
        0
    } else {
        (count - 15) / 255 + 1
    }
}

/// The bytes a last sequence of `literals` literals takes.
fn final_literals_size(literals: usize) -> usize {
    1 + extra_bytes(literals) + literals
}

/// Read a count continued past its token nibble, advancing `at`.
fn read_count(block: &[u8], at: &mut usize, nibble: usize) -> usize {
    let mut count = nibble;
    if nibble == 15 {
        loop {
            let byte = block[*at];
            *at += 1;
            count += byte as usize;
            if byte != 255 {
                break;
            }
        }
    }
    count
}

/// Copy from `block`, compressed from `input`, the sequences that fit in
/// `output` along with the literals that must follow them, and fill what is
/// left with literals. Returns the input bytes taken and output bytes written.
fn cut_block(input: &[u8], block: &[u8], output: &mut [u8]) -> (usize, usize) {
    let (mut at, mut consumed) = (0usize, 0usize);
    let mut kept = 0usize;
    while at < block.len() {
        let token = block[at] as usize;
        at += 1;
        let literals = read_count(block, &mut at, token >> 4);
        at += literals;
        if at == block.len() {
            break;
        }
        at += 2;
        let length = read_count(block, &mut at, token & 15) + MIN_MATCH;
        // What must follow this sequence for the block to end legally: at
        // least five literals, with its match twelve bytes before the end.
        let tail = LAST_LITERALS.max(MATCH_START_MARGIN.saturating_sub(length));
        if at + final_literals_size(tail) > output.len() {
            break;
        }
        consumed += literals + length;
        kept = at;
    }
    output[..kept].copy_from_slice(&block[..kept]);

    let room = output.len() - kept;
    let left = input.len() - consumed;
    let mut literals = left.min(room - 1);
    while final_literals_size(literals) > room {
        literals -= 1;
    }
    output[kept] = (literals.min(15) as u8) << 4;
    let mut out = kept + 1;
    if literals >= 15 {
        let mut count = literals - 15;
        while count >= 255 {
            output[out] = 255;
            out += 1;
            count -= 255;
        }
        output[out] = count as u8;
        out += 1;
    }
    output[out..out + literals].copy_from_slice(&input[consumed..consumed + literals]);
    (consumed + literals, out + literals)
}

/// Compress as much of `input` as fits in `output`, as one standard block that
/// decodes on its own, at compression `level` as for [`compress_hc`].
///
/// Returns the input bytes taken and the output bytes written, in that order.
/// An empty `output` takes nothing and writes nothing; any other output holds
/// at least an empty block. What was not taken is the caller's to compress
/// next.
///
/// [`compress_hc`]: super::compress_hc
///
/// # Example
/// ```
/// use lz4_flex::block::{compress_hc_dest_size, decompress};
/// let input = b"Hello, this is some data to compress! ".repeat(4000);
/// let mut output = vec![0u8; 4096];
/// let (taken, written) = compress_hc_dest_size(&input, &mut output, 12);
/// assert!(written <= output.len());
/// assert_eq!(decompress(&output[..written], taken).unwrap(), input[..taken]);
/// ```
pub fn compress_hc_dest_size(input: &[u8], output: &mut [u8], level: u8) -> (usize, usize) {
    if output.is_empty() {
        return (0, 0);
    }
    let mut window = input
        .len()
        .min(MIN_WINDOW.max(output.len().saturating_mul(8)));
    loop {
        let block = compress_hc_to_vec(&input[..window], level);
        if block.len() > output.len() || window == input.len() {
            if block.len() <= output.len() {
                output[..block.len()].copy_from_slice(&block);
                return (window, block.len());
            }
            return cut_block(&input[..window], &block, output);
        }
        window = input.len().min(window.saturating_mul(2));
    }
}
