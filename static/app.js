(() => {
  const state = {
    authenticated: false,
    sessions: [],
    activeSessionId: null,
    currentPath: '',
    editingFile: null,
    selectedDiffFile: null,
    ws: null,
    term: null,
    fitAddon: null,
    userScrolledUp: false,
  };

  const $ = (id) => document.getElementById(id);

  async function api(url, options = {}) {
    const resp = await fetch(url, {
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json', ...(options.headers || {}) },
      ...options,
    });
    const text = await resp.text();
    let data = {};
    try { data = text ? JSON.parse(text) : {}; } catch (_) {}
    if (!resp.ok) {
      throw new Error(data.error || `HTTP ${resp.status}`);
    }
    return data;
  }

  function syncVisualViewportHeight() {
    const app = $('app-view');
    if (!app || !window.visualViewport) return;
    const vv = window.visualViewport;
    app.style.height = `${vv.height}px`;
    if (state.fitAddon && state.term) {
      try {
        state.fitAddon.fit();
        sendResize();
      } catch (_) {}
    }
  }

  if (window.visualViewport) {
    window.visualViewport.addEventListener('resize', syncVisualViewportHeight);
  }
  window.addEventListener('resize', () => {
    if (state.fitAddon && state.term) {
      state.fitAddon.fit();
      sendResize();
    }
  });

  async function bootstrap() {
    const session = await api('/api/auth/session');
    if (!session.authenticated) {
      $('login-username').value = session.username || 'hqcode';
      $('login-view').classList.remove('hidden');
      $('app-view').classList.add('hidden');
      return;
    }
    $('login-view').classList.add('hidden');
    $('app-view').classList.remove('hidden');
    syncVisualViewportHeight();
    await Promise.all([loadDetectedAgents(), loadSessions(), loadFiles(''), refreshGitStatus()]);
  }

  $('login-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    $('login-error').classList.add('hidden');
    try {
      await api('/api/auth/login', {
        method: 'POST',
        body: JSON.stringify({
          username: $('login-username').value,
          password: $('login-password').value,
        }),
      });
      await bootstrap();
    } catch (err) {
      $('login-error').textContent = err.message;
      $('login-error').classList.remove('hidden');
    }
  });

  $('btn-logout').addEventListener('click', async () => {
    await api('/api/auth/logout', { method: 'POST' });
    location.reload();
  });

  async function loadDetectedAgents() {
    const agents = await api('/api/agents/detected');
    const host = $('detected-agents');
    host.innerHTML = '';
    for (const agent of agents) {
      const btn = document.createElement('button');
      btn.className = 'agent-btn';
      btn.disabled = !agent.available;
      btn.innerHTML = `<span>${agent.label}</span><span>${agent.available ? '▶' : '—'}</span>`;
      btn.addEventListener('click', () => {
        const cmd = agent.id === 'shell' ? '' : agent.command;
        createSession(agent.label, cmd);
      });
      host.appendChild(btn);
    }
  }

  async function loadSessions() {
    const data = await api('/api/terminal/sessions');
    state.sessions = data.sessions || [];
    renderSessions();
    if (!state.activeSessionId && state.sessions.length > 0) {
      selectSession(state.sessions[0].id);
    }
  }

  function renderSessions() {
    const list = $('session-list');
    list.innerHTML = '';
    for (const s of state.sessions) {
      const row = document.createElement('div');
      row.className = `session-item${s.id === state.activeSessionId ? ' active' : ''}`;
      const title = document.createElement('span');
      title.className = 'session-title';
      title.textContent = `${s.alive ? '●' : '○'} ${s.name}`;
      title.addEventListener('click', () => {
        selectSession(s.id);
        $('sidebar').classList.remove('open');
      });

      const renameBtn = document.createElement('button');
      renameBtn.className = 'mini-btn';
      renameBtn.textContent = '✎';
      renameBtn.title = 'Rename';
      renameBtn.addEventListener('click', async (ev) => {
        ev.stopPropagation();
        const next = prompt('Rename session:', s.name);
        if (next && next.trim()) {
          await api(`/api/terminal/sessions/${s.id}`, {
            method: 'PUT',
            body: JSON.stringify({ name: next.trim() }),
          });
          await loadSessions();
        }
      });

      const closeBtn = document.createElement('button');
      closeBtn.className = 'mini-btn';
      closeBtn.textContent = '✕';
      closeBtn.title = 'Close session';
      closeBtn.addEventListener('click', async (ev) => {
        ev.stopPropagation();
        await api(`/api/terminal/sessions/${s.id}`, { method: 'DELETE' });
        if (state.activeSessionId === s.id) {
          state.activeSessionId = null;
          disconnectTerminal();
        }
        await loadSessions();
      });

      row.append(title, renameBtn, closeBtn);
      list.appendChild(row);
    }
  }

  async function createSession(label, startupCommand) {
    const res = await api('/api/terminal/sessions', {
      method: 'POST',
      body: JSON.stringify({
        name: label ? `${label} ${state.sessions.length + 1}` : null,
        startup_command: startupCommand || null,
      }),
    });
    await loadSessions();
    if (res.session) {
      selectSession(res.session.id);
      $('sidebar').classList.remove('open');
    }
  }

  $('btn-new-shell').addEventListener('click', () => createSession('Shell', ''));

  function ensureTerm() {
    if (state.term) return;
    const term = new window.Terminal({
      cursorBlink: true,
      scrollback: 5000,
      fontSize: 13,
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace',
      theme: {
        background: '#080a0c',
        foreground: '#e6edf3',
        cursor: '#58a6ff',
      },
    });
    const fitAddon = new window.FitAddon.FitAddon();
    term.loadAddon(fitAddon);
    term.open($('xterm-host'));
    fitAddon.fit();

    term.onScroll(() => {
      const buf = term.buffer.active;
      const atBottom = buf.viewportY >= buf.baseY - 2;
      state.userScrolledUp = !atBottom;
    });

    term.onData((data) => {
      if (state.ws && state.ws.readyState === WebSocket.OPEN) {
        state.ws.send(JSON.stringify({ type: 'input', data }));
      }
    });

    state.term = term;
    state.fitAddon = fitAddon;
  }

  function disconnectTerminal() {
    if (state.ws) {
      state.ws.close();
      state.ws = null;
    }
    $('xterm-host').classList.add('hidden');
    $('empty-state').classList.remove('hidden');
  }

  function sendResize() {
    if (state.ws && state.ws.readyState === WebSocket.OPEN && state.term) {
      state.ws.send(JSON.stringify({
        type: 'resize',
        cols: state.term.cols,
        rows: state.term.rows,
      }));
    }
  }

  function selectSession(sessionId) {
    state.activeSessionId = sessionId;
    renderSessions();
    $('empty-state').classList.add('hidden');
    $('xterm-host').classList.remove('hidden');
    ensureTerm();

    if (state.ws) {
      state.ws.close();
      state.ws = null;
    }
    state.term.reset();
    state.userScrolledUp = false;
    state.fitAddon.fit();

    const proto = location.protocol === 'https:' ? 'wss' : 'ws';
    const ws = new WebSocket(`${proto}://${location.host}/ws/terminal/${sessionId}`);
    ws.binaryType = 'arraybuffer';

    ws.onopen = () => sendResize();
    ws.onmessage = (event) => {
      if (event.data instanceof ArrayBuffer) {
        const bytes = new Uint8Array(event.data);
        const keepPos = state.userScrolledUp;
        const savedY = state.term.buffer.active.viewportY;
        state.term.write(bytes, () => {
          if (keepPos) {
            state.term.scrollToLine(savedY);
          }
        });
      }
    };
    ws.onclose = () => {
      if (state.activeSessionId === sessionId) {
        setTimeout(() => {
          if (state.activeSessionId === sessionId && (!state.ws || state.ws.readyState === WebSocket.CLOSED)) {
            selectSession(sessionId);
          }
        }, 1500);
      }
    };

    state.ws = ws;
    state.term.focus();
  }

  async function loadFiles(path) {
    state.currentPath = path;
    const data = await api(`/api/entries?path=${encodeURIComponent(path)}`);
    $('workspace-path-pill').textContent = data.display_path;
    const bc = $('file-breadcrumbs');
    bc.textContent = data.parent_path !== null ? '← .. (Up)' : '/';
    bc.onclick = () => {
      if (data.parent_path !== null) loadFiles(data.parent_path);
    };

    const list = $('file-list');
    list.innerHTML = '';
    for (const entry of data.entries) {
      const row = document.createElement('div');
      row.className = 'file-item';
      row.innerHTML = `<span class="file-name">${entry.kind === 'dir' ? '📁' : '📄'} ${entry.name}</span>`;
      row.addEventListener('click', () => {
        if (entry.kind === 'dir') {
          loadFiles(entry.path);
        } else {
          openFile(entry.path);
        }
      });
      list.appendChild(row);
    }
  }

  $('btn-refresh-files').addEventListener('click', () => loadFiles(state.currentPath));

  async function openFile(path) {
    const file = await api(`/api/file?path=${encodeURIComponent(path)}`);
    state.editingFile = file.path;
    $('editor-file-path').textContent = file.display_path;
    $('editor-textarea').value = file.editable ? file.content : (file.message || 'Read-only');
    $('editor-textarea').readOnly = !file.editable;
    $('btn-save-file').disabled = !file.editable;
    $('editor-overlay').classList.remove('hidden');
  }

  $('btn-save-file').addEventListener('click', async () => {
    if (!state.editingFile) return;
    await api('/api/file', {
      method: 'PUT',
      body: JSON.stringify({
        path: state.editingFile,
        content: $('editor-textarea').value,
      }),
    });
    $('editor-overlay').classList.add('hidden');
    refreshGitStatus();
  });

  $('btn-close-editor').addEventListener('click', () => $('editor-overlay').classList.add('hidden'));

  async function refreshGitStatus() {
    try {
      const status = await api('/api/git/status');
      const badge = $('diff-count-badge');
      if (!status.is_repo || status.files.length === 0) {
        badge.classList.add('hidden');
      } else {
        badge.textContent = String(status.files.length);
        badge.classList.remove('hidden');
      }
      $('diff-branch-pill').textContent = status.is_repo ? status.branch : 'no-git';
      renderDiffFileList(status.files || []);
    } catch (_) {}
  }

  function renderDiffFileList(files) {
    const list = $('diff-file-list');
    list.innerHTML = '';
    if (files.length === 0) {
      list.innerHTML = '<div style="color:#8b949e;padding:6px;">Working tree clean.</div>';
      $('diff-viewer').textContent = '';
      return;
    }
    for (const f of files) {
      const row = document.createElement('div');
      row.className = `diff-file-row${state.selectedDiffFile === f.path ? ' active' : ''}`;
      const label = document.createElement('span');
      label.className = 'file-name mono';
      label.textContent = `[${f.status}] ${f.path}`;
      label.addEventListener('click', () => loadDiff(f.path));

      const revertBtn = document.createElement('button');
      revertBtn.className = 'mini-btn';
      revertBtn.textContent = 'Revert';
      revertBtn.addEventListener('click', async (ev) => {
        ev.stopPropagation();
        if (!confirm(`Revert ${f.path}?`)) return;
        await api('/api/git/revert', {
          method: 'POST',
          body: JSON.stringify({ file: f.path }),
        });
        await refreshGitStatus();
        await loadDiff(null);
      });

      row.append(label, revertBtn);
      list.appendChild(row);
    }
  }

  async function loadDiff(file) {
    state.selectedDiffFile = file;
    const q = file ? `?file=${encodeURIComponent(file)}` : '';
    const data = await api(`/api/git/diff${q}`);
    renderColorDiff(data.diff || '');
  }

  function renderColorDiff(diffText) {
    const viewer = $('diff-viewer');
    viewer.innerHTML = '';
    if (!diffText.trim()) {
      viewer.textContent = 'No diff content.';
      return;
    }
    for (const line of diffText.split('\n')) {
      const span = document.createElement('span');
      if (line.startsWith('+') && !line.startsWith('+++')) {
        span.className = 'diff-line-add';
      } else if (line.startsWith('-') && !line.startsWith('---')) {
        span.className = 'diff-line-del';
      } else if (line.startsWith('@@')) {
        span.className = 'diff-line-hunk';
      } else if (line.startsWith('diff ') || line.startsWith('index ') || line.startsWith('---') || line.startsWith('+++')) {
        span.className = 'diff-line-meta';
      }
      span.textContent = line || ' ';
      viewer.appendChild(span);
    }
  }

  $('btn-open-diff').addEventListener('click', async () => {
    $('diff-drawer').classList.toggle('hidden');
    if (!$('diff-drawer').classList.contains('hidden')) {
      await refreshGitStatus();
      await loadDiff(state.selectedDiffFile);
    }
  });
  $('btn-close-diff').addEventListener('click', () => $('diff-drawer').classList.add('hidden'));
  $('btn-refresh-diff').addEventListener('click', async () => {
    await refreshGitStatus();
    await loadDiff(state.selectedDiffFile);
  });

  $('btn-git-commit').addEventListener('click', async () => {
    const message = $('commit-message-input').value.trim();
    if (!message) return;
    await api('/api/git/commit', {
      method: 'POST',
      body: JSON.stringify({ message }),
    });
    $('commit-message-input').value = '';
    await refreshGitStatus();
    await loadDiff(null);
  });

  $('btn-toggle-sidebar').addEventListener('click', () => {
    $('sidebar').classList.toggle('open');
  });

  // Mobile soft keys
  document.querySelectorAll('#hq-mobile-keys .mkey').forEach((btn) => {
    btn.addEventListener('click', async () => {
      if (btn.dataset.action === 'diff') {
        $('btn-open-diff').click();
        return;
      }
      if (!state.activeSessionId) return;
      const map = {
        esc: '\x1b',
        tab: '\t',
        ctrlc: '\x03',
        up: '\x1b[A',
        down: '\x1b[B',
        approve: 'y\r',
      };
      if (btn.dataset.key === 'enter') {
        await api(`/api/terminal/sessions/${state.activeSessionId}/enter`, { method: 'POST' });
        return;
      }
      const seq = map[btn.dataset.key];
      if (seq && state.ws && state.ws.readyState === WebSocket.OPEN) {
        state.ws.send(JSON.stringify({ type: 'input', data: seq }));
      }
    });
  });

  $('btn-mobile-send').addEventListener('click', async () => {
    const val = $('mobile-quick-input').value;
    if (!val || !state.activeSessionId) return;
    await api(`/api/terminal/sessions/${state.activeSessionId}/input`, {
      method: 'POST',
      body: JSON.stringify({ data: `${val}\r` }),
    });
    $('mobile-quick-input').value = '';
  });

  // Presets & Password modal
  async function loadPresets() {
    const data = await api('/api/presets');
    const host = $('preset-list');
    host.innerHTML = '';
    for (const p of data.presets || []) {
      const row = document.createElement('div');
      row.className = 'preset-row';
      row.innerHTML = `<span><strong>${p.name}</strong> <span class="mono" style="color:#8b949e">(${p.kind} · ${p.model})</span></span>`;
      const actions = document.createElement('div');
      actions.style.display = 'flex';
      actions.style.gap = '6px';
      const applyBtn = document.createElement('button');
      applyBtn.className = 'mini-btn';
      applyBtn.textContent = 'Apply';
      applyBtn.onclick = async () => {
        await api(`/api/presets/${p.id}/apply`, { method: 'POST' });
        alert(`Applied preset ${p.name}`);
      };
      const delBtn = document.createElement('button');
      delBtn.className = 'mini-btn';
      delBtn.textContent = '✕';
      delBtn.onclick = async () => {
        await api(`/api/presets/${p.id}`, { method: 'DELETE' });
        await loadPresets();
      };
      actions.append(applyBtn, delBtn);
      row.appendChild(actions);
      host.appendChild(row);
    }
  }

  $('btn-open-settings').addEventListener('click', async () => {
    $('settings-modal').classList.remove('hidden');
    await loadPresets();
  });
  $('btn-close-settings').addEventListener('click', () => $('settings-modal').classList.add('hidden'));

  $('preset-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    await api('/api/presets', {
      method: 'POST',
      body: JSON.stringify({
        id: '',
        name: $('preset-name').value,
        kind: $('preset-kind').value,
        model: $('preset-model').value,
        base_url: $('preset-base-url').value,
        api_key: $('preset-api-key').value,
        wire_api: 'responses',
      }),
    });
    e.target.reset();
    await loadPresets();
  });

  $('password-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    await api('/api/auth/password', {
      method: 'POST',
      body: JSON.stringify({
        current_password: $('pw-current').value,
        new_password: $('pw-new').value,
      }),
    });
    e.target.reset();
    alert('Password updated');
  });

  bootstrap();
})();
