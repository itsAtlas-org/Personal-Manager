// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Pinboard's page: the one line that makes the real connection and starts it. What starts is
// frame/boot.tsx, which a test drives with stand-ins; this file has no logic to test.
import "./style.css";
import { connectHost } from "../../../src/kit/host";
import { boot } from "./frame/boot";

void boot({ doc: document, win: window, connect: connectHost });
