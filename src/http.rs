use std::borrow::Cow;

use eyre::{Context, Result, bail};
use reqwest::Response;
use serde::de::DeserializeOwned;

const DIAGNOSTIC_BODY_LIMIT: usize = 1024;
const TRUNCATION_MARKER: &str = "... [truncated]";

pub(crate) async fn parse_json_response<T: DeserializeOwned>(
    response: std::result::Result<Response, reqwest::Error>,
    backend: &str,
) -> Result<T> {
    let response = response
        .map_err(reqwest::Error::without_url)
        .wrap_err_with(|| format!("failed to query {backend}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .wrap_err_with(|| format!("failed to read {backend} response body"))?;

    if !status.is_success() {
        bail!(
            "{backend} query failed with status {status}: {}",
            diagnostic_body(&body)
        );
    }

    serde_json::from_str(&body)
        .wrap_err_with(|| format!("failed to parse {backend} response as the expected schema"))
}

pub(crate) fn diagnostic_body(body: &str) -> Cow<'_, str> {
    if body.len() <= DIAGNOSTIC_BODY_LIMIT {
        return Cow::Borrowed(body);
    }

    let mut end = DIAGNOSTIC_BODY_LIMIT - TRUNCATION_MARKER.len();
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!("{}{TRUNCATION_MARKER}", &body[..end]))
}

#[cfg(test)]
mod tests {
    use std::{io::Write, net::TcpListener, thread};

    use reqwest::Client;

    use super::*;

    #[test]
    fn bounds_diagnostics_at_utf_8_boundaries() {
        let body = format!("{}é{} sentinel", "x".repeat(1008), "y".repeat(20));

        let diagnostic = diagnostic_body(&body);

        assert!(diagnostic.len() <= DIAGNOSTIC_BODY_LIMIT);
        assert!(diagnostic.starts_with(&"x".repeat(1008)));
        assert!(diagnostic.ends_with(TRUNCATION_MARKER));
        assert!(!diagnostic.contains('é'));
        assert!(!diagnostic.contains("sentinel"));
    }

    #[tokio::test]
    async fn reports_response_body_read_failures() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nab",
                )
                .unwrap();
        });
        let response = Client::new()
            .get(format!("http://{address}/sentinel-secret"))
            .send()
            .await;

        let error = parse_json_response::<serde_json::Value>(response, "Test")
            .await
            .unwrap_err();

        server.join().unwrap();
        assert_eq!(error.to_string(), "failed to read Test response body");
        assert!(!format!("{error:?}").contains("sentinel-secret"));
    }
}
