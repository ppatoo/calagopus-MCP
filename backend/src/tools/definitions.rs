use serde_json::{Value, json};

pub fn get_tools_list() -> Value {
    json!({
        "tools": [
            // --- Game Server & Domain Tools ---
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
                        "lines": { "type": "integer", "description": "Number of log lines to return (default 50, max 500)" }
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
                        "overwrite": { "type": "boolean", "description": "If false and file exists, return error (default false)" }
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
                        "expiry_seconds": { "type": "integer", "description": "JWT link expiry in seconds (default 900, range 60-86400)" }
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
                        "checksum":     { "type": "string", "description": "Optional expected SHA-1. The downloaded file is hashed and removed on mismatch." }
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
                        "filename":    { "type": "string", "description": "Bare JAR filename in /plugins (no path separators)" }
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
