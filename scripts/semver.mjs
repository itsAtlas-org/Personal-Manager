// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Semver order for the `X.Y.Z[-pre]` versions PM and its mods use. Moved out of
// check-version-lockstep.mjs (which runs at import) so check-mods.mjs compares a mod's version with
// the very same function that compares PM's: two copies of an ordering rule are how a gate comes to
// accept a version the other one refuses. Pure Node built-ins, no dependencies.

export function parseSemver(v) {
  const dash = v.indexOf("-");
  const core = dash === -1 ? v : v.slice(0, dash);
  const pre = dash === -1 ? "" : v.slice(dash + 1);
  const [maj, min, pat] = core.split(".").map(Number);
  if ([maj, min, pat].some((n) => !Number.isInteger(n))) {
    throw new Error(`Not a valid X.Y.Z version: "${v}"`);
  }
  return { maj, min, pat, pre };
}

/** -1 if a<b, 0 if equal, 1 if a>b. A release outranks its own pre-release. */
export function semverCmp(a, b) {
  const A = parseSemver(a);
  const B = parseSemver(b);
  for (const k of ["maj", "min", "pat"]) {
    if (A[k] !== B[k]) return A[k] < B[k] ? -1 : 1;
  }
  if (A.pre === B.pre) return 0;
  if (A.pre === "") return 1;
  if (B.pre === "") return -1;
  const ai = A.pre.split(".");
  const bi = B.pre.split(".");
  for (let i = 0; i < Math.max(ai.length, bi.length); i++) {
    if (ai[i] === undefined) return -1;
    if (bi[i] === undefined) return 1;
    const an = /^\d+$/.test(ai[i]);
    const bn = /^\d+$/.test(bi[i]);
    if (an && bn) {
      const d = Number(ai[i]) - Number(bi[i]);
      if (d !== 0) return d < 0 ? -1 : 1;
    } else if (an !== bn) {
      return an ? -1 : 1; // numeric identifiers rank below alphanumeric
    } else if (ai[i] !== bi[i]) {
      return ai[i] < bi[i] ? -1 : 1;
    }
  }
  return 0;
}
