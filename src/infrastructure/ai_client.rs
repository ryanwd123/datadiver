//! A blocking client for OpenAI-compatible Chat Completions endpoints. Run it
//! on the background executor.

use std::time::Duration;

use serde_json::{Value, json};
use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig};

use crate::domain::ai::{AiProvider, ChatMessage};

const TIMEOUT: Duration = Duration::from_secs(180);

fn agent() -> Agent {
    Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        // The system store also trusts company CAs, which gateways behind TLS
        // inspection need.
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into()
}

/// Sends `messages` and returns the reply text.
pub fn chat(provider: &AiProvider, messages: &[ChatMessage]) -> Result<String, String> {
    let url = provider.chat_url();
    let body = json!({
        "model": provider.model.trim(),
        "messages": messages,
        "stream": false,
    });
    let mut request = agent()
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json");
    if let Some(key) = provider.resolve_api_key()? {
        request = request.header("Authorization", format!("Bearer {key}"));
    }
    for header in &provider.headers {
        request = request.header(header.name.trim(), header.value.trim());
    }
    let response = request
        .send(body.to_string())
        .map_err(|error| format!("{url}: {error}"))?;
    let status = response.status();
    let text = response
        .into_body()
        .read_to_string()
        .map_err(|error| format!("{url}: reading the reply failed: {error}"))?;
    if !status.is_success() {
        return Err(format!("{url} returned {status}: {}", error_message(&text)));
    }
    reply_text(&text)
}

/// A one-line request to check the URL, key, headers, and model.
pub fn test_connection(provider: &AiProvider) -> Result<String, String> {
    let reply = chat(
        provider,
        &[ChatMessage::new("user", "Reply with the single word OK.")],
    )?;
    let reply = reply.trim();
    let reply = reply.chars().take(80).collect::<String>();
    Ok(format!("{} answered: {reply}", provider.model.trim()))
}

fn reply_text(text: &str) -> Result<String, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("reply is not JSON: {error}"))?;
    let content = &value["choices"][0]["message"]["content"];
    let reply = match content {
        Value::String(text) => text.clone(),
        // Some gateways send content as parts.
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    };
    if reply.trim().is_empty() {
        let reason = value["choices"][0]["finish_reason"]
            .as_str()
            .unwrap_or("none");
        return Err(format!(
            "the model sent an empty reply (finish reason: {reason})"
        ));
    }
    Ok(reply)
}

fn error_message(text: &str) -> String {
    let value = serde_json::from_str::<Value>(text).unwrap_or(Value::Null);
    let message = value["error"]["message"]
        .as_str()
        .or_else(|| value["error"].as_str())
        .or_else(|| value["message"].as_str())
        .or_else(|| value["detail"].as_str());
    let message = message.unwrap_or(text).trim();
    message.chars().take(500).collect()
}

/// A local stand-in for a provider, for tests.
#[cfg(test)]
pub(crate) mod test_server {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread;

    /// A Chat Completions body whose reply is `content`.
    pub fn reply(content: &str) -> String {
        serde_json::json!({"choices": [{"message": {"role": "assistant", "content": content}}]})
            .to_string()
    }

    /// Answers one request per `(status, body)`, in order, and returns the
    /// base URL and the raw requests.
    pub fn serve(
        responses: Vec<(&'static str, String)>,
    ) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut head = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    head.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut request_body = vec![0; length];
                reader.read_exact(&mut request_body).unwrap();
                let mut stream = reader.into_inner();
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                requests.push(head + &String::from_utf8(request_body).unwrap());
            }
            requests
        });
        (url, handle)
    }
}

#[cfg(test)]
mod tests {
    use super::test_server::{reply, serve};
    use super::*;
    use crate::domain::ai::AiHeader;
    use std::thread;

    fn serve_once(
        status: &'static str,
        body: &'static str,
    ) -> (String, thread::JoinHandle<String>) {
        let (url, server) = serve(vec![(status, body.to_owned())]);
        (url, thread::spawn(move || server.join().unwrap().remove(0)))
    }

    fn provider(base_url: String) -> AiProvider {
        AiProvider {
            name: "local".into(),
            base_url,
            api_key: "sk-test".into(),
            model: "gpt-6-sole".into(),
            headers: vec![AiHeader {
                name: "X-AI-Factory-Client".into(),
                value: "datadiver".into(),
            }],
            send_data: false,
        }
    }

    #[test]
    fn sends_key_headers_and_model_and_reads_the_reply() {
        let (url, server) = serve(vec![("200 OK", reply("```sql\nSELECT 1\n```"))]);
        let reply = chat(&provider(url), &[ChatMessage::new("user", "hi")]).unwrap();
        assert_eq!(reply, "```sql\nSELECT 1\n```");
        let request = server.join().unwrap().remove(0);
        let lower = request.to_ascii_lowercase();
        assert!(request.starts_with("POST /v1/chat/completions "));
        assert!(lower.contains("authorization: bearer sk-test"));
        assert!(lower.contains("x-ai-factory-client: datadiver"));
        assert!(request.contains(r#""model":"gpt-6-sole""#));
        assert!(request.contains(r#""content":"hi""#));
    }

    #[test]
    fn reports_the_server_error_message() {
        let (url, server) = serve_once(
            "401 Unauthorized",
            r#"{"error":{"message":"Invalid API key"}}"#,
        );
        let error = chat(&provider(url), &[ChatMessage::new("user", "hi")]).unwrap_err();
        server.join().unwrap();
        assert!(error.contains("401"), "{error}");
        assert!(error.contains("Invalid API key"), "{error}");
    }

    #[test]
    fn reads_content_parts_and_rejects_empty_replies() {
        assert_eq!(
            reply_text(
                r#"{"choices":[{"message":{"content":[{"type":"text","text":"SELECT "},{"type":"text","text":"2"}]}}]}"#
            ),
            Ok("SELECT 2".into())
        );
        assert!(
            reply_text(r#"{"choices":[{"message":{"content":null},"finish_reason":"length"}]}"#)
                .unwrap_err()
                .contains("length")
        );
    }
}
