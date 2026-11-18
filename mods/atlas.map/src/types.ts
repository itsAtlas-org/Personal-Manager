// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The shapes the Map's engine passes around. A document is the Host API's narrow DTO (documents.list:
// no path, no account id), reshaped the way the Map reads it; nothing here is PM's own `Document`.

/** The name a document with no home project is filed under, as the core Map has always shown it. */
export const UNSORTED = "Unsorted";

/** One document as the Map uses it. `project` is its home project's name, or "" for none. */
export interface MapDoc {
  id: number;
  title: string;
  project: string;
  importance: string | null;
  chunkCount: number;
  reviewed: boolean;
  /** "vault" | "index_only" | "chat" | "photo" | "spreadsheet": whatever PM says, kept as text. */
  sourceType: string;
  tags: string[];
  /** ISO timestamp, as PM stored it. */
  ingestedAt: string;
}

/** A document's place in the meaning layout: both axes in [0,1]. */
export interface SemanticCoord {
  id: number;
  x: number;
  y: number;
}
