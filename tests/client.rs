use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{io::Read, io::Write, net::TcpListener, thread};

use monapay::{
    verify_webhook, Client, Config, CurlTransport, HttpRequest, HttpResponse, TransactionOptions,
    Transport, Value,
};

#[derive(Default)]
struct FakeState {
    calls: Vec<HttpRequest>,
    logins: usize,
    me_calls: usize,
}

#[derive(Default)]
struct FakeTransport {
    state: Mutex<FakeState>,
}

impl Transport for FakeTransport {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, monapay::Error> {
        let mut state = self.state.lock().unwrap();
        state.calls.push(request.clone());
        if request.url.ends_with("/api/v1/oauth/token") {
            state.logins += 1;
            assert_eq!(
                request
                    .body
                    .as_deref()
                    .and_then(|body| Value::parse_json(body).ok()),
                Some(Value::object([
                    ("grant_type", Value::from("client_credentials")),
                    ("client_id", Value::from("client-id")),
                    ("client_secret", Value::from("secret")),
                ]))
            );
            return Ok(envelope(Value::object([
                (
                    "access_token",
                    Value::from(format!("token-{}", state.logins)),
                ),
                ("expires_in", Value::from(3600)),
            ])));
        }
        if request.url.ends_with("/api/v1/client/me") {
            state.me_calls += 1;
            if state.me_calls == 1 {
                return Ok(HttpResponse {
                    status: 401,
                    body: r#"{"detail":"expired"}"#.to_owned(),
                });
            }
        }
        Ok(envelope(Value::object([("id", Value::from("ok"))])))
    }
}

fn envelope(data: Value) -> HttpResponse {
    HttpResponse {
        status: 200,
        body: Value::object([("success", Value::Bool(true)), ("data", data)]).to_json(),
    }
}

#[test]
fn cache_headers_and_refresh_once() {
    let transport = Arc::new(FakeTransport::default());
    let client = Client::new(
        Config::client_credentials("client-id", "secret")
            .base_url("https://example.test/")
            .transport(transport.clone()),
    )
    .unwrap();
    client
        .webhooks()
        .create(Value::object([("name", Value::from("Shop"))]))
        .unwrap();
    client.me().unwrap();

    let state = transport.state.lock().unwrap();
    assert_eq!(state.logins, 2);
    assert_eq!(
        state.calls[1]
            .headers
            .get("Authorization")
            .map(String::as_str),
        Some("Bearer token-1")
    );
    assert_eq!(
        state.calls[1]
            .headers
            .get("X-Client-Secret")
            .map(String::as_str),
        Some("secret")
    );
    assert!(!state
        .calls
        .last()
        .unwrap()
        .headers
        .contains_key("X-Client-Secret"));
    assert_eq!(
        state
            .calls
            .last()
            .unwrap()
            .headers
            .get("Authorization")
            .map(String::as_str),
        Some("Bearer token-2")
    );
}

#[test]
fn create_checkout_sends_idempotency_and_client_secret() {
    let transport = Arc::new(FakeTransport::default());
    let client = Client::new(
        Config::client_credentials("client-id", "secret")
            .base_url("https://example.test")
            .transport(transport.clone()),
    )
    .unwrap();
    client
        .checkouts()
        .create(
            Value::object([("amount", Value::from(250_000))]),
            Some("checkout-key"),
        )
        .unwrap();
    let state = transport.state.lock().unwrap();
    let request = state.calls.last().unwrap();
    assert_eq!(
        request.headers.get("Idempotency-Key").map(String::as_str),
        Some("checkout-key")
    );
    assert_eq!(
        request.headers.get("X-Client-Secret").map(String::as_str),
        Some("secret")
    );
}

struct PagingTransport {
    pages: Mutex<Vec<String>>,
}

impl Transport for PagingTransport {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, monapay::Error> {
        if request.url.ends_with("/api/v1/client/login") {
            return Ok(envelope(Value::object([(
                "access_token",
                Value::from("token"),
            )])));
        }
        assert!(!request.url.contains("since_id"));
        let page = request
            .url
            .split("page=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_owned();
        self.pages.lock().unwrap().push(page.clone());
        let items = if page == "1" {
            vec![
                Value::object([("id", Value::from("tx-3"))]),
                Value::object([("id", Value::from("tx-2"))]),
            ]
        } else {
            vec![Value::object([("id", Value::from("tx-1"))])]
        };
        Ok(envelope(Value::object([
            ("data", Value::Array(items)),
            ("last_page", Value::from(2)),
        ])))
    }
}

#[test]
fn iterator_paginates_and_stops_at_since_id() {
    let transport = Arc::new(PagingTransport {
        pages: Mutex::new(Vec::new()),
    });
    let client = Client::new(
        Config::new("user", "pass")
            .base_url("https://example.test")
            .transport(transport.clone()),
    )
    .unwrap();
    let options = TransactionOptions {
        page: 1,
        limit: 2,
        since_id: Some("tx-1".to_owned()),
    };
    let ids = client
        .transactions()
        .iterate("MONA 01", options)
        .map(|item| {
            item.unwrap()
                .get("id")
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(ids, ["tx-3", "tx-2"]);
    assert_eq!(*transport.pages.lock().unwrap(), ["1", "2"]);
}

#[test]
fn verifies_webhook_and_rejects_bad_signature() {
    let raw = br#"{"amount":2500000,"transaction_code":"FT1"}"#;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    // This value is generated in the test with a tiny independent reference implementation.
    let signature = format!(
        "sha256={}",
        reference_hmac_hex(
            b"test-secret",
            format!("{timestamp}.{}", String::from_utf8_lossy(raw)).as_bytes()
        )
    );
    let valid = verify_webhook(raw, &timestamp, &signature, "test-secret", 300).unwrap();
    assert!(valid.ok);
    let invalid = verify_webhook(
        raw,
        &timestamp,
        &format!("sha256={}", "0".repeat(64)),
        "test-secret",
        300,
    )
    .unwrap();
    assert_eq!(invalid.reason, Some("invalid_signature"));
}

#[test]
#[ignore = "sandbox Codex chặn bind localhost; chạy bằng cargo test -- --ignored trên release runner"]
fn curl_transport_uses_stdin_config_and_reads_status() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let length = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..length]);
        assert!(request.starts_with("POST /echo HTTP/1.1"));
        assert!(request
            .to_ascii_lowercase()
            .contains("x-client-secret: secret"));
        stream
            .write_all(b"HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}")
            .unwrap();
    });
    let response = CurlTransport
        .send(HttpRequest {
            method: "POST".to_owned(),
            url: format!("http://{address}/echo"),
            headers: [("X-Client-Secret".to_owned(), "secret".to_owned())]
                .into_iter()
                .collect(),
            body: Some("{\"hello\":\"MONA\"}".to_owned()),
        })
        .unwrap();
    server.join().unwrap();
    assert_eq!(response.status, 201);
    assert_eq!(response.body, "{\"ok\":true}");
}

fn reference_hmac_hex(key: &[u8], message: &[u8]) -> String {
    // RFC 4231 implementation kept local to the integration test to exercise public verification.
    let normalized = if key.len() > 64 {
        reference_sha256(key).to_vec()
    } else {
        key.to_vec()
    };
    let mut block = [0u8; 64];
    block[..normalized.len()].copy_from_slice(&normalized);
    let mut inner = block.iter().map(|byte| byte ^ 0x36).collect::<Vec<_>>();
    inner.extend_from_slice(message);
    let mut outer = block.iter().map(|byte| byte ^ 0x5c).collect::<Vec<_>>();
    outer.extend_from_slice(&reference_sha256(&inner));
    reference_sha256(&outer)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn reference_sha256(input: &[u8]) -> [u8; 32] {
    // A test-only call through macOS is intentionally avoided; use the same published SHA constants
    // through a compact independent state layout.
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut bytes = input.to_vec();
    let bits = (bytes.len() as u64) * 8;
    bytes.push(0x80);
    while bytes.len() % 64 != 56 {
        bytes.push(0);
    }
    bytes.extend_from_slice(&bits.to_be_bytes());
    let mut h = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    for chunk in bytes.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let a = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let b = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(a)
                .wrapping_add(w[i - 7])
                .wrapping_add(b);
        }
        let mut v = h;
        for i in 0..64 {
            let t1 = v[7]
                .wrapping_add(v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25))
                .wrapping_add((v[4] & v[5]) ^ (!v[4] & v[6]))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let t2 = (v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22))
                .wrapping_add((v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]));
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    let mut out = [0u8; 32];
    for i in 0..8 {
        out[i * 4..i * 4 + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}
