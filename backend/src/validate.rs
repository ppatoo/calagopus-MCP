//! Input validation for MCP tool arguments. Every value that reaches Wings,
//! Modrinth, a URL fetch, or a SQL query limit goes through one of these helpers.

use std::net::IpAddr;
use url::{Host, Url};

pub const MAX_LIMIT: i64 = 100;
pub const MAX_OFFSET: i64 = 1_000_000;
pub const MAX_CONSOLE_COMMAND: usize = 1_000;
pub const MAX_WRITE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PLUGIN_BYTES: u64 = 512 * 1024 * 1024;

pub fn clamp_limit(value: i64) -> i64 {
    value.clamp(1, MAX_LIMIT)
}

pub fn clamp_offset(value: i64) -> i64 {
    value.clamp(0, MAX_OFFSET)
}

/// A path inside a server volume. Rejects traversal and NUL/backslash tricks.
pub fn volume_path(raw: &str) -> Result<&str, String> {
    let path = raw.trim();
    if path.is_empty() {
        return Err("Parameter 'path' is required.".into());
    }
    if path.len() > 4096 {
        return Err("Path is too long (max 4096 characters).".into());
    }
    if path.contains(|c: char| c == '\0' || c == '\\') {
        return Err("Path contains an invalid character.".into());
    }
    if path.split('/').any(|segment| segment == "..") {
        return Err("Path must not contain '..' segments.".into());
    }
    Ok(path)
}

/// A directory inside a server volume. Empty input means the volume root.
pub fn directory(raw: &str) -> Result<&str, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok("/");
    }
    volume_path(trimmed).map_err(|_| "Invalid directory path.".to_string())
}

/// A bare plugin JAR filename: no separators, no leading dot, must end in .jar.
pub fn plugin_filename(raw: &str) -> Result<&str, String> {
    let name = raw.trim();
    let chars_ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'));
    if name.is_empty()
        || name.len() > 255
        || name.starts_with('.')
        || !chars_ok
        || !name.to_ascii_lowercase().ends_with(".jar")
    {
        return Err(
            "Filename must be a bare .jar name using letters, digits, '.', '-', '_' or '+' (max 255 characters)."
                .into(),
        );
    }
    Ok(name)
}

pub fn modrinth_id(raw: &str) -> Result<&str, String> {
    if !raw.is_empty()
        && raw.len() <= 64
        && raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(raw)
    } else {
        Err("Invalid Modrinth plugin_id.".into())
    }
}

fn blocked_address() -> String {
    "URL points to a private, loopback or link-local address, which is not allowed.".into()
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10 (CGNAT)
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
                || (s[0] & 0xffc0) == 0xfe80 // link local fe80::/10
                || v6
                    .to_ipv4_mapped()
                    .map_or(false, |v4| is_blocked_ip(IpAddr::V4(v4)))
        }
    }
}

/// Parse a URL the node is asked to fetch. Only http(s), no credentials, and no
/// obviously internal hosts. This cannot stop DNS rebinding; see README notes.
pub fn remote_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw.trim()).map_err(|_| "URL is not valid.".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("URL must use http:// or https://.".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URL must not contain credentials.".into());
    }
    match url.host() {
        None => return Err("URL has no host.".into()),
        Some(Host::Ipv4(ip)) => {
            if is_blocked_ip(IpAddr::V4(ip)) {
                return Err(blocked_address());
            }
        }
        Some(Host::Ipv6(ip)) => {
            if is_blocked_ip(IpAddr::V6(ip)) {
                return Err(blocked_address());
            }
        }
        Some(Host::Domain(domain)) => {
            let d = domain.to_ascii_lowercase();
            if d == "localhost"
                || d.ends_with(".localhost")
                || d.ends_with(".local")
                || d.ends_with(".internal")
                || !d.contains('.')
            {
                return Err(blocked_address());
            }
        }
    }
    Ok(url)
}

/// The archive filename taken from the URL path (never the query string).
pub fn archive_file_name(url: &Url) -> Result<String, String> {
    let name = url
        .path_segments()
        .and_then(|segments| segments.filter(|s| !s.is_empty()).last())
        .unwrap_or("");
    let lower = name.to_ascii_lowercase();
    let is_archive = [".zip", ".tar", ".tar.gz", ".tgz"]
        .iter()
        .any(|ext| lower.ends_with(ext));
    let safe = !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if is_archive && safe {
        Ok(name.to_string())
    } else {
        Err("archive_url must point to a .zip, .tar, .tar.gz or .tgz file with a simple filename.".into())
    }
}

pub fn sha1_hex(raw: &str) -> Result<String, String> {
    let value = raw.trim().to_ascii_lowercase();
    if value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(value)
    } else {
        Err("checksum must be a 40-character hex SHA-1 value.".into())
    }
}

/// A hostname such as play.example.com. No scheme, port, path, or wildcard.
pub fn domain_name(raw: &str) -> Result<String, String> {
    let domain = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    let invalid = || {
        "Invalid domain name. Use a hostname like play.example.com (no scheme, port or path)."
            .to_string()
    };
    if domain.is_empty() || domain.len() > 253 {
        return Err(invalid());
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return Err(invalid());
    }
    for label in &labels {
        let ok = !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !ok {
            return Err(invalid());
        }
    }
    Ok(domain)
}

/// Returns (owner, repo, provider host) for a github.com or gitlab.com repository URL.
pub fn git_repo(raw: &str) -> Result<(String, String, &'static str), String> {
    let url = Url::parse(raw.trim()).map_err(|_| "repo_url is not a valid URL.".to_string())?;
    if url.scheme() != "https" {
        return Err("repo_url must use https://.".into());
    }
    let host_raw = url.host_str().unwrap_or("").to_ascii_lowercase();
    let host = host_raw.strip_prefix("www.").unwrap_or(&host_raw);
    let provider: &'static str = match host {
        "github.com" => "github.com",
        "gitlab.com" => "gitlab.com",
        _ => return Err("repo_url must point to github.com or gitlab.com.".into()),
    };
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|x| !x.is_empty()).collect::<Vec<&str>>())
        .unwrap_or_default();
    if segments.len() != 2 {
        return Err("repo_url must look like https://github.com/owner/repo.".into());
    }
    let owner = segments[0];
    let repo = segments[1].strip_suffix(".git").unwrap_or(segments[1]);
    let name_ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 100
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !name_ok(owner) || !name_ok(repo) {
        return Err("Invalid owner or repository name.".into());
    }
    Ok((owner.to_string(), repo.to_string(), provider))
}

pub fn branch_name(raw: &str) -> Result<&str, String> {
    let branch = raw.trim();
    let ok = !branch.is_empty()
        && branch.len() <= 255
        && !branch.starts_with('/')
        && !branch.starts_with('-')
        && !branch.ends_with('/')
        && !branch.contains("..")
        && !branch.contains("//")
        && branch
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'));
    if ok {
        Ok(branch)
    } else {
        Err("Invalid branch name.".into())
    }
}

pub fn branch_slug(branch: &str) -> String {
    branch.replace('/', "-")
}

/// A console command must be one non-empty line.
pub fn console_command(raw: &str) -> Result<&str, String> {
    if raw.trim().is_empty() {
        Err("Parameter 'command' cannot be empty.".into())
    } else if raw.len() > MAX_CONSOLE_COMMAND {
        Err(format!("Command is too long (max {MAX_CONSOLE_COMMAND} characters)."))
    } else if raw.contains(|c: char| c == '\n' || c == '\r' || c == '\0') {
        Err("Command must be a single line.".into())
    } else {
        Ok(raw)
    }
}
