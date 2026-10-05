# monapay

Rust SDK for the MONA Pay API: create checkout links and VietQR codes, manage virtual accounts, webhooks and email notifications, and verify signed webhooks.

Requires Rust 1.70+. No crate dependencies: the client, JSON codec and SHA-256/HMAC are implemented with `std`.

## Install

The package is not on crates.io yet. Until it is, install it from GitHub:

```bash
cargo add monapay --git https://github.com/mona-software/monapay-rust
```

Once it is published on crates.io:

```bash
cargo add monapay
```

The default transport runs the system `curl` executable (see [HTTP transport](#http-transport)).

## Quick start

```rust
use monapay::{Client, Value};

fn main() -> Result<(), monapay::Error> {
    let client = Client::from_env()?;
    let checkout = client.checkouts().create(
        Value::object([
            ("amount", 250000.into()),
            ("order_code", "DH10234".into()),
            ("return_url", "https://shop.example/payment/return".into()),
        ]),
        None,
    )?;
    println!("{}", checkout.get("checkout_url").and_then(Value::as_str).unwrap_or_default());
    Ok(())
}
```

## Usage

### Client

```rust
use monapay::{Client, Config};

// From environment variables (see Configuration)
let client = Client::from_env()?;

// Or explicitly
let client = Client::new(
    Config::client_credentials(client_id, client_secret)
        .base_url("https://api.monapay.vn"), // optional
)?;

let profile = client.me()?;
println!("{}", profile.to_json());
```

- Client credentials are the recommended login. `Config::new(username, password)` is a legacy fallback and does not work for accounts with 2FA enabled.
- The token is cached until 60 seconds before `expires_in`. A request that fails with HTTP 401 triggers a new login and is retried once.
- `X-Client-Secret` is sent on non-GET requests when a client secret is set.
- Request and response bodies are `monapay::Value`. Build objects with `Value::object([...])`, read them with `get`, `as_str`, `as_i64` and friends.
- Methods return the `data` field of the response. Errors are `monapay::Error` with `message`, `status` and `body`.

### Resources

| Accessor | Methods |
| --- | --- |
| `keys()` | `generate`, `list`, `destroy`, `reveal`, `rotate` |
| `bank_accounts()` | `list` |
| `virtual_accounts()` | `register`, `verify`, `register_notification`, `verify_notification`, `list` |
| `payment_profile()` | `get`, `set`, `rotate_return_secret`, `reveal_return_secret` |
| `checkouts()` | `create`, `get`, `list`, `cancel` |
| `qr()` | `generate`, `cancel` |
| `transactions()` | `list`, `iterate`, `retry` |
| `sandbox()` | `create_transaction` |
| `webhooks()` | `list`, `create`, `update`, `remove`, `test` |
| `webhook_logs()` | `list`, `stats` |
| `email_configs()` | `list`, `create`, `get`, `update`, `remove`, `verify`, `resend_verification`, `test` |
| `email_logs()` | `list`, `stats` |
| `email_suppressions()` | `list`, `remove` |

### Hosted checkout

`checkouts().create` and `checkouts().cancel` take an `Option<&str>` idempotency key. Pass `None` to let the SDK generate one, or `Some(key)` to set your own. Fulfil orders from the `CHECKOUT_PAID` webhook or from `checkouts().get`, not from the browser redirect.

### Sandbox

```rust
let tx = client.sandbox().create_transaction(Value::object([
    ("virtual_account_number", "MONA123".into()),
    ("amount", 10000.into()),
    ("description", "Sandbox test".into()),
]))?;
```

### Transactions

```rust
use monapay::TransactionOptions;

for transaction in client.transactions().iterate("MONA123", TransactionOptions::default()) {
    println!("{}", transaction?.to_json());
}
```

`TransactionOptions` has `page`, `limit` and `since_id`. With `since_id` set, the iterator stops before the item whose `id` or `transaction_code` matches; it is not sent to the API.

### Webhooks

Verify the raw request bytes. `timestamp` and `signature` are the `X-Mona-Timestamp` and `X-Mona-Signature` header values; the last argument is the tolerance in seconds.

```rust
let result = monapay::verify_webhook(raw_bytes, timestamp, signature, webhook_secret, 300)?;
if !result.ok {
    // respond with HTTP 401
}
```

The verifier uses HMAC-SHA256 and a fixed-time digest comparison. Use `transaction_code` as the idempotency key. `examples/webhook_handler.rs` shows a handler function that can sit inside an Axum or Actix route.

### HTTP transport

Rust `std` has no TLS client. The default `CurlTransport` runs the system `curl` executable and passes the full request config (credentials and body included) over stdin, not in argv and not through a shell. To use another HTTP stack, implement the `Transport` trait and pass it with `Config::transport(Arc::new(...))`.

## Configuration

`Client::from_env()` reads:

| Variable | Purpose |
| --- | --- |
| `MONAPAY_CLIENT_ID`, `MONAPAY_CLIENT_SECRET` | API key credentials (recommended) |
| `MONAPAY_USERNAME`, `MONAPAY_PASSWORD` | Legacy password login; does not work for accounts with 2FA enabled |
| `MONAPAY_BASE_URL` | API base URL, defaults to `https://api.monapay.vn` |

Documentation: https://monapay.vn/docs

## Development

```bash
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

## License

MIT

**MONA Pay is part of MONA Cloud by The MONA Group.**
