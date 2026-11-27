// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Google Contacts, the pure core (Contacts PR B, spec 7.3 to 7.5): no DB, no network and no clock
//! in any of it, so every rule is a table test.
//!
//! - [`map`]: Google's `Person` JSON to PM's [`Card`](crate::people::card::Card) and back, and the
//!   one `READ_MASK` / `SOURCES` pair every call uses.
//! - [`merge`]: the three-way merge engine (base, PM, the other side's new copy), with conflicts
//!   that freeze their field. It knows nothing of Google, so a file merge can reuse it.
//! - [`plan`]: the requests, built and not sent: pinned list parameters, `updateContact` that
//!   edits Google's own fresh arrays in place, contact groups.
//! - [`classify`]: what an answer means, and whether to retry (once, and never a create).
//!
//! Not model-path code: nothing on a model path may name it (E13; the contacts write fence).
//! Nothing here derives `Debug` for a type holding contact values, and nothing puts a value, a
//! token or a response body into an error.

pub mod classify;
pub mod map;
pub mod merge;
pub mod plan;
