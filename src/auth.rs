use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::Response,
};
use base64::Engine;

#[derive(Clone)]
pub struct AuthConfig {
    /// If present, holds the expected (username, password). If None, the
    /// server remains unprotected (default behavior).
    pub credentials: Option<(String, String)>,
}

/// HTTP Basic Auth middleware: if auth.credentials is None, everything
/// passes through unchecked. Otherwise it requires an "Authorization:
/// Basic <base64(user:pass)>" header matching exactly the configured
/// credentials, responding 401 with WWW-Authenticate otherwise (the
/// browser then shows its native login prompt).
pub async fn require_basic_auth(
    State(auth): State<AuthConfig>,
    req: Request,
    next: Next,
) -> Response {
    let Some(expected) = &auth.credentials else {
        return next.run(req).await;
    };

    let header_value = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let authorized = check_basic_auth_header(header_value, expected);

    if authorized {
        next.run(req).await
    } else {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(header::WWW_AUTHENTICATE, "Basic realm=\"sbalite\"")
            .body(Body::empty())
            .unwrap()
    }
}

/// Pure credential check, separated from the axum middleware above so it
/// can be unit tested directly without spinning up a server or an HTTP
/// request.
pub fn check_basic_auth_header(header_value: Option<&str>, expected: &(String, String)) -> bool {
    let (expected_user, expected_pass) = expected;

    header_value
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|encoded| {
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()
        })
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|creds| {
            creds
                .split_once(':')
                .map(|(u, p)| (u.to_string(), p.to_string()))
        })
        .map(|(user, pass)| &user == expected_user && &pass == expected_pass)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic_header(user: &str, pass: &str) -> String {
        let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        format!("Basic {encoded}")
    }

    #[test]
    fn basic_auth_accepts_correct_credentials() {
        let expected = ("admin".to_string(), "secret".to_string());
        let header = basic_header("admin", "secret");
        assert!(check_basic_auth_header(Some(&header), &expected));
    }

    #[test]
    fn basic_auth_rejects_wrong_password() {
        let expected = ("admin".to_string(), "secret".to_string());
        let header = basic_header("admin", "wrong");
        assert!(!check_basic_auth_header(Some(&header), &expected));
    }

    #[test]
    fn basic_auth_rejects_wrong_username() {
        let expected = ("admin".to_string(), "secret".to_string());
        let header = basic_header("someone-else", "secret");
        assert!(!check_basic_auth_header(Some(&header), &expected));
    }

    #[test]
    fn basic_auth_rejects_missing_header() {
        let expected = ("admin".to_string(), "secret".to_string());
        assert!(!check_basic_auth_header(None, &expected));
    }

    #[test]
    fn basic_auth_rejects_non_basic_scheme() {
        let expected = ("admin".to_string(), "secret".to_string());
        assert!(!check_basic_auth_header(Some("Bearer sometoken"), &expected));
    }

    #[test]
    fn basic_auth_rejects_malformed_base64() {
        let expected = ("admin".to_string(), "secret".to_string());
        assert!(!check_basic_auth_header(
            Some("Basic not-valid-base64!!!"),
            &expected
        ));
    }
}