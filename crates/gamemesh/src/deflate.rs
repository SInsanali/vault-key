//! A deflate compressor (RFC 1951) in a zlib wrapper (RFC 1950): LZ77 matching over a 32 KiB
//! window, then Huffman coding with per-block ("dynamic") code tables, or the fixed tables
//! when those come out smaller. Written from the two RFCs; std only.

/// Longest and shortest match deflate can express, and how far back one may reach.
const MAX_MATCH: usize = 258;
const MIN_MATCH: usize = 3;
const WINDOW: usize = 32 * 1024;
/// How many earlier positions with the same three-byte hash are tried for each match.
const MAX_CHAIN: usize = 24;
/// A match at least this long is taken without looking further.
const GOOD_ENOUGH: usize = 96;
/// A match shorter than this is dropped if the next position offers a longer one.
const LAZY_BELOW: usize = 32;
/// Tokens per block: each block gets code tables fitted to its own contents.
const BLOCK_TOKENS: usize = 48 * 1024;
const HASH_BITS: u32 = 15;

const LENGTH_BASE: [u16; 29] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LENGTH_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193,
    12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
/// The order in which a dynamic block lists the lengths of its code-length codes.
const CODE_LENGTH_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

#[derive(Clone, Copy)]
enum Token {
    Literal(u8),
    /// A copy of `length` bytes from `distance` bytes back.
    Match { length: u16, distance: u16 },
}

/// Bits are packed from the least significant bit of each byte upwards.
struct BitWriter {
    out: Vec<u8>,
    bits: u64,
    count: u32,
}

impl BitWriter {
    fn put(&mut self, value: u32, count: u32) {
        self.bits |= u64::from(value) << self.count;
        self.count += count;
        while self.count >= 8 {
            self.out.push(self.bits as u8);
            self.bits >>= 8;
            self.count -= 8;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.out.push(self.bits as u8);
        }
        self.out
    }
}

/// A Huffman code: for each symbol its length in bits (0 = unused) and its code, already
/// bit-reversed for the writer.
struct Code {
    lengths: Vec<u8>,
    codes: Vec<u16>,
}

impl Code {
    /// The canonical code for these lengths (RFC 1951 §3.2.2).
    fn from_lengths(lengths: Vec<u8>) -> Self {
        let mut count = [0u16; 16];
        for &l in &lengths {
            count[usize::from(l)] += 1;
        }
        count[0] = 0;
        let mut next = [0u16; 16];
        let mut code = 0u16;
        for bits in 1..16 {
            code = (code + count[bits - 1]) << 1;
            next[bits] = code;
        }
        let codes = lengths
            .iter()
            .map(|&l| {
                if l == 0 {
                    return 0;
                }
                let c = next[usize::from(l)];
                next[usize::from(l)] += 1;
                c.reverse_bits() >> (16 - u32::from(l))
            })
            .collect();
        Self { lengths, codes }
    }

    /// A code fitted to symbol frequencies, no code longer than `limit` bits. At least one
    /// symbol always gets a code, so the table is never empty.
    fn from_frequencies(frequencies: &[u32], limit: u8) -> Self {
        let mut freq = frequencies.to_vec();
        if freq.iter().all(|&f| f == 0) {
            freq[0] = 1;
        }
        loop {
            let lengths = huffman_lengths(&freq);
            if lengths.iter().all(|&l| l <= limit) {
                return Self::from_lengths(lengths);
            }
            // Too deep: flatten the frequencies and try again. This ends, because equal
            // frequencies give a balanced tree, which is always shallow enough here.
            for f in freq.iter_mut().filter(|f| **f > 0) {
                *f = (*f).div_ceil(2);
            }
        }
    }

    fn put(&self, w: &mut BitWriter, symbol: usize) {
        w.put(u32::from(self.codes[symbol]), u32::from(self.lengths[symbol]));
    }

    fn cost(&self, frequencies: &[u32]) -> u64 {
        frequencies.iter().zip(&self.lengths).map(|(&f, &l)| u64::from(f) * u64::from(l)).sum()
    }
}

/// Code lengths of an optimal prefix code for these frequencies (unused symbols get 0; a lone
/// used symbol gets 1).
fn huffman_lengths(freq: &[u32]) -> Vec<u8> {
    let mut lengths = vec![0u8; freq.len()];
    let mut leaves: Vec<usize> = (0..freq.len()).filter(|&i| freq[i] > 0).collect();
    if leaves.len() == 1 {
        lengths[leaves[0]] = 1;
        return lengths;
    }
    leaves.sort_by_key(|&i| freq[i]);
    // Two queues: sorted leaves and merged nodes (which come out in rising weight). Each
    // node records its parent; a leaf's depth is the number of parents above it.
    let n = leaves.len();
    let mut weight: Vec<u64> = leaves.iter().map(|&i| u64::from(freq[i])).collect();
    let mut parent = vec![usize::MAX; 2 * n - 1];
    let (mut leaf, mut node) = (0, n);
    for new in n..2 * n - 1 {
        let mut pick = |weight: &Vec<u64>| -> usize {
            let take_leaf = leaf < n && (node >= new || weight[leaf] <= weight[node]);
            if take_leaf {
                leaf += 1;
                leaf - 1
            } else {
                node += 1;
                node - 1
            }
        };
        let (a, b) = (pick(&weight), pick(&weight));
        weight.push(weight[a] + weight[b]);
        parent[a] = new;
        parent[b] = new;
    }
    for (slot, &symbol) in leaves.iter().enumerate() {
        let (mut depth, mut at) = (0u32, slot);
        while parent[at] != usize::MAX {
            at = parent[at];
            depth += 1;
        }
        lengths[symbol] = depth.min(255) as u8;
    }
    lengths
}

fn length_symbol(length: u16) -> usize {
    LENGTH_BASE.iter().rposition(|&base| base <= length).unwrap_or(0)
}

fn distance_symbol(distance: u16) -> usize {
    DIST_BASE.iter().rposition(|&base| base <= distance).unwrap_or(0)
}

fn hash3(data: &[u8], i: usize) -> usize {
    let v = u32::from(data[i]) | u32::from(data[i + 1]) << 8 | u32::from(data[i + 2]) << 16;
    (v.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
}

/// The hash chains: `head[h]` is the latest position whose three bytes hash to `h`,
/// `previous[i]` the one before position `i` with the same hash.
struct Chains {
    head: Vec<u32>,
    previous: Vec<u32>,
}

impl Chains {
    fn insert(&mut self, data: &[u8], i: usize) {
        if i + MIN_MATCH <= data.len() {
            let h = hash3(data, i);
            self.previous[i] = self.head[h];
            self.head[h] = i as u32;
        }
    }

    /// The longest earlier match for the bytes at `i`: (length, distance), length 0 for none.
    fn longest(&self, data: &[u8], i: usize) -> (usize, usize) {
        let mut best = (0, 0);
        if i + MIN_MATCH > data.len() {
            return best;
        }
        let limit = (data.len() - i).min(MAX_MATCH);
        let mut candidate = self.head[hash3(data, i)];
        let mut tries = MAX_CHAIN;
        while candidate != u32::MAX && tries > 0 {
            let c = candidate as usize;
            if i - c > WINDOW {
                break;
            }
            let len = data[c..].iter().zip(&data[i..i + limit]).take_while(|(a, b)| a == b).count();
            if len > best.0 {
                best = (len, i - c);
                if len >= GOOD_ENOUGH.min(limit) {
                    break;
                }
            }
            candidate = self.previous[c];
            tries -= 1;
        }
        best
    }
}

/// LZ77 with hash chains over three-byte prefixes. A short match is given up for a literal
/// when the next position has a longer one ("lazy" matching).
fn tokenize(data: &[u8]) -> Vec<Token> {
    let mut tokens = Vec::with_capacity(data.len() / 2 + 16);
    let mut chains = Chains { head: vec![u32::MAX; 1 << HASH_BITS], previous: vec![u32::MAX; data.len()] };
    let mut i = 0;
    let mut found = chains.longest(data, 0);
    while i < data.len() {
        let (length, distance) = found;
        chains.insert(data, i);
        if length < MIN_MATCH {
            tokens.push(Token::Literal(data[i]));
            i += 1;
            found = chains.longest(data, i);
            continue;
        }
        if length < LAZY_BELOW {
            let next = chains.longest(data, i + 1);
            if next.0 > length {
                tokens.push(Token::Literal(data[i]));
                i += 1;
                found = next;
                continue;
            }
        }
        tokens.push(Token::Match { length: length as u16, distance: distance as u16 });
        for k in i + 1..i + length {
            chains.insert(data, k);
        }
        i += length;
        found = chains.longest(data, i);
    }
    tokens
}

/// Writes one block of tokens with the smaller of fixed and dynamic codes.
fn write_block(w: &mut BitWriter, tokens: &[Token], last: bool) {
    let mut literal_freq = [0u32; 286];
    let mut distance_freq = [0u32; 30];
    let mut extra_bits = 0u64;
    for token in tokens {
        match *token {
            Token::Literal(b) => literal_freq[usize::from(b)] += 1,
            Token::Match { length, distance } => {
                let (l, d) = (length_symbol(length), distance_symbol(distance));
                literal_freq[257 + l] += 1;
                distance_freq[d] += 1;
                extra_bits += u64::from(LENGTH_EXTRA[l]) + u64::from(DIST_EXTRA[d]);
            }
        }
    }
    literal_freq[256] = 1; // end of block

    let literals = Code::from_frequencies(&literal_freq, 15);
    let distances = Code::from_frequencies(&distance_freq, 15);

    // The dynamic header: both length tables, run-length coded with the code-length alphabet.
    let literal_count = literals.lengths.iter().rposition(|&l| l != 0).map_or(257, |i| i + 1).max(257);
    let distance_count = distances.lengths.iter().rposition(|&l| l != 0).map_or(1, |i| i + 1);
    let mut all = literals.lengths[..literal_count].to_vec();
    all.extend_from_slice(&distances.lengths[..distance_count]);
    let mut runs: Vec<(usize, u32, u32)> = Vec::new(); // symbol, extra value, extra bits
    let mut i = 0;
    while i < all.len() {
        let value = all[i];
        let mut run = all[i..].iter().take_while(|&&l| l == value).count();
        i += run;
        if value == 0 {
            while run >= 11 {
                let n = run.min(138);
                runs.push((18, (n - 11) as u32, 7));
                run -= n;
            }
            if run >= 3 {
                runs.push((17, (run - 3) as u32, 3));
                run = 0;
            }
        } else if run >= 4 {
            runs.push((usize::from(value), 0, 0));
            run -= 1;
            while run >= 3 {
                let n = run.min(6);
                runs.push((16, (n - 3) as u32, 2));
                run -= n;
            }
        }
        for _ in 0..run {
            runs.push((usize::from(value), 0, 0));
        }
    }
    let mut run_freq = [0u32; 19];
    for &(symbol, _, _) in &runs {
        run_freq[symbol] += 1;
    }
    let run_code = Code::from_frequencies(&run_freq, 7);
    let run_count = CODE_LENGTH_ORDER.iter().rposition(|&s| run_code.lengths[s] != 0).map_or(4, |i| i + 1).max(4);

    let dynamic_cost = 14
        + 3 * run_count as u64
        + runs.iter().map(|&(s, _, bits)| u64::from(run_code.lengths[s]) + u64::from(bits)).sum::<u64>()
        + literals.cost(&literal_freq)
        + distances.cost(&distance_freq)
        + extra_bits;
    let (fixed_literals, fixed_distances) = fixed_codes();
    let fixed_cost = fixed_literals.cost(&literal_freq) + fixed_distances.cost(&distance_freq) + extra_bits;

    w.put(u32::from(last), 1);
    let (literals, distances) = if fixed_cost <= dynamic_cost {
        w.put(1, 2);
        (fixed_literals, fixed_distances)
    } else {
        w.put(2, 2);
        w.put((literal_count - 257) as u32, 5);
        w.put((distance_count - 1) as u32, 5);
        w.put((run_count - 4) as u32, 4);
        for &symbol in &CODE_LENGTH_ORDER[..run_count] {
            w.put(u32::from(run_code.lengths[symbol]), 3);
        }
        for &(symbol, extra, bits) in &runs {
            run_code.put(w, symbol);
            w.put(extra, bits);
        }
        (literals, distances)
    };
    for token in tokens {
        match *token {
            Token::Literal(b) => literals.put(w, usize::from(b)),
            Token::Match { length, distance } => {
                let l = length_symbol(length);
                literals.put(w, 257 + l);
                w.put(u32::from(length - LENGTH_BASE[l]), u32::from(LENGTH_EXTRA[l]));
                let d = distance_symbol(distance);
                distances.put(w, d);
                w.put(u32::from(distance - DIST_BASE[d]), u32::from(DIST_EXTRA[d]));
            }
        }
    }
    literals.put(w, 256);
}

/// The fixed code tables of RFC 1951 §3.2.6.
fn fixed_codes() -> (Code, Code) {
    let mut lengths = vec![8u8; 288];
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    (Code::from_lengths(lengths), Code::from_lengths(vec![5u8; 30]))
}

/// Compresses `data` into a raw deflate stream.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut w = BitWriter { out: Vec::with_capacity(data.len() / 4 + 64), bits: 0, count: 0 };
    let tokens = tokenize(data);
    if tokens.is_empty() {
        write_block(&mut w, &[], true);
    }
    let mut blocks = tokens.chunks(BLOCK_TOKENS).peekable();
    while let Some(block) = blocks.next() {
        write_block(&mut w, block, blocks.peek().is_none());
    }
    w.finish()
}

/// Compresses `data` into a zlib stream: header, deflate data, Adler-32 of the input.
pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x9C];
    out.extend(deflate(data));
    out.extend(adler32(data).to_be_bytes());
    out
}

pub(crate) fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 is the most bytes that can be summed before the 32-bit sums must be reduced.
    for block in data.chunks(5552) {
        for &byte in block {
            a += u32::from(byte);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

/// An independent inflater, for tests only: it shares no tables with the compressor beyond
/// the constants the RFC fixes.
#[cfg(test)]
pub(crate) mod inflate {
    use super::{CODE_LENGTH_ORDER, DIST_BASE, DIST_EXTRA, LENGTH_BASE, LENGTH_EXTRA};

    struct Bits<'a> {
        data: &'a [u8],
        pos: usize,
    }

    impl Bits<'_> {
        fn bit(&mut self) -> Result<u32, String> {
            let byte = *self.data.get(self.pos / 8).ok_or("ran out of input")?;
            let bit = (byte >> (self.pos % 8)) & 1;
            self.pos += 1;
            Ok(u32::from(bit))
        }

        fn bits(&mut self, count: u32) -> Result<u32, String> {
            let mut value = 0;
            for i in 0..count {
                value |= self.bit()? << i;
            }
            Ok(value)
        }
    }

    /// Decoding table by code length: how many codes of each length, and the symbols in
    /// canonical order.
    struct Table {
        count: [u16; 16],
        symbols: Vec<u16>,
    }

    impl Table {
        fn new(lengths: &[u8]) -> Result<Self, String> {
            let mut count = [0u16; 16];
            for &l in lengths {
                count[usize::from(l)] += 1;
            }
            count[0] = 0;
            // Reject over-subscribed codes.
            let mut left = 1i32;
            for &c in &count[1..] {
                left = (left << 1) - i32::from(c);
                if left < 0 {
                    return Err("over-subscribed code".into());
                }
            }
            let mut symbols = Vec::new();
            for len in 1..16u8 {
                symbols.extend((0..lengths.len() as u16).filter(|&s| lengths[usize::from(s)] == len));
            }
            Ok(Self { count, symbols })
        }

        fn decode(&self, bits: &mut Bits<'_>) -> Result<u16, String> {
            let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
            for len in 1..16 {
                code |= bits.bit()? as i32;
                let count = i32::from(self.count[len]);
                if code - count < first {
                    return self.symbols.get((index + code - first) as usize).copied().ok_or_else(|| "bad code".to_string());
                }
                index += count;
                first = (first + count) << 1;
                code <<= 1;
            }
            Err("code not in table".into())
        }
    }

    pub(crate) fn inflate(data: &[u8]) -> Result<Vec<u8>, String> {
        let mut bits = Bits { data, pos: 0 };
        let mut out = Vec::new();
        loop {
            let last = bits.bit()? == 1;
            match bits.bits(2)? {
                0 => {
                    bits.pos = bits.pos.div_ceil(8) * 8;
                    let len = bits.bits(16)?;
                    if bits.bits(16)? != !len & 0xFFFF {
                        return Err("stored length check".into());
                    }
                    for _ in 0..len {
                        out.push(bits.bits(8)? as u8);
                    }
                }
                kind @ (1 | 2) => {
                    let (literals, distances) = if kind == 1 {
                        let mut l = vec![8u8; 288];
                        l[144..256].fill(9);
                        l[256..280].fill(7);
                        (Table::new(&l)?, Table::new(&[5u8; 30])?)
                    } else {
                        let literal_count = bits.bits(5)? as usize + 257;
                        let distance_count = bits.bits(5)? as usize + 1;
                        let run_count = bits.bits(4)? as usize + 4;
                        let mut run_lengths = [0u8; 19];
                        for &symbol in &CODE_LENGTH_ORDER[..run_count] {
                            run_lengths[symbol] = bits.bits(3)? as u8;
                        }
                        let runs = Table::new(&run_lengths)?;
                        let mut lengths: Vec<u8> = Vec::new();
                        while lengths.len() < literal_count + distance_count {
                            match runs.decode(&mut bits)? {
                                s @ 0..=15 => lengths.push(s as u8),
                                16 => {
                                    let previous = *lengths.last().ok_or("repeat with nothing before it")?;
                                    let n = 3 + bits.bits(2)? as usize;
                                    lengths.extend(std::iter::repeat_n(previous, n));
                                }
                                17 => lengths.extend(std::iter::repeat_n(0, 3 + bits.bits(3)? as usize)),
                                _ => lengths.extend(std::iter::repeat_n(0, 11 + bits.bits(7)? as usize)),
                            }
                        }
                        if lengths.len() != literal_count + distance_count {
                            return Err("code lengths overrun".into());
                        }
                        (Table::new(&lengths[..literal_count])?, Table::new(&lengths[literal_count..])?)
                    };
                    loop {
                        let symbol = usize::from(literals.decode(&mut bits)?);
                        match symbol {
                            0..=255 => out.push(symbol as u8),
                            256 => break,
                            257..=285 => {
                                let l = symbol - 257;
                                let length = usize::from(LENGTH_BASE[l]) + bits.bits(u32::from(LENGTH_EXTRA[l]))? as usize;
                                let d = usize::from(distances.decode(&mut bits)?);
                                if d >= 30 {
                                    return Err("bad distance symbol".into());
                                }
                                let distance = usize::from(DIST_BASE[d]) + bits.bits(u32::from(DIST_EXTRA[d]))? as usize;
                                if distance > out.len() {
                                    return Err("distance before the start".into());
                                }
                                for _ in 0..length {
                                    out.push(out[out.len() - distance]);
                                }
                            }
                            _ => return Err("bad literal/length symbol".into()),
                        }
                    }
                }
                _ => return Err("reserved block type".into()),
            }
            if last {
                return Ok(out);
            }
        }
    }

    /// Unwraps a zlib stream and checks its Adler-32.
    pub(crate) fn unzlib(data: &[u8]) -> Result<Vec<u8>, String> {
        if data.len() < 6 || data[0] & 0x0F != 8 || (u16::from(data[0]) << 8 | u16::from(data[1])) % 31 != 0 {
            return Err("bad zlib header".into());
        }
        let out = inflate(&data[2..data.len() - 4])?;
        let stored = u32::from_be_bytes([data[data.len() - 4], data[data.len() - 3], data[data.len() - 2], data[data.len() - 1]]);
        if stored != super::adler32(&out) {
            return Err("Adler-32 mismatch".into());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::inflate::{inflate, unzlib};
    use super::*;

    fn noise(len: usize, mut seed: u32) -> Vec<u8> {
        (0..len)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed >> 8) as u8
            })
            .collect()
    }

    fn round_trip(data: &[u8]) -> usize {
        let packed = zlib(data);
        assert_eq!(unzlib(&packed).as_deref(), Ok(data), "{} bytes", data.len());
        packed.len()
    }

    #[test]
    fn the_test_inflater_reads_streams_it_did_not_write() {
        // "hello" as one stored block, and as fixed-Huffman output of a reference zlib.
        assert_eq!(inflate(&[0x01, 0x05, 0x00, 0xFA, 0xFF, b'h', b'e', b'l', b'l', b'o']).unwrap(), b"hello");
        assert_eq!(inflate(&[0xCB, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00]).unwrap(), b"hello");
        assert_eq!(unzlib(&[0x78, 0x9C, 0xCB, 0x48, 0xCD, 0xC9, 0xC9, 0x07, 0x00, 0x06, 0x2C, 0x02, 0x15]).unwrap(), b"hello");
        // "aaaaaaaaaa" from a reference zlib: a literal and a length-9 match at distance 1.
        assert_eq!(inflate(&[0x4B, 0x4C, 0x84, 0x01, 0x00]).unwrap(), b"aaaaaaaaaa");
        assert!(inflate(&[0x07]).is_err()); // reserved block type
        assert!(inflate(&[0xCB, 0x48]).is_err()); // truncated
    }

    #[test]
    fn round_trips_all_kinds_of_input() {
        assert_eq!(round_trip(&[]), 2 + 2 + 4);
        round_trip(&[7]);
        round_trip(b"ab");
        round_trip(b"abcabcabcabcabcabcabcabcabcabcabc");
        round_trip(&[0u8; 3]);
        // Runs longer than one match, and longer than the window.
        assert!(round_trip(&vec![0u8; 100_000]) < 200);
        let mut far = noise(40_000, 5);
        far.extend_from_within(..1_000); // a repeat beyond the 32 KiB window
        round_trip(&far);
        // Text compresses well; noise does not grow by more than the block headers.
        let text: Vec<u8> = b"the quick brown fox jumps over the lazy dog. ".iter().copied().cycle().take(20_000).collect();
        assert!(round_trip(&text) < 400);
        let random = noise(70_000, 9);
        assert!(round_trip(&random) < random.len() + random.len() / 100 + 64);
        // Several blocks, each with its own tables.
        let mut mixed = noise(60_000, 1);
        mixed.extend(std::iter::repeat_n(3u8, 60_000));
        mixed.extend((0..120_000u32).map(|i| (i % 7) as u8));
        mixed.extend(noise(30_000, 2).iter().map(|b| b & 3));
        round_trip(&mixed);
        // Every length around the match limits.
        for len in 0..600 {
            round_trip(&vec![b'x'; len]);
            round_trip(&noise(len, len as u32 + 1));
        }
    }

    #[test]
    fn skewed_frequencies_stay_within_the_length_limits() {
        // Fibonacci-like frequencies make the deepest possible trees.
        let mut freq = vec![0u32; 286];
        let (mut a, mut b) = (1u32, 1u32);
        for f in freq.iter_mut().take(40) {
            *f = a;
            (a, b) = (b, a.saturating_add(b));
        }
        let code = Code::from_frequencies(&freq, 15);
        assert!(code.lengths.iter().all(|&l| l <= 15));
        let kraft: f64 = code.lengths.iter().filter(|&&l| l > 0).map(|&l| 0.5f64.powi(i32::from(l))).sum();
        assert!(kraft <= 1.0 + 1e-9);
        let short = Code::from_frequencies(&freq[..19], 7);
        assert!(short.lengths.iter().all(|&l| l <= 7));
        // No symbols at all still gives a usable table.
        assert_eq!(Code::from_frequencies(&[0; 30], 15).lengths[0], 1);
        // And data with such frequencies survives.
        let mut data = Vec::new();
        let (mut a, mut b) = (1usize, 1usize);
        for symbol in 0..24u8 {
            data.extend(std::iter::repeat_n(symbol.wrapping_mul(37), a));
            (a, b) = (b, a + b);
        }
        let shuffled: Vec<u8> = (0..data.len()).map(|i| data[(i * 7919) % data.len()]).collect();
        round_trip(&shuffled);
    }

    #[test]
    fn checksum_matches_known_values() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(&[]), 1);
    }
}
