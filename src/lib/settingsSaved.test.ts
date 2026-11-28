// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { isSettingWrite } from "./settingsSaved";

// The tick's whole credibility rests on this predicate. The failure that matters is a FALSE
// NEGATIVE — a real settings write that stays silent teaches the user the indicator can't be
// trusted — which is why the rule is "every set_* except a named few" rather than an allow-list
// someone has to remember to extend.
describe("isSettingWrite", () => {
  it("announces the settings commands the tabs actually call", () => {
    for (const cmd of [
      "set_openrouter_key",
      "set_time_zone",
      "set_chat_models",
      "set_indexing_speed",
      "set_reranking",
      "set_app_lock",
      "set_tray_enabled",
      "set_backup_schedule",
      "set_backup_destinations",
      "set_calendar_selected",
      "set_calendar_quiet",
      "set_drive_scope",
      "set_local_llm_endpoint",
      "set_help_mode",
      "set_pref",
      // Your own provider keys: the opt-in, the role's provider, the tick-box.
      "set_direct_opt_in",
      "set_role_cloud_provider",
      "set_direct_data_sharing",
    ]) {
      expect(isSettingWrite(cmd)).toBe(true);
    }
  });

  /** The key commands make a network call and can be refused. A refused key must never claim
   *  "Saved", so none of them is named `set_*`. */
  it("stays silent for the key commands, which can be refused", () => {
    for (const cmd of [
      "save_direct_key",
      "recheck_direct_key",
      "clear_direct_key",
      "direct_providers_state",
      // The list editor's commands: a refused edit (a provider that is off, a duplicate, the last
      // row) must never claim "Saved", so none of them is named `set_*`.
      "route_status",
      "route_add",
      "route_move",
      "route_remove",
      "route_set_effort",
      "route_reset",
      "route_try_again",
      "promote_entry",
    ]) {
      expect(isSettingWrite(cmd)).toBe(false);
    }
  });

  it("stays silent for the set_* commands that write user content, not preferences", () => {
    for (const cmd of [
      "set_document_metadata",
      "set_project_metadata",
      "set_conversation_project",
      "set_milestone_event",
      "set_milestone_state",
      "set_milestone_status",
    ]) {
      expect(isSettingWrite(cmd)).toBe(false);
    }
  });

  it("ignores reads and every other command shape", () => {
    for (const cmd of [
      "get_settings",
      "list_projects",
      "sync_drive",
      "ingest_paths",
      "delete_milestone",
      "vault_status",
      "reset_settings", // not a set_ prefix; would need adding deliberately if it ever mattered
    ]) {
      expect(isSettingWrite(cmd)).toBe(false);
    }
  });

  it("a NEW settings command announces with no change here — that is the point", () => {
    expect(isSettingWrite("set_something_invented_next_quarter")).toBe(true);
  });
});
