import React, { useEffect, useState } from 'react';

export default function McpExtensionWidget() {
  const [secretKey, setSecretKey] = useState<string>('Loading...');
  const [copiedKey, setCopiedKey] = useState<boolean>(false);
  const [copiedUrl, setCopiedUrl] = useState<boolean>(false);

  useEffect(() => {
    fetch('/api/extensions/mcp/v1/key')
      .then((res) => res.json())
      .then((data) => {
        if (data.secret_key) {
          setSecretKey(data.secret_key);
        }
      })
      .catch(() => setSecretKey('Unable to fetch key'));
  }, []);

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
    <div className="p-6 bg-slate-900 border border-slate-800 rounded-lg shadow-xl text-slate-100">
      <div className="flex items-center justify-between mb-4">
        <h2 className="text-xl font-bold text-sky-400 flex items-center gap-2">
          ⚡ Calagopus MCP Connector
        </h2>
        <span className="px-3 py-1 bg-emerald-500/20 text-emerald-400 border border-emerald-500/30 text-xs font-semibold rounded-full">
          Active (v1.3.0)
        </span>
      </div>

      <p className="text-sm text-slate-400 mb-6">
        The Model Context Protocol (MCP) Connector enables AI agents (Claude, Gemini, Antigravity, Cursor) to manage servers, query live stats, list nodes, and run console commands via standardized JSON-RPC 2.0 &amp; SSE endpoints.
      </p>

      {/* Secret Key Card */}
      <div className="p-4 mb-5 bg-amber-950/40 border border-amber-500/40 rounded-lg">
        <div className="flex items-center justify-between mb-2">
          <span className="text-xs uppercase tracking-wider font-bold text-amber-400 flex items-center gap-1">
            🔑 Panel Secret Authentication Key
          </span>
          <button
            onClick={() => copyText(secretKey, 'key')}
            className="px-3 py-1 text-xs font-semibold bg-amber-500/20 text-amber-300 hover:bg-amber-500/30 border border-amber-500/40 rounded transition cursor-pointer"
          >
            {copiedKey ? '✓ Copied!' : 'Copy Key'}
          </button>
        </div>
        <div className="font-mono text-sm text-amber-200 bg-slate-950/90 p-3 rounded border border-amber-900/60 break-all select-all">
          {secretKey}
        </div>
        <p className="text-xs text-amber-400/80 mt-2">
          This secret key is uniquely generated and saved in PostgreSQL for this panel instance.
        </p>
      </div>

      <div className="space-y-4">
        <div className="p-4 bg-slate-950/60 rounded border border-slate-800">
          <div className="flex items-center justify-between mb-2">
            <div className="text-xs uppercase tracking-wider font-bold text-slate-400">
              Authenticated SSE Stream Endpoint URL
            </div>
            <button
              onClick={() => copyText(sseFullUrl, 'url')}
              className="px-2 py-1 text-xs font-medium bg-slate-800 text-slate-300 hover:bg-slate-700 rounded border border-slate-700 transition cursor-pointer"
            >
              {copiedUrl ? '✓ Copied!' : 'Copy Full URL'}
            </button>
          </div>
          <code className="text-xs text-sky-300 font-mono select-all break-all block p-2 bg-slate-900/80 rounded border border-slate-800">
            {sseFullUrl}
          </code>
        </div>

        <div className="p-4 bg-slate-950/60 rounded border border-slate-800">
          <div className="text-xs uppercase tracking-wider font-bold text-slate-400 mb-1">
            HTTP Header Authentication
          </div>
          <code className="text-xs text-emerald-300 font-mono select-all break-all block p-2 bg-slate-900/80 rounded border border-slate-800">
            Authorization: Bearer {secretKey}
          </code>
        </div>
      </div>

      <div className="mt-6 pt-4 border-t border-slate-800">
        <h3 className="text-sm font-semibold text-slate-300 mb-2">Available MCP Tools (26 Tools Total)</h3>
        <div className="grid grid-cols-2 sm:grid-cols-3 gap-2 text-xs text-slate-400 font-mono">
          <div className="p-2 bg-slate-800/40 rounded">• list-servers</div>
          <div className="p-2 bg-slate-800/40 rounded">• get-server</div>
          <div className="p-2 bg-slate-800/40 rounded">• power-server</div>
          <div className="p-2 bg-slate-800/40 rounded">• send-console-command</div>
          <div className="p-2 bg-slate-800/40 rounded">• read-console</div>
          <div className="p-2 bg-slate-800/40 rounded">• list-machines</div>
          <div className="p-2 bg-slate-800/40 rounded">• get-system-health</div>
          <div className="p-2 bg-slate-800/40 rounded">• list-files</div>
          <div className="p-2 bg-slate-800/40 rounded">• read-file</div>
          <div className="p-2 bg-slate-800/40 rounded">• write-file</div>
          <div className="p-2 bg-slate-800/40 rounded">• upload-file-from-url</div>
          <div className="p-2 bg-slate-800/40 rounded">• download-files</div>
          <div className="p-2 bg-slate-800/40 rounded">• list-backups</div>
          <div className="p-2 bg-slate-800/40 rounded">• create-backup</div>
          <div className="p-2 bg-slate-800/40 rounded">• download-backup</div>
          <div className="p-2 bg-slate-800/40 rounded">• search-plugins</div>
          <div className="p-2 bg-slate-800/40 rounded">• install-plugin</div>
          <div className="p-2 bg-slate-800/40 rounded">• remove-plugin</div>
        </div>
      </div>
    </div>
  );
}
