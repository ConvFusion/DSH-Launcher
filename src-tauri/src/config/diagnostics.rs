//! The boundary between private operational state and shareable diagnostics.

use serde_json::Value;

/// Redact credential values while preserving URL spelling, ordinary parameters,
/// routes and surrounding errors. Runtime URLs never pass through this function
/// before use or persistence.
pub fn redact_diagnostic_text(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        // Also cover JSON-escaped slashes in historical textual logs.
        let scheme = ["https://", "http://", "https:\\/\\/", "http:\\/\\/"]
            .iter()
            .find(|scheme| {
                bytes
                    .get(i..i + scheme.len())
                    .is_some_and(|part| part.eq_ignore_ascii_case(scheme.as_bytes()))
            });
        let Some(scheme) = scheme else {
            i += 1;
            continue;
        };
        let start = i + scheme.len();
        // Match the startup extractor's whitespace boundary. Quotes are valid
        // inside URL paths/queries: stopping at one can expose a later &token=.
        let end = bytes[start..]
            .iter()
            .position(u8::is_ascii_whitespace)
            .map_or(bytes.len(), |offset| start + offset);
        // A quoted URL in a compact JSON log ends before the next field. An
        // embedded quote followed by '&token=' must still stay inside the URL.
        let end = match bytes.get(i.wrapping_sub(1)) {
            Some(quote @ (b'"' | b'\'')) => bytes[start..end]
                .windows(2)
                .position(|pair| pair[0] == *quote && matches!(pair[1], b',' | b'}' | b']'))
                .map_or(end, |offset| start + offset),
            _ => end,
        };
        // Keep the username and endpoint in authenticated registry/proxy URLs.
        let authority_end = bytes[start..end]
            .iter()
            .position(|b| matches!(b, b'/' | b'?' | b'#'))
            .map_or(end, |offset| start + offset);
        if let Some(at) = text[start..authority_end].rfind('@') {
            let at = start + at;
            if let Some(colon) = text[start..at].find(':') {
                let password = start + colon + 1;
                if password < at && &text[password..at] != "[REDACTED]" {
                    out.push_str(&text[copied..password]);
                    out.push_str("[REDACTED]");
                    copied = at;
                }
            }
        }
        let mut cursor = authority_end;
        while cursor < end {
            if !matches!(bytes[cursor], b'?' | b'#' | b'&' | b';') {
                cursor += 1;
                continue;
            }
            let key_start = cursor + 1;
            let key_end = bytes[key_start..end]
                .iter()
                .position(|b| matches!(b, b'=' | b'?' | b'#' | b'&' | b';'))
                .map_or(end, |offset| key_start + offset);
            if key_end == end || bytes[key_end] != b'=' {
                cursor = key_end;
                continue;
            }
            let value_start = key_end + 1;
            // A later '?' may start a nested URL's query; scan it separately.
            if !sensitive_key(&text[key_start..key_end]) {
                cursor = value_start;
                continue;
            }
            let value_end = bytes[value_start..end]
                .iter()
                .position(|b| matches!(b, b'&' | b'#' | b';' | 0x1b))
                .map_or(end, |offset| value_start + offset);
            let mut secret_end = value_end;
            // Presentation punctuation is not part of a credential. Do not
            // stop at embedded quotes, which are valid in a URL value.
            while secret_end > value_start
                && matches!(
                    bytes[secret_end - 1],
                    b'"' | b'\'' | b')' | b']' | b'}' | b','
                )
            {
                if text[value_start..secret_end].trim_start_matches(['\'', '"']) == "[REDACTED]" {
                    break;
                }
                secret_end -= 1;
            }
            let secret_start = if secret_end > value_start
                && matches!(bytes[value_start], b'"' | b'\'')
                && bytes.get(secret_end) == Some(&bytes[value_start])
            {
                value_start + 1
            } else {
                value_start
            };
            if secret_end > secret_start && &text[secret_start..secret_end] != "[REDACTED]" {
                out.push_str(&text[copied..secret_start]);
                out.push_str("[REDACTED]");
                copied = secret_end;
            }
            cursor = value_end;
        }
        i = end;
    }
    out.push_str(&text[copied..]);
    out
}

// Explicit credential names avoid hiding diagnostic fields such as token_count,
// auth_enabled, public_key, cache_key or ordinary OAuth error codes.
fn sensitive_key(key: &str) -> bool {
    let mut decoded = Vec::with_capacity(key.len());
    let mut bytes = key.as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let Some(hi) = bytes.next().and_then(|b| (b as char).to_digit(16)) else {
                return false;
            };
            let Some(lo) = bytes.next().and_then(|b| (b as char).to_digit(16)) else {
                return false;
            };
            decoded.push((hi * 16 + lo) as u8);
        } else {
            decoded.push(byte);
        }
    }
    let key: String = decoded
        .into_iter()
        .filter(|b| !matches!(b, b'_' | b'-'))
        .map(|b| (b as char).to_ascii_lowercase())
        .collect();
    matches!(
        key.as_str(),
        "authorization"
            | "proxyauthorization"
            | "apikey"
            | "password"
            | "passwd"
            | "pwd"
            | "secret"
            | "signature"
            | "sig"
            | "xamzcredential"
            | "xamzsignature"
            | "xamzsecuritytoken"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
            | "authtoken"
            | "bearertoken"
            | "sessiontoken"
            | "csrftoken"
            | "xsrftoken"
    ) || key.ends_with("secret")
        || key.ends_with("password")
        || key.ends_with("apikey")
}

/// Preserve JSON fields and original types; decode escapes before filtering.
/// Invalid JSON is explicitly omitted so the rest of the support bundle remains
/// available, without ever falling back to the raw file contents.
pub fn diagnostic_file(name: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    if !matches!(name, "state.json" | "config.json") {
        return Ok(redact_diagnostic_text(&String::from_utf8_lossy(data)).into_bytes());
    }
    let mut value = serde_json::from_slice::<Value>(data).unwrap_or_else(|error| {
        serde_json::json!({
            "diagnostic_error": format!(
                "Invalid {name} at line {}, column {}; source contents omitted.",
                error.line(), error.column()
            ),
        })
    });
    redact_json_strings(&mut value);
    serde_json::to_vec_pretty(&value).map_err(|_| format!("cannot export {name}"))
}

fn redact_json_strings(value: &mut Value) {
    match value {
        Value::String(text) => *text = redact_diagnostic_text(text),
        Value::Array(values) => values.iter_mut().for_each(redact_json_strings),
        Value::Object(values) => {
            for (key, value) in values {
                if sensitive_key(key) && !value.is_null() && value.as_str() != Some("") {
                    *value = Value::String("[REDACTED]".into());
                } else {
                    redact_json_strings(value);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProcessStateFile;

    #[cfg(unix)]
    #[test]
    fn diagnostic_log_sinks_redact_before_persistence_and_stderr() {
        // A separate test process isolates HOME without mutating this process's
        // environment or ever touching the user's actual launcher logs.
        let dir = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "config::diagnostics::tests::diagnostic_log_sink_child",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("HOME", dir.path())
            .env("DSH_DIAGNOSTIC_TEST_HOME", dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("synthetic-log-secret"));
        for name in ["launcher.log", "harness.log", "install.log"] {
            let text =
                std::fs::read_to_string(dir.path().join(".dsh-launcher/logs").join(name)).unwrap();
            assert!(!text.contains("synthetic-log-secret"));
            assert!(text.contains("http://localhost/?token=[REDACTED]&port=3080#logs"));
            assert!(text.contains("ERROR connection refused"));
        }
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "only run by the parent test with an isolated HOME"]
    fn diagnostic_log_sink_child() {
        let home =
            std::env::var_os("DSH_DIAGNOSTIC_TEST_HOME").expect("isolated test HOME required");
        assert_eq!(dirs::home_dir().unwrap(), std::path::PathBuf::from(home));
        let text = "opened http://localhost/?token=synthetic-log-secret&port=3080#logs ERROR connection refused";
        super::super::log(text);
        super::super::harness_log_line(text);
        super::super::install_log_line(text);
    }

    #[test]
    fn diagnostic_urls_cover_encodings_duplicates_fragments_and_multiple_urls() {
        for url in [
            "http://127.0.0.1:3080/?token=synthetic-secret",
            "http://127.0.0.1:3080/?foo='&token=synthetic-secret",
            "http://127.0.0.1:3080/a'b?token=synthetic-secret",
            "http://127.0.0.1:3080/?token='synthetic-secret'",
            "https://localhost/?x=1&token=synthetic-secret&token=other-secret",
            "http://[::1]:3080/?%74oken=synthetic-secret",
            "HTTP://localhost/?token=synthetic%2Dsecret",
            "http://localhost/#token=synthetic-secret",
            "http://localhost/&token=synthetic-secret",
            r"http:\/\/localhost/?token=synthetic-secret",
        ] {
            let input = format!("版本 0.1.6 opened '{url}' in browser; ERROR connection refused");
            let out = redact_diagnostic_text(&input);
            assert!(!out.contains("synthetic"), "{out}");
            assert!(!out.contains("other-secret"), "{out}");
            assert!(out.contains("[REDACTED]"));
            assert!(out.contains(" in browser; ERROR connection refused"));
            assert!(out.starts_with("版本 0.1.6"));
        }
        assert_eq!(
            redact_diagnostic_text("http://a/?token=one https://b/?token=two"),
            "http://a/?token=[REDACTED] https://b/?token=[REDACTED]"
        );
        assert_eq!(
            redact_diagnostic_text("http://a/?token=one\u{1b}[0m ready"),
            "http://a/?token=[REDACTED]\u{1b}[0m ready"
        );
    }

    #[test]
    fn diagnostic_plain_text_and_tokenless_urls_remain_intact() {
        for text in [
            "ERROR: node not found (pid 123 port 3080)",
            "https://github.com/ConvFusion/DSH-Launcher",
            "http://[::1]:3080/",
            "http://localhost:3080/?profile=web&port=3080&debug=true#logs",
            "https://registry.npmjs.org/pkg?version=1.2.3&cache_key=abc&public_key=def",
            "http://localhost/?token_count=42&auth_enabled=true&next_token=page2#error=401",
            "",
            "版本信息",
        ] {
            assert_eq!(redact_diagnostic_text(text), text);
        }
    }

    #[test]
    fn diagnostic_json_preserves_fields_and_decodes_before_redaction() {
        let data = br#"{"node_path":"http:\/\/localhost/\u003fto\u006ben=synthetic-secret&profile=web#logs", "future_secret":"synthetic-secret", "future_setting":{"enabled":true}, "browser":{"type":"chrome", "future_secret":"synthetic-secret"}, "server":{"host":"localhost", "port":3080}}"#;
        let exported = diagnostic_file("config.json", data).unwrap();
        let text = String::from_utf8(exported).unwrap();
        assert!(!text.contains("synthetic-secret"));
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["future_secret"], "[REDACTED]");
        assert_eq!(value["browser"]["future_secret"], "[REDACTED]");
        assert_eq!(value["future_setting"]["enabled"], true);
        assert_eq!(value["browser"]["type"], "chrome");
        assert_eq!(value["server"]["port"], 3080);
        assert_eq!(
            value["node_path"],
            "http://localhost/?token=[REDACTED]&profile=web#logs"
        );
    }

    #[test]
    fn diagnostic_invalid_json_is_explicitly_omitted() {
        for name in ["config.json", "state.json"] {
            for data in [b"{token=synthetic-secret".as_slice(), b"\xff"] {
                let exported = diagnostic_file(name, data).unwrap();
                let value: Value = serde_json::from_slice(&exported).unwrap();
                assert!(value["diagnostic_error"]
                    .as_str()
                    .unwrap()
                    .contains("source contents omitted"));
                assert!(!String::from_utf8(exported)
                    .unwrap()
                    .contains("synthetic-secret"));
            }
        }
        let bytes =
            diagnostic_file("harness.log.old", b"\xff http://a/?token=synthetic-secret").unwrap();
        assert!(!String::from_utf8(bytes)
            .unwrap()
            .contains("synthetic-secret"));
    }

    #[test]
    fn diagnostic_export_preserves_private_runtime_state_and_legacy_records() {
        let state = ProcessStateFile {
            pid: 123,
            host: "127.0.0.1".into(),
            port: 3080,
            started_at: "start".into(),
            process_start_time: Some(42),
            token_url: Some("http://localhost/?token=synthetic-secret".into()),
        };
        let private = serde_json::to_vec(&state).unwrap();
        let exported = diagnostic_file("state.json", &private).unwrap();
        let value: Value = serde_json::from_slice(&exported).unwrap();
        assert_eq!(value["token_url"], "http://localhost/?token=[REDACTED]");
        let restored: ProcessStateFile = serde_json::from_slice(&private).unwrap();
        assert_eq!(restored.token_url, state.token_url);
        let legacy = br#"{"pid":123,"host":"localhost","port":3080,"started_at":"start"}"#;
        let value: Value =
            serde_json::from_slice(&diagnostic_file("state.json", legacy).unwrap()).unwrap();
        assert_eq!(value["pid"], 123);
        assert!(value.get("process_start_time").is_none());
    }

    #[test]
    fn diagnostic_redaction_preserves_every_noncredential_url_component() {
        for (input, expected) in [
            (
                "opened 'http://localhost:3080/a'b?profile=web&token='private'&debug=true#logs' ERROR 401",
                "opened 'http://localhost:3080/a'b?profile=web&token='[REDACTED]'&debug=true#logs' ERROR 401",
            ),
            (
                "http://[::1]:3080/?%74o%6Ben=private&token=second&empty=&port=3080#/settings?theme=dark",
                "http://[::1]:3080/?%74o%6Ben=[REDACTED]&token=[REDACTED]&empty=&port=3080#/settings?theme=dark",
            ),
            (
                "https://registry/?version=1#access_token=private&scope=read&token_type=Bearer",
                "https://registry/?version=1#access_token=[REDACTED]&scope=read&token_type=Bearer",
            ),
            (
                "HTTP://alice:private@proxy:8080/path?API-Key=private&retry=3#status",
                "HTTP://alice:[REDACTED]@proxy:8080/path?API-Key=[REDACTED]&retry=3#status",
            ),
            (
                "http://alice:p&token=private@proxy:8080/path?token=private&retry=3",
                "http://alice:[REDACTED]@proxy:8080/path?token=[REDACTED]&retry=3",
            ),
            (
                r#"http:\/\/localhost/?token=private&mode=dev#logs"#,
                r#"http:\/\/localhost/?token=[REDACTED]&mode=dev#logs"#,
            ),
            (
                "http://localhost/?token=&mode=dev",
                "http://localhost/?token=&mode=dev",
            ),
            (
                "http://localhost/?redirect=http://other/?token=private&profile=web",
                "http://localhost/?redirect=http://other/?token=[REDACTED]&profile=web",
            ),
            (
                r#"{"url":"http://localhost/?token=private&profile=web","code":401,"message":"connection refused"}"#,
                r#"{"url":"http://localhost/?token=[REDACTED]&profile=web","code":401,"message":"connection refused"}"#,
            ),
        ] {
            assert_eq!(redact_diagnostic_text(input), expected);
            assert_eq!(redact_diagnostic_text(expected), expected);
        }
    }

    #[test]
    fn diagnostic_json_preserves_invalid_types_arrays_and_missing_fields() {
        for input in [
            r#"{"server":{"port":"not-a-number"},"theme":"custom","unknown":42,"secret":null,"api_key":""}"#,
            r#"{"pid":"invalid","host":"localhost","token_url":null}"#,
            r#"[{"version":"1.2.3","api_key":"private"},null,42]"#,
            "null",
        ] {
            let mut expected: Value = serde_json::from_str(input).unwrap();
            if let Some(items) = expected.as_array_mut() {
                items[0]["api_key"] = Value::String("[REDACTED]".into());
            }
            for name in ["config.json", "state.json"] {
                let exported = diagnostic_file(name, input.as_bytes()).unwrap();
                assert_eq!(
                    serde_json::from_slice::<Value>(&exported).unwrap(),
                    expected
                );
            }
        }
    }

    #[test]
    fn diagnostic_sensitive_names_are_targeted_and_encoding_aware() {
        for key in [
            "token",
            "%74o%6ben",
            "Access_Token",
            "refresh-token",
            "idToken",
            "authToken",
            "session_token",
            "csrf_token",
            "API-Key",
            "convfusionDevApiKey",
            "password",
            "client_secret",
            "Authorization",
            "X-Amz-Signature",
        ] {
            let input = format!("https://localhost/?{key}=private&profile=web#logs");
            assert_eq!(
                redact_diagnostic_text(&input),
                format!("https://localhost/?{key}=[REDACTED]&profile=web#logs")
            );
        }
        for key in [
            "token_count",
            "next_token",
            "token_type",
            "auth_enabled",
            "public_key",
            "cache_key",
            "password_enabled",
            "error",
            "code",
            "profile",
            "version",
            "unknown",
            "%broken",
        ] {
            let input = format!("http://localhost/?{key}=diagnostic-value#logs");
            assert_eq!(redact_diagnostic_text(&input), input);
        }
    }
}
