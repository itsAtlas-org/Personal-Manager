// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Not under src/: a Tailwind class here is outside both folders the mod build may scan, so it must not
// reach the stylesheet (scripts/mods-build.test.mjs).
export const Decoy = () => <p className="p-[17px] text-[#decade]">decoy</p>;
