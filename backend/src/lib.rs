#![allow(clippy::default_constructed_unit_structs)]
#![allow(unused_imports)]

use axum::{
    Json,
    body::Bytes,
    extract::State as AxumState,
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::any,
};
use futures_util::stream::{self, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use shared::{
    State,
    models::ByUuid,
    extensions::{
        ConstructedExtension, Extension, ExtensionRouteBuilder, distr::MetadataToml,
    },
};
use sqlx::Row;
use std::{convert::Infallible, sync::Arc, time::Duration};
use utoipa_axum::router::OpenApiRouter;

pub mod tools;
pub mod validate;

pub const PACKAGE: &str = "dev.calagopus.mcpserver";
pub const VERSION: &str = "1.3.9";

const SECRET_SETTING: &str = "dev_calagopus_mcp::secret_key";

/// Read-only mode blocks every tool that changes state.
pub fn readonly_mode() -> bool {
    std::env::var("CALAGOPUS_MCP_READONLY").as_deref() == Ok("true")
}

/// Accepting the key in the URL query string is kept for existing clients, but can
/// be turned off with CALAGOPUS_MCP_ALLOW_QUERY_KEY=false. URLs end up in logs.
fn query_key_allowed() -> bool {
    std::env::var("CALAGOPUS_MCP_ALLOW_QUERY_KEY").as_deref() != Ok("false")
}

#[derive(Default)]
pub struct ExtensionStruct;

/// Returns the active MCP secret. Database errors are returned as errors and never
/// answered with a fresh key, so a transient fault cannot invalidate every client.
pub async fn get_or_generate_mcp_secret(state: &State) -> Result<String, String> {
    if let Ok(env_sec) = std::env::var("CALAGOPUS_MCP_SECRET") {
        let trimmed = env_sec.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    let stored: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = $1")
        .bind(SECRET_SETTING)
        .fetch_optional(state.database.write())
        .await
        .map_err(|e| {
            tracing::error!("Failed to read the MCP secret: {e:?}");
            "Could not load the MCP secret.".to_string()
        })?;

    match stored.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(sec) => Ok(sec),
        None => rotate_mcp_secret(state).await,
    }
}

pub async fn rotate_mcp_secret(state: &State) -> Result<String, String> {
    let new_key = format!("calagopus_mcp_sec_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(SECRET_SETTING)
    .bind(&new_key)
    .execute(state.database.write())
    .await
    .map_err(|e| {
        tracing::error!("Failed to persist the MCP secret: {e:?}");
        "Could not persist the new MCP secret.".to_string()
    })?;

    tracing::info!("Rotated the Calagopus MCP secret key.");
    Ok(new_key)
}

async fn ensure_domain_table(state: &State) {
    let create_table_query = "
        CREATE TABLE IF NOT EXISTS server_domains (
            uuid uuid PRIMARY KEY NOT NULL,
            server_uuid uuid NOT NULL,
            domain varchar(255) NOT NULL,
            created timestamp DEFAULT now() NOT NULL
        );
    ";
    if let Err(e) = sqlx::query(create_table_query).execute(state.database.write()).await {
        tracing::error!("Failed to initialize the server_domains table: {e}");
    }
}

#[async_trait::async_trait]
impl Extension for ExtensionStruct {
    async fn initialize(&mut self, state: State) {
        if let Err(e) = get_or_generate_mcp_secret(&state).await {
            tracing::error!("MCP secret unavailable at startup: {e}");
        }
        ensure_domain_table(&state).await;
        tracing::info!("Initializing Calagopus MCP Connector Extension v{VERSION} by Pato.");
    }

    async fn initialize_router(
        &mut self,
        _state: State,
        builder: ExtensionRouteBuilder,
    ) -> ExtensionRouteBuilder {
        builder
            .add_global_router(|router| {
                let mcp_router = OpenApiRouter::new()
                    .route("/api/extensions/mcp/v1/info", any(universal_mcp_handler))
                    .route("/api/extensions/mcp/v1/messages", any(universal_mcp_handler))
                    .route("/api/extensions/mcp/v1/sse", any(universal_mcp_handler))
                    .route("/api/extensions/mcp/v1/key", any(legacy_unauthorized_key_handler))
                    .route("/api/extensions/mcp/v1/key/rotate", any(legacy_unauthorized_key_handler))
                    .route("/api/extensions/mcp/v1/status", axum::routing::get(get_public_mcp_status_handler))
                    .route("/mcp", any(universal_mcp_handler))
                    .route("/api/mcp", any(universal_mcp_handler));

                router.merge(mcp_router)
            })
            .add_admin_api_router(|routes| {
                let admin_router = OpenApiRouter::new()
                    .route("/extensions/mcp/v1/key", axum::routing::get(get_mcp_key_handler))
                    .route("/extensions/mcp/v1/key/rotate", axum::routing::post(rotate_mcp_key_handler))
                    .route("/extensions/mcp/v1/status", axum::routing::get(get_admin_mcp_status_handler))
                    .route(&format!("/extensions/{PACKAGE}/key"), axum::routing::get(get_mcp_key_handler))
                    .route(&format!("/extensions/{PACKAGE}/key/rotate"), axum::routing::post(rotate_mcp_key_handler))
                    .route(&format!("/extensions/{PACKAGE}/status"), axum::routing::get(get_admin_mcp_status_handler));

                routes.merge(admin_router)
            })
    }
}

pub fn get_extension() -> ConstructedExtension {
    ConstructedExtension {
        metadata_toml: MetadataToml {
            package_name: PACKAGE.to_string(),
            name: "MCP Connector".to_string(),
            panel_version: semver::VersionReq::parse(">= 1.1.0").unwrap(),
            license_text: Some("Custom License - Copyright (c) 2026 Pato. Non-commercial, Attribution Required.".to_string()),
        },
        package_name: PACKAGE,
        description: "Exposes Calagopus Game Panel management tools (26 tools) via Model Context Protocol (MCP) JSON-RPC 2.0 and SSE transports.",
        authors: &["Pato"],
        version: semver::Version::new(1, 3, 9),
        extension: Arc::new(ExtensionStruct),
    }
}

/// Constant-time string equality check to prevent timing side-channel attacks
#[inline(never)]
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    if a_bytes.len() != b_bytes.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a_bytes.iter().zip(b_bytes.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn legacy_unauthorized_key_handler() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "Unauthorized",
            "message": "Direct unauthenticated access to the MCP secret key is denied. Access is restricted to authenticated panel administrators via '/api/admin/extensions/mcp/v1/key'."
        })),
    )
        .into_response()
}

/// Public status: no secret, no endpoint URLs.
async fn get_public_mcp_status_handler() -> Response {
    Json(json!({
        "package_name": PACKAGE,
        "name": "MCP Connector",
        "version": VERSION,
        "author": "Pato",
        "status": "active",
        "tools_count": 26,
        "auth_required": true
    })).into_response()
}

fn server_error(message: &str) -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "error", "message": message }))).into_response()
}

/// Admin status (panel administrators only). Includes the secret for the admin UI.
async fn get_admin_mcp_status_handler(AxumState(state): AxumState<State>) -> Response {
    let key = match get_or_generate_mcp_secret(&state).await {
        Ok(k) => k,
        Err(e) => return server_error(&e),
    };
    Json(json!({
        "package_name": PACKAGE,
        "name": "MCP Connector",
        "version": VERSION,
        "author": "Pato",
        "status": "active",
        "secret_key": key,
        "tools_count": 26,
        "readonly": readonly_mode(),
        "endpoints": {
            "key": "/api/admin/extensions/mcp/v1/key",
            "rotate_key": "/api/admin/extensions/mcp/v1/key/rotate",
            "sse": "/api/extensions/mcp/v1/sse",
            "messages": "/api/extensions/mcp/v1/messages",
            "info": "/api/extensions/mcp/v1/info"
        }
    })).into_response()
}

async fn get_mcp_key_handler(AxumState(state): AxumState<State>) -> Response {
    match get_or_generate_mcp_secret(&state).await {
        Ok(key) => Json(json!({
            "status": "ok",
            "secret_key": key,
            "header_example": "Authorization: Bearer <secret_key>"
        })).into_response(),
        Err(e) => server_error(&e),
    }
}

async fn rotate_mcp_key_handler(AxumState(state): AxumState<State>) -> Response {
    match rotate_mcp_secret(&state).await {
        Ok(new_key) => Json(json!({
            "status": "ok",
            "message": "MCP Secret Key successfully rotated.",
            "secret_key": new_key,
            "header_example": "Authorization: Bearer <secret_key>"
        })).into_response(),
        Err(e) => server_error(&e),
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JsonRpcRequest {
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: Option<String>,
    params: Option<Value>,
}

async fn check_mcp_auth(state: &State, headers: &HeaderMap, uri: &axum::http::Uri) -> Result<(), Response> {
    let expected_secret = match get_or_generate_mcp_secret(state).await {
        Ok(s) => s,
        Err(_) => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "jsonrpc": "2.0",
                    "error": { "code": -32002, "message": "MCP authentication is temporarily unavailable." }
                })),
            ).into_response());
        }
    };

    if let Some(auth_val) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(token) = auth_val.strip_prefix("Bearer ") {
            if constant_time_eq(token.trim(), &expected_secret) {
                return Ok(());
            }
        }
    }

    if let Some(key_val) = headers.get("x-mcp-key").or_else(|| headers.get("x-api-key")).and_then(|v| v.to_str().ok()) {
        if constant_time_eq(key_val.trim(), &expected_secret) {
            return Ok(());
        }
    }

    if query_key_allowed() {
        if let Some(query) = uri.query() {
            for pair in query.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    if (k == "api_key" || k == "token" || k == "key" || k == "api-key" || k == "mcp_key")
                        && constant_time_eq(v, &expected_secret)
                    {
                        return Ok(());
                    }
                }
            }
        }
    }

    tracing::warn!("Unauthorized MCP request to '{}'", uri.path());

    let unauthorized_response = Json(json!({
        "jsonrpc": "2.0",
        "error": {
            "code": -32001,
            "message": "Unauthorized: Invalid or missing MCP authentication secret key. Provide 'Authorization: Bearer <key>' or 'X-MCP-Key: <key>'."
        }
    }));

    Err((StatusCode::UNAUTHORIZED, unauthorized_response).into_response())
}

async fn universal_mcp_handler(
    method: Method,
    headers: HeaderMap,
    uri: axum::http::Uri,
    AxumState(state): AxumState<State>,
    body: Bytes,
) -> Response {
    if let Err(unauth_response) = check_mcp_auth(&state, &headers, &uri).await {
        return unauth_response;
    }

    let is_sse_request = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .map_or(false, |accept| accept.contains("text/event-stream") || accept.contains("*/*"));

    if method == Method::GET && (is_sse_request || body.is_empty()) {
        let host = headers.get("host").and_then(|v| v.to_str().ok()).unwrap_or("127.0.0.1:8000").to_string();
        let scheme = if headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https") { "https".to_string() } else { "http".to_string() };
        return mcp_sse_handler(host, scheme).into_response();
    }

    let payload: JsonRpcRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(_) => {
            return Json(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32700, "message": "Parse error" }
            })).into_response();
        }
    };

    let target_method = payload.method.clone().unwrap_or_default();

    // JSON-RPC notifications carry no id and get no response body.
    if payload.id.is_none() && target_method.starts_with("notifications/") {
        return StatusCode::ACCEPTED.into_response();
    }

    let request_id = payload.id.clone().unwrap_or(Value::Null);

    let response_result = match target_method.as_str() {
        "initialize" => json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {
                    "listChanged": true
                }
            },
            "serverInfo": {
                "name": "calagopus-mcp-connector",
                "version": VERSION
            }
        }),
        "tools/list" => tools::definitions::get_tools_list(),
        "tools/call" => execute_tool(&state, payload.params).await,
        _ => {
            return Json(json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {
                    "code": -32601,
                    "message": "Method not found"
                }
            })).into_response();
        }
    };

    Json(json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": response_result
    })).into_response()
}

fn mcp_sse_handler(host: String, scheme: String) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let endpoint_url = format!("{scheme}://{host}/api/extensions/mcp/v1/messages?session_id={session_id}");

    let init_event = Event::default().event("endpoint").data(endpoint_url);
    let first = stream::once(async move { Ok(init_event) });

    let ping_stream = stream::unfold((), |_| async {
        tokio::time::sleep(Duration::from_secs(15)).await;
        Some((Ok(Event::default().comment("ping")), ()))
    });

    let combined = first.chain(ping_stream);

    Sse::new(combined).keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(15)))
}

async fn execute_tool(state: &State, params: Option<Value>) -> Value {
    let params = params.unwrap_or(json!({}));
    let raw_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    let normalized = raw_name.to_lowercase().replace('-', "_");
    let norm_name = normalized
        .strip_prefix("calagopus_")
        .map(str::to_string)
        .unwrap_or_else(|| normalized.clone());

    // Log the tool name only. Arguments can contain file contents and commands.
    tracing::info!(tool = %norm_name, "Executing MCP tool");

    tools::domain::execute_domain_tool(state, &norm_name, &arguments).await
}
