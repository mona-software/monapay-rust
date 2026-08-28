use std::collections::HashMap;

use monapay::verify_webhook;

/// The same function can sit inside an Axum or Actix handler. Framework body
/// extractors must provide the original bytes, not a re-serialized JSON value.
fn handle_webhook(raw: &[u8], headers: &HashMap<String, String>, secret: &str) -> u16 {
    let timestamp = headers
        .get("x-mona-timestamp")
        .map(String::as_str)
        .unwrap_or("");
    let signature = headers
        .get("x-mona-signature")
        .map(String::as_str)
        .unwrap_or("");
    match verify_webhook(raw, timestamp, signature, secret, 300) {
        Ok(result) if result.ok => {
            let payload = result.payload.unwrap();
            // Persist with transaction_code as a unique/idempotency key.
            println!("verified MONA Pay transaction: {}", payload.to_json());
            200
        }
        _ => 401,
    }
}

fn main() {
    let _ = handle_webhook(b"{}", &HashMap::new(), "replace-me");
}
