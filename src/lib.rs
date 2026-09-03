//! Standard-library-only MONA Pay client.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const VERSION: &str = "0.3.0";
pub const DEFAULT_BASE_URL: &str = "https://api.monapay.vn";

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl Value {
    pub fn object<I, K>(entries: I) -> Self
    where
        I: IntoIterator<Item = (K, Value)>,
        K: Into<String>,
    {
        Value::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    pub fn array<I>(items: I) -> Self
    where
        I: IntoIterator<Item = Value>,
    {
        Value::Array(items.into_iter().collect())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object()?.get(key)
    }

    pub fn as_object(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Object(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Number(value) if value.is_finite() => Some(*value as i64),
            _ => None,
        }
    }

    pub fn to_json(&self) -> String {
        let mut output = String::new();
        write_json(self, &mut output);
        output
    }

    pub fn parse_json(input: &str) -> Result<Self, Error> {
        JsonParser::new(input).parse()
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::String(value.to_owned())
    }
}
impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::String(value)
    }
}
impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}
impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Value::Number(value as f64)
    }
}
impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Number(value as f64)
    }
}
impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Value::Number(value as f64)
    }
}
impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Value::Number(value as f64)
    }
}
impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Number(value)
    }
}

#[derive(Clone, Debug)]
pub struct Error {
    pub message: String,
    pub status: Option<u16>,
    pub body: Option<Value>,
}

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: None,
            body: None,
        }
    }

    fn api(message: impl Into<String>, status: u16, body: Option<Value>) -> Self {
        Self {
            message: message.into(),
            status: Some(status),
            body,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            Some(status) => write!(formatter, "MONA Pay HTTP {status}: {}", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<String>,
}

#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub trait Transport: Send + Sync {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error>;
}

/// HTTPS transport backed by the system `curl` executable.
///
/// The generated curl config, including auth headers/body, is sent over stdin;
/// credentials are never included in process arguments or passed through a shell.
#[derive(Clone, Debug, Default)]
pub struct CurlTransport;

impl Transport for CurlTransport {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        validate_transport_value("method", &request.method)?;
        validate_transport_value("URL", &request.url)?;
        let mut config = String::from("silent\nshow-error\nconnect-timeout = 10\nmax-time = 30\n");
        config.push_str(&format!("request = \"{}\"\n", curl_escape(&request.method)));
        config.push_str(&format!("url = \"{}\"\n", curl_escape(&request.url)));
        for (name, value) in &request.headers {
            validate_transport_value("header name", name)?;
            validate_transport_value("header value", value)?;
            config.push_str(&format!(
                "header = \"{}\"\n",
                curl_escape(&format!("{name}: {value}"))
            ));
        }
        if let Some(body) = &request.body {
            config.push_str(&format!("data-binary = \"{}\"\n", curl_escape(body)));
        }
        config.push_str("write-out = \"\\n__MONAPAY_HTTP_STATUS__:%{http_code}\"\n");

        let mut child = Command::new("curl")
            .args(["--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| Error::new(format!("không chạy được curl: {error}")))?;
        child
            .stdin
            .take()
            .ok_or_else(|| Error::new("không mở được stdin của curl"))?
            .write_all(config.as_bytes())
            .map_err(|error| Error::new(format!("không gửi được curl config: {error}")))?;
        let output = child
            .wait_with_output()
            .map_err(|error| Error::new(format!("không đọc được curl response: {error}")))?;
        if !output.status.success() {
            return Err(Error::new(format!(
                "curl lỗi: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let raw = String::from_utf8(output.stdout)
            .map_err(|_| Error::new("MONA Pay trả response không phải UTF-8"))?;
        let marker = "\n__MONAPAY_HTTP_STATUS__:";
        let at = raw
            .rfind(marker)
            .ok_or_else(|| Error::new("curl response thiếu HTTP status"))?;
        let status = raw[at + marker.len()..]
            .trim()
            .parse::<u16>()
            .map_err(|_| Error::new("curl trả HTTP status không hợp lệ"))?;
        Ok(HttpResponse {
            status,
            body: raw[..at].to_owned(),
        })
    }
}

fn validate_transport_value(label: &str, value: &str) -> Result<(), Error> {
    if value
        .bytes()
        .any(|byte| byte == 0 || byte == b'\r' || byte == b'\n')
    {
        return Err(Error::new(format!(
            "{label} chứa control character không hợp lệ"
        )));
    }
    Ok(())
}

fn curl_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

pub struct Config {
    client_id: String,
    username: String,
    password: String,
    client_secret: String,
    base_url: String,
    transport: Arc<dyn Transport>,
}

impl Config {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            client_id: String::new(),
            username: username.into(),
            password: password.into(),
            client_secret: String::new(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            transport: Arc::new(CurlTransport),
        }
    }

    pub fn client_credentials(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            username: String::new(),
            password: String::new(),
            client_secret: client_secret.into(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            transport: Arc::new(CurlTransport),
        }
    }

    pub fn client_secret(mut self, value: impl Into<String>) -> Self {
        self.client_secret = value.into();
        self
    }

    pub fn base_url(mut self, value: impl Into<String>) -> Self {
        self.base_url = value.into();
        self
    }

    pub fn transport(mut self, value: Arc<dyn Transport>) -> Self {
        self.transport = value;
        self
    }
}

struct Inner {
    client_id: String,
    username: String,
    password: String,
    base_url: String,
    access_token: Mutex<Option<CachedToken>>,
    client_secret: Mutex<String>,
    transport: Arc<dyn Transport>,
}

#[derive(Clone)]
struct CachedToken {
    value: String,
    expires_at: SystemTime,
}

#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    pub fn new(config: Config) -> Result<Self, Error> {
        let has_client_credentials =
            !config.client_id.trim().is_empty() && !config.client_secret.trim().is_empty();
        let has_password_credentials =
            !config.username.trim().is_empty() && !config.password.is_empty();
        if !has_client_credentials && !has_password_credentials {
            return Err(Error::new("cần client ID + client secret hoặc username + password; không dùng password cho AI agent vì sẽ gãy khi bật 2FA"));
        }
        let base_url = config.base_url.trim_end_matches('/').to_owned();
        if !(base_url.starts_with("https://") || base_url.starts_with("http://")) {
            return Err(Error::new("base_url phải dùng http hoặc https"));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                client_id: config.client_id,
                username: config.username,
                password: config.password,
                base_url,
                access_token: Mutex::new(None),
                client_secret: Mutex::new(config.client_secret),
                transport: config.transport,
            }),
        })
    }

    pub fn from_env() -> Result<Self, Error> {
        let client_id = std::env::var("MONAPAY_CLIENT_ID").unwrap_or_default();
        let client_secret = std::env::var("MONAPAY_CLIENT_SECRET").unwrap_or_default();
        let mut config = if !client_id.is_empty() && !client_secret.is_empty() {
            Config::client_credentials(client_id, client_secret)
        } else {
            Config::new(
                std::env::var("MONAPAY_USERNAME").unwrap_or_default(),
                std::env::var("MONAPAY_PASSWORD").unwrap_or_default(),
            )
            .client_secret(client_secret)
        };
        if let Ok(base_url) = std::env::var("MONAPAY_BASE_URL") {
            config = config.base_url(base_url);
        }
        Self::new(config)
    }

    pub fn login(&self) -> Result<String, Error> {
        if let Some(token) = self.token()? {
            return Ok(token);
        }
        let client_secret = self.inner.client_secret.lock().map_err(lock_error)?.clone();
        let using_client_credentials =
            !self.inner.client_id.is_empty() && !client_secret.is_empty();
        let data = self.send(
            "POST",
            if using_client_credentials {
                "/api/v1/oauth/token"
            } else {
                "/api/v1/client/login"
            },
            Some(if using_client_credentials {
                Value::object([
                    ("grant_type", Value::from("client_credentials")),
                    ("client_id", Value::from(self.inner.client_id.clone())),
                    ("client_secret", Value::from(client_secret)),
                ])
            } else {
                Value::object([
                    ("username", Value::from(self.inner.username.clone())),
                    ("password", Value::from(self.inner.password.clone())),
                ])
            }),
            &[],
            None,
            false,
        )?;
        let token = data
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::new("Response đăng nhập không có access_token"))?
            .to_owned();
        let mut cached = self.inner.access_token.lock().map_err(lock_error)?;
        let default_expires = if using_client_credentials {
            3600
        } else {
            86_400
        };
        let expires_in = data
            .get("expires_in")
            .and_then(Value::as_i64)
            .unwrap_or(default_expires)
            .max(0) as u64;
        let cached_token = CachedToken {
            value: token.clone(),
            expires_at: SystemTime::now() + Duration::from_secs(expires_in.saturating_sub(60)),
        };
        if cached.is_none() {
            *cached = Some(cached_token);
        }
        Ok(cached
            .as_ref()
            .map(|item| item.value.clone())
            .unwrap_or(token))
    }

    pub fn me(&self) -> Result<Value, Error> {
        self.request("GET", "/api/v1/client/me", None, &[])
    }

    pub fn keys(&self) -> Keys<'_> {
        Keys { client: self }
    }
    pub fn virtual_accounts(&self) -> VirtualAccounts<'_> {
        VirtualAccounts { client: self }
    }
    pub fn bank_accounts(&self) -> BankAccounts<'_> {
        BankAccounts { client: self }
    }
    pub fn qr(&self) -> QrPayments<'_> {
        QrPayments { client: self }
    }
    pub fn transactions(&self) -> Transactions<'_> {
        Transactions { client: self }
    }
    pub fn webhooks(&self) -> Webhooks<'_> {
        Webhooks { client: self }
    }
    pub fn webhook_logs(&self) -> WebhookLogs<'_> {
        WebhookLogs { client: self }
    }
    pub fn sandbox(&self) -> Sandbox<'_> {
        Sandbox { client: self }
    }
    pub fn email_configs(&self) -> EmailConfigs<'_> {
        EmailConfigs { client: self }
    }
    pub fn email_logs(&self) -> EmailLogs<'_> {
        EmailLogs { client: self }
    }
    pub fn email_suppressions(&self) -> EmailSuppressions<'_> {
        EmailSuppressions { client: self }
    }

    pub fn set_client_secret(&self, value: impl Into<String>) -> Result<(), Error> {
        *self.inner.client_secret.lock().map_err(lock_error)? = value.into();
        Ok(())
    }

    fn token(&self) -> Result<Option<String>, Error> {
        let mut cached = self.inner.access_token.lock().map_err(lock_error)?;
        if cached
            .as_ref()
            .map(|item| item.expires_at <= SystemTime::now())
            .unwrap_or(false)
        {
            *cached = None;
        }
        Ok(cached.as_ref().map(|item| item.value.clone()))
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        query: &[(&str, String)],
    ) -> Result<Value, Error> {
        let token = self.login()?;
        match self.send(method, path, body.clone(), query, Some(&token), true) {
            Err(error) if error.status == Some(401) => {
                let mut cached = self.inner.access_token.lock().map_err(lock_error)?;
                if cached.as_ref().map(|item| item.value.as_str()) == Some(token.as_str()) {
                    *cached = None;
                }
                drop(cached);
                let refreshed = self.login()?;
                self.send(method, path, body, query, Some(&refreshed), true)
            }
            result => result,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        query: &[(&str, String)],
        token: Option<&str>,
        authenticated: bool,
    ) -> Result<Value, Error> {
        let mut url = format!("{}{}", self.inner.base_url, path);
        let query_string = query
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        if !query_string.is_empty() {
            url.push('?');
            url.push_str(&query_string);
        }
        let mut headers = BTreeMap::from([("Accept".to_owned(), "application/json".to_owned())]);
        let raw_body = body.as_ref().map(Value::to_json);
        if raw_body.is_some() {
            headers.insert("Content-Type".to_owned(), "application/json".to_owned());
        }
        if authenticated {
            if let Some(token) = token {
                headers.insert("Authorization".to_owned(), format!("Bearer {token}"));
            }
            if method != "GET" {
                let secret = self.inner.client_secret.lock().map_err(lock_error)?.clone();
                if !secret.is_empty() {
                    headers.insert("X-Client-Secret".to_owned(), secret);
                }
            }
        }
        let response = self.inner.transport.send(HttpRequest {
            method: method.to_owned(),
            url,
            headers,
            body: raw_body,
        })?;
        let envelope = if response.body.is_empty() {
            Value::Object(BTreeMap::new())
        } else {
            Value::parse_json(&response.body).map_err(|_| {
                Error::api(
                    format!(
                        "MONA Pay trả response không phải JSON (HTTP {})",
                        response.status
                    ),
                    response.status,
                    Some(Value::from(response.body.clone())),
                )
            })?
        };
        let failed_envelope = envelope.get("success").and_then(Value::as_bool) == Some(false);
        if !(200..300).contains(&response.status) || failed_envelope {
            let message = envelope
                .get("message")
                .or_else(|| envelope.get("detail"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("MONA Pay API lỗi HTTP {}", response.status));
            return Err(Error::api(message, response.status, Some(envelope)));
        }
        Ok(envelope.get("data").cloned().unwrap_or(Value::Null))
    }
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> Error {
    Error::new("MONA Pay client lock bị poison")
}

#[derive(Clone, Copy)]
pub struct Keys<'a> {
    client: &'a Client,
}

impl Keys<'_> {
    pub fn generate(&self, name: &str) -> Result<Value, Error> {
        let data = self.client.request(
            "POST",
            "/api/v1/client-keys/generate",
            Some(Value::object([(
                "name",
                Value::from(if name.is_empty() { "Default Key" } else { name }),
            )])),
            &[],
        )?;
        if let Some(secret) = data
            .get("client_secret")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            self.client.set_client_secret(secret)?;
        }
        Ok(data)
    }

    pub fn list(&self) -> Result<Value, Error> {
        self.client
            .request("GET", "/api/v1/client-keys/list", None, &[])
    }

    pub fn destroy(&self, key_id: &str) -> Result<Value, Error> {
        self.client.request(
            "DELETE",
            &format!("/api/v1/client-keys/destroy/{}", percent_encode(key_id)),
            None,
            &[],
        )
    }
}

#[derive(Clone, Copy)]
pub struct VirtualAccounts<'a> {
    client: &'a Client,
}

impl VirtualAccounts<'_> {
    pub fn register(&self, body: Value) -> Result<Value, Error> {
        self.client.request(
            "POST",
            "/api/v1/acb/virtual-account/registration",
            Some(body),
            &[],
        )
    }

    pub fn verify(&self, request_id: &str, code: &str) -> Result<Value, Error> {
        self.client.request(
            "POST",
            &format!(
                "/api/v1/acb/{}/virtual-account/verification",
                percent_encode(request_id)
            ),
            Some(Value::object([("code", Value::from(code))])),
            &[],
        )
    }

    pub fn register_notification(
        &self,
        virtual_account_id: &str,
        body: Value,
    ) -> Result<Value, Error> {
        self.client.request(
            "POST",
            &format!(
                "/api/v1/acb/{}/notification/registration",
                percent_encode(virtual_account_id)
            ),
            Some(body),
            &[],
        )
    }

    pub fn verify_notification(&self, request_id: &str, code: &str) -> Result<Value, Error> {
        self.client.request(
            "POST",
            &format!(
                "/api/v1/acb/{}/notification/verification",
                percent_encode(request_id)
            ),
            Some(Value::object([("code", Value::from(code))])),
            &[],
        )
    }

    pub fn list(&self, bank_account_id: &str) -> Result<Value, Error> {
        self.client.request(
            "GET",
            &format!(
                "/api/v1/acb/{}/virtual-account/retrieve",
                percent_encode(bank_account_id)
            ),
            None,
            &[],
        )
    }
}

#[derive(Clone, Copy)]
pub struct BankAccounts<'a> {
    client: &'a Client,
}

impl BankAccounts<'_> {
    pub fn list(&self) -> Result<Value, Error> {
        self.client
            .request("GET", "/api/v1/client/bank-accounts", None, &[])
    }
}

#[derive(Clone, Copy)]
pub struct QrPayments<'a> {
    client: &'a Client,
}

impl QrPayments<'_> {
    pub fn generate(&self, body: Value) -> Result<Value, Error> {
        self.client
            .request("POST", "/api/v1/acb/qr-payment/generate", Some(body), &[])
    }

    pub fn cancel(&self, qr_code_id: &str, body: Option<Value>) -> Result<Value, Error> {
        self.client.request(
            "DELETE",
            &format!(
                "/api/v1/acb/qr-payment/{}/cancellation",
                percent_encode(qr_code_id)
            ),
            body,
            &[],
        )
    }
}

#[derive(Clone, Debug)]
pub struct TransactionOptions {
    pub page: u32,
    pub limit: u32,
    pub since_id: Option<String>,
}

impl Default for TransactionOptions {
    fn default() -> Self {
        Self {
            page: 1,
            limit: 100,
            since_id: None,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Transactions<'a> {
    client: &'a Client,
}

impl<'client> Transactions<'client> {
    pub fn list(
        &self,
        virtual_account_number: &str,
        options: &TransactionOptions,
    ) -> Result<Value, Error> {
        if virtual_account_number.is_empty() {
            return Err(Error::new("virtual_account_number là bắt buộc"));
        }
        self.client.request(
            "GET",
            "/api/v1/acb/virtual-account/transactions",
            None,
            &[
                ("virtual_account_number", virtual_account_number.to_owned()),
                ("page", options.page.max(1).to_string()),
                ("limit", options.limit.max(1).to_string()),
            ],
        )
    }

    pub fn iterate(
        &self,
        virtual_account_number: impl Into<String>,
        options: TransactionOptions,
    ) -> TransactionIterator<'client> {
        TransactionIterator {
            resource: Transactions {
                client: self.client,
            },
            virtual_account_number: virtual_account_number.into(),
            page: options.page.max(1),
            limit: options.limit.max(1),
            since_id: options.since_id,
            items: VecDeque::new(),
            done: false,
        }
    }

    pub fn retry(
        &self,
        transaction_id: &str,
        target_type: &str,
        target_id: Option<&str>,
    ) -> Result<Value, Error> {
        let mut body = BTreeMap::from([("target_type".to_owned(), Value::from(target_type))]);
        if let Some(target_id) = target_id.filter(|value| !value.is_empty()) {
            body.insert("target_id".to_owned(), Value::from(target_id));
        }
        self.client.request(
            "POST",
            &format!(
                "/api/v1/acb/virtual-account/transactions/{}/retry",
                percent_encode(transaction_id)
            ),
            Some(Value::Object(body)),
            &[],
        )
    }
}

pub struct TransactionIterator<'a> {
    resource: Transactions<'a>,
    virtual_account_number: String,
    page: u32,
    limit: u32,
    since_id: Option<String>,
    items: VecDeque<Value>,
    done: bool,
}

impl Iterator for TransactionIterator<'_> {
    type Item = Result<Value, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.items.pop_front() {
                return Some(Ok(item));
            }
            if self.done {
                return None;
            }
            let options = TransactionOptions {
                page: self.page,
                limit: self.limit,
                since_id: None,
            };
            let response = match self.resource.list(&self.virtual_account_number, &options) {
                Ok(response) => response,
                Err(error) => {
                    self.done = true;
                    return Some(Err(error));
                }
            };
            let object = match response.as_object() {
                Some(object) => object,
                None => {
                    self.done = true;
                    return Some(Err(Error::new("Response giao dịch không phải object")));
                }
            };
            if let Some(items) = object.get("data").and_then(Value::as_array) {
                for item in items {
                    if self
                        .since_id
                        .as_deref()
                        .map(|checkpoint| transaction_matches(item, checkpoint))
                        .unwrap_or(false)
                    {
                        self.done = true;
                        break;
                    }
                    self.items.push_back(item.clone());
                }
            }
            if !self.done {
                self.done = match object.get("has_next").and_then(Value::as_bool) {
                    Some(has_next) => !has_next,
                    None => {
                        self.page
                            >= object
                                .get("last_page")
                                .and_then(Value::as_i64)
                                .unwrap_or(self.page as i64) as u32
                    }
                };
            }
            self.page += 1;
        }
    }
}

fn transaction_matches(item: &Value, checkpoint: &str) -> bool {
    ["id", "transaction_code"]
        .iter()
        .any(|key| item.get(key).map(value_as_checkpoint).as_deref() == Some(checkpoint))
}

fn value_as_checkpoint(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Number(value) if value.fract() == 0.0 => format!("{value:.0}"),
        other => other.to_json(),
    }
}

#[derive(Clone, Copy)]
pub struct Webhooks<'a> {
    client: &'a Client,
}

impl Webhooks<'_> {
    pub fn list(&self) -> Result<Value, Error> {
        self.client
            .request("GET", "/api/v1/client-webhooks", None, &[])
    }
    pub fn create(&self, body: Value) -> Result<Value, Error> {
        self.client
            .request("POST", "/api/v1/client-webhooks", Some(body), &[])
    }
    pub fn update(&self, config_id: &str, body: Value) -> Result<Value, Error> {
        self.client.request(
            "PUT",
            &format!("/api/v1/client-webhooks/{}", percent_encode(config_id)),
            Some(body),
            &[],
        )
    }
    pub fn remove(&self, config_id: &str) -> Result<Value, Error> {
        self.client.request(
            "DELETE",
            &format!("/api/v1/client-webhooks/{}", percent_encode(config_id)),
            None,
            &[],
        )
    }
    pub fn test(&self, body: Option<Value>) -> Result<Value, Error> {
        self.client.request(
            "POST",
            "/api/v1/client-webhooks/test",
            Some(body.unwrap_or_else(|| Value::object([("is_dummy", Value::Bool(true))]))),
            &[],
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct WebhookLogOptions {
    pub status: Option<String>,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub page: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Clone, Copy)]
pub struct WebhookLogs<'a> {
    client: &'a Client,
}

impl WebhookLogs<'_> {
    pub fn list(&self, options: &WebhookLogOptions) -> Result<Value, Error> {
        let query = webhook_log_query(options);
        self.client
            .request("GET", "/api/v1/webhook-logs", None, &query_refs(&query))
    }
    pub fn stats(&self, options: &WebhookLogOptions) -> Result<Value, Error> {
        let query = webhook_log_query(options);
        self.client.request(
            "GET",
            "/api/v1/webhook-logs/stats",
            None,
            &query_refs(&query),
        )
    }
}

fn webhook_log_query(options: &WebhookLogOptions) -> Vec<(String, String)> {
    let mut query = Vec::new();
    if let Some(value) = &options.status {
        query.push(("status".to_owned(), value.clone()));
    }
    if let Some(value) = &options.from_date {
        query.push(("from_date".to_owned(), value.clone()));
    }
    if let Some(value) = &options.to_date {
        query.push(("to_date".to_owned(), value.clone()));
    }
    if let Some(value) = options.page {
        query.push(("page".to_owned(), value.to_string()));
    }
    if let Some(value) = options.limit {
        query.push(("limit".to_owned(), value.to_string()));
    }
    query
}

fn query_refs(query: &[(String, String)]) -> Vec<(&str, String)> {
    query
        .iter()
        .map(|(key, value)| (key.as_str(), value.clone()))
        .collect()
}

#[derive(Clone, Copy)]
pub struct Sandbox<'a> {
    client: &'a Client,
}

impl Sandbox<'_> {
    pub fn create_transaction(&self, body: Value) -> Result<Value, Error> {
        self.client
            .request("POST", "/api/v1/sandbox/transactions", Some(body), &[])
    }
}

#[derive(Clone, Copy)]
pub struct EmailConfigs<'a> {
    client: &'a Client,
}

impl EmailConfigs<'_> {
    pub fn list(&self) -> Result<Value, Error> {
        self.client
            .request("GET", "/api/v1/email-configs", None, &[])
    }
    pub fn create(&self, body: Value) -> Result<Value, Error> {
        self.client
            .request("POST", "/api/v1/email-configs", Some(body), &[])
    }
    pub fn get(&self, config_id: &str) -> Result<Value, Error> {
        self.client.request(
            "GET",
            &format!("/api/v1/email-configs/{}", percent_encode(config_id)),
            None,
            &[],
        )
    }
    pub fn update(&self, config_id: &str, body: Value) -> Result<Value, Error> {
        self.client.request(
            "PUT",
            &format!("/api/v1/email-configs/{}", percent_encode(config_id)),
            Some(body),
            &[],
        )
    }
    pub fn remove(&self, config_id: &str) -> Result<Value, Error> {
        self.client.request(
            "DELETE",
            &format!("/api/v1/email-configs/{}", percent_encode(config_id)),
            None,
            &[],
        )
    }
    pub fn verify(&self, config_id: &str, email: &str, code: &str) -> Result<Value, Error> {
        self.client.request(
            "POST",
            &format!("/api/v1/email-configs/{}/verify", percent_encode(config_id)),
            Some(Value::object([
                ("email", Value::from(email)),
                ("code", Value::from(code)),
            ])),
            &[],
        )
    }
    pub fn resend_verification(&self, config_id: &str, email: &str) -> Result<Value, Error> {
        self.client.request(
            "POST",
            &format!(
                "/api/v1/email-configs/{}/resend-verification",
                percent_encode(config_id)
            ),
            Some(Value::object([("email", Value::from(email))])),
            &[],
        )
    }
    pub fn test(&self, config_id: &str) -> Result<Value, Error> {
        self.client.request(
            "POST",
            &format!("/api/v1/email-configs/{}/test", percent_encode(config_id)),
            Some(Value::object(Vec::<(String, Value)>::new())),
            &[],
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct EmailLogOptions {
    pub config_id: Option<String>,
    pub status: Option<String>,
    pub event_type: Option<String>,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub page: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Clone, Copy)]
pub struct EmailLogs<'a> {
    client: &'a Client,
}

impl EmailLogs<'_> {
    pub fn list(&self, options: &EmailLogOptions) -> Result<Value, Error> {
        let query = email_log_query(options, true);
        self.client
            .request("GET", "/api/v1/email-logs", None, &query_refs(&query))
    }
    pub fn stats(&self, options: &EmailLogOptions) -> Result<Value, Error> {
        let query = email_log_query(options, false);
        self.client
            .request("GET", "/api/v1/email-logs/stats", None, &query_refs(&query))
    }
}

fn email_log_query(options: &EmailLogOptions, include_filters: bool) -> Vec<(String, String)> {
    let mut query = Vec::new();
    if include_filters {
        for (key, value) in [
            ("config_id", &options.config_id),
            ("status", &options.status),
            ("event_type", &options.event_type),
        ] {
            if let Some(value) = value {
                query.push((key.to_owned(), value.clone()));
            }
        }
    }
    for (key, value) in [
        ("from_date", &options.from_date),
        ("to_date", &options.to_date),
    ] {
        if let Some(value) = value {
            query.push((key.to_owned(), value.clone()));
        }
    }
    if include_filters {
        if let Some(value) = options.page {
            query.push(("page".to_owned(), value.to_string()));
        }
        if let Some(value) = options.limit {
            query.push(("limit".to_owned(), value.to_string()));
        }
    }
    query
}

#[derive(Clone, Copy)]
pub struct EmailSuppressions<'a> {
    client: &'a Client,
}

impl EmailSuppressions<'_> {
    pub fn list(&self) -> Result<Value, Error> {
        self.client
            .request("GET", "/api/v1/email-suppressions", None, &[])
    }
    pub fn remove(&self, email: &str) -> Result<Value, Error> {
        self.client.request(
            "DELETE",
            &format!("/api/v1/email-suppressions/{}", percent_encode(email)),
            None,
            &[],
        )
    }
}

fn percent_encode(value: &str) -> String {
    let mut output = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            output.push(*byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

#[derive(Clone, Debug, PartialEq)]
pub struct WebhookResult {
    pub ok: bool,
    pub reason: Option<&'static str>,
    pub payload: Option<Value>,
}

impl WebhookResult {
    fn invalid(reason: &'static str) -> Self {
        Self {
            ok: false,
            reason: Some(reason),
            payload: None,
        }
    }
}

pub fn verify_webhook(
    raw: &[u8],
    timestamp: &str,
    signature: &str,
    secret: &str,
    tolerance: i64,
) -> Result<WebhookResult, Error> {
    if tolerance < 0 {
        return Err(Error::new("tolerance phải là số không âm"));
    }
    if timestamp.is_empty() {
        return Ok(WebhookResult::invalid("missing_timestamp"));
    }
    if !timestamp.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(WebhookResult::invalid("invalid_timestamp"));
    }
    let unix = match timestamp.parse::<i64>() {
        Ok(value) => value,
        Err(_) => return Ok(WebhookResult::invalid("invalid_timestamp")),
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::new("system clock nằm trước Unix epoch"))?
        .as_secs() as i64;
    if now.abs_diff(unix) > tolerance as u64 {
        return Ok(WebhookResult::invalid("timestamp_out_of_tolerance"));
    }
    if signature.is_empty() {
        return Ok(WebhookResult::invalid("missing_signature"));
    }

    let mut signed = Vec::with_capacity(timestamp.len() + raw.len() + 1);
    signed.extend_from_slice(timestamp.as_bytes());
    signed.push(b'.');
    signed.extend_from_slice(raw);
    let expected = hmac_sha256(secret.as_bytes(), &signed);
    let valid_format = signature.len() == 71
        && signature.starts_with("sha256=")
        && signature[7..].bytes().all(|byte| byte.is_ascii_hexdigit());
    let supplied = if valid_format {
        decode_hex_32(&signature[7..]).unwrap_or([0; 32])
    } else {
        [0; 32]
    };
    if !constant_time_equal(&expected, &supplied) || !valid_format {
        return Ok(WebhookResult::invalid("invalid_signature"));
    }
    let raw = match std::str::from_utf8(raw) {
        Ok(value) => value,
        Err(_) => return Ok(WebhookResult::invalid("invalid_json")),
    };
    let payload = match Value::parse_json(raw) {
        Ok(value) => value,
        Err(_) => return Ok(WebhookResult::invalid("invalid_json")),
    };
    Ok(WebhookResult {
        ok: true,
        reason: None,
        payload: Some(payload),
    })
}

fn constant_time_equal(left: &[u8; 32], right: &[u8; 32]) -> bool {
    let mut different = 0u8;
    for index in 0..32 {
        different |= left[index] ^ right[index];
    }
    different == 0
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut output = [0u8; 32];
    for (index, slot) in output.iter_mut().enumerate() {
        *slot = (hex_nibble(value.as_bytes()[index * 2])? << 4)
            | hex_nibble(value.as_bytes()[index * 2 + 1])?;
    }
    Some(output)
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let normalized = if key.len() > 64 {
        sha256(key).to_vec()
    } else {
        key.to_vec()
    };
    let mut block = [0u8; 64];
    block[..normalized.len()].copy_from_slice(&normalized);
    let mut inner = Vec::with_capacity(64 + message.len());
    let mut outer = Vec::with_capacity(96);
    for byte in block {
        inner.push(byte ^ 0x36);
        outer.push(byte ^ 0x5c);
    }
    inner.extend_from_slice(message);
    outer.extend_from_slice(&sha256(&inner));
    sha256(&outer)
}

fn sha256(input: &[u8]) -> [u8; 32] {
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
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut message = input.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    let mut state = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    for chunk in message.chunks_exact(64) {
        let mut words = [0u32; 64];
        for (index, word) in words.iter_mut().take(16).enumerate() {
            let at = index * 4;
            *word = u32::from_be_bytes([chunk[at], chunk[at + 1], chunk[at + 2], chunk[at + 3]]);
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(sum1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = sum0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut output = [0u8; 32];
    for (index, value) in state.iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&value.to_be_bytes());
    }
    output
}

fn write_json(value: &Value, output: &mut String) {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) if value.is_finite() && value.fract() == 0.0 => {
            output.push_str(&format!("{value:.0}"))
        }
        Value::Number(value) if value.is_finite() => output.push_str(&value.to_string()),
        Value::Number(_) => output.push_str("null"),
        Value::String(value) => write_json_string(value, output),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_json(value, output);
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            for (index, (key, value)) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_json_string(key, output);
                output.push(':');
                write_json(value, output);
            }
            output.push('}');
        }
    }
}

fn write_json_string(value: &str, output: &mut String) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0C}' => output.push_str("\\f"),
            value if value < '\u{20}' => output.push_str(&format!("\\u{:04x}", value as u32)),
            value => output.push(value),
        }
    }
    output.push('"');
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            position: 0,
        }
    }

    fn parse(mut self) -> Result<Value, Error> {
        self.whitespace();
        let value = self.value()?;
        self.whitespace();
        if self.position != self.bytes.len() {
            return Err(Error::new("JSON có dữ liệu thừa"));
        }
        Ok(value)
    }

    fn value(&mut self) -> Result<Value, Error> {
        self.whitespace();
        match self.peek() {
            Some(b'n') => {
                self.literal(b"null")?;
                Ok(Value::Null)
            }
            Some(b't') => {
                self.literal(b"true")?;
                Ok(Value::Bool(true))
            }
            Some(b'f') => {
                self.literal(b"false")?;
                Ok(Value::Bool(false))
            }
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b'[') => self.array(),
            Some(b'{') => self.object(),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(Error::new("JSON value không hợp lệ")),
        }
    }

    fn literal(&mut self, expected: &[u8]) -> Result<(), Error> {
        if self
            .bytes
            .get(self.position..self.position + expected.len())
            == Some(expected)
        {
            self.position += expected.len();
            Ok(())
        } else {
            Err(Error::new("JSON literal không hợp lệ"))
        }
    }

    fn string(&mut self) -> Result<String, Error> {
        self.expect(b'"')?;
        let mut output = Vec::new();
        loop {
            let byte = self
                .next()
                .ok_or_else(|| Error::new("JSON string chưa đóng"))?;
            match byte {
                b'"' => {
                    return String::from_utf8(output)
                        .map_err(|_| Error::new("JSON string không phải UTF-8"))
                }
                b'\\' => match self
                    .next()
                    .ok_or_else(|| Error::new("JSON escape thiếu ký tự"))?
                {
                    b'"' => output.push(b'"'),
                    b'\\' => output.push(b'\\'),
                    b'/' => output.push(b'/'),
                    b'b' => output.push(8),
                    b'f' => output.push(12),
                    b'n' => output.push(b'\n'),
                    b'r' => output.push(b'\r'),
                    b't' => output.push(b'\t'),
                    b'u' => {
                        let mut codepoint = self.hex4()? as u32;
                        if (0xD800..=0xDBFF).contains(&codepoint) {
                            self.expect(b'\\')?;
                            self.expect(b'u')?;
                            let low = self.hex4()? as u32;
                            if !(0xDC00..=0xDFFF).contains(&low) {
                                return Err(Error::new("JSON surrogate pair không hợp lệ"));
                            }
                            codepoint = 0x10000 + ((codepoint - 0xD800) << 10) + (low - 0xDC00);
                        }
                        let character = char::from_u32(codepoint)
                            .ok_or_else(|| Error::new("JSON unicode không hợp lệ"))?;
                        let mut buffer = [0u8; 4];
                        output.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                    }
                    _ => return Err(Error::new("JSON escape không hợp lệ")),
                },
                value if value < 0x20 => {
                    return Err(Error::new("JSON string có control character"))
                }
                value => output.push(value),
            }
        }
    }

    fn hex4(&mut self) -> Result<u16, Error> {
        let mut value = 0u16;
        for _ in 0..4 {
            value = (value << 4)
                | hex_nibble(
                    self.next()
                        .ok_or_else(|| Error::new("JSON unicode escape bị cắt"))?,
                )
                .ok_or_else(|| Error::new("JSON unicode escape không hợp lệ"))?
                    as u16;
        }
        Ok(value)
    }

    fn array(&mut self) -> Result<Value, Error> {
        self.expect(b'[')?;
        self.whitespace();
        let mut values = Vec::new();
        if self.peek() == Some(b']') {
            self.position += 1;
            return Ok(Value::Array(values));
        }
        loop {
            values.push(self.value()?);
            self.whitespace();
            match self.next() {
                Some(b',') => self.whitespace(),
                Some(b']') => return Ok(Value::Array(values)),
                _ => return Err(Error::new("JSON array không hợp lệ")),
            }
        }
    }

    fn object(&mut self) -> Result<Value, Error> {
        self.expect(b'{')?;
        self.whitespace();
        let mut values = BTreeMap::new();
        if self.peek() == Some(b'}') {
            self.position += 1;
            return Ok(Value::Object(values));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(Error::new("JSON object key phải là string"));
            }
            let key = self.string()?;
            self.whitespace();
            self.expect(b':')?;
            values.insert(key, self.value()?);
            self.whitespace();
            match self.next() {
                Some(b',') => self.whitespace(),
                Some(b'}') => return Ok(Value::Object(values)),
                _ => return Err(Error::new("JSON object không hợp lệ")),
            }
        }
    }

    fn number(&mut self) -> Result<Value, Error> {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        match self.peek() {
            Some(b'0') => self.position += 1,
            Some(b'1'..=b'9') => {
                self.position += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.position += 1;
                }
            }
            _ => return Err(Error::new("JSON number không hợp lệ")),
        }
        if self.peek() == Some(b'.') {
            self.position += 1;
            let before = self.position;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
            if before == self.position {
                return Err(Error::new("JSON fraction không hợp lệ"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            let before = self.position;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
            if before == self.position {
                return Err(Error::new("JSON exponent không hợp lệ"));
            }
        }
        let raw = std::str::from_utf8(&self.bytes[start..self.position])
            .map_err(|_| Error::new("JSON number không hợp lệ"))?;
        let value = raw
            .parse::<f64>()
            .map_err(|_| Error::new("JSON number không hợp lệ"))?;
        if !value.is_finite() {
            return Err(Error::new("JSON number vượt giới hạn"));
        }
        Ok(Value::Number(value))
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.position += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), Error> {
        if self.next() == Some(expected) {
            Ok(())
        } else {
            Err(Error::new("JSON token không hợp lệ"))
        }
    }
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }
    fn next(&mut self) -> Option<u8> {
        let value = self.peek()?;
        self.position += 1;
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_and_hmac_match_known_vectors() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&hmac_sha256(
                b"key",
                b"The quick brown fox jumps over the lazy dog"
            )),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn json_round_trip_unicode() {
        let value = Value::parse_json(r#"{"name":"MONA \uD83D\uDE80","amount":2500000,"ok":true}"#)
            .unwrap();
        assert_eq!(value.get("name").and_then(Value::as_str), Some("MONA 🚀"));
        assert_eq!(Value::parse_json(&value.to_json()).unwrap(), value);
    }

    fn hex(value: &[u8]) -> String {
        value.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
