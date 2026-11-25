// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The toolchain test's React + Tailwind page (scripts/mods-build.test.mjs): JSX, a Tailwind class used
// only in this folder, and a kit component whose own classes the build must also find.

import "./style.css";
import { createRoot } from "react-dom/client";
import { Button } from "../../../../src/kit/ui/Button";

function Hello({ name }: { name: string }) {
  return (
    <main className="p-[13px] text-[#c0ffee]">
      <h1>JSX-RENDERED-MARKER {name}</h1>
      <Button>kit button</Button>
    </main>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Hello name="world" />);
