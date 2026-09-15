(function () {
  'use strict';

  // ============================================================
  // DATA LOADING
  // ============================================================

  const base64 = document.getElementById('session-data').textContent;
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  const session = JSON.parse(new TextDecoder('utf-8').decode(bytes));
  const allMessages = Array.isArray(session.conversation) ? session.conversation : [];

  let showInternal = false;

  // ============================================================
  // HELPERS
  // ============================================================

  function escapeHtml(text) {
    if (text == null) return '';
    return String(text)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  function sanitizeUrl(url) {
    if (!url) return null;
    const cleaned = String(url).replace(/[\u0000-\u001f]/g, '').trim();
    if (/^(https?:|mailto:|#|\/|\.)/i.test(cleaned)) return cleaned;
    if (/^[^:]+$/.test(cleaned)) return cleaned;
    return null;
  }

  const MILLISECOND_TIMESTAMP_THRESHOLD = 10000000000;

  function formatTimestamp(timestamp) {
    if (!timestamp) return '';
    const unixSeconds = timestamp > MILLISECOND_TIMESTAMP_THRESHOLD ? timestamp / 1000 : timestamp;
    const d = new Date(unixSeconds * 1000);
    if (isNaN(d.getTime())) return '';
    return d.toLocaleString();
  }

  function shortenPath(p) {
    if (!p) return '';
    const cwd = session.working_dir;
    if (cwd && p.startsWith(cwd)) {
      const rest = p.slice(cwd.length).replace(/^\//, '');
      return rest || p;
    }
    return p;
  }

  function isUserAudience(block) {
    const audience = block && block.annotations && block.annotations.audience;
    return !Array.isArray(audience) || audience.includes('user');
  }

  function userVisibleContentBlock(block) {
    if (!block || typeof block !== 'object') return null;
    if ((block.type === 'text' || block.type === 'image') && !isUserAudience(block)) return null;
    if (block.type !== 'toolResponse') return block;

    const result = block.toolResult;
    const value = result && result.status === 'success' && result.value;
    if (!value || !Array.isArray(value.content)) return block;
    return {
      ...block,
      toolResult: {
        ...result,
        value: { ...value, content: value.content.filter(isUserAudience) },
      },
    };
  }

  function userVisibleMessage(msg) {
    const content = Array.isArray(msg.content)
      ? msg.content.map(userVisibleContentBlock).filter((block) => block)
      : [];
    return { ...msg, content };
  }

  function hasAssistantOnlyContent(block) {
    if (!block || typeof block !== 'object') return false;
    if ((block.type === 'text' || block.type === 'image') && !isUserAudience(block)) return true;
    if (block.type !== 'toolResponse') return false;
    const result = block.toolResult;
    const value = result && result.status === 'success' && result.value;
    return !!(value && Array.isArray(value.content) && value.content.some((item) => !isUserAudience(item)));
  }

  // ============================================================
  // MARKDOWN
  // ============================================================

  marked.use({
    breaks: true,
    gfm: true,
    tokenizer: {
      html() { return undefined; },
      tag() { return undefined; }
    },
    renderer: {
      link(token) {
        const href = sanitizeUrl(token.href);
        if (href === null) return this.parser.parseInline(token.tokens);
        let out = '<a href="' + escapeHtml(href) + '"';
        if (token.title) out += ' title="' + escapeHtml(token.title) + '"';
        out += '>' + this.parser.parseInline(token.tokens) + '</a>';
        return out;
      },
      image(token) {
        const href = sanitizeUrl(token.href);
        if (href === null) return escapeHtml(token.text || '');
        let out = '<img src="' + escapeHtml(href) + '" alt="' + escapeHtml(token.text || '') + '"';
        if (token.title) out += ' title="' + escapeHtml(token.title) + '"';
        out += '>';
        return out;
      },
      code(token) {
        const code = token.text;
        const lang = token.lang;
        let highlighted;
        if (lang && hljs.getLanguage(lang)) {
          try { highlighted = hljs.highlight(code, { language: lang }).value; }
          catch { highlighted = escapeHtml(code); }
        } else {
          try { highlighted = hljs.highlightAuto(code).value; }
          catch { highlighted = escapeHtml(code); }
        }
        return '<pre><code class="hljs">' + highlighted + '</code></pre>';
      },
      codespan(token) {
        return '<code>' + escapeHtml(token.text) + '</code>';
      }
    }
  });

  function renderMarkdown(text) {
    try { return marked.parse(text || ''); }
    catch { return escapeHtml(text || ''); }
  }

  // ============================================================
  // TOOL RENDERING
  // ============================================================

  // Split "extension__tool" into { namespace, tool }, mirroring goose's own
  // tool-name convention (see ToolNameParts / markdown export).
  function toolNameParts(name) {
    if (!name) return { namespace: null, tool: 'tool' };
    const idx = name.indexOf('__');
    if (idx >= 0) {
      return { namespace: name.slice(0, idx), tool: name.slice(idx + 2) };
    }
    if (name === 'shell' || name === 'write' || name === 'edit') {
      return { namespace: 'developer', tool: name };
    }
    return { namespace: null, tool: name };
  }

  function firstString(args, keys) {
    if (!args || typeof args !== 'object') return null;
    for (const k of keys) {
      if (typeof args[k] === 'string' && args[k].length) return args[k];
    }
    return null;
  }

  const LANG_BY_EXT = {
    rs: 'rust', js: 'javascript', mjs: 'javascript', ts: 'typescript', tsx: 'typescript',
    jsx: 'javascript', py: 'python', go: 'go', rb: 'ruby', java: 'java', c: 'c', h: 'c',
    cpp: 'cpp', cc: 'cpp', hpp: 'cpp', cs: 'csharp', php: 'php', swift: 'swift', kt: 'kotlin',
    sh: 'bash', bash: 'bash', zsh: 'bash', json: 'json', yaml: 'yaml', yml: 'yaml', toml: 'ini',
    ini: 'ini', xml: 'xml', html: 'xml', css: 'css', scss: 'scss', sql: 'sql', md: 'markdown',
  };

  function langFromPath(path) {
    if (!path) return null;
    const ext = path.split('.').pop().toLowerCase();
    return LANG_BY_EXT[ext] || null;
  }

  function highlight(text, lang) {
    if (lang && hljs.getLanguage(lang)) {
      try { return hljs.highlight(text, { language: lang }).value; } catch {}
    }
    return escapeHtml(text);
  }

  // Render the header line for a tool call based on its name/arguments.
  // Namespace is shown path-style (developer/shell) to encode the real
  // extension -> tool hierarchy.
  function renderToolHeader(name, args) {
    const parts = toolNameParts(name);
    const command = firstString(args, ['command', 'cmd']);
    const path = firstString(args, ['path', 'file_path', 'filePath', 'file']);
    const isShell = parts.tool === 'shell';

    // Tools whose `content` arg is a full file/document body (shown like write).
    const showsContent = (parts.tool === 'write' || parts.tool === 'todo_write') &&
      args && typeof args.content === 'string';

    const ns = parts.namespace ? '<span class="tool-ns">' + escapeHtml(parts.namespace) + '/</span>' : '';
    let html = '<div class="tool-header">' + ns + '<span class="tool-name">' + escapeHtml(parts.tool) + '</span>';
    if (path) html += ' <span class="tool-path">' + escapeHtml(shortenPath(path)) + '</span>';

    if (showsContent) {
      const n = args.content.split('\n').length;
      html += ' <span class="tool-count">(' + n + (n === 1 ? ' line' : ' lines') + ')</span>';
    }
    html += '</div>';

    if (showsContent) {
      html += collapsibleCode(args.content, langFromPath(path));
    } else if (parts.tool === 'edit' && args && typeof args.before === 'string' && typeof args.after === 'string') {
      html += renderDiff(args.before, args.after);
    } else if (command) {
      html += '<div class="tool-command' + (isShell ? ' shell' : '') + '">' + escapeHtml(command) + '</div>';
    } else if (args && typeof args === 'object' && Object.keys(args).length && !path) {
      html += '<div class="tool-args">' + escapeHtml(JSON.stringify(args, null, 2)) + '</div>';
    }
    return html;
  }

  // Render an rmcp content block array (tool result payload) to HTML.
  function renderResultContent(content, outputClass = 'tool-output') {
    if (!Array.isArray(content)) return '';
    let html = '';
    let textParts = [];
    for (const block of content) {
      if (!block || typeof block !== 'object') continue;
      if (block.type === 'text') {
        textParts.push(block.text || '');
      } else if (block.type === 'image') {
        html += '<img class="tool-image" src="data:' + escapeHtml(block.mimeType || 'image/png') +
          ';base64,' + escapeHtml(block.data || '') + '" />';
      } else if (block.type === 'resource' && block.resource) {
        if (typeof block.resource.text === 'string') textParts.push(block.resource.text);
      }
    }
    const text = textParts.join('\n').replace(/\n+$/, '');
    if (text) {
      html = '<div class="' + outputClass + '">' + collapsibleCode(text, null) + '</div>' + html;
    }
    return html;
  }

  const PREVIEW_LINES = 12;

  // A syntax-highlighted code block, collapsed to PREVIEW_LINES with a
  // "show N more lines" toggle when longer.
  function collapsibleCode(text, lang) {
    const fullHtml = highlight(text, lang);
    const lines = text.split('\n');
    if (lines.length <= PREVIEW_LINES) {
      return '<pre class="code"><code class="hljs">' + fullHtml + '</code></pre>';
    }
    const more = lines.length - PREVIEW_LINES;
    const label = 'show ' + more + ' more lines';
    const previewHtml = highlight(lines.slice(0, PREVIEW_LINES).join('\n'), lang);
    return '<div class="collapsible">' +
      '<pre class="code col-preview"><code class="hljs">' + previewHtml + '</code></pre>' +
      '<pre class="code col-full"><code class="hljs">' + fullHtml + '</code></pre>' +
      '<button class="show-more" data-label="' + escapeHtml(label) + '">' + escapeHtml(label) + '</button>' +
      '</div>';
  }

  // Compact line diff: trim common head/tail, show the changed middle as
  // removed/added lines with a little surrounding context.
  function renderDiff(before, after) {
    const a = before.split('\n');
    const b = after.split('\n');
    let head = 0;
    while (head < a.length && head < b.length && a[head] === b[head]) head++;
    let tail = 0;
    while (tail < a.length - head && tail < b.length - head &&
      a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail++;

    const ctx = 2;
    const rows = [];
    for (let i = Math.max(0, head - ctx); i < head; i++) rows.push(['ctx', a[i]]);
    for (let i = head; i < a.length - tail; i++) rows.push(['del', a[i]]);
    for (let i = head; i < b.length - tail; i++) rows.push(['add', b[i]]);
    const tailStart = a.length - tail;
    for (let i = tailStart; i < Math.min(a.length, tailStart + ctx); i++) rows.push(['ctx', a[i]]);

    const line = ([kind, text]) => {
      const mark = kind === 'del' ? '-' : kind === 'add' ? '+' : ' ';
      return '<div class="diff-' + kind + '">' + escapeHtml(mark + ' ' + (text || '')) + '</div>';
    };

    if (rows.length <= PREVIEW_LINES) {
      return '<div class="tool-diff">' + rows.map(line).join('') + '</div>';
    }
    const more = rows.length - PREVIEW_LINES;
    const label = 'show ' + more + ' more lines';
    return '<div class="collapsible">' +
      '<div class="tool-diff col-preview">' + rows.slice(0, PREVIEW_LINES).map(line).join('') + '</div>' +
      '<div class="tool-diff col-full">' + rows.map(line).join('') + '</div>' +
      '<button class="show-more" data-label="' + escapeHtml(label) + '">' + escapeHtml(label) + '</button>' +
      '</div>';
  }

  function renderToolResult(response) {
    const result = response.toolResult;
    if (!result) return '';
    if (result.status === 'error') {
      return '<div class="tool-error">' + escapeHtml(result.error || 'Error') + '</div>';
    }
    const value = result.value || {};
    const content = value.content || value;
    return renderResultContent(content, value.isError === true ? 'tool-error' : 'tool-output');
  }

  function toolResultIsError(result) {
    return !!(result && (result.status === 'error' ||
      (result.value && result.value.isError === true)));
  }

  // ============================================================
  // MESSAGE RENDERING
  // ============================================================

  // Collect all tool responses keyed by tool call id.
  let responsesById = new Map();

  function collectToolResponses(messages) {
    const responses = new Map();
    for (const msg of messages) {
      if (!Array.isArray(msg.content)) continue;
      for (const block of msg.content) {
        if (block && block.type === 'toolResponse' && block.id) {
          responses.set(block.id, block);
        }
      }
    }
    return responses;
  }

  function isInternal(msg) {
    return msg.metadata && msg.metadata.userVisible === false;
  }

  // A message is skippable when it only carries tool responses (rendered inline
  // with their originating tool request).
  function isOnlyToolResponses(msg) {
    if (!Array.isArray(msg.content) || msg.content.length === 0) return false;
    return msg.content.every((b) => b && b.type === 'toolResponse');
  }

  function renderContentBlock(block) {
    switch (block.type) {
      case 'text':
        return '<div class="markdown-content">' + renderMarkdown(block.text) + '</div>';
      case 'thinking': {
        const text = block.thinking || '';
        const firstLine = text.split('\n')[0].slice(0, 120);
        return '<div class="thinking-block" onclick="this.classList.toggle(\'expanded\')">' +
          '<span class="thinking-label">thinking</span> ' +
          '<span class="thinking-collapsed">' + escapeHtml(firstLine) + '…</span>' +
          '<div class="thinking-content">' + escapeHtml(text) + '</div></div>';
      }
      case 'redactedThinking':
        return '<div class="thinking-block"><span class="thinking-label">thinking</span> ' +
          '<span class="thinking-collapsed">[redacted]</span></div>';
      case 'toolRequest': {
        const call = block.toolCall || {};
        let status = 'pending';
        let inner = '';
        if (call.status === 'error') {
          status = 'error';
          inner = '<div class="tool-header"><span class="tool-name">tool</span></div>' +
            '<div class="tool-error">' + escapeHtml(call.error || 'Error') + '</div>';
        } else {
          const value = call.value || {};
          const tool = toolNameParts(value.name).tool;
          inner = renderToolHeader(value.name, value.arguments);
          const response = responsesById.get(block.id);
          if (response) {
            const result = response.toolResult;
            status = toolResultIsError(result) ? 'error' : 'success';
            // write/edit/todo_write already show their content/diff; the
            // success confirmation text is redundant, so suppress it.
            const suppress = status === 'success' &&
              (tool === 'write' || tool === 'edit' || tool === 'todo_write');
            if (!suppress) inner += renderToolResult(response);
          }
        }
        return '<div class="tool-execution ' + status + '">' + inner + '</div>';
      }
      case 'toolResponse':
        return '';
      case 'image':
        return '<img class="message-image" src="data:' + escapeHtml(block.mimeType || 'image/png') +
          ';base64,' + escapeHtml(block.data || '') + '" />';
      case 'document':
        return '<div class="document-block">[Document: ' +
          escapeHtml(block.name || block.mimeType || 'attachment') + ']</div>';
      case 'error':
        return '<div class="error-text">' + escapeHtml(block.message || 'Error') + '</div>';
      case 'systemNotification':
        return '<div class="system-notification">' + escapeHtml(block.msg || '') + '</div>';
      default:
        return '';
    }
  }

  function renderMessage(msg, id) {
    const blocks = Array.isArray(msg.content) ? msg.content : [];
    const bodyParts = blocks.map(renderContentBlock).filter((h) => h);
    if (bodyParts.length === 0) return '';

    const role = msg.role === 'user' ? 'user' : 'assistant';
    const internalClass = isInternal(msg) ? ' internal' : '';
    const timestamp = formatTimestamp(msg.created);
    const tsHtml = timestamp ? '<div class="message-timestamp">' + escapeHtml(timestamp) + '</div>' : '';

    if (role === 'user') {
      return '<div class="user-message' + internalClass + '" id="' + id + '">' +
        '<div class="role-label user">you</div>' + tsHtml + bodyParts.join('') + '</div>';
    }
    return '<div class="assistant-message" id="' + id + '">' +
      '<div class="role-label assistant">goose</div>' + tsHtml +
      bodyParts.join('') + '</div>';
  }

  // Condensed one-line summary of a message for the table of contents.
  function tocLabel(msg) {
    const blocks = Array.isArray(msg.content) ? msg.content : [];
    const text = blocks.find((b) => b && b.type === 'text' && (b.text || '').trim());
    if (text) return text.text.trim().split('\n').find((l) => l.trim()) || '';

    const call = blocks.find((b) => b && b.type === 'toolRequest');
    if (call) {
      const value = (call.toolCall && call.toolCall.value) || {};
      const parts = toolNameParts(value.name);
      const detail = firstString(value.arguments, ['command', 'cmd', 'path', 'file_path', 'filePath', 'file']);
      return detail ? parts.tool + ': ' + shortenPath(detail) : parts.tool;
    }
    if (blocks.some((b) => b && (b.type === 'thinking' || b.type === 'redactedThinking'))) return 'thinking';
    if (blocks.some((b) => b && b.type === 'image')) return 'image';
    if (blocks.some((b) => b && b.type === 'error')) return 'error';
    return blocks.length ? blocks[0].type : '';
  }

  function truncate(text, max) {
    if (text.length <= max) return text;
    return text.slice(0, max - 1).trimEnd() + '\u2026';
  }

  function renderToc(entries) {
    const list = document.getElementById('toc-list');
    if (entries.length === 0) {
      list.innerHTML = '';
      return;
    }
    list.innerHTML = entries
      .map((e) => {
        const role = e.role === 'user' ? 'user' : 'assistant';
        return '<a class="toc-item ' + role + (e.internal ? ' internal' : '') +
          '" href="#' + e.id + '" data-target="' + e.id + '">' +
          '<span class="toc-role">' + (role === 'user' ? 'you' : 'goose') + '</span>' +
          '<span class="toc-text">' + escapeHtml(truncate(e.label, 52)) + '</span></a>';
      })
      .join('');

    for (const link of list.querySelectorAll('.toc-item')) {
      link.addEventListener('click', (ev) => {
        ev.preventDefault();
        const target = document.getElementById(link.dataset.target);
        if (target) target.scrollIntoView({ behavior: 'smooth', block: 'start' });
        closeTocOnMobile();
      });
    }
    observeTurns(entries);
  }

  let turnObserver = null;
  function observeTurns(entries) {
    if (turnObserver) turnObserver.disconnect();
    if (!('IntersectionObserver' in window)) return;
    const visible = new Set();
    turnObserver = new IntersectionObserver(
      (records) => {
        for (const r of records) {
          if (r.isIntersecting) visible.add(r.target.id);
          else visible.delete(r.target.id);
        }
        const active = entries.find((e) => visible.has(e.id)) || entries[0];
        for (const link of document.querySelectorAll('.toc-item')) {
          link.classList.toggle('active', link.dataset.target === (active && active.id));
        }
      },
      { rootMargin: '0px 0px -70% 0px' },
    );
    for (const e of entries) {
      const el = document.getElementById(e.id);
      if (el) turnObserver.observe(el);
    }
  }

  function renderMessages() {
    const container = document.getElementById('messages');
    const parts = [];
    const tocEntries = [];
    let index = 0;
    const messages = showInternal
      ? allMessages
      : allMessages.filter((msg) => !isInternal(msg)).map(userVisibleMessage);
    responsesById = collectToolResponses(messages);
    for (const msg of messages) {
      if (isOnlyToolResponses(msg)) continue;
      if (!showInternal && isInternal(msg)) continue;
      const id = 'turn-' + index;
      const html = renderMessage(msg, id);
      if (html) {
        parts.push(html);
        tocEntries.push({ id, role: msg.role, label: tocLabel(msg), internal: isInternal(msg) });
        index++;
      }
    }
    container.innerHTML = parts.length
      ? parts.join('')
      : '<div class="empty-state">No messages to display.</div>';
    renderToc(tocEntries);
  }

  // ============================================================
  // HEADER
  // ============================================================

  function infoItem(label, value) {
    if (value == null || value === '') return '';
    return '<div class="info-label">' + escapeHtml(label) + '</div>' +
      '<div class="info-value">' + escapeHtml(value) + '</div>';
  }

  function renderHeader() {
    const model = session.model_config && session.model_config.model_name;
    const usage = session.usage || {};
    const tokens = usage.total_tokens != null ? usage.total_tokens.toLocaleString() : null;
    const cost = session.accumulated_cost != null ? '$' + session.accumulated_cost.toFixed(4) : null;

    let info = '';
    info += infoItem('session', session.id);
    info += infoItem('created', session.created_at);
    info += infoItem('updated', session.updated_at);
    info += infoItem('provider', session.provider_name);
    info += infoItem('model', model);
    info += infoItem('messages', session.message_count);
    info += infoItem('tokens', tokens);
    info += infoItem('cost', cost);

    const title = session.name && session.name.length ? session.name : 'Untitled session';
    const dir = session.working_dir
      ? '<div class="header-dir">' + escapeHtml(session.working_dir) + '</div>'
      : '';
    const hasInternal = allMessages.some((msg) =>
      isInternal(msg) || (Array.isArray(msg.content) && msg.content.some(hasAssistantOnlyContent)));
    const toggle = hasInternal
      ? '<div class="header-actions"><button class="toggle-btn" id="toggle-internal">Show internal messages</button></div>'
      : '';

    document.getElementById('header-container').innerHTML =
      '<div class="header"><h1>' + escapeHtml(title) + '</h1>' + dir +
      '<div class="header-info">' + info + '</div>' + toggle + '</div>';

    const btn = document.getElementById('toggle-internal');
    if (btn) {
      btn.addEventListener('click', () => {
        showInternal = !showInternal;
        btn.classList.toggle('active', showInternal);
        btn.textContent = showInternal ? 'Hide internal messages' : 'Show internal messages';
        renderMessages();
      });
    }
  }

  // ============================================================
  // INIT
  // ============================================================

  // ============================================================
  // MOBILE TOC TOGGLE
  // ============================================================

  const toc = document.getElementById('toc');
  const tocToggle = document.getElementById('toc-toggle');
  const tocOverlay = document.getElementById('toc-overlay');

  function closeTocOnMobile() {
    if (window.matchMedia('(max-width: 900px)').matches) {
      toc.classList.remove('open');
      tocOverlay.classList.remove('open');
      tocToggle.setAttribute('aria-expanded', 'false');
    }
  }

  tocToggle.addEventListener('click', () => {
    const open = toc.classList.toggle('open');
    tocOverlay.classList.toggle('open', open);
    tocToggle.setAttribute('aria-expanded', String(open));
  });
  tocOverlay.addEventListener('click', closeTocOnMobile);

  // "show more" expanders (delegated so it survives re-renders).
  document.getElementById('messages').addEventListener('click', (ev) => {
    const btn = ev.target.closest('.show-more');
    if (!btn) return;
    const block = btn.closest('.collapsible');
    if (!block) return;
    const expanded = block.classList.toggle('expanded');
    btn.textContent = expanded ? 'show less' : btn.dataset.label;
  });

  // ============================================================
  // INIT
  // ============================================================

  document.title = 'goose · ' + (session.name || session.id || 'session');
  renderHeader();
  renderMessages();
})();
