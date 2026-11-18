// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! One embedding per document: each document's leaf-chunk vectors decoded, averaged and
//! L2-normalised, the node cap that keeps the most important and most substantial documents, and the
//! cheap signature that says whether any of it changed. The mod API's `embeddings.*` calls
//! (`mods/api/embeddings.rs`) read it, so a mod sees the same documents as nodes, the same averaged
//! vectors and the same cap, however many mods ask.
//!
//! **Which documents.** Archived documents (any source, chat included) are left out of everything
//! here (`d.importance IS NOT 'archive'` also admits the untriaged NULL): the Map hides what the user
//! shelved, which stays keyword-findable. Only leaf chunks are embedded (`chunk_vec.rowid ==
//! chunks.id`), so a document with no vectors is no node.
//!
//! **The node cap is applied before the vectors are read.** The cheap [`leaf_stats`] pass (counts and
//! importance, no blobs) picks the documents the cap keeps ([`apply_cap`]), and only their vectors are
//! read. Averaging every document first, as the core Map once did, holds one `f64` row of the embedding
//! dimension per document in memory (400 MB at 50,000 documents of 1,024 dimensions); a mod can ask
//! for this whenever it likes, so the read is sized by the cap, not by the library.

use std::collections::BTreeMap;

use rusqlite::{params, types::ValueRef, Connection};
use sha2::{Digest, Sha256};

use crate::db;
use crate::error::{Error, Result};

/// The cap on individually-projected nodes when nobody chose one.
pub const DEFAULT_NODE_CAP: usize = 1000;
/// The smallest and largest node cap a user (or a mod) may choose. 5,000 documents of a 1,024-d
/// embedder is 20 MB of vectors, which is what the Host API sizes its binary answer for.
pub const NODE_CAP_MIN: usize = 200;
pub const NODE_CAP_MAX: usize = 5000;

/// One document's averaged embedding, plus the inputs that rank it when the node cap bites.
#[derive(Debug, Clone, PartialEq)]
pub struct DocVec {
    pub doc_id: i64,
    pub vec: Vec<f32>,
    pub leaf_count: usize,
    pub importance_rank: u8,
}

/// What ranks a document for the cap, from the cheap pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeafStats {
    pub doc_id: i64,
    pub leaf_count: usize,
    pub importance_rank: u8,
}

fn rank_importance(s: Option<&str>) -> u8 {
    match s {
        Some("high") => 3,
        Some("medium") => 2,
        Some("low") => 1,
        _ => 0,
    }
}

/// Decode one `chunk_vec` row into `dim` `f32`s. sqlite-vec returns the vector as a compact `f32`
/// blob; we also accept the JSON-text form it's written in, so this is robust either way.
fn decode_vector(value: ValueRef<'_>, dim: usize) -> Result<Vec<f32>> {
    match value {
        ValueRef::Blob(b) => {
            if b.len() != dim * 4 {
                return Err(Error::Other(format!(
                    "vector blob is {} bytes, expected {} for a {dim}-d vector",
                    b.len(),
                    dim * 4
                )));
            }
            Ok(b.as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect())
        }
        ValueRef::Text(t) => serde_json::from_slice::<Vec<f32>>(t)
            .map_err(|e| Error::Other(format!("decode vector json: {e}"))),
        _ => Err(Error::Other(
            "unexpected vector value type in chunk_vec".into(),
        )),
    }
}

fn l2_normalize(v: &mut [f32]) {
    let norm = v
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt();
    if norm > 1e-12 {
        for x in v.iter_mut() {
            *x = (*x as f64 / norm) as f32;
        }
    }
}

/// `ids` as the JSON array text the queries filter with (`json_each`), so one bound parameter
/// carries any number of ids (SQLite caps the count of `?` variables). `None` is no filter.
fn id_filter(ids: Option<&[i64]>) -> String {
    match ids {
        None => "null".to_owned(),
        Some(ids) => serde_json::to_string(ids).unwrap_or_else(|_| "[]".to_owned()),
    }
}

/// The cheap pass: every embedded document's leaf count and importance rank, ascending by id, with no
/// vector blob read. `ids` (when given) restricts it to those documents.
pub fn leaf_stats(conn: &Connection, ids: Option<&[i64]>) -> Result<Vec<LeafStats>> {
    let mut stmt = conn.prepare(
        "SELECT c.document_id, count(*), d.importance \
         FROM chunk_vec cv JOIN chunks c ON cv.rowid = c.id \
         JOIN documents d ON d.id = c.document_id \
         WHERE c.kind = 'leaf' AND d.importance IS NOT 'archive' \
           AND (?1 IS NULL OR c.document_id IN (SELECT value FROM json_each(?1))) \
         GROUP BY c.document_id ORDER BY c.document_id",
    )?;
    let filter = ids.map(|_| id_filter(ids));
    let rows = stmt.query_map(params![filter], |r| {
        let importance: Option<String> = r.get(2)?;
        Ok(LeafStats {
            doc_id: r.get(0)?,
            leaf_count: r.get::<_, i64>(1)?.max(0) as usize,
            importance_rank: rank_importance(importance.as_deref()),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The documents `cap` keeps, in the order they are kept. Under the cap every document stays, by id.
/// Over it, the most important and then the most substantial: importance rank, then leaf count, both
/// descending, then id. (The rest fall to their project centroid on the Map's side: still shown,
/// without paying the reducer for the tail.)
pub fn apply_cap(mut stats: Vec<LeafStats>, cap: usize) -> Vec<LeafStats> {
    if stats.len() > cap {
        stats.sort_by(|a, b| {
            b.importance_rank
                .cmp(&a.importance_rank)
                .then(b.leaf_count.cmp(&a.leaf_count))
                .then(a.doc_id.cmp(&b.doc_id))
        });
        stats.truncate(cap);
    }
    stats
}

/// The averaged, normalised vector of each of `chosen`, in `chosen`'s order. The dimension is read
/// from the registry (no magic number). A document in `chosen` that has no vector any more (archived
/// or re-indexed since the stats were read) is left out.
fn read_vectors(conn: &Connection, chosen: &[LeafStats]) -> Result<Vec<DocVec>> {
    if chosen.is_empty() {
        return Ok(Vec::new());
    }
    let dim = db::vec0_dim(conn)?;
    let ids: Vec<i64> = chosen.iter().map(|s| s.doc_id).collect();
    let mut acc: BTreeMap<i64, (Vec<f64>, usize)> = BTreeMap::new();
    let mut stmt = conn.prepare(
        "SELECT c.document_id, cv.embedding \
         FROM chunk_vec cv JOIN chunks c ON cv.rowid = c.id \
         JOIN documents d ON d.id = c.document_id \
         WHERE c.kind = 'leaf' AND d.importance IS NOT 'archive' \
           AND c.document_id IN (SELECT value FROM json_each(?1))",
    )?;
    let mut rows = stmt.query(params![id_filter(Some(&ids))])?;
    while let Some(row) = rows.next()? {
        let doc_id: i64 = row.get(0)?;
        let v = decode_vector(row.get_ref(1)?, dim)?;
        let entry = acc.entry(doc_id).or_insert_with(|| (vec![0.0f64; dim], 0));
        for (s, x) in entry.0.iter_mut().zip(v.iter()) {
            *s += *x as f64;
        }
        entry.1 += 1;
    }
    let mut out = Vec::with_capacity(acc.len());
    for stats in chosen {
        let Some((sum, n)) = acc.remove(&stats.doc_id) else {
            continue;
        };
        if n == 0 {
            continue;
        }
        let mut mean: Vec<f32> = sum.iter().map(|s| (*s / n as f64) as f32).collect();
        l2_normalize(&mut mean);
        out.push(DocVec {
            doc_id: stats.doc_id,
            vec: mean,
            leaf_count: n,
            importance_rank: stats.importance_rank,
        });
    }
    Ok(out)
}

/// The vectors of the documents `cap` keeps (of `ids`, or of every embedded document), in the order
/// [`apply_cap`] gives. This is what `embeddings.documentVectors` reads.
pub fn capped_document_vectors(
    conn: &Connection,
    ids: Option<&[i64]>,
    cap: usize,
) -> Result<Vec<DocVec>> {
    let kept = apply_cap(leaf_stats(conn, ids)?, cap);
    read_vectors(conn, &kept)
}

/// Cheap signature of the document set (ids + leaf-chunk counts + a leaf-identity marker), without
/// reading any vector blobs, so the freshness check on a warm cache stays fast. Archived documents
/// (any source, chat included) are excluded exactly as in [`capped_document_vectors`], so
/// archiving/un-archiving shifts the signature and invalidates the cache: the Map recomputes
/// without the archived node.
///
/// The third column, `MAX(c.id)`, is the B4-1 fix: leaf `count` alone missed a *content* edit that
/// re-chunked a document to the same number of leaves (common for small edits, same 256-token
/// packing), so the Map kept placing that document by its old meaning forever. Re-ingest DELETEs then
/// re-INSERTs a document's chunk rows, and `chunks.id` is `AUTOINCREMENT` (strictly monotonic, never
/// reused), so the max leaf rowid always rises on re-ingest, flipping the hash even at constant leaf
/// count. `MAX` (not `SUM`) also can't overflow the `i64` we read it back as.
pub fn doc_signatures(conn: &Connection) -> Result<Vec<(i64, i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT c.document_id, count(*), MAX(c.id) \
         FROM chunk_vec cv JOIN chunks c ON cv.rowid = c.id \
         JOIN documents d ON d.id = c.document_id \
         WHERE c.kind = 'leaf' AND d.importance IS NOT 'archive' \
         GROUP BY c.document_id ORDER BY c.document_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn doc_set_hash(sigs: &[(i64, i64, i64)]) -> String {
    let mut h = Sha256::new();
    for (id, count, max_leaf_id) in sigs {
        h.update(id.to_le_bytes());
        h.update(count.to_le_bytes());
        h.update(max_leaf_id.to_le_bytes());
    }
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn store() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open(&dir.path().join("pm.sqlite"), KEY).unwrap();
        (dir, conn)
    }

    /// A document with one leaf chunk per value in `leaves`, each a constant vector of that value.
    fn doc(conn: &Connection, name: &str, importance: Option<&str>, leaves: &[f64]) -> i64 {
        let dim = db::vec0_dim(conn).unwrap();
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash, project, importance) \
             VALUES (?1, 'T', ?1, 'PM', ?2)",
            params![name, importance],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        for (ordinal, v) in leaves.iter().enumerate() {
            conn.execute(
                "INSERT INTO chunks(document_id, ordinal, content, char_count, kind) \
                 VALUES (?1, ?2, 'c', 1, 'leaf')",
                params![id, ordinal as i64],
            )
            .unwrap();
            let chunk = conn.last_insert_rowid();
            let json = format!("[{}]", vec![v.to_string(); dim].join(","));
            conn.execute(
                "INSERT INTO chunk_vec(rowid, embedding) VALUES (?1, ?2)",
                params![chunk, json],
            )
            .unwrap();
        }
        id
    }

    #[test]
    fn the_cap_keeps_importance_then_substance_then_the_lower_id() {
        let stats = |doc_id, leaf_count, importance_rank| LeafStats {
            doc_id,
            leaf_count,
            importance_rank,
        };
        let all = vec![
            stats(1, 9, 0),
            stats(2, 1, 3),
            stats(3, 5, 3),
            stats(4, 5, 3),
            stats(5, 9, 1),
        ];
        // Over the cap: high importance first, the more substantial of two, the lower id on a tie.
        let kept: Vec<i64> = apply_cap(all.clone(), 3).iter().map(|s| s.doc_id).collect();
        assert_eq!(kept, vec![3, 4, 2]);
        // At or under it nothing is dropped or reordered: ascending by id, as the cheap pass gave it.
        for cap in [5, 6, usize::MAX] {
            let kept: Vec<i64> = apply_cap(all.clone(), cap)
                .iter()
                .map(|s| s.doc_id)
                .collect();
            assert_eq!(kept, vec![1, 2, 3, 4, 5], "cap {cap}");
        }
        assert!(apply_cap(all, 0).is_empty());
    }

    #[test]
    fn the_capped_read_is_the_uncapped_read_cut_down_and_reads_only_what_it_keeps() {
        let (_dir, conn) = store();
        let a = doc(&conn, "a.md", Some("low"), &[0.1]);
        let b = doc(&conn, "b.md", Some("high"), &[0.2, 0.4]);
        let c = doc(&conn, "c.md", None, &[0.3, 0.3, 0.3]);
        let d = doc(&conn, "d.md", Some("high"), &[0.5]);
        doc(&conn, "gone.md", Some("archive"), &[0.9]);
        doc(&conn, "empty.md", Some("high"), &[]);

        let every = capped_document_vectors(&conn, None, usize::MAX).unwrap();
        assert_eq!(
            every.iter().map(|d| d.doc_id).collect::<Vec<_>>(),
            vec![a, b, c, d]
        );
        // The same answer through the capped path, document by document (the average is the same).
        let same = capped_document_vectors(&conn, None, 10).unwrap();
        assert_eq!(same, every);
        // Cut to two: the high-importance documents, the more substantial first.
        let two = capped_document_vectors(&conn, None, 2).unwrap();
        assert_eq!(two.iter().map(|d| d.doc_id).collect::<Vec<_>>(), vec![b, d]);
        for kept in &two {
            assert_eq!(
                kept,
                every.iter().find(|e| e.doc_id == kept.doc_id).unwrap()
            );
        }
        // Named ids restrict the set before the cap; a missing, archived or vectorless id is simply
        // absent.
        let named = capped_document_vectors(&conn, Some(&[c, a, 9999, d + 1, d + 2]), 10).unwrap();
        assert_eq!(
            named.iter().map(|d| d.doc_id).collect::<Vec<_>>(),
            vec![a, c]
        );
        assert!(capped_document_vectors(&conn, Some(&[]), 10)
            .unwrap()
            .is_empty());
        assert!(capped_document_vectors(&conn, None, 0).unwrap().is_empty());
    }

    #[test]
    fn a_vector_is_the_normalised_mean_of_its_leaves() {
        let (_dir, conn) = store();
        let id = doc(&conn, "a.md", Some("high"), &[0.0, 0.2]);
        let dim = db::vec0_dim(&conn).unwrap();
        let got = capped_document_vectors(&conn, None, 10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].doc_id, got[0].leaf_count), (id, 2));
        assert_eq!(got[0].importance_rank, 3);
        let expected = 1.0f32 / (dim as f32).sqrt();
        assert!(got[0].vec.iter().all(|x| (x - expected).abs() < 1e-5));
    }
}
