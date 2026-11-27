// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What Google's answer to a People API request means (spec 7.5, "Classifier"). Pure: the method,
//! the status code and the error body in, a verdict out, so every case is tested against the error
//! shapes Google documents for the People API (developers.google.com/people/api/rest, the
//! updateContact page for the stale etag, the list page for `EXPIRED_SYNC_TOKEN`).
//!
//! **Not the Calendar writer's classifier.** Calendar answers a stale etag with 412 `conditionNotMet`;
//! the People API answers it with **400** and the reason `failedPrecondition`, so a copy of the
//! calendar's table would read a stale etag as a bad request and never re-read the contact.
//!
//! **No verdict carries Google's text.** A 400 can echo contact fields (spec 7.5, "Its own
//! sender"), so the body is only ever read here, for its machine-readable `status` and `reason`
//! words, and none of it is returned.
//!
//! Retrying is decided here too and kept to one retry. A rate limit is refused before Google applies
//! anything, so it is retried once for any request. A server error may have been applied, so it is
//! retried only where a second copy can't apply twice: a read, or an update guarded by the contact's
//! etag (an applied first copy makes the etag stale). A create is never retried: the caller looks
//! for the contact by its `pm.uid` instead (spec 7.5, Create).

use serde_json::Value;

/// The request a verdict is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// `people.get`, `people:batchGet`, `people/me`, `contactGroups.get`.
    Get,
    /// `people.connections.list` (and `contactGroups.list`): a read whose sync token can expire.
    List,
    /// `createContact`, `contactGroups.create`.
    Create,
    /// `updateContact`, `contactGroups.update`: always sent with the etag read a moment before.
    Update,
    /// `deleteContact`, `contactGroups.delete`.
    Delete,
}

/// What Google's answer means for PM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 2xx.
    Ok,
    /// 400 `failedPrecondition` on an update: the etag sent is no longer current. Read the contact
    /// again, once, and redo the merge.
    Conflict,
    /// 404: the contact (or group) is not there.
    Gone,
    /// 429, or 403 `RESOURCE_EXHAUSTED` / a rate-limit reason. Refused before anything applied.
    RateLimited,
    /// 403 for a token that lacks the scope (`ACCESS_TOKEN_SCOPE_INSUFFICIENT`).
    InsufficientScope,
    /// 401: the access token was refused even after the refresh the sender already tried.
    Reauth,
    /// 403 `SERVICE_DISABLED` / `accessNotConfigured`: the People API is not switched on in the
    /// user's Google project. The one error whose body (Google's enable link) may be shown.
    ApiDisabled,
    /// A list call's sync token has expired or is invalid: do a full sync. Matched as HTTP 410 and
    /// as the `EXPIRED_SYNC_TOKEN` `ErrorInfo` reason, which Google documents as the same event.
    ExpiredSyncToken,
    /// 400 that is not a stale etag: a request Google won't take.
    BadRequest,
    /// 5xx.
    ServerError,
    /// Any other status (a 409 for a duplicate group name, a 403 for no access).
    Other(u16),
}

/// The machine words of a Google error body: the HTTP-style `status` ("FAILED_PRECONDITION"), and
/// every `reason` found (`error.errors[].reason`, the older shape, and `error.details[].reason`,
/// the `ErrorInfo` shape). Nothing else is read, so nothing else can leak.
struct Words {
    status: String,
    reasons: Vec<String>,
}

impl Words {
    fn of(body: &str) -> Words {
        let parsed: Option<Value> = serde_json::from_str(body).ok();
        let error = parsed.as_ref().and_then(|v| v.get("error"));
        let status = error
            .and_then(|e| e.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mut reasons = Vec::new();
        for list in ["errors", "details"] {
            if let Some(items) = error.and_then(|e| e.get(list)).and_then(Value::as_array) {
                for item in items {
                    if let Some(r) = item.get("reason").and_then(Value::as_str) {
                        reasons.push(r.to_string());
                    }
                }
            }
        }
        Words { status, reasons }
    }

    fn has(&self, reason: &str) -> bool {
        self.reasons.iter().any(|r| r == reason)
    }

    fn is_status(&self, status: &str) -> bool {
        self.status == status
    }
}

/// Whether a list call's answer says its sync token is expired: HTTP 410, or the `ErrorInfo`
/// reason `EXPIRED_SYNC_TOKEN` at any status (Google documents it as a 400 `FAILED_PRECONDITION`
/// with that reason on the list page, and `authorized_get_dated` already maps 410 to `Gone`).
pub fn is_expired_sync_token(status: u16, body: &str) -> bool {
    status == 410 || (status >= 400 && Words::of(body).has("EXPIRED_SYNC_TOKEN"))
}

/// Read Google's verdict on a request from its method, status and body.
#[allow(dead_code)] // the sender (g3) classifies every answer; until then only the tests call it
pub fn classify(method: Method, status: u16, body: &str) -> Verdict {
    if (200..300).contains(&status) {
        return Verdict::Ok;
    }
    // The list call's expired token comes first: it is a 410 or a 400, and the 400 would
    // otherwise read as a bad request.
    if method == Method::List && is_expired_sync_token(status, body) {
        return Verdict::ExpiredSyncToken;
    }
    let words = Words::of(body);
    match status {
        400 => {
            if method == Method::Update
                && (words.has("failedPrecondition")
                    || words.has("FAILED_PRECONDITION")
                    || words.is_status("FAILED_PRECONDITION"))
            {
                Verdict::Conflict
            } else {
                Verdict::BadRequest
            }
        }
        401 => Verdict::Reauth,
        403 => {
            if words.has("SERVICE_DISABLED") || words.has("accessNotConfigured") {
                Verdict::ApiDisabled
            } else if words.has("ACCESS_TOKEN_SCOPE_INSUFFICIENT")
                || words.has("insufficientPermissions")
            {
                Verdict::InsufficientScope
            } else if words.is_status("RESOURCE_EXHAUSTED")
                || words.has("rateLimitExceeded")
                || words.has("userRateLimitExceeded")
                || words.has("RATE_LIMIT_EXCEEDED")
            {
                Verdict::RateLimited
            } else {
                Verdict::Other(403)
            }
        }
        404 => Verdict::Gone,
        // A 410 outside the list call: the thing is gone too.
        410 => Verdict::Gone,
        429 => Verdict::RateLimited,
        500..=599 => Verdict::ServerError,
        other => Verdict::Other(other),
    }
}

/// Whether to send `method` once more after `verdict`. Only ever once (`attempt` counts from 1).
#[allow(dead_code)] // the sender (g3) decides every retry here; until then only the tests call it
pub fn should_retry(method: Method, verdict: Verdict, attempt: u32) -> bool {
    if attempt > 1 {
        return false;
    }
    match verdict {
        // Refused before anything was applied, so a second copy can't apply twice.
        Verdict::RateLimited => true,
        // Maybe applied. Spelled out per method so a new one has to decide.
        Verdict::ServerError => match method {
            Method::Get | Method::List | Method::Update => true,
            // A second create could make a second contact; a second delete is the ladder's job.
            Method::Create | Method::Delete => false,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape Google's current APIs answer with: a code, a status word, and `ErrorInfo` details.
    fn info(code: u16, status: &str, reason: &str) -> String {
        serde_json::json!({ "error": {
            "code": code,
            "message": "example message that must never be returned",
            "status": status,
            "details": [{
                "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                "reason": reason,
                "domain": "googleapis.com"
            }]
        }})
        .to_string()
    }

    /// The older shape the People API's updateContact page still shows: `errors[].reason`.
    fn legacy(code: u16, reason: &str) -> String {
        serde_json::json!({ "error": {
            "code": code,
            "message": "example message that must never be returned",
            "errors": [{ "domain": "global", "reason": reason, "message": "x" }]
        }})
        .to_string()
    }

    #[test]
    fn a_stale_etag_is_a_conflict_not_a_bad_request() {
        // Calendar's 412 does not exist here: the People API says 400 `failedPrecondition`.
        for body in [
            legacy(400, "failedPrecondition"),
            info(400, "FAILED_PRECONDITION", "failedPrecondition"),
            serde_json::json!({"error":{"code":400,"status":"FAILED_PRECONDITION"}}).to_string(),
        ] {
            assert_eq!(classify(Method::Update, 400, &body), Verdict::Conflict);
        }
        // The same 400 on a create is just a bad request: there is no etag to be stale.
        assert_eq!(
            classify(Method::Create, 400, &legacy(400, "failedPrecondition")),
            Verdict::BadRequest
        );
        // A 400 about the request itself stays a bad request on an update.
        assert_eq!(
            classify(
                Method::Update,
                400,
                &info(400, "INVALID_ARGUMENT", "badValue")
            ),
            Verdict::BadRequest
        );
        // And a 412 is not what this API says: it is not read as a conflict.
        assert_eq!(classify(Method::Update, 412, ""), Verdict::Other(412));
    }

    #[test]
    fn the_expired_sync_token_is_matched_as_a_410_and_as_an_error_info_reason() {
        assert_eq!(classify(Method::List, 410, "{}"), Verdict::ExpiredSyncToken);
        assert_eq!(
            classify(Method::List, 410, "not json at all"),
            Verdict::ExpiredSyncToken
        );
        let documented = info(400, "FAILED_PRECONDITION", "EXPIRED_SYNC_TOKEN");
        assert_eq!(
            classify(Method::List, 400, &documented),
            Verdict::ExpiredSyncToken
        );
        assert!(is_expired_sync_token(400, &documented));
        assert!(is_expired_sync_token(410, ""));
        // Only the list call has a token: elsewhere a 410 is Gone and the reason is a bad request.
        assert_eq!(classify(Method::Get, 410, ""), Verdict::Gone);
        assert_eq!(classify(Method::Get, 400, &documented), Verdict::BadRequest);
        // A different 400 on the list call is not an expired token.
        assert_eq!(
            classify(
                Method::List,
                400,
                &info(400, "INVALID_ARGUMENT", "badValue")
            ),
            Verdict::BadRequest
        );
    }

    #[test]
    fn a_disabled_api_a_missing_scope_and_a_rate_limit_are_told_apart() {
        assert_eq!(
            classify(
                Method::Get,
                403,
                &info(403, "PERMISSION_DENIED", "SERVICE_DISABLED")
            ),
            Verdict::ApiDisabled
        );
        assert_eq!(
            classify(Method::Get, 403, &legacy(403, "accessNotConfigured")),
            Verdict::ApiDisabled
        );
        assert_eq!(
            classify(
                Method::Update,
                403,
                &info(403, "PERMISSION_DENIED", "ACCESS_TOKEN_SCOPE_INSUFFICIENT")
            ),
            Verdict::InsufficientScope
        );
        assert_eq!(
            classify(Method::Update, 403, &legacy(403, "insufficientPermissions")),
            Verdict::InsufficientScope
        );
        assert_eq!(
            classify(
                Method::Create,
                403,
                &info(403, "RESOURCE_EXHAUSTED", "RATE_LIMIT_EXCEEDED")
            ),
            Verdict::RateLimited
        );
        assert_eq!(
            classify(
                Method::Create,
                429,
                &info(429, "RESOURCE_EXHAUSTED", "RATE_LIMIT_EXCEEDED")
            ),
            Verdict::RateLimited
        );
        assert_eq!(
            classify(Method::List, 429, "<html>Too many</html>"),
            Verdict::RateLimited
        );
        // Any other 403 is no access, never "consent again" (that would loop).
        assert_eq!(
            classify(
                Method::Get,
                403,
                &info(403, "PERMISSION_DENIED", "forbidden")
            ),
            Verdict::Other(403)
        );
    }

    #[test]
    fn the_plain_statuses_have_their_verdicts() {
        assert_eq!(classify(Method::Get, 200, ""), Verdict::Ok);
        assert_eq!(classify(Method::Delete, 204, ""), Verdict::Ok);
        assert_eq!(
            classify(Method::Get, 401, &legacy(401, "authError")),
            Verdict::Reauth
        );
        assert_eq!(
            classify(Method::Get, 404, &info(404, "NOT_FOUND", "x")),
            Verdict::Gone
        );
        assert_eq!(classify(Method::Update, 404, "{}"), Verdict::Gone);
        assert_eq!(
            classify(Method::Get, 500, &info(500, "INTERNAL", "x")),
            Verdict::ServerError
        );
        assert_eq!(
            classify(Method::Get, 503, "Service Unavailable"),
            Verdict::ServerError
        );
        // A duplicate group name is a 409: it falls through with its status, not its text.
        assert_eq!(
            classify(Method::Create, 409, &info(409, "ALREADY_EXISTS", "x")),
            Verdict::Other(409)
        );
        assert_eq!(classify(Method::Get, 400, "not json"), Verdict::BadRequest);
    }

    #[test]
    fn only_transient_failures_are_retried_and_only_once() {
        let methods = [
            Method::Get,
            Method::List,
            Method::Create,
            Method::Update,
            Method::Delete,
        ];
        for method in methods {
            // Refused before it applied: always worth one more try.
            assert!(should_retry(method, Verdict::RateLimited, 1), "{method:?}");
            assert!(!should_retry(method, Verdict::RateLimited, 2), "{method:?}");
            assert!(!should_retry(method, Verdict::ServerError, 2), "{method:?}");
            // A server error may have applied: only a read or an etag-guarded update goes again,
            // never a create (a second contact) and not a delete (the ladder's job).
            assert_eq!(
                should_retry(method, Verdict::ServerError, 1),
                matches!(method, Method::Get | Method::List | Method::Update),
                "{method:?}"
            );
            for settled in [
                Verdict::Conflict,
                Verdict::Gone,
                Verdict::InsufficientScope,
                Verdict::Reauth,
                Verdict::ApiDisabled,
                Verdict::ExpiredSyncToken,
                Verdict::BadRequest,
                Verdict::Other(409),
                Verdict::Ok,
            ] {
                assert!(!should_retry(method, settled, 1), "{method:?} {settled:?}");
            }
        }
    }

    #[test]
    fn a_verdict_never_carries_the_body() {
        // Verdict is Copy and holds no string: a contact value echoed in a 400 has nowhere to go.
        let echo = r#"{"error":{"code":400,"message":"Invalid value for sam@example.com","status":"INVALID_ARGUMENT"}}"#;
        let verdict = classify(Method::Update, 400, echo);
        assert_eq!(verdict, Verdict::BadRequest);
        assert!(!format!("{verdict:?}").contains("example.com"));
    }
}
