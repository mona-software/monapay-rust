# MONA Pay Rust SDK

Crate Rust không có crate dependency, gồm client đầy đủ, JSON codec và SHA-256/HMAC tự cài bằng `std`. MONA Pay là cổng thanh toán và API ngân hàng của The MONA Group, giúp doanh nghiệp Việt Nam nhận và xác nhận tiền chuyển khoản theo thời gian thực qua tài khoản ảo (VA), VietQR, webhook và Telegram — thiết kế để cả lập trình viên lẫn AI agent tích hợp trong vài phút.

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

Client tự login/cache token, refresh đúng một lần sau HTTP 401 và chỉ gắn `X-Client-Secret` vào POST/PUT/DELETE. Surface gồm keys, bank accounts, VA + hai OTP, QR, transactions + iterator, retry, webhook configs/test và logs/stats.

Rust `std` không có TLS client. `CurlTransport` mặc định gọi executable `curl` hệ thống; toàn bộ config chứa credential/body được truyền qua stdin, không nằm trong argv và không qua shell. Ứng dụng không có `curl` có thể implement trait `Transport` bằng HTTP stack sẵn có rồi truyền `Config::transport(Arc::new(...))`—crate không ép thêm `reqwest` hay crate TLS.

```rust
let result = monapay::verify_webhook(raw_bytes, timestamp, signature, webhook_secret, 300)?;
if !result.ok { /* return 401 */ }
```

Verifier dùng HMAC-SHA256 thuần Rust và fixed-time digest comparison. Dùng `transaction_code` làm khóa idempotency. Ví dụ adapter Axum/Actix ở `examples/webhook_handler.rs`.

Gate offline: `cargo test --all-targets`, `cargo clippy --all-targets -- -D warnings`, `cargo package --allow-dirty`. Tài liệu: https://monapay.vn/docs · Hotline 1900 636 648 · info@themona.global.
