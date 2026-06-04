//! Minimal, pure-Rust SentencePiece detokenizer.
//!
//! We deliberately avoid the `sentencepiece` crate: it links the native
//! SentencePiece C++ library, which is hostile to the future `wasm32` target.
//! For detokenization we only need the ordered list of token *pieces* — field
//! 1 of the `ModelProto`, each a `SentencePiece` sub-message whose field 1 is
//! the piece string. This is a tiny hand-rolled protobuf reader that extracts
//! exactly that, nothing else.
//!
//! Detokenization follows SentencePiece convention: the meta symbol `▁`
//! (U+2581) marks a leading space, so we replace it with a space and
//! `trim_start` the assembled string.

use crate::error::{Error, Result};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// The `▁` (lower one-eighth block) meta symbol SentencePiece uses for spaces.
const SPACE_META: char = '\u{2581}';

/// An ordered SentencePiece vocabulary: `id -> piece string`.
pub struct SentencePieceVocab {
    pieces: Vec<String>,
}

impl SentencePieceVocab {
    /// Load and parse a `tokenizer.model` (SentencePiece `ModelProto`) file.
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut file = File::open(path.as_ref())
            .map_err(|e| Error::Tokenizer(format!("failed to open tokenizer.model: {e}")))?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|e| Error::Tokenizer(format!("failed to read tokenizer.model: {e}")))?;
        Self::from_bytes(&data)
    }

    /// Parse a SentencePiece `ModelProto` from raw bytes.
    ///
    /// Useful on wasm32, where the `tokenizer.model` bytes are fetched by JS
    /// and handed in directly (there is no filesystem). [`Self::from_file`]
    /// simply reads the file and delegates here, so both paths share identical
    /// parsing.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let pieces = parse_model_proto(data)?;
        if pieces.is_empty() {
            return Err(Error::Tokenizer(
                "no pieces found in tokenizer.model".into(),
            ));
        }
        Ok(Self { pieces })
    }

    /// Number of pieces in the vocabulary.
    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    /// Whether the vocabulary is empty.
    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    /// Detokenize a sequence of token ids into text.
    ///
    /// Out-of-range ids are skipped. The `▁` meta symbol becomes a space and a
    /// single leading space (from a word-initial piece) is trimmed.
    pub fn decode(&self, ids: &[usize]) -> String {
        let mut out = String::new();
        for &id in ids {
            if let Some(piece) = self.pieces.get(id) {
                out.push_str(&piece.replace(SPACE_META, " "));
            }
        }
        out.trim_start().to_string()
    }

    /// Detokenize a single token id (no trimming), useful for streaming
    /// partials.
    pub fn decode_single(&self, id: usize) -> String {
        self.pieces
            .get(id)
            .map(|p| p.replace(SPACE_META, " "))
            .unwrap_or_default()
    }
}

/// Parse the top-level `ModelProto`, collecting field-1 `SentencePiece`
/// sub-messages in order.
fn parse_model_proto(data: &[u8]) -> Result<Vec<String>> {
    let mut pieces = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        let (header, used) = read_varint(&data[pos..])?;
        pos += used;
        let field_num = header >> 3;
        let wire_type = header & 0x7;

        match (field_num, wire_type) {
            // field 1, length-delimited => a `SentencePiece` message.
            (1, 2) => {
                let (len, used) = read_varint(&data[pos..])?;
                pos += used;
                let end = pos + len as usize;
                if end > data.len() {
                    break;
                }
                if let Ok(piece) = parse_piece_message(&data[pos..end]) {
                    pieces.push(piece);
                }
                pos = end;
            }
            // Skip any other field by wire type.
            _ => pos = skip_field(data, pos, wire_type)?,
        }
    }

    Ok(pieces)
}

/// Parse a `SentencePiece` sub-message, returning its field-1 piece string.
fn parse_piece_message(data: &[u8]) -> Result<String> {
    let mut pos = 0;
    let mut piece = String::new();

    while pos < data.len() {
        let (header, used) = read_varint(&data[pos..])?;
        pos += used;
        let field_num = header >> 3;
        let wire_type = header & 0x7;

        if field_num == 1 && wire_type == 2 {
            let (len, used) = read_varint(&data[pos..])?;
            pos += used;
            let end = pos + len as usize;
            if end <= data.len() {
                piece = String::from_utf8_lossy(&data[pos..end]).into_owned();
            }
            pos = end;
        } else {
            pos = skip_field(data, pos, wire_type)?;
        }
    }

    Ok(piece)
}

/// Advance past a single protobuf field given its wire type.
fn skip_field(data: &[u8], mut pos: usize, wire_type: u64) -> Result<usize> {
    match wire_type {
        // varint
        0 => {
            let (_, used) = read_varint(&data[pos..])?;
            pos += used;
        }
        // 64-bit
        1 => pos += 8,
        // length-delimited
        2 => {
            let (len, used) = read_varint(&data[pos..])?;
            pos += used + len as usize;
        }
        // 32-bit
        5 => pos += 4,
        other => return Err(Error::Tokenizer(format!("unknown wire type {other}"))),
    }
    Ok(pos)
}

/// Read a base-128 varint, returning `(value, bytes_consumed)`.
fn read_varint(data: &[u8]) -> Result<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0;
    let mut pos = 0;

    // varints are at most 10 bytes for a 64-bit value.
    while pos < data.len() && pos < 10 {
        let byte = data[pos];
        result |= ((byte & 0x7F) as u64) << shift;
        pos += 1;
        if byte & 0x80 == 0 {
            return Ok((result, pos));
        }
        shift += 7;
    }
    Err(Error::Tokenizer("invalid varint".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_replaces_meta_and_trims_leading_space() {
        let vocab = SentencePieceVocab {
            pieces: vec![
                "\u{2581}the".to_string(),
                "\u{2581}quick".to_string(),
                "er".to_string(),
            ],
        };
        // "▁the" + "▁quick" + "er" => " the quicker" => trimmed "the quicker".
        assert_eq!(vocab.decode(&[0, 1, 2]), "the quicker");
    }

    #[test]
    fn out_of_range_ids_are_skipped() {
        let vocab = SentencePieceVocab {
            pieces: vec!["\u{2581}hi".to_string()],
        };
        assert_eq!(vocab.decode(&[0, 999]), "hi");
    }
}
