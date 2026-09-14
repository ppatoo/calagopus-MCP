import { useEffect, useState } from 'react';
import { Extension } from 'shared';

function McpExtensionWidget() {
  const [secretKey, setSecretKey] = useState<string>('Loading...');
  const [copiedKey, setCopiedKey] = useState<boolean>(false);
  const [copiedUrl, setCopiedUrl] = useState<boolean>(false);
  const [rotating, setRotating] = useState<boolean>(false);
  const [message, setMessage] = useState<string | null>(null);

  const fetchKey = () => {
    fetch('/api/extensions/mcp/v1/key')
      .then((res) => res.json())
      .then((data) => {
        if (data.secret_key) {
          setSecretKey(data.secret_key);
        }
      })
      .catch(() => setSecretKey('Unable to fetch secret key'));
  };

  useEffect(() => {
    fetchKey();
  }, []);

  const handleRotateKey = async () => {
    if (!window.confirm('Are you sure you want to rotate the MCP Secret Key? Any existing connected AI agents or MCP clients will need to be updated with the new key.')) {
      return;
    }

    setRotating(true);
    setMessage(null);
    try {
      const res = await fetch('/api/extensions/mcp/v1/key/rotate', { method: 'POST' });
      const data = await res.json();
      if (data.secret_key) {
        setSecretKey(data.secret_key);
        setMessage('✨ MCP Secret Key successfully re-rolled!');
        setTimeout(() => setMessage(null), 4000);
      }
    } catch (err) {
      setMessage('❌ Failed to rotate secret key.');
    } finally {
      setRotating(false);
    }
  };

  const copyText = (text: string, type: 'key' | 'url') => {
    navigator.clipboard.writeText(text);
    if (type === 'key') {
      setCopiedKey(true);
      setTimeout(() => setCopiedKey(false), 2000);
    } else {
      setCopiedUrl(true);
      setTimeout(() => setCopiedUrl(false), 2000);
    }
  };

  const origin = typeof window !== 'undefined' ? window.location.origin : 'http://localhost:8000';
  const sseFullUrl = `${origin}/api/extensions/mcp/v1/sse?api_key=${secretKey}`;

  return (
    <div className="p-6 bg-slate-900 border border-slate-800 rounded-lg shadow-xl text-slate-100 max-w-5xl mx-auto">
      {/* Header */}
      <div className="flex flex-col sm:flex-row sm:items-center justify-between pb-4 mb-6 border-b border-slate-800 gap-2">
        <div>
          <h2 className="text-2xl font-bold text-sky-400 flex items-center gap-2">
            ⚡ Calagopus MCP Connector
          </h2>
          <p className="text-xs text-slate-400 mt-1">
            Author: <span className="text-slate-200 font-semibold">Pato</span> | Package: <code className="text-sky-300">dev.calagopus.mcpserver</code>
          </p>
        </div>
        <div>
          <span className="px-3 py-1 bg-emerald-500/20 text-emerald-400 border border-emerald-500/30 text-xs font-semibold rounded-full">
            Active (v1.3.3)
          </span>
        </div>
      </div>

      <p className="text-sm text-slate-300 mb-6 leading-relaxed">
        The Model Context Protocol (MCP) Connector enables AI agents (Claude, Gemini, Antigravity, Cursor) to manage servers, query live stats, list nodes, and run console commands via standardized JSON-RPC 2.0 &amp; SSE endpoints.
      </p>

      {/* Toast message notification */}
      {message && (
        <div className="mb-4 p-3 bg-emerald-950/80 border border-emerald-500/50 text-emerald-300 text-xs font-medium rounded-lg flex items-center justify-between">
          <span>{message}</span>
        </div>
      )}

      {/* Extension Configuration & Key Card */}
      <div className="p-5 mb-6 bg-slate-950/80 border border-amber-500/30 rounded-xl shadow-inner">
        <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-3">
          <span className="text-xs uppercase tracking-wider font-bold text-amber-400 flex items-center gap-1.5">
            🔑 Panel Secret Authentication Key
          </span>
          <div className="flex items-center gap-2">
            <button
              onClick={() => copyText(secretKey, 'key')}
              className="px-3 py-1.5 text-xs font-semibold bg-amber-500/20 text-amber-300 hover:bg-amber-500/30 border border-amber-500/40 rounded-md transition cursor-pointer"
            >
              {copiedKey ? '✓ Copied!' : 'Copy Key'}
            </button>
            <button
              onClick={handleRotateKey}
              disabled={rotating}
              className="px-3 py-1.5 text-xs font-semibold bg-rose-500/20 text-rose-300 hover:bg-rose-500/30 border border-rose-500/40 rounded-md transition cursor-pointer disabled:opacity-50 flex items-center gap-1"
            >
              {rotating ? '🔄 Rotating...' : '🔄 Rotate Secret Key'}
            </button>
          </div>
        </div>

        <div className="font-mono text-sm text-amber-200 bg-slate-900/90 p-3 rounded-lg border border-amber-900/60 break-all select-all shadow-sm">
          {secretKey}
        </div>

        <p className="text-xs text-slate-400 mt-2.5">
          This key is stored in PostgreSQL and used to authenticate all incoming MCP requests. Rotating the key will instantly generate a new 32-hex random key and invalidate previous keys.
        </p>
      </div>

      {/* Endpoints & Configuration */}
      <div className="space-y-4 mb-6">
        <div className="p-4 bg-slate-950/60 rounded-lg border border-slate-800">
          <div className="flex items-center justify-between mb-2">
            <div className="text-xs uppercase tracking-wider font-bold text-slate-400">
              Authenticated SSE Stream Endpoint URL
            </div>
            <button
              onClick={() => copyText(sseFullUrl, 'url')}
              className="px-2.5 py-1 text-xs font-medium bg-slate-800 text-slate-300 hover:bg-slate-700 rounded border border-slate-700 transition cursor-pointer"
            >
              {copiedUrl ? '✓ Copied!' : 'Copy Full URL'}
            </button>
          </div>
          <code className="text-xs text-sky-300 font-mono select-all break-all block p-2.5 bg-slate-900/90 rounded border border-slate-800">
            {sseFullUrl}
          </code>
        </div>

        <div className="p-4 bg-slate-950/60 rounded-lg border border-slate-800">
          <div className="text-xs uppercase tracking-wider font-bold text-slate-400 mb-1.5">
            HTTP Header Authentication Example
          </div>
          <code className="text-xs text-emerald-300 font-mono select-all break-all block p-2.5 bg-slate-900/90 rounded border border-slate-800">
            Authorization: Bearer {secretKey}
          </code>
        </div>
      </div>

      {/* Tools Catalog */}
      <div className="pt-5 border-t border-slate-800">
        <h3 className="text-sm font-semibold text-slate-300 mb-3 flex items-center justify-between">
          <span>Available MCP Tools</span>
          <span className="text-xs font-normal text-slate-400">26 Tools Registered</span>
        </h3>
        <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 gap-2 text-xs text-slate-400 font-mono">
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• list-servers</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• get-server</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• power-server</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• send-console-command</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• read-console</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• list-machines</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• get-system-health</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• list-files</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• read-file</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• write-file</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• upload-file-from-url</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• download-files</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• list-backups</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• create-backup</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• download-backup</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• get-site</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• deploy-repo</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• deploy-files</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• attach-domain</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• detach-domain</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• search-plugins</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• list-plugins</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• install-plugin</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• remove-plugin</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• list-nests</div>
          <div className="p-2 bg-slate-800/40 rounded border border-slate-800/60">• list-users</div>
        </div>
      </div>
    </div>
  );
}

export class McpExtension extends Extension {
  public cardConfigurationPage = McpExtensionWidget;
}

export default new McpExtension();
