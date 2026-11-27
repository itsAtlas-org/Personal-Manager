// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { contactsApiDisabled, PEOPLE_API_ENABLE_URL } from "./contactsApiDisabled";

describe("contactsApiDisabled", () => {
  it("is the library page for the People API", () => {
    expect(PEOPLE_API_ENABLE_URL).toBe(
      "https://console.cloud.google.com/apis/library/people.googleapis.com",
    );
  });

  // The words the backend writes (contacts_accounts::api_disabled_error), with Google's own link.
  it("reads the backend's error and takes the link it names", () => {
    const error =
      "Google's People API isn't turned on for your Google Cloud project, so Contacts can't reach Google yet. Turn it on at https://console.developers.google.com/apis/api/people.googleapis.com/overview?project=123456789, wait a minute, then try again.";
    expect(contactsApiDisabled(error)).toEqual({
      enableUrl:
        "https://console.developers.google.com/apis/api/people.googleapis.com/overview?project=123456789",
    });
  });

  it("falls back to the library page when no link came with it", () => {
    expect(
      contactsApiDisabled("Google's People API isn't turned on for your Google Cloud project."),
    ).toEqual({ enableUrl: PEOPLE_API_ENABLE_URL });
    expect(contactsApiDisabled('403 {"reason":"accessNotConfigured"}')).toEqual({
      enableUrl: PEOPLE_API_ENABLE_URL,
    });
    expect(contactsApiDisabled("People API has not been used in project 1 before")).toEqual({
      enableUrl: PEOPLE_API_ENABLE_URL,
    });
  });

  it("never links a URL that is not on a Google console host", () => {
    for (const link of [
      "https://evil.example.com/apis/api/people.googleapis.com",
      "http://console.cloud.google.com/apis",
      "https://console.cloud.google.com.evil.example.com/x",
    ]) {
      expect(
        contactsApiDisabled(
          `Google's People API isn't turned on. Turn it on at ${link}, then retry.`,
        ),
      ).toEqual({ enableUrl: PEOPLE_API_ENABLE_URL });
    }
  });

  it("says nothing for any other error", () => {
    expect(contactsApiDisabled(null)).toBeNull();
    expect(contactsApiDisabled(undefined)).toBeNull();
    expect(contactsApiDisabled("")).toBeNull();
    expect(
      contactsApiDisabled("Google is busy right now. Wait a minute and try again."),
    ).toBeNull();
  });
});
