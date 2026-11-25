// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The type snapshot for "made by" (PR C, K5): `Task` and `Milestone` gain `madeBy`, the one field the
// backend names in camelCase beside its snake_case siblings, and `lib/types.ts` follows the Rust structs.
// A change to either side that the other does not follow fails here.

import { describe, expect, expectTypeOf, it } from "vitest";
import madeByRs from "../../src-tauri/src/made_by.rs?raw";
import milestonesRs from "../../src-tauri/src/milestones.rs?raw";
import tasksRs from "../../src-tauri/src/tasks.rs?raw";
import typesTs from "./types.ts?raw";
import type { MadeBy, Milestone, Task } from "./types";

/** The text of `pub struct <name> { ... }` in a Rust source. */
function structBody(source: string, name: string): string {
  const start = source.indexOf(`pub struct ${name} `);
  expect(start).toBeGreaterThan(-1);
  return source.slice(start, source.indexOf("\n}\n", start));
}

describe("madeBy, as the backend sends it", () => {
  it("is one nullable field on a task and on a milestone, of one shape", () => {
    expectTypeOf<Task["madeBy"]>().toEqualTypeOf<MadeBy | null>();
    expectTypeOf<Milestone["madeBy"]>().toEqualTypeOf<MadeBy | null>();
    expectTypeOf<MadeBy>().toEqualTypeOf<{
      name: string;
      tier: "first" | "second" | "third";
      signed: boolean;
    }>();
  });

  it("is named madeBy on the wire by both Rust structs, and is Option there", () => {
    for (const [source, name] of [
      [tasksRs, "Task"],
      [milestonesRs, "Milestone"],
    ] as const) {
      const body = structBody(source, name);
      expect(body).toMatch(
        /#\[serde\(rename = "madeBy"\)\]\s+pub made_by: Option<crate::made_by::MadeBy>,/,
      );
      expect(typesTs).toMatch(/madeBy: MadeBy \| null;/);
    }
  });

  it("carries exactly name, tier and signed", () => {
    const body = structBody(madeByRs, "MadeBy");
    const fields = [...body.matchAll(/^\s+pub (\w+):/gm)].map((m) => m[1]);
    expect(fields).toEqual(["name", "tier", "signed"]);
  });
});
