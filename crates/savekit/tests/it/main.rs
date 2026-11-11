//! Tests of savekit's public API. They run on a synthetic save and on every real save in
//! `fixtures/local` at the repository root (none are needed).

mod support;

mod character;
mod checksums;
mod codec;
mod fixtures;
mod huffman;
mod inventory;
mod item_blob;
mod item_fields;
mod proto;
mod robustness;
mod verifier;
