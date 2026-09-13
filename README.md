# Calagopus Model Context Protocol (MCP) Connector Extension

[![License: Custom](https://img.shields.io/badge/License-Custom%20(ppatoo)-red.svg)](LICENSE)
[![Calagopus Extension Spec](https://img.shields.io/badge/Calagopus-Extension%20v1.2.1-blue.svg)](https://calagopus.com)

The **Calagopus MCP Connector** (`dev.calagopus.mcpserver`) is an extension for the [Calagopus Game Panel](https://calagopus.com) that exposes **26 management tools** via the Model Context Protocol (MCP) using JSON-RPC 2.0 and Server-Sent Events (SSE) transports.

This extension enables AI coding assistants, agents, and desktop applications (Cursor, Claude Desktop, Antigravity, custom MCP clients) to seamlessly manage game servers, daemon containers, volume files, backup archives, domain SSL certificates, and Minecraft plugin catalogs.

---

## ⚠️ Disclaimer & Liability Notice

> **IMPORTANT**: This codebase is crude and experimental. The author (**ppatoo**) holds **NO LIABILITY or responsibility** for any security vulnerabilities, exploits, software bugs, data loss, server downtime, system damage, or any direct/indirect issues resulting from installing, operating, or modifying this code. Use and deploy entirely at your own risk.

---

## 📄 License & Terms of Use

This project is licensed under a **Custom Source & Use License**:

1. **Ownership & Attribution**: Code is owned by **ppatoo**. Any modifications, derived works, forks, or redistributions **must retain copyright notices** and provide prominent credit to **ppatoo**.
2. **Non-Commercial Restriction**: This code **cannot be made paid**, sold, put behind a paywall, or monetized by anyone. It must remain free and open access.
3. **No Warranty**: Provided "as-is" without warranty of any kind.

See the full terms in the [LICENSE](LICENSE) file.

---

## 📦 Package Archive Location

- **Extension Archive File**: `dev_calagopus_mcpserver.c7s` (Also available in `/home/pato/Downloads/dev_calagopus_mcpserver.c7s`)
- **Package Identifier**: `dev.calagopus.mcpserver`
- **Version**: `1.2.1`

---

## 🚀 Quick Start / Installation

### 1. Install via Calagopus Panel CLI
```bash
# Add extension package archive
panel-rs extensions add dev_calagopus_mcpserver.c7s

# Inspect extension metadata & dependencies
panel-rs extensions inspect dev_calagopus_mcpserver.c7s

# Apply and rebuild panel
panel-rs extensions apply
```

### 2. Configure MCP Client (`mcp_config.json`)

Add the Calagopus MCP Server to your MCP client configuration (`~/.gemini/config/mcp_config.json`, Cursor, or Claude Desktop):

```json
{
  "mcpServers": {
    "calagopus-local": {
      "url": "http://127.0.0.1:8000/api/extensions/mcp/v1/sse?api_key=calagopus_mcp_sec_052ef6fe079321fe1fac23eae66f7db7",
      "transport": "sse"
    }
  }
}
```

---

## 🛠️ Complete 26 MCP Tools Reference

All tools support both hyphenated (`list-servers`), underscore (`list_servers`), and prefixed (`calagopus_list_servers`) name aliases.

### 1. Server Management & Live State
- **`list-servers`**: List visible game servers with UUIDs, status, allocations, node mappings, memory & disk.
- **`get-server`**: Get detailed server metadata, allocation, egg config, and live Wings daemon metrics.
- **`power-server`**: Send `start`, `stop`, `restart`, or `kill` action signals to a server daemon container.
- **`send-console-command`**: Run console command string on a running server.
- **`read-console`**: Retrieve recent tail of server console log output.

### 2. Machine & Host Infrastructure
- **`list-machines`**: List enrolled node hosts with health status, SFTP port, URL, memory & disk capacity.
- **`get-system-health`**: Query live Calagopus panel system metrics, database connections, and cache latency.

### 3. File System & File Transfers
- **`list-files`**: Browse directory contents inside a server's container volume.
- **`read-file`**: Read text file contents up to 1 MB from a server volume.
- **`write-file`**: Create a file or replace its contents in the server volume (No undo).
- **`upload-file-from-url`**: Have the machine node fetch a remote file onto the server volume.
- **`download-files`**: Generate a signed download link for a file, directory, or server volume archive.

### 4. Backups & Disaster Recovery
- **`list-backups`**: Every backup a server holds, with state, size, checksum, and creation date.
- **`create-backup`**: Take a background backup archive for a server.
- **`download-backup`**: Short-lived download link for a finished backup archive.

### 5. Web App & Domain SSL Management
- **`get-site`**: Web app address, custom domain bindings, SSL state, and deploy status.
- **`deploy-repo`**: Deploy a public GitHub repository onto a web app.
- **`deploy-files`**: Deploy local archive/files directly to a web app.
- **`attach-domain`**: Point custom domain at a web app with automated SSL certificate provisioning.
- **`detach-domain`**: Remove custom domain from a web app.

### 6. Minecraft Plugin Catalog Integration
- **`search-plugins`**: Live search Minecraft plugin catalog (Modrinth API) filtered by software (`paper`, `spigot`, `velocity`, `fabric`) & MC version.
- **`list-plugins`**: List every plugin JAR file present in `/plugins`.
- **`install-plugin`**: Install a plugin JAR and required dependencies from the catalog into `/plugins`.
- **`remove-plugin`**: Delete a plugin JAR from `/plugins`.

### 7. Administrative Resources
- **`list-nests`**: List all game nests, egg repositories, and server templates configured in Calagopus.
- **`list-users`**: List registered panel users, email addresses, roles, and administrative privileges.

---

## 📡 API Endpoints

- **Discovery Endpoint**: `GET /api/extensions/mcp/v1/info`
- **SSE Stream Transport**: `GET /api/extensions/mcp/v1/sse`
- **Message Transport**: `POST /api/extensions/mcp/v1/sse` or `POST /api/extensions/mcp/v1/messages`
