// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The npm audit gate's decision (scripts/npm-audit-rules.mjs; scripts/npm-audit.mjs is the command). Each rule is shown a report that breaks it:
// the gate fails in the passing direction silently if a rule drifts, so every way out is tested shut.

import { describe, expect, it } from "vitest";
import {
  ACCEPTED,
  advisories,
  decide,
  entryProblem,
  ghsaOf,
  readReport,
} from "./npm-audit-rules.mjs";

const ID = "GHSA-fc5q-39xc-vjpf";
const OTHER = "GHSA-2222-3333-4444";

/** A report with one advisory against `pkg`, plus a package that only depends on it. */
function report(entries) {
  const vulnerabilities = {};
  for (const { id, pkg, severity = "high", title = "a bug" } of entries) {
    vulnerabilities[pkg] = {
      name: pkg,
      severity,
      via: [{ source: 1, name: pkg, title, url: `https://github.com/advisories/${id}`, severity }],
    };
    vulnerabilities[`uses-${pkg}`] = { name: `uses-${pkg}`, severity, via: [pkg] };
  }
  return { auditReportVersion: 2, vulnerabilities, metadata: {} };
}
const EMPTY = { auditReportVersion: 2, vulnerabilities: {}, metadata: {} };
const entry = (over = {}) => ({
  id: ID,
  package: "picomatch",
  until: "2026-12-31",
  decided: "Bobby, in a test",
  why: "Dev-only tooling with no patched release yet, in a test.",
  ...over,
});

describe("npm-audit gate", () => {
  it("passes a clean report", () => {
    expect(decide(EMPTY, EMPTY, [], "2026-11-17")).toEqual({
      failures: [],
      accepted: [],
      stale: [],
    });
  });

  it("fails an advisory at moderate or above that nobody accepted", () => {
    for (const severity of ["moderate", "high", "critical"]) {
      const r = decide(
        report([{ id: OTHER, pkg: "lodash", severity }]),
        EMPTY,
        [entry()],
        "2026-11-17",
      );
      expect(r.failures).toHaveLength(1);
      expect(r.failures[0]).toContain(OTHER);
    }
  });

  it("ignores advisories below moderate, as --audit-level=moderate did", () => {
    for (const severity of ["info", "low"]) {
      expect(
        decide(report([{ id: OTHER, pkg: "x", severity }]), EMPTY, [], "2026-11-17").failures,
      ).toEqual([]);
    }
  });

  it("an unknown severity counts as critical, never as nothing", () => {
    expect(
      decide(report([{ id: OTHER, pkg: "x", severity: "dire" }]), EMPTY, [], "2026-11-17").failures,
    ).toHaveLength(1);
  });

  it("lets an accepted advisory through while it stays out of the shipped tree and in date", () => {
    const r = decide(report([{ id: ID, pkg: "picomatch" }]), EMPTY, [entry()], "2026-12-31");
    expect(r.failures).toEqual([]);
    expect(r.accepted).toHaveLength(1);
    expect(r.accepted[0]).toContain(ID);
  });

  it("fails an accepted advisory once it reaches the shipped tree", () => {
    const both = report([{ id: ID, pkg: "picomatch" }]);
    const r = decide(both, both, [entry()], "2026-11-17");
    expect(r.failures).toHaveLength(1);
    expect(r.failures[0]).toContain("shipped app");
  });

  it("fails an accepted advisory after its end date", () => {
    const r = decide(report([{ id: ID, pkg: "picomatch" }]), EMPTY, [entry()], "2027-01-01");
    expect(r.failures).toHaveLength(1);
    expect(r.failures[0]).toContain("ended on 2026-12-31");
  });

  it("an acceptance covers only its own advisory and package", () => {
    const r = decide(
      report([
        { id: ID, pkg: "other-glob" },
        { id: OTHER, pkg: "picomatch" },
      ]),
      EMPTY,
      [entry()],
      "2026-11-17",
    );
    expect(r.failures).toHaveLength(2);
    expect(r.failures.join("\n")).toContain("accepted for picomatch, not other-glob");
    expect(r.failures.join("\n")).toContain(OTHER);
  });

  it("accepting an advisory for one package never hides it on another, in either order", () => {
    const shared = (order) => ({
      vulnerabilities: Object.fromEntries(
        order.map((pkg) => [
          pkg,
          {
            severity: "high",
            via: [
              {
                source: 1,
                name: pkg,
                title: "t",
                url: `https://github.com/advisories/${ID}`,
                severity: "high",
              },
            ],
          },
        ]),
      ),
    });
    for (const order of [
      ["evilpkg", "picomatch"],
      ["picomatch", "evilpkg"],
    ]) {
      const r = decide(shared(order), EMPTY, [entry()], "2026-11-17");
      expect(r.failures).toHaveLength(1);
      expect(r.failures[0]).toContain("evilpkg");
      expect(r.accepted).toHaveLength(1);
    }
  });

  it("a package listed only because it depends on a vulnerable one adds no advisory", () => {
    const found = advisories(report([{ id: ID, pkg: "picomatch" }]));
    expect([...found.keys()]).toEqual([`${ID}|picomatch`]);
    expect(found.get(`${ID}|picomatch`).package).toBe("picomatch");
  });

  it("an advisory with no GHSA link still counts, under the npm source id", () => {
    const r = {
      vulnerabilities: {
        x: { severity: "high", via: [{ source: 99, name: "x", title: "t", severity: "high" }] },
      },
    };
    expect(decide(r, EMPTY, [entry()], "2026-11-17").failures[0]).toContain("npm-advisory-99");
  });

  it("reports an acceptance that matches nothing as stale, without failing", () => {
    const r = decide(EMPTY, EMPTY, [entry()], "2026-11-17");
    expect(r.failures).toEqual([]);
    expect(r.stale).toHaveLength(1);
    expect(r.stale[0]).toContain(ID);
  });

  it("refuses a malformed or doubled acceptance, and it accepts nothing", () => {
    const bad = [
      entry({ id: "CVE-2026-1" }),
      entry({ package: " " }),
      entry({ until: "31-12-2026" }),
      entry({ until: "2026-13-45" }),
      entry({ why: "" }),
      entry({ decided: "" }),
    ];
    for (const e of bad) {
      expect(entryProblem(e)).not.toBeNull();
      const r = decide(report([{ id: ID, pkg: "picomatch" }]), EMPTY, [e], "2026-11-17");
      expect(r.accepted).toEqual([]);
      expect(r.failures.length).toBeGreaterThanOrEqual(2);
    }
    expect(decide(EMPTY, EMPTY, [entry(), entry()], "2026-11-17").failures.join()).toContain(
      "twice",
    );
  });

  it("an advisory in the shipped tree's report alone still fails", () => {
    const r = decide(EMPTY, report([{ id: OTHER, pkg: "x" }]), [], "2026-11-17");
    expect(r.failures).toHaveLength(1);
  });

  it("never reads an npm failure as a clean report", () => {
    expect(() => readReport("", "t")).toThrow(/no JSON/);
    expect(() => readReport("[]", "t")).toThrow(/not a report/);
    expect(() =>
      readReport(JSON.stringify({ error: { code: "ENOTFOUND", summary: "no network" } }), "t"),
    ).toThrow(/no network/);
    expect(() => readReport(JSON.stringify({ auditReportVersion: 2 }), "t")).toThrow(
      /no vulnerabilities/,
    );
    expect(readReport(JSON.stringify(EMPTY), "t")).toEqual(EMPTY);
  });

  it("reads the GHSA id out of an advisory URL", () => {
    expect(ghsaOf(`https://github.com/advisories/${ID}`)).toBe(ID);
    expect(ghsaOf("https://example.com/x")).toBeNull();
  });

  it("every entry in ACCEPTED is well formed and dated", () => {
    for (const e of ACCEPTED) expect(entryProblem(e)).toBeNull();
    expect(new Set(ACCEPTED.map((e) => e.id)).size).toBe(ACCEPTED.length);
  });
});
