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
    let mut framer = Framer::default();
    let mut pending = std::collections::VecDeque::new();
    loop {
        let frame = match pending.pop_front() {
            Some(frame) => frame,
            None => match framer.read(&mut reader, MAX_REQUEST_BYTES).await? {
                Some(frame) => frame,
                None => break,
            },
        };
        let request = match frame.and_then(parse_request) {
            Ok(request) => request,
            Err((code, message)) => {
                write_response(
                    stdout,
                    &json!({"jsonrpc":"2.0","id":null,"error":{"code":code,"message":message}}),
                    server.settings.max_bytes,
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
        if id
            .as_ref()
            .is_some_and(|id| !request_id_fits_response_cap(id, server.settings.max_bytes))
        {
            write_response(stdout, &json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"request_id_too_large"}}), server.settings.max_bytes).await?;
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
            "tools/list" => Ok(json!({"tools": catalog::tool_definitions(server.settings.import_enabled, server.settings.writes_enabled)})),
            "tools/call" => match params.get("name").and_then(Value::as_str) {
                Some(name) => {
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if catalog::registered_tool_definitions(true, true).as_array().is_some_and(|tools| tools.iter().any(|tool| tool["name"] == name)) {
                    let response = if name == "post_import" {
                        await_post(
                            server.call_tool_response(name, arguments.clone()),
                            PostRequest {
                                id: id.as_ref().expect("tool requests have IDs"),
                                args: &arguments,
                            },
                            &server,
                            &mut reader, &mut framer, &mut pending,
                            stdout,
                        ).await?
                    } else { Some(server.call_tool_response(name, arguments.clone()).await) };
                    match response {
                        Some(tool_response) => {
                            recovery_batch_id = tool_response.recovery_batch_id;
                            egress = Some(tool_response.egress);
                            Ok(tool_response.value)
                        }
                        None => {
                            let cancelled = server.finish_tool_response(name, &arguments, Utc::now(),
                                server.cancelled_import(&arguments));
                            recovery_batch_id = cancelled.recovery_batch_id;
                            egress = Some(cancelled.egress);
                            Ok(cancelled.value)
                        }
                    }
                } else {
                    egress = Some(EgressContext {
                        evidence: None,
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
    mut egress: Option<EgressContext>,
    recovery_batch_id: Option<String>,
    is_tool: bool,
) -> Result<(), String> {
    let mut pending_evidence = egress.as_mut().and_then(|context| context.evidence.take());
    let outcome: Result<(), String> = async {
        let mut response = match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id.clone(),"result":result}),
            Err(code) => {
                let error_code = if code == "request_cancelled" {
                    -32800
                } else if code == "method_not_found" {
                    -32601
                } else {
                    -32602
                };
                json!({"jsonrpc":"2.0","id":id.clone(),"error":{"code":error_code,"message":code}})
            }
        };
        // A reconciliation result is an intentional, structured tool payload:
        // callers need its saved batch ID, counters, readback state and evidence.
        // Only replace cap-stage errors, whose structured content has no result.
        if recovery_batch_id.is_some()
            && response["result"]["isError"] == true
            && response["result"]["structuredContent"]["result"]["error"].is_null()
        {
            let code = response["result"]["structuredContent"]["result"]["error"]["code"]
                .as_str()
                .or_else(|| response["result"]["structuredContent"]["error"]["code"].as_str())
                .unwrap_or("import_publication_recovery_required");
            response = recovery_error(id.clone(), recovery_batch_id.as_deref(), code);
        }
        // Tools may reduce an explicitly paged result. Control messages (including
        // the tool catalogue) must either fit intact or return a bounded refusal.
        let recovery_code = recovery_batch_id.as_ref().and_then(|_| {
            response["result"]["structuredContent"]["result"]["error"]["code"]
                .as_str()
                .map(str::to_string)
        });
        let fits = if is_tool {
            enforce_jsonrpc_response_byte_cap(&mut response, server.settings.max_bytes).is_ok()
        } else {
            response.to_string().len() < server.settings.max_bytes
        };
        if !fits {
            response = recovery_error(
                id.clone(),
                recovery_batch_id.as_deref(),
                recovery_code
                    .as_deref()
                    .unwrap_or("agent_response_too_large"),
            );
        }
        let mut serialized_response = serialize_response(&response, server.settings.max_bytes)?;
        let mut terminal_egress_error = None;
        let mut prepared_receipt = None;
        if let Some(egress) = egress {
            match server.append_framed_egress(egress, &response, &serialized_response) {
                Ok(receipt) => prepared_receipt = Some(receipt),
                Err(error) => {
                    if let Some(evidence) = pending_evidence.as_mut() {
                        evidence.state = "partial";
                        evidence.reason_code = Some(error.clone());
                    }
                    if matches!(
                        error.as_str(),
                        "egress_record_rollback_failed" | "egress_log_incomplete"
                    ) {
                        terminal_egress_error = Some(error.clone());
                    }
                    if recovery_batch_id.is_some() {
                        response = recovery_error_with_dispatch(
                            id,
                            recovery_batch_id.as_deref(),
                            &error,
                            &response,
                            server.settings.max_bytes,
                        );
                    } else if !attach_build_egress_failure(&mut response) {
                        return Err(error);
                    }
                    enforce_jsonrpc_response_byte_cap(&mut response, server.settings.max_bytes)?;
                    serialized_response = serialize_response(&response, server.settings.max_bytes)?;
                }
            }
        }
        if let Some(mut evidence) = pending_evidence.take() {
            if let Some(code) = response["error"]["message"].as_str() {
                evidence.state = "partial";
                evidence.reason_code = Some(code.to_string());
            }
            server.record_evidence(evidence);
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
    .await;
    // Serialization or receipt failure can withhold the frame entirely. Keep
    // its completed source commitments, but do not leave a complete claim.
    if let Some(mut evidence) = pending_evidence {
        evidence.state = "partial";
        evidence.reason_code = outcome.as_ref().err().cloned();
        server.record_evidence(evidence);
    }
    outcome
}

fn recovery_error(id: Value, batch_id: Option<&str>, message: &str) -> Value {
    let mut response = json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":message}});
    if let Some(batch_id) = batch_id {
        response["error"]["data"] = json!({"batch_id":batch_id});
    }
    response
}

fn recovery_error_with_dispatch(
    id: Value,
    batch_id: Option<&str>,
    message: &str,
    original: &Value,
    max_bytes: usize,
) -> Value {
    let fallback = recovery_error(id.clone(), batch_id, message);
    let mut response = fallback.clone();
    let result = &original["result"]["structuredContent"]["result"];
    let mut data = response["error"]["data"].take();
    if let Some(attempted) = result["attempt_recorded"].as_bool() {
        data["attempt_recorded"] = json!(attempted);
    }
    if let Some(dispatch) = compact_dispatch_response(&result["dispatch_response"]) {
        data["dispatch"] = json!({"state":"reconciliation_required","resent":false});
        data["dispatch_response"] = dispatch;
    }
    response["error"]["data"] = data;
    if response.to_string().len() < max_bytes {
        response
    } else {
        fallback
    }
}

pub(super) fn compact_dispatch_response(response: &Value) -> Option<Value> {
    let request_sha256 = response["request_sha256"].as_str()?;
    let response_sha256 = response["response_sha256"].as_str()?;
    if !is_sha256(request_sha256) || !is_sha256(response_sha256) {
        return None;
    }
    let bytes = response["bytes"].as_u64()?;
    let outcome = &response["outcome"];
    let outcome = compact_dispatch_outcome(outcome);
    Some(json!({
        "request_sha256":request_sha256,
        "response_sha256":response_sha256,
        "bytes":bytes,
        "outcome":outcome,
    }))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn compact_dispatch_outcome(outcome: &Value) -> Option<Value> {
    let application_status = outcome["application_status"].as_str()?;
    if !matches!(application_status, "success" | "failure" | "not_reported") {
        return None;
    }
    let counters = outcome["counters"].as_object()?;
    let counter = |name| counters.get(name).and_then(Value::as_u64);
    Some(json!({
        "application_status":application_status,
        "counters":{
            "created":counter("created")?,
            "altered":counter("altered")?,
            "deleted":counter("deleted")?,
            "ignored":counter("ignored")?,
            "errors":counter("errors")?,
            "cancelled":counter("cancelled")?,
            "exceptions":counter("exceptions")?,
            "line_error_count":counter("line_error_count")?
        },
        "exceptions_were_reported":outcome["exceptions_were_reported"].as_bool()?
    }))
}

fn request_id_fits_response_cap(id: &Value, max_bytes: usize) -> bool {
    recovery_error(
        id.clone(),
        Some("bridge-00000000-0000-0000-0000-000000000000"),
        "import_publication_recovery_required",
    )
    .to_string()
    .len()
        < max_bytes
}

// Independently bounds caller-controlled memory; response caps cannot bound stdin.
const MAX_REQUEST_BYTES: usize = 5_000_000;
type Frame = Result<String, (i32, &'static str)>;

// State survives a cancelled read future when a write finishes between frame
// fragments. Dropping a local Vec here would corrupt the next MCP request.
#[derive(Default)]
struct Framer {
    bytes: Vec<u8>,
    oversized: bool,
}

impl Framer {
    async fn read<R: AsyncBufRead + Unpin>(
        &mut self,
        reader: &mut R,
        cap: usize,
    ) -> Result<Option<Frame>, String> {
        loop {
            let chunk = reader.fill_buf().await.map_err(|_| "stdio_read_failed")?;
            if chunk.is_empty() {
                let incomplete = !self.bytes.is_empty() || self.oversized;
                self.bytes.clear();
                self.oversized = false;
                return Ok(incomplete.then_some(Err((-32700, "Incomplete message"))));
            }
            let newline = chunk.iter().position(|byte| *byte == b'\n');
            let length = newline.map_or(chunk.len(), |index| index + 1);
            if !self.oversized && self.bytes.len().saturating_add(length) <= cap {
                self.bytes.extend_from_slice(&chunk[..length]);
            } else {
                self.oversized = true;
                self.bytes.clear();
            }
            reader.consume(length);
            if newline.is_some() {
                let oversized = std::mem::take(&mut self.oversized);
                let bytes = std::mem::take(&mut self.bytes);
                return Ok(Some(if oversized {
                    Err((-32600, "request_too_large"))
                } else {
                    String::from_utf8(bytes).map_err(|_| (-32700, "Parse error"))
                }));
            }
        }
    }
}

struct PostRequest<'a> {
    id: &'a Value,
    args: &'a Value,
}

// Keep receiving cancellation and disconnect while a native approval or write
// is pending. A durable dispatch intent makes cancellation observational: the
// original future must finish its response journal and readback before we drop
// it. Before an intent, cancellation remains prompt and does not start a post.
async fn await_post<R, W, F>(
    future: F,
    request: PostRequest<'_>,
    server: &Server,
    reader: &mut R,
    framer: &mut Framer,
    pending: &mut std::collections::VecDeque<Frame>,
    stdout: &mut W,
) -> Result<Option<ToolResponse>, String>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    F: std::future::Future<Output = ToolResponse>,
{
    tokio::pin!(future);
    let mut phase = PostPhase::Running;
    let mut classifier_retry = tokio::time::interval(std::time::Duration::from_millis(10));
    loop {
        tokio::select! {
            biased;
            // Do not let a continuously readable stdin starve the pending
            // approval/write future. Cancellation remains visible on every turn.
            frame = framer.read(reader, MAX_REQUEST_BYTES) => {
                let frame = match frame {
                    Ok(Some(frame)) => frame,
                    Ok(None) => return finish_interrupted_post(
                        future.as_mut(),
                        request,
                        server,
                        Some("stdio_client_disconnected".into()),
                        phase == PostPhase::Draining,
                    )
                    .await,
                    Err(error) => return finish_interrupted_post(future.as_mut(), request, server, Some(error), phase == PostPhase::Draining).await,
                };
                if let Some(target) = cancellation_target(&frame) {
                    if target == *request.id {
                        if phase == PostPhase::Draining {
                            continue;
                        }
                        match post_dispatch_state(server, request.args) {
                            PostDispatchState::NotDispatched => return Ok(None),
                            PostDispatchState::MayHaveDispatched if phase == PostPhase::Running => {
                                phase = PostPhase::Draining;
                            }
                            PostDispatchState::AdmissionBusy => {
                                if phase == PostPhase::Running {
                                    phase = PostPhase::Classifying;
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if let Err(error) = cancel_queued_request(server, stdout, pending, &target).await {
                        return finish_interrupted_post(future.as_mut(), request, server, Some(error), phase == PostPhase::Draining).await;
                    }
                    continue;
                }
                if let Some(ping) = frame.as_ref().ok()
                    .and_then(|text| parse_request(text.clone()).ok())
                    .filter(|request| request["method"] == "ping")
                {
                    if let Some(ping_id) = ping.get("id") {
                        let result = if request_id_fits_response_cap(ping_id, server.settings.max_bytes) {
                            finish_response(server, stdout, ping_id.clone(), Ok(json!({})), None, None, false).await
                        } else {
                            refuse_pending_frame(server, stdout, frame).await
                        };
                        if let Err(error) = result {
                            return finish_interrupted_post(future.as_mut(), request, server, Some(error), phase == PostPhase::Draining).await;
                        }
                    }
                    continue;
                }
                let queued_bytes: usize = pending.iter().filter_map(|frame| frame.as_ref().ok()).map(String::len).sum();
                let size = frame.as_ref().map_or(0, String::len);
                if pending.len() >= 8 || queued_bytes.saturating_add(size) > MAX_REQUEST_BYTES {
                    if let Err(error) = refuse_pending_frame(server, stdout, frame).await {
                        return finish_interrupted_post(future.as_mut(), request, server, Some(error), phase == PostPhase::Draining).await;
                    }
                    continue;
                }
                pending.push_back(frame);
            }
            // Keep servicing framed input while a concurrent admission holds
            // the durable snapshot lock. In particular, a ping must not wait
            // for the builder's Tally reads to finish.
            _ = classifier_retry.tick(), if phase == PostPhase::Classifying => {
                match post_dispatch_state(server, request.args) {
                    PostDispatchState::NotDispatched => return Ok(None),
                    PostDispatchState::MayHaveDispatched => phase = PostPhase::Draining,
                    PostDispatchState::AdmissionBusy => {}
                }
            }
            response = &mut future, if phase != PostPhase::Classifying => return Ok(Some(response)),
        }
    }
}

/// A malformed request cannot have reached `post_import`; all valid requests
/// fail closed. Missing or unreadable history is an uncertain durable boundary
/// and therefore drains the existing future instead of dropping it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PostDispatchState {
    NotDispatched,
    MayHaveDispatched,
    AdmissionBusy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PostPhase {
    Running,
    Classifying,
    Draining,
}

fn post_dispatch_state(server: &Server, args: &Value) -> PostDispatchState {
    match server.cancelled_import(args) {
        Ok(outcome) if outcome.payload["result"]["attempt_recorded"] == Value::Bool(false) => {
            PostDispatchState::NotDispatched
        }
        Ok(_) => PostDispatchState::MayHaveDispatched,
        Err(ToolFailure { code, .. }) if code == "import_admission_busy" => {
            PostDispatchState::AdmissionBusy
        }
        // Invalid cancellation arguments cannot name this post. The normal
        // request path therefore remains pre-intent and promptly cancellable.
        Err(_) => PostDispatchState::NotDispatched,
    }
}

async fn post_may_have_dispatched(server: &Server, args: &Value) -> bool {
    loop {
        match post_dispatch_state(server, args) {
            PostDispatchState::NotDispatched => return false,
            // A concurrent build holds this lock across source reads. Keep the
            // post future suspended until its durable phase can be observed;
            // otherwise it could record a new intent after cancellation.
            PostDispatchState::AdmissionBusy => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            PostDispatchState::MayHaveDispatched => return true,
        }
    }
}

async fn finish_interrupted_post<F>(
    mut future: std::pin::Pin<&mut F>,
    request: PostRequest<'_>,
    server: &Server,
    interruption: Option<String>,
    known_dispatched: bool,
) -> Result<Option<ToolResponse>, String>
where
    F: std::future::Future<Output = ToolResponse>,
{
    if known_dispatched || post_may_have_dispatched(server, request.args).await {
        // The caller may be gone, but the original execution owns the only
        // response wire. Let the ordinary response path publish it if stdout
        // remains usable; no replacement post is created.
        return Ok(Some(future.as_mut().await));
    }
    match interruption {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

fn cancellation_target(frame: &Frame) -> Option<Value> {
    let request = parse_request(frame.as_ref().ok()?.clone()).ok()?;
    if request.get("id").is_none() && request["method"] == "notifications/cancelled" {
        request["params"].get("requestId").cloned()
    } else {
        None
    }
}

// Consume a queued cancellation while the current request is still pending.
// Leaving the notification behind its request would start that request first.
async fn cancel_queued_request<W: AsyncWrite + Unpin>(
    server: &Server,
    stdout: &mut W,
    pending: &mut std::collections::VecDeque<Frame>,
    target: &Value,
) -> Result<(), String> {
    let position = pending.iter().position(|frame| {
        frame
            .as_ref()
            .ok()
            .and_then(|text| parse_request(text.clone()).ok())
            .is_some_and(|request| request.get("id") == Some(target))
    });
    let Some(position) = position else {
        return Ok(());
    };
    let frame = pending.remove(position).expect("located pending frame");
    if !request_id_fits_response_cap(target, server.settings.max_bytes) {
        return refuse_pending_frame(server, stdout, frame).await;
    }
    let Ok(request) = frame.and_then(parse_request) else {
        return Ok(());
    };
    let is_tool = request["method"] == "tools/call";
    let args = request["params"]
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let name = request["params"]["name"].as_str().unwrap_or("unknown");
    if is_tool && name == "post_import" {
        let response =
            server.finish_tool_response(name, &args, Utc::now(), server.cancelled_import(&args));
        return finish_response(
            server,
            stdout,
            target.clone(),
            Ok(response.value),
            Some(response.egress),
            response.recovery_batch_id,
            true,
        )
        .await;
    }
    let egress = is_tool.then(|| EgressContext {
        evidence: None,
        tool: name.into(),
        args_sha256: sha256_json(&args),
        company_guid: args
            .get("company_guid")
            .and_then(Value::as_str)
            .map(str::to_string),
    });
    finish_response(
        server,
        stdout,
        target.clone(),
        Err("request_cancelled".into()),
        egress,
        None,
        is_tool,
    )
    .await
}

async fn refuse_pending_frame<W: AsyncWrite + Unpin>(
    server: &Server,
    stdout: &mut W,
    frame: Frame,
) -> Result<(), String> {
    let request = match frame {
        Ok(frame) => match parse_request(frame) {
            Ok(request) => request,
            Err((code, message)) => {
                return write_response(
                    stdout,
                    &json!({"jsonrpc":"2.0","id":null,"error":{"code":code,"message":message}}),
                    server.settings.max_bytes,
                )
                .await;
            }
        },
        Err((code, message)) => {
            return write_response(
                stdout,
                &json!({"jsonrpc":"2.0","id":null,"error":{"code":code,"message":message}}),
                server.settings.max_bytes,
            )
            .await;
        }
    };
    let Some(id) = request.get("id").cloned() else {
        if request["method"] == "tools/call" {
            let tool = request["params"]["name"].as_str().unwrap_or("unknown");
            let arguments = request["params"]
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            server.append_notification_refusal_egress(tool, &arguments)?;
        }
        return Ok(());
    };
    let response = if request_id_fits_response_cap(&id, server.settings.max_bytes) {
        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"stdio_pending_requests_exceeded"}})
    } else {
        json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"request_id_too_large"}})
    };
    if request["method"] == "tools/call" {
        let tool = request["params"]["name"].as_str().unwrap_or("unknown");
        let arguments = request["params"]
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        return write_refusal_with_egress(
            server,
            stdout,
            EgressContext {
                evidence: None,
                tool: tool.to_string(),
                args_sha256: sha256_json(&arguments),
                company_guid: arguments
                    .get("company_guid")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
            response,
        )
        .await;
    }
    write_response(stdout, &response, server.settings.max_bytes).await
}

async fn write_refusal_with_egress<W: AsyncWrite + Unpin>(
    server: &Server,
    stdout: &mut W,
    context: EgressContext,
    response: Value,
) -> Result<(), String> {
    let serialized = serialize_response(&response, server.settings.max_bytes)?;
    let receipt = server.append_framed_egress(context, &response, &serialized)?;
    stdout
        .write_all(serialized.as_bytes())
        .await
        .map_err(|_| "stdio_write_failed".to_string())?;
    stdout
        .flush()
        .await
        .map_err(|_| "stdio_flush_failed".to_string())?;
    server.append_stdio_write_completed(receipt)
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
    max_bytes: usize,
) -> Result<(), String> {
    let serialized = serialize_response(response, max_bytes)?;
    stdout
        .write_all(serialized.as_bytes())
        .await
        .map_err(|_| "stdio_write_failed".to_string())?;
    stdout
        .flush()
        .await
        .map_err(|_| "stdio_flush_failed".to_string())
}

fn serialize_response(response: &Value, max_bytes: usize) -> Result<String, String> {
    let serialized = format!("{response}\n");
    if serialized.len() > max_bytes {
        return Err("agent_response_too_large".into());
    }
    Ok(serialized)
}

#[cfg(test)]
#[path = "agent_protocol_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "agent_post_cancellation_tests.rs"]
mod post_cancellation_tests;

#[cfg(test)]
#[path = "agent_post_recovery_tests.rs"]
mod post_recovery_tests;
