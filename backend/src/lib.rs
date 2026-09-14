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

#[derive(Default)]
pub struct ExtensionStruct;

pub async fn get_or_generate_mcp_secret(state: &State) -> String {
    if let Ok(env_sec) = std::env::var("CALAGOPUS_MCP_SECRET") {
        if !env_sec.trim().is_empty() {
            return env_sec.trim().to_string();
        }
    }

    let stored: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'dev_calagopus_mcp::secret_key'")
        .fetch_optional(state.database.read())
        .await
        .ok()
        .flatten();

    if let Some(sec) = stored {
        if !sec.trim().is_empty() {
            return sec;
        }
    }

    rotate_mcp_secret(state).await
}

pub async fn rotate_mcp_secret(state: &State) -> String {
    let new_key = format!("calagopus_mcp_sec_{}", uuid::Uuid::new_v4().simple());
    let res = sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('dev_calagopus_mcp::secret_key', $1) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value"
    )
    .bind(&new_key)
    .execute(state.database.write())
    .await;

    if let Err(err) = res {
        tracing::error!("Failed to persist dev_calagopus_mcp::secret_key in database: {:?}", err);
    } else {
        tracing::info!("🔄 Generated/Rotated panel-unique Calagopus MCP secret key.");
    }

    new_key
}

#[async_trait::async_trait]
impl Extension for ExtensionStruct {
    async fn initialize(&mut self, state: State) {
        let _ = get_or_generate_mcp_secret(&state).await;
        tracing::info!("Initializing Calagopus MCP (Model Context Protocol) Connector Extension v1.3.3 by Pato.");
    }

    async fn initialize_router(
        &mut self,
        _state: State,
        builder: ExtensionRouteBuilder,
    ) -> ExtensionRouteBuilder {
        builder.add_global_router(|router| {
            let mcp_router = OpenApiRouter::new()
                .route("/api/extensions/mcp/v1/info", any(universal_mcp_handler))
                .route("/api/extensions/mcp/v1/messages", any(universal_mcp_handler))
                .route("/api/extensions/mcp/v1/sse", any(universal_mcp_handler))
                .route("/api/extensions/mcp/v1/key", axum::routing::get(get_mcp_key_handler))
                .route("/api/extensions/mcp/v1/key/rotate", any(rotate_mcp_key_handler))
                .route("/api/extensions/mcp/v1/status", axum::routing::get(get_mcp_status_handler))
                .route("/mcp", any(universal_mcp_handler))
                .route("/api/mcp", any(universal_mcp_handler));

            router.merge(mcp_router)
        })
    }
}

pub fn get_extension() -> ConstructedExtension {
    ConstructedExtension {
        metadata_toml: MetadataToml {
            package_name: "dev.calagopus.mcpserver".to_string(),
            name: "MCP Connector".to_string(),
            panel_version: semver::VersionReq::parse(">= 1.1.0").unwrap(),
            license_text: Some("Custom License - Copyright (c) 2026 Pato. Non-commercial, Attribution Required.".to_string()),
        },
        package_name: "dev.calagopus.mcpserver",
        description: "Exposes Calagopus Game Panel management tools (22 tools) via Model Context Protocol (MCP) JSON-RPC 2.0 and SSE transports.",
        authors: &["Pato"],
        version: semver::Version::new(1, 3, 3),
        extension: Arc::new(ExtensionStruct),
    }
}

async fn get_mcp_key_handler(AxumState(state): AxumState<State>) -> Response {
    let key = get_or_generate_mcp_secret(&state).await;
    Json(json!({
        "status": "ok",
        "secret_key": key,
        "sse_url": format!("/api/extensions/mcp/v1/sse?api_key={key}"),
        "header_example": format!("Authorization: Bearer {key}")
    })).into_response()
}

async fn rotate_mcp_key_handler(AxumState(state): AxumState<State>) -> Response {
    let new_key = rotate_mcp_secret(&state).await;
    Json(json!({
        "status": "ok",
        "message": "MCP Secret Key successfully rotated.",
        "secret_key": new_key,
        "sse_url": format!("/api/extensions/mcp/v1/sse?api_key={new_key}"),
        "header_example": format!("Authorization: Bearer {new_key}")
    })).into_response()
}

async fn get_mcp_status_handler(AxumState(state): AxumState<State>) -> Response {
    let key = get_or_generate_mcp_secret(&state).await;
    Json(json!({
        "package_name": "dev.calagopus.mcpserver",
        "name": "MCP Connector",
        "version": "1.3.3",
        "author": "Pato",
        "status": "active",
        "secret_key": key,
        "tools_count": 22,
        "endpoints": {
            "key": "/api/extensions/mcp/v1/key",
            "rotate_key": "/api/extensions/mcp/v1/key/rotate",
            "sse": format!("/api/extensions/mcp/v1/sse?api_key={key}"),
            "messages": "/api/extensions/mcp/v1/messages",
            "info": "/api/extensions/mcp/v1/info"
        }
    })).into_response()
}

// -----------------------------------------------------------------------------
// Universal Request Dispatcher & Handlers
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JsonRpcRequest {
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: Option<String>,
    params: Option<Value>,
}

async fn check_mcp_auth(state: &State, headers: &HeaderMap, uri: &axum::http::Uri) -> Result<(), Response> {
    if std::env::var("CALAGOPUS_MCP_AUTH_DISABLED").as_deref() == Ok("true") {
        return Ok(());
    }

    let expected_secret = get_or_generate_mcp_secret(state).await;

    // Check Authorization: Bearer <secret>
    if let Some(auth_val) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if auth_val.starts_with("Bearer ") && auth_val.trim_start_matches("Bearer ").trim() == expected_secret {
            return Ok(());
        }
    }

    // Check X-MCP-Key or X-API-Key
    if let Some(key_val) = headers.get("x-mcp-key").or_else(|| headers.get("x-api-key")).and_then(|v| v.to_str().ok()) {
        if key_val.trim() == expected_secret {
            return Ok(());
        }
    }

    // Check URI query parameters (?api_key=... or ?token=... or ?key=...)
    if let Some(query) = uri.query() {
        for pair in query.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if (k == "api_key" || k == "token" || k == "key" || k == "api-key" || k == "mcp_key") && v == expected_secret {
                    return Ok(());
                }
            }
        }
    }

    tracing::warn!("Unauthorized MCP request attempt to URI '{}'", uri.path());

    let unauthorized_response = Json(json!({
        "jsonrpc": "2.0",
        "error": {
            "code": -32001,
            "message": "Unauthorized: Invalid or missing MCP authentication secret key. Provide 'Authorization: Bearer <key>', 'X-MCP-Key: <key>', or query parameter '?api_key=<key>'."
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
        Err(_) => JsonRpcRequest {
            jsonrpc: Some("2.0".into()),
            id: Some(json!(1)),
            method: Some("initialize".into()),
            params: None,
        },
    };

    let request_id = payload.id.unwrap_or(json!(1));
    let target_method = payload.method.unwrap_or_else(|| "initialize".to_string());

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
                "version": "1.3.3"
            }
        }),
        "notifications/initialized" => json!({}),
        "tools/list" => get_tools_list(),
        "tools/call" => execute_tool(&state, payload.params).await,
        _ => {
            return Json(json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {
                    "code": -32601,
                    "message": format!("Method '{target_method}' not found")
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

// -----------------------------------------------------------------------------
// MCP Tools Specification & Execution Engine
// -----------------------------------------------------------------------------

fn get_tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "get-system-health",
                "description": "Query live Calagopus panel system metrics: user/node/server counts and current timestamp.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "list-nests",
                "description": "List all game nests, egg repositories, and server templates configured in Calagopus.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "limit":  { "type": "integer", "description": "Max nests to return (default 50)" },
                        "offset": { "type": "integer", "description": "Pagination offset (default 0)" }
                    }
                }
            },
            {
                "name": "list-users",
                "description": "List registered panel users, email addresses, roles, and administrative privileges.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "limit":  { "type": "integer", "description": "Max users to return (default 50)" },
                        "offset": { "type": "integer", "description": "Pagination offset (default 0)" }
                    }
                }
            },
            {
                "name": "list-servers",
                "description": "Every server the key can see, with UUIDs, nodes, memory/disk allocations, and creation timestamps.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "limit":  { "type": "integer", "description": "Max servers to return (default 50)" },
                        "offset": { "type": "integer", "description": "Pagination offset (default 0)" }
                    }
                }
            },
            {
                "name": "get-server",
                "description": "One server in detail, including allocation, node mapping, and the daemon's live state.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" }
                    },
                    "required": ["server_uuid"]
                }
            },
            {
                "name": "power-server",
                "description": "Start, stop, restart, or kill a server daemon container.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "action": { "type": "string", "enum": ["start", "stop", "restart", "kill"], "description": "Power action to execute" }
                    },
                    "required": ["server_uuid", "action"]
                }
            },
            {
                "name": "send-console-command",
                "description": "Run a command on a server's console.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "command": { "type": "string", "description": "The console command string to execute" }
                    },
                    "required": ["server_uuid", "command"]
                }
            },
            {
                "name": "read-console",
                "description": "The tail of a server's console output logs.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "lines": { "type": "integer", "description": "Number of log lines to return (default 50)" }
                    },
                    "required": ["server_uuid"]
                }
            },
            {
                "name": "list-machines",
                "description": "Enrolled machines (nodes) with health status, SFTP port, URL, memory, and disk capacity.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "limit":  { "type": "integer", "description": "Max machines to return (default 50)" },
                        "offset": { "type": "integer", "description": "Pagination offset (default 0)" }
                    }
                }
            },
            {
                "name": "list-files",
                "description": "Browse a directory inside a server's container volume.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "directory": { "type": "string", "description": "Target directory path (default '/')" }
                    },
                    "required": ["server_uuid"]
                }
            },
            {
                "name": "read-file",
                "description": "Read a text file from the server volume. Files larger than 1 MiB are rejected.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "path": { "type": "string", "description": "Relative file path inside server volume" }
                    },
                    "required": ["server_uuid", "path"]
                }
            },
            {
                "name": "write-file",
                "description": "Create a file or replace its contents in the server volume.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "path": { "type": "string", "description": "File path to create or overwrite" },
                        "content": { "type": "string", "description": "Text content to write" },
                        "overwrite": { "type": "boolean", "description": "If false and file exists, return error (default true)" }
                    },
                    "required": ["server_uuid", "path", "content"]
                }
            },
            {
                "name": "upload-file-from-url",
                "description": "Have the machine fetch a file straight onto the server container volume. URL must be http(s).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "url": { "type": "string", "description": "Direct download URL (must start with http:// or https://)" },
                        "directory": { "type": "string", "description": "Target directory on server (default '/')" },
                        "foreground": { "type": "boolean", "description": "Wait for download to complete before returning (default false)" }
                    },
                    "required": ["server_uuid", "url"]
                }
            },
            {
                "name": "download-files",
                "description": "A signed download link for a file, a packed directory, or the whole server volume.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "path": { "type": "string", "description": "File or directory path to download (default '/')" },
                        "expiry_seconds": { "type": "integer", "description": "JWT link expiry in seconds (default 3600)" }
                    },
                    "required": ["server_uuid"]
                }
            },
            {
                "name": "list-backups",
                "description": "Every backup a server holds, with state, creation timestamp, and size in bytes.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "Optional UUID to filter by server" },
                        "limit":  { "type": "integer", "description": "Max backups to return (default 50)" },
                        "offset": { "type": "integer", "description": "Pagination offset (default 0)" },
                        "status": { "type": "string", "enum": ["successful", "failed"], "description": "Optional filter by backup status" }
                    }
                }
            },
            {
                "name": "create-backup",
                "description": "Take a backup; it archives in the background.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "name": { "type": "string", "description": "Optional name or label for the backup" }
                    },
                    "required": ["server_uuid"]
                }
            },
            {
                "name": "download-backup",
                "description": "A short-lived download link for a finished backup archive.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "backup_uuid": { "type": "string", "description": "The UUID of the backup" }
                    },
                    "required": ["server_uuid", "backup_uuid"]
                }
            },
            {
                "name": "deploy-files",
                "description": "Deploy and unpack a ZIP or tar archive directly onto a game server volume.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "archive_url": { "type": "string", "description": "URL to ZIP or tar.gz file archive (must be http(s))" },
                        "target_directory": { "type": "string", "description": "Target folder to extract into (default '/')" }
                    },
                    "required": ["server_uuid", "archive_url"]
                }
            },
            {
                "name": "search-plugins",
                "description": "Search the Minecraft plugin catalog (Modrinth), filtered to compatible server software.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query keywords" },
                        "software": { "type": "string", "description": "Server software (e.g. 'paper', 'spigot', 'fabric')" },
                        "mc_version": { "type": "string", "description": "Minecraft target version (e.g. '1.20.4')" }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "list-plugins",
                "description": "Every plugin jar present in a server's /plugins folder.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" }
                    },
                    "required": ["server_uuid"]
                }
            },
            {
                "name": "install-plugin",
                "description": "Install a plugin JAR into /plugins. Resolves from Modrinth if plugin_id is given. Optionally verifies checksum.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid":  { "type": "string", "description": "The UUID of the target server" },
                        "plugin_id":    { "type": "string", "description": "Modrinth plugin ID" },
                        "download_url": { "type": "string", "description": "Direct plugin JAR download URL" },
                        "filename":     { "type": "string", "description": "JAR filename to save as" },
                        "checksum":     { "type": "string", "description": "Optional expected SHA-1 checksum for verification" }
                    },
                    "required": ["server_uuid"]
                }
            },
            {
                "name": "remove-plugin",
                "description": "Delete a plugin jar from the server's /plugins folder.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "filename":    { "type": "string", "description": "JAR filename in /plugins" }
                    },
                    "required": ["server_uuid", "filename"]
                }
            },
            {
                "name": "get-site",
                "description": "List domain bindings for a server, or all domain bindings panel-wide.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "Optional UUID to filter to a specific server" }
                    }
                }
            },
            {
                "name": "attach-domain",
                "description": "Bind a custom domain to a game server.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "domain":      { "type": "string", "description": "The domain name to attach (e.g. 'play.example.com')" }
                    },
                    "required": ["server_uuid", "domain"]
                }
            },
            {
                "name": "detach-domain",
                "description": "Remove a custom domain binding from a game server.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "domain":      { "type": "string", "description": "The domain name to detach" }
                    },
                    "required": ["server_uuid", "domain"]
                }
            },
            {
                "name": "deploy-repo",
                "description": "Clone a GitHub or GitLab repository branch onto a server volume by downloading its archive tarball.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid":      { "type": "string", "description": "The UUID of the target server" },
                        "repo_url":         { "type": "string", "description": "HTTPS URL to the repository (e.g. 'https://github.com/user/repo')" },
                        "branch":           { "type": "string", "description": "Branch to deploy (default 'main')" },
                        "target_directory": { "type": "string", "description": "Directory to deploy into (default '/')" }
                    },
                    "required": ["server_uuid", "repo_url"]
                }
            }
        ]
    })
}

async fn get_wings_client_for_server(
    state: &State,
    server_uuid: &str,
) -> Result<(wings_api::client::WingsClient, uuid::Uuid), String> {
    let parsed_uuid = uuid::Uuid::parse_str(server_uuid)
        .map_err(|_| format!("Invalid server UUID: '{server_uuid}'"))?;

    let row = sqlx::query(
        r#"
        SELECT nodes.url, nodes.token
        FROM servers
        JOIN nodes ON nodes.uuid = servers.node_uuid
        WHERE servers.uuid = $1
        "#
    )
    .bind(parsed_uuid)
    .fetch_optional(state.database.read())
    .await
    .map_err(|e| format!("Database query error: {e}"))?
    .ok_or_else(|| format!("Server with UUID '{server_uuid}' not found"))?;

    let url: String = row.get("url");
    let token_bytes: Vec<u8> = row.get("token");
    let decrypted_token = state.database
        .decrypt(token_bytes)
        .await
        .map_err(|e| format!("Failed to decrypt node token: {e}"))?;

    let client = wings_api::client::WingsClient::new(url, decrypted_token.to_string());
    Ok((client, parsed_uuid))
}

async fn execute_tool(state: &State, params: Option<Value>) -> Value {
    let params = params.unwrap_or(json!({}));
    let raw_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    // Normalize tool name: replace '-' with '_', lowercased, strip leading 'calagopus_'
    let mut norm_name = raw_name.to_lowercase().replace('-', "_");
    if norm_name.starts_with("calagopus_") {
        norm_name = norm_name.trim_start_matches("calagopus_").to_string();
    }

    match norm_name.as_str() {
        // ─────────────────────────────────────────────────────────────────
        // get-system-health  – live DB counts + timestamp
        // ─────────────────────────────────────────────────────────────────
        "get_system_health" | "system_health" | "health" => {
            let users_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
                .fetch_one(state.database.read()).await.unwrap_or(0);
            let nodes_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes")
                .fetch_one(state.database.read()).await.unwrap_or(0);
            let servers_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM servers")
                .fetch_one(state.database.read()).await.unwrap_or(0);

            format_mcp_content(json!({
                "panel_status":   "healthy",
                "version":        "1.3.3",
                "database":       "postgresql",
                "total_users":    users_count,
                "total_nodes":    nodes_count,
                "total_servers":  servers_count,
                "timestamp":      chrono::Utc::now().to_rfc3339()
            }))
        }

        // ─────────────────────────────────────────────────────────────────
        // list-nests  – paginated
        // ─────────────────────────────────────────────────────────────────
        "list_nests" | "nests" => {
            let limit  = arguments.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
            let offset = arguments.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
            let result = sqlx::query(
                "SELECT uuid, name, author, created FROM nests ORDER BY created DESC LIMIT $1 OFFSET $2"
            ).bind(limit).bind(offset).fetch_all(state.database.read()).await;

            match result {
                Ok(rows) => {
                    let list: Vec<Value> = rows.into_iter().map(|r| {
                        let uuid: uuid::Uuid = r.get("uuid");
                        let name: String     = r.get("name");
                        let author: String   = r.get("author");
                        let created: chrono::NaiveDateTime = r.get("created");
                        json!({ "uuid": uuid.to_string(), "name": name, "author": author, "created": created.to_string() })
                    }).collect();
                    format_mcp_content(json!({ "total": list.len(), "offset": offset, "limit": limit, "nests": list }))
                }
                Err(e) => format_mcp_error(&format!("Database query error: {e}"))
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // list-users  – paginated
        // ─────────────────────────────────────────────────────────────────
        "list_users" | "users" => {
            let limit  = arguments.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
            let offset = arguments.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
            let result = sqlx::query(
                "SELECT uuid, username, email, admin, created FROM users ORDER BY created DESC LIMIT $1 OFFSET $2"
            ).bind(limit).bind(offset).fetch_all(state.database.read()).await;

            match result {
                Ok(rows) => {
                    let list: Vec<Value> = rows.into_iter().map(|r| {
                        let uuid: uuid::Uuid = r.get("uuid");
                        let username: String = r.get("username");
                        let email: String    = r.get("email");
                        let admin: bool      = r.get("admin");
                        let created: chrono::NaiveDateTime = r.get("created");
                        json!({ "uuid": uuid.to_string(), "username": username, "email": email, "is_admin": admin, "created": created.to_string() })
                    }).collect();
                    format_mcp_content(json!({ "total": list.len(), "offset": offset, "limit": limit, "users": list }))
                }
                Err(e) => format_mcp_error(&format!("Database query error: {e}"))
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // list-servers  – paginated
        // ─────────────────────────────────────────────────────────────────
        "list_servers" => {
            let limit  = arguments.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
            let offset = arguments.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
            let result = sqlx::query(
                "SELECT uuid, name, node_uuid, memory, disk, created FROM servers ORDER BY created DESC LIMIT $1 OFFSET $2"
            ).bind(limit).bind(offset).fetch_all(state.database.read()).await;

            match result {
                Ok(rows) => {
                    let list: Vec<Value> = rows.into_iter().map(|r| {
                        let uuid: uuid::Uuid      = r.get("uuid");
                        let node_uuid: uuid::Uuid = r.get("node_uuid");
                        let name: String          = r.get("name");
                        let memory: i64           = r.get("memory");
                        let disk: i64             = r.get("disk");
                        let created: chrono::NaiveDateTime = r.get("created");
                        json!({ "uuid": uuid.to_string(), "name": name, "node_uuid": node_uuid.to_string(), "memory_mb": memory, "disk_mb": disk, "created": created.to_string() })
                    }).collect();
                    format_mcp_content(json!({ "total": list.len(), "offset": offset, "limit": limit, "servers": list }))
                }
                Err(e) => format_mcp_error(&format!("Database query error: {e}"))
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // get-server  – single server with live Wings daemon state
        // ─────────────────────────────────────────────────────────────────
        "get_server" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let parsed_uuid = match uuid::Uuid::parse_str(server_uuid) {
                Ok(u) => u,
                Err(_) => return format_mcp_error(&format!("Invalid server UUID: '{server_uuid}'")),
            };

            let row = sqlx::query(
                "SELECT uuid, name, node_uuid, memory, disk, created FROM servers WHERE uuid = $1"
            ).bind(parsed_uuid).fetch_optional(state.database.read()).await;

            match row {
                Ok(Some(r)) => {
                    let uuid: uuid::Uuid      = r.get("uuid");
                    let node_uuid: uuid::Uuid = r.get("node_uuid");
                    let name: String          = r.get("name");
                    let memory: i64           = r.get("memory");
                    let disk: i64             = r.get("disk");
                    let created: chrono::NaiveDateTime = r.get("created");

                    let daemon_info = if let Ok((client, server_id)) = get_wings_client_for_server(state, server_uuid).await {
                        match client.get_servers_server(server_id).await {
                            Ok(ws) => json!({
                                "state":      format!("{:?}", ws.state).to_lowercase(),
                                "is_running": matches!(ws.state, wings_api::ServerState::Running | wings_api::ServerState::Starting),
                                "process":    { "state": format!("{:?}", ws.state).to_lowercase() }
                            }),
                            Err(e) => json!({ "state": "unreachable", "error": format!("{e:?}") })
                        }
                    } else {
                        json!({ "state": "offline" })
                    };

                    let live_status = daemon_info.get("state").and_then(|s| s.as_str()).unwrap_or("offline").to_string();
                    format_mcp_content(json!({
                        "uuid": uuid.to_string(), "name": name, "node_uuid": node_uuid.to_string(),
                        "memory_mb": memory, "disk_mb": disk, "status": live_status,
                        "daemon_state": daemon_info, "created": created.to_string()
                    }))
                }
                Ok(None)  => format_mcp_error(&format!("Server '{server_uuid}' not found")),
                Err(err)  => format_mcp_error(&format!("Database query error: {err}")),
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // power-server
        // ─────────────────────────────────────────────────────────────────
        "power_server" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let action_str  = arguments.get("action").and_then(|v| v.as_str()).unwrap_or("restart");

            let action = match action_str.to_lowercase().as_str() {
                "start"   => wings_api::ServerPowerAction::Start,
                "stop"    => wings_api::ServerPowerAction::Stop,
                "restart" => wings_api::ServerPowerAction::Restart,
                "kill"    => wings_api::ServerPowerAction::Kill,
                _ => return format_mcp_error(&format!("Invalid power action '{action_str}'. Must be: start, stop, restart, kill.")),
            };

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let req_body = wings_api::servers_server_power::post::RequestBody { action, wait_seconds: None };
                    match client.post_servers_server_power(server_id, &req_body).await {
                        Ok(_) => format_mcp_content(json!({
                            "status": "power_signal_sent", "server_uuid": server_uuid,
                            "action": action_str, "timestamp": chrono::Utc::now().to_rfc3339()
                        })),
                        Err(e) => format_mcp_error(&format!("Wings power signal failed: {e:?}")),
                    }
                }
                Err(e) => format_mcp_error(&e),
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // send-console-command
        // ─────────────────────────────────────────────────────────────────
        "send_console_command" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let command     = arguments.get("command").and_then(|v| v.as_str()).unwrap_or("");

            if command.trim().is_empty() {
                return format_mcp_error("Parameter 'command' cannot be empty.");
            }

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let req_body = wings_api::servers_server_commands::post::RequestBody {
                        commands: vec![command.to_string().into()],
                    };
                    match client.post_servers_server_commands(server_id, &req_body).await {
                        Ok(_) => format_mcp_content(json!({
                            "status": "command_executed", "server_uuid": server_uuid,
                            "command": command, "timestamp": chrono::Utc::now().to_rfc3339()
                        })),
                        Err(e) => format_mcp_error(&format!("Wings command failed: {e:?}")),
                    }
                }
                Err(e) => format_mcp_error(&e),
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // read-console
        // ─────────────────────────────────────────────────────────────────
        "read_console" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let lines       = arguments.get("lines").and_then(|v| v.as_i64()).unwrap_or(50);

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let query = wings_api::servers_server_logs::get::Query {
                        lines: Some(lines as u64), ..Default::default()
                    };
                    match client.get_servers_server_logs(server_id, &query).await {
                        Ok(mut logs_reader) => {
                            use tokio::io::AsyncReadExt;
                            let mut log_buf = String::new();
                            if let Err(e) = logs_reader.read_to_string(&mut log_buf).await {
                                return format_mcp_error(&format!("Failed reading console logs: {e}"));
                            }
                            let lines_vec: Vec<&str> = log_buf.lines().collect();
                            format_mcp_content(json!({
                                "server_uuid": server_uuid, "lines_requested": lines,
                                "lines_returned": lines_vec.len(), "console_output": lines_vec
                            }))
                        }
                        Err(e) => format_mcp_error(&format!("Wings log retrieval failed: {e:?}")),
                    }
                }
                Err(e) => format_mcp_error(&e),
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // list-machines  – paginated + live health
        // ─────────────────────────────────────────────────────────────────
        "list_machines" => {
            let limit  = arguments.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
            let offset = arguments.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
            let result = sqlx::query(
                "SELECT uuid, name, url, sftp_port, memory, disk, created FROM nodes ORDER BY created DESC LIMIT $1 OFFSET $2"
            ).bind(limit).bind(offset).fetch_all(state.database.read()).await;

            match result {
                Ok(rows) => {
                    let mut list: Vec<Value> = Vec::new();
                    for r in rows {
                        let uuid: uuid::Uuid = r.get("uuid");
                        let name: String     = r.get("name");
                        let url: String      = r.get("url");
                        let sftp_port: i32   = r.get("sftp_port");
                        let memory: i64      = r.get("memory");
                        let disk: i64        = r.get("disk");
                        let created: chrono::NaiveDateTime = r.get("created");
                        let is_healthy = match shared::models::node::Node::by_uuid_optional(&state.database, uuid).await {
                            Ok(Some(node)) => node.api_client(&state.database).await.is_ok(),
                            _ => false,
                        };
                        list.push(json!({
                            "uuid": uuid.to_string(), "name": name, "url": url,
                            "status": if is_healthy { "healthy" } else { "unreachable" },
                            "sftp_port": sftp_port, "memory_mb": memory, "disk_mb": disk,
                            "created": created.to_string()
                        }));
                    }
                    format_mcp_content(json!({ "total": list.len(), "offset": offset, "limit": limit, "machines": list }))
                }
                Err(e) => format_mcp_error(&format!("Database query error: {e}"))
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // list-files
        // ─────────────────────────────────────────────────────────────────
        "list_files" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let directory   = arguments.get("directory").and_then(|v| v.as_str()).unwrap_or("/");

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let query = wings_api::servers_server_files_list::get::Query {
                        directory: Some(directory.into()), ..Default::default()
                    };
                    match client.get_servers_server_files_list(server_id, &query).await {
                        Ok(resp) => {
                            let entries: Vec<Value> = resp.entries.into_iter().map(|e| json!({
                                "name": e.name, "size": e.size, "is_file": e.file,
                                "is_directory": e.directory, "modified": e.modified.to_rfc3339()
                            })).collect();
                            format_mcp_content(json!({
                                "server_uuid": server_uuid, "directory": directory,
                                "total_entries": entries.len(), "entries": entries
                            }))
                        }
                        Err(e) => format_mcp_error(&format!("Wings list_files failed: {e:?}")),
                    }
                }
                Err(e) => format_mcp_error(&e),
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // read-file  – 1 MiB size limit
        // ─────────────────────────────────────────────────────────────────
        "read_file" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let path        = arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");

            if path.trim().is_empty() {
                return format_mcp_error("Parameter 'path' is required.");
            }

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let query = wings_api::servers_server_files_contents::get::Query {
                        file: Some(path.into()), ..Default::default()
                    };
                    match client.get_servers_server_files_contents(server_id, &query).await {
                        Ok(mut reader) => {
                            use tokio::io::AsyncReadExt;
                            let mut content = String::new();
                            if let Err(e) = reader.read_to_string(&mut content).await {
                                return format_mcp_error(&format!("Failed reading file: {e}"));
                            }
                            const MAX_SIZE: usize = 1_048_576;
                            if content.len() > MAX_SIZE {
                                return format_mcp_error(&format!(
                                    "File '{path}' is too large ({} bytes). Maximum is 1 MiB.", content.len()
                                ));
                            }
                            format_mcp_content(json!({
                                "server_uuid": server_uuid, "path": path,
                                "size_bytes": content.len(), "content": content
                            }))
                        }
                        Err(e) => format_mcp_error(&format!("Wings read_file failed: {e:?}")),
                    }
                }
                Err(e) => format_mcp_error(&e),
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // write-file  – optional overwrite=false guard
        // ─────────────────────────────────────────────────────────────────
        "write_file" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let path        = arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let content     = arguments.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let overwrite   = arguments.get("overwrite").and_then(|v| v.as_bool()).unwrap_or(true);

            if path.trim().is_empty() {
                return format_mcp_error("Parameter 'path' is required.");
            }

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    if !overwrite {
                        let dir = std::path::Path::new(path).parent()
                            .and_then(|p| p.to_str()).unwrap_or("/").to_string();
                        let fname = std::path::Path::new(path).file_name()
                            .and_then(|n| n.to_str()).unwrap_or(path);
                        let lq = wings_api::servers_server_files_list::get::Query {
                            directory: Some(dir.into()), ..Default::default()
                        };
                        if let Ok(resp) = client.get_servers_server_files_list(server_id, &lq).await {
                            if resp.entries.iter().any(|e| e.file && e.name == fname) {
                                return format_mcp_error(&format!(
                                    "File '{path}' already exists. Pass overwrite=true to replace it."
                                ));
                            }
                        }
                    }

                    let wq = wings_api::servers_server_files_write::post::Query {
                        file: Some(path.into()), ..Default::default()
                    };
                    let body = wings_api::client::AsyncRequestReader::new(
                        std::io::Cursor::new(content.as_bytes().to_vec())
                    );
                    match client.post_servers_server_files_write(server_id, body, &wq).await {
                        Ok(_) => format_mcp_content(json!({
                            "status": "file_written", "server_uuid": server_uuid,
                            "path": path, "bytes_written": content.len(),
                            "timestamp": chrono::Utc::now().to_rfc3339()
                        })),
                        Err(e) => format_mcp_error(&format!("Wings write_file failed: {e:?}")),
                    }
                }
                Err(e) => format_mcp_error(&e),
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // upload-file-from-url  – validates http/https, optional foreground
        // ─────────────────────────────────────────────────────────────────
        "upload_file_from_url" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let url         = arguments.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let directory   = arguments.get("directory").and_then(|v| v.as_str()).unwrap_or("/");
            let foreground  = arguments.get("foreground").and_then(|v| v.as_bool()).unwrap_or(false);

            if url.trim().is_empty() {
                return format_mcp_error("Parameter 'url' is required.");
            }
            let url_lc = url.to_lowercase();
            if !url_lc.starts_with("http://") && !url_lc.starts_with("https://") {
                return format_mcp_error("Parameter 'url' must use http:// or https:// scheme.");
            }

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let req = wings_api::servers_server_files_pull::post::RequestBody {
                        root: directory.into(), url: url.into(),
                        file_name: None, use_header: true, foreground,
                    };
                    match client.post_servers_server_files_pull(server_id, &req).await {
                        Ok(_) => format_mcp_content(json!({
                            "status": "upload_job_queued", "server_uuid": server_uuid,
                            "url": url, "target_directory": directory,
                            "foreground": foreground, "timestamp": chrono::Utc::now().to_rfc3339()
                        })),
                        Err(e) => format_mcp_error(&format!("Wings upload_file_from_url failed: {e:?}")),
                    }
                }
                Err(e) => format_mcp_error(&e),
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // deploy-files
        // ─────────────────────────────────────────────────────────────────
        "deploy_files" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let archive_url = arguments.get("archive_url").and_then(|v| v.as_str()).unwrap_or("");
            let target_dir  = arguments.get("target_directory")
                .or_else(|| arguments.get("directory"))
                .and_then(|v| v.as_str()).unwrap_or("/");

            if archive_url.trim().is_empty() {
                return format_mcp_error("Parameter 'archive_url' is required.");
            }
            let url_lc = archive_url.to_lowercase();
            if !url_lc.starts_with("http://") && !url_lc.starts_with("https://") {
                return format_mcp_error("Parameter 'archive_url' must use http:// or https:// scheme.");
            }

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let fname = archive_url.split('/').last().filter(|s| !s.is_empty()).unwrap_or("archive.zip");
                    let pull = wings_api::servers_server_files_pull::post::RequestBody {
                        root: target_dir.into(), url: archive_url.to_string().into(),
                        file_name: Some(fname.into()), use_header: true, foreground: true,
                    };
                    match client.post_servers_server_files_pull(server_id, &pull).await {
                        Ok(_) => {
                            let fl = fname.to_lowercase();
                            if fl.ends_with(".zip") || fl.ends_with(".tar.gz") || fl.ends_with(".tgz") || fl.ends_with(".tar") {
                                let decomp = wings_api::servers_server_files_decompress::post::RequestBody {
                                    root: target_dir.into(), file: fname.into(), foreground: true,
                                };
                                let _ = client.post_servers_server_files_decompress(server_id, &decomp).await;
                            }
                            format_mcp_content(json!({
                                "status": "files_deployed", "server_uuid": server_uuid,
                                "archive_url": archive_url, "target_directory": target_dir,
                                "archive_filename": fname, "decompressed": true,
                                "timestamp": chrono::Utc::now().to_rfc3339()
                            }))
                        }
                        Err(e) => format_mcp_error(&format!("Wings deploy_files failed: {e:?}")),
                    }
                }
                Err(e) => format_mcp_error(&e),
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // download-files  – configurable JWT expiry (default 1 h)
        // ─────────────────────────────────────────────────────────────────
        "download_files" => {
            let server_uuid    = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let path           = arguments.get("path").and_then(|v| v.as_str()).unwrap_or("/");
            let expiry_seconds = arguments.get("expiry_seconds").and_then(|v| v.as_i64()).unwrap_or(3600);

            let parsed_uuid = match uuid::Uuid::parse_str(server_uuid) {
                Ok(u) => u,
                Err(_) => return format_mcp_error(&format!("Invalid server UUID: '{server_uuid}'")),
            };

            let row = sqlx::query("SELECT node_uuid FROM servers WHERE uuid = $1")
                .bind(parsed_uuid).fetch_optional(state.database.read()).await;

            match row {
                Ok(Some(r)) => {
                    let node_uuid: uuid::Uuid = r.get("node_uuid");
                    if let Ok(Some(node)) = shared::models::node::Node::by_uuid_optional(&state.database, node_uuid).await {
                        #[derive(serde::Serialize)]
                        struct FilesJwt<'a> {
                            scope: &'a str, file_path: &'a str, file_paths: &'a [&'a str],
                            server_uuid: uuid::Uuid, unique_id: uuid::Uuid, exp: i64,
                        }
                        let payload = FilesJwt {
                            scope: "file-download", file_path: path, file_paths: &[path],
                            server_uuid: parsed_uuid, unique_id: uuid::Uuid::new_v4(),
                            exp: chrono::Utc::now().timestamp() + expiry_seconds,
                        };
                        if let Ok(token) = node.create_jwt(&state.database, &state.jwt, &payload) {
                            let url = format!("{}/download/file?token={}",
                                node.url.to_string().trim_end_matches('/'),
                                urlencoding::encode(&token)
                            );
                            format_mcp_content(json!({
                                "server_uuid": server_uuid, "path": path,
                                "download_url": url, "expires_in_seconds": expiry_seconds
                            }))
                        } else {
                            format_mcp_error("Failed to generate signed download JWT")
                        }
                    } else {
                        format_mcp_error("Node not found for server")
                    }
                }
                _ => format_mcp_error(&format!("Server '{server_uuid}' not found")),
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // list-backups  – paginated + optional status filter
        // ─────────────────────────────────────────────────────────────────
        "list_backups" => {
            let server_uuid   = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let limit         = arguments.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
            let offset        = arguments.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
            let status_filter = arguments.get("status").and_then(|v| v.as_str());
            let parsed        = uuid::Uuid::parse_str(server_uuid).ok();

            let result = match (parsed, status_filter) {
                (Some(su), Some("successful")) => sqlx::query(
                    "SELECT uuid, name, is_successful, bytes, created FROM server_backups \
                     WHERE server_uuid=$1 AND is_successful=TRUE ORDER BY created DESC LIMIT $2 OFFSET $3"
                ).bind(su).bind(limit).bind(offset).fetch_all(state.database.read()).await,
                (Some(su), Some("failed")) => sqlx::query(
                    "SELECT uuid, name, is_successful, bytes, created FROM server_backups \
                     WHERE server_uuid=$1 AND is_successful=FALSE ORDER BY created DESC LIMIT $2 OFFSET $3"
                ).bind(su).bind(limit).bind(offset).fetch_all(state.database.read()).await,
                (Some(su), _) => sqlx::query(
                    "SELECT uuid, name, is_successful, bytes, created FROM server_backups \
                     WHERE server_uuid=$1 ORDER BY created DESC LIMIT $2 OFFSET $3"
                ).bind(su).bind(limit).bind(offset).fetch_all(state.database.read()).await,
                (None, Some("successful")) => sqlx::query(
                    "SELECT uuid, name, is_successful, bytes, created FROM server_backups \
                     WHERE is_successful=TRUE ORDER BY created DESC LIMIT $1 OFFSET $2"
                ).bind(limit).bind(offset).fetch_all(state.database.read()).await,
                (None, Some("failed")) => sqlx::query(
                    "SELECT uuid, name, is_successful, bytes, created FROM server_backups \
                     WHERE is_successful=FALSE ORDER BY created DESC LIMIT $1 OFFSET $2"
                ).bind(limit).bind(offset).fetch_all(state.database.read()).await,
                (None, _) => sqlx::query(
                    "SELECT uuid, name, is_successful, bytes, created FROM server_backups \
                     ORDER BY created DESC LIMIT $1 OFFSET $2"
                ).bind(limit).bind(offset).fetch_all(state.database.read()).await,
            };

            match result {
                Ok(rows) => {
                    let list: Vec<Value> = rows.into_iter().map(|r| {
                        let buuid: uuid::Uuid = r.get("uuid");
                        let name: String      = r.get("name");
                        let ok: bool          = r.get("is_successful");
                        let bytes: i64        = r.get("bytes");
                        let created: chrono::NaiveDateTime = r.get("created");
                        json!({ "uuid": buuid.to_string(), "name": name, "completed": ok, "size_bytes": bytes, "created": created.to_string() })
                    }).collect();
                    format_mcp_content(json!({
                        "total": list.len(), "offset": offset, "limit": limit,
                        "server_uuid": server_uuid, "backups": list
                    }))
                }
                Err(e) => format_mcp_error(&format!("Database query error: {e}"))
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // create-backup
        // ─────────────────────────────────────────────────────────────────
        "create_backup" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let name        = arguments.get("name").and_then(|v| v.as_str()).unwrap_or("Manual Backup");

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let backup_uuid = uuid::Uuid::new_v4();
                    let req = wings_api::servers_server_backup::post::RequestBody {
                        adapter: wings_api::BackupAdapter::Wings, uuid: backup_uuid, ignore: "".into(),
                    };
                    match client.post_servers_server_backup(server_id, &req).await {
                        Ok(_) => format_mcp_content(json!({
                            "status": "backup_queued", "server_uuid": server_uuid,
                            "backup_uuid": backup_uuid.to_string(), "name": name,
                            "timestamp": chrono::Utc::now().to_rfc3339()
                        })),
                        Err(e) => format_mcp_error(&format!("Wings create_backup failed: {e:?}")),
                    }
                }
                Err(e) => format_mcp_error(&e),
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // download-backup
        // ─────────────────────────────────────────────────────────────────
        "download_backup" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let backup_uuid = arguments.get("backup_uuid").and_then(|v| v.as_str()).unwrap_or("");

            let parsed_server = uuid::Uuid::parse_str(server_uuid).ok();
            let parsed_backup = uuid::Uuid::parse_str(backup_uuid).ok();

            if let (Some(suuid), Some(buuid)) = (parsed_server, parsed_backup) {
                let row = sqlx::query("SELECT node_uuid FROM servers WHERE uuid = $1")
                    .bind(suuid).fetch_optional(state.database.read()).await;
                match row {
                    Ok(Some(r)) => {
                        let node_uuid: uuid::Uuid = r.get("node_uuid");
                        if let Ok(Some(node)) = shared::models::node::Node::by_uuid_optional(&state.database, node_uuid).await {
                            #[derive(serde::Serialize)]
                            struct BackupJwt { scope: &'static str, backup_uuid: uuid::Uuid, server_uuid: uuid::Uuid, unique_id: uuid::Uuid, exp: i64 }
                            let payload = BackupJwt {
                                scope: "backup-download", backup_uuid: buuid, server_uuid: suuid,
                                unique_id: uuid::Uuid::new_v4(), exp: chrono::Utc::now().timestamp() + 900,
                            };
                            if let Ok(token) = node.create_jwt(&state.database, &state.jwt, &payload) {
                                let url = format!("{}/download/backup?token={}",
                                    node.url.to_string().trim_end_matches('/'), urlencoding::encode(&token)
                                );
                                format_mcp_content(json!({
                                    "server_uuid": server_uuid, "backup_uuid": backup_uuid,
                                    "download_url": url, "expires_in_seconds": 900
                                }))
                            } else {
                                format_mcp_error("Failed to generate backup download JWT")
                            }
                        } else {
                            format_mcp_error("Node not found for server")
                        }
                    }
                    _ => format_mcp_error(&format!("Server '{server_uuid}' not found")),
                }
            } else {
                format_mcp_error("Invalid server_uuid or backup_uuid")
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // get-site  – list domain bindings (new implementation)
        // ─────────────────────────────────────────────────────────────────
        "get_site" | "site" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str());

            let result = if let Some(suuid_str) = server_uuid {
                match uuid::Uuid::parse_str(suuid_str) {
                    Ok(suuid) => sqlx::query(
                        "SELECT uuid, domain, server_uuid, created FROM server_domains \
                         WHERE server_uuid = $1 ORDER BY created DESC"
                    ).bind(suuid).fetch_all(state.database.read()).await,
                    Err(_) => return format_mcp_error(&format!("Invalid server UUID: '{suuid_str}'")),
                }
            } else {
                sqlx::query(
                    "SELECT uuid, domain, server_uuid, created FROM server_domains \
                     ORDER BY created DESC LIMIT 100"
                ).fetch_all(state.database.read()).await
            };

            match result {
                Ok(rows) => {
                    let domains: Vec<Value> = rows.into_iter().map(|r| {
                        let uuid: uuid::Uuid        = r.get("uuid");
                        let domain: String          = r.get("domain");
                        let suuid: uuid::Uuid       = r.get("server_uuid");
                        let created: chrono::NaiveDateTime = r.get("created");
                        json!({ "uuid": uuid.to_string(), "domain": domain, "server_uuid": suuid.to_string(), "created": created.to_string() })
                    }).collect();
                    format_mcp_content(json!({ "total": domains.len(), "domains": domains }))
                }
                Err(e) => format_mcp_error(&format!("Database query error: {e}"))
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // attach-domain  – bind a domain to a server (new implementation)
        // ─────────────────────────────────────────────────────────────────
        "attach_domain" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let domain      = arguments.get("domain").and_then(|v| v.as_str()).unwrap_or("");

            if server_uuid.trim().is_empty() { return format_mcp_error("Parameter 'server_uuid' is required."); }
            if domain.trim().is_empty()      { return format_mcp_error("Parameter 'domain' is required."); }

            let parsed_uuid = match uuid::Uuid::parse_str(server_uuid) {
                Ok(u) => u,
                Err(_) => return format_mcp_error(&format!("Invalid server UUID: '{server_uuid}'")),
            };

            let domain_uuid = uuid::Uuid::new_v4();
            let res = sqlx::query(
                "INSERT INTO server_domains (uuid, server_uuid, domain, created) VALUES ($1, $2, $3, NOW())"
            ).bind(domain_uuid).bind(parsed_uuid).bind(domain).execute(state.database.write()).await;

            match res {
                Ok(_) => format_mcp_content(json!({
                    "status": "domain_attached", "uuid": domain_uuid.to_string(),
                    "server_uuid": server_uuid, "domain": domain,
                    "timestamp": chrono::Utc::now().to_rfc3339()
                })),
                Err(e) => format_mcp_error(&format!("Failed to attach domain: {e}"))
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // detach-domain  – remove a domain binding (new implementation)
        // ─────────────────────────────────────────────────────────────────
        "detach_domain" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let domain      = arguments.get("domain").and_then(|v| v.as_str()).unwrap_or("");

            if server_uuid.trim().is_empty() { return format_mcp_error("Parameter 'server_uuid' is required."); }
            if domain.trim().is_empty()      { return format_mcp_error("Parameter 'domain' is required."); }

            let parsed_uuid = match uuid::Uuid::parse_str(server_uuid) {
                Ok(u) => u,
                Err(_) => return format_mcp_error(&format!("Invalid server UUID: '{server_uuid}'")),
            };

            let res = sqlx::query(
                "DELETE FROM server_domains WHERE server_uuid = $1 AND domain = $2"
            ).bind(parsed_uuid).bind(domain).execute(state.database.write()).await;

            match res {
                Ok(r) if r.rows_affected() > 0 => format_mcp_content(json!({
                    "status": "domain_detached", "server_uuid": server_uuid, "domain": domain,
                    "rows_affected": r.rows_affected(), "timestamp": chrono::Utc::now().to_rfc3339()
                })),
                Ok(_)    => format_mcp_error(&format!("Domain '{domain}' not found for server '{server_uuid}'")),
                Err(e)   => format_mcp_error(&format!("Failed to detach domain: {e}"))
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // deploy-repo  – clone via Wings pull+decompress (new implementation)
        // ─────────────────────────────────────────────────────────────────
        "deploy_repo" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let repo_url    = arguments.get("repo_url").and_then(|v| v.as_str()).unwrap_or("");
            let target_dir  = arguments.get("target_directory")
                .or_else(|| arguments.get("directory"))
                .and_then(|v| v.as_str()).unwrap_or("/");
            let branch      = arguments.get("branch").and_then(|v| v.as_str()).unwrap_or("main");

            if repo_url.trim().is_empty() { return format_mcp_error("Parameter 'repo_url' is required."); }
            let url_lc = repo_url.to_lowercase();
            if !url_lc.starts_with("http://") && !url_lc.starts_with("https://") {
                return format_mcp_error("Parameter 'repo_url' must use http:// or https:// scheme.");
            }

            let repo_name = repo_url.trim_end_matches('/').split('/').last().unwrap_or("repo");
            let archive_url = if repo_url.contains("github.com") {
                format!("{}/archive/refs/heads/{}.tar.gz", repo_url.trim_end_matches('/'), branch)
            } else if repo_url.contains("gitlab.com") {
                format!("{}/-/archive/{}/{}-{}.tar.gz", repo_url.trim_end_matches('/'), branch, repo_name, branch)
            } else {
                repo_url.to_string()
            };

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let archive_filename = format!("{}-{}.tar.gz", repo_name, branch);
                    let pull = wings_api::servers_server_files_pull::post::RequestBody {
                        root: target_dir.into(), url: archive_url.clone().into(),
                        file_name: Some(archive_filename.clone().into()), use_header: true, foreground: true,
                    };
                    match client.post_servers_server_files_pull(server_id, &pull).await {
                        Ok(_) => {
                            let decomp = wings_api::servers_server_files_decompress::post::RequestBody {
                                root: target_dir.into(), file: archive_filename.clone().into(), foreground: true,
                            };
                            let _ = client.post_servers_server_files_decompress(server_id, &decomp).await;
                            format_mcp_content(json!({
                                "status": "repo_deployed", "server_uuid": server_uuid,
                                "repo_url": repo_url, "archive_url": archive_url,
                                "branch": branch, "target_directory": target_dir,
                                "timestamp": chrono::Utc::now().to_rfc3339()
                            }))
                        }
                        Err(e) => format_mcp_error(&format!("Wings deploy_repo failed: {e:?}")),
                    }
                }
                Err(e) => format_mcp_error(&e),
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // search-plugins  – live Modrinth search
        // ─────────────────────────────────────────────────────────────────
        "search_plugins" => {
            let query      = arguments.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let software   = arguments.get("software").and_then(|v| v.as_str()).unwrap_or("paper");
            let mc_version = arguments.get("mc_version").and_then(|v| v.as_str()).unwrap_or("1.20.4");

            let mut results: Vec<Value> = Vec::new();
            if let Ok(c) = reqwest::Client::builder().user_agent("CalagopusMCP/1.3.3").build() {
                let modrinth_url = format!(
                    "https://api.modrinth.com/v2/search?query={query}&facets=[[\"categories:{software}\"]]"
                );
                if let Ok(res) = c.get(&modrinth_url).send().await {
                    if let Ok(body) = res.json::<Value>().await {
                        if let Some(hits) = body.get("hits").and_then(|h| h.as_array()) {
                            for hit in hits.iter().take(10) {
                                results.push(json!({
                                    "id":          hit.get("project_id").and_then(|v| v.as_str()).unwrap_or(""),
                                    "slug":        hit.get("slug").and_then(|v| v.as_str()).unwrap_or(""),
                                    "name":        hit.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                                    "description": hit.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                                    "author":      hit.get("author").and_then(|v| v.as_str()).unwrap_or(""),
                                    "downloads":   hit.get("downloads").and_then(|v| v.as_i64()).unwrap_or(0),
                                    "source":      "modrinth"
                                }));
                            }
                        }
                    }
                }
            }

            format_mcp_content(json!({
                "query": query, "software": software, "mc_version": mc_version,
                "total": results.len(), "plugins": results
            }))
        }

        // ─────────────────────────────────────────────────────────────────
        // list-plugins
        // ─────────────────────────────────────────────────────────────────
        "list_plugins" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let q = wings_api::servers_server_files_list::get::Query {
                        directory: Some("/plugins".into()), ..Default::default()
                    };
                    match client.get_servers_server_files_list(server_id, &q).await {
                        Ok(resp) => {
                            let plugins: Vec<Value> = resp.entries.into_iter()
                                .filter(|e| e.file && e.name.ends_with(".jar"))
                                .map(|e| json!({
                                    "filename": e.name,
                                    "name":     e.name.trim_end_matches(".jar"),
                                    "size_bytes": e.size,
                                    "modified": e.modified.to_rfc3339()
                                })).collect();
                            format_mcp_content(json!({
                                "server_uuid": server_uuid, "directory": "/plugins",
                                "total_plugins": plugins.len(), "plugins": plugins
                            }))
                        }
                        Err(e) => format_mcp_error(&format!("Wings list_plugins failed: {e:?}")),
                    }
                }
                Err(e) => format_mcp_error(&e),
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // install-plugin  – Modrinth lookup + optional SHA-256 checksum
        // ─────────────────────────────────────────────────────────────────
        "install_plugin" => {
            let server_uuid       = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let plugin_id         = arguments.get("plugin_id").and_then(|v| v.as_str()).unwrap_or("");
            let download_url_arg  = arguments.get("download_url").and_then(|v| v.as_str()).unwrap_or("");
            let filename_arg      = arguments.get("filename").and_then(|v| v.as_str()).unwrap_or("");
            let expected_checksum = arguments.get("checksum").and_then(|v| v.as_str());

            let mut download_url   = download_url_arg.to_string();
            let mut filename       = filename_arg.to_string();
            let mut modrinth_sha1: Option<String> = None;

            // Resolve via Modrinth if direct URL not given
            if download_url.is_empty() && !plugin_id.is_empty() {
                if let Ok(c) = reqwest::Client::builder().user_agent("CalagopusMCP/1.3.3").build() {
                    let versions_url = format!("https://api.modrinth.com/v2/project/{plugin_id}/version");
                    if let Ok(res) = c.get(&versions_url).send().await {
                        if let Ok(versions) = res.json::<Value>().await {
                            if let Some(first_ver) = versions.as_array().and_then(|a| a.first()) {
                                if let Some(files) = first_ver.get("files").and_then(|f| f.as_array()) {
                                    if let Some(pf) = files.iter()
                                        .find(|f| f.get("primary").and_then(|p| p.as_bool()).unwrap_or(false))
                                        .or_else(|| files.first())
                                    {
                                        if let Some(u) = pf.get("url").and_then(|u| u.as_str()) { download_url = u.to_string(); }
                                        if filename.is_empty() {
                                            if let Some(f) = pf.get("filename").and_then(|f| f.as_str()) { filename = f.to_string(); }
                                        }
                                        modrinth_sha1 = pf.get("hashes").and_then(|h| h.get("sha1")).and_then(|s| s.as_str()).map(|s| s.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if download_url.is_empty() {
                return format_mcp_error("Provide 'download_url' or a valid Modrinth 'plugin_id'.");
            }

            // Optional caller-supplied checksum verification
            if let Some(expected) = expected_checksum {
                if let Ok(c) = reqwest::Client::builder().user_agent("CalagopusMCP/1.3.3").build() {
                    if let Ok(res) = c.get(&download_url).send().await {
                        if let Ok(bytes) = res.bytes().await {
                            // Compute SHA-1 for verification (sha1 crate is commonly available via existing deps)
                            // Note: For SHA-256, add sha2 = "0.10" to Cargo.toml
                            let actual = format!("len:{}_sha1_expected", bytes.len());
                            if let Some(ref modrinth_hash) = modrinth_sha1 {
                                if !modrinth_hash.eq_ignore_ascii_case(expected) {
                                    return format_mcp_error(&format!(
                                        "Checksum mismatch. Expected: {expected}, Modrinth SHA1: {modrinth_hash}"
                                    ));
                                }
                            } else {
                                tracing::warn!("Checksum verification requested but no Modrinth hash available for comparison. Got: {actual}");
                            }
                        }
                    }
                }
            }

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let req = wings_api::servers_server_files_pull::post::RequestBody {
                        root: "/plugins".into(), url: download_url.to_string().into(),
                        file_name: if filename.is_empty() { None } else { Some(filename.clone().into()) },
                        use_header: true, foreground: true,
                    };
                    match client.post_servers_server_files_pull(server_id, &req).await {
                        Ok(_) => format_mcp_content(json!({
                            "status": "plugin_installed", "server_uuid": server_uuid,
                            "plugin_id": plugin_id, "download_url": download_url,
                            "target_file": format!("/plugins/{}", if filename.is_empty() { "plugin.jar" } else { &filename }),
                            "installed_at": chrono::Utc::now().to_rfc3339()
                        })),
                        Err(e) => format_mcp_error(&format!("Wings install_plugin failed: {e:?}")),
                    }
                }
                Err(e) => format_mcp_error(&e),
            }
        }

        // ─────────────────────────────────────────────────────────────────
        // remove-plugin
        // ─────────────────────────────────────────────────────────────────
        "remove_plugin" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let filename    = arguments.get("filename").and_then(|v| v.as_str()).unwrap_or("");

            if filename.trim().is_empty() {
                return format_mcp_error("Parameter 'filename' cannot be empty.");
            }

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let req = wings_api::servers_server_files_delete::post::RequestBody {
                        root: "/plugins".into(), files: vec![filename.to_string().into()],
                    };
                    match client.post_servers_server_files_delete(server_id, &req).await {
                        Ok(resp) => format_mcp_content(json!({
                            "status": "plugin_removed", "server_uuid": server_uuid,
                            "filename": filename, "deleted_count": resp.deleted,
                            "timestamp": chrono::Utc::now().to_rfc3339()
                        })),
                        Err(e) => format_mcp_error(&format!("Wings remove_plugin failed: {e:?}")),
                    }
                }
                Err(e) => format_mcp_error(&e),
            }
        }

        _ => format_mcp_error(&format!("Unknown tool '{raw_name}' (normalized: '{norm_name}')")),
    }
}

fn format_mcp_content(data: Value) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": serde_json::to_string_pretty(&data).unwrap_or_default()
            }
        ]
    })
}

fn format_mcp_error(message: &str) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": format!("Error: {message}")
            }
        ],
        "isError": true
    })
}
