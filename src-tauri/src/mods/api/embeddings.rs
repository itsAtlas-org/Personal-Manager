// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A mod's view of the document embeddings (Mods spec §1.5), with the `documents.vectors`
//! permission, because a vector can be inverted back toward its text: `embeddings.documentVectors`
//! and `embeddings.signature`. The decode, average, normalise and node-cap work is
//! `doc_vectors.rs`, which the core Map reads too.
//!
//! **The vectors travel as bytes, not JSON** (spec §1.5: 1.5 MB at the defaults, 20 MB at the
//! maximum). The reply is [`super::Reply::Binary`], handed to the webview as a raw IPC body, and PM's
//! page transfers the buffer to the frame without copying (`modBridge.ts`'s `readBinary`). The layout,
//! all little-endian:
//!
//! ```text
//! offset 0                  u32  version (1)
//!        4                  u32  count: documents
//!        8                  u32  dimension: floats per document
//!        12                 u32  0 (reserved)
//!        16                 f64 x count          document ids, in the order the vectors follow
//!        16 + 8 x count     f32 x count x dimension   the vectors, document by document
//! ```
//!
//! Ids are `f64` because a document id is a JavaScript number everywhere in PM, and the layout puts
//! them on an 8-byte boundary and the floats on a 4-byte one, so a frame can view the buffer with a
//! `Float64Array` and a `Float32Array` as it is. An id past 2^53 (never, for a row counter) is a PM
//! failure, not a rounded id.
//!
//! **Sized by construction.** The most documents an answer can carry is what fits under
//! [`limits::MAX_BINARY_RESULT_BYTES`] at this vault's dimension ([`max_documents`]); a request is cut
//! to that by the node cap's own order (most important, most substantial), so it is never refused as
//! too large. At 1,024 dimensions all 5,000 documents fit (19.6 MiB).

use serde::Deserialize;
use serde_json::{json, Value};

use super::handlers::parse;
use super::limits;
use super::{blocking, recheck, still_current, Authorised, Host, ModError, Reply};
use crate::db;
use crate::doc_vectors::{self, DocVec};

/// The layout's version, in its first word.
pub const FORMAT_VERSION: u32 = 1;
/// The header's size in bytes (four `u32`s), which keeps the ids on an 8-byte boundary.
pub const HEADER_BYTES: usize = 16;
/// Ids are exact in an `f64` up to here.
const MAX_EXACT_ID: i64 = 1 << 53;

/// The most documents of `dimension` floats one answer holds. Zero when even one doesn't fit.
pub fn max_documents(dimension: usize) -> usize {
    let each = 8 + 4 * dimension;
    limits::MAX_BINARY_RESULT_BYTES.saturating_sub(HEADER_BYTES) / each
}

/// `docs` as the layout above. Every vector must be `dimension` long and every id exact.
pub fn encode(dimension: usize, docs: &[DocVec]) -> Result<Vec<u8>, ModError> {
    let count = docs.len();
    if count > max_documents(dimension) {
        return Err(ModError::internal(format!(
            "{count} vectors of {dimension} floats are past the binary cap"
        )));
    }
    let (count_word, dimension_word) = (
        u32::try_from(count).map_err(ModError::internal)?,
        u32::try_from(dimension).map_err(ModError::internal)?,
    );
    let mut out = Vec::with_capacity(HEADER_BYTES + count * (8 + 4 * dimension));
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&count_word.to_le_bytes());
    out.extend_from_slice(&dimension_word.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for doc in docs {
        if !(0..=MAX_EXACT_ID).contains(&doc.doc_id) {
            return Err(ModError::internal(format!(
                "document id {} isn't exact as a double",
                doc.doc_id
            )));
        }
        out.extend_from_slice(&(doc.doc_id as f64).to_le_bytes());
    }
    for doc in docs {
        if doc.vec.len() != dimension {
            return Err(ModError::internal(format!(
                "document {} has {} floats, not {dimension}",
                doc.doc_id,
                doc.vec.len()
            )));
        }
        for x in &doc.vec {
            out.extend_from_slice(&x.to_le_bytes());
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VectorsArgs {
    /// Only these documents (the unreadable and the vectorless are simply absent); every document
    /// when omitted.
    #[serde(default)]
    ids: Option<Vec<i64>>,
    /// The most documents to return: the node cap. Defaults to the Map's default.
    #[serde(default)]
    cap: Option<usize>,
}

/// `embeddings.documentVectors(ids?, cap?)` → the binary layout above.
pub(super) async fn document_vectors<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let VectorsArgs { ids, cap } = parse(args)?;
    if ids
        .as_ref()
        .is_some_and(|ids| ids.len() > limits::MAX_DOCS_PER_CALL)
    {
        return Err(ModError::bad_args(format!(
            "Name at most {} documents.",
            limits::MAX_DOCS_PER_CALL
        )));
    }
    let cap = cap.unwrap_or(doc_vectors::DEFAULT_NODE_CAP);
    if cap == 0 || cap > doc_vectors::NODE_CAP_MAX {
        return Err(ModError::bad_args(format!(
            "cap is 1 to {}.",
            doc_vectors::NODE_CAP_MAX
        )));
    }
    let bytes = {
        let host = host.clone();
        let auth = auth.clone();
        blocking(move || {
            #[cfg(test)]
            host.at(super::Point::BeforeData);
            let conn = host.store().conn().map_err(|_| ModError::locked())?;
            recheck(&host, &conn, &auth)?;
            let dimension = db::vec0_dim(&conn).map_err(ModError::internal)?;
            let cap = cap.min(max_documents(dimension));
            let docs = doc_vectors::capped_document_vectors(&conn, ids.as_deref(), cap)
                .map_err(ModError::internal)?;
            encode(dimension, &docs)
        })
        .await?
    };
    still_current(host, auth)?;
    Ok(Reply::Binary { bytes })
}

/// `embeddings.signature()` → `{count, dimension, hash}`: how many documents have vectors, the
/// vectors' width, and a hash over which documents and which of their leaves. It changes when a
/// document is added, removed, archived or re-indexed, even to the same number of chunks, and reads no
/// vector, so a mod asks it before it asks for the vectors.
pub(super) async fn signature<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    super::handlers::no_args(args)?;
    let (count, dimension, hash) = {
        let host = host.clone();
        let auth = auth.clone();
        blocking(move || {
            #[cfg(test)]
            host.at(super::Point::BeforeData);
            let conn = host.store().conn().map_err(|_| ModError::locked())?;
            recheck(&host, &conn, &auth)?;
            let dimension = db::vec0_dim(&conn).map_err(ModError::internal)?;
            let sigs = doc_vectors::doc_signatures(&conn).map_err(ModError::internal)?;
            Ok((sigs.len(), dimension, doc_vectors::doc_set_hash(&sigs)))
        })
        .await?
    };
    still_current(host, auth)?;
    Ok(Reply::value(
        json!({ "count": count, "dimension": dimension, "hash": hash }),
    ))
}

/// The layout read back the way a frame reads it: the header, then the ids as doubles and the
/// floats as singles, each from its own offset.
#[cfg(test)]
pub(super) fn decode(bytes: &[u8]) -> (u32, u32, Vec<f64>, Vec<f32>) {
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    assert_eq!(word(0), FORMAT_VERSION);
    assert_eq!(word(12), 0, "reserved");
    let (count, dimension) = (word(4), word(8));
    let ids_end = HEADER_BYTES + 8 * count as usize;
    assert_eq!(
        bytes.len(),
        ids_end + 4 * (count * dimension) as usize,
        "the length is the header's arithmetic"
    );
    let ids = bytes[HEADER_BYTES..ids_end]
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| f64::from_le_bytes(*c))
        .collect();
    let floats = bytes[ids_end..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    (count, dimension, ids, floats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(id: i64, vec: Vec<f32>) -> DocVec {
        DocVec {
            doc_id: id,
            vec,
            leaf_count: 1,
            importance_rank: 0,
        }
    }

    #[test]
    fn the_layout_round_trips_and_its_offsets_are_what_a_frame_views() {
        let docs = vec![
            doc(3, vec![0.5, -0.25, 1.0]),
            doc(1, vec![f32::MIN_POSITIVE, 0.0, -0.0]),
        ];
        let bytes = encode(3, &docs).unwrap();
        let (count, dimension, ids, floats) = decode(&bytes);
        assert_eq!((count, dimension), (2, 3));
        assert_eq!(ids, vec![3.0, 1.0], "in the order given, not sorted");
        assert_eq!(
            floats,
            docs.iter()
                .flat_map(|d| d.vec.clone())
                .collect::<Vec<f32>>()
        );
        assert_eq!(bytes.len(), 16 + 2 * 8 + 2 * 3 * 4);
        // The alignment the frame's typed-array views need.
        assert_eq!(HEADER_BYTES % 8, 0, "ids start on an 8-byte boundary");
        assert_eq!((HEADER_BYTES + 8 * 2) % 4, 0, "floats on a 4-byte one");
        // Pinned bytes: version 1, count 2, dimension 3, reserved 0, little-endian.
        assert_eq!(
            &bytes[..16],
            &[1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0]
        );
        // An empty answer is a bare header.
        assert_eq!(encode(384, &[]).unwrap().len(), HEADER_BYTES);
    }

    #[test]
    fn a_bad_vector_or_id_is_pms_failure_never_a_wrong_answer() {
        assert!(encode(3, &[doc(1, vec![0.0; 2])]).is_err());
        assert!(encode(2, &[doc(-1, vec![0.0; 2])]).is_err());
        assert!(encode(2, &[doc((1 << 53) + 1, vec![0.0; 2])]).is_err());
        assert!(encode(2, &[doc(1 << 53, vec![0.0; 2])]).is_ok());
    }

    #[test]
    fn the_cap_the_largest_pilot_needs_fits_and_nothing_past_the_limit_does() {
        // The spec's maximum: 5,000 documents of 1,024 dimensions, in under 20 MiB.
        assert!(max_documents(1024) >= limits::MAX_DOCS_PER_CALL);
        let size = HEADER_BYTES + 5000 * (8 + 4 * 1024);
        assert!(size <= limits::MAX_BINARY_RESULT_BYTES, "{size}");
        // And the default 1,000 x 384.
        assert!(max_documents(384) >= 1000);
        // The count that fits is exactly the one under the cap.
        for dimension in [1usize, 384, 1024, 4096] {
            let n = max_documents(dimension);
            assert!(HEADER_BYTES + n * (8 + 4 * dimension) <= limits::MAX_BINARY_RESULT_BYTES);
            assert!(HEADER_BYTES + (n + 1) * (8 + 4 * dimension) > limits::MAX_BINARY_RESULT_BYTES);
        }
        // Past it, encode refuses rather than building an answer the cap would refuse.
        assert_eq!(max_documents(2_000_000), 2);
        let three: Vec<DocVec> = (1..=3).map(|i| doc(i, Vec::new())).collect();
        assert!(encode(2_000_000, &three).is_err());
    }
}
