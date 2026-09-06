//! Bounded stdio framing and MCP request lifecycle.
use super::*;

pub(super) async fn serve_stdio<R, W>(
    server: Server,
    mut reader: R,
    stdout: &mut W,
) -> Result<(), String>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut initialized = false;
    while let Some(frame) = read_frame(&mut reader, MAX_REQUEST_BYTES).await? {
        let request = match frame.and_then(parse_request) {
            Ok(request) => request,
            Err((code, message)) => {
                write_response(
                    stdout,
                    &json!({"jsonrpc":"2.0","id":null,"error":{"code":code,"message":message}}),
                )
                .await?;
                continue;
            }
        };
        let id = request.get("id").cloned();
        let method = request["method"].as_str().expect("validated method");
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        // Reserve the largest recovery envelope before a tool can persist a
        // batch. Batch IDs are generated locally as bridge-<UUID> (42 bytes).
        if id.as_ref().is_some_and(|id| {
            recovery_error(
                id.clone(),
                Some("bridge-00000000-0000-0000-0000-000000000000"),
                "import_publication_recovery_required",
            )
            .to_string()
            .len()
                + 1
                > server.settings.max_bytes
        }) {
            write_response(stdout, &json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"request_id_too_large"}})).await?;
            continue;
        }
        if id.is_none() && method == "tools/call" {
            let tool = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            server.append_notification_refusal_egress(tool, &arguments)?;
            continue;
        }
        let mut egress = None;
        let mut recovery_batch_id = None;
        let result = match method {
            "initialize" if initialized => Err("already_initialized".to_string()),
            "initialize" if id.is_none() => continue,
            "initialize" => params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .ok_or_else(|| "initialize_protocol_version_required".to_string())
                .and_then(negotiate_protocol)
                .map(|protocol_version| {
                    initialized = true;
                    json!({"protocolVersion": protocol_version, "capabilities": {"tools": {}}, "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION}})
                }),
            "notifications/initialized" => {
                if id.is_none() {
                    continue;
                }
                Ok(json!({}))
            }
            "ping" => Ok(json!({})),
            _ if !initialized => Err("initialize_required".to_string()),
            "tools/list" => Ok(json!({"tools": tool_definitions(server.settings.import_enabled)})),
            "tools/call" => match params.get("name").and_then(Value::as_str) {
                Some(name) => {
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if registered_tool_definitions(true).as_array().is_some_and(|tools| tools.iter().any(|tool| tool["name"] == name)) {
                    let tool_response = server.call_tool_response(name, arguments).await;
                    recovery_batch_id = tool_response.recovery_batch_id;
                    egress = Some(tool_response.egress);
                    Ok(tool_response.value)
                } else {
                    egress = Some(EgressContext {
                        tool: name.to_string(),
                        args_sha256: sha256_json(&arguments),
                        company_guid: None,
                    });
                    Err("tool_not_found".to_string())
                }
                }
                None => Err("tool_name_required".to_string()),
            },
            _ => Err("method_not_found".to_string()),
        };
        if let Some(id) = id {
            finish_response(
                &server,
                stdout,
                id,
                result,
                egress,
                recovery_batch_id,
                method == "tools/call",
            )
            .await?;
        }
    }
    Ok(())
}

// The same final framing path handles success, cap replacements and durable
// batch recovery after receipt failure. Tests exercise this with persisted builds.
pub(super) async fn finish_response<W: AsyncWrite + Unpin>(
    server: &Server,
    stdout: &mut W,
    id: Value,
    result: Result<Value, String>,
    egress: Option<EgressContext>,
    recovery_batch_id: Option<String>,
    is_tool: bool,
) -> Result<(), String> {
    let mut response = match result {
        Ok(result) => json!({"jsonrpc":"2.0","id":id.clone(),"result":result}),
        Err(code) => {
            let error_code = if code == "method_not_found" {
                -32601
            } else {
                -32602
            };
            json!({"jsonrpc":"2.0","id":id.clone(),"error":{"code":error_code,"message":code}})
        }
    };
    if recovery_batch_id.is_some() && response["result"]["isError"] == true {
        let code = response["result"]["structuredContent"]["result"]["error"]["code"]
            .as_str()
            .or_else(|| response["result"]["structuredContent"]["error"]["code"].as_str())
            .unwrap_or("import_publication_recovery_required");
        response = recovery_error(id.clone(), recovery_batch_id.as_deref(), code);
    }
    if is_tool
        && enforce_jsonrpc_response_byte_cap(&mut response, server.settings.max_bytes).is_err()
    {
        response = recovery_error(
            id.clone(),
            recovery_batch_id.as_deref(),
            "agent_response_too_large",
        );
    }
    let mut serialized_response = format!("{response}\n");
    let mut terminal_egress_error = None;
    let mut prepared_receipt = None;
    if let Some(egress) = egress {
        match server.append_framed_egress(egress, &response, &serialized_response) {
            Ok(receipt) => prepared_receipt = Some(receipt),
            Err(error) => {
                if matches!(
                    error.as_str(),
                    "egress_record_rollback_failed" | "egress_log_incomplete"
                ) {
                    terminal_egress_error = Some(error.clone());
                }
                if recovery_batch_id.is_some() {
                    response = recovery_error(id, recovery_batch_id.as_deref(), &error);
                } else if !attach_build_egress_failure(&mut response) {
                    return Err(error);
                }
                enforce_jsonrpc_response_byte_cap(&mut response, server.settings.max_bytes)?;
                serialized_response = format!("{response}\n");
            }
        }
    }
    stdout
        .write_all(serialized_response.as_bytes())
        .await
        .map_err(|_| "stdio_write_failed".to_string())?;
    stdout
        .flush()
        .await
        .map_err(|_| "stdio_flush_failed".to_string())?;
    if let Some(receipt) = prepared_receipt {
        // A missing completion is deliberately ambiguous: output may have been
        // partial, fully written, or consumed before recording failed. Stop on
        // append failure rather than dispatch another request without audit.
        server.append_stdio_write_completed(receipt)?;
    }
    match terminal_egress_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn recovery_error(id: Value, batch_id: Option<&str>, message: &str) -> Value {
    let mut response = json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":message}});
    if let Some(batch_id) = batch_id {
        response["error"]["data"] = json!({"batch_id":batch_id});
    }
    response
}

// Independently bounds caller-controlled memory; response caps cannot bound stdin.
const MAX_REQUEST_BYTES: usize = 5_000_000;
type Frame = Result<String, (i32, &'static str)>;

async fn read_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    cap: usize,
) -> Result<Option<Frame>, String> {
    let mut bytes = Vec::new();
    let mut oversized = false;
    loop {
        let chunk = reader
            .fill_buf()
            .await
            .map_err(|_| "stdio_read_failed".to_string())?;
        if chunk.is_empty() {
            return if bytes.is_empty() && !oversized {
                Ok(None)
            } else {
                Ok(Some(Err((-32700, "Incomplete message"))))
            };
        }
        let newline = chunk.iter().position(|byte| *byte == b'\n');
        let length = newline.map_or(chunk.len(), |index| index + 1);
        if !oversized && bytes.len().saturating_add(length) <= cap {
            bytes.extend_from_slice(&chunk[..length]);
        } else {
            oversized = true;
            bytes.clear();
        }
        reader.consume(length);
        if newline.is_some() {
            return Ok(Some(if oversized {
                Err((-32600, "request_too_large"))
            } else {
                String::from_utf8(bytes).map_err(|_| (-32700, "Parse error"))
            }));
        }
    }
}

fn parse_request(line: String) -> Result<Value, (i32, &'static str)> {
    let value: Value = serde_json::from_str(&line).map_err(|_| (-32700, "Parse error"))?;
    let valid = value.is_object()
        && value["jsonrpc"] == "2.0"
        && value["method"].is_string()
        && value.get("params").is_none_or(Value::is_object)
        && value
            .get("id")
            .is_none_or(|id| id.is_string() || id.is_i64() || id.is_u64());
    if valid {
        Ok(value)
    } else {
        Err((-32600, "Invalid Request"))
    }
}

async fn write_response<W: AsyncWrite + Unpin>(
    stdout: &mut W,
    response: &Value,
) -> Result<(), String> {
    stdout
        .write_all(format!("{response}\n").as_bytes())
        .await
        .map_err(|_| "stdio_write_failed".to_string())?;
    stdout
        .flush()
        .await
        .map_err(|_| "stdio_flush_failed".to_string())
}

#[cfg(test)]
#[path = "agent_protocol_tests.rs"]
mod tests;
