//! Huffman layer of the WSG blob (format doc section 3): MSB-first bits, a pre-order
//! serialised tree (`1` = leaf followed by 8 bits, `0` = internal node), left = 0, right = 1.

use std::collections::VecDeque;

use crate::error::{Result, SaveKitError};

fn fail(why: &str) -> SaveKitError {
    SaveKitError::CorruptHuffman(why.to_string())
}

/// A full binary tree with 256 distinct leaves has 511 nodes.
const MAX_NODES: usize = 511;
const UNSET: u16 = u16::MAX;

#[derive(Clone, Copy)]
enum Node {
    Leaf(u8),
    Internal { left: u16, right: u16 },
}

struct BitReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl BitReader<'_> {
    fn bit(&mut self) -> Result<bool> {
        let byte = *self.bytes.get(self.pos >> 3).ok_or_else(|| fail("ran out of bits"))?;
        let bit = (byte >> (7 - (self.pos & 7))) & 1 == 1;
        self.pos += 1;
        Ok(bit)
    }
}

/// Reads the tree at the start of `bits`, then decodes exactly `output_len` symbols.
/// Bits after the last symbol are ignored.
pub fn decode(bits: &[u8], output_len: usize) -> Result<Vec<u8>> {
    let mut reader = BitReader { bytes: bits, pos: 0 };

    let mut nodes: Vec<Node> = Vec::new();
    // Internal nodes still waiting for a child, innermost last.
    let mut open: Vec<u16> = Vec::new();
    loop {
        if nodes.len() >= MAX_NODES {
            return Err(fail("tree too large"));
        }
        let index = nodes.len() as u16;
        let node = if reader.bit()? {
            let mut value = 0u8;
            for _ in 0..8 {
                value = (value << 1) | u8::from(reader.bit()?);
            }
            Node::Leaf(value)
        } else {
            Node::Internal { left: UNSET, right: UNSET }
        };
        nodes.push(node);

        if let Some(&parent) = open.last() {
            if let Some(Node::Internal { left, right }) = nodes.get_mut(usize::from(parent)) {
                if *left == UNSET {
                    *left = index;
                } else {
                    *right = index;
                    open.pop();
                }
            }
        }
        if matches!(node, Node::Internal { .. }) {
            open.push(index);
        }
        if open.is_empty() {
            break;
        }
    }

    if output_len == 0 {
        return Ok(Vec::new());
    }
    let root = nodes[0];
    if matches!(root, Node::Leaf(_)) {
        return Err(fail("single-leaf tree cannot encode data"));
    }

    // Every symbol takes at least one bit, so the stream bounds the output.
    let mut out = Vec::with_capacity(output_len.min(bits.len().saturating_mul(8)));
    for _ in 0..output_len {
        let mut node = root;
        loop {
            match node {
                Node::Leaf(value) => {
                    out.push(value);
                    break;
                }
                Node::Internal { left, right } => {
                    let next = if reader.bit()? { right } else { left };
                    node = *nodes.get(usize::from(next)).ok_or_else(|| fail("broken tree"))?;
                }
            }
        }
    }
    Ok(out)
}

#[derive(Clone, Copy)]
enum Shape {
    Leaf(u8),
    Internal { left: usize, right: usize },
}

struct Weighted {
    weight: usize,
    shape: Shape,
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    current: u8,
    used: u8,
}

impl BitWriter {
    fn bit(&mut self, bit: bool) {
        self.current = (self.current << 1) | u8::from(bit);
        self.used += 1;
        if self.used == 8 {
            self.bytes.push(self.current);
            self.current = 0;
            self.used = 0;
        }
    }

    fn byte(&mut self, byte: u8) {
        for i in (0..8).rev() {
            self.bit((byte >> i) & 1 == 1);
        }
    }

    /// Pads the last byte with zero bits.
    fn finish(mut self) -> Vec<u8> {
        if self.used > 0 {
            self.bytes.push(self.current << (8 - self.used));
        }
        self.bytes
    }
}

/// Builds a tree for `data` (format doc 3.4), then writes the tree and the codes. The result is
/// padded with zero bits to a whole byte; it does not include the WSG blob's 4 trailing bytes.
pub fn encode(data: &[u8]) -> Vec<u8> {
    let mut counts = [0usize; 256];
    for &b in data {
        counts[usize::from(b)] += 1;
    }
    let mut nodes: Vec<Weighted> = (0..=255u8)
        .filter(|&v| counts[usize::from(v)] > 0)
        .map(|v| Weighted { weight: counts[usize::from(v)], shape: Shape::Leaf(v) })
        .collect();
    // A tree needs two leaves to give non-empty codes, so add unused byte values if needed.
    let mut filler = 0u8;
    while nodes.len() < 2 {
        if !nodes.iter().any(|n| matches!(n.shape, Shape::Leaf(v) if v == filler)) {
            nodes.push(Weighted { weight: 0, shape: Shape::Leaf(filler) });
        }
        filler += 1;
    }

    // Leaves in order of weight, then byte value.
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    order.sort_by_key(|&i| match nodes[i].shape {
        Shape::Leaf(value) => (nodes[i].weight, value),
        Shape::Internal { .. } => (nodes[i].weight, u8::MAX),
    });
    let mut queue: VecDeque<usize> = order.into();
    while queue.len() > 1 {
        let (Some(a), Some(b)) = (queue.pop_front(), queue.pop_front()) else { break };
        let weight = nodes[a].weight + nodes[b].weight;
        nodes.push(Weighted { weight, shape: Shape::Internal { left: a, right: b } });
        // A new internal node goes after every existing node of the same weight.
        let at = queue.partition_point(|&i| nodes[i].weight <= weight);
        queue.insert(at, nodes.len() - 1);
    }
    // There are at least two leaves, so at least one merge happened and the last one made the root.
    let root = nodes.len() - 1;

    // Pre-order walk: write the tree and collect each byte's code.
    let mut writer = BitWriter::default();
    let mut codes: Vec<Vec<bool>> = vec![Vec::new(); 256];
    let mut work: Vec<(usize, Vec<bool>)> = vec![(root, Vec::new())];
    while let Some((index, path)) = work.pop() {
        match nodes[index].shape {
            Shape::Leaf(value) => {
                writer.bit(true);
                writer.byte(value);
                codes[usize::from(value)] = path;
            }
            Shape::Internal { left, right } => {
                writer.bit(false);
                let mut right_path = path.clone();
                right_path.push(true);
                let mut left_path = path;
                left_path.push(false);
                work.push((right, right_path));
                work.push((left, left_path));
            }
        }
    }
    for &b in data {
        for &bit in &codes[usize::from(b)] {
            writer.bit(bit);
        }
    }
    writer.finish()
}
