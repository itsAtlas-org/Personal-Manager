// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { guideFor, type SetupPlatform } from "./setupGuide";

// The guide copy is per-platform (the OS detection itself is a UA regex exercised in
// the app); these pin the routing so a copy edit can't silently hand Linux users the
// Windows python.org instructions again.

const PLATFORMS: SetupPlatform[] = ["windows", "mac", "linux"];

describe("guideFor", () => {
  it("gives every platform a non-empty guide for every mode", () => {
    const modes = [
      "install",
      "python_missing",
      "python_too_old",
      "python_download_failed",
      "pip_failed",
      "requirements_missing",
      "packaging_bug",
      "unknown",
    ] as const;
    for (const platform of PLATFORMS) {
      for (const mode of modes) {
        const g = guideFor(mode, platform);
        expect(g.title.length, `${platform}/${mode} title`).toBeGreaterThan(0);
        expect(g.steps.length, `${platform}/${mode} steps`).toBeGreaterThan(0);
      }
    }
  });

  it("routes python_missing to the platform's own package story", () => {
    expect(guideFor("python_missing", "linux").steps[0]).toContain("dnf");
    expect(guideFor("python_missing", "windows").steps[0]).toContain("python.org");
    expect(guideFor("python_missing", "mac").steps[0]).toContain("brew");
  });

  it("asks for the Python 3.12 floor everywhere and never points at a Python PM refuses", () => {
    for (const platform of ["linux", "mac", "windows"] as const) {
      for (const mode of ["python_missing", "python_too_old"] as const) {
        const text = JSON.stringify(guideFor(mode, platform));
        expect(text, `${platform}/${mode}`).toContain("Python 3.12");
        // 3.11 was the floor until the 3.12 raise, and Ubuntu 22.04's python3.11 is refused.
        expect(text, `${platform}/${mode}`).not.toMatch(/3\.11/);
      }
    }
    // The Fedora package is the versioned one: its plain python3 can be newer than PM supports.
    expect(guideFor("python_missing", "linux").steps[0]).toContain("dnf install python3.12");
  });

  it("never tells a Linux or Mac user about python.exe", () => {
    for (const platform of ["linux", "mac"] as const) {
      const g = guideFor("python_missing", platform);
      expect(JSON.stringify(g)).not.toContain("python.exe");
    }
  });

  it("describes the self-contained bundled setup on windows and linux alike", () => {
    expect(guideFor("install", "linux")).toEqual(guideFor("install", "windows"));
  });
});
