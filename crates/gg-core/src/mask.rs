//! Credentials in URLs (`https://user:token@host/…`) are hidden from everything the core hands
//! the UI (R36): git's messages and output in outcomes, the fetch state, errors, events and
//! the command log. git hides them in the URLs it prints itself, but hooks, helpers and the
//! config (a remote's address) do not.
//!
//! Applied at the boundary: [`Core::dispatch`](crate::Core::dispatch) masks its results and
//! errors, and the event sink the core is given is wrapped in [`MaskingSink`]. Commit content
//! (rows, details, search) is served as git has it, and the user's own settings and
//! preferences, which the UI sends back, are left alone.

use std::borrow::Cow;
use std::sync::Arc;

use serde_json::Value;

use crate::EventSink;
use crate::error::CoreError;

/// `text` with the userinfo of every URL in it (`https://user:token@host/…`, where a
/// credential may be) replaced by `***`: `https://***@host/…`. The authority ends at the
/// first `/`, `?`, `#`, quote, angle bracket, backtick, space or control character; its last
/// `@` ends the userinfo. `git@host:path` (no scheme) carries no secret and stays.
pub(crate) fn mask_userinfo(text: &str) -> Cow<'_, str> {
    if !text.contains("://") {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let start = at + 3;
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest
            .find(|c: char| {
                matches!(c, '/' | '?' | '#' | '\'' | '"' | '<' | '>' | '`')
                    || c.is_whitespace()
                    || c.is_control()
            })
            .unwrap_or(rest.len());
        if let Some(last) = rest[..end].rfind('@') {
            out.push_str("***");
            rest = &rest[last..];
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// `value` with [`mask_userinfo`] applied to every string in it (object keys are field names
/// and stay).
pub(crate) fn mask_json(mut value: Value) -> Value {
    mask_in_place(&mut value);
    value
}

fn mask_in_place(value: &mut Value) {
    match value {
        Value::String(s) => {
            if let Cow::Owned(masked) = mask_userinfo(s) {
                *s = masked;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask_in_place),
        Value::Object(fields) => fields.values_mut().for_each(mask_in_place),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

impl CoreError {
    /// This error with credentials in URLs masked in its message and detail.
    pub(crate) fn masked(self) -> Self {
        Self {
            message: mask_userinfo(&self.message).into_owned(),
            detail: self.detail.map(|d| mask_userinfo(&d).into_owned()),
            ..self
        }
    }
}

/// Whether the result of command `cmd` is masked. Not: commit content, which is shown as git
/// has it (the user wrote it; masking would make the app disagree with `git log`), and the
/// user's own settings and preferences, which the UI edits and sends back (a masked editor
/// command would be saved masked).
pub(crate) fn masks_result_of(cmd: &str) -> bool {
    !matches!(
        cmd,
        "get_rows"
            | "find_row"
            | "search_commits"
            | "search_bodies"
            | "commit_details"
            | "get_app_state"
            | "update_settings"
            | "get_repo_prefs"
            | "set_repo_prefs"
    )
}

/// Passes events on with credentials in URLs masked.
pub(crate) struct MaskingSink(pub(crate) Arc<dyn EventSink>);

impl EventSink for MaskingSink {
    fn emit(&self, name: &str, payload: Value) {
        self.0.emit(name, mask_json(payload));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn userinfo_is_masked_only_inside_urls() {
        assert_eq!(mask_userinfo("plain text"), "plain text");
        assert_eq!(mask_userinfo("a@b.c, x://"), "a@b.c, x://");
        assert_eq!(
            mask_userinfo("http://host/p@q"),
            "http://host/p@q",
            "@ in the path"
        );
        assert_eq!(
            mask_userinfo("https://u:p@h/ then https://h2/?a=b@c"),
            "https://***@h/ then https://h2/?a=b@c"
        );
        assert_eq!(mask_userinfo("file:///tmp/a@b"), "file:///tmp/a@b");
        assert_eq!(mask_userinfo("https://a@b@host/x"), "https://***@host/x");
        assert_eq!(mask_userinfo("é https://ü:ß@h"), "é https://***@h");
    }

    #[test]
    fn every_string_of_a_json_value_is_masked() {
        let value = json!({
            "message": "fatal: https://u:SECRET@h/r",
            "https://k:SECRET@h/": 1,
            "dangers": [{"message": "to ssh://SECRET@h/x", "token": null}],
            "fetch": {"lastError": "x", "lastErrorDetail": "y https://SECRET@h"},
            "n": 3, "ok": true,
        });
        let masked = mask_json(value);
        assert_eq!(masked["message"], "fatal: https://***@h/r");
        assert_eq!(masked["dangers"][0]["message"], "to ssh://***@h/x");
        assert_eq!(masked["fetch"]["lastErrorDetail"], "y https://***@h");
        assert_eq!(masked["n"], 3);
        // Keys are field names, never git's text.
        assert_eq!(masked["https://k:SECRET@h/"], 1);
    }

    #[test]
    fn errors_are_masked_in_message_and_detail() {
        let e = CoreError::new(crate::error::ErrorCode::GitFailed, "at https://u:p@h/x")
            .with_detail("remote: https://u:p@h/y")
            .masked();
        assert_eq!(e.message, "at https://***@h/x");
        assert_eq!(e.detail.as_deref(), Some("remote: https://***@h/y"));
    }

    #[test]
    fn commit_content_and_the_users_settings_are_served_unmasked() {
        for cmd in [
            "get_rows",
            "commit_details",
            "get_app_state",
            "set_repo_prefs",
        ] {
            assert!(!masks_result_of(cmd), "{cmd}");
        }
        for cmd in [
            "run_op",
            "get_snapshot",
            "open_repo",
            "refresh",
            "command_log",
        ] {
            assert!(masks_result_of(cmd), "{cmd}");
        }
    }
}
