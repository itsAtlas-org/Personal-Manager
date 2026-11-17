// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Shared by the engine's tests only (no page imports it, so the bundle never holds it): a seeded
// document generator, and the change-detector digest of a layout. The generator and the digest are
// written the same way in src/lib/mapLayout.test.ts, which took its literals from the core Map's code;
// the tests here hold those literals, so the port and the old code are held to one oracle.

import type { MapDoc } from "./types";

/** mulberry32: a small seeded generator, so every run on every machine sees the same documents. */
export function seeded(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const PROJECTS = ["Atlas", "Beta", "Gamma", "Delta", "Epsilon", "Zeta", "Eta"];

/** `n` documents over `projects` projects (a blank project for every 11th), chunk counts 1..40. */
export function documents(n: number, projects: number): MapDoc[] {
  const rand = seeded(1234 + n);
  const docs: MapDoc[] = [];
  for (let i = 0; i < n; i++) {
    docs.push({
      id: i + 1,
      title: `Document ${i + 1}`,
      project: i % 11 === 10 ? "" : PROJECTS[Math.floor(rand() * projects)],
      chunkCount: 1 + Math.floor(rand() * 40),
      reviewed: true,
      sourceType: "vault",
      tags: [],
      importance: null,
      ingestedAt: "2026-01-01T00:00:00Z",
    });
  }
  return docs;
}

/** One document with the fields a test cares about. */
export function doc(id: number, project: string, chunkCount = 4, title = `T${id}`): MapDoc {
  return {
    id,
    title,
    project,
    chunkCount,
    reviewed: true,
    sourceType: "vault",
    tags: [],
    importance: null,
    ingestedAt: "2026-01-01T00:00:00Z",
  };
}

/** Two 32-bit FNV-1a hashes of the values rounded to 3 decimals: a change detector, not for security.
 *  Long enough that two different layouts never share one by accident. */
export function digest(values: number[]): string {
  let h1 = 0x811c9dc5;
  let h2 = 0x01000193;
  for (const v of values) {
    const s = `${v.toFixed(3)};`;
    for (let i = 0; i < s.length; i++) {
      h1 = Math.imul(h1 ^ s.charCodeAt(i), 0x01000193) >>> 0;
      h2 = Math.imul(h2 ^ s.charCodeAt(i), 0x85ebca6b) >>> 0;
    }
  }
  return `${h1.toString(16).padStart(8, "0")}${h2.toString(16).padStart(8, "0")}`;
}

export const flat = (positions: { x: number; y: number }[]) => positions.flatMap((p) => [p.x, p.y]);

/** A vector for each of `docs`, `dimension` wide: a seeded cloud with two clearly different spreads
 *  (so the top two principal axes are well separated, and the projection is the same on every run). */
export function vectorsFor(docs: MapDoc[], dimension: number, seed = 7): Map<number, Float32Array> {
  const rand = seeded(seed);
  const out = new Map<number, Float32Array>();
  for (const d of docs) {
    const v = new Float32Array(dimension);
    for (let j = 0; j < dimension; j++) {
      const scale = j === 0 ? 5 : j === 1 ? 2.5 : 0.3;
      v[j] = (rand() - 0.5) * scale;
    }
    out.set(d.id, v);
  }
  return out;
}
