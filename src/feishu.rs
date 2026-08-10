use anyhow::{Context, Result, bail};
use reqwest::blocking::{Client, Response};
use serde::{Deserialize, Serialize};
use std::thread;
use std::time::Duration;

use crate::card::{RenderedCard, render};
use crate::model::Notification;
use crate::settings::FeishuConfig;

const FEISHU_API_BASE: &str = "https://open.feishu.cn/open-apis";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const RETRY_DELAY: Duration = Duration::from_millis(300);

#[derive(Debug, Clone)]
pub struct FeishuClient {
    client: Client,
    api_base: String,
}

#[derive(Debug, Clone)]
pub struct DeliveryReceipt {
    pub message_id: Option<String>,
    pub card: RenderedCard,
}

impl FeishuClient {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("无法创建飞书网络客户端")?;
        Ok(Self {
            client,
            api_base: FEISHU_API_BASE.to_owned(),
        })
    }

    #[cfg(test)]
    fn with_api_base(api_base: impl Into<String>) -> Self {
        Self {
            client: Client::builder().build().expect("HTTP client"),
            api_base: api_base.into(),
        }
    }

    pub fn verify_credentials(&self, config: &FeishuConfig) -> Result<()> {
        self.tenant_access_token(config).map(|_| ())
    }

    pub fn send(
        &self,
        config: &FeishuConfig,
        notification: &Notification,
    ) -> Result<DeliveryReceipt> {
        let token = self.tenant_access_token(config)?;
        let card = render(notification);
        let endpoint = format!(
            "{}/im/v1/messages?receive_id_type={}",
            self.api_base,
            config.receiver_id_type.as_api_value()
        );
        let payload = SendMessageRequest {
            receive_id: &config.receiver_id,
            msg_type: "interactive",
            content: &card.serialized_content,
        };

        let response = self.send_with_retry(|| {
            self.client
                .post(&endpoint)
                .bearer_auth(&token)
                .json(&payload)
                .send()
        })?;
        let response: FeishuResponse<MessageData> = parse_response(response, "发送消息")?;
        ensure_success(&response, "发送消息")?;

        Ok(DeliveryReceipt {
            message_id: response.data.and_then(|data| data.message_id),
            card,
        })
    }

    fn tenant_access_token(&self, config: &FeishuConfig) -> Result<String> {
        let endpoint = format!("{}/auth/v3/tenant_access_token/internal", self.api_base);
        let payload = TenantTokenRequest {
            app_id: &config.app_id,
            app_secret: &config.app_secret,
        };
        let response =
            self.send_with_retry(|| self.client.post(&endpoint).json(&payload).send())?;
        let response: FeishuResponse<serde_json::Value> =
            parse_response(response, "获取 tenant access token")?;
        ensure_success(&response, "获取 tenant access token")?;
        response
            .tenant_access_token
            .filter(|token| !token.trim().is_empty())
            .context("飞书没有返回 tenant access token")
    }

    fn send_with_retry<F>(&self, mut send: F) -> Result<Response>
    where
        F: FnMut() -> reqwest::Result<Response>,
    {
        let mut last_error = None;
        for attempt in 0..2 {
            match send() {
                Ok(response) if response.status().is_server_error() && attempt == 0 => {
                    thread::sleep(RETRY_DELAY);
                }
                Ok(response) => return Ok(response),
                Err(error) if (error.is_timeout() || error.is_connect()) && attempt == 0 => {
                    last_error = Some(error);
                    thread::sleep(RETRY_DELAY);
                }
                Err(error) => return Err(error).context("无法连接飞书，请检查网络后重试"),
            }
        }

        Err(last_error.context("飞书请求没有返回响应")?.into())
    }
}

#[derive(Debug, Serialize)]
struct TenantTokenRequest<'a> {
    app_id: &'a str,
    app_secret: &'a str,
}

#[derive(Debug, Serialize)]
struct SendMessageRequest<'a> {
    receive_id: &'a str,
    msg_type: &'static str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct FeishuResponse<T> {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    msg: String,
    tenant_access_token: Option<String>,
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct MessageData {
    #[serde(default)]
    message_id: Option<String>,
}

fn parse_response<T>(response: Response, operation: &str) -> Result<FeishuResponse<T>>
where
    T: for<'de> Deserialize<'de>,
{
    let status = response.status();
    let body = response
        .text()
        .with_context(|| format!("飞书{operation}后无法读取响应"))?;
    let parsed: FeishuResponse<T> = serde_json::from_str(&body)
        .with_context(|| format!("飞书{operation}时返回了无法解析的响应"))?;

    if !status.is_success() {
        bail!(
            "飞书{operation}失败：HTTP {}，错误码 {}，{}",
            status.as_u16(),
            parsed.code,
            safe_message(&parsed.msg)
        );
    }
    Ok(parsed)
}

fn ensure_success<T>(response: &FeishuResponse<T>, operation: &str) -> Result<()> {
    if response.code != 0 {
        bail!(
            "飞书{operation}失败：错误码 {}，{}",
            response.code,
            safe_message(&response.msg)
        );
    }
    Ok(())
}

fn safe_message(value: &str) -> String {
    value
        .chars()
        .take(500)
        .collect::<String>()
        .replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::{FeishuClient, safe_message};
    use crate::model::Notification;
    use crate::settings::{FeishuConfig, ReceiverIdType};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn error_message_is_short_and_single_line() {
        let message = format!("{}\nsecret-looking-detail", "a".repeat(600));
        let safe = safe_message(&message);
        assert!(safe.len() <= 500);
        assert!(!safe.contains('\n'));
    }

    #[test]
    fn test_client_can_use_a_local_endpoint() {
        let client = FeishuClient::with_api_base("http://127.0.0.1:12345");
        assert_eq!(client.api_base, "http://127.0.0.1:12345");
    }

    #[test]
    fn sends_a_card_after_requesting_a_tenant_token() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listen");
        let address = listener.local_addr().expect("listener address");
        let server = thread::spawn(move || {
            let (mut token_stream, _) = listener.accept().expect("token connection");
            let token_request = read_http_request(&mut token_stream);
            assert!(token_request.starts_with("POST /auth/v3/tenant_access_token/internal "));
            assert!(token_request.contains("\"app_id\":\"cli_test\""));
            assert!(token_request.contains("\"app_secret\":\"secret_test\""));
            write_http_response(
                &mut token_stream,
                r#"{"code":0,"msg":"ok","tenant_access_token":"tenant-test"}"#,
            );

            let (mut message_stream, _) = listener.accept().expect("message connection");
            let message_request = read_http_request(&mut message_stream);
            assert!(message_request.starts_with("POST /im/v1/messages?receive_id_type=open_id "));
            assert!(
                message_request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer tenant-test")
            );
            let body = message_request
                .split("\r\n\r\n")
                .nth(1)
                .expect("message body");
            let request_body: serde_json::Value =
                serde_json::from_str(body).expect("message request JSON");
            assert_eq!(request_body["receive_id"], "ou_test");
            assert_eq!(request_body["msg_type"], "interactive");
            let card: serde_json::Value =
                serde_json::from_str(request_body["content"].as_str().expect("card content"))
                    .expect("card JSON");
            let title = card["header"]["title"]["content"]
                .as_str()
                .expect("card title");
            assert!(title.starts_with("\u{2705} "));
            assert!(title.ends_with(" Test conversation"));
            write_http_response(
                &mut message_stream,
                r#"{"code":0,"msg":"ok","data":{"message_id":"om_test"}}"#,
            );
        });

        let client = FeishuClient::with_api_base(format!("http://{address}"));
        let config = FeishuConfig {
            app_id: "cli_test".to_owned(),
            app_secret: "secret_test".to_owned(),
            receiver_id_type: ReceiverIdType::OpenId,
            receiver_id: "ou_test".to_owned(),
        };
        let receipt = client
            .send(
                &config,
                &Notification::completed(
                    "Test conversation",
                    "Test task",
                    "Test result",
                    Some(Duration::from_secs(3)),
                    "turn-test",
                ),
            )
            .expect("send Feishu card");
        assert_eq!(receipt.message_id.as_deref(), Some("om_test"));
        server.join().expect("server thread");
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4_096];
        loop {
            let read = stream.read(&mut buffer).expect("read request");
            assert!(read > 0, "connection closed before request completed");
            bytes.extend_from_slice(&buffer[..read]);
            let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let headers = std::str::from_utf8(&bytes[..header_end]).expect("UTF-8 headers");
            let content_length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .or_else(|| {
                    headers
                        .lines()
                        .find_map(|line| line.strip_prefix("Content-Length:"))
                })
                .map(|value| value.trim().parse::<usize>().expect("content length"))
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + content_length {
                return String::from_utf8(bytes).expect("UTF-8 request");
            }
        }
    }

    fn write_http_response(stream: &mut TcpStream, body: &str) {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response");
        stream.flush().expect("flush response");
    }
}
