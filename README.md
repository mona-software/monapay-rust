# MONA Pay Rust SDK

Crate Rust không có crate dependency, gồm client đầy đủ, JSON codec và SHA-256/HMAC tự cài bằng `std`. MONA Pay là cổng thanh toán và API ngân hàng của The MONA Group, giúp doanh nghiệp Việt Nam nhận và xác nhận tiền chuyển khoản theo thời gian thực qua tài khoản ảo (VA), VietQR, webhook và Telegram, thiết kế để cả lập trình viên lẫn AI agent tích hợp trong vài phút.

## Xác thực cho AI agent

```bash
export MONAPAY_CLIENT_ID="client-id"
export MONAPAY_CLIENT_SECRET="client-secret"
export MONAPAY_BASE_URL="https://api.monapay.vn"
```

```rust
let client = Client::from_env()?;
let profile = client.me()?;
let qr = client.qr().generate(qr_body)?;
let sandbox = client.sandbox().create_transaction(Value::object([("virtual_account_number", "MONA123".into()), ("amount", 10000.into()), ("description", "AI test".into())]))?;
println!("{}", profile.to_json());
```

`Client::from_env()` ưu tiên client credentials, cache token tới gần hạn và tự lấy lại khi gặp HTTP 401. Username/password chỉ là fallback tương thích cũ, không dùng cho AI agent vì sẽ gãy khi bật 2FA.

```rust
use monapay::{Client, Config, TransactionOptions};

let client = Client::new(
    Config::new(username, password).client_secret(client_secret)
)?;
let profile = client.me()?;
for transaction in client.transactions().iterate("MONA123", TransactionOptions::default()) {
    println!("{}", transaction?.to_json());
}
```

Client cache token theo hạn, refresh đúng một lần sau HTTP 401 và chỉ gắn `X-Client-Secret` vào POST/PUT/DELETE. Surface gồm keys, payment profile, hosted checkout, bank accounts, VA + hai OTP, QR, transactions + iterator/retry, webhook, sandbox, email configs/logs/suppressions.

## Trang thanh toán (hosted checkout)

```rust
let checkout = client.checkouts().create(Value::object([("amount", 250000.into()), ("order_code", "DH10234".into()), ("return_url", "https://shop.vn/payment/return".into())]), None)?;
redirect(checkout.get("checkout_url").unwrap().as_str().unwrap());
if event.get("type").and_then(Value::as_str) == Some("CHECKOUT_PAID") {
    fulfill_once(event.get("data").unwrap().get("order_code").unwrap());
}
```

SDK tự sinh `Idempotency-Key` cho `create` và `cancel`; truyền `Some(key)` khi anh chị cần dùng key riêng. Nguồn sự thật để giao hàng là webhook `CHECKOUT_PAID` hoặc kết quả `get`, không phải redirect trình duyệt.

Rust `std` không có TLS client. `CurlTransport` mặc định gọi executable `curl` hệ thống; toàn bộ config chứa credential/body được truyền qua stdin, không nằm trong argv và không qua shell. Ứng dụng không có `curl` có thể implement trait `Transport` bằng HTTP stack sẵn có rồi truyền `Config::transport(Arc::new(...))`; crate không ép thêm `reqwest` hay crate TLS.

```rust
let result = monapay::verify_webhook(raw_bytes, timestamp, signature, webhook_secret, 300)?;
if !result.ok { /* return 401 */ }
```

Verifier dùng HMAC-SHA256 thuần Rust và fixed-time digest comparison. Dùng `transaction_code` làm khóa idempotency. Ví dụ adapter Axum/Actix ở `examples/webhook_handler.rs`.

Gate offline: `cargo test --all-targets`, `cargo clippy --all-targets -- -D warnings`, `cargo package --allow-dirty`. Tài liệu: https://monapay.vn/docs · Hotline 1900 636 648 · info@themona.global.
