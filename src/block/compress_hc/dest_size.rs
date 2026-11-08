//! Fixed-output compression: as much of the input as fits in the output.
//!
//! Filesystems that compress into fixed-size physical clusters, such as EROFS,
//! need this mode. The block is cut at the last sequence that fits, so the
//! output is what the parser chose, up to that point. The optimal parser stops
//! once the output is full. The others compress a window of the input.

use super::hash_chain::HashTableHCU32;
use super::optimal::parse_optimal_sequences;
use super::{
    compress_hc_to_vec, hc_level_params, HcCompressionStrategy, HcLevelParams, LZ4_OPT_NUM,
};
use crate::block::handle_last_literals;
use crate::sink::{Sink, SliceSink};
use alloc::vec;

const MIN_MATCH: usize = 4;
const LAST_LITERALS: usize = 5;
/// The last match starts at least this far before the end of the block.
const MATCH_START_MARGIN: usize = 12;

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

/// The first window to compress for an output of `output_length` bytes: a
/// little more than it holds, so incompressible input overflows it and only
/// what is cut off is wasted. Input that compresses fits, and the window grows.
fn first_window(input_length: usize, output_length: usize) -> usize {
    input_length.min(output_length.saturating_add(output_length / 32 + 32))
}

/// The bytes the optimal parser can write when asked to stop at
/// `output_length`. It checks between steps, so one step can run past: the
/// literals before it, the at most `LZ4_OPT_NUM` positions it parses, and a
/// last match as long as the input.
fn parse_capacity(input_length: usize, output_length: usize) -> usize {
    output_length + output_length / 255 + LZ4_OPT_NUM * 2 + input_length / 255 + 64
}

/// [`compress_hc_dest_size`] for the optimal parser, which stops searching
/// when the output is full.
fn optimal_dest_size(input: &[u8], output: &mut [u8], params: HcLevelParams) -> (usize, usize) {
    let mut hash_table = HashTableHCU32::new(params.max_attempts, input.len());
    let mut block = vec![0u8; parse_capacity(input.len(), output.len())];
    let mut sink = SliceSink::new(&mut block, 0);
    let literal_start = parse_optimal_sequences(
        input,
        0,
        &mut sink,
        params,
        &mut hash_table,
        &[],
        0,
        output.len(),
    );
    if let Some(literal_start) = literal_start {
        handle_last_literals(&mut sink, &input[literal_start..]);
    }
    let written = sink.pos();
    if literal_start.is_some() && written <= output.len() {
        output[..written].copy_from_slice(&block[..written]);
        return (input.len(), written);
    }
    cut_block(input, &block[..written], output)
}

/// Compress as much of `input` as fits in `output`, as one standard block that
/// decodes on its own, at compression `level` as for [`compress_hc_to_vec`].
///
/// Returns the input bytes taken and the output bytes written, in that order.
/// An empty `output` takes nothing and writes nothing; any other output holds
/// at least an empty block. What was not taken is the caller's to compress
/// next.
///
/// [`compress_hc_to_vec`]: super::compress_hc_to_vec
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
    let params = hc_level_params(level.min(12));
    if params.strategy == HcCompressionStrategy::Optimal {
        return optimal_dest_size(input, output, params);
    }
    let mut window = first_window(input.len(), output.len());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::decompress;
    use alloc::vec::Vec;

    fn noise(length: usize) -> Vec<u8> {
        let mut state = 88172645463325252u64;
        (0..length)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    }

    /// Input that makes one parser step write as much as it can: noise up to
    /// the limit, then a long match or many short ones.
    fn overshooting_inputs() -> Vec<Vec<u8>> {
        let lead = noise(4096);
        let mut long_match = lead.clone();
        long_match.resize(lead.len() + (4 << 20), 0);
        let mut short_matches = lead.clone();
        let tail = noise(1 << 16);
        for chunk in tail.chunks(5) {
            short_matches.extend_from_slice(&lead[..4]);
            short_matches.push(chunk[0]);
        }
        let mut literals_before_a_match = noise(1 << 16);
        let start = literals_before_a_match[..8192].to_vec();
        literals_before_a_match.extend_from_slice(&start);
        Vec::from([long_match, short_matches, literals_before_a_match])
    }

    fn parse(input: &[u8], limit: usize) -> (Option<usize>, usize, usize) {
        let params = hc_level_params(12);
        let mut hash_table = HashTableHCU32::new(params.max_attempts, input.len());
        let mut block = vec![0u8; parse_capacity(input.len(), limit)];
        let mut sink = SliceSink::new(&mut block, 0);
        let literal_start =
            parse_optimal_sequences(input, 0, &mut sink, params, &mut hash_table, &[], 0, limit);
        (literal_start, sink.pos(), hash_table.positions_inserted())
    }

    /// The point of stopping: input past a full output is never searched.
    #[test]
    fn the_optimal_parser_does_not_search_past_a_full_output() {
        const OUTPUT: usize = 4096;
        let (literal_start, _, inserted) = parse(&noise(1 << 20), OUTPUT);
        assert_eq!(literal_start, None);
        assert!(inserted <= OUTPUT + 16, "searched {inserted} positions");
    }

    /// The parser checks only between steps, and the scratch block has to
    /// hold what the step that fills it writes past the limit.
    #[test]
    fn the_step_that_fills_the_output_stays_within_the_scratch_block() {
        for input in overshooting_inputs() {
            for limit in (1..64).chain((3900..4300).step_by(7)).chain([8192, 32768]) {
                let (_, written, _) = parse(&input, limit);
                assert!(written <= parse_capacity(input.len(), limit), "{limit}");

                let mut output = vec![0u8; limit];
                let (taken, written) = compress_hc_dest_size(&input, &mut output, 12);
                let back = decompress(&output[..written], taken).unwrap();
                assert!(back == input[..taken], "{limit}");
            }
        }
    }

    /// Whatever is compressed past the point where the output is full is
    /// thrown away, so the first window is only a little more than the
    /// output holds; compressible input earns a bigger one by fitting.
    #[test]
    fn the_first_window_is_little_more_than_the_output_holds() {
        const OUTPUT: usize = 32 << 10;
        let window = first_window(1 << 20, OUTPUT);
        assert!(window > OUTPUT, "{window}");
        assert!(window <= OUTPUT + OUTPUT / 16, "{window}");
    }

    #[test]
    fn the_first_window_is_never_more_than_the_input() {
        assert_eq!(first_window(100, 32 << 10), 100);
    }
}
