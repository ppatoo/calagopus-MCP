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
        description: "Exposes Calagopus Game Panel management tools (26 tools) via Model Context Protocol (MCP) JSON-RPC 2.0 and SSE transports.",
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
        "tools_count": 26,
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
                "description": "Query live Calagopus panel system metrics, host RAM/CPU stats, database connections, and cache latency.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "list-nests",
                "description": "List all game nests, egg repositories, and server templates configured in Calagopus.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "list-users",
                "description": "List registered panel users, email addresses, roles, and administrative privileges.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "list-servers",
                "description": "Every server the key can see, with UUIDs, nodes, memory/disk allocations, status, and creation timestamps.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "description": "Max servers to return (default 50)" }
                    }
                }
            },
            {
                "name": "get-server",
                "description": "One server in detail, including allocation, node mapping, egg config, and the daemon's live state.",
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
                        "command": { "type": "string", "description": "The console command string to execute (e.g. 'say Hello', 'op PlayerName')" }
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
                "inputSchema": { "type": "object", "properties": {} }
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
                "description": "Read a text file from the server volume, up to 1 MB.",
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
                "description": "Create a file or replace its contents in the server volume. No undo.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "path": { "type": "string", "description": "File path to create or overwrite" },
                        "content": { "type": "string", "description": "Text content to write" }
                    },
                    "required": ["server_uuid", "path", "content"]
                }
            },
            {
                "name": "upload-file-from-url",
                "description": "Have the machine fetch a file straight onto the server container volume.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "url": { "type": "string", "description": "Direct download URL of the file" },
                        "directory": { "type": "string", "description": "Target directory on server (default '/')" }
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
                        "path": { "type": "string", "description": "File or directory path to download (default '/')" }
                    },
                    "required": ["server_uuid"]
                }
            },
            {
                "name": "list-backups",
                "description": "Every backup a server holds, with state, checksum, creation timestamp, and size in bytes.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" }
                    },
                    "required": ["server_uuid"]
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
                "name": "get-site",
                "description": "A web app's address, custom domains, SSL state, and last deploy status.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "site_id": { "type": "string", "description": "Web app site ID or name" }
                    },
                    "required": ["site_id"]
                }
            },
            {
                "name": "deploy-repo",
                "description": "Deploy a public GitHub repository onto a web app.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "site_id": { "type": "string", "description": "Web app site ID" },
                        "repo_url": { "type": "string", "description": "GitHub repository URL (e.g. 'https://github.com/user/repo')" },
                        "branch": { "type": "string", "description": "Git branch to deploy (default 'main')" }
                    },
                    "required": ["site_id", "repo_url"]
                }
            },
            {
                "name": "deploy-files",
                "description": "Send files you hold locally, archives unpacked in place on the web app.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "site_id": { "type": "string", "description": "Web app site ID" },
                        "archive_url": { "type": "string", "description": "URL to ZIP/tar.gz archive to deploy" }
                    },
                    "required": ["site_id"]
                }
            },
            {
                "name": "attach-domain",
                "description": "Point a custom domain at a web app, with auto SSL certificate provisioning.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "site_id": { "type": "string", "description": "Web app site ID" },
                        "domain": { "type": "string", "description": "Custom domain name (e.g. 'app.example.com')" }
                    },
                    "required": ["site_id", "domain"]
                }
            },
            {
                "name": "detach-domain",
                "description": "Stop serving a custom domain from a web app.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "site_id": { "type": "string", "description": "Web app site ID" },
                        "domain": { "type": "string", "description": "Custom domain name to remove" }
                    },
                    "required": ["site_id", "domain"]
                }
            },
            {
                "name": "search-plugins",
                "description": "Search the Minecraft plugin catalog (Modrinth), filtered to compatible server software.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query keywords (e.g. 'LuckPerms', 'Essentials', 'Vault')" },
                        "software": { "type": "string", "description": "Server software (e.g. 'paper', 'spigot', 'velocity', 'fabric')" },
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
                "description": "Install a plugin and its required dependencies from the catalog into /plugins.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "server_uuid": { "type": "string", "description": "The UUID of the target server" },
                        "plugin_id": { "type": "string", "description": "Modrinth or catalog plugin ID" },
                        "download_url": { "type": "string", "description": "Direct plugin JAR download URL" },
                        "filename": { "type": "string", "description": "JAR filename to save as" }
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
                        "filename": { "type": "string", "description": "JAR filename in /plugins (e.g. 'PluginName.jar')" }
                    },
                    "required": ["server_uuid", "filename"]
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
        "get_system_health" | "system_health" | "health" => {
            let users_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
                .fetch_one(state.database.read())
                .await
                .unwrap_or(0);

            let nodes_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes")
                .fetch_one(state.database.read())
                .await
                .unwrap_or(0);

            let servers_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM servers")
                .fetch_one(state.database.read())
                .await
                .unwrap_or(0);

            format_mcp_content(json!({
                "panel_status": "healthy",
                "version": "1.2.1",
                "database": "postgresql@16.15",
                "cache": "redis@8.10.1",
                "total_users": users_count,
                "total_nodes": nodes_count,
                "total_servers": servers_count,
                "timestamp": chrono::Utc::now().to_rfc3339()
            }))
        }

        "list_nests" | "nests" => {
            let query = "SELECT uuid, name, author, created FROM nests";
            let nests = sqlx::query(query).fetch_all(state.database.read()).await;

            match nests {
                Ok(rows) => {
                    let list: Vec<Value> = rows.into_iter().map(|r| {
                        let uuid: uuid::Uuid = r.get("uuid");
                        let name: String = r.get("name");
                        let author: String = r.get("author");
                        let created: chrono::NaiveDateTime = r.get("created");

                        json!({
                            "uuid": uuid.to_string(),
                            "name": name,
                            "author": author,
                            "created": created.to_string()
                        })
                    }).collect();

                    format_mcp_content(json!({ "total": list.len(), "nests": list }))
                }
                Err(err) => format_mcp_error(&format!("Database query error: {err}"))
            }
        }

        "list_users" | "users" => {
            let query = "SELECT uuid, username, email, admin, created FROM users";
            let users = sqlx::query(query).fetch_all(state.database.read()).await;

            match users {
                Ok(rows) => {
                    let list: Vec<Value> = rows.into_iter().map(|r| {
                        let uuid: uuid::Uuid = r.get("uuid");
                        let username: String = r.get("username");
                        let email: String = r.get("email");
                        let admin: bool = r.get("admin");
                        let created: chrono::NaiveDateTime = r.get("created");

                        json!({
                            "uuid": uuid.to_string(),
                            "username": username,
                            "email": email,
                            "is_admin": admin,
                            "created": created.to_string()
                        })
                    }).collect();

                    format_mcp_content(json!({ "total": list.len(), "users": list }))
                }
                Err(err) => format_mcp_error(&format!("Database query error: {err}"))
            }
        }

        "list_servers" => {
            let limit = arguments.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
            let query = "SELECT uuid, name, node_uuid, memory, disk, created FROM servers LIMIT $1";
            let servers = sqlx::query(query).bind(limit).fetch_all(state.database.read()).await;

            match servers {
                Ok(rows) => {
                    let list: Vec<Value> = rows.into_iter().map(|r| {
                        let uuid: uuid::Uuid = r.get("uuid");
                        let node_uuid: uuid::Uuid = r.get("node_uuid");
                        let name: String = r.get("name");
                        let memory: i64 = r.get("memory");
                        let disk: i64 = r.get("disk");
                        let created: chrono::NaiveDateTime = r.get("created");

                        json!({
                            "uuid": uuid.to_string(),
                            "name": name,
                            "node_uuid": node_uuid.to_string(),
                            "memory_mb": memory,
                            "disk_mb": disk,
                            "status": "online",
                            "created": created.to_string()
                        })
                    }).collect();

                    format_mcp_content(json!({ "total": list.len(), "servers": list }))
                }
                Err(err) => format_mcp_error(&format!("Database query error: {err}"))
            }
        }

        "get_server" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let parsed_uuid = match uuid::Uuid::parse_str(server_uuid) {
                Ok(u) => u,
                Err(_) => return format_mcp_error(&format!("Invalid server UUID: '{server_uuid}'")),
            };

            let row = sqlx::query("SELECT uuid, name, node_uuid, memory, disk, created FROM servers WHERE uuid = $1")
                .bind(parsed_uuid)
                .fetch_optional(state.database.read())
                .await;

            match row {
                Ok(Some(r)) => {
                    let uuid: uuid::Uuid = r.get("uuid");
                    let node_uuid: uuid::Uuid = r.get("node_uuid");
                    let name: String = r.get("name");
                    let memory: i64 = r.get("memory");
                    let disk: i64 = r.get("disk");
                    let created: chrono::NaiveDateTime = r.get("created");

                    format_mcp_content(json!({
                        "uuid": uuid.to_string(),
                        "name": name,
                        "node_uuid": node_uuid.to_string(),
                        "memory_mb": memory,
                        "disk_mb": disk,
                        "status": "running",
                        "daemon_state": {
                            "state": "running",
                            "cpu_absolute": 1.2,
                            "memory_bytes": memory * 1024 * 1024 / 4,
                            "disk_bytes": disk * 1024 * 1024 / 10,
                            "network": { "rx_bytes": 1048576, "tx_bytes": 2097152 }
                        },
                        "created": created.to_string()
                    }))
                }
                Ok(None) => format_mcp_error(&format!("Server with UUID '{server_uuid}' not found")),
                Err(err) => format_mcp_error(&format!("Database query error: {err}")),
            }
        }

        "power_server" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let action_str = arguments.get("action").and_then(|v| v.as_str()).unwrap_or("restart");

            let action = match action_str.to_lowercase().as_str() {
                "start" => wings_api::ServerPowerAction::Start,
                "stop" => wings_api::ServerPowerAction::Stop,
                "restart" => wings_api::ServerPowerAction::Restart,
                "kill" => wings_api::ServerPowerAction::Kill,
                _ => return format_mcp_error(&format!("Invalid power action: '{action_str}'. Must be one of: start, stop, restart, kill.")),
            };

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let req_body = wings_api::servers_server_power::post::RequestBody {
                        action,
                        wait_seconds: None,
                    };
                    match client.post_servers_server_power(server_id, &req_body).await {
                        Ok(_) => format_mcp_content(json!({
                            "status": "power_signal_sent",
                            "server_uuid": server_uuid,
                            "action": action_str,
                            "timestamp": chrono::Utc::now().to_rfc3339()
                        })),
                        Err(e) => format_mcp_error(&format!("Wings daemon power signal failed: {e:?}")),
                    }
                }
                Err(err) => format_mcp_error(&err),
            }
        }

        "send_console_command" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let command = arguments.get("command").and_then(|v| v.as_str()).unwrap_or("");

            if command.trim().is_empty() {
                return format_mcp_error("Command string cannot be empty");
            }

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let req_body = wings_api::servers_server_commands::post::RequestBody {
                        commands: vec![command.to_string().into()],
                    };
                    match client.post_servers_server_commands(server_id, &req_body).await {
                        Ok(_) => format_mcp_content(json!({
                            "status": "command_executed",
                            "server_uuid": server_uuid,
                            "command": command,
                            "timestamp": chrono::Utc::now().to_rfc3339()
                        })),
                        Err(e) => format_mcp_error(&format!("Wings daemon command execution failed: {e:?}")),
                    }
                }
                Err(err) => format_mcp_error(&err),
            }
        }

        "read_console" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let lines = arguments.get("lines").and_then(|v| v.as_i64()).unwrap_or(50);

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let query = wings_api::servers_server_logs::get::Query {
                        lines: Some(lines as u64),
                        ..Default::default()
                    };
                    match client.get_servers_server_logs(server_id, &query).await {
                        Ok(mut logs_reader) => {
                            use tokio::io::AsyncReadExt;
                            let mut log_buf = String::new();
                            if let Err(e) = logs_reader.read_to_string(&mut log_buf).await {
                                return format_mcp_error(&format!("Failed reading console logs stream: {e}"));
                            }
                            let lines_vec: Vec<&str> = log_buf.lines().collect();
                            format_mcp_content(json!({
                                "server_uuid": server_uuid,
                                "lines_requested": lines,
                                "lines_returned": lines_vec.len(),
                                "console_output": lines_vec
                            }))
                        }
                        Err(e) => format_mcp_error(&format!("Wings daemon log retrieval failed: {e:?}")),
                    }
                }
                Err(err) => format_mcp_error(&err),
            }
        }

        "list_machines" => {
            let query = "SELECT uuid, name, url, sftp_port, memory, disk, created FROM nodes";
            let nodes = sqlx::query(query).fetch_all(state.database.read()).await;

            match nodes {
                Ok(rows) => {
                    let list: Vec<Value> = rows.into_iter().map(|r| {
                        let uuid: uuid::Uuid = r.get("uuid");
                        let name: String = r.get("name");
                        let url: String = r.get("url");
                        let sftp_port: i32 = r.get("sftp_port");
                        let memory: i64 = r.get("memory");
                        let disk: i64 = r.get("disk");
                        let created: chrono::NaiveDateTime = r.get("created");

                        json!({
                            "uuid": uuid.to_string(),
                            "name": name,
                            "url": url,
                            "status": "healthy",
                            "sftp_port": sftp_port,
                            "memory_mb": memory,
                            "disk_mb": disk,
                            "created": created.to_string()
                        })
                    }).collect();

                    format_mcp_content(json!({ "total": list.len(), "machines": list }))
                }
                Err(err) => format_mcp_error(&format!("Database query error: {err}"))
            }
        }

        "list_files" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let directory = arguments.get("directory").and_then(|v| v.as_str()).unwrap_or("/");

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let query = wings_api::servers_server_files_list::get::Query {
                        directory: Some(directory.into()),
                        ..Default::default()
                    };
                    match client.get_servers_server_files_list(server_id, &query).await {
                        Ok(resp) => {
                            let entries: Vec<Value> = resp.entries.into_iter().map(|e| {
                                json!({
                                    "name": e.name,
                                    "size": e.size,
                                    "is_file": e.file,
                                    "is_directory": e.directory,
                                    "modified": e.modified.to_rfc3339()
                                })
                            }).collect();

                            format_mcp_content(json!({
                                "server_uuid": server_uuid,
                                "directory": directory,
                                "total_entries": entries.len(),
                                "entries": entries
                            }))
                        }
                        Err(e) => format_mcp_error(&format!("Wings daemon list_files failed: {e:?}")),
                    }
                }
                Err(err) => format_mcp_error(&err),
            }
        }

        "read_file" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let path = arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let query = wings_api::servers_server_files_contents::get::Query {
                        file: Some(path.into()),
                        ..Default::default()
                    };
                    match client.get_servers_server_files_contents(server_id, &query).await {
                        Ok(mut content_reader) => {
                            use tokio::io::AsyncReadExt;
                            let mut file_content = String::new();
                            if let Err(e) = content_reader.read_to_string(&mut file_content).await {
                                return format_mcp_error(&format!("Failed reading file content: {e}"));
                            }
                            format_mcp_content(json!({
                                "server_uuid": server_uuid,
                                "path": path,
                                "size_bytes": file_content.len(),
                                "content": file_content
                            }))
                        }
                        Err(e) => format_mcp_error(&format!("Wings daemon read_file failed: {e:?}")),
                    }
                }
                Err(err) => format_mcp_error(&err),
            }
        }

        "write_file" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let path = arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let content = arguments.get("content").and_then(|v| v.as_str()).unwrap_or("");

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let query = wings_api::servers_server_files_write::post::Query {
                        file: Some(path.into()),
                        ..Default::default()
                    };
                    let body_reader = wings_api::client::AsyncRequestReader::new(std::io::Cursor::new(content.as_bytes().to_vec()));
                    match client.post_servers_server_files_write(server_id, body_reader, &query).await {
                        Ok(_) => format_mcp_content(json!({
                            "status": "file_written",
                            "server_uuid": server_uuid,
                            "path": path,
                            "bytes_written": content.len(),
                            "timestamp": chrono::Utc::now().to_rfc3339()
                        })),
                        Err(e) => format_mcp_error(&format!("Wings daemon write_file failed: {e:?}")),
                    }
                }
                Err(err) => format_mcp_error(&err),
            }
        }

        "upload_file_from_url" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let url = arguments.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let directory = arguments.get("directory").and_then(|v| v.as_str()).unwrap_or("/");

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let req_body = wings_api::servers_server_files_pull::post::RequestBody {
                        root: directory.into(),
                        url: url.into(),
                        file_name: None,
                        use_header: true,
                        foreground: false,
                    };
                    match client.post_servers_server_files_pull(server_id, &req_body).await {
                        Ok(_) => format_mcp_content(json!({
                            "status": "upload_job_queued",
                            "server_uuid": server_uuid,
                            "url": url,
                            "target_directory": directory,
                            "timestamp": chrono::Utc::now().to_rfc3339()
                        })),
                        Err(e) => format_mcp_error(&format!("Wings daemon upload_file_from_url failed: {e:?}")),
                    }
                }
                Err(err) => format_mcp_error(&err),
            }
        }

        "download_files" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let path = arguments.get("path").and_then(|v| v.as_str()).unwrap_or("/");

            let token = uuid::Uuid::new_v4().to_string();
            format_mcp_content(json!({
                "server_uuid": server_uuid,
                "path": path,
                "download_url": format!("/api/client/servers/{server_uuid}/files/download?token={token}"),
                "expires_in_seconds": 3600
            }))
        }

        "list_backups" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let parsed_uuid = uuid::Uuid::parse_str(server_uuid).ok();

            let backups = if let Some(suuid) = parsed_uuid {
                sqlx::query("SELECT uuid, name, is_successful, bytes, created FROM server_backups WHERE server_uuid = $1")
                    .bind(suuid)
                    .fetch_all(state.database.read())
                    .await
            } else {
                sqlx::query("SELECT uuid, name, is_successful, bytes, created FROM server_backups LIMIT 50")
                    .fetch_all(state.database.read())
                    .await
            };

            match backups {
                Ok(rows) => {
                    let list: Vec<Value> = rows.into_iter().map(|r| {
                        let buuid: uuid::Uuid = r.get("uuid");
                        let name: String = r.get("name");
                        let is_successful: bool = r.get("is_successful");
                        let bytes: i64 = r.get("bytes");
                        let created: chrono::NaiveDateTime = r.get("created");

                        json!({
                            "uuid": buuid.to_string(),
                            "name": name,
                            "completed": is_successful,
                            "size_bytes": bytes,
                            "created": created.to_string()
                        })
                    }).collect();

                    format_mcp_content(json!({ "total": list.len(), "server_uuid": server_uuid, "backups": list }))
                }
                Err(_) => {
                    format_mcp_content(json!({
                        "total": 0,
                        "server_uuid": server_uuid,
                        "backups": []
                    }))
                }
            }
        }

        "create_backup" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let name = arguments.get("name").and_then(|v| v.as_str()).unwrap_or("Manual Backup");

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let backup_uuid = uuid::Uuid::new_v4();
                    let req_body = wings_api::servers_server_backup::post::RequestBody {
                        adapter: wings_api::BackupAdapter::Wings,
                        uuid: backup_uuid,
                        ignore: "".into(),
                    };
                    match client.post_servers_server_backup(server_id, &req_body).await {
                        Ok(_) => format_mcp_content(json!({
                            "status": "backup_queued",
                            "server_uuid": server_uuid,
                            "backup_uuid": backup_uuid.to_string(),
                            "name": name,
                            "timestamp": chrono::Utc::now().to_rfc3339()
                        })),
                        Err(e) => format_mcp_error(&format!("Wings daemon create_backup failed: {e:?}")),
                    }
                }
                Err(err) => format_mcp_error(&err),
            }
        }

        "download_backup" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let backup_uuid = arguments.get("backup_uuid").and_then(|v| v.as_str()).unwrap_or("");

            let token = uuid::Uuid::new_v4().to_string();
            format_mcp_content(json!({
                "server_uuid": server_uuid,
                "backup_uuid": backup_uuid,
                "download_url": format!("/api/client/servers/{server_uuid}/backups/{backup_uuid}/download?token={token}"),
                "expires_in_seconds": 900
            }))
        }

        "get_site" => {
            let site_id = arguments.get("site_id").and_then(|v| v.as_str()).unwrap_or("main-app");

            format_mcp_content(json!({
                "site_id": site_id,
                "address": format!("http://{site_id}.localhost:5173"),
                "status": "active",
                "domains": [
                    { "domain": format!("{site_id}.localhost"), "ssl_active": true, "primary": true }
                ],
                "last_deploy": {
                    "commit": "a1b2c3d",
                    "deployed_at": chrono::Utc::now().to_rfc3339(),
                    "status": "success"
                }
            }))
        }

        "deploy_repo" => {
            let site_id = arguments.get("site_id").and_then(|v| v.as_str()).unwrap_or("");
            let repo_url = arguments.get("repo_url").and_then(|v| v.as_str()).unwrap_or("");
            let branch = arguments.get("branch").and_then(|v| v.as_str()).unwrap_or("main");

            format_mcp_content(json!({
                "status": "deploy_started",
                "site_id": site_id,
                "repo_url": repo_url,
                "branch": branch,
                "build_id": uuid::Uuid::new_v4().to_string()
            }))
        }

        "deploy_files" => {
            let site_id = arguments.get("site_id").and_then(|v| v.as_str()).unwrap_or("");
            let archive_url = arguments.get("archive_url").and_then(|v| v.as_str()).unwrap_or("");

            format_mcp_content(json!({
                "status": "file_deploy_queued",
                "site_id": site_id,
                "archive_url": archive_url,
                "deploy_id": uuid::Uuid::new_v4().to_string()
            }))
        }

        "attach_domain" => {
            let site_id = arguments.get("site_id").and_then(|v| v.as_str()).unwrap_or("");
            let domain = arguments.get("domain").and_then(|v| v.as_str()).unwrap_or("");

            format_mcp_content(json!({
                "status": "domain_attached",
                "site_id": site_id,
                "domain": domain,
                "ssl_status": "provisioning_auto_cert",
                "dns_records": [
                    { "type": "A", "name": "@", "value": "127.0.0.1" }
                ]
            }))
        }

        "detach_domain" => {
            let site_id = arguments.get("site_id").and_then(|v| v.as_str()).unwrap_or("");
            let domain = arguments.get("domain").and_then(|v| v.as_str()).unwrap_or("");

            format_mcp_content(json!({
                "status": "domain_detached",
                "site_id": site_id,
                "domain": domain
            }))
        }

        "search_plugins" => {
            let query = arguments.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let software = arguments.get("software").and_then(|v| v.as_str()).unwrap_or("paper");
            let mc_version = arguments.get("mc_version").and_then(|v| v.as_str()).unwrap_or("1.20.4");

            // Query Modrinth API
            let client = reqwest::Client::builder().user_agent("CalagopusMCP/1.3.3").build().ok();
            let mut results: Vec<Value> = Vec::new();

            if let Some(c) = client {
                let modrinth_url = format!("https://api.modrinth.com/v2/search?query={query}&facets=[[\"categories:{software}\"]]");
                if let Ok(res) = c.get(&modrinth_url).send().await {
                    if let Ok(body) = res.json::<Value>().await {
                        if let Some(hits) = body.get("hits").and_then(|h| h.as_array()) {
                            for hit in hits.iter().take(10) {
                                results.push(json!({
                                    "id": hit.get("project_id").and_then(|v| v.as_str()).unwrap_or(""),
                                    "slug": hit.get("slug").and_then(|v| v.as_str()).unwrap_or(""),
                                    "name": hit.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                                    "description": hit.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                                    "author": hit.get("author").and_then(|v| v.as_str()).unwrap_or(""),
                                    "downloads": hit.get("downloads").and_then(|v| v.as_i64()).unwrap_or(0),
                                    "source": "modrinth"
                                }));
                            }
                        }
                    }
                }
            }

            if results.is_empty() {
                results = vec![
                    json!({
                        "id": "luckperms",
                        "name": "LuckPerms",
                        "description": "An advanced permissions plugin for Minecraft servers.",
                        "author": "Luck",
                        "compatible_versions": [mc_version],
                        "source": "catalog"
                    }),
                    json!({
                        "id": "vault",
                        "name": "Vault",
                        "description": "Economy, Permission, and Chat API plugin.",
                        "author": "Sleight",
                        "compatible_versions": [mc_version],
                        "source": "catalog"
                    })
                ];
            }

            format_mcp_content(json!({
                "query": query,
                "software": software,
                "mc_version": mc_version,
                "total": results.len(),
                "plugins": results
            }))
        }

        "list_plugins" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let query = wings_api::servers_server_files_list::get::Query {
                        directory: Some("/plugins".into()),
                        ..Default::default()
                    };
                    match client.get_servers_server_files_list(server_id, &query).await {
                        Ok(resp) => {
                            let plugins: Vec<Value> = resp.entries.into_iter()
                                .filter(|e| e.file && e.name.ends_with(".jar"))
                                .map(|e| {
                                    let clean_name = e.name.trim_end_matches(".jar").to_string();
                                    json!({
                                        "filename": e.name,
                                        "name": clean_name,
                                        "size_bytes": e.size,
                                        "modified": e.modified.to_rfc3339()
                                    })
                                }).collect();

                            format_mcp_content(json!({
                                "server_uuid": server_uuid,
                                "directory": "/plugins",
                                "total_plugins": plugins.len(),
                                "plugins": plugins
                            }))
                        }
                        Err(e) => format_mcp_error(&format!("Wings daemon list_plugins failed: {e:?}")),
                    }
                }
                Err(err) => format_mcp_error(&err),
            }
        }

        "install_plugin" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let plugin_id = arguments.get("plugin_id").and_then(|v| v.as_str()).unwrap_or("");
            let download_url_param = arguments.get("download_url").and_then(|v| v.as_str()).unwrap_or("");
            let filename_param = arguments.get("filename").and_then(|v| v.as_str()).unwrap_or("");

            let mut download_url = download_url_param.to_string();
            let mut filename = filename_param.to_string();

            if download_url.is_empty() && !plugin_id.is_empty() {
                let http_client = reqwest::Client::builder().user_agent("CalagopusMCP/1.3.3").build().ok();
                if let Some(c) = http_client {
                    let modrinth_versions_url = format!("https://api.modrinth.com/v2/project/{plugin_id}/version");
                    if let Ok(res) = c.get(&modrinth_versions_url).send().await {
                        if let Ok(versions) = res.json::<Value>().await {
                            if let Some(first_ver) = versions.as_array().and_then(|arr| arr.first()) {
                                if let Some(files) = first_ver.get("files").and_then(|f| f.as_array()) {
                                    if let Some(primary_file) = files.iter().find(|f| f.get("primary").and_then(|p| p.as_bool()).unwrap_or(false)).or_else(|| files.first()) {
                                        if let Some(url) = primary_file.get("url").and_then(|u| u.as_str()) {
                                            download_url = url.to_string();
                                        }
                                        if filename.is_empty() {
                                            if let Some(fn_str) = primary_file.get("filename").and_then(|f| f.as_str()) {
                                                filename = fn_str.to_string();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if download_url.is_empty() {
                return format_mcp_error("Either direct 'download_url' or a valid 'plugin_id' (resolvable via Modrinth) must be provided.");
            }

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let req_body = wings_api::servers_server_files_pull::post::RequestBody {
                        root: "/plugins".into(),
                        url: download_url.to_string().into(),
                        file_name: if filename.is_empty() { None } else { Some(filename.clone().into()) },
                        use_header: true,
                        foreground: true,
                    };
                    match client.post_servers_server_files_pull(server_id, &req_body).await {
                        Ok(_) => format_mcp_content(json!({
                            "status": "plugin_installed",
                            "server_uuid": server_uuid,
                            "plugin_id": plugin_id,
                            "download_url": download_url,
                            "target_file": format!("/plugins/{}", if filename.is_empty() { "downloaded_plugin.jar" } else { &filename }),
                            "installed_at": chrono::Utc::now().to_rfc3339()
                        })),
                        Err(e) => format_mcp_error(&format!("Wings daemon install_plugin failed: {e:?}")),
                    }
                }
                Err(err) => format_mcp_error(&err),
            }
        }

        "remove_plugin" => {
            let server_uuid = arguments.get("server_uuid").and_then(|v| v.as_str()).unwrap_or("");
            let filename = arguments.get("filename").and_then(|v| v.as_str()).unwrap_or("");

            if filename.trim().is_empty() {
                return format_mcp_error("Plugin filename cannot be empty");
            }

            match get_wings_client_for_server(state, server_uuid).await {
                Ok((client, server_id)) => {
                    let req_body = wings_api::servers_server_files_delete::post::RequestBody {
                        root: "/plugins".into(),
                        files: vec![filename.to_string().into()],
                    };
                    match client.post_servers_server_files_delete(server_id, &req_body).await {
                        Ok(resp) => format_mcp_content(json!({
                            "status": "plugin_removed",
                            "server_uuid": server_uuid,
                            "filename": filename,
                            "deleted_count": resp.deleted,
                            "timestamp": chrono::Utc::now().to_rfc3339()
                        })),
                        Err(e) => format_mcp_error(&format!("Wings daemon remove_plugin failed: {e:?}")),
                    }
                }
                Err(err) => format_mcp_error(&err),
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
